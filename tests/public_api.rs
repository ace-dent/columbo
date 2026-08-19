// SPDX-License-Identifier: MIT

//! End-to-end checks through the public, reusable Rust API.

use std::thread;

use columbo::{optimize, ErrorKind, Format, Options, MAX_EXPANSION_RATIO};

const EMPTY_RAW: &[u8] = &[0x03, 0x00];
const EMPTY_ZLIB: &[u8] = &[0x78, 0x01, 0x03, 0x00, 0x00, 0x00, 0x00, 0x01];
const MAXIMUM_FLEVEL_EMPTY_ZLIB: &[u8] = &[0x78, 0xda, 0x03, 0x00, 0x00, 0x00, 0x00, 0x01];
// One stored byte followed by an empty final fixed block. Its 0x78, 0x01
// prefix is also a valid RFC 1950 header, making byte-only detection ambiguous.
const ZLIB_LIKE_RAW: &[u8] = &[0x78, 0x01, 0x00, 0xfe, 0xff, b'x', 0x03, 0x00];

#[test]
fn auto_detection_and_explicit_modes_agree() {
    let options = Options::default();
    let automatic = optimize(EMPTY_ZLIB, Format::Auto, &options).unwrap();
    let explicit = optimize(EMPTY_ZLIB, Format::Zlib, &options).unwrap();

    assert_eq!(automatic, explicit);
    assert_eq!(automatic.data, MAXIMUM_FLEVEL_EMPTY_ZLIB);
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
        assert_eq!(worker.join().unwrap().data, MAXIMUM_FLEVEL_EMPTY_ZLIB);
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
