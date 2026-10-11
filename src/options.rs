// SPDX-License-Identifier: MIT
// SPDX-FileCopyrightText: 2026 Andrew C.E. Dent <https://github.com/ace-dent>

use std::time::Duration;

/// Default limit on compressed input bytes per top-level call.
pub const MAX_INPUT_BYTES: u64 = 1 << 30;
/// Default cumulative decoded-byte ceiling for one top-level call.
pub const MAX_DECODED_BYTES: u64 = 1 << 30;
/// Default maximum decoded-to-input expansion ratio.
pub const MAX_EXPANSION_RATIO: u64 = 512;
/// Decoded bytes always allowed by the expansion ratio, however small the input.
pub const MIN_EXPANSION_LIMIT_BYTES: u64 = 64 * 1024 * 1024;
/// Smallest CLI timeout; shorter requests are raised to it.
pub const MIN_TIMEOUT: Duration = Duration::from_secs(10);
/// Default wall-clock search budget for one top-level call.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(180);
/// Largest CLI timeout; longer requests are lowered to it.
pub const MAX_TIMEOUT: Duration = Duration::from_secs(4_000);

/// Input interpretation requested by the caller.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Format {
    /// Detect a supported wrapper from its signature.
    #[default]
    Auto,
    /// A headerless RFC 1951 Deflate stream.
    Raw,
    /// A PNG or APNG file.
    Png,
    /// An RFC 1950 zlib stream.
    Zlib,
    /// An RFC 1952 GZIP stream, including concatenated members.
    Gzip,
    /// A classic ZIP archive (ZIP64 is rejected).
    Zip,
}

/// Controls one optimization run.
///
/// Options are reusable, and independent calls are thread-safe. Calls with
/// verbose or visual reports share standard output and run one at a time;
/// quiet calls run concurrently. Reporting never changes the optimization.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Options {
    /// Enable the slower block-boundary and token-spelling search (`--max`).
    pub exhaustive: bool,
    /// Report route timings, bit gains, and final block choices to standard
    /// output.
    pub verbose: bool,
    /// Print a two-row block map (source and output) for each Deflate stream
    /// once it finishes.
    ///
    /// Takes effect only when standard output is an interactive terminal;
    /// otherwise the call runs without visual output.
    pub visual: bool,
    /// Remove supported metadata, credentials, signatures, and extraneous
    /// payloads while rebuilding the file. Preserve decoded content; reject
    /// ambiguous layouts and unsupported compound ZIP stripping.
    pub strip_metadata: bool,
    /// Emit conservative Deflate that is accepted by strict and older decoders.
    ///
    /// Strict mode gives dynamic blocks complete Huffman codes for the
    /// literal/length and distance alphabets. Relaxed mode permits the empty
    /// or single-code distance alphabets that RFC 1951 allows, single-code
    /// literal/length alphabets accepted by common decoders, and defluff's
    /// non-standard symbol-284 spelling of length 258.
    pub strict: bool,
    /// Soft wall-clock scheduling budget for the whole input file.
    ///
    /// Embedded PNG frames, GZIP members, and ZIP entries share this budget;
    /// no new route starts after it expires. An active route may run on for
    /// ten percent of the timeout plus one second to finalize its best
    /// candidate. Validation and the complete no-growth fallback are never
    /// skipped. In Max mode, the Default comparison floor finishes even after
    /// the budget expires, so Max never returns a worse result than Default.
    pub timeout: Duration,
    /// Maximum number of compressed bytes accepted from the supplied file.
    pub max_input_bytes: u64,
    /// Maximum cumulative decoded payload bytes across the supplied file.
    pub max_decoded_bytes: u64,
    /// Maximum cumulative decoded bytes relative to the top-level input size.
    ///
    /// The ratio limit is never below [`MIN_EXPANSION_LIMIT_BYTES`] (64 MiB),
    /// and the smaller of it and `max_decoded_bytes` applies. This rejects a
    /// compact Deflate bomb before it can consume the full absolute allowance.
    /// Set this to `None` only when the caller trusts unusually
    /// high-compression input and supplies an appropriate absolute limit.
    pub max_expansion_ratio: Option<u64>,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            exhaustive: false,
            verbose: false,
            visual: false,
            strip_metadata: false,
            strict: true,
            timeout: DEFAULT_TIMEOUT,
            max_input_bytes: MAX_INPUT_BYTES,
            max_decoded_bytes: MAX_DECODED_BYTES,
            max_expansion_ratio: Some(MAX_EXPANSION_RATIO),
        }
    }
}
