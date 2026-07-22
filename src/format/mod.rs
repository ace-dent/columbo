// SPDX-License-Identifier: MIT

mod gzip;
mod png;
mod zip;
mod zlib;

use std::time::{Duration, Instant};

use crate::{Error, Format, Optimization, Options, Result};

/// Allocate a vector without letting an attacker-controlled capacity turn an
/// otherwise recoverable wrapper error into an allocation panic or abort.
pub(super) fn try_vec_with_capacity<T>(capacity: usize, message: &'static str) -> Result<Vec<T>> {
    let mut output = Vec::new();
    output
        .try_reserve_exact(capacity)
        .map_err(|_| Error::new(message))?;
    Ok(output)
}

/// Fallibly copy wrapper bytes whose length ultimately comes from the input.
pub(super) fn try_copy_bytes(source: &[u8], message: &'static str) -> Result<Vec<u8>> {
    let mut output = try_vec_with_capacity(source.len(), message)?;
    output.extend_from_slice(source);
    Ok(output)
}

/// Reserve before extending a wrapper output. `Vec::extend_from_slice` may
/// otherwise invoke the infallible allocation path when compatibility mode is
/// allowed to grow a stream beyond its input-sized initial reservation.
pub(super) fn try_append_bytes(
    output: &mut Vec<u8>,
    source: &[u8],
    message: &'static str,
) -> Result<()> {
    output
        .try_reserve(source.len())
        .map_err(|_| Error::new(message))?;
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
/// applies to the file as a whole.  Raw optimizers remain self-contained and
/// thread-safe; the wrapper passes each one only the search time still left.
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
}

pub(crate) fn optimize(input: &[u8], requested: Format, options: &Options) -> Result<Optimization> {
    if input.len() as u64 > options.max_input_bytes {
        return Err(Error::new("input exceeds configured safety limit"));
    }

    let format = match requested {
        Format::Auto => detect(input),
        explicit => explicit,
    };

    match format {
        Format::Auto | Format::Raw => {
            super::deflate::optimize_raw(input, options).map(|raw| Optimization {
                data: raw.data,
                timed_out: raw.timed_out,
            })
        }
        Format::Png => png::optimize(input, options),
        Format::Zlib => zlib::optimize(input, options),
        Format::Gzip => gzip::optimize(input, options),
        Format::Zip => zip::optimize(input, options),
    }
}

fn detect(input: &[u8]) -> Format {
    if input.starts_with(b"\x89PNG\r\n\x1a\n") {
        Format::Png
    } else if looks_like_zlib(input) {
        Format::Zlib
    } else if input.starts_with(&[0x1f, 0x8b]) {
        Format::Gzip
    } else if looks_like_zip(input) {
        Format::Zip
    } else {
        Format::Raw
    }
}

fn looks_like_zlib(input: &[u8]) -> bool {
    matches!(input, [0x78, 0x01 | 0x5e | 0x9c | 0xda, ..])
}

fn looks_like_zip(input: &[u8]) -> bool {
    matches!(
        input,
        [b'P', b'K', 0x03, 0x04, ..] | [b'P', b'K', 0x05, 0x06, ..] | [b'P', b'K', 0x07, 0x08, ..]
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_deadline_is_shared_in_normal_mode_too() {
        let options = Options {
            timeout: Duration::from_secs(10),
            ..Options::default()
        };
        let expired = SearchDeadline {
            started: Instant::now() - Duration::from_secs(2),
            timeout: Duration::from_secs(1),
        };

        assert_eq!(expired.options_for_call(&options).timeout, Duration::ZERO);
    }

    #[test]
    fn custom_input_limit_uses_a_truthful_diagnostic() {
        let options = Options {
            max_input_bytes: 1,
            ..Options::default()
        };

        let error = optimize(&[0x03, 0x00], Format::Raw, &options).unwrap_err();
        assert_eq!(error.message(), "input exceeds configured safety limit");
    }

    #[test]
    fn duration_scaling_handles_extreme_public_options() {
        let scaled = scale_duration(Duration::MAX, 0.98);
        assert!(scaled < Duration::MAX);
        assert_eq!(scale_duration(Duration::MAX, 1.0), Duration::MAX);
        assert_eq!(scale_duration(Duration::MAX, f64::NAN), Duration::ZERO);
    }
}
