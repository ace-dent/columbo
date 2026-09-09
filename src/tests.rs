// SPDX-License-Identifier: MIT

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
        // Xorshift64 gives a reproducible spread of lengths and bytes
        // without adding a random-number dependency to the crate.
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
