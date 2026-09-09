// SPDX-License-Identifier: MIT

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

#[test]
fn file_deadline_is_shared_in_normal_mode_too() {
    let options = Options {
        timeout: Duration::from_secs(10),
        ..Options::default()
    };
    let expired = SearchDeadline {
        started: Instant::now() - Duration::from_secs(2),
        timeout: Duration::from_secs(1),
    };

    assert_eq!(expired.options_for_call(&options).timeout, Duration::ZERO);
    assert!(expired.is_expired());

    let active = SearchDeadline {
        started: Instant::now(),
        timeout: Duration::from_secs(10),
    };
    assert!(!active.is_expired());
}

#[test]
fn only_a_child_owning_the_file_remainder_receives_global_grace() {
    let options = Options {
        timeout: Duration::from_secs(10),
        ..Options::default()
    };
    let deadline = SearchDeadline::new(&options);

    assert_eq!(
        deadline.grace_for_call(Duration::from_secs(1)),
        Duration::from_millis(200)
    );
    let remainder = deadline.remaining();
    let grace = deadline.grace_for_call(remainder);
    assert!(grace > Duration::from_millis(1_900));
    assert!(grace <= Duration::from_secs(2));
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
fn duration_scaling_handles_extreme_public_options() {
    let scaled = scale_duration(Duration::MAX, 0.98);
    assert!(scaled < Duration::MAX);
    assert_eq!(scale_duration(Duration::MAX, 1.0), Duration::MAX);
    assert_eq!(scale_duration(Duration::MAX, f64::NAN), Duration::ZERO);
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
