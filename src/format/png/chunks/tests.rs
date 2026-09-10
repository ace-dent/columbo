// SPDX-License-Identifier: MIT

use super::super::test_support::{black_scanline_zlib, chunk, frame_control, ihdr};
use super::*;

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
