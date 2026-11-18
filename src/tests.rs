// SPDX-License-Identifier: MIT
// SPDX-FileCopyrightText: 2026 Andrew C.E. Dent <https://github.com/ace-dent>

use std::time::Duration;

use super::*;

#[test]
fn write_savings_are_byte_first_then_meaningful_bits() {
    let shorter = Optimization::from_metrics(2, vec![0], 7, 99, false);
    assert_eq!(shorter.bits_saved, 8);
    assert!(shorter.should_replace());

    let bit_only = Optimization::from_metrics(2, vec![0; 2], 7, 6, false);
    assert_eq!(bit_only.bits_saved, 1);

    let tied = Optimization::from_metrics(2, vec![0; 2], 7, 7, false);
    assert_eq!(tied.bits_saved, 0);
    assert!(!tied.should_replace());

    let larger = Optimization::from_metrics(2, vec![0; 3], 99, 1, false);
    assert_eq!(larger.bits_saved, 0);
    assert!(!larger.should_replace());

    let mut normalized = Optimization::from_metrics(2, vec![1; 2], 7, 7, false);
    normalized.require_rewrite();
    assert_eq!(normalized.bits_saved, 0);
    assert!(normalized.should_replace());
}

#[test]
fn a_later_pass_adds_its_savings_to_the_first() {
    // Passes that keep the source's length add their bits.
    let mut result = Optimization::from_metrics(10, vec![0; 10], 80, 77, false);
    let later = Optimization::from_metrics(10, vec![1; 10], 77, 75, true);
    assert!(result.replace_with_later_pass(10, later));
    assert_eq!(result.bits_saved, 5);
    assert_eq!(result.data, [1; 10]);
    assert!(result.timed_out);

    // A shorter file counts bytes against the first input.
    let later = Optimization::from_metrics(10, vec![2; 8], 75, 60, false);
    assert!(result.replace_with_later_pass(10, later));
    assert_eq!(result.bits_saved, 16);
    assert!(result.timed_out);

    // A pass that saves nothing leaves the result alone.
    let later = Optimization::from_metrics(8, vec![3; 8], 60, 60, false);
    assert!(!result.replace_with_later_pass(10, later));
    assert_eq!(result.data, [2; 8]);
    assert_eq!(result.bits_saved, 16);

    // After a strict repair that grew the file, a pass back to the source's
    // length saves bytes against that repair only: nothing against the source.
    let mut repaired = Optimization::from_metrics(10, vec![0; 12], 80, 90, false);
    let later = Optimization::from_metrics(12, vec![1; 10], 90, 79, false);
    assert!(repaired.replace_with_later_pass(10, later));
    assert_eq!(repaired.data, [1; 10]);
    assert_eq!(repaired.bits_saved, 0);
}

#[test]
fn deterministic_malformed_corpus_never_panics_or_grows_output() {
    let options = Options {
        strict: false,
        timeout: Duration::ZERO,
        max_input_bytes: 512,
        max_decoded_bytes: 4_096,
        ..Options::default()
    };
    let formats = [
        Format::Auto,
        Format::Raw,
        Format::Png,
        Format::Zlib,
        Format::Gzip,
        Format::Zip,
    ];
    let mut state = 0x6a09_e667_f3bc_c909_u64;

    for case in 0..512_usize {
        // Xorshift64 gives reproducible inputs without a random-number dependency.
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        let length = (state as usize) % 257;
        let mut input = Vec::with_capacity(length);
        for _ in 0..length {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            input.push(state as u8);
        }

        for format in formats {
            if let Ok(output) = optimize(&input, format, &options) {
                assert!(
                    output.data.len() <= input.len(),
                    "case {case} grew in {format:?} mode"
                );
            }
        }
    }
}

#[test]
fn allocation_errors_name_what_could_not_be_allocated() {
    let error = Error::allocation("PNG output");
    assert_eq!(error.kind(), ErrorKind::Internal);
    assert_eq!(error.message(), "could not allocate PNG output");
}
