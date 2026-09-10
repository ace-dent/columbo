// SPDX-License-Identifier: MIT

//! PNG/APNG chunk parsing, validation, and encoding.

use crate::checksum::crc32_update;
use crate::{Error, Result};

pub(super) const SIGNATURE: &[u8; 8] = b"\x89PNG\r\n\x1a\n";
/// Every APNG frame owns and validates an independent zlib stream. Bound that
/// invocation count separately from the generic chunk count because an empty
/// frame consumes almost no decoded-byte budget.
const MAX_APNG_FRAMES: usize = 16_384;
/// Compressed ancillary chunks are each decoded and checksum-validated even
/// when the optional search deadline is exhausted. Keep zero-length metadata
/// streams from multiplying that mandatory parser setup indefinitely.
const MAX_COMPRESSED_METADATA_STREAMS: usize = 4_096;
/// Twelve-byte empty chunks otherwise amplify into several independent Rust
/// model records. This remains far beyond practical PNG/APNG use while
/// keeping parser bookkeeping and mandatory CRC work comfortably bounded.
const MAX_PNG_CHUNKS: usize = 65_536;

/// A validated chunk borrowing both its payload and original encoding.
#[derive(Clone, Copy)]
pub(super) struct Chunk<'a> {
    pub(super) kind: [u8; 4],
    pub(super) data: &'a [u8],
    pub(super) encoded: &'a [u8],
    pub(super) discard_on_output: bool,
}

/// Validated container structure and the collected independent image streams.
///
/// Only the PNG rewriter accesses the fields; format dispatch passes the model
/// from preflight to optimization without parsing the container again.
pub(in crate::format) struct ParsedPng<'a> {
    pub(super) chunks: Vec<Chunk<'a>>,
    pub(super) datastream_len: usize,
    pub(super) idat: Vec<u8>,
    pub(super) idat_decoded_size: u64,
    pub(super) fdat_frames: Vec<Vec<u8>>,
    pub(super) fdat_decoded_sizes: Vec<u64>,
    pub(super) has_rewrite_sensitive_ancillary: bool,
    pub(super) has_vestigial_rgba_trns: bool,
}

#[derive(Default)]
struct ParseState {
    saw_ihdr: bool,
    width: u32,
    height: u32,
    bit_depth: u8,
    color_type: u8,
    interlace_method: u8,
    saw_plte: bool,
    palette_entries: u32,
    saw_idat: bool,
    saw_iend: bool,
    after_idat_run: bool,

    saw_actl: bool,
    animation_frames: u32,
    fctl_count: u32,
    sequence_expected: u32,
    frame_open: bool,
    frame_has_data: bool,
    current_fdat_decoded_size: Option<u64>,

    saw_chrm: bool,
    saw_gama: bool,
    saw_iccp: bool,
    saw_sbit: bool,
    saw_srgb: bool,
    saw_cicp: bool,
    saw_mdcv: bool,
    saw_clli: bool,
    saw_trns: bool,
    saw_bkgd: bool,
    saw_hist: bool,
    saw_phys: bool,
    saw_scal: bool,
    saw_exif: bool,
    saw_time: bool,
    saw_cabx: bool,
}

/// Validate chunk structure and CRCs before collecting image stream payloads.
/// The zlib and Deflate layers validate compressed data during optimization.
pub(super) fn parse(input: &[u8], strip_metadata: bool) -> Result<ParsedPng<'_>> {
    if !input.starts_with(SIGNATURE) {
        return Err(Error::new("invalid PNG signature"));
    }

    let mut chunks = Vec::new();
    let mut idat = Vec::new();
    let mut fdat = Vec::new();
    let mut fdat_frames = Vec::new();
    let mut fdat_decoded_sizes = Vec::new();
    let mut state = ParseState::default();
    let mut has_rewrite_sensitive_ancillary = false;
    let mut has_vestigial_rgba_trns = false;
    let mut compressed_metadata_streams = 0_usize;
    let mut position = SIGNATURE.len();

    while input.len().saturating_sub(position) >= 12 {
        if chunks.len() >= MAX_PNG_CHUNKS {
            return Err(Error::resource_limit("PNG contains too many chunks"));
        }
        let length = be32(input, position)?;
        if length > 0x7fff_ffff {
            return Err(Error::new("invalid PNG chunk length"));
        }
        let length = length as usize;
        if length > input.len() - position - 12 {
            return Err(Error::new("truncated PNG chunk"));
        }

        let kind: [u8; 4] = input[position + 4..position + 8].try_into().unwrap();
        let data = &input[position + 8..position + 8 + length];
        let after = position + 12 + length;
        let stored_crc =
            u32::from_be_bytes(input[position + 8 + length..after].try_into().unwrap());

        if !valid_chunk_type(kind) {
            return Err(Error::new("invalid PNG chunk type"));
        }
        let calculated_crc = crc32_update(crc32_update(0, &kind), data);
        if calculated_crc != stored_crc {
            return Err(Error::integrity_mismatch("bad PNG chunk CRC"));
        }

        if position == SIGNATURE.len() {
            validate_ihdr(kind, data, &mut state)?;
        } else if kind == *b"IHDR" {
            return Err(Error::new("invalid PNG IHDR"));
        }
        if kind == *b"IEND" && !data.is_empty() {
            return Err(Error::new("invalid PNG IEND"));
        }
        if kind[0] & 0x20 == 0 && !is_known_critical(kind) {
            return Err(Error::new("unknown PNG critical chunk"));
        }
        let strip_chunk = should_strip_kind(kind, strip_metadata);
        if is_rewrite_sensitive_ancillary(kind) {
            has_rewrite_sensitive_ancillary = true;
        }

        validate_palette(kind, data, &mut state)?;
        // Metadata whose semantics or placement are invalid is useful to no
        // output that explicitly strips it. Still validate chunk boundaries,
        // type bytes, and CRC above: --strip is not a general PNG repair mode.
        let discard_on_output = if strip_chunk {
            false
        } else {
            validate_ancillary(kind, data, &mut state)?
        };
        has_vestigial_rgba_trns |= discard_on_output;
        // fcTL begins the next fdAT zlib stream; IEND closes the final one.
        if matches!(&kind, b"fcTL" | b"IEND") && !fdat.is_empty() {
            fdat_frames
                .try_reserve(1)
                .map_err(|_| Error::internal("could not allocate PNG frame model"))?;
            fdat_decoded_sizes
                .try_reserve(1)
                .map_err(|_| Error::internal("could not allocate PNG frame model"))?;
            fdat_frames.push(std::mem::take(&mut fdat));
            fdat_decoded_sizes.push(
                state
                    .current_fdat_decoded_size
                    .take()
                    .ok_or_else(|| Error::new("invalid APNG frame data"))?,
            );
        }

        validate_animation_control(kind, data, &mut state)?;
        if !strip_chunk && compressed_zlib_offset(kind, data).is_some() {
            compressed_metadata_streams += 1;
            if compressed_metadata_streams > MAX_COMPRESSED_METADATA_STREAMS {
                return Err(Error::resource_limit(
                    "PNG contains too many compressed metadata streams",
                ));
            }
        }

        if kind == *b"IDAT" {
            if state.after_idat_run {
                return Err(Error::new("non-consecutive IDAT chunk"));
            }
            if !state.saw_idat && state.color_type == 3 && !state.saw_plte {
                return Err(Error::new("missing PNG PLTE"));
            }
            state.saw_idat = true;
            idat.try_reserve(data.len())
                .map_err(|_| Error::internal("could not allocate PNG image stream"))?;
            idat.extend_from_slice(data);
        } else if kind == *b"fdAT" {
            if !state.saw_actl
                || !state.saw_idat
                || !state.frame_open
                || data.len() < 4
                || u32::from_be_bytes(data[..4].try_into().unwrap()) != state.sequence_expected
            {
                return Err(Error::new("bad APNG fdAT chunk"));
            }
            state.sequence_expected += 1;
            // A sequence-number-only chunk may occur between real fdAT
            // packets, but it cannot by itself satisfy the frame-data
            // requirement.
            state.frame_has_data |= data.len() > 4;
            fdat.try_reserve(data.len() - 4)
                .map_err(|_| Error::internal("could not allocate PNG frame stream"))?;
            fdat.extend_from_slice(&data[4..]);
        } else if state.saw_idat && !strip_chunk {
            state.after_idat_run = true;
        }

        chunks
            .try_reserve(1)
            .map_err(|_| Error::internal("could not allocate PNG chunk model"))?;
        chunks.push(Chunk {
            kind,
            data,
            encoded: &input[position..after],
            discard_on_output,
        });
        position = after;
        if kind == *b"IEND" {
            state.saw_iend = true;
            // IEND terminates the PNG datastream. Tolerate an enclosing file's
            // suffix on input, but leave it outside the chunk model so every
            // reconstructed output discards it.
            break;
        }
    }

    if state.saw_actl
        && (state.fctl_count != state.animation_frames
            || (state.frame_open && !state.frame_has_data))
    {
        return Err(Error::new("invalid APNG frame count"));
    }
    if !state.saw_ihdr || !state.saw_iend {
        return Err(Error::new("invalid PNG trailer"));
    }
    if !state.saw_idat {
        return Err(Error::new("no IDAT chunk found"));
    }
    if has_vestigial_rgba_trns && has_rewrite_sensitive_ancillary && !strip_metadata {
        return Err(Error::new(
            "cannot remove invalid PNG tRNS while preserving rewrite-sensitive metadata",
        ));
    }
    if idat.len() < 6 {
        return Err(Error::new("IDAT zlib stream too small"));
    }
    if (idat[0] & 0x0f) != 8 || (idat[0] >> 4) > 7 || idat[1] & 0x20 != 0 {
        return Err(Error::unsupported_feature("unsupported PNG zlib header"));
    }
    if ((u16::from(idat[0]) << 8) | u16::from(idat[1])) % 31 != 0 {
        return Err(Error::new("invalid PNG zlib header check"));
    }

    let idat_decoded_size = png_image_decoded_size(&state)?;
    Ok(ParsedPng {
        chunks,
        datastream_len: position,
        idat,
        idat_decoded_size,
        fdat_frames,
        fdat_decoded_sizes,
        has_rewrite_sensitive_ancillary,
        has_vestigial_rgba_trns,
    })
}

fn validate_ihdr(kind: [u8; 4], data: &[u8], state: &mut ParseState) -> Result<()> {
    if kind != *b"IHDR" || data.len() != 13 {
        return Err(Error::new("invalid PNG IHDR"));
    }
    state.width = u32::from_be_bytes(data[..4].try_into().unwrap());
    state.height = u32::from_be_bytes(data[4..8].try_into().unwrap());
    state.bit_depth = data[8];
    state.color_type = data[9];
    state.interlace_method = data[12];
    if state.width == 0
        || state.height == 0
        || !valid_bit_depth(state.color_type, state.bit_depth)
        || data[10] != 0
        || data[11] != 0
        || data[12] > 1
    {
        return Err(Error::new("invalid PNG IHDR"));
    }
    state.saw_ihdr = true;
    Ok(())
}

/// Return the exact number of filtered scanline bytes carried by IDAT.
///
/// This is also a security boundary for parallel Max: each branch receives
/// this value as its decode ceiling, so a small malicious zlib stream cannot
/// make two workers retain unexpectedly large payload models.
fn png_image_decoded_size(state: &ParseState) -> Result<u64> {
    png_decoded_size(
        state.width,
        state.height,
        state.bit_depth,
        state.color_type,
        state.interlace_method,
    )
}

fn png_decoded_size(
    width: u32,
    height: u32,
    bit_depth: u8,
    color_type: u8,
    interlace_method: u8,
) -> Result<u64> {
    let samples_per_pixel = match color_type {
        0 | 3 => 1_u64,
        2 => 3,
        4 => 2,
        6 => 4,
        _ => return Err(Error::new("invalid PNG IHDR")),
    };
    let bits_per_pixel = samples_per_pixel * u64::from(bit_depth);
    let pass_size = |width: u64, height: u64| -> Option<u64> {
        if width == 0 || height == 0 {
            return Some(0);
        }
        let row_bits = width.checked_mul(bits_per_pixel)?;
        let row_bytes = row_bits.checked_add(7)? / 8;
        height.checked_mul(row_bytes.checked_add(1)?)
    };

    let width = u64::from(width);
    let height = u64::from(height);
    if interlace_method == 0 {
        return pass_size(width, height)
            .ok_or_else(|| Error::resource_limit("PNG image dimensions are too large"));
    }

    // Adam7 pass geometry: (starting x, starting y, x step, y step).
    const ADAM7: [(u64, u64, u64, u64); 7] = [
        (0, 0, 8, 8),
        (4, 0, 8, 8),
        (0, 4, 4, 8),
        (2, 0, 4, 4),
        (0, 2, 2, 4),
        (1, 0, 2, 2),
        (0, 1, 1, 2),
    ];
    ADAM7
        .iter()
        .try_fold(0_u64, |total, &(start_x, start_y, step_x, step_y)| {
            let pass_width = width
                .checked_sub(start_x)
                .map_or(0, |remaining| remaining.div_ceil(step_x));
            let pass_height = height
                .checked_sub(start_y)
                .map_or(0, |remaining| remaining.div_ceil(step_y));
            total.checked_add(pass_size(pass_width, pass_height)?)
        })
        .ok_or_else(|| Error::resource_limit("PNG image dimensions are too large"))
}

fn validate_palette(kind: [u8; 4], data: &[u8], state: &mut ParseState) -> Result<()> {
    if kind != *b"PLTE" {
        return Ok(());
    }
    if state.saw_plte
        || state.saw_idat
        || data.is_empty()
        || data.len() > 768
        || data.len() % 3 != 0
        || matches!(state.color_type, 0 | 4)
    {
        return Err(Error::new("invalid PNG PLTE"));
    }
    state.palette_entries = (data.len() / 3) as u32;
    if state.color_type == 3 && state.palette_entries > (1_u32 << state.bit_depth) {
        return Err(Error::new("invalid PNG PLTE"));
    }
    state.saw_plte = true;
    Ok(())
}

/// Validate ancillary semantics and report whether a known-invalid chunk must
/// be omitted from every successful output.
///
/// Some exporters leave an indexed palette's tRNS behind after converting the
/// image data and IHDR to RGBA. PLTE itself is a valid suggested palette for
/// color type 6, but tRNS is forbidden because RGBA already carries alpha.
/// Tolerate only that narrow, structurally consistent signature. The chunk is
/// never allowed to influence pixel interpretation and is never preserved.
fn validate_ancillary(kind: [u8; 4], data: &[u8], state: &mut ParseState) -> Result<bool> {
    let mut discard_on_output = false;
    macro_rules! once_before_image {
        ($kind:literal, $seen:ident, $length:expr, $message:literal) => {
            if kind == *$kind {
                if state.$seen || state.saw_plte || state.saw_idat || data.len() != $length {
                    return Err(Error::new($message));
                }
                state.$seen = true;
            }
        };
    }

    once_before_image!(b"cHRM", saw_chrm, 32, "invalid PNG cHRM");
    once_before_image!(b"gAMA", saw_gama, 4, "invalid PNG gAMA");
    once_before_image!(b"cICP", saw_cicp, 4, "invalid PNG cICP");
    once_before_image!(b"mDCV", saw_mdcv, 24, "invalid PNG mDCV");
    once_before_image!(b"cLLI", saw_clli, 8, "invalid PNG cLLI");

    if kind == *b"iCCP" {
        let name_end = find_nul(data, 0);
        if state.saw_iccp
            || state.saw_plte
            || state.saw_idat
            || name_end.is_none()
            || name_end == Some(0)
            || name_end.is_some_and(|end| end > 79)
            || name_end.map_or(true, |end| end + 2 > data.len() || data[end + 1] != 0)
        {
            return Err(Error::new("invalid PNG iCCP"));
        }
        state.saw_iccp = true;
    }

    if kind == *b"sBIT" {
        let expected = match state.color_type {
            0 => 1,
            2 | 3 => 3,
            4 => 2,
            _ => 4,
        };
        if state.saw_sbit || state.saw_plte || state.saw_idat || data.len() != expected {
            return Err(Error::new("invalid PNG sBIT"));
        }
        state.saw_sbit = true;
    }
    if kind == *b"sRGB" {
        if state.saw_srgb || state.saw_plte || state.saw_idat || data.len() != 1 || data[0] > 3 {
            return Err(Error::new("invalid PNG sRGB"));
        }
        state.saw_srgb = true;
    }
    if kind == *b"tRNS" {
        let valid_for_color_type = match state.color_type {
            0 => data.len() == 2,
            2 => data.len() == 6,
            3 => state.saw_plte && data.len() <= state.palette_entries as usize,
            6 => {
                let vestigial_palette_alpha = state.saw_plte
                    && !data.is_empty()
                    && data.len() <= state.palette_entries as usize;
                discard_on_output = vestigial_palette_alpha;
                vestigial_palette_alpha
            }
            _ => false,
        };
        if state.saw_trns || state.saw_idat || !valid_for_color_type {
            return Err(Error::new("invalid PNG tRNS"));
        }
        state.saw_trns = true;
    }
    if kind == *b"bKGD" {
        let expected = match state.color_type {
            0 | 4 => 2,
            3 => 1,
            _ => 6,
        };
        if state.saw_bkgd
            || state.saw_idat
            || (state.color_type == 3 && !state.saw_plte)
            || data.len() != expected
        {
            return Err(Error::new("invalid PNG bKGD"));
        }
        state.saw_bkgd = true;
    }
    if kind == *b"hIST" {
        if state.saw_hist
            || state.saw_idat
            || !state.saw_plte
            || data.len() != state.palette_entries as usize * 2
        {
            return Err(Error::new("invalid PNG hIST"));
        }
        state.saw_hist = true;
    }
    if kind == *b"pHYs" {
        if state.saw_phys || state.saw_idat || data.len() != 9 || data[8] > 1 {
            return Err(Error::new("invalid PNG pHYs"));
        }
        state.saw_phys = true;
    }
    if kind == *b"sCAL" {
        let separator = find_nul(data, 1);
        if state.saw_scal
            || state.saw_idat
            || data.len() < 4
            || !matches!(data[0], 1 | 2)
            || separator.is_none()
            || separator.is_some_and(|offset| {
                offset == 1
                    || offset + 1 == data.len()
                    || !valid_positive_png_float(&data[1..offset])
                    || !valid_positive_png_float(&data[offset + 1..])
            })
        {
            return Err(Error::new("invalid PNG sCAL"));
        }
        state.saw_scal = true;
    }
    if kind == *b"eXIf" {
        if state.saw_exif || state.saw_idat {
            return Err(Error::new("invalid PNG eXIf"));
        }
        state.saw_exif = true;
    }
    if kind == *b"tIME" {
        let year = if data.len() == 7 {
            u16::from_be_bytes(data[..2].try_into().unwrap())
        } else {
            0
        };
        if state.saw_time
            || data.len() != 7
            || year == 0
            || data[2] == 0
            || data[2] > 12
            || data[3] == 0
            || data[3] > 31
            || data[4] > 23
            || data[5] > 59
            || data[6] > 60
        {
            return Err(Error::new("invalid PNG tIME"));
        }
        state.saw_time = true;
    }
    if kind == *b"caBX" {
        // Content Credentials are bound to the PNG datastream and cannot be
        // updated by a Deflate optimizer. PNG additionally permits only one
        // caBX and requires it to precede IDAT.
        if state.saw_cabx || state.saw_idat {
            return Err(Error::new("invalid PNG caBX"));
        }
        state.saw_cabx = true;
    }
    validate_compressed_metadata(kind, data)?;

    if kind == *b"sPLT" {
        if state.saw_idat || data.len() < 3 {
            return Err(Error::new("invalid PNG sPLT"));
        }
        let name_end = find_nul(data, 0);
        if name_end.is_none()
            || name_end == Some(0)
            || name_end.map_or(true, |end| {
                end + 2 > data.len() || !matches!(data[end + 1], 8 | 16)
            })
        {
            return Err(Error::new("invalid PNG sPLT"));
        }
        let name_end = name_end.unwrap();
        let entry_size = if data[name_end + 1] == 8 { 6 } else { 10 };
        if (data.len() - name_end - 2) % entry_size != 0 {
            return Err(Error::new("invalid PNG sPLT"));
        }
    }
    Ok(discard_on_output)
}

/// Validate the decimal notation registered for PNG extension chunks.
///
/// `sCAL` requires a value greater than zero, but converting untrusted text to
/// `f64` would incorrectly reject valid extreme exponents through overflow or
/// underflow. The sign and nonzero decimal digits establish positivity without
/// imposing an artificial numeric range.
fn valid_positive_png_float(value: &[u8]) -> bool {
    if value.is_empty() {
        return false;
    }

    let mut index = 0;
    match value[0] {
        b'+' => index += 1,
        b'-' => return false,
        _ => {}
    }

    let mut integer_digits = 0;
    let mut nonzero_mantissa = false;
    while index < value.len() && value[index].is_ascii_digit() {
        nonzero_mantissa |= value[index] != b'0';
        integer_digits += 1;
        index += 1;
    }

    let mut fraction_digits = 0;
    if value.get(index) == Some(&b'.') {
        index += 1;
        while index < value.len() && value[index].is_ascii_digit() {
            nonzero_mantissa |= value[index] != b'0';
            fraction_digits += 1;
            index += 1;
        }
    }
    if integer_digits == 0 && fraction_digits == 0 {
        return false;
    }

    if matches!(value.get(index).copied(), Some(b'e' | b'E')) {
        index += 1;
        if matches!(value.get(index).copied(), Some(b'+' | b'-')) {
            index += 1;
        }
        let exponent_start = index;
        while index < value.len() && value[index].is_ascii_digit() {
            index += 1;
        }
        if index == exponent_start {
            return false;
        }
    }

    index == value.len() && nonzero_mantissa
}

fn validate_compressed_metadata(kind: [u8; 4], data: &[u8]) -> Result<()> {
    if kind == *b"zTXt" {
        let keyword_end = find_nul(data, 0);
        if keyword_end.is_none()
            || keyword_end == Some(0)
            || keyword_end.is_some_and(|end| end > 79)
            || keyword_end.map_or(true, |end| end + 2 > data.len() || data[end + 1] != 0)
        {
            return Err(Error::new("invalid PNG zTXt"));
        }
    }
    if kind == *b"iTXt" {
        let keyword_end = find_nul(data, 0);
        let Some(keyword_end) = keyword_end else {
            return Err(Error::new("invalid PNG iTXt"));
        };
        if keyword_end == 0
            || keyword_end > 79
            || keyword_end + 3 > data.len()
            || data[keyword_end + 1] > 1
            || (data[keyword_end + 1] == 1 && data[keyword_end + 2] != 0)
        {
            return Err(Error::new("invalid PNG iTXt"));
        }
        let Some(language_end) = find_nul(data, keyword_end + 3) else {
            return Err(Error::new("invalid PNG iTXt"));
        };
        if find_nul(data, language_end + 1).is_none() {
            return Err(Error::new("invalid PNG iTXt"));
        }
    }
    Ok(())
}

fn validate_animation_control(kind: [u8; 4], data: &[u8], state: &mut ParseState) -> Result<()> {
    if kind == *b"acTL" {
        if data.len() != 8 || state.saw_actl || state.saw_idat {
            return Err(Error::new("invalid APNG acTL chunk"));
        }
        state.animation_frames = u32::from_be_bytes(data[..4].try_into().unwrap());
        if state.animation_frames == 0 || u64::from(state.animation_frames) > MAX_APNG_FRAMES as u64
        {
            return Err(Error::new("invalid APNG acTL chunk"));
        }
        state.saw_actl = true;
    }

    if kind == *b"fcTL" {
        if !state.saw_actl
            || data.len() != 26
            || u32::from_be_bytes(data[..4].try_into().unwrap()) != state.sequence_expected
        {
            return Err(Error::new("invalid APNG fcTL chunk"));
        }
        state.sequence_expected += 1;
        state.fctl_count += 1;
        let frame_width = u32::from_be_bytes(data[4..8].try_into().unwrap());
        let frame_height = u32::from_be_bytes(data[8..12].try_into().unwrap());
        let x_offset = u32::from_be_bytes(data[12..16].try_into().unwrap());
        let y_offset = u32::from_be_bytes(data[16..20].try_into().unwrap());
        if frame_width == 0
            || frame_height == 0
            || frame_width > state.width
            || frame_height > state.height
            || x_offset > state.width - frame_width
            || y_offset > state.height - frame_height
            || data[24] > 2
            || data[25] > 1
        {
            return Err(Error::new("invalid APNG fcTL chunk"));
        }

        if !state.saw_idat {
            if state.frame_open
                || x_offset != 0
                || y_offset != 0
                || frame_width != state.width
                || frame_height != state.height
            {
                return Err(Error::new("invalid APNG fcTL chunk"));
            }
            // The default image uses IDAT, validated separately below.
            state.frame_has_data = true;
            state.current_fdat_decoded_size = None;
        } else {
            if state.frame_open && !state.frame_has_data {
                return Err(Error::new("missing APNG frame data"));
            }
            state.frame_has_data = false;
            state.current_fdat_decoded_size = Some(png_decoded_size(
                frame_width,
                frame_height,
                state.bit_depth,
                state.color_type,
                state.interlace_method,
            )?);
        }
        state.frame_open = true;
    }
    Ok(())
}

pub(super) fn compressed_zlib_offset(kind: [u8; 4], data: &[u8]) -> Option<usize> {
    if kind == *b"zTXt" {
        let keyword_end = find_nul(data, 0)?;
        return (keyword_end + 2 <= data.len() && data[keyword_end + 1] == 0)
            .then_some(keyword_end + 2);
    }
    if kind == *b"iTXt" {
        let keyword_end = find_nul(data, 0)?;
        if keyword_end + 3 > data.len() || data[keyword_end + 1] != 1 || data[keyword_end + 2] != 0
        {
            return None;
        }
        let language_end = find_nul(data, keyword_end + 3)?;
        let translated_end = find_nul(data, language_end + 1)?;
        return Some(translated_end + 1);
    }
    if kind == *b"iCCP" {
        let name_end = find_nul(data, 0)?;
        return (name_end + 2 <= data.len() && data[name_end + 1] == 0).then_some(name_end + 2);
    }
    None
}

fn find_nul(data: &[u8], start: usize) -> Option<usize> {
    data.get(start..)?
        .iter()
        .position(|&byte| byte == 0)
        .map(|offset| start + offset)
}

pub(super) fn append_chunk(output: &mut Vec<u8>, kind: [u8; 4], data: &[u8]) -> Result<()> {
    let length = u32::try_from(data.len()).map_err(|_| Error::new("PNG chunk too large"))?;
    let encoded_len = data
        .len()
        .checked_add(12)
        .ok_or_else(|| Error::new("PNG chunk too large"))?;
    output
        .try_reserve(encoded_len)
        .map_err(|_| Error::internal("could not allocate PNG output"))?;
    output.extend_from_slice(&length.to_be_bytes());
    output.extend_from_slice(&kind);
    output.extend_from_slice(data);
    let crc = crc32_update(crc32_update(0, &kind), data);
    output.extend_from_slice(&crc.to_be_bytes());
    Ok(())
}

pub(super) fn should_strip_kind(kind: [u8; 4], strip_metadata: bool) -> bool {
    strip_metadata && (is_strippable_metadata(kind) || is_unknown_unsafe_ancillary(kind))
}

fn is_strippable_metadata(kind: [u8; 4]) -> bool {
    matches!(
        &kind,
        b"bKGD"
            | b"caBX"
            | b"cHRM"
            | b"cICP"
            | b"cLLI"
            | b"eXIf"
            | b"gAMA"
            | b"hIST"
            | b"iCCP"
            | b"iDOT"
            | b"iTXt"
            | b"mDCV"
            | b"pHYs"
            | b"sCAL"
            | b"sBIT"
            | b"sPLT"
            | b"sRGB"
            | b"sTER"
            | b"dSIG"
            | b"tEXt"
            | b"tIME"
            | b"zTXt"
    )
}

fn is_known_critical(kind: [u8; 4]) -> bool {
    matches!(&kind, b"IHDR" | b"PLTE" | b"IDAT" | b"IEND")
}

fn is_known_ancillary(kind: [u8; 4]) -> bool {
    is_strippable_metadata(kind) || matches!(&kind, b"tRNS" | b"acTL" | b"fcTL" | b"fdAT")
}

fn is_unknown_unsafe_ancillary(kind: [u8; 4]) -> bool {
    kind[0] & 0x20 != 0 && !is_known_ancillary(kind) && kind[3] & 0x20 == 0
}

fn is_rewrite_sensitive_ancillary(kind: [u8; 4]) -> bool {
    // caBX and dSIG authenticate original datastream bytes; iDOT describes
    // Apple's original IDAT layout. Columbo cannot rebuild any of them after
    // changing critical chunks, so default mode takes the same conservative
    // path used for an unrecognized unsafe-to-copy ancillary chunk.
    matches!(&kind, b"caBX" | b"dSIG" | b"iDOT") || is_unknown_unsafe_ancillary(kind)
}

fn valid_chunk_type(kind: [u8; 4]) -> bool {
    kind.iter().all(u8::is_ascii_alphabetic) && kind[2] & 0x20 == 0
}

fn valid_bit_depth(color_type: u8, bit_depth: u8) -> bool {
    match color_type {
        0 => matches!(bit_depth, 1 | 2 | 4 | 8 | 16),
        2 | 4 | 6 => matches!(bit_depth, 8 | 16),
        3 => matches!(bit_depth, 1 | 2 | 4 | 8),
        _ => false,
    }
}

fn be32(input: &[u8], offset: usize) -> Result<u32> {
    input
        .get(offset..offset + 4)
        .and_then(|bytes| bytes.try_into().ok())
        .map(u32::from_be_bytes)
        .ok_or_else(|| Error::new("truncated PNG chunk"))
}

#[cfg(test)]
mod tests;
