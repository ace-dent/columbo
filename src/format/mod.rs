// SPDX-License-Identifier: MIT

//! Format detection, shared resource limits, and container-specific rewriting.

mod gzip;
mod png;
mod zip;
mod zlib;

use std::time::{Duration, Instant};

use crate::{Error, ErrorKind, Format, Optimization, Options, Result};

#[cfg(test)]
const SAME_BYTE_BIT_WIN_RAW: &[u8] = &[
    0x75, 0xc0, 0x41, 0x0d, 0x00, 0x00, 0x0c, 0x03, 0x21, 0x6d, 0xf8, 0x37, 0xb5, 0x7f, 0x97, 0x03,
    0xcb, 0xb2, 0x3c, 0x82, 0x20, 0x08, 0x0e,
];

#[cfg(test)]
fn same_byte_bit_win_zlib() -> Vec<u8> {
    let mut stream = vec![0x78, 0x01];
    stream.extend_from_slice(SAME_BYTE_BIT_WIN_RAW);
    stream.extend_from_slice(&crate::checksum::adler32(&[b'A'; 168]).to_be_bytes());
    stream
}

/// Allocate a vector without letting an attacker-controlled capacity turn an
/// otherwise recoverable wrapper error into an allocation panic or abort.
pub(super) fn try_vec_with_capacity<T>(capacity: usize, message: &'static str) -> Result<Vec<T>> {
    let mut output = Vec::new();
    output
        .try_reserve_exact(capacity)
        .map_err(|_| Error::internal(message))?;
    Ok(output)
}

/// Fallibly copy wrapper bytes whose length ultimately comes from the input.
pub(super) fn try_copy_bytes(source: &[u8], message: &'static str) -> Result<Vec<u8>> {
    let mut output = try_vec_with_capacity(source.len(), message)?;
    output.extend_from_slice(source);
    Ok(output)
}

/// Reserve before extending a wrapper output. `Vec::extend_from_slice` may
/// otherwise invoke the infallible allocation path when strict mode is allowed
/// to grow a stream beyond its input-sized initial reservation.
pub(super) fn try_append_bytes(
    output: &mut Vec<u8>,
    source: &[u8],
    message: &'static str,
) -> Result<()> {
    output
        .try_reserve(source.len())
        .map_err(|_| Error::internal(message))?;
    output.extend_from_slice(source);
    Ok(())
}

/// Scale an optional-search allowance without panicking on caller-supplied
/// extreme durations.
///
/// Container schedulers only pass finite fractions in `0.0..=1.0`. Keeping
/// the guards here still matters because [`Options`] is a public API and may
/// contain `Duration::MAX`; rounding that value through `f64` and feeding it
/// directly to `Duration::from_secs_f64` can otherwise overflow and panic.
pub(super) fn scale_duration(duration: Duration, factor: f64) -> Duration {
    if !factor.is_finite() || factor <= 0.0 {
        return Duration::ZERO;
    }
    if factor >= 1.0 {
        return duration;
    }

    let seconds = duration.as_secs_f64() * factor;
    if !seconds.is_finite() || seconds >= u64::MAX as f64 {
        duration
    } else {
        Duration::from_secs_f64(seconds)
    }
}

/// A container may hold many independent Deflate streams, but `--timeout`
/// applies to the file as a whole. Raw optimizers remain self-contained and
/// thread-safe; the wrapper passes each one only the search time still left.
#[derive(Clone, Copy)]
pub(super) struct SearchDeadline {
    started: Instant,
    timeout: Duration,
}

impl SearchDeadline {
    pub(super) fn new(options: &Options) -> Self {
        Self {
            started: Instant::now(),
            timeout: options.timeout,
        }
    }

    pub(super) fn options_for_call(&self, options: &Options) -> Options {
        let mut call = options.clone();
        let remaining = self.remaining();
        call.timeout = call.timeout.min(remaining);
        call
    }

    pub(super) fn remaining(&self) -> Duration {
        self.timeout.saturating_sub(self.started.elapsed())
    }

    pub(super) fn is_expired(&self) -> bool {
        self.started.elapsed() >= self.timeout
    }

    /// Give grace only to a call that owns the file's actual remainder.
    ///
    /// Proportional child slices are scheduling boundaries, not independent
    /// user timeouts. Multiplying the one-second grace by every container
    /// stream would starve later streams and exceed the documented file-wide
    /// allowance.
    pub(super) fn grace_for_call(&self, call_timeout: Duration) -> Duration {
        let elapsed = self.started.elapsed();
        let soft_remaining = self.timeout.saturating_sub(elapsed);
        if call_timeout < soft_remaining {
            if self.timeout.is_zero() {
                return Duration::ZERO;
            }
            return scale_duration(
                crate::deflate::timeout_grace(self.timeout),
                call_timeout.as_secs_f64() / self.timeout.as_secs_f64(),
            );
        }
        self.timeout
            .saturating_add(crate::deflate::timeout_grace(self.timeout))
            .saturating_sub(elapsed)
            .saturating_sub(call_timeout)
    }
}

pub(crate) fn optimize(input: &[u8], requested: Format, options: &Options) -> Result<Optimization> {
    if input.len() as u64 > options.max_input_bytes {
        return Err(Error::resource_limit(
            "input exceeds configured safety limit",
        ));
    }

    let mut effective_options = options_with_input_expansion_limit(input.len(), options);
    if effective_options.visual && !crate::progress::reports_enabled(&effective_options) {
        effective_options.visual = false;
    }
    let options = &effective_options;
    let reporting = options.verbose || options.visual;

    let detection = match requested {
        Format::Auto => detect(input),
        explicit => Detection::Confirmed(explicit),
    };
    let detected = detection.format();

    let result = match detected {
        Format::Auto | Format::Raw => {
            crate::progress::format_detected(options, detected, reporting.then_some(1));
            super::deflate::optimize_raw(input, options)
                .map(|raw| {
                    Optimization::from_metrics(
                        input.len(),
                        raw.data,
                        raw.info.source_deflate_bits,
                        raw.info.deflate_bits,
                        raw.timed_out,
                    )
                })
                .map_err(|error| {
                    // Failure to recognize malformed raw input is a format
                    // error. Resource and internal failures keep their kind
                    // so callers can distinguish them from damaged data.
                    if requested == Format::Auto && error.kind() == ErrorKind::InvalidInput {
                        Error::unsupported_format("unsupported or invalid input format")
                    } else {
                        error
                    }
                })
        }
        Format::Png => {
            let parsed = png::preflight(input, options.strip_metadata)?;
            let count = if reporting {
                Some(png::stream_count(&parsed)?)
            } else {
                None
            };
            crate::progress::format_detected(options, detected, count);
            png::optimize_preflight(input, options, parsed)
        }
        Format::Zlib => {
            crate::progress::format_detected(options, detected, reporting.then_some(1));
            zlib::optimize(input, options)
        }
        Format::Gzip => {
            let members = gzip::preflight(input, options.max_decoded_bytes)?;
            let count = reporting.then_some(members.len());
            crate::progress::format_detected(options, detected, count);
            gzip::optimize_preflight(input, options, members)
        }
        Format::Zip => {
            let parsed = zip::preflight(input, options.strip_metadata, options.max_decoded_bytes)?;
            let count = reporting.then_some(zip::stream_count(&parsed));
            crate::progress::format_detected(options, detected, count);
            zip::optimize_preflight(input, options, &parsed)
        }
    };
    crate::progress::finish_file(options);
    result
}

pub(crate) fn deflate_stream_count(
    input: &[u8],
    requested: Format,
    options: &Options,
) -> Result<usize> {
    if input.len() as u64 > options.max_input_bytes {
        return Err(Error::resource_limit(
            "input exceeds configured safety limit",
        ));
    }
    let effective_options = options_with_input_expansion_limit(input.len(), options);
    let options = &effective_options;
    let detected = match requested {
        Format::Auto => detect(input).format(),
        explicit => explicit,
    };
    match detected {
        Format::Auto | Format::Raw | Format::Zlib => Ok(1),
        Format::Png => png::deflate_stream_count(input, options.strip_metadata),
        Format::Gzip => gzip::deflate_stream_count(input, options.max_decoded_bytes),
        Format::Zip => zip::deflate_stream_count(input),
    }
}

/// Apply the bomb-resistance policy once at the top-level input boundary.
///
/// Container children must share this reduced cumulative allowance rather
/// than each deriving a fresh ratio from its own compressed slice. Otherwise
/// a many-member archive could multiply the minimum allowance.
fn options_with_input_expansion_limit(input_bytes: usize, options: &Options) -> Options {
    let mut effective = options.clone();
    if let Some(ratio) = options.max_expansion_ratio {
        let ratio_limit = u64::try_from(input_bytes)
            .unwrap_or(u64::MAX)
            .saturating_mul(ratio)
            .max(crate::MIN_EXPANSION_LIMIT_BYTES);
        effective.max_decoded_bytes = effective.max_decoded_bytes.min(ratio_limit);
    }
    effective
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Detection {
    Confirmed(Format),
    Recognized(Format),
    RawCandidate,
}

impl Detection {
    fn format(self) -> Format {
        match self {
            Self::Confirmed(format) | Self::Recognized(format) => format,
            Self::RawCandidate => Format::Raw,
        }
    }
}

fn detect(input: &[u8]) -> Detection {
    if input.starts_with(b"\x89PNG\r\n\x1a\n") {
        Detection::Confirmed(Format::Png)
    } else if gzip::has_rfc1952_header(input) {
        Detection::Confirmed(Format::Gzip)
    } else if zip::has_plausible_end_record(input) {
        Detection::Confirmed(Format::Zip)
    } else if looks_like_zlib(input) {
        Detection::Confirmed(Format::Zlib)
    } else if gzip::has_signature(input) {
        Detection::Recognized(Format::Gzip)
    } else if zip::has_recognizable_structure(input) {
        Detection::Recognized(Format::Zip)
    } else {
        Detection::RawCandidate
    }
}

fn looks_like_zlib(input: &[u8]) -> bool {
    zlib::has_rfc1950_header(input)
}

#[cfg(test)]
mod tests;
