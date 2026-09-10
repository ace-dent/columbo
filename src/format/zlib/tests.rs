// SPDX-License-Identifier: MIT

use std::time::Duration;

use super::*;
use crate::format::test_support::same_byte_bit_win_zlib;

fn feedback_zlib() -> Vec<u8> {
    let raw = [
        0x25, 0xc0, 0x01, 0x01, 0xc0, 0x30, 0x0c, 0xc3, 0x30, 0x6c, 0xb5, 0x9b, 0xf0, 0x87, 0xf4,
        0x7d, 0xd3, 0xcc, 0xcc, 0xcc, 0xcc, 0x01, 0x00, 0x00, 0xc0, 0x71, 0x5d, 0xaa, 0xaa, 0xaa,
        0xfe, 0x76, 0x77, 0x93, 0x24, 0x49, 0x9e, 0xa7, 0x6d, 0xdb, 0xf6, 0x03,
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
    let input = same_byte_bit_win_zlib();
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
fn output_window_normalization_does_not_hide_an_invalid_source_window() {
    // A stored block supplies 257 zero bytes of history. The following
    // fixed block copies three bytes from distance 257, just beyond a
    // 256-byte advertised window. This synthetic stream needs no corpus.
    let mut input = vec![0x78, 0x01, 0x00, 0x01, 0x01, 0xfe, 0xfe];
    input.resize(input.len() + 257, 0);
    input.extend_from_slice(&[0x03, 0x06, 0x00, 0x00]);
    input.extend_from_slice(&crate::checksum::test_support::adler32(&[0; 260]).to_be_bytes());
    assert!(optimize(&input, &Options::default()).is_ok());
    let source_cmf = input[0];
    input[..2].copy_from_slice(&optimized_header(source_cmf, 0));
    let error = optimize(
        &input,
        &Options {
            timeout: Duration::ZERO,
            ..Options::default()
        },
    )
    .unwrap_err();
    assert_eq!(
        error.message(),
        "zlib Deflate distance exceeds advertised window"
    );
}

#[test]
fn rejects_too_short_stream() {
    let error = optimize(&[0x78, 0x9c], &Options::default()).unwrap_err();
    assert_eq!(error.message(), "zlib stream too small");
}

#[test]
fn valid_empty_stream_advertises_smallest_window_and_maximum_compression() {
    // Empty fixed-Huffman Deflate stream followed by Adler-32("").
    let input = [0x78, 0x01, 0x03, 0x00, 0x00, 0x00, 0x00, 0x01];
    let result = optimize(&input, &Options::default()).unwrap();
    assert_eq!(&result.data[..2], &[0x08, 0xd7]);
    assert_eq!(&result.data[2..], &input[2..]);
    assert!(has_rfc1950_header(&result.data));
    assert_eq!(result.data[1] >> 6, 3);
    assert!(!result.timed_out);
}

#[test]
fn window_header_tracks_every_distance_boundary() {
    for (distance, expected_cinfo) in [
        (0, 0),
        (1, 0),
        (256, 0),
        (257, 1),
        (512, 1),
        (513, 2),
        (16_384, 6),
        (16_385, 7),
        (32_768, 7),
    ] {
        let header = optimized_header(0x78, distance);
        assert_eq!(header[0] >> 4, expected_cinfo, "distance={distance}");
        assert_eq!(header[0] & 0x0f, 8, "distance={distance}");
        assert_eq!(header[1] >> 6, 3, "distance={distance}");
        assert_eq!(u16::from_be_bytes(header) % 31, 0, "distance={distance}");
    }
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
