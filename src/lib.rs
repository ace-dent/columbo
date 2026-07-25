// SPDX-License-Identifier: MIT

//! Deflate-stream optimization with container-aware, compatibility-safe rewriting.
//!
//! Columbo is a structural optimizer, not a recompressor: it never searches
//! for new LZ77 matches and never delegates to Zopfli, libdeflate, or a similar
//! compressor. It only re-encodes choices already present in the input stream,
//! such as block boundaries, Huffman tables, and spelling an existing match as
//! its decoded literals when that is cheaper.
//!
//! The public API deliberately separates format detection from optimization
//! settings. This keeps callers deterministic while allowing the command-line
//! program to retain the original Columbo C implementation's convenient
//! auto-detection.

#![forbid(unsafe_code)]

mod checksum;
mod deflate;
mod error;
mod format;
mod options;

pub use error::{Error, Result};
pub use options::{
    Format, Options, DEFAULT_TIMEOUT, MAX_DECODED_BYTES, MAX_INPUT_BYTES, MAX_TIMEOUT, MIN_TIMEOUT,
};

/// The result of one optimization run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Optimization {
    /// The selected file or stream bytes.
    pub data: Vec<u8>,
    /// Whether search reached its deadline.
    pub timed_out: bool,
}

/// Optimize one raw stream or supported container.
///
/// Strict mode may enlarge an input while completing a dynamic Huffman
/// alphabet or canonicalizing a non-standard length-258 spelling. Relaxed
/// mode and ordinary strict inputs retain the no-growth guarantee.
/// Pathologically fragmented streams may also hit an internal parsed-model
/// memory budget before reaching the configurable byte limits.
pub fn optimize(input: &[u8], format: Format, options: &Options) -> Result<Optimization> {
    format::optimize(input, format, options)
}

#[cfg(test)]
mod robustness_tests {
    use std::time::Duration;

    use super::*;

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
}
