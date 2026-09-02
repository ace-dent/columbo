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
mod presentation;
mod progress;
mod spinner;
mod terminal;

pub use error::{Error, ErrorKind, Result};
pub use options::{
    Format, Options, DEFAULT_TIMEOUT, MAX_DECODED_BYTES, MAX_EXPANSION_RATIO, MAX_INPUT_BYTES,
    MAX_TIMEOUT, MIN_EXPANSION_LIMIT_BYTES, MIN_TIMEOUT,
};

/// The result of one optimization run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Optimization {
    /// The selected file or stream bytes.
    pub data: Vec<u8>,
    /// Bits saved by the selected output under Columbo's byte-first ordering.
    ///
    /// A shorter file saves eight bits per removed byte. When the file byte
    /// length is unchanged, this instead reports the reduction in aggregate
    /// meaningful compressed-payload bits across every optimized stream. For
    /// Deflate this excludes final-byte padding; a ZIP member whose terminal
    /// representation changes to Store is charged its decoded payload bits.
    /// A value of zero means there was no physical compression saving. Call
    /// [`Optimization::should_replace`] to also account for a mandatory,
    /// equal-sized wrapper normalization.
    pub bits_saved: u64,
    /// Whether search reached its deadline.
    pub timed_out: bool,
    rewrite_required: bool,
}

impl Optimization {
    pub(crate) fn from_metrics(
        source_bytes: usize,
        data: Vec<u8>,
        source_deflate_bits: u64,
        output_deflate_bits: u64,
        timed_out: bool,
    ) -> Self {
        let bits_saved = match source_bytes.cmp(&data.len()) {
            std::cmp::Ordering::Greater => u64::try_from(source_bytes - data.len())
                .map_or(u64::MAX, |bytes| bytes.saturating_mul(8)),
            std::cmp::Ordering::Equal => source_deflate_bits.saturating_sub(output_deflate_bits),
            std::cmp::Ordering::Less => 0,
        };
        Self {
            data,
            bits_saved,
            timed_out,
            rewrite_required: false,
        }
    }

    /// Whether the selected bytes should replace the source representation.
    ///
    /// This is normally equivalent to `bits_saved != 0`. It is also true for
    /// a mandatory equal-sized wrapper normalization, such as changing a
    /// four-byte ZIP member from Deflate to Store when neither method can use
    /// fewer physical payload bytes.
    pub fn should_replace(&self) -> bool {
        self.bits_saved != 0 || self.rewrite_required
    }

    pub(crate) fn require_rewrite(&mut self) {
        self.rewrite_required = true;
    }
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

/// Count the independent Deflate streams Columbo can inspect in one input.
///
/// Container declarations alone are not trusted: PNG/APNG and GZIP inputs are
/// parsed far enough to distinguish physical streams, and encrypted or empty
/// ZIP members that cannot expose a Deflate bitstream are not counted. The CLI
/// uses this preflight only for verbose and visual headers.
pub fn deflate_stream_count(input: &[u8], format: Format, options: &Options) -> Result<usize> {
    format::deflate_stream_count(input, format, options)
}

#[cfg(test)]
mod robustness_tests {
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
}
