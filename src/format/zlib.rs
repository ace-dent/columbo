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
    // The checksum and window/method byte remain unchanged. FLEVEL is only an
    // encoder-effort hint, so rewritten streams advertise Columbo's maximum
    // optimization effort while retaining a valid FCHECK value.
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
    data.extend_from_slice(&maximum_compression_header(input[0]));
    data.extend_from_slice(&raw.data);
    data.extend_from_slice(&input[input.len() - 4..]);

    // Strict compatibility can require a slightly larger Huffman alphabet.
    // Relaxed mode retains the project's no-growth guarantee.
    if data.len() > input.len() && !options.strict {
        data.clear();
        try_append_bytes(&mut data, input, OUTPUT_ALLOCATION_ERROR)?;
        data[..2].copy_from_slice(&maximum_compression_header(input[0]));
        raw.info.deflate_bits = raw.info.source_deflate_bits;
    }

    Ok(StreamOptimization {
        data,
        info: Some(raw.info),
        timed_out: raw.timed_out,
    })
}

/// Preserve CM/CINFO while advertising RFC 1950's maximum compression level.
///
/// FCHECK occupies the low five bits of FLG and makes the two-byte header a
/// multiple of 31. FDICT is clear because dictionary-backed streams are
/// rejected before this helper is reached.
fn maximum_compression_header(cmf: u8) -> [u8; 2] {
    const MAXIMUM_FLEVEL: u8 = 0b11 << 6;

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
mod tests {
    use std::time::Duration;

    use super::*;

    fn feedback_zlib() -> Vec<u8> {
        let raw = [
            0x25, 0xc0, 0x01, 0x01, 0xc0, 0x30, 0x0c, 0xc3, 0x30, 0x6c, 0xb5, 0x9b, 0xf0, 0x87,
            0xf4, 0x7d, 0xd3, 0xcc, 0xcc, 0xcc, 0xcc, 0x01, 0x00, 0x00, 0xc0, 0x71, 0x5d, 0xaa,
            0xaa, 0xaa, 0xfe, 0x76, 0x77, 0x93, 0x24, 0x49, 0x9e, 0xa7, 0x6d, 0xdb, 0xf6, 0x03,
        ];
        let (_, info) = crate::deflate::inspect_raw_prefix(&raw, 86).unwrap();
        let mut stream = vec![0x78, 0x01];
        stream.extend_from_slice(&raw);
        stream.extend_from_slice(&info.adler32.to_be_bytes());
        stream
    }

    fn deflate_bits(input: &[u8]) -> u64 {
        crate::deflate::inspect_raw_prefix(&input[2..input.len() - 4], u64::MAX)
            .unwrap()
            .1
            .source_deflate_bits
    }

    #[test]
    fn same_byte_deflate_win_reports_bits_saved() {
        let input = super::super::same_byte_bit_win_zlib();
        let optimized = optimize(&input, &Options::default()).unwrap();

        assert_eq!(optimized.data.len(), input.len());
        assert_eq!(optimized.bits_saved, 1);
    }

    #[test]
    fn zero_budget_zlib_max_retains_default_in_bytes_and_bits() {
        let input = feedback_zlib();
        for strict in [true, false] {
            let default_options = Options {
                strict,
                timeout: Duration::from_secs(1),
                ..Options::default()
            };
            let default = optimize(&input, &default_options).unwrap();
            let maximum = optimize(
                &input,
                &Options {
                    exhaustive: true,
                    timeout: Duration::ZERO,
                    ..default_options
                },
            )
            .unwrap();

            assert!(maximum.data.len() <= default.data.len(), "strict={strict}");
            assert!(
                deflate_bits(&maximum.data) <= deflate_bits(&default.data),
                "strict={strict}"
            );
        }
    }

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
    fn valid_empty_stream_advertises_maximum_compression() {
        // Empty fixed-Huffman Deflate stream followed by Adler-32("").
        let input = [0x78, 0x01, 0x03, 0x00, 0x00, 0x00, 0x00, 0x01];
        let result = optimize(&input, &Options::default()).unwrap();
        assert_eq!(&result.data[..2], &[0x78, 0xda]);
        assert_eq!(&result.data[2..], &input[2..]);
        assert!(has_rfc1950_header(&result.data));
        assert_eq!(result.data[1] >> 6, 3);
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
            DefaultFloor::Shared,
        )
        .unwrap();
        assert_eq!(result.data, input);
        let info = result.info.unwrap();
        assert_eq!(info.size, 0);
        assert_eq!(info.deflate_bits, info.source_deflate_bits);
    }
}
