// SPDX-License-Identifier: MIT

use std::time::Duration;

/// Default maximum number of compressed input bytes accepted per top-level call.
pub const MAX_INPUT_BYTES: u64 = 1 << 30;
/// Default cumulative decoded-byte ceiling for one top-level call.
pub const MAX_DECODED_BYTES: u64 = 1 << 30;
/// Smallest timeout accepted by the command-line interface.
pub const MIN_TIMEOUT: Duration = Duration::from_secs(10);
/// Default wall-clock search budget for one top-level call.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(180);
/// Largest timeout accepted by the command-line interface.
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
    /// A classic ZIP archive (ZIP64 is intentionally rejected).
    Zip,
}

/// Controls one optimization run.
///
/// Options are immutable and reusable. Mutable search state lives in the
/// optimizer created for each call, so independent calls are thread-safe.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Options {
    /// Enable the slower block-boundary and token-spelling search (`--max`).
    pub exhaustive: bool,
    /// Report route timings, bit gains, and final block choices to standard
    /// output.
    ///
    /// Reporting is deliberately kept out of hot token and Huffman loops.
    /// Quiet and verbose runs use the same optimization and memory policies.
    pub verbose: bool,
    /// Remove supported wrapper metadata while rebuilding the file.
    pub strip_metadata: bool,
    /// Emit conservative Deflate that is accepted by strict and older decoders.
    ///
    /// Strict mode completes dynamic literal/length and distance alphabets.
    /// Relaxed mode permits RFC-sanctioned empty or singleton distance
    /// alphabets, singleton literal/length alphabets accepted by common
    /// decoders, and the Defluff-derived, non-standard symbol-284 spelling of
    /// length 258.
    pub strict: bool,
    /// Soft wall-clock scheduling budget for the whole input file.
    ///
    /// Embedded PNG frames, GZIP members, and ZIP entries share this budget;
    /// no new route starts after it expires. An active route may use ten
    /// percent plus one second to finalize its best candidate. Validation and
    /// the complete no-growth fallback are never skipped.
    pub timeout: Duration,
    /// Maximum number of compressed bytes accepted from the supplied file.
    pub max_input_bytes: u64,
    /// Maximum cumulative decoded payload bytes across the supplied file.
    pub max_decoded_bytes: u64,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            exhaustive: false,
            verbose: false,
            strip_metadata: false,
            strict: true,
            timeout: DEFAULT_TIMEOUT,
            max_input_bytes: MAX_INPUT_BYTES,
            max_decoded_bytes: MAX_DECODED_BYTES,
        }
    }
}
