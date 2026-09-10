// SPDX-License-Identifier: MIT

use super::*;
use crate::checksum::test_support::adler32;
use crate::format::test_support::same_byte_bit_win_zlib;

fn optimize(input: &[u8], options: &Options) -> Result<Optimization> {
    let parsed = preflight(input, options.strip_metadata)?;
    optimize_preflight(input, options, parsed)
}

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

fn frame_control(sequence: u32) -> Vec<u8> {
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

fn feedback_ihdr() -> [u8; 13] {
    let mut data = ihdr();
    data[..4].copy_from_slice(&85_u32.to_be_bytes());
    data
}

fn assert_maximum_flevel(stream: &[u8]) {
    assert!(zlib::has_rfc1950_header(stream));
    assert_eq!(stream[1] >> 6, 3);
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
fn strip_does_not_turn_bad_metadata_crc_into_a_repair() {
    let mut input = SIGNATURE.to_vec();
    input.extend(chunk(*b"IHDR", &ihdr()));
    let mut bad_cabx = chunk(*b"caBX", b"credential");
    *bad_cabx.last_mut().unwrap() ^= 1;
    input.extend(bad_cabx);
    input.extend(chunk(*b"IDAT", &black_scanline_zlib()));
    input.extend(chunk(*b"IEND", &[]));

    for strip_metadata in [false, true] {
        let error = optimize(
            &input,
            &Options {
                strip_metadata,
                ..Options::default()
            },
        )
        .unwrap_err();
        assert_eq!(error.message(), "bad PNG chunk CRC");
    }
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
    let parsed = parse(&result.data, false).unwrap();
    assert_maximum_flevel(&parsed.idat);
}

#[test]
fn zero_budget_static_png_max_retains_default_in_bytes_and_bits() {
    let mut input = SIGNATURE.to_vec();
    input.extend(chunk(*b"IHDR", &feedback_ihdr()));
    input.extend(chunk(*b"IDAT", &feedback_zlib()));
    input.extend(chunk(*b"IEND", &[]));
    for strict in [true, false] {
        let default_options = Options {
            strict,
            timeout: Duration::from_secs(1),
            ..Options::default()
        };
        let default =
            optimize_preflight_once(&input, &default_options, parse(&input, false).unwrap())
                .unwrap();
        let maximum = optimize_preflight_once(
            &input,
            &Options {
                exhaustive: true,
                timeout: Duration::ZERO,
                ..default_options
            },
            parse(&input, false).unwrap(),
        )
        .unwrap();

        assert!(maximum.data.len() <= default.data.len(), "strict={strict}");
        assert!(
            maximum.output_deflate_bits <= default.output_deflate_bits,
            "strict={strict}"
        );
    }
}

#[test]
fn zero_budget_unraced_apng_max_retains_default_in_bytes_and_bits() {
    let zlib = feedback_zlib();
    let mut input = SIGNATURE.to_vec();
    input.extend(chunk(*b"IHDR", &feedback_ihdr()));
    let mut actl = Vec::new();
    actl.extend_from_slice(&2_u32.to_be_bytes());
    actl.extend_from_slice(&0_u32.to_be_bytes());
    input.extend(chunk(*b"acTL", &actl));
    let mut first_control = frame_control(0);
    first_control[4..8].copy_from_slice(&85_u32.to_be_bytes());
    input.extend(chunk(*b"fcTL", &first_control));
    input.extend(chunk(*b"IDAT", &zlib));
    let mut second_control = frame_control(1);
    second_control[4..8].copy_from_slice(&85_u32.to_be_bytes());
    input.extend(chunk(*b"fcTL", &second_control));
    let mut frame_data = 2_u32.to_be_bytes().to_vec();
    frame_data.extend_from_slice(&zlib);
    input.extend(chunk(*b"fdAT", &frame_data));
    input.extend(chunk(*b"IEND", &[]));

    for strict in [true, false] {
        let default_options = Options {
            strict,
            timeout: Duration::from_secs(2),
            ..Options::default()
        };
        let default =
            optimize_preflight_once(&input, &default_options, parse(&input, false).unwrap())
                .unwrap();
        let maximum = optimize_preflight_once(
            &input,
            &Options {
                exhaustive: true,
                timeout: Duration::ZERO,
                ..default_options
            },
            parse(&input, false).unwrap(),
        )
        .unwrap();

        assert!(maximum.data.len() <= default.data.len(), "strict={strict}");
        assert!(
            maximum.output_deflate_bits <= default.output_deflate_bits,
            "strict={strict}"
        );
    }
}

#[test]
fn strips_everything_after_iend_in_every_mode() {
    let mut input = SIGNATURE.to_vec();
    input.extend(chunk(*b"IHDR", &ihdr()));
    input.extend(chunk(*b"IDAT", &black_scanline_zlib()));
    input.extend(chunk(*b"IEND", &[]));
    input.extend_from_slice(b"opaque payload after the PNG datastream");

    let modes = [
        Options::default(),
        Options {
            strict: false,
            ..Options::default()
        },
        Options {
            strip_metadata: true,
            ..Options::default()
        },
        Options {
            exhaustive: true,
            timeout: Duration::ZERO,
            ..Options::default()
        },
    ];
    let iend = chunk(*b"IEND", &[]);
    for options in modes {
        let result = optimize(&input, &options).unwrap();
        assert!(result.data.ends_with(&iend));
        let saved_bytes = u64::try_from(input.len() - result.data.len()).unwrap();
        assert_eq!(result.bits_saved, saved_bytes * 8);
        let parsed = parse(&result.data, false).unwrap();
        assert_eq!(parsed.datastream_len, result.data.len());
    }
}

#[test]
fn strips_everything_after_an_apng_iend() {
    let zlib = black_scanline_zlib();
    let mut input = SIGNATURE.to_vec();
    input.extend(chunk(*b"IHDR", &ihdr()));
    let mut actl = Vec::new();
    actl.extend_from_slice(&2_u32.to_be_bytes());
    actl.extend_from_slice(&0_u32.to_be_bytes());
    input.extend(chunk(*b"acTL", &actl));
    input.extend(chunk(*b"fcTL", &frame_control(0)));
    input.extend(chunk(*b"IDAT", &zlib));
    input.extend(chunk(*b"fcTL", &frame_control(1)));
    let mut frame_data = 2_u32.to_be_bytes().to_vec();
    frame_data.extend_from_slice(&zlib);
    input.extend(chunk(*b"fdAT", &frame_data));
    input.extend(chunk(*b"IEND", &[]));
    input.extend_from_slice(b"payload after the APNG datastream");

    let result = optimize(&input, &Options::default()).unwrap();
    assert!(result.data.ends_with(&chunk(*b"IEND", &[])));
    let saved_bytes = u64::try_from(input.len() - result.data.len()).unwrap();
    assert_eq!(result.bits_saved, saved_bytes * 8);
    let parsed = parse(&result.data, false).unwrap();
    assert_eq!(parsed.datastream_len, result.data.len());
    assert_eq!(parsed.fdat_frames.len(), 1);
}

#[test]
fn accepts_and_removes_vestigial_rgba_trns_in_every_mode() {
    let mut header = ihdr();
    header[9] = 6;
    let decoded = [0, 10, 20, 30, 40];
    let mut input = SIGNATURE.to_vec();
    input.extend(chunk(*b"IHDR", &header));
    input.extend(chunk(*b"PLTE", &[0, 0, 0, 255, 255, 255]));
    input.extend(chunk(*b"tRNS", &[0, 255]));
    input.extend(chunk(*b"IDAT", &stored_zlib(&decoded)));
    input.extend(chunk(*b"IEND", &[]));

    let modes = [
        Options::default(),
        Options {
            strict: false,
            ..Options::default()
        },
        Options {
            strip_metadata: true,
            ..Options::default()
        },
        Options {
            exhaustive: true,
            timeout: Duration::ZERO,
            ..Options::default()
        },
    ];
    for options in modes {
        let result = optimize(&input, &options).unwrap();
        let parsed = parse(&result.data, false).unwrap();
        assert_eq!(parsed.chunks[0].data[9], 6);
        assert!(parsed.chunks.iter().any(|chunk| chunk.kind == *b"PLTE"));
        assert!(parsed.chunks.iter().all(|chunk| chunk.kind != *b"tRNS"));
        assert!(zlib_decodes_to(&parsed.idat, &decoded));
        let saved_bytes = u64::try_from(input.len() - result.data.len()).unwrap();
        assert_eq!(result.bits_saved, saved_bytes * 8);
    }
}

#[test]
fn removes_vestigial_rgba_trns_and_trailing_bytes_from_apng() {
    let mut header = ihdr();
    header[9] = 6;
    let decoded = [0, 10, 20, 30, 40];
    let zlib = stored_zlib(&decoded);
    let mut input = SIGNATURE.to_vec();
    input.extend(chunk(*b"IHDR", &header));
    let mut actl = Vec::new();
    actl.extend_from_slice(&2_u32.to_be_bytes());
    actl.extend_from_slice(&0_u32.to_be_bytes());
    input.extend(chunk(*b"acTL", &actl));
    input.extend(chunk(*b"PLTE", &[0, 0, 0]));
    input.extend(chunk(*b"tRNS", &[0]));
    input.extend(chunk(*b"fcTL", &frame_control(0)));
    input.extend(chunk(*b"IDAT", &zlib));
    input.extend(chunk(*b"fcTL", &frame_control(1)));
    let mut frame_data = 2_u32.to_be_bytes().to_vec();
    frame_data.extend_from_slice(&zlib);
    input.extend(chunk(*b"fdAT", &frame_data));
    input.extend(chunk(*b"IEND", &[]));
    input.extend_from_slice(b"payload after IEND");

    let result = optimize(&input, &Options::default()).unwrap();
    let parsed = parse(&result.data, false).unwrap();
    assert_eq!(parsed.datastream_len, result.data.len());
    assert!(parsed.chunks.iter().all(|chunk| chunk.kind != *b"tRNS"));
    assert!(zlib_decodes_to(&parsed.idat, &decoded));
    assert_eq!(parsed.fdat_frames.len(), 1);
    assert!(zlib_decodes_to(&parsed.fdat_frames[0], &decoded));
    let saved_bytes = u64::try_from(input.len() - result.data.len()).unwrap();
    assert_eq!(result.bits_saved, saved_bytes * 8);
}

#[test]
fn valid_indexed_trns_remains_pixel_semantics_even_with_strip() {
    let mut header = ihdr();
    header[9] = 3;
    let mut input = SIGNATURE.to_vec();
    input.extend(chunk(*b"IHDR", &header));
    input.extend(chunk(*b"PLTE", &[0, 0, 0]));
    input.extend(chunk(*b"tRNS", &[0]));
    input.extend(chunk(*b"IDAT", &black_scanline_zlib()));
    input.extend(chunk(*b"IEND", &[]));

    for strip_metadata in [false, true] {
        let result = optimize(
            &input,
            &Options {
                strip_metadata,
                ..Options::default()
            },
        )
        .unwrap();
        let parsed = parse(&result.data, false).unwrap();
        assert!(parsed.chunks.iter().any(|chunk| chunk.kind == *b"tRNS"));
    }
}

#[test]
fn rejects_rgba_trns_outside_the_vestigial_palette_signature() {
    let mut header = ihdr();
    header[9] = 6;
    let cases: [(&[u8], Option<&[u8]>); 3] = [
        (&[0], None),
        (&[], Some(&[0, 0, 0])),
        (&[0, 255], Some(&[0, 0, 0])),
    ];

    for (transparency, palette) in cases {
        let mut input = SIGNATURE.to_vec();
        input.extend(chunk(*b"IHDR", &header));
        if let Some(palette) = palette {
            input.extend(chunk(*b"PLTE", palette));
        }
        input.extend(chunk(*b"tRNS", transparency));
        input.extend(chunk(*b"IDAT", &stored_zlib(&[0, 0, 0, 0, 0])));
        input.extend(chunk(*b"IEND", &[]));

        for strip_metadata in [false, true] {
            let error = optimize(
                &input,
                &Options {
                    strip_metadata,
                    ..Options::default()
                },
            )
            .unwrap_err();
            assert_eq!(error.message(), "invalid PNG tRNS");
        }
    }
}

#[test]
fn rejects_vestigial_rgba_trns_after_idat_or_when_duplicated() {
    let mut header = ihdr();
    header[9] = 6;
    let idat = chunk(*b"IDAT", &stored_zlib(&[0, 0, 0, 0, 0]));
    let trns = chunk(*b"tRNS", &[0]);

    let mut after_idat = SIGNATURE.to_vec();
    after_idat.extend(chunk(*b"IHDR", &header));
    after_idat.extend(chunk(*b"PLTE", &[0, 0, 0]));
    after_idat.extend(&idat);
    after_idat.extend(&trns);
    after_idat.extend(chunk(*b"IEND", &[]));

    let mut duplicated = SIGNATURE.to_vec();
    duplicated.extend(chunk(*b"IHDR", &header));
    duplicated.extend(chunk(*b"PLTE", &[0, 0, 0]));
    duplicated.extend(&trns);
    duplicated.extend(&trns);
    duplicated.extend(&idat);
    duplicated.extend(chunk(*b"IEND", &[]));

    for input in [&after_idat, &duplicated] {
        let error = optimize(input, &Options::default()).unwrap_err();
        assert_eq!(error.message(), "invalid PNG tRNS");
    }
}

#[test]
fn vestigial_rgba_trns_does_not_hide_a_wrong_color_mode() {
    let mut header = ihdr();
    header[9] = 6;
    let mut input = SIGNATURE.to_vec();
    input.extend(chunk(*b"IHDR", &header));
    input.extend(chunk(*b"PLTE", &[0, 0, 0]));
    input.extend(chunk(*b"tRNS", &[0]));
    // This is a two-byte grayscale scanline, not the five bytes required
    // for a one-pixel, eight-bit RGBA scanline.
    input.extend(chunk(*b"IDAT", &black_scanline_zlib()));
    input.extend(chunk(*b"IEND", &[]));

    let error = optimize(&input, &Options::default()).unwrap_err();
    assert_eq!(error.message(), "PNG image data size does not match IHDR");
}

#[test]
fn still_rejects_trns_for_grayscale_alpha() {
    let mut header = ihdr();
    header[9] = 4;
    let mut input = SIGNATURE.to_vec();
    input.extend(chunk(*b"IHDR", &header));
    input.extend(chunk(*b"tRNS", &[0]));
    input.extend(chunk(*b"IDAT", &stored_zlib(&[0, 0, 0])));
    input.extend(chunk(*b"IEND", &[]));

    let error = optimize(&input, &Options::default()).unwrap_err();
    assert_eq!(error.message(), "invalid PNG tRNS");
}

#[test]
fn static_png_has_one_physical_deflate_stream() {
    let mut input = SIGNATURE.to_vec();
    input.extend(chunk(*b"IHDR", &ihdr()));
    input.extend(chunk(*b"IDAT", &black_scanline_zlib()));
    input.extend(chunk(*b"IEND", &[]));

    assert_eq!(deflate_stream_count(&input, false).unwrap(), 1);
}

#[test]
fn compressed_metadata_counts_after_image_streams() {
    let mut input = SIGNATURE.to_vec();
    input.extend(chunk(*b"IHDR", &ihdr()));
    input.extend(chunk(*b"IDAT", &black_scanline_zlib()));
    let mut text = b"Comment\0\0".to_vec();
    text.extend(black_scanline_zlib());
    input.extend(chunk(*b"zTXt", &text));
    input.extend(chunk(*b"IEND", &[]));

    assert_eq!(deflate_stream_count(&input, false).unwrap(), 2);
    let parsed = parse(&input, false).unwrap();
    assert_eq!(
        metadata_stream_ids(&parsed)
            .unwrap()
            .into_iter()
            .flatten()
            .collect::<Vec<_>>(),
        vec![2]
    );
}

#[test]
fn duplicate_apng_frames_share_a_named_visual_group() {
    assert_eq!(
        image_job_stream_group(ImageJob::Idat, &[0, 0, 2]),
        (1, vec![])
    );
    assert_eq!(
        image_job_stream_group(ImageJob::Frame(0), &[0, 0, 2]),
        (2, vec![3])
    );
    assert_eq!(
        image_job_stream_group(ImageJob::Frame(2), &[0, 0, 2]),
        (4, vec![])
    );
}

#[test]
fn idat_reports_same_byte_bit_savings() {
    let mut header = ihdr();
    // The synthetic raw stream expands to 168 bytes. Model that as one
    // filter byte followed by 167 grayscale samples.
    header[..4].copy_from_slice(&167_u32.to_be_bytes());
    let mut input = SIGNATURE.to_vec();
    input.extend(chunk(*b"IHDR", &header));
    input.extend(chunk(*b"IDAT", &same_byte_bit_win_zlib()));
    input.extend(chunk(*b"IEND", &[]));

    let optimized = optimize(&input, &Options::default()).unwrap();

    assert_eq!(optimized.data.len(), input.len());
    assert_eq!(optimized.bits_saved, 1);
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
    parse(&result.data, false).unwrap();
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
fn rejects_idat_size_mismatch_in_default_and_max() {
    let mut input = SIGNATURE.to_vec();
    input.extend(chunk(*b"IHDR", &ihdr()));
    // A 1x1 eight-bit grayscale image requires exactly two bytes: one
    // filter byte and one sample. The checksum is valid for three bytes.
    input.extend(chunk(*b"IDAT", &stored_zlib(&[0, 0, 0])));
    input.extend(chunk(*b"IEND", &[]));

    for exhaustive in [false, true] {
        let options = Options {
            exhaustive,
            timeout: Duration::ZERO,
            ..Options::default()
        };
        let error = optimize(&input, &options).unwrap_err();
        assert_eq!(error.message(), "PNG image data size does not match IHDR");
    }
}

#[test]
fn parallel_max_image_requires_bounded_input_and_decoded_work() {
    assert!(parallel_max_image_is_bounded(
        PARALLEL_MAX_IMAGE_COMPRESSED,
        PARALLEL_MAX_IMAGE_DECODED,
    ));
    assert!(!parallel_max_image_is_bounded(
        PARALLEL_MAX_IMAGE_COMPRESSED + 1,
        PARALLEL_MAX_IMAGE_DECODED,
    ));
    assert!(!parallel_max_image_is_bounded(
        PARALLEL_MAX_IMAGE_COMPRESSED,
        PARALLEL_MAX_IMAGE_DECODED + 1,
    ));
    assert!(parallel_multi_image_is_bounded(
        PARALLEL_MAX_IMAGE_COMPRESSED,
        PARALLEL_MAX_IMAGE_DECODED,
    ));
    assert!(!parallel_multi_image_is_bounded(
        PARALLEL_MAX_IMAGE_COMPRESSED + 1,
        PARALLEL_MAX_IMAGE_DECODED,
    ));
}

#[test]
fn multi_image_apng_uses_mode_specific_raw_floor_policies() {
    assert_eq!(
        image_default_floor(true, false),
        DefaultFloor::CompleteThenBounded
    );
    assert_eq!(
        image_default_floor(true, true),
        DefaultFloor::CompleteThenBounded
    );
    assert_eq!(image_default_floor(false, false), DefaultFloor::ApngDefault);
    assert_eq!(image_default_floor(false, true), DefaultFloor::ApngMax);
}

#[test]
fn uncached_metadata_max_uses_an_exact_shared_default_floor() {
    assert_eq!(metadata_default_floor(false), DefaultFloor::Shared);
    assert_eq!(metadata_default_floor(true), DefaultFloor::SharedExact);
}

#[test]
fn cached_metadata_max_requires_byte_and_bit_dominance() {
    let candidate = |length, bits| CompressedBodyOptimization {
        replacement: Some(vec![0; length]),
        source_deflate_bits: 120,
        output_deflate_bits: bits,
        decoded_size: Some(1),
    };
    let floor = candidate(9, 100);

    assert!(!compressed_body_strictly_dominates(
        &candidate(8, 101),
        &floor,
        10
    ));
    assert!(compressed_body_strictly_dominates(
        &candidate(8, 99),
        &floor,
        10
    ));
}

#[test]
fn parallel_image_timeout_accounts_for_each_child_grace() {
    let timeout = parallel_image_job_timeout(Duration::from_secs(10), 2, 1, 2);
    assert!(timeout >= Duration::from_millis(4_540));
    assert!(timeout <= Duration::from_millis(4_550));
    assert_eq!(
        parallel_image_job_timeout(Duration::from_secs(10), 1, 1, 1),
        Duration::from_secs(10)
    );
}

#[test]
fn parallel_max_selection_is_byte_first_then_bit_first() {
    let stream = |length, bits, timed_out| zlib::StreamOptimization {
        data: vec![0; length],
        info: Some(RawInfo {
            deflate_bits: bits,
            ..RawInfo::default()
        }),
        timed_out,
    };

    let selected = best_zlib_optimization(stream(10, 100, false), stream(10, 99, true));
    assert_eq!(selected.info.unwrap().deflate_bits, 99);
    assert!(selected.timed_out);

    let selected = best_zlib_optimization(stream(10, 90, true), stream(9, 100, false));
    assert_eq!(selected.data.len(), 9);
    assert!(selected.timed_out);
}

#[test]
fn apng_max_file_floor_requires_no_worse_bytes_and_bits() {
    let result = |tag, length, bits, timed_out| PngOptimization {
        data: vec![tag; length],
        source_deflate_bits: 120,
        output_deflate_bits: bits,
        timed_out,
    };

    let selected =
        retain_dominating_maximum(result(1, 10, 100, false), result(2, 9, 101, false), false);
    assert_eq!(selected.data[0], 1, "a Deflate-bit regression loses");

    let selected =
        retain_dominating_maximum(result(1, 10, 100, false), result(2, 11, 90, false), false);
    assert_eq!(selected.data[0], 1, "a byte regression loses");

    let selected =
        retain_dominating_maximum(result(1, 10, 100, false), result(2, 10, 99, false), true);
    assert_eq!(selected.data[0], 2, "a same-byte bit win is retained");
    assert!(selected.timed_out, "the overall Max deadline is retained");

    let selected =
        retain_dominating_maximum(result(1, 10, 100, true), result(2, 9, 99, false), false);
    assert_eq!(selected.data[0], 2, "a win in both metrics is retained");
    assert!(selected.timed_out, "the Default floor timeout is retained");
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

    let parsed = parse(&result.data, false).unwrap();
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
        deadline: SearchDeadline::new(&options),
    };

    let error = optimize_image_streams(
        &black_scanline_zlib(),
        2,
        &[invalid_frame],
        &[0],
        false,
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
fn cross_frame_reuse_preserves_each_members_source_bit_count() {
    let source = stored_zlib(&[0, 0]);
    let optimized = zlib::optimize_embedded(
        &source,
        &Options::default(),
        2,
        false,
        DefaultFloor::Complete,
    )
    .unwrap();
    assert!(optimized.data.len() < source.len());
    let best_info = optimized.info.unwrap();
    let mut source_info = best_info.clone();
    source_info.source_deflate_bits = 101;
    source_info.deflate_bits = 101;
    let mut donor_info = best_info;
    donor_info.source_deflate_bits = 202;
    let donor_bits = donor_info.deflate_bits;
    let mut frames = vec![
        FrameOptimization {
            data: source,
            info: Some(source_info),
        },
        FrameOptimization {
            data: optimized.data,
            info: Some(donor_info),
        },
    ];

    reuse_best_exact_frames(&mut frames, &mut || false);

    let replaced = frames[0].info.as_ref().unwrap();
    assert_eq!(replaced.source_deflate_bits, 101);
    assert_eq!(replaced.deflate_bits, donor_bits);
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
    assert_eq!(
        frame_representatives(&frames, &[1; 5]).unwrap(),
        [0, 1, 0, 1, 4]
    );
    assert_eq!(
        frame_representatives(&frames, &[1, 1, 2, 1, 1]).unwrap(),
        [0, 1, 2, 1, 4]
    );
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
fn image_timeout_is_proportional_and_gives_largest_the_remainder() {
    let configured = Duration::from_secs(10);
    let remaining = Duration::from_secs(8);

    assert_eq!(
        image_stream_timeout(
            configured,
            remaining,
            2,
            10,
            NON_LARGEST_IMAGE_SEARCH_FRACTION,
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
            true,
        ),
        Duration::ZERO
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
            deadline: SearchDeadline::new(&options),
        };

        let started = std::time::Instant::now();
        let (optimized_idat, optimized_frames) = optimize_image_streams(
            &source_idat,
            2,
            &frames,
            &[2; 12],
            false,
            &options,
            &mut budget,
        )
        .unwrap();

        assert!(started.elapsed() < Duration::from_secs(2));
        assert_eq!(optimized_frames.len(), frames.len());
        assert!(optimized_idat.data.len() <= source_idat.len());
        assert!(optimized_frames
            .iter()
            .zip(&source_lengths)
            .all(|(frame, &source_len)| frame.data.len() <= source_len));
    }
}

#[test]
fn image_scheduler_reclaims_a_local_slice_without_a_file_timeout() {
    let idat = black_scanline_zlib();
    let slice_options = Options {
        timeout: Duration::ZERO,
        ..Options::default()
    };
    let initial = run_png_image_zlib(&idat, &slice_options, 2, DefaultFloor::Shared).unwrap();
    assert!(initial.timed_out);

    let options = Options {
        timeout: Duration::from_secs(1),
        ..Options::default()
    };
    let deadline = SearchDeadline::new(&options);
    let mut results = vec![(ImageJob::Idat, initial)];
    reclaim_timed_out_image_jobs(
        &mut results,
        &idat,
        2,
        &[],
        &[],
        &[],
        &[],
        DefaultFloor::Shared,
        false,
        &options,
        &deadline,
    )
    .unwrap();

    assert!(!results[0].1.timed_out);
    assert!(!deadline.is_expired());
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
        deadline: SearchDeadline::new(&options),
    };
    let error = optimize_image_streams(&idat, 0, &frames, &[2, 2], false, &options, &mut budget)
        .unwrap_err();
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
        deadline: SearchDeadline::new(&options),
    };
    optimize_image_streams(&idat, 0, &frames, &[2, 2], false, &options, &mut budget).unwrap();
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
    let parsed = parse(&result.data, false).unwrap();
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
fn uncompressed_itxt_is_preserved_without_entering_zlib_optimization() {
    let metadata = b"Comment\0\0\0\0\0plain UTF-8 text";
    let mut input = SIGNATURE.to_vec();
    input.extend(chunk(*b"IHDR", &ihdr()));
    input.extend(chunk(*b"iTXt", metadata));
    input.extend(chunk(*b"IDAT", &black_scanline_zlib()));
    input.extend(chunk(*b"IEND", &[]));

    let result = optimize(&input, &Options::default()).unwrap();
    let parsed = parse(&result.data, false).unwrap();
    let preserved = parsed
        .chunks
        .iter()
        .find(|chunk| chunk.kind == *b"iTXt")
        .expect("uncompressed iTXt should be preserved");
    assert_eq!(preserved.data, metadata);
}

#[test]
fn preserves_png_datastream_with_unknown_unsafe_ancillary_chunk() {
    let zlib = black_scanline_zlib();
    let mut input = SIGNATURE.to_vec();
    input.extend(chunk(*b"IHDR", &ihdr()));
    // Ancillary (lowercase first byte), unknown, and unsafe to copy after
    // critical-data changes (uppercase fourth byte).
    input.extend(chunk(*b"vpAG", b"private contract"));
    input.extend(chunk(*b"IDAT", &zlib[..5]));
    input.extend(chunk(*b"IDAT", &zlib[5..]));
    input.extend(chunk(*b"IEND", &[]));
    let datastream = input.clone();
    let trailing = b"payload after IEND";
    input.extend_from_slice(trailing);

    let result = optimize(&input, &Options::default()).unwrap();
    assert_eq!(result.data, datastream);
    assert_eq!(result.bits_saved, trailing.len() as u64 * 8);
}

#[test]
fn preserves_or_explicitly_strips_rewrite_sensitive_metadata() {
    for (kind, data) in [
        (*b"caBX", b"credential".as_slice()),
        (*b"dSIG", b"signature".as_slice()),
        (*b"iDOT", &[0_u8; 28][..]),
    ] {
        let zlib = black_scanline_zlib();
        let mut input = SIGNATURE.to_vec();
        input.extend(chunk(*b"IHDR", &ihdr()));
        input.extend(chunk(kind, data));
        input.extend(chunk(*b"IDAT", &zlib[..5]));
        input.extend(chunk(*b"IDAT", &zlib[5..]));
        input.extend(chunk(*b"IEND", &[]));

        let preserved = optimize(&input, &Options::default()).unwrap();
        assert_eq!(preserved.data, input, "{}", String::from_utf8_lossy(&kind));

        let stripped = optimize(
            &input,
            &Options {
                strip_metadata: true,
                ..Options::default()
            },
        )
        .unwrap();
        let parsed = parse(&stripped.data, false).unwrap();
        assert!(parsed.chunks.iter().all(|chunk| chunk.kind != kind));
        assert_eq!(
            parsed
                .chunks
                .iter()
                .filter(|chunk| chunk.kind == *b"IDAT")
                .count(),
            1
        );
    }
}

#[test]
fn vestigial_rgba_trns_requires_strip_with_rewrite_sensitive_metadata() {
    let mut header = ihdr();
    header[9] = 6;
    let mut input = SIGNATURE.to_vec();
    input.extend(chunk(*b"IHDR", &header));
    input.extend(chunk(*b"vpAG", b"private contract"));
    input.extend(chunk(*b"PLTE", &[0, 0, 0]));
    input.extend(chunk(*b"tRNS", &[0]));
    input.extend(chunk(*b"IDAT", &stored_zlib(&[0, 0, 0, 0, 0])));
    input.extend(chunk(*b"IEND", &[]));

    let error = optimize(&input, &Options::default()).unwrap_err();
    assert_eq!(
        error.message(),
        "cannot remove invalid PNG tRNS while preserving rewrite-sensitive metadata"
    );

    let result = optimize(
        &input,
        &Options {
            strip_metadata: true,
            ..Options::default()
        },
    )
    .unwrap();
    let parsed = parse(&result.data, false).unwrap();
    assert!(parsed
        .chunks
        .iter()
        .all(|chunk| !matches!(&chunk.kind, b"tRNS" | b"vpAG")));
}

#[test]
fn invalid_cabx_is_removed_only_in_strip_mode() {
    for layout in 0..3 {
        let zlib = black_scanline_zlib();
        let mut input = SIGNATURE.to_vec();
        input.extend(chunk(*b"IHDR", &ihdr()));
        if layout == 0 {
            input.extend(chunk(*b"caBX", b"first"));
            input.extend(chunk(*b"caBX", b"second"));
        }
        if layout == 2 {
            input.extend(chunk(*b"IDAT", &zlib[..5]));
            input.extend(chunk(*b"caBX", b"interrupted IDAT"));
            input.extend(chunk(*b"IDAT", &zlib[5..]));
        } else {
            input.extend(chunk(*b"IDAT", &zlib));
        }
        if layout == 1 {
            input.extend(chunk(*b"caBX", b"misordered"));
        }
        input.extend(chunk(*b"IEND", &[]));

        let error = optimize(&input, &Options::default()).unwrap_err();
        assert_eq!(error.message(), "invalid PNG caBX");
        assert!(deflate_stream_count(&input, false).is_err());

        let options = Options {
            strip_metadata: true,
            ..Options::default()
        };
        assert_eq!(deflate_stream_count(&input, true).unwrap(), 1);
        let stripped = optimize(&input, &options).unwrap();
        let parsed = parse(&stripped.data, false).unwrap();
        assert!(!parsed.chunks.iter().any(|chunk| chunk.kind == *b"caBX"));
    }
}

#[test]
fn malformed_supported_metadata_is_removed_only_in_strip_mode() {
    let mut input = SIGNATURE.to_vec();
    input.extend(chunk(*b"IHDR", &ihdr()));
    input.extend(chunk(*b"sCAL", b"\x01width\0"));
    input.extend(chunk(*b"IDAT", &black_scanline_zlib()));
    input.extend(chunk(*b"IEND", &[]));

    let error = optimize(&input, &Options::default()).unwrap_err();
    assert_eq!(error.message(), "invalid PNG sCAL");

    let stripped = optimize(
        &input,
        &Options {
            strip_metadata: true,
            ..Options::default()
        },
    )
    .unwrap();
    let parsed = parse(&stripped.data, false).unwrap();
    assert!(!parsed.chunks.iter().any(|chunk| chunk.kind == *b"sCAL"));
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

    input.extend(chunk(*b"fcTL", &frame_control(0)));
    input.extend(chunk(*b"IDAT", &zlib));
    input.extend(chunk(*b"fcTL", &frame_control(1)));
    let mut frame_data = 2_u32.to_be_bytes().to_vec();
    frame_data.extend_from_slice(&zlib);
    input.extend(chunk(*b"fdAT", &frame_data));
    input.extend(chunk(*b"IEND", &[]));

    let result = optimize(&input, &Options::default()).unwrap();
    assert!(result.data.len() <= input.len());
    let parsed = parse(&result.data, false).unwrap();
    assert_eq!(parsed.fdat_frames.len(), 1);
    assert_eq!(parsed.fdat_decoded_sizes, [2]);
    assert_maximum_flevel(&parsed.idat);
    assert_maximum_flevel(&parsed.fdat_frames[0]);
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

#[test]
fn relaxed_compressed_metadata_retains_flevel_only_improvement() {
    let empty_zlib = [0x78, 0x01, 0x03, 0x00, 0x00, 0x00, 0x00, 0x01];
    let mut metadata = b"Comment\0\0".to_vec();
    metadata.extend_from_slice(&empty_zlib);
    let mut input = SIGNATURE.to_vec();
    input.extend(chunk(*b"IHDR", &ihdr()));
    input.extend(chunk(*b"zTXt", &metadata));
    input.extend(chunk(*b"IDAT", &black_scanline_zlib()));
    input.extend(chunk(*b"IEND", &[]));

    let result = optimize(
        &input,
        &Options {
            strict: false,
            ..Options::default()
        },
    )
    .unwrap();
    let parsed = parse(&result.data, false).unwrap();
    let metadata = parsed
        .chunks
        .iter()
        .find(|chunk| chunk.kind == *b"zTXt")
        .expect("compressed metadata should be retained");
    let offset = compressed_zlib_offset(metadata.kind, metadata.data).unwrap();
    assert_maximum_flevel(&metadata.data[offset..]);
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
    assert!(!result.timed_out);
    parse(&result.data, false).unwrap();
}

#[test]
fn metadata_probe_policy_is_identical_in_detailed_modes() {
    let metadata_zlib = [0x78, 0x9c, 0xab, 0x00, 0x00, 0x00, 0x79, 0x00, 0x79];
    let mut metadata = b"Comment\0\0".to_vec();
    metadata.extend_from_slice(&metadata_zlib);

    let mut input = SIGNATURE.to_vec();
    input.extend(chunk(*b"IHDR", &ihdr()));
    input.extend(chunk(*b"zTXt", &metadata));
    input.extend(chunk(*b"IDAT", &black_scanline_zlib()));
    input.extend(chunk(*b"IEND", &[]));

    let quiet_options = Options {
        max_decoded_bytes: 3,
        ..Options::default()
    };
    let quiet = optimize(&input, &quiet_options).unwrap();

    let mut verbose_options = quiet_options.clone();
    verbose_options.verbose = true;
    let verbose = optimize(&input, &verbose_options).unwrap();

    let mut visual_options = quiet_options;
    visual_options.visual = true;
    let visual = optimize(&input, &visual_options).unwrap();

    assert_eq!(verbose.data, quiet.data);
    assert_eq!(verbose.bits_saved, quiet.bits_saved);
    assert_eq!(visual.data, quiet.data);
    assert_eq!(visual.bits_saved, quiet.bits_saved);
}

#[test]
fn reporting_modes_do_not_change_the_mandatory_metadata_reserve() {
    for (exhaustive, metadata_bytes, expected) in
        [(false, 0, false), (false, 1, true), (true, 1, false)]
    {
        let quiet = Options {
            exhaustive,
            ..Options::default()
        };
        let mut verbose = quiet.clone();
        verbose.verbose = true;
        let mut visual = quiet.clone();
        visual.visual = true;

        assert_eq!(
            image_work_needs_metadata_reserve(&quiet, metadata_bytes),
            expected
        );
        assert_eq!(
            image_work_needs_metadata_reserve(&verbose, metadata_bytes),
            expected
        );
        assert_eq!(
            image_work_needs_metadata_reserve(&visual, metadata_bytes),
            expected
        );
    }
}

#[test]
fn parallel_max_metadata_floor_preserves_the_combined_decode_budget() {
    let metadata_zlib = [0x78, 0x9c, 0xab, 0x00, 0x00, 0x00, 0x79, 0x00, 0x79];
    let mut metadata = b"Comment\0\0".to_vec();
    metadata.extend_from_slice(&metadata_zlib);

    let mut input = SIGNATURE.to_vec();
    input.extend(chunk(*b"IHDR", &ihdr()));
    input.extend(chunk(*b"zTXt", &metadata));
    input.extend(chunk(*b"IDAT", &black_scanline_zlib()));
    input.extend(chunk(*b"IEND", &[]));

    let options = Options {
        exhaustive: true,
        timeout: Duration::from_millis(20),
        max_decoded_bytes: 3,
        ..Options::default()
    };
    let result = optimize(&input, &options).unwrap();
    parse(&result.data, false).unwrap();

    let error = optimize(
        &input,
        &Options {
            max_decoded_bytes: 2,
            ..options
        },
    )
    .unwrap_err();
    assert_eq!(
        error.message(),
        "decoded PNG data exceeds configured safety limit"
    );
}

#[test]
fn bounded_max_precomputes_complete_metadata_floors() {
    let mut metadata = b"Comment\0\0".to_vec();
    metadata.extend_from_slice(&same_byte_bit_win_zlib());
    let mut input = SIGNATURE.to_vec();
    input.extend(chunk(*b"IHDR", &ihdr()));
    input.extend(chunk(*b"zTXt", &metadata));
    input.extend(chunk(*b"IDAT", &black_scanline_zlib()));
    input.extend(chunk(*b"IEND", &[]));

    let options = Options {
        exhaustive: true,
        timeout: Duration::ZERO,
        ..Options::default()
    };
    let parsed = parse(&input, false).unwrap();
    let stream_ids = metadata_stream_ids(&parsed).unwrap();
    let mut cached = Vec::new();
    cached.resize_with(parsed.chunks.len(), || None);
    let mut budget = DecodeBudget {
        remaining: options.max_decoded_bytes,
        deadline: SearchDeadline::new(&options),
    };

    precompute_max_metadata_floors(&parsed, &stream_ids, &options, &mut budget, &mut cached)
        .unwrap();

    let metadata_index = parsed
        .chunks
        .iter()
        .position(|chunk| chunk.kind == *b"zTXt")
        .unwrap();
    let floor = cached[metadata_index]
        .as_ref()
        .expect("Max should retain a bounded complete metadata floor");
    assert!(floor.replacement.is_some());
    assert!(floor.output_deflate_bits < floor.source_deflate_bits);
    assert_eq!(floor.decoded_size, Some(168));
    assert_eq!(
        options.max_decoded_bytes - budget.remaining,
        168,
        "the cached floor should charge decoded metadata exactly once"
    );

    let remaining_after_floor = budget.remaining;
    let metadata_chunk = &parsed.chunks[metadata_index];
    let refined = refine_cached_compressed_body(
        metadata_chunk.kind,
        metadata_chunk.data,
        floor,
        &options,
        &mut budget,
    )
    .unwrap();
    assert_eq!(
        budget.remaining, remaining_after_floor,
        "a Max descendant must not charge the same decoded stream twice"
    );
    let floor_len = floor
        .replacement
        .as_ref()
        .map_or(metadata_chunk.data.len(), Vec::len);
    let refined_len = refined
        .replacement
        .as_ref()
        .map_or(metadata_chunk.data.len(), Vec::len);
    assert!(
        (refined_len, refined.output_deflate_bits) <= (floor_len, floor.output_deflate_bits),
        "Max must retain the complete metadata floor"
    );
}

#[test]
fn compressed_metadata_contributes_same_byte_bit_savings_in_both_policies() {
    let mut metadata = b"Comment\0\0".to_vec();
    // This source is one bit behind strict output and three bits behind
    // relaxed output, without changing the byte length.
    metadata.extend_from_slice(&same_byte_bit_win_zlib());
    let image = zlib::optimize_embedded(
        &black_scanline_zlib(),
        &Options::default(),
        2,
        false,
        DefaultFloor::Complete,
    )
    .unwrap()
    .data;
    let mut input = SIGNATURE.to_vec();
    input.extend(chunk(*b"IHDR", &ihdr()));
    input.extend(chunk(*b"zTXt", &metadata));
    input.extend(chunk(*b"IDAT", &image));
    input.extend(chunk(*b"IEND", &[]));

    for strict in [true, false] {
        let optimized = optimize(
            &input,
            &Options {
                strict,
                ..Options::default()
            },
        )
        .unwrap();

        assert_eq!(optimized.data.len(), input.len());
        assert_eq!(optimized.bits_saved, if strict { 1 } else { 3 });
    }
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
        deadline: SearchDeadline::new(&options),
    };

    let retained = optimize_png_zlib(
        &lookalike,
        &options,
        true,
        DefaultFloor::Shared,
        &mut budget,
    )
    .unwrap();
    assert_eq!(retained.data, lookalike);
    assert_eq!(budget.remaining, 0);

    let error = optimize_png_zlib(
        &lookalike,
        &options,
        true,
        DefaultFloor::Shared,
        &mut budget,
    )
    .unwrap_err();
    assert_eq!(
        error.message(),
        "decoded PNG data exceeds configured safety limit"
    );
}
