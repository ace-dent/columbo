// SPDX-License-Identifier: MIT
// SPDX-FileCopyrightText: 2026 Andrew C.E. Dent <https://github.com/ace-dent>

//! zlib validation and window normalization for standalone or embedded streams.

use crate::deflate::{optimize_raw_prefix_with_floor, DefaultFloor, RawInfo};
use crate::{Error, Optimization, Options, Result};

use super::{try_append_bytes, try_copy_bytes, try_vec_with_capacity};

const OUTPUT_ALLOCATION: &str = "zlib output";

/// Result of optimizing a zlib stream embedded in a container.
#[derive(Debug)]
pub(super) struct StreamOptimization {
    pub(super) data: Vec<u8>,
    /// Present whenever the raw parser succeeded, even if lenient handling
    /// returned the original stream bytes.
    pub(super) info: Option<RawInfo>,
    pub(super) timed_out: bool,
    pub(super) removed_data_bytes: usize,
}

pub(super) fn optimize(input: &[u8], options: &Options) -> Result<Optimization> {
    // A non-lenient embedded call reports every wrapper error.
    let optimized = optimize_embedded(
        input,
        options,
        options.max_decoded_bytes,
        false,
        DefaultFloor::Complete,
    )?;
    let info = optimized
        .info
        .as_ref()
        .expect("a validated top-level zlib stream has raw information");
    Ok(Optimization::from_metrics(
        input.len(),
        optimized.data,
        info.source_deflate_bits,
        info.deflate_bits,
        optimized.timed_out,
    )
    .with_removed_data(optimized.removed_data_bytes))
}

/// Optimize a zlib stream embedded in another container.
///
/// PNG metadata may contain data that only resembles a zlib stream. With
/// `lenient_header`, such data is returned unchanged; the top-level zlib
/// handler rejects it.
pub(super) fn optimize_embedded(
    input: &[u8],
    options: &Options,
    decoded_limit: u64,
    lenient_header: bool,
    default_floor: DefaultFloor,
) -> Result<StreamOptimization> {
    let unsupported_dictionary = has_rfc1950_header(input) && input[1] & 0x20 != 0;
    if input.len() < 6 || !has_rfc1950_header(input) || unsupported_dictionary {
        if lenient_header {
            return Ok(StreamOptimization {
                data: try_copy_bytes(input, OUTPUT_ALLOCATION)?,
                info: None,
                timed_out: false,
                removed_data_bytes: 0,
            });
        }
        return Err(if input.len() < 6 {
            Error::new("zlib stream too small")
        } else if unsupported_dictionary {
            Error::unsupported_feature("preset zlib dictionaries are not supported")
        } else {
            Error::new("invalid zlib header")
        });
    }

    let raw_input = &input[2..];
    let mut raw = optimize_raw_prefix_with_floor(raw_input, options, decoded_limit, default_floor)?;
    let checksum_start = 2 + raw.consumed;
    let stream_end = checksum_start
        .checked_add(4)
        .filter(|&end| end <= input.len())
        .ok_or_else(|| Error::new("missing zlib Adler-32"))?;
    // In lenient mode (PNG metadata only), data after the Adler-32 marks a
    // lookalike, which is returned unchanged.
    if lenient_header && stream_end != input.len() {
        raw.info.deflate_bits = raw.info.source_deflate_bits;
        return Ok(StreamOptimization {
            data: try_copy_bytes(input, OUTPUT_ALLOCATION)?,
            info: Some(raw.info),
            timed_out: raw.timed_out,
            removed_data_bytes: 0,
        });
    }

    let advertised_window = 1_u32 << ((input[0] >> 4) + 8);
    if u32::from(raw.info.max_distance) > advertised_window {
        if lenient_header {
            raw.info.deflate_bits = raw.info.source_deflate_bits;
            return Ok(StreamOptimization {
                data: try_copy_bytes(input, OUTPUT_ALLOCATION)?,
                info: Some(raw.info),
                timed_out: raw.timed_out,
                removed_data_bytes: 0,
            });
        }
        return Err(Error::new(
            "zlib Deflate distance exceeds advertised window",
        ));
    }

    let stored_adler = u32::from_be_bytes(input[checksum_start..stream_end].try_into().unwrap());
    if raw.info.adler32 != stored_adler {
        return Err(Error::integrity_mismatch("zlib Adler-32 mismatch"));
    }

    let output_size = raw
        .data
        .len()
        .checked_add(6)
        .ok_or_else(|| Error::new("zlib output is too large"))?;
    let mut data = try_vec_with_capacity(output_size, OUTPUT_ALLOCATION)?;
    data.extend_from_slice(&optimized_header(input[0], raw.output_max_distance));
    data.extend_from_slice(&raw.data);
    data.extend_from_slice(&input[checksum_start..stream_end]);

    // Strict output can grow, since it may need a larger Huffman alphabet.
    // Without strict mode or --strip, a larger result falls back to the
    // source stream.
    if data.len() > stream_end && !options.strict && !options.strip_metadata {
        data.clear();
        try_append_bytes(&mut data, &input[..stream_end], OUTPUT_ALLOCATION)?;
        raw.output_max_distance = raw.info.max_distance;
        data[..2].copy_from_slice(&optimized_header(input[0], raw.output_max_distance));
        raw.info.deflate_bits = raw.info.source_deflate_bits;
    }

    if !options.strip_metadata {
        try_append_bytes(&mut data, &input[stream_end..], OUTPUT_ALLOCATION)?;
    }

    Ok(StreamOptimization {
        data,
        info: Some(raw.info),
        timed_out: raw.timed_out,
        removed_data_bytes: if options.strip_metadata {
            input.len() - stream_end
        } else {
            0
        },
    })
}

/// Build a header with the smallest sufficient RFC 1950 window and FLEVEL 3
/// (maximum compression).
///
/// FCHECK, the low five bits of FLG, makes the 16-bit header a multiple of 31.
/// FDICT is clear because preset-dictionary streams are rejected earlier.
fn optimized_header(cmf: u8, max_distance: u16) -> [u8; 2] {
    const MAXIMUM_FLEVEL: u8 = 0b11 << 6;

    let required = u32::from(max_distance).max(1).next_power_of_two();
    let window_bits = required.ilog2().max(8);
    let cinfo = (window_bits - 8) as u8;
    let cmf = (cinfo << 4) | (cmf & 0x0f);
    let unchecked = (u16::from(cmf) << 8) | u16::from(MAXIMUM_FLEVEL);
    let fcheck = (31 - unchecked % 31) % 31;
    [cmf, MAXIMUM_FLEVEL | fcheck as u8]
}

/// Recognize a valid two-byte RFC 1950 header (CM 8, CINFO at most 7, FCHECK).
///
/// FDICT is accepted, so a preset-dictionary stream is detected as zlib and
/// reported as unsupported.
pub(super) fn has_rfc1950_header(input: &[u8]) -> bool {
    if input.len() < 2 {
        return false;
    }
    let cmf = input[0];
    let flg = input[1];
    (cmf & 0x0f) == 8 && (cmf >> 4) <= 7 && ((u16::from(cmf) << 8) | u16::from(flg)) % 31 == 0
}

#[cfg(test)]
mod tests;
