// SPDX-License-Identifier: MIT
// SPDX-FileCopyrightText: 2026 Andrew C.E. Dent <https://github.com/ace-dent>

//! Deflate optimization with container-aware, compatibility-safe rewriting.
//!
//! Columbo is a structural optimizer, not a recompressor: it never searches
//! for new LZ77 matches and never calls Zopfli, libdeflate, or a similar
//! compressor. It re-encodes the input's existing tokens, choosing block
//! boundaries and Huffman codes, and may spell an existing match as its
//! decoded literals or as shorter matches at the same distance when that is
//! cheaper.
//!
//! The input [`Format`] is passed separately from [`Options`]. Callers can name
//! it for deterministic handling, or pass [`Format::Auto`] to detect it as the
//! CLI does.

#![forbid(unsafe_code)]

mod checksum;
mod deflate;
mod error;
mod format;
mod options;
mod progress;
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
    /// Bits saved by the selected output, counting bytes first, then meaningful bits.
    ///
    /// A shorter file saves eight bits per removed byte. When the byte length
    /// is unchanged, this is the total reduction in meaningful payload bits
    /// across all optimized streams: Deflate bits excluding final-byte padding,
    /// with a ZIP member converted to Store counted as its decoded size in
    /// bits. Call [`Optimization::should_replace`] to also account for a
    /// mandatory, equal-sized wrapper normalization.
    pub bits_saved: u64,
    /// Whether search reached its deadline.
    pub timed_out: bool,
    /// Extraneous payload bytes removed by [`Options::strip_metadata`]
    /// (`--strip`), excluding metadata and compression savings. Zeroed Deflate
    /// padding bits are not counted.
    pub removed_data_bytes: u64,
    rewrite_required: bool,
    blocking_chunk: Option<[u8; 4]>,
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
            removed_data_bytes: 0,
            rewrite_required: false,
            blocking_chunk: None,
        }
    }

    /// Whether the selected bytes should replace the source representation.
    ///
    /// Usually equivalent to `bits_saved != 0`. Also true for a mandatory
    /// equal-sized wrapper normalization (a ZIP member of at most four decoded
    /// bytes changed from Deflate to Store), and for any changed output when
    /// [`Options::strip_metadata`] is set.
    pub fn should_replace(&self) -> bool {
        self.bits_saved != 0 || self.rewrite_required
    }

    /// The PNG chunk type that made Columbo return the source unchanged.
    ///
    /// A signature, such as `caBX` or `dSIG`, or an unknown unsafe-to-copy
    /// ancillary chunk would be invalidated by rewriting the image data.
    /// Setting [`Options::strip_metadata`] removes it and allows optimization.
    pub fn blocking_chunk(&self) -> Option<&str> {
        self.blocking_chunk
            .as_ref()
            .and_then(|kind| std::str::from_utf8(kind).ok())
    }

    pub(crate) fn blocked_by(mut self, kind: [u8; 4]) -> Self {
        self.blocking_chunk = Some(kind);
        self
    }

    pub(crate) fn require_rewrite(&mut self) {
        self.rewrite_required = true;
    }

    pub(crate) fn with_removed_data(mut self, bytes: usize) -> Self {
        self.removed_data_bytes = bytes as u64;
        self
    }
}

/// Optimize one raw Deflate stream or supported container.
///
/// The output is never larger than the input, except in strict mode when it
/// must complete an incomplete dynamic Huffman code or rewrite the
/// non-standard symbol-284 spelling of length 258. Highly fragmented streams
/// can exceed an internal memory budget for the parsed model before reaching
/// the configurable byte limits.
pub fn optimize(input: &[u8], format: Format, options: &Options) -> Result<Optimization> {
    let result = format::optimize(input, format, options);
    deflate::release_thread_header_plans();
    deflate::release_thread_block_plans();
    result
}

/// Count the independent Deflate streams Columbo can inspect in one input.
///
/// Container declarations alone are not trusted: PNG/APNG and GZIP inputs are
/// parsed far enough to distinguish physical streams, and only unencrypted,
/// non-empty Deflate ZIP members are counted. PNG counts include fdAT frames
/// and zlib-compressed metadata chunks.
pub fn deflate_stream_count(input: &[u8], format: Format, options: &Options) -> Result<usize> {
    format::deflate_stream_count(input, format, options)
}

#[cfg(test)]
mod tests;
