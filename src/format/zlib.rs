// SPDX-License-Identifier: MIT

//! zlib validation and window normalization for standalone or embedded streams.

use crate::deflate::{optimize_raw_prefix_with_floor, DefaultFloor, RawInfo};
use crate::{Error, Optimization, Options, Result};

use super::{try_append_bytes, try_copy_bytes, try_vec_with_capacity};

const OUTPUT_ALLOCATION_ERROR: &str = "could not allocate zlib output";

/// Extra facts needed by container formats that embed a zlib stream.
#[derive(Debug)]
pub(super) struct StreamOptimization {
    pub(super) data: Vec<u8>,
    /// Decode facts are present whenever the raw parser ran successfully,
    /// even if lenient metadata handling retained the original wrapper bytes.
    pub(super) info: Option<RawInfo>,
    pub(super) timed_out: bool,
}

pub(super) fn optimize(input: &[u8], options: &Options) -> Result<Optimization> {
    if input.len() < 6 {
        return Err(Error::new("zlib stream too small"));
    }
    if !has_rfc1950_header(input) {
        return Err(Error::new("invalid zlib header"));
    }
    if input[1] & 0x20 != 0 {
        return Err(Error::unsupported_feature(
            "preset zlib dictionaries are not supported",
        ));
    }

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
    ))
}

/// Optimize a zlib stream embedded in another container.
///
/// PNG metadata is allowed to contain data that merely resembles a compressed
/// stream. `lenient_header` therefore retains such data unchanged, matching
/// the original Columbo C implementation, while the top-level zlib handler
/// rejects it.
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
                data: try_copy_bytes(input, OUTPUT_ALLOCATION_ERROR)?,
                info: None,
                timed_out: false,
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

    // A zlib stream is exactly: two-byte header, raw Deflate data, Adler-32.
    // FLEVEL is only an encoder-effort hint, so rewritten streams advertise
    // Columbo's maximum optimization effort. CINFO records the smallest RFC
    // 1950 window that can decode the emitted distances.
    let raw_input = &input[2..input.len() - 4];
    let mut raw = optimize_raw_prefix_with_floor(raw_input, options, decoded_limit, default_floor)?;

    // Raw parsing always completes one stream. Any bytes left before the
    // wrapper checksum therefore make a top-level zlib stream malformed.
    // Lenient PNG metadata keeps lookalike data unchanged instead of turning
    // an optional ancillary chunk into a whole-file error.
    if raw.consumed != raw_input.len() {
        if lenient_header {
            raw.info.deflate_bits = raw.info.source_deflate_bits;
            return Ok(StreamOptimization {
                data: try_copy_bytes(input, OUTPUT_ALLOCATION_ERROR)?,
                info: Some(raw.info),
                timed_out: raw.timed_out,
            });
        }
        return Err(Error::new("trailing data after zlib stream"));
    }

    let advertised_window = 1_u32 << ((input[0] >> 4) + 8);
    if u32::from(raw.info.max_distance) > advertised_window {
        if lenient_header {
            raw.info.deflate_bits = raw.info.source_deflate_bits;
            return Ok(StreamOptimization {
                data: try_copy_bytes(input, OUTPUT_ALLOCATION_ERROR)?,
                info: Some(raw.info),
                timed_out: raw.timed_out,
            });
        }
        return Err(Error::new(
            "zlib Deflate distance exceeds advertised window",
        ));
    }

    let stored_adler = u32::from_be_bytes(input[input.len() - 4..].try_into().unwrap());
    if raw.info.adler32 != stored_adler {
        return Err(Error::integrity_mismatch("zlib Adler-32 mismatch"));
    }

    let output_size = raw
        .data
        .len()
        .checked_add(6)
        .ok_or_else(|| Error::new("zlib output is too large"))?;
    let mut data = try_vec_with_capacity(output_size, OUTPUT_ALLOCATION_ERROR)?;
    data.extend_from_slice(&optimized_header(input[0], raw.output_max_distance));
    data.extend_from_slice(&raw.data);
    data.extend_from_slice(&input[input.len() - 4..]);

    // Strict compatibility can require a slightly larger Huffman alphabet.
    // Relaxed mode retains the project's no-growth guarantee.
    if data.len() > input.len() && !options.strict {
        data.clear();
        try_append_bytes(&mut data, input, OUTPUT_ALLOCATION_ERROR)?;
        raw.output_max_distance = raw.info.max_distance;
        data[..2].copy_from_slice(&optimized_header(input[0], raw.output_max_distance));
        raw.info.deflate_bits = raw.info.source_deflate_bits;
    }

    Ok(StreamOptimization {
        data,
        info: Some(raw.info),
        timed_out: raw.timed_out,
    })
}

/// Advertise the smallest sufficient RFC 1950 window and maximum effort.
///
/// FCHECK occupies the low five bits of FLG and makes the two-byte header a
/// multiple of 31. FDICT is clear because dictionary-backed streams are
/// rejected before this helper is reached.
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

/// Recognize the complete two-byte RFC 1950 header.
///
/// FDICT remains part of the wrapper signature even though Columbo cannot
/// optimize a stream that depends on a caller-supplied preset dictionary.
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
