// SPDX-License-Identifier: MIT
// SPDX-FileCopyrightText: 2026 Andrew C.E. Dent <https://github.com/ace-dent>

//! Shared file deadlines and proportional container search allowances.

use std::time::{Duration, Instant};

use crate::Options;

/// Scale an allowance by `factor` without panicking: a public [`Options`] may
/// hold `Duration::MAX`, which would overflow `Duration::from_secs_f64`.
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

/// File-wide search deadline shared by a container's Deflate streams. Each raw
/// optimizer call gets at most the time left.
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

    // Called once per stream or member from many places: out of line, it is
    // one copy instead of one per call site.
    #[inline(never)]
    pub(super) fn remaining(&self) -> Duration {
        self.timeout.saturating_sub(self.started.elapsed())
    }

    pub(super) fn is_expired(&self) -> bool {
        self.started.elapsed() >= self.timeout
    }

    /// Grace period for one raw optimizer call.
    ///
    /// A call that receives all the remaining file time also receives the rest
    /// of the file's grace. A shorter slice receives the file's grace scaled by
    /// its share of the file timeout, so the streams' graces together stay
    /// within the file's.
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
        self.hard_time_after(call_timeout, elapsed)
    }

    /// Grace period for a stage whose output the next stage of the same
    /// lineage starts from, when that next stage takes the lineage's remaining
    /// time from the clock.
    ///
    /// The scaled share of [`Self::grace_for_call`] keeps sibling streams'
    /// overruns from adding up past the file's hard stop. A stage's overrun
    /// shortens its own next stage instead, and the finish it buys is the next
    /// stage's starting point, so the stage keeps the raw optimizer's grace
    /// for its timeout, capped at the file's hard stop.
    pub(super) fn grace_for_stage(&self, call_timeout: Duration) -> Duration {
        crate::deflate::timeout_grace(call_timeout)
            .min(self.hard_time_after(call_timeout, self.started.elapsed()))
    }

    /// Time from the end of a `call_timeout` starting now, `elapsed` into the
    /// file, to the file's hard stop.
    fn hard_time_after(&self, call_timeout: Duration, elapsed: Duration) -> Duration {
        self.timeout
            .saturating_add(crate::deflate::timeout_grace(self.timeout))
            .saturating_sub(elapsed)
            .saturating_sub(call_timeout)
    }
}

#[cfg(test)]
mod tests;
