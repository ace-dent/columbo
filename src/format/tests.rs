// SPDX-License-Identifier: MIT
// SPDX-FileCopyrightText: 2026 Andrew C.E. Dent <https://github.com/ace-dent>

use super::test_support::SAME_BYTE_BIT_WIN_RAW;
use super::*;

fn zlib_header(cinfo: u8, flevel: u8, preset_dictionary: bool) -> [u8; 2] {
    let cmf = (cinfo << 4) | 8;
    let mut flg = (flevel << 6) | (u8::from(preset_dictionary) << 5);
    let header = (u16::from(cmf) << 8) | u16::from(flg);
    flg += ((31 - header % 31) % 31) as u8;
    [cmf, flg]
}

fn prefixed_empty_zip(prefix: &[u8]) -> Vec<u8> {
    let mut input = prefix.to_vec();
    input.extend_from_slice(b"PK\x05\x06");
    input.extend_from_slice(&[0; 12]); // Disk numbers, entry counts, central size.
    input.extend_from_slice(&(prefix.len() as u32).to_le_bytes());
    input.extend_from_slice(&0_u16.to_le_bytes()); // No archive comment.
    input
}

/// zlib's stream for 95 bytes of six values. Max's first pass ends at 309
/// bits, and a pass over that output saves one more.
const SECOND_PASS_RAW: &[u8] = &[
    0x85, 0x8b, 0xc9, 0x09, 0x00, 0x30, 0x10, 0x02, 0x8b, 0x4a, 0x2d, 0x8b, 0x25, 0x84, 0x69, 0xc1,
    0xea, 0x33, 0x8f, 0xfd, 0x47, 0x50, 0xc1, 0x83, 0x54, 0x04, 0x52, 0xc2, 0x1d, 0x05, 0xca, 0x18,
    0x60, 0x41, 0x93, 0x8e, 0x01, 0xbb, 0xfb, 0x18, 0x7b, 0x51, 0x8e, 0xe4, 0x01,
];

#[test]
fn max_repeats_passes_until_one_changes_nothing() {
    let max = Options {
        exhaustive: true,
        ..Options::default()
    };
    let first = optimize_pass(SECOND_PASS_RAW, Format::Raw, Format::Raw, &max, 1).unwrap();
    let second = optimize_pass(&first.data, Format::Raw, Format::Raw, &max, 1).unwrap();
    assert!(
        second.bits_saved != 0,
        "a second pass no longer improves this stream; the test needs another"
    );

    let repeated = optimize(SECOND_PASS_RAW, Format::Raw, &max).unwrap();
    assert!(repeated.data.len() <= second.data.len());
    assert_eq!(
        repeated.bits_saved,
        8 * (SECOND_PASS_RAW.len() - repeated.data.len()) as u64
    );
    assert!(!repeated.timed_out);
    // Max, run again on its own output, has nothing left to save.
    let again = optimize(&repeated.data, Format::Raw, &max).unwrap();
    assert_eq!(again.data, repeated.data);
    assert_eq!(again.bits_saved, 0);

    // Only Max repeats, and only with time left.
    let default = Options::default();
    assert_eq!(
        optimize(SECOND_PASS_RAW, Format::Raw, &default)
            .unwrap()
            .data,
        optimize_pass(SECOND_PASS_RAW, Format::Raw, Format::Raw, &default, 1)
            .unwrap()
            .data
    );
    let no_time = Options {
        timeout: std::time::Duration::ZERO,
        ..max.clone()
    };
    assert_eq!(
        optimize(SECOND_PASS_RAW, Format::Raw, &no_time)
            .unwrap()
            .data,
        optimize_pass(SECOND_PASS_RAW, Format::Raw, Format::Raw, &no_time, 1)
            .unwrap()
            .data
    );
}

/// A 62-byte stream whose Default finish changes the block after the sweep's
/// header fit: the distance ladder and the smoothed trees end at 475 bits,
/// and symbol set removal (R5) then saves 4 more.
const CLOSING_FIT_RAW: &[u8] = &[
    0x8b, 0xf6, 0xc5, 0x06, 0xa2, 0xc1, 0xc8, 0xd7, 0x37, 0x04, 0xc6, 0x8d, 0x86, 0x09, 0x43, 0x59,
    0x98, 0xea, 0xe1, 0x54, 0x34, 0x0e, 0x35, 0x78, 0x40, 0x74, 0x34, 0x18, 0xa1, 0x18, 0x17, 0x8d,
    0x69, 0x32, 0x8a, 0xd3, 0xa0, 0x6e, 0x0b, 0xc1, 0x74, 0x05, 0x90, 0x11, 0x82, 0x45, 0x10, 0x85,
    0x17, 0x8d, 0x64, 0x07, 0xb2, 0x6a, 0xac, 0x9a, 0xe0, 0x4e, 0x22, 0x19, 0x00, 0x00,
];

#[test]
fn default_fits_headers_again_after_the_finish_changes_the_stream() {
    let default = optimize(CLOSING_FIT_RAW, Format::Raw, &Options::default()).unwrap();
    // 60 bytes without the closing header fit.
    assert_eq!(default.data.len(), 59);
    // Max's Default endpoint runs it too, so Max does not trail Default.
    let max = Options {
        exhaustive: true,
        ..Options::default()
    };
    let max = optimize(CLOSING_FIT_RAW, Format::Raw, &max).unwrap();
    assert!(max.data.len() <= default.data.len());
}

#[test]
fn custom_input_limit_uses_a_truthful_diagnostic() {
    let options = Options {
        max_input_bytes: 1,
        ..Options::default()
    };

    let error = optimize(&[0x03, 0x00], Format::Raw, &options).unwrap_err();
    assert_eq!(error.message(), "input exceeds configured safety limit");
}

#[test]
fn top_level_expansion_limit_combines_ratio_floor_and_absolute_ceiling() {
    let defaults = Options::default();
    let small = options_with_input_expansion_limit(1, &defaults);
    assert_eq!(small.max_decoded_bytes, crate::MIN_EXPANSION_LIMIT_BYTES);

    let larger_input = usize::try_from(crate::MIN_EXPANSION_LIMIT_BYTES).unwrap();
    let larger = options_with_input_expansion_limit(larger_input, &defaults);
    assert_eq!(larger.max_decoded_bytes, defaults.max_decoded_bytes);

    let absolute = Options {
        max_decoded_bytes: 123,
        ..defaults.clone()
    };
    assert_eq!(
        options_with_input_expansion_limit(1, &absolute).max_decoded_bytes,
        123
    );

    let trusted = Options {
        max_expansion_ratio: None,
        ..defaults.clone()
    };
    assert_eq!(
        options_with_input_expansion_limit(1, &trusted).max_decoded_bytes,
        defaults.max_decoded_bytes
    );

    let saturating = Options {
        max_decoded_bytes: u64::MAX,
        max_expansion_ratio: Some(u64::MAX),
        ..defaults
    };
    assert_eq!(
        options_with_input_expansion_limit(usize::MAX, &saturating).max_decoded_bytes,
        u64::MAX
    );
}

#[test]
fn raw_same_byte_bit_win_reports_write_savings() {
    let optimized = optimize(SAME_BYTE_BIT_WIN_RAW, Format::Raw, &Options::default()).unwrap();

    assert_eq!(optimized.data.len(), SAME_BYTE_BIT_WIN_RAW.len());
    assert_eq!(optimized.bits_saved, 1);
}

#[test]
fn changed_padding_without_a_meaningful_bit_win_reports_no_savings() {
    // The final six high bits are outside the ten-bit empty fixed stream.
    // Strict re-emission may zero them, but that is not compression.
    let input = [0x03, 0xfc];
    let optimized = optimize(&input, Format::Raw, &Options::default()).unwrap();

    assert_eq!(optimized.data.len(), input.len());
    assert_eq!(optimized.bits_saved, 0);
}

#[test]
fn raw_suffix_is_preserved_or_explicitly_stripped() {
    let input = [0x03, 0xfc, 0xde, 0xad];
    for strip_metadata in [false, true] {
        for exhaustive in [false, true] {
            let result = optimize(
                &input,
                Format::Raw,
                &Options {
                    strip_metadata,
                    exhaustive,
                    strict: false,
                    timeout: std::time::Duration::ZERO,
                    ..Options::default()
                },
            )
            .unwrap();
            if strip_metadata {
                assert_eq!(result.data, [0x03, 0]);
                assert_eq!(result.removed_data_bytes, 2);
                assert!(result.should_replace());
            } else {
                assert_eq!(result.data, input);
                assert_eq!(result.removed_data_bytes, 0);
            }
        }
    }
}

#[test]
fn stripping_padding_is_written_without_claiming_compression_savings() {
    let result = optimize(
        &[0x03, 0xfc],
        Format::Raw,
        &Options {
            strip_metadata: true,
            strict: false,
            ..Options::default()
        },
    )
    .unwrap();
    assert_eq!(result.data, [0x03, 0]);
    assert_eq!(result.bits_saved, 0);
    assert_eq!(result.removed_data_bytes, 0);
    assert!(result.should_replace());
}

#[test]
fn raw_suffix_requires_explicit_format_to_avoid_false_detection() {
    for strip_metadata in [false, true] {
        assert!(optimize(
            &[0x03, 0x00, 0xff],
            Format::Auto,
            &Options {
                strip_metadata,
                ..Options::default()
            }
        )
        .is_err());
    }
}

#[test]
fn auto_detects_every_rfc_1950_window_and_emits_the_smallest_safe_window() {
    for cinfo in 0..=7 {
        for flevel in 0..=3 {
            let mut input = zlib_header(cinfo, flevel, false).to_vec();
            input.extend_from_slice(&[0x03, 0x00]); // Empty fixed Deflate stream.
            input.extend_from_slice(&1_u32.to_be_bytes()); // Adler-32("").

            let optimized = optimize(&input, Format::Auto, &Options::default()).unwrap();
            assert_eq!(optimized.data[0], 0x08, "CINFO={cinfo}, FLEVEL={flevel}");
            assert_eq!(optimized.data[1] >> 6, 3, "CINFO={cinfo}, FLEVEL={flevel}");
            assert_eq!(
                optimized.data[2..],
                input[2..],
                "CINFO={cinfo}, FLEVEL={flevel}"
            );
            assert_eq!(
                u16::from_be_bytes(optimized.data[..2].try_into().unwrap()) % 31,
                0,
                "CINFO={cinfo}, FLEVEL={flevel}"
            );
        }
    }
}

#[test]
fn invalid_fcheck_bytes_do_not_steal_raw_deflate_streams() {
    for cinfo in 0..=7 {
        let cmf = (cinfo << 4) | 8;
        // The first byte starts a non-final stored block. The next four
        // bytes encode an empty LEN/NLEN pair, followed by an empty final
        // fixed block. CMF looks zlib-like, but CMF/FLG fails FCHECK.
        let input = [cmf, 0x00, 0x00, 0xff, 0xff, 0x03, 0x00];
        assert!(!zlib::has_rfc1950_header(&input));
        assert_eq!(detect(&input), Detection::RawCandidate);

        let automatic = optimize(&input, Format::Auto, &Options::default()).unwrap();
        let explicit = optimize(&input, Format::Raw, &Options::default()).unwrap();
        assert_eq!(automatic, explicit, "CINFO={cinfo}");
    }
}

#[test]
fn gzip_probe_distinguishes_valid_and_damaged_headers() {
    assert_eq!(
        detect(&[0x1f, 0x8b, 8, 0]),
        Detection::Confirmed(Format::Gzip)
    );
    assert_eq!(
        detect(&[0x1f, 0x8b, 7, 0]),
        Detection::Recognized(Format::Gzip)
    );
    assert_eq!(
        detect(&[0x1f, 0x8b, 8, 0xe0]),
        Detection::Recognized(Format::Gzip)
    );
}

#[test]
fn zip_probe_rejects_weak_signature_collisions() {
    assert_eq!(detect(b"PK\x07\x08"), Detection::RawCandidate);

    let mut fake_end = b"not a zip".to_vec();
    fake_end.extend_from_slice(b"PK\x05\x06");
    fake_end.extend_from_slice(&[0; 18]);
    assert_eq!(detect(&fake_end), Detection::RawCandidate);
}

#[test]
fn auto_detection_reports_an_unsupported_zlib_dictionary() {
    let mut input = zlib_header(7, 2, true).to_vec();
    input.extend_from_slice(&0_u32.to_be_bytes()); // DICTID.
    input.extend_from_slice(&[0x03, 0x00]); // Empty fixed Deflate stream.
    input.extend_from_slice(&1_u32.to_be_bytes()); // Adler-32("").

    let error = optimize(&input, Format::Auto, &Options::default()).unwrap_err();
    assert_eq!(
        error.message(),
        "preset zlib dictionaries are not supported"
    );
}

#[test]
fn auto_reports_an_unrecognized_invalid_input_without_deflate_internals() {
    let error = optimize(&[0x07], Format::Auto, &Options::default()).unwrap_err();
    assert_eq!(error.message(), "unsupported or invalid input format");

    let explicit = optimize(&[0x07], Format::Raw, &Options::default()).unwrap_err();
    assert_eq!(explicit.message(), "invalid Deflate block type");
}

#[test]
fn auto_retains_recognized_container_diagnostics() {
    let cases: &[(Vec<u8>, &str)] = &[
        (b"\x89PNG\r\n\x1a\n".to_vec(), "invalid PNG trailer"),
        (vec![0x1f, 0x8b], "invalid GZIP signature"),
        (vec![0x78, 0x01], "zlib stream too small"),
        (
            b"PK\x03\x04".to_vec(),
            "ZIP end of central directory not found",
        ),
        (
            b"PK\x01\x02".to_vec(),
            "ZIP end of central directory not found",
        ),
    ];

    for (input, expected) in cases {
        let error = optimize(input, Format::Auto, &Options::default()).unwrap_err();
        assert_eq!(error.message(), *expected);
    }
}

#[test]
fn auto_detects_a_prefixed_zip_from_its_end_record() {
    let input = prefixed_empty_zip(b"MZ self-extracting prefix");

    assert_eq!(detect(&input), Detection::Confirmed(Format::Zip));
    let optimized = optimize(&input, Format::Auto, &Options::default()).unwrap();
    assert_eq!(optimized.data, input);
}

#[test]
fn a_structural_zip_outweighs_a_zlib_like_prefix() {
    let input = prefixed_empty_zip(&[0x78, 0x01]);

    assert!(looks_like_zlib(&input));
    assert_eq!(detect(&input), Detection::Confirmed(Format::Zip));
    let optimized = optimize(&input, Format::Auto, &Options::default()).unwrap();
    assert_eq!(optimized.data, input);
}
