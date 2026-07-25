// SPDX-License-Identifier: MIT

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
        return Err(Error::new("preset zlib dictionaries are not supported"));
    }

    let optimized = optimize_embedded(
        input,
        options,
        options.max_decoded_bytes,
        false,
        DefaultFloor::Complete,
    )?;
    Ok(Optimization {
        data: optimized.data,
        timed_out: optimized.timed_out,
    })
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
        return Err(Error::new(if input.len() < 6 {
            "zlib stream too small"
        } else if unsupported_dictionary {
            "preset zlib dictionaries are not supported"
        } else {
            "invalid zlib header"
        }));
    }

    // A zlib stream is exactly: two-byte header, raw Deflate data, Adler-32.
    // Keep both wrapper fields byte-for-byte so Columbo only changes Deflate.
    let raw_input = &input[2..input.len() - 4];
    let raw = optimize_raw_prefix_with_floor(raw_input, options, decoded_limit, default_floor)?;

    // Raw parsing always completes one stream. Any bytes left before the
    // wrapper checksum therefore make a top-level zlib stream malformed.
    // Lenient PNG metadata keeps lookalike data unchanged instead of turning
    // an optional ancillary chunk into a whole-file error.
    if raw.consumed != raw_input.len() {
        if lenient_header {
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
        return Err(Error::new("zlib Adler-32 mismatch"));
    }

    let output_size = raw
        .data
        .len()
        .checked_add(6)
        .ok_or_else(|| Error::new("zlib output is too large"))?;
    let mut data = try_vec_with_capacity(output_size, OUTPUT_ALLOCATION_ERROR)?;
    data.extend_from_slice(&input[..2]);
    data.extend_from_slice(&raw.data);
    data.extend_from_slice(&input[input.len() - 4..]);

    // Compatibility mode intentionally permits growth; every normal mode has
    // the project's strict no-growth guarantee.
    if data.len() > input.len() && !options.min_distance_codes {
        data.clear();
        try_append_bytes(&mut data, input, OUTPUT_ALLOCATION_ERROR)?;
    }

    Ok(StreamOptimization {
        data,
        info: Some(raw.info),
        timed_out: raw.timed_out,
    })
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
mod tests {
    use super::*;

    #[test]
    fn rejects_preset_dictionary_header() {
        // 0x78 0x20 has FDICT set and a valid FCHECK value.
        let error = optimize(&[0x78, 0x20, 0, 0, 0, 0], &Options::default()).unwrap_err();
        assert_eq!(
            error.message(),
            "preset zlib dictionaries are not supported"
        );
    }

    #[test]
    fn rejects_a_window_exponent_reserved_by_rfc_1950() {
        // 0x88 0x1c has CM=8 and a valid FCHECK, but CINFO=8 is reserved.
        let input = [0x88, 0x1c, 0x03, 0x00, 0x00, 0x00, 0x00, 0x01];
        let error = optimize(&input, &Options::default()).unwrap_err();
        assert_eq!(error.message(), "invalid zlib header");
    }

    #[test]
    fn rejects_too_short_stream() {
        let error = optimize(&[0x78, 0x9c], &Options::default()).unwrap_err();
        assert_eq!(error.message(), "zlib stream too small");
    }

    #[test]
    fn preserves_a_valid_empty_stream() {
        // Empty fixed-Huffman Deflate stream followed by Adler-32("").
        let input = [0x78, 0x01, 0x03, 0x00, 0x00, 0x00, 0x00, 0x01];
        let result = optimize(&input, &Options::default()).unwrap();
        assert_eq!(result.data, input);
        assert!(!result.timed_out);
    }

    #[test]
    fn validates_adler32() {
        let input = [0x78, 0x01, 0x03, 0x00, 0x00, 0x00, 0x00, 0x02];
        let error = optimize(&input, &Options::default()).unwrap_err();
        assert_eq!(error.message(), "zlib Adler-32 mismatch");
    }

    #[test]
    fn rejects_trailing_data_after_a_complete_stream() {
        let mut input = vec![0x78, 0x01, 0x03, 0x00, 0x00, 0x00, 0x00, 0x01];
        input.extend_from_slice(b"junk");

        let error = optimize(&input, &Options::default()).unwrap_err();
        assert_eq!(error.message(), "trailing data after zlib stream");
    }

    #[test]
    fn lenient_metadata_preserves_zlib_lookalikes_with_trailing_data() {
        let mut input = vec![0x78, 0x01, 0x03, 0x00, 0x00, 0x00, 0x00, 0x01];
        input.extend_from_slice(b"junk");

        let result = optimize_embedded(
            &input,
            &Options::default(),
            1024,
            true,
            DefaultFloor::Bounded,
        )
        .unwrap();
        assert_eq!(result.data, input);
        assert_eq!(result.info.unwrap().size, 0);
    }
}
