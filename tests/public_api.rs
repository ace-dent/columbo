// SPDX-License-Identifier: MIT

//! End-to-end checks through the public, reusable Rust API.

use std::thread;

use columbo::{optimize, ErrorKind, Format, Options, MAX_EXPANSION_RATIO};

const EMPTY_RAW: &[u8] = &[0x03, 0x00];
const EMPTY_ZLIB: &[u8] = &[0x78, 0x01, 0x03, 0x00, 0x00, 0x00, 0x00, 0x01];
const NORMALIZED_EMPTY_ZLIB: &[u8] = &[0x08, 0xd7, 0x03, 0x00, 0x00, 0x00, 0x00, 0x01];
// One stored byte followed by an empty final fixed block. Its 0x78, 0x01
// prefix is also a valid RFC 1950 header, making byte-only detection ambiguous.
const ZLIB_LIKE_RAW: &[u8] = &[0x78, 0x01, 0x00, 0xfe, 0xff, b'x', 0x03, 0x00];
const RLE_SMOOTHING_PNG: &[u8] = include_bytes!("fixtures/png/PngSuite/tbbn2c16.png");
const RLE_SMOOTHING_DEPTH_11_PNG: &[u8] = include_bytes!("fixtures/png/PngSuite/bgyn6a16.png");
const CLASSIC_RLE_SMOOTHING_PNG: &[u8] = include_bytes!("fixtures/png/PngSuite/tbrn2c08.png");

#[test]
fn auto_detection_and_explicit_modes_agree() {
    let options = Options::default();
    let automatic = optimize(EMPTY_ZLIB, Format::Auto, &options).unwrap();
    let explicit = optimize(EMPTY_ZLIB, Format::Zlib, &options).unwrap();

    assert_eq!(automatic, explicit);
    assert_eq!(automatic.data, NORMALIZED_EMPTY_ZLIB);
}

#[test]
fn explicit_raw_mode_resolves_an_ambiguous_header() {
    let options = Options::default();

    assert!(optimize(ZLIB_LIKE_RAW, Format::Auto, &options).is_err());
    let explicit = optimize(ZLIB_LIKE_RAW, Format::Raw, &options).unwrap();
    assert!(!explicit.data.is_empty());
}

#[test]
fn strict_mode_is_the_default() {
    assert!(Options::default().strict);
    assert!(!Options::default().verbose);
    assert!(!Options::default().visual);
    assert_eq!(
        Options::default().max_expansion_ratio,
        Some(MAX_EXPANSION_RATIO)
    );
}

#[test]
fn reporting_modes_do_not_change_optimized_bytes() {
    let quiet = Options {
        exhaustive: true,
        ..Options::default()
    };
    let mut verbose = quiet.clone();
    verbose.verbose = true;

    let quiet_result = optimize(EMPTY_RAW, Format::Raw, &quiet).unwrap();
    let verbose_result = optimize(EMPTY_RAW, Format::Raw, &verbose).unwrap();

    assert_eq!(verbose_result, quiet_result);

    let mut visual = quiet.clone();
    visual.visual = true;
    let visual_result = optimize(EMPTY_RAW, Format::Raw, &visual).unwrap();
    assert_eq!(visual_result, quiet_result);
}

#[test]
fn relaxed_optimization_never_grows_a_stream() {
    let options = Options {
        strict: false,
        ..Options::default()
    };
    let result = optimize(EMPTY_RAW, Format::Raw, &options).unwrap();

    assert!(result.data.len() <= EMPTY_RAW.len());
}

#[test]
fn reusable_options_are_safe_across_threads() {
    let options = Options::default();
    let workers: Vec<_> = (0..4)
        .map(|_| {
            let options = options.clone();
            thread::spawn(move || optimize(EMPTY_ZLIB, Format::Zlib, &options).unwrap())
        })
        .collect();

    for worker in workers {
        assert_eq!(worker.join().unwrap().data, NORMALIZED_EMPTY_ZLIB);
    }
}

#[test]
fn errors_expose_stable_machine_readable_kinds() {
    let options = Options::default();

    assert_eq!(
        optimize(&[0x07], Format::Auto, &options)
            .unwrap_err()
            .kind(),
        ErrorKind::UnsupportedFormat
    );

    let mut bad_adler = EMPTY_ZLIB.to_vec();
    *bad_adler.last_mut().unwrap() ^= 1;
    assert_eq!(
        optimize(&bad_adler, Format::Zlib, &options)
            .unwrap_err()
            .kind(),
        ErrorKind::IntegrityMismatch
    );

    assert_eq!(
        optimize(&[0x78, 0x20, 0, 0, 0, 0], Format::Zlib, &options)
            .unwrap_err()
            .kind(),
        ErrorKind::UnsupportedFeature
    );

    let limited = Options {
        max_input_bytes: 1,
        ..options
    };
    assert_eq!(
        optimize(EMPTY_RAW, Format::Raw, &limited)
            .unwrap_err()
            .kind(),
        ErrorKind::ResourceLimit
    );
}

#[test]
fn compact_png_uses_the_rle_smoothed_reduced_depth_tree_floor() {
    let optimized = optimize(RLE_SMOOTHING_PNG, Format::Png, &Options::default()).unwrap();

    // The pre-floor endpoint is 2,039 bytes; smoothing at depth 15 reaches
    // 2,037, and the reduced-depth frontier reaches 2,032. Keep this monotone
    // so a future improvement can make the fixture smaller.
    assert!(optimized.data.len() <= 2_032);
    assert!(optimized.bits_saved >= 9 * 8);
}

#[test]
fn rle_smoothed_tree_frontier_retains_the_depth_11_win() {
    let optimized = optimize(RLE_SMOOTHING_DEPTH_11_PNG, Format::Png, &Options::default()).unwrap();

    // Depths 15/10/9 stop at 3,443 bytes; depth 11 saves the next byte.
    assert!(optimized.data.len() <= 3_442);
    assert!(optimized.bits_saved >= 11 * 8);
}

#[test]
fn rle_smoothed_tree_frontier_retains_the_classic_zopfli_win() {
    let optimized = optimize(CLASSIC_RLE_SMOOTHING_PNG, Format::Png, &Options::default()).unwrap();

    // The fixed-point family stops at 1,610 bytes; the classic nearby-count
    // family reaches 1,608.
    assert!(optimized.data.len() <= 1_608);
    assert!(optimized.bits_saved >= 25 * 8);
}

#[test]
fn original_match_restoration_reaches_png_output_and_the_max_default_floor() {
    let source = include_bytes!("fixtures/png/PngSuite/f00n0g08.png");
    let ordinary = optimize(source, Format::Png, &Options::default()).unwrap();
    // The completed pre-restoration endpoint occupied 297 bytes. Restoring
    // the original 18-byte match at distance 34 saves its next physical byte.
    assert!(ordinary.data.len() <= 296);
    for (verbose, visual) in [(true, false), (false, true)] {
        let reported = optimize(
            source,
            Format::Png,
            &Options {
                verbose,
                visual,
                ..Options::default()
            },
        )
        .unwrap();
        assert_eq!(reported, ordinary);
    }
    let max = optimize(
        source,
        Format::Png,
        &Options {
            exhaustive: true,
            timeout: std::time::Duration::ZERO,
            ..Options::default()
        },
    )
    .unwrap();
    assert!(max.data.len() <= ordinary.data.len());
    assert!(max.bits_saved >= ordinary.bits_saved);
}

#[test]
fn payload_header_tradeoff_reaches_png_and_the_mandatory_max_floor() {
    let source = include_bytes!("fixtures/png/PngSuite/basi4a16.png");
    let ordinary = optimize(source, Format::Png, &Options::default()).unwrap();
    // The completed parent was 2,827 bytes. A seven-bit payload tax removes
    // eighteen header bits, saving eleven meaningful bits and one file byte.
    assert!(ordinary.data.len() <= 2_826);
    let max = optimize(
        source,
        Format::Png,
        &Options {
            exhaustive: true,
            timeout: std::time::Duration::ZERO,
            ..Options::default()
        },
    )
    .unwrap();
    assert!(max.data.len() <= ordinary.data.len());
    assert!(max.bits_saved >= ordinary.bits_saved);
}
