// SPDX-License-Identifier: MIT

//! Shared file deadlines and proportional container search allowances.

use std::time::{Duration, Instant};

use crate::Options;

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

#[cfg(test)]
mod tests;
