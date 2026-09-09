// SPDX-License-Identifier: MIT

//! Opt-in regressions using private, untracked corpus files.
//! Run with `cargo test --test local_corpus -- --ignored`.

use columbo::{optimize, Format, Options};

fn corpus_file(relative: &str) -> Vec<u8> {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(relative);
    std::fs::read(path).unwrap_or_else(|_| panic!("missing local corpus fixture: {relative}"))
}

#[test]
#[ignore = "requires the private tests/fixtures corpus"]
fn compact_png_uses_the_rle_smoothed_reduced_depth_tree_floor() {
    let optimized = optimize(
        &corpus_file("png/PngSuite/tbbn2c16.png"),
        Format::Png,
        &Options::default(),
    )
    .unwrap();

    // The pre-floor endpoint is 2,039 bytes; smoothing at depth 15 reaches
    // 2,037, and the reduced-depth frontier reaches 2,032. Keep this monotone
    // so a future improvement can make the fixture smaller.
    assert!(optimized.data.len() <= 2_032);
    assert!(optimized.bits_saved >= 9 * 8);
}

#[test]
#[ignore = "requires the private tests/fixtures corpus"]
fn rle_smoothed_tree_frontier_retains_the_depth_11_win() {
    let optimized = optimize(
        &corpus_file("png/PngSuite/bgyn6a16.png"),
        Format::Png,
        &Options::default(),
    )
    .unwrap();

    // Depths 15/10/9 stop at 3,443 bytes; depth 11 saves the next byte.
    assert!(optimized.data.len() <= 3_442);
    assert!(optimized.bits_saved >= 11 * 8);
}

#[test]
#[ignore = "requires the private tests/fixtures corpus"]
fn rle_smoothed_tree_frontier_retains_the_classic_zopfli_win() {
    let optimized = optimize(
        &corpus_file("png/PngSuite/tbrn2c08.png"),
        Format::Png,
        &Options::default(),
    )
    .unwrap();

    // The fixed-point family stops at 1,610 bytes; the classic nearby-count
    // family reaches 1,608.
    assert!(optimized.data.len() <= 1_608);
    assert!(optimized.bits_saved >= 25 * 8);
}

#[test]
#[ignore = "requires the private tests/fixtures corpus"]
fn original_match_restoration_reaches_png_output_and_the_max_default_floor() {
    let source = &corpus_file("png/PngSuite/f00n0g08.png");
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
#[ignore = "requires the private tests/fixtures corpus"]
fn payload_header_tradeoff_reaches_png_and_the_mandatory_max_floor() {
    let source = &corpus_file("png/PngSuite/basi4a16.png");
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

#[test]
#[ignore = "requires the private tests/fixtures corpus"]
fn literal_span_reaches_png_output_and_the_mandatory_max_floor() {
    let source = &corpus_file("png/PngSuite/basi0g04.png");
    let ordinary = optimize(source, Format::Png, &Options::default()).unwrap();
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
    // With no optional time, the complete PNG Default floor must include its
    // final advertised-span spelling, even when the physical byte count ties.
    assert_eq!(max.data, ordinary.data);
}
