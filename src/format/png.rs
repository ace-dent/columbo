// SPDX-License-Identifier: MIT

use std::time::Duration;

use crate::checksum::crc32_update;
use crate::deflate::{decoded_bytes_for_comparison, raw_stream_decodes_to, DefaultFloor, RawInfo};
use crate::{Error, Optimization, Options, Result};

use super::{scale_duration, zlib, SearchDeadline};

const SIGNATURE: &[u8; 8] = b"\x89PNG\r\n\x1a\n";
/// Exact cross-frame reuse is optional. Bounding the retained comparison bytes
/// keeps an APNG with many very large frames from doubling its memory use.
const MAX_EXACT_REUSE_BYTES: usize = 32 * 1024 * 1024;
const MAX_EXACT_REUSE_WORK_BYTES: u64 = 64 * 1024 * 1024;
const MAX_METADATA_PROBE_WORK_BYTES: u64 = 64 * 1024 * 1024;
/// Every APNG frame owns and validates an independent zlib stream. Bound that
/// invocation count separately from the generic chunk count because an empty
/// frame consumes almost no decoded-byte budget.
const MAX_APNG_FRAMES: usize = 16_384;
/// Compressed ancillary chunks are each decoded and checksum-validated even
/// when the optional search deadline is exhausted. Keep zero-length metadata
/// streams from multiplying that mandatory parser setup indefinitely.
const MAX_COMPRESSED_METADATA_STREAMS: usize = 4_096;
/// Twelve-byte empty chunks otherwise amplify into several independent Rust
/// model records. A million chunks is already far beyond practical PNG/APNG
/// use while keeping parser bookkeeping comfortably bounded.
const MAX_PNG_CHUNKS: usize = 1_000_000;

#[derive(Clone, Copy)]
struct Chunk<'a> {
    kind: [u8; 4],
    data: &'a [u8],
    encoded: &'a [u8],
}

struct ParsedPng<'a> {
    chunks: Vec<Chunk<'a>>,
    idat: Vec<u8>,
    fdat_frames: Vec<Vec<u8>>,
    has_unknown_unsafe_ancillary: bool,
}

#[derive(Default)]
struct ParseState {
    saw_ihdr: bool,
    width: u32,
    height: u32,
    bit_depth: u8,
    color_type: u8,
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
}

struct DecodeBudget {
    remaining: u64,
    timed_out: bool,
    deadline: SearchDeadline,
}

#[derive(Clone, Debug)]
struct FrameOptimization {
    data: Vec<u8>,
    info: Option<RawInfo>,
}

pub(super) fn optimize(input: &[u8], options: &Options) -> Result<Optimization> {
    let mut budget = DecodeBudget {
        remaining: options.max_decoded_bytes,
        timed_out: false,
        deadline: SearchDeadline::new(options),
    };
    let parsed = parse(input)?;

    // Small compressed metadata gets a short first pass in the original
    // Columbo C implementation so a profile or text comment cannot consume the
    // image stream's search time. If that pass finds no reduction,
    // reconstruction gives it one normal pass.
    let mut quick_replacements = Vec::new();
    quick_replacements
        .try_reserve_exact(parsed.chunks.len())
        .map_err(|_| Error::new("could not allocate PNG chunk model"))?;
    quick_replacements.resize_with(parsed.chunks.len(), || None);
    let mut probe_work_remaining = MAX_METADATA_PROBE_WORK_BYTES;
    for (index, chunk) in parsed.chunks.iter().enumerate() {
        if should_strip(chunk.kind, options) {
            continue;
        }
        let Some(offset) = compressed_zlib_offset(chunk.kind, chunk.data) else {
            continue;
        };
        if chunk.data.len() - offset > 4_096 {
            continue;
        }

        // Quick probes are optional and may be retried. Give them a separate,
        // monotonically decreasing compressed+decoded work allowance so a
        // long list of non-winning metadata streams cannot repeatedly consume
        // the complete file expansion budget.
        let compressed_work = (chunk.data.len() - offset) as u64;
        if compressed_work > probe_work_remaining {
            continue;
        }
        probe_work_remaining -= compressed_work;

        let mut quick_options = options.clone();
        quick_options.timeout = quick_options.timeout.min(Duration::from_millis(100));
        let remaining_before_probe = budget.remaining;
        let timed_out_before_probe = budget.timed_out;
        let probe_allowance = remaining_before_probe.min(probe_work_remaining);
        budget.remaining = probe_allowance;
        let probe = optimize_compressed_body(
            chunk.kind,
            chunk.data,
            &quick_options,
            DefaultFloor::Shared,
            &mut budget,
        );
        let decoded_work = probe_allowance.saturating_sub(budget.remaining);
        probe_work_remaining = probe_work_remaining.saturating_sub(decoded_work);
        let replacement = match probe {
            Ok(replacement) => replacement,
            Err(error) if error.message().contains("decoded PNG data exceeds") => {
                // The lower-level decoder cannot report partial decoded bytes
                // on this error. Conservatively spend the rest of the optional
                // probe allowance so a second tiny high-expansion stream cannot
                // repeat the same near-limit decode.
                probe_work_remaining = 0;
                budget.remaining = remaining_before_probe;
                budget.timed_out = timed_out_before_probe;
                continue;
            }
            Err(error) => return Err(error),
        };
        quick_replacements[index] = replacement;

        // A non-winning quick probe is retried later with the stream's normal
        // search allowance. Charge its decoded bytes only on that definitive
        // pass; otherwise the same metadata stream would consume the global
        // expansion budget twice. The 100 ms probe also has a local deadline,
        // so it must not report a file-wide timeout by itself.
        if quick_replacements[index].is_none() {
            budget.remaining = remaining_before_probe;
        } else {
            budget.remaining = remaining_before_probe.saturating_sub(decoded_work);
        }
        budget.timed_out = timed_out_before_probe;
    }

    let (optimized_idat, optimized_frames) =
        optimize_image_streams(&parsed.idat, &parsed.fdat_frames, options, &mut budget)?;

    // An unknown unsafe-to-copy ancillary chunk may depend on the exact
    // critical image representation. Its contract is unknowable, so after
    // validating every image stream preserve the complete source unless
    // --strip explicitly removes that chunk.
    if !options.strip_metadata && parsed.has_unknown_unsafe_ancillary {
        let data =
            try_clone_bytes(input).ok_or_else(|| Error::new("could not allocate PNG output"))?;
        return Ok(Optimization {
            data,
            timed_out: budget.timed_out,
        });
    }

    let mut output = Vec::new();
    output
        .try_reserve_exact(input.len())
        .map_err(|_| Error::new("could not allocate PNG output"))?;
    output.extend_from_slice(SIGNATURE);
    let mut idat_written = false;
    let mut frame_index = 0_usize;
    let mut frame_written = false;
    let mut animation_sequence = 0_u32;

    for (index, chunk) in parsed.chunks.iter().enumerate() {
        if should_strip(chunk.kind, options) {
            continue;
        }

        match &chunk.kind {
            b"IDAT" => {
                if !idat_written {
                    // IDAT boundaries are only packetization. Coalescing them
                    // saves twelve bytes for every redundant chunk.
                    append_chunk(&mut output, *b"IDAT", &optimized_idat)?;
                    idat_written = true;
                }
            }
            b"fcTL" => {
                frame_written = false;
                let mut body = try_clone_bytes(chunk.data)
                    .ok_or_else(|| Error::new("could not allocate APNG control chunk"))?;
                body[..4].copy_from_slice(&animation_sequence.to_be_bytes());
                animation_sequence += 1;
                append_chunk(&mut output, *b"fcTL", &body)?;
            }
            b"fdAT" => {
                if !frame_written {
                    let frame = optimized_frames
                        .get(frame_index)
                        .ok_or_else(|| Error::new("could not rebuild APNG frame"))?;
                    let body_len = frame
                        .data
                        .len()
                        .checked_add(4)
                        .ok_or_else(|| Error::new("APNG frame too large"))?;
                    let mut body = Vec::new();
                    body.try_reserve_exact(body_len)
                        .map_err(|_| Error::new("could not allocate APNG frame"))?;
                    body.extend_from_slice(&animation_sequence.to_be_bytes());
                    body.extend_from_slice(&frame.data);
                    animation_sequence += 1;
                    append_chunk(&mut output, *b"fdAT", &body)?;
                    frame_index += 1;
                    frame_written = true;
                }
            }
            b"zTXt" | b"iTXt" | b"iCCP" => {
                let replacement = if let Some(body) = &quick_replacements[index] {
                    Some(
                        try_clone_bytes(body)
                            .ok_or_else(|| Error::new("could not allocate PNG metadata result"))?,
                    )
                } else {
                    optimize_compressed_body(
                        chunk.kind,
                        chunk.data,
                        options,
                        DefaultFloor::Shared,
                        &mut budget,
                    )?
                };
                append_chunk(
                    &mut output,
                    chunk.kind,
                    replacement.as_deref().unwrap_or(chunk.data),
                )?;
            }
            _ => {
                output
                    .try_reserve(chunk.encoded.len())
                    .map_err(|_| Error::new("could not allocate PNG output"))?;
                output.extend_from_slice(chunk.encoded);
            }
        }
    }

    if output.len() > input.len() && !options.strict {
        output.clear();
        output.extend_from_slice(input);
    }

    Ok(Optimization {
        data: output,
        timed_out: budget.timed_out,
    })
}

fn parse(input: &[u8]) -> Result<ParsedPng<'_>> {
    if !input.starts_with(SIGNATURE) {
        return Err(Error::new("invalid PNG signature"));
    }

    let mut chunks = Vec::new();
    let mut idat = Vec::new();
    let mut fdat = Vec::new();
    let mut fdat_frames = Vec::new();
    let mut state = ParseState::default();
    let mut has_unknown_unsafe_ancillary = false;
    let mut compressed_metadata_streams = 0_usize;
    let mut position = SIGNATURE.len();

    while input.len().saturating_sub(position) >= 12 {
        if chunks.len() >= MAX_PNG_CHUNKS {
            return Err(Error::new("PNG contains too many chunks"));
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
            return Err(Error::new("bad PNG chunk CRC"));
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
        if is_unknown_unsafe_ancillary(kind) {
            has_unknown_unsafe_ancillary = true;
        }

        validate_palette(kind, data, &mut state)?;
        validate_ancillary(kind, data, &mut state)?;
        validate_animation_control(kind, data, &mut state)?;
        if compressed_zlib_offset(kind, data).is_some() {
            compressed_metadata_streams += 1;
            if compressed_metadata_streams > MAX_COMPRESSED_METADATA_STREAMS {
                return Err(Error::new(
                    "PNG contains too many compressed metadata streams",
                ));
            }
        }

        // fcTL begins the next fdAT zlib stream; IEND closes the final one.
        if matches!(&kind, b"fcTL" | b"IEND") && !fdat.is_empty() {
            fdat_frames
                .try_reserve(1)
                .map_err(|_| Error::new("could not allocate PNG frame model"))?;
            fdat_frames.push(std::mem::take(&mut fdat));
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
                .map_err(|_| Error::new("could not allocate PNG image stream"))?;
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
            state.frame_has_data = true;
            fdat.try_reserve(data.len() - 4)
                .map_err(|_| Error::new("could not allocate PNG frame stream"))?;
            fdat.extend_from_slice(&data[4..]);
        } else if state.saw_idat {
            state.after_idat_run = true;
        }

        chunks
            .try_reserve(1)
            .map_err(|_| Error::new("could not allocate PNG chunk model"))?;
        chunks.push(Chunk {
            kind,
            data,
            encoded: &input[position..after],
        });
        position = after;
        if kind == *b"IEND" {
            state.saw_iend = true;
            break;
        }
    }

    if state.saw_actl
        && (state.fctl_count != state.animation_frames
            || (state.frame_open && !state.frame_has_data))
    {
        return Err(Error::new("invalid APNG frame count"));
    }
    if !state.saw_ihdr || !state.saw_iend || position != input.len() {
        return Err(Error::new("invalid PNG trailer"));
    }
    if !state.saw_idat {
        return Err(Error::new("no IDAT chunk found"));
    }
    if idat.len() < 6 {
        return Err(Error::new("IDAT zlib stream too small"));
    }
    if (idat[0] & 0x0f) != 8 || (idat[0] >> 4) > 7 || idat[1] & 0x20 != 0 {
        return Err(Error::new("unsupported PNG zlib header"));
    }
    if ((u16::from(idat[0]) << 8) | u16::from(idat[1])) % 31 != 0 {
        return Err(Error::new("invalid PNG zlib header check"));
    }

    Ok(ParsedPng {
        chunks,
        idat,
        fdat_frames,
        has_unknown_unsafe_ancillary,
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

fn validate_ancillary(kind: [u8; 4], data: &[u8], state: &mut ParseState) -> Result<()> {
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
        let expected = match state.color_type {
            0 => 2,
            2 => 6,
            3 => data.len(),
            _ => 0,
        };
        if state.saw_trns
            || state.saw_idat
            || expected == 0
            || (state.color_type == 3
                && (!state.saw_plte || data.len() > state.palette_entries as usize))
            || (state.color_type != 3 && data.len() != expected)
        {
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
    Ok(())
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
        } else {
            if state.frame_open && !state.frame_has_data {
                return Err(Error::new("missing APNG frame data"));
            }
            state.frame_has_data = false;
        }
        state.frame_open = true;
    }
    Ok(())
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ImageJob {
    Idat,
    Frame(usize),
}

// Parsing, checksum validation, and PNG reconstruction also consume wall time,
// but only the raw Deflate searches receive the proportional slices below.
// Reserve ten percent outside the non-largest slices (twenty percent for a
// container with more than 32 unique image streams), then permit the final
// largest stream one bounded comparison-floor allowance. This mirrors the
// original Columbo C optimizer's per-stream fallback recovery; the black-box
// timeout tests cap the complete process, including this at-most-two-second
// allowance.
const NON_LARGEST_IMAGE_SEARCH_FRACTION: f64 = 0.90;
const MANY_IMAGE_SEARCH_FRACTION: f64 = 0.80;
const MANY_IMAGE_JOB_THRESHOLD: usize = 32;
const LARGEST_IMAGE_FLOOR_ALLOWANCE_FRACTION: f64 = 0.20;
const LARGEST_IMAGE_FLOOR_ALLOWANCE_CAP: Duration = Duration::from_secs(2);

/// Optimize the IDAT stream and each unique APNG frame under one file budget.
///
/// Small streams run first so a large IDAT cannot consume the whole deadline.
/// Duplicate frames contribute their full byte weight to their representative
/// because improving that one stream saves the same bytes at every occurrence.
fn optimize_image_streams(
    idat: &[u8],
    frames: &[Vec<u8>],
    options: &Options,
    budget: &mut DecodeBudget,
) -> Result<(Vec<u8>, Vec<FrameOptimization>)> {
    let representatives = frame_representatives(frames)?;
    let mut representative_weights = Vec::new();
    representative_weights
        .try_reserve_exact(frames.len())
        .map_err(|_| Error::new("could not allocate PNG frame model"))?;
    representative_weights.resize(frames.len(), 0_usize);
    for (index, &representative) in representatives.iter().enumerate() {
        representative_weights[representative] = representative_weights[representative]
            .checked_add(frames[index].len())
            .ok_or_else(|| Error::new("PNG frame data too large"))?;
    }

    let mut jobs = Vec::new();
    jobs.try_reserve_exact(
        frames
            .len()
            .checked_add(1)
            .ok_or_else(|| Error::new("too many PNG frames"))?,
    )
    .map_err(|_| Error::new("could not allocate PNG frame jobs"))?;
    jobs.push(ImageJob::Idat);
    jobs.extend(
        representatives
            .iter()
            .enumerate()
            .filter_map(|(index, &representative)| {
                (index == representative).then_some(ImageJob::Frame(index))
            }),
    );

    let total_weight = frames
        .iter()
        .try_fold(idat.len(), |total, frame| total.checked_add(frame.len()))
        .ok_or_else(|| Error::new("PNG frame data too large"))?;
    // Include source order explicitly so the in-place unstable sort retains
    // IDAT-before-fdAT ordering on ties without allocating a merge buffer.
    // Normal and max mode share one file deadline, so both need small-first
    // scheduling; otherwise the first large frame can starve every later one.
    jobs.sort_unstable_by_key(|job| {
        let source_order = match job {
            ImageJob::Idat => 0,
            ImageJob::Frame(index) => index.saturating_add(1),
        };
        (image_job_size(*job, idat, frames), source_order)
    });
    // Only the final job receives the reserved remainder. Distinct streams can
    // have the same largest byte length; treating every tie as the reserve sink
    // would let an earlier tie consume the time intended for the final job.
    let reserved_largest = *jobs.last().expect("the IDAT job is always present");
    let non_largest_fraction = if jobs.len() > MANY_IMAGE_JOB_THRESHOLD {
        MANY_IMAGE_SEARCH_FRACTION
    } else {
        NON_LARGEST_IMAGE_SEARCH_FRACTION
    };
    // Do not multiply fallback grace across very large animations: their many
    // mandatory validation/floor passes already account for the wall-clock
    // headroom. Smaller containers can use one bounded final-stream recovery.
    let largest_floor_allowance = if jobs.len() > MANY_IMAGE_JOB_THRESHOLD {
        Duration::ZERO
    } else {
        scale_duration(options.timeout, LARGEST_IMAGE_FLOOR_ALLOWANCE_FRACTION)
            .min(LARGEST_IMAGE_FLOOR_ALLOWANCE_CAP)
    };

    let mut optimized_idat = None;
    let mut optimized = Vec::<Option<FrameOptimization>>::new();
    optimized
        .try_reserve_exact(frames.len())
        .map_err(|_| Error::new("could not allocate PNG frame results"))?;
    optimized.resize_with(frames.len(), || None);
    let image_floor = if jobs.len() == 1 {
        DefaultFloor::Bounded
    } else {
        DefaultFloor::Shared
    };
    for job in jobs {
        let weight = match job {
            ImageJob::Idat => idat.len(),
            ImageJob::Frame(representative) => representative_weights[representative],
        };
        let mut call_options = options.clone();
        let file_remaining = budget.deadline.remaining();
        call_options.timeout = image_stream_timeout(
            options.timeout,
            file_remaining,
            weight,
            total_weight,
            non_largest_fraction,
            largest_floor_allowance,
            job == reserved_largest,
        );
        let extends_file_deadline = call_options.timeout > file_remaining;

        // A spent search budget disables optional searches, not validation.
        // Every IDAT/fdAT stream must still be fully decoded, checksum-checked,
        // and charged to the file-wide expansion limit.
        let stream = match job {
            ImageJob::Idat => idat,
            ImageJob::Frame(index) => frames[index].as_slice(),
        };
        let result =
            optimize_scheduled_png_zlib(stream, &call_options, false, image_floor, budget)?;
        if extends_file_deadline && budget.deadline.remaining().is_zero() {
            budget.timed_out = true;
        }
        match job {
            ImageJob::Idat => optimized_idat = Some(result.data),
            ImageJob::Frame(index) => {
                optimized[index] = Some(FrameOptimization {
                    data: result.data,
                    info: result.info,
                });
            }
        }
    }

    for (index, &representative) in representatives.iter().enumerate() {
        if index != representative {
            let frame = optimized[representative]
                .as_ref()
                .expect("every duplicate frame has an optimized representative");
            // Exact compressed duplicates share optimization work, not decode
            // budget. Each fdAT stream is an independent decoded payload in
            // the container and must count toward the file-wide safety limit.
            let decoded_size = frame
                .info
                .as_ref()
                .ok_or_else(|| Error::new("invalid PNG frame zlib stream"))?
                .size;
            if decoded_size > budget.remaining {
                return Err(Error::new(
                    "decoded PNG data exceeds configured safety limit",
                ));
            }
            budget.remaining -= decoded_size;
            optimized[index] = Some(
                try_clone_frame(frame)
                    .ok_or_else(|| Error::new("could not allocate duplicate PNG frame result"))?,
            );
        }
    }
    let mut complete = Vec::new();
    complete
        .try_reserve_exact(optimized.len())
        .map_err(|_| Error::new("could not allocate PNG frame results"))?;
    for frame in optimized {
        complete.push(frame.expect("every APNG frame has a representative result"));
    }

    budget.timed_out |=
        reuse_best_exact_frames(&mut complete, &mut || budget.deadline.remaining().is_zero());
    Ok((
        optimized_idat.expect("the IDAT job is always present"),
        complete,
    ))
}

/// Find the earliest exact-compressed representative in O(n log n) compares.
/// Sorting slices directly avoids adversarial hash-collision buckets.
fn frame_representatives(frames: &[Vec<u8>]) -> Result<Vec<usize>> {
    let mut order = Vec::new();
    order
        .try_reserve_exact(frames.len())
        .map_err(|_| Error::new("could not allocate PNG frame model"))?;
    order.extend(0..frames.len());
    order.sort_unstable_by(|&left, &right| {
        frames[left]
            .as_slice()
            .cmp(frames[right].as_slice())
            .then_with(|| left.cmp(&right))
    });

    let mut representatives = Vec::new();
    representatives
        .try_reserve_exact(frames.len())
        .map_err(|_| Error::new("could not allocate PNG frame model"))?;
    representatives.extend(0..frames.len());
    let mut group_start = 0;
    while group_start < order.len() {
        let first = order[group_start];
        let group_len =
            order[group_start..].partition_point(|&index| frames[index] == frames[first]);
        for &index in &order[group_start..group_start + group_len] {
            representatives[index] = first;
        }
        group_start += group_len;
    }
    Ok(representatives)
}

fn try_clone_frame(frame: &FrameOptimization) -> Option<FrameOptimization> {
    Some(FrameOptimization {
        data: try_clone_bytes(&frame.data)?,
        info: frame.info.clone(),
    })
}

fn image_job_size(job: ImageJob, idat: &[u8], frames: &[Vec<u8>]) -> usize {
    match job {
        ImageJob::Idat => idat.len(),
        ImageJob::Frame(index) => frames[index].len(),
    }
}

fn image_stream_timeout(
    configured: Duration,
    remaining: Duration,
    weight: usize,
    total_weight: usize,
    non_largest_fraction: f64,
    largest_floor: Duration,
    is_largest: bool,
) -> Duration {
    if weight == 0 || total_weight == 0 {
        return Duration::ZERO;
    }
    if remaining.is_zero() {
        return if is_largest {
            largest_floor
        } else {
            Duration::ZERO
        };
    }
    let headroom = scale_duration(remaining, 0.98);
    if is_largest {
        return headroom.max(largest_floor);
    }
    let proportional = scale_duration(
        configured,
        weight as f64 / total_weight as f64 * non_largest_fraction,
    );
    proportional.min(headroom)
}

/// Reuse a smaller frame representation only after exact decoded comparison.
///
/// CRC-32 and Adler-32 are useful filters, but together they are not an
/// identity proof: deliberately different byte strings can collide. We decode
/// one bounded reference for each checksum group and compare every candidate
/// byte-for-byte before substituting its compressed bytes.
fn reuse_best_exact_frames<F>(frames: &mut [FrameOptimization], expired: &mut F) -> bool
where
    F: FnMut() -> bool,
{
    // Build and sort checksum summaries first. Singleton summaries cannot
    // participate in reuse and therefore cost no extra decode. A fallible flat
    // vector keeps this optional route deterministic without many tiny map
    // allocations on an APNG containing thousands of frames.
    let mut summaries = Vec::<((u64, u32, u32), usize)>::new();
    if summaries.try_reserve_exact(frames.len()).is_err() {
        return false;
    }
    for (index, frame) in frames.iter().enumerate() {
        if let Some(info) = &frame.info {
            summaries.push(((info.size, info.crc32, info.adler32), index));
        }
    }
    summaries.sort_unstable();

    let mut grouped = Vec::new();
    if grouped.try_reserve_exact(frames.len()).is_err() {
        return false;
    }
    grouped.resize(frames.len(), false);
    let mut work_remaining = MAX_EXACT_REUSE_WORK_BYTES;
    let mut timed_out = false;
    let mut group_start = 0;
    'groups: while group_start < summaries.len() {
        let summary = summaries[group_start].0;
        let group_len =
            summaries[group_start..].partition_point(|&(candidate, _)| candidate == summary);
        let group_end = group_start + group_len;
        if group_len == 1 {
            group_start = group_end;
            continue;
        }

        // Equal scores cannot improve one another, regardless of content.
        let first_score = frame_score(&frames[summaries[group_start].1]);
        if summaries[group_start..group_end]
            .iter()
            .all(|&(_, index)| frame_score(&frames[index]) == first_score)
        {
            group_start = group_end;
            continue;
        }

        for position in group_start..group_end {
            let index = summaries[position].1;
            if grouped[index] {
                continue;
            }
            if expired() {
                timed_out = true;
                break 'groups;
            }
            let decoded_size = frames[index]
                .info
                .as_ref()
                .expect("a summary member has decode information")
                .size;
            let reference_work = exact_comparison_work(&frames[index], decoded_size);
            if reference_work > work_remaining {
                break 'groups;
            }
            work_remaining -= reference_work;
            let Some(reference_decoded) =
                decoded_zlib_for_comparison(&frames[index].data, decoded_size)
            else {
                grouped[index] = true;
                continue;
            };

            let mut members = Vec::new();
            if members.try_reserve_exact(group_end - position).is_err() {
                return timed_out;
            }
            members.push(index);
            for &(_, candidate) in &summaries[position + 1..group_end] {
                if grouped[candidate] {
                    continue;
                }
                let equal = if frames[candidate].data == frames[index].data {
                    true
                } else {
                    if expired() {
                        timed_out = true;
                        break 'groups;
                    }
                    let candidate_work = exact_comparison_work(&frames[candidate], decoded_size);
                    if candidate_work > work_remaining {
                        break 'groups;
                    }
                    work_remaining -= candidate_work;
                    zlib_decodes_to(&frames[candidate].data, &reference_decoded)
                };
                if equal {
                    members.push(candidate);
                }
            }

            let mut best = index;
            for &candidate in &members[1..] {
                if frame_is_better(&frames[candidate], &frames[best]) {
                    best = candidate;
                }
            }
            let Some(best_data) = try_clone_bytes(&frames[best].data) else {
                return timed_out;
            };
            let best_info = frames[best]
                .info
                .clone()
                .expect("an exact-reuse member has decoded stream information");
            for member in members {
                grouped[member] = true;
                if frame_is_better(&frames[best], &frames[member]) {
                    let Some(replacement) = try_clone_bytes(&best_data) else {
                        continue;
                    };
                    frames[member].data = replacement;
                    frames[member].info = Some(best_info.clone());
                }
            }
        }
        group_start = group_end;
    }
    timed_out
}

fn exact_comparison_work(frame: &FrameOptimization, decoded_size: u64) -> u64 {
    decoded_size.saturating_add(frame.data.len() as u64).max(1)
}

fn try_clone_bytes(source: &[u8]) -> Option<Vec<u8>> {
    let mut copy = Vec::new();
    copy.try_reserve_exact(source.len()).ok()?;
    copy.extend_from_slice(source);
    Some(copy)
}

fn frame_score(frame: &FrameOptimization) -> (usize, u64) {
    (
        frame.data.len(),
        frame
            .info
            .as_ref()
            .map_or(u64::MAX, |info| info.deflate_bits),
    )
}

fn frame_is_better(candidate: &FrameOptimization, reference: &FrameOptimization) -> bool {
    candidate.data.len() < reference.data.len()
        || (candidate.data.len() == reference.data.len()
            && candidate
                .info
                .as_ref()
                .zip(reference.info.as_ref())
                .is_some_and(|(candidate, reference)| {
                    candidate.deflate_bits < reference.deflate_bits
                }))
}

fn decoded_zlib_for_comparison(input: &[u8], decoded_size: u64) -> Option<Vec<u8>> {
    let raw = zlib_raw_payload(input)?;
    decoded_bytes_for_comparison(raw, decoded_size, MAX_EXACT_REUSE_BYTES)
}

fn zlib_decodes_to(input: &[u8], expected: &[u8]) -> bool {
    let Some(raw) = zlib_raw_payload(input) else {
        return false;
    };
    raw_stream_decodes_to(raw, expected.len() as u64, expected)
}

fn zlib_raw_payload(input: &[u8]) -> Option<&[u8]> {
    (input.len() >= 6).then(|| &input[2..input.len() - 4])
}

fn optimize_png_zlib(
    input: &[u8],
    options: &Options,
    lenient_header: bool,
    default_floor: DefaultFloor,
    budget: &mut DecodeBudget,
) -> Result<zlib::StreamOptimization> {
    let call_options = budget.deadline.options_for_call(options);
    optimize_png_zlib_with_options(input, &call_options, lenient_header, default_floor, budget)
}

/// Optimize an image stream whose local slice was already computed from the
/// file schedule. Unlike metadata calls, this deliberately does not clamp the
/// final comparison-floor allowance a second time to the outer deadline.
fn optimize_scheduled_png_zlib(
    input: &[u8],
    options: &Options,
    lenient_header: bool,
    default_floor: DefaultFloor,
    budget: &mut DecodeBudget,
) -> Result<zlib::StreamOptimization> {
    optimize_png_zlib_with_options(input, options, lenient_header, default_floor, budget)
}

fn optimize_png_zlib_with_options(
    input: &[u8],
    call_options: &Options,
    lenient_header: bool,
    default_floor: DefaultFloor,
    budget: &mut DecodeBudget,
) -> Result<zlib::StreamOptimization> {
    let result = zlib::optimize_embedded(
        input,
        call_options,
        budget.remaining,
        lenient_header,
        default_floor,
    )
    .map_err(|error| {
        if error.message().contains("internal memory safety") {
            error
        } else if error.message().contains("limit") || error.message().contains("safety") {
            Error::new("decoded PNG data exceeds configured safety limit")
        } else {
            error
        }
    })?;
    if let Some(info) = &result.info {
        if info.size > budget.remaining {
            return Err(Error::new(
                "decoded PNG data exceeds configured safety limit",
            ));
        }
        budget.remaining -= info.size;
    }
    budget.timed_out |= result.timed_out;
    Ok(result)
}

fn optimize_compressed_body(
    kind: [u8; 4],
    data: &[u8],
    options: &Options,
    default_floor: DefaultFloor,
    budget: &mut DecodeBudget,
) -> Result<Option<Vec<u8>>> {
    let Some(zlib_offset) = compressed_zlib_offset(kind, data) else {
        return Ok(None);
    };
    let optimized = optimize_png_zlib(&data[zlib_offset..], options, true, default_floor, budget)?;
    if zlib_offset + optimized.data.len() >= data.len() && !options.strict {
        return Ok(None);
    }
    let body_len = zlib_offset
        .checked_add(optimized.data.len())
        .ok_or_else(|| Error::new("PNG compressed metadata too large"))?;
    let mut body = Vec::new();
    body.try_reserve_exact(body_len)
        .map_err(|_| Error::new("could not allocate PNG compressed metadata"))?;
    body.extend_from_slice(&data[..zlib_offset]);
    body.extend_from_slice(&optimized.data);
    Ok(Some(body))
}

fn compressed_zlib_offset(kind: [u8; 4], data: &[u8]) -> Option<usize> {
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

fn append_chunk(output: &mut Vec<u8>, kind: [u8; 4], data: &[u8]) -> Result<()> {
    let length = u32::try_from(data.len()).map_err(|_| Error::new("PNG chunk too large"))?;
    let encoded_len = data
        .len()
        .checked_add(12)
        .ok_or_else(|| Error::new("PNG chunk too large"))?;
    output
        .try_reserve(encoded_len)
        .map_err(|_| Error::new("could not allocate PNG output"))?;
    output.extend_from_slice(&length.to_be_bytes());
    output.extend_from_slice(&kind);
    output.extend_from_slice(data);
    let crc = crc32_update(crc32_update(0, &kind), data);
    output.extend_from_slice(&crc.to_be_bytes());
    Ok(())
}

fn should_strip(kind: [u8; 4], options: &Options) -> bool {
    options.strip_metadata && (is_strippable_metadata(kind) || is_unknown_unsafe_ancillary(kind))
}

fn is_strippable_metadata(kind: [u8; 4]) -> bool {
    matches!(
        &kind,
        b"bKGD"
            | b"cHRM"
            | b"cICP"
            | b"cLLI"
            | b"eXIf"
            | b"gAMA"
            | b"hIST"
            | b"iCCP"
            | b"iTXt"
            | b"mDCV"
            | b"pHYs"
            | b"sCAL"
            | b"sBIT"
            | b"sPLT"
            | b"sRGB"
            | b"sTER"
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
mod tests {
    use super::*;
    use crate::checksum::adler32;

    fn chunk(kind: [u8; 4], data: &[u8]) -> Vec<u8> {
        let mut bytes = Vec::new();
        append_chunk(&mut bytes, kind, data).unwrap();
        bytes
    }

    fn ihdr() -> [u8; 13] {
        let mut data = [0_u8; 13];
        data[3] = 1; // width
        data[7] = 1; // height
        data[8] = 8; // grayscale, eight bits per sample
        data
    }

    fn black_scanline_zlib() -> Vec<u8> {
        vec![
            0x78, 0x01, // zlib header
            0x01, 0x02, 0x00, 0xfd, 0xff, 0x00, 0x00, // stored Deflate block
            0x00, 0x02, 0x00, 0x01, // Adler-32([filter=0, pixel=0])
        ]
    }

    fn stored_zlib(decoded: &[u8]) -> Vec<u8> {
        let length = u16::try_from(decoded.len()).unwrap();
        let mut stream = vec![0x78, 0x01, 0x01];
        stream.extend_from_slice(&length.to_le_bytes());
        stream.extend_from_slice(&(!length).to_le_bytes());
        stream.extend_from_slice(decoded);
        stream.extend_from_slice(&adler32(decoded).to_be_bytes());
        stream
    }

    #[test]
    fn validates_crc_before_decoding_idat() {
        let mut input = SIGNATURE.to_vec();
        let mut ihdr = [0_u8; 13];
        ihdr[3] = 1;
        ihdr[7] = 1;
        ihdr[8] = 8;
        ihdr[9] = 0;
        input.extend(chunk(*b"IHDR", &ihdr));
        let mut bad_idat = chunk(*b"IDAT", &[0x78, 0x01, 1, 0, 0, 0]);
        *bad_idat.last_mut().unwrap() ^= 1;
        input.extend(bad_idat);
        input.extend(chunk(*b"IEND", &[]));

        let error = optimize(&input, &Options::default()).unwrap_err();
        assert_eq!(error.message(), "bad PNG chunk CRC");
    }

    #[test]
    fn rejects_nonconsecutive_idat_chunks() {
        let mut input = SIGNATURE.to_vec();
        let mut ihdr = [0_u8; 13];
        ihdr[3] = 1;
        ihdr[7] = 1;
        ihdr[8] = 8;
        input.extend(chunk(*b"IHDR", &ihdr));
        input.extend(chunk(*b"IDAT", &[0x78, 0x01, 1]));
        input.extend(chunk(*b"tEXt", b"x"));
        input.extend(chunk(*b"IDAT", &[0, 0, 0]));
        input.extend(chunk(*b"IEND", &[]));

        let error = optimize(&input, &Options::default()).unwrap_err();
        assert_eq!(error.message(), "non-consecutive IDAT chunk");
    }

    #[test]
    fn optimizes_and_revalidates_a_minimal_png() {
        let mut input = SIGNATURE.to_vec();
        input.extend(chunk(*b"IHDR", &ihdr()));
        input.extend(chunk(*b"IDAT", &black_scanline_zlib()));
        input.extend(chunk(*b"IEND", &[]));

        let result = optimize(&input, &Options::default()).unwrap();
        assert!(result.data.len() <= input.len());
        parse(&result.data).unwrap();
    }

    #[test]
    fn bounded_max_deadline_still_returns_a_valid_png() {
        let mut input = SIGNATURE.to_vec();
        input.extend(chunk(*b"IHDR", &ihdr()));
        input.extend(chunk(*b"IDAT", &black_scanline_zlib()));
        input.extend(chunk(*b"IEND", &[]));
        let options = Options {
            exhaustive: true,
            timeout: Duration::ZERO,
            ..Options::default()
        };

        let result = optimize(&input, &options).unwrap();
        assert!(result.timed_out);
        assert!(result.data.len() <= input.len());
        parse(&result.data).unwrap();
    }

    #[test]
    fn validates_scal_floats_without_imposing_a_machine_numeric_range() {
        let valid: &[&[u8]] = &[
            b"1",
            b"+1.",
            b".5",
            b"0.0001",
            b"5e-324",
            b"1E+999999999999999999999999999999999999999",
        ];
        for value in valid {
            assert!(valid_positive_png_float(value), "{value:?}");
        }

        let invalid: &[&[u8]] = &[
            b"",
            b"0",
            b"+0.0e999",
            b"-1",
            b".",
            b"+.",
            b"1e",
            b"1e+",
            b"1 0",
            b"1_0",
            b"NaN",
            b"inf",
        ];
        for value in invalid {
            assert!(!valid_positive_png_float(value), "{value:?}");
        }
    }

    #[test]
    fn rejects_malformed_duplicate_and_misordered_scal_chunks() {
        let valid = [1, b'+', b'1', b'.', b'0', b'e', b'-', b'9', 0, b'.', b'5'];
        let mut state = ParseState::default();
        validate_ancillary(*b"sCAL", &valid, &mut state).unwrap();

        let duplicate = validate_ancillary(*b"sCAL", &valid, &mut state).unwrap_err();
        assert_eq!(duplicate.message(), "invalid PNG sCAL");

        let invalid: &[&[u8]] = &[
            &[0, b'1', 0, b'1'],
            &[3, b'1', 0, b'1'],
            &[1, b'1'],
            &[1, 0, b'1'],
            &[1, b'1', 0],
            &[1, b'1', 0, b'1', 0],
            &[1, b'0', 0, b'1'],
            &[1, b'1', 0, b'-', b'1'],
        ];
        for data in invalid {
            let error = validate_ancillary(*b"sCAL", data, &mut ParseState::default()).unwrap_err();
            assert_eq!(error.message(), "invalid PNG sCAL", "{data:?}");
        }

        let mut after_idat = ParseState {
            saw_idat: true,
            ..ParseState::default()
        };
        let misordered = validate_ancillary(*b"sCAL", &valid, &mut after_idat).unwrap_err();
        assert_eq!(misordered.message(), "invalid PNG sCAL");
    }

    #[test]
    fn coalesces_ten_idat_chunks_and_preserves_registered_scal() {
        // This is the already-minimal 1x1 zlib stream used by the 24-chunk
        // corpus case. Only removing nine redundant IDAT wrappers can shrink
        // it, for an exact saving of 9 * 12 bytes.
        let zlib = [0x78, 0x01, 0x63, 0xf8, 0x0f, 0x00, 0x01, 0x01, 0x01, 0x00];
        let scal = [1, b'1', b'.', b'0', 0, b'1', b'.', b'0'];
        let mut input = SIGNATURE.to_vec();
        input.extend(chunk(*b"IHDR", &ihdr()));
        input.extend(chunk(*b"sCAL", &scal));
        for byte in zlib {
            input.extend(chunk(*b"IDAT", &[byte]));
        }
        input.extend(chunk(*b"IEND", &[]));

        let result = optimize(&input, &Options::default()).unwrap();
        assert_eq!(input.len() - result.data.len(), 108);

        let parsed = parse(&result.data).unwrap();
        assert_eq!(
            parsed
                .chunks
                .iter()
                .filter(|chunk| chunk.kind == *b"IDAT")
                .count(),
            1
        );
        let preserved = parsed
            .chunks
            .iter()
            .find(|chunk| chunk.kind == *b"sCAL")
            .expect("registered sCAL metadata should be preserved");
        assert_eq!(preserved.data, scal);
    }

    #[test]
    fn zero_search_budget_still_validates_every_frame_stream() {
        let invalid_frame = vec![
            0x78, 0x01, 0x03, 0x00, // valid empty Deflate stream
            0x00, 0x00, 0x00, 0x02, // wrong Adler-32 for empty data
        ];
        let options = Options {
            exhaustive: true,
            timeout: Duration::ZERO,
            ..Options::default()
        };
        let mut budget = DecodeBudget {
            remaining: options.max_decoded_bytes,
            timed_out: false,
            deadline: SearchDeadline::new(&options),
        };

        let error = optimize_image_streams(
            &black_scanline_zlib(),
            &[invalid_frame],
            &options,
            &mut budget,
        )
        .unwrap_err();
        assert_eq!(error.message(), "zlib Adler-32 mismatch");
    }

    #[test]
    fn checksum_tuple_is_only_a_filter_for_cross_frame_reuse() {
        let first_data = stored_zlib(&[0, 0]);
        let second_data = stored_zlib(&[0, 1]);
        assert_eq!(first_data.len(), second_data.len());

        // Deliberately forge equal summary fields. The second representation
        // appears cheaper by bit count, but its exact decoded bytes differ.
        let summary = RawInfo {
            size: 2,
            crc32: 7,
            adler32: 11,
            deflate_bits: 80,
            ..RawInfo::default()
        };
        let mut frames = vec![
            FrameOptimization {
                data: first_data.clone(),
                info: Some(summary.clone()),
            },
            FrameOptimization {
                data: second_data.clone(),
                info: Some(RawInfo {
                    deflate_bits: 79,
                    ..summary
                }),
            },
        ];

        reuse_best_exact_frames(&mut frames, &mut || false);
        assert_eq!(frames[0].data, first_data);
        assert_eq!(frames[1].data, second_data);
    }

    #[test]
    fn exact_frame_reuse_stops_when_its_deadline_is_spent() {
        let summary = RawInfo {
            size: 2,
            crc32: 7,
            adler32: 11,
            ..RawInfo::default()
        };
        let mut frames = vec![
            FrameOptimization {
                data: stored_zlib(&[0, 0]),
                info: Some(RawInfo {
                    deflate_bits: 80,
                    ..summary.clone()
                }),
            },
            FrameOptimization {
                data: stored_zlib(&[0, 1]),
                info: Some(RawInfo {
                    deflate_bits: 79,
                    ..summary
                }),
            },
        ];
        let before: Vec<Vec<u8>> = frames.iter().map(|frame| frame.data.clone()).collect();

        assert!(reuse_best_exact_frames(&mut frames, &mut || true));
        assert_eq!(
            frames
                .iter()
                .map(|frame| frame.data.clone())
                .collect::<Vec<_>>(),
            before
        );
    }

    #[test]
    fn identical_frame_grouping_uses_the_earliest_exact_source() {
        let frames = vec![
            b"beta".to_vec(),
            b"alpha".to_vec(),
            b"beta".to_vec(),
            b"alpha".to_vec(),
            b"gamma".to_vec(),
        ];
        assert_eq!(frame_representatives(&frames).unwrap(), [0, 1, 0, 1, 4]);
    }

    #[test]
    fn rejects_reserved_png_zlib_window_exponent() {
        let mut input = SIGNATURE.to_vec();
        input.extend(chunk(*b"IHDR", &ihdr()));
        input.extend(chunk(
            *b"IDAT",
            &[0x88, 0x1c, 0x03, 0x00, 0x00, 0x00, 0x00, 0x01],
        ));
        input.extend(chunk(*b"IEND", &[]));

        let error = optimize(&input, &Options::default()).unwrap_err();
        assert_eq!(error.message(), "unsupported PNG zlib header");
    }

    #[test]
    fn rejects_pathological_apng_frame_counts_early() {
        let mut state = ParseState::default();
        let mut control = [0_u8; 8];
        control[..4].copy_from_slice(&((MAX_APNG_FRAMES as u32) + 1).to_be_bytes());

        let error = validate_animation_control(*b"acTL", &control, &mut state).unwrap_err();
        assert_eq!(error.message(), "invalid APNG acTL chunk");
    }

    #[test]
    fn rejects_pathological_compressed_metadata_counts_early() {
        let mut input = SIGNATURE.to_vec();
        input.extend(chunk(*b"IHDR", &ihdr()));
        for _ in 0..=MAX_COMPRESSED_METADATA_STREAMS {
            input.extend(chunk(*b"zTXt", b"k\0\0"));
        }

        let error = match parse(&input) {
            Err(error) => error,
            Ok(_) => panic!("excess compressed metadata should fail"),
        };
        assert_eq!(
            error.message(),
            "PNG contains too many compressed metadata streams"
        );
    }

    #[test]
    fn image_timeout_is_proportional_and_reserves_largest_leftover() {
        let configured = Duration::from_secs(10);
        let remaining = Duration::from_secs(8);

        assert_eq!(
            image_stream_timeout(
                configured,
                remaining,
                2,
                10,
                NON_LARGEST_IMAGE_SEARCH_FRACTION,
                Duration::from_secs(2),
                false,
            ),
            Duration::from_millis(1_800)
        );
        assert_eq!(
            image_stream_timeout(
                configured,
                remaining,
                2,
                10,
                MANY_IMAGE_SEARCH_FRACTION,
                Duration::ZERO,
                false,
            ),
            Duration::from_millis(1_600)
        );
        assert_eq!(
            image_stream_timeout(
                configured,
                remaining,
                2,
                10,
                NON_LARGEST_IMAGE_SEARCH_FRACTION,
                Duration::from_secs(2),
                true,
            ),
            Duration::from_millis(7_840)
        );
        assert_eq!(
            image_stream_timeout(
                configured,
                Duration::ZERO,
                2,
                10,
                NON_LARGEST_IMAGE_SEARCH_FRACTION,
                Duration::from_secs(2),
                true,
            ),
            Duration::from_secs(2)
        );
    }

    #[test]
    fn image_scheduler_obeys_a_tiny_wall_budget_in_both_modes() {
        let source_idat = black_scanline_zlib();
        // Distinct payloads prevent representative folding, so this exercises
        // the actual multi-job schedule rather than one cloned job.
        let frames: Vec<_> = (0_u8..12).map(|value| stored_zlib(&[value, 0])).collect();
        let source_lengths: Vec<_> = frames.iter().map(Vec::len).collect();
        for exhaustive in [false, true] {
            let options = Options {
                exhaustive,
                timeout: Duration::from_millis(20),
                ..Options::default()
            };
            let mut budget = DecodeBudget {
                remaining: options.max_decoded_bytes,
                timed_out: false,
                deadline: SearchDeadline::new(&options),
            };

            let started = std::time::Instant::now();
            let (optimized_idat, optimized_frames) =
                optimize_image_streams(&source_idat, &frames, &options, &mut budget).unwrap();

            assert!(started.elapsed() < Duration::from_secs(2));
            assert_eq!(optimized_frames.len(), frames.len());
            assert!(optimized_idat.len() <= source_idat.len());
            assert!(optimized_frames
                .iter()
                .zip(&source_lengths)
                .all(|(frame, &source_len)| frame.data.len() <= source_len));
        }
    }

    #[test]
    fn duplicate_frames_share_search_but_each_consume_decode_budget() {
        let idat = stored_zlib(b"");
        let frame = black_scanline_zlib(); // Two decoded scanline bytes.
        let frames = vec![frame.clone(), frame];

        let options = Options {
            timeout: Duration::ZERO,
            max_decoded_bytes: 3,
            ..Options::default()
        };
        let mut budget = DecodeBudget {
            remaining: options.max_decoded_bytes,
            timed_out: false,
            deadline: SearchDeadline::new(&options),
        };
        let error = optimize_image_streams(&idat, &frames, &options, &mut budget).unwrap_err();
        assert_eq!(
            error.message(),
            "decoded PNG data exceeds configured safety limit"
        );

        let options = Options {
            max_decoded_bytes: 4,
            ..options
        };
        let mut budget = DecodeBudget {
            remaining: options.max_decoded_bytes,
            timed_out: false,
            deadline: SearchDeadline::new(&options),
        };
        optimize_image_streams(&idat, &frames, &options, &mut budget).unwrap();
        assert_eq!(budget.remaining, 0);
    }

    #[test]
    fn coalesces_idat_chunks_and_strips_text_metadata() {
        let zlib = black_scanline_zlib();
        let mut input = SIGNATURE.to_vec();
        input.extend(chunk(*b"IHDR", &ihdr()));
        input.extend(chunk(*b"tEXt", b"Comment\0fixture"));
        input.extend(chunk(*b"IDAT", &zlib[..5]));
        input.extend(chunk(*b"IDAT", &zlib[5..]));
        input.extend(chunk(*b"IEND", &[]));

        let options = Options {
            strip_metadata: true,
            ..Options::default()
        };
        let result = optimize(&input, &options).unwrap();
        let parsed = parse(&result.data).unwrap();
        assert_eq!(
            parsed
                .chunks
                .iter()
                .filter(|chunk| chunk.kind == *b"IDAT")
                .count(),
            1
        );
        assert!(!parsed.chunks.iter().any(|chunk| chunk.kind == *b"tEXt"));
        assert!(result.data.len() < input.len());
    }

    #[test]
    fn preserves_png_with_unknown_unsafe_ancillary_chunk() {
        let zlib = black_scanline_zlib();
        let mut input = SIGNATURE.to_vec();
        input.extend(chunk(*b"IHDR", &ihdr()));
        // Ancillary (lowercase first byte), unknown, and unsafe to copy after
        // critical-data changes (uppercase fourth byte).
        input.extend(chunk(*b"vpAG", b"private contract"));
        input.extend(chunk(*b"IDAT", &zlib[..5]));
        input.extend(chunk(*b"IDAT", &zlib[5..]));
        input.extend(chunk(*b"IEND", &[]));

        let result = optimize(&input, &Options::default()).unwrap();
        assert_eq!(result.data, input);
    }

    #[test]
    fn rebuilds_apng_frame_streams_and_sequence_numbers() {
        let zlib = black_scanline_zlib();
        let mut input = SIGNATURE.to_vec();
        input.extend(chunk(*b"IHDR", &ihdr()));
        let mut actl = Vec::new();
        actl.extend_from_slice(&2_u32.to_be_bytes());
        actl.extend_from_slice(&0_u32.to_be_bytes());
        input.extend(chunk(*b"acTL", &actl));

        let frame_control = |sequence: u32| {
            let mut body = Vec::new();
            body.extend_from_slice(&sequence.to_be_bytes());
            body.extend_from_slice(&1_u32.to_be_bytes()); // width
            body.extend_from_slice(&1_u32.to_be_bytes()); // height
            body.extend_from_slice(&0_u32.to_be_bytes()); // x offset
            body.extend_from_slice(&0_u32.to_be_bytes()); // y offset
            body.extend_from_slice(&1_u16.to_be_bytes()); // delay numerator
            body.extend_from_slice(&10_u16.to_be_bytes()); // delay denominator
            body.extend_from_slice(&[0, 0]); // dispose and blend operations
            body
        };
        input.extend(chunk(*b"fcTL", &frame_control(0)));
        input.extend(chunk(*b"IDAT", &zlib));
        input.extend(chunk(*b"fcTL", &frame_control(1)));
        let mut frame_data = 2_u32.to_be_bytes().to_vec();
        frame_data.extend_from_slice(&zlib);
        input.extend(chunk(*b"fdAT", &frame_data));
        input.extend(chunk(*b"IEND", &[]));

        let result = optimize(&input, &Options::default()).unwrap();
        assert!(result.data.len() <= input.len());
        let parsed = parse(&result.data).unwrap();
        assert_eq!(parsed.fdat_frames.len(), 1);
    }

    #[test]
    fn metadata_probe_counts_decoded_bytes_only_once() {
        // This valid zTXt stream expands to one byte and is already too small
        // for the quick pass to shrink. Together with the two-byte image row,
        // it exactly fills the deliberately tiny decoded-data budget.
        let metadata_zlib = [0x78, 0x9c, 0xab, 0x00, 0x00, 0x00, 0x79, 0x00, 0x79];
        let mut metadata = b"Comment\0\0".to_vec();
        metadata.extend_from_slice(&metadata_zlib);

        let mut input = SIGNATURE.to_vec();
        input.extend(chunk(*b"IHDR", &ihdr()));
        input.extend(chunk(*b"zTXt", &metadata));
        input.extend(chunk(*b"IDAT", &black_scanline_zlib()));
        input.extend(chunk(*b"IEND", &[]));

        let options = Options {
            max_decoded_bytes: 3,
            ..Options::default()
        };
        let result = optimize(&input, &options).unwrap();
        parse(&result.data).unwrap();
    }

    #[test]
    fn retained_lenient_metadata_still_consumes_decode_budget() {
        let mut lookalike = stored_zlib(b"x");
        lookalike.extend_from_slice(b"trailing");
        let options = Options {
            max_decoded_bytes: 1,
            ..Options::default()
        };
        let mut budget = DecodeBudget {
            remaining: options.max_decoded_bytes,
            timed_out: false,
            deadline: SearchDeadline::new(&options),
        };

        let retained = optimize_png_zlib(
            &lookalike,
            &options,
            true,
            DefaultFloor::Bounded,
            &mut budget,
        )
        .unwrap();
        assert_eq!(retained.data, lookalike);
        assert_eq!(budget.remaining, 0);

        let error = optimize_png_zlib(
            &lookalike,
            &options,
            true,
            DefaultFloor::Bounded,
            &mut budget,
        )
        .unwrap_err();
        assert_eq!(
            error.message(),
            "decoded PNG data exceeds configured safety limit"
        );
    }
}
