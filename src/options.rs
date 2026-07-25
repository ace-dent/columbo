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
    /// Print block-level decisions to standard error.
    pub inspect: bool,
    /// Remove supported wrapper metadata while rebuilding the file.
    pub strip_metadata: bool,
    /// Emit at least two distance codes for compatibility with old decoders.
    pub min_distance_codes: bool,
    /// Permit the Defluff-derived optimization that spells length 258 with
    /// symbol 284.
    ///
    /// Columbo admits the candidate more broadly than Defluff. Some strict
    /// Deflate decoders may reject this non-standard representation.
    pub allow_258_alias: bool,
    /// Wall-clock search budget for the whole input file.
    ///
    /// Embedded PNG frames, GZIP members, and ZIP entries share this budget;
    /// validation and the complete no-growth fallback are never skipped.
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
            inspect: false,
            strip_metadata: false,
            min_distance_codes: false,
            allow_258_alias: false,
            timeout: DEFAULT_TIMEOUT,
            max_input_bytes: MAX_INPUT_BYTES,
            max_decoded_bytes: MAX_DECODED_BYTES,
        }
    }
}
