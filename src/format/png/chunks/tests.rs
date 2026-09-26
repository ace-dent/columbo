// SPDX-License-Identifier: MIT

use super::super::test_support::{black_scanline_zlib, chunk, frame_control, ihdr};
use super::*;
use crate::ErrorKind;

#[test]
fn names_unknown_critical_chunks_before_and_after_ihdr() {
    for strip_metadata in [false, true] {
        let mut valid = SIGNATURE.to_vec();
        valid.extend(chunk(*b"IHDR", &ihdr()));
        valid.extend(chunk(*b"IDAT", &black_scanline_zlib()));
        valid.extend(chunk(*b"IEND", &[]));
        assert!(parse(&valid, strip_metadata).is_ok());

        for (kind, expected) in [
            (*b"CgBI", "unknown PNG critical chunk: CgBI"),
            (*b"ZzZz", "unknown PNG critical chunk: ZzZz"),
        ] {
            for position in [
                SIGNATURE.len(),
                SIGNATURE.len() + chunk(*b"IHDR", &ihdr()).len(),
            ] {
                let mut input = valid.clone();
                input.splice(position..position, chunk(kind, &[]));
                let error = parse(&input, strip_metadata)
                    .err()
                    .expect("must reject unknown critical chunk");
                assert_eq!(error.kind(), ErrorKind::InvalidInput);
                assert_eq!(error.message(), expected);
            }
        }
    }
}

#[test]
fn requires_ihdr_before_known_critical_and_ancillary_chunks() {
    for kind in [*b"PLTE", *b"IDAT", *b"IEND", *b"tEXt", *b"aaAa"] {
        let mut input = SIGNATURE.to_vec();
        input.extend(chunk(kind, &[]));
        input.extend(chunk(*b"IHDR", &ihdr()));
        input.extend(chunk(*b"IDAT", &black_scanline_zlib()));
        input.extend(chunk(*b"IEND", &[]));

        for strip_metadata in [false, true] {
            let error = parse(&input, strip_metadata)
                .err()
                .expect("must require IHDR first");
            assert_eq!(error.kind(), ErrorKind::InvalidInput);
            assert_eq!(error.message(), "invalid PNG IHDR");
        }
    }
}

#[test]
fn preserves_malformed_and_duplicate_ihdr_errors() {
    let mut zero_width = ihdr();
    zero_width[..4].fill(0);
    for header in [
        chunk(*b"IHDR", &zero_width),
        chunk(*b"IHDR", &ihdr()[..12]),
        [chunk(*b"IHDR", &ihdr()), chunk(*b"IHDR", &ihdr())].concat(),
    ] {
        let mut input = SIGNATURE.to_vec();
        input.extend(header);
        input.extend(chunk(*b"IDAT", &black_scanline_zlib()));
        input.extend(chunk(*b"IEND", &[]));

        for strip_metadata in [false, true] {
            let error = parse(&input, strip_metadata)
                .err()
                .expect("must reject invalid IHDR");
            assert_eq!(error.kind(), ErrorKind::InvalidInput);
            assert_eq!(error.message(), "invalid PNG IHDR");
        }
    }
}

#[test]
fn validates_chunk_structure_and_crc_before_unknown_critical_type() {
    let mut bad_crc = chunk(*b"ZzZz", &[]);
    *bad_crc.last_mut().unwrap() ^= 1;
    let mut truncated = chunk(*b"ZzZz", &[]);
    truncated[..4].copy_from_slice(&1_u32.to_be_bytes());
    let mut excessive_length = chunk(*b"ZzZz", &[]);
    excessive_length[..4].copy_from_slice(&0x8000_0000_u32.to_be_bytes());
    for (encoded, expected_kind, expected_message) in [
        (bad_crc, ErrorKind::IntegrityMismatch, "bad PNG chunk CRC"),
        (truncated, ErrorKind::InvalidInput, "truncated PNG chunk"),
        (
            excessive_length,
            ErrorKind::InvalidInput,
            "invalid PNG chunk length",
        ),
        (
            chunk(*b"Zzzz", &[]),
            ErrorKind::InvalidInput,
            "invalid PNG chunk type",
        ),
    ] {
        for after_ihdr in [false, true] {
            let mut input = SIGNATURE.to_vec();
            if after_ihdr {
                input.extend(chunk(*b"IHDR", &ihdr()));
            }
            input.extend_from_slice(&encoded);

            for strip_metadata in [false, true] {
                let error = parse(&input, strip_metadata)
                    .err()
                    .expect("must reject malformed chunk");
                assert_eq!(error.kind(), expected_kind);
                assert_eq!(error.message(), expected_message);
            }
        }
    }
}

#[test]
fn image_decoded_size_matches_scanline_and_adam7_geometry() {
    let indexed = ParseState {
        width: 13,
        height: 7,
        bit_depth: 1,
        color_type: 3,
        ..ParseState::default()
    };
    // Each row contains one filter byte and ceil(13 / 8) data bytes.
    assert_eq!(png_image_decoded_size(&indexed).unwrap(), 21);

    let interlaced = ParseState {
        width: 8,
        height: 8,
        bit_depth: 8,
        color_type: 0,
        interlace_method: 1,
        ..ParseState::default()
    };
    // The seven filtered Adam7 passes contain 2+2+3+6+10+20+36 bytes.
    assert_eq!(png_image_decoded_size(&interlaced).unwrap(), 79);
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

    let error = match parse(&input, false) {
        Err(error) => error,
        Ok(_) => panic!("excess compressed metadata should fail"),
    };
    assert_eq!(
        error.message(),
        "PNG contains too many compressed metadata streams"
    );
}

#[test]
fn rejects_pathological_chunk_counts_early() {
    let mut input = SIGNATURE.to_vec();
    input.extend(chunk(*b"IHDR", &ihdr()));
    for _ in 0..MAX_PNG_CHUNKS {
        input.extend(chunk(*b"aaAa", &[]));
    }

    let error = match parse(&input, false) {
        Err(error) => error,
        Ok(_) => panic!("excess PNG chunks should fail"),
    };
    assert_eq!(error.message(), "PNG contains too many chunks");
}

#[test]
fn sequence_number_only_fdat_does_not_satisfy_frame_data() {
    let mut input = SIGNATURE.to_vec();
    input.extend(chunk(*b"IHDR", &ihdr()));
    let mut actl = Vec::new();
    actl.extend_from_slice(&2_u32.to_be_bytes());
    actl.extend_from_slice(&0_u32.to_be_bytes());
    input.extend(chunk(*b"acTL", &actl));
    input.extend(chunk(*b"fcTL", &frame_control(0)));
    input.extend(chunk(*b"IDAT", &black_scanline_zlib()));
    input.extend(chunk(*b"fcTL", &frame_control(1)));
    input.extend(chunk(*b"fdAT", &2_u32.to_be_bytes()));
    input.extend(chunk(*b"IEND", &[]));

    let error = match parse(&input, false) {
        Err(error) => error,
        Ok(_) => panic!("sequence-number-only fdAT should be rejected"),
    };
    assert_eq!(error.message(), "invalid APNG frame count");
}
