// SPDX-License-Identifier: MIT

//! End-to-end checks through the public, reusable Rust API.

use std::thread;

use columbo::{optimize, Format, Options};

const EMPTY_RAW: &[u8] = &[0x03, 0x00];
const EMPTY_ZLIB: &[u8] = &[0x78, 0x01, 0x03, 0x00, 0x00, 0x00, 0x00, 0x01];

#[test]
fn auto_detection_and_explicit_modes_agree() {
    let options = Options::default();
    let automatic = optimize(EMPTY_ZLIB, Format::Auto, &options).unwrap();
    let explicit = optimize(EMPTY_ZLIB, Format::Zlib, &options).unwrap();

    assert_eq!(automatic, explicit);
    assert_eq!(automatic.data, EMPTY_ZLIB);
}

#[test]
fn normal_optimization_never_grows_a_stream() {
    let options = Options::default();
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
        assert_eq!(worker.join().unwrap().data, EMPTY_ZLIB);
    }
}
