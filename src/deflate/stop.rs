// SPDX-License-Identifier: MIT
// SPDX-FileCopyrightText: 2026 Andrew C.E. Dent <https://github.com/ace-dent>

//! Cooperative stopping shared by Deflate search planners: deadlines, grace
//! periods and route cancellation.

use std::sync::atomic::{AtomicU8, Ordering};
use std::time::{Duration, Instant};

const DEADLINE_TIMED_OUT: u8 = 1;
const DEADLINE_ROUTES_CANCELED: u8 = 2;
pub(crate) const TIMEOUT_GRACE_DIVISOR: u32 = 10;
pub(crate) const TIMEOUT_GRACE_BASE: Duration = Duration::from_secs(1);
const INITIAL_BOUNDED_PHASE_NUMERATOR: u32 = 4;
const INITIAL_BOUNDED_PHASE_DENOMINATOR: u32 = 5;

/// Grace period after the soft deadline.
pub(crate) fn timeout_grace(duration: Duration) -> Duration {
    if duration.is_zero() {
        return Duration::ZERO;
    }
    TIMEOUT_GRACE_BASE.saturating_add(duration / TIMEOUT_GRACE_DIVISOR)
}

pub(crate) fn initial_bounded_phase_share(remaining: Duration) -> Duration {
    remaining.saturating_mul(INITIAL_BOUNDED_PHASE_NUMERATOR) / INITIAL_BOUNDED_PHASE_DENOMINATOR
}

pub(crate) struct Deadline {
    pub(crate) started: Instant,
    pub(crate) duration: Duration,
    pub(crate) grace: Duration,
    /// Timeout and route-cancellation flags share one atomic, so a stop check
    /// is one load. They guard no other data, so relaxed ordering is enough.
    state: AtomicU8,
}

impl Deadline {
    pub(crate) fn with_grace(started: Instant, duration: Duration, grace: Duration) -> Self {
        Self {
            started,
            duration,
            grace,
            state: AtomicU8::new(0),
        }
    }

    pub(crate) fn remaining(&self) -> Duration {
        self.duration.saturating_sub(self.started.elapsed())
    }

    /// A deadline with this one's hard stop whose soft deadline is now, so
    /// work under it gets only linear finalization: no route can start.
    pub(crate) fn finalization_only(&self) -> Self {
        let hard_duration = self.duration.saturating_add(self.grace);
        let elapsed = self.started.elapsed().min(hard_duration);
        Self::with_grace(self.started, elapsed, hard_duration - elapsed)
    }

    // Deadline checks read the clock, so a call costs little, while an inlined
    // copy at each of their many call sites costs code size.
    #[inline(never)]
    pub(crate) fn can_start_route(&self) -> bool {
        if self.soft_expired() {
            return false;
        }
        self.state.load(Ordering::Relaxed) & DEADLINE_ROUTES_CANCELED == 0
    }

    #[inline(never)]
    pub(crate) fn soft_expired(&self) -> bool {
        if self.started.elapsed() < self.duration {
            return false;
        }
        self.state.fetch_or(DEADLINE_TIMED_OUT, Ordering::Relaxed);
        true
    }

    /// Hard stop for an active route finishing a candidate.
    #[inline(never)]
    pub(crate) fn expired(&self) -> bool {
        if self.state.load(Ordering::Relaxed) & DEADLINE_ROUTES_CANCELED != 0 {
            return true;
        }
        let hard_duration = self.duration.saturating_add(self.grace);
        if self.started.elapsed() < hard_duration {
            return false;
        }
        self.state.fetch_or(DEADLINE_TIMED_OUT, Ordering::Relaxed);
        true
    }

    #[inline(never)]
    pub(crate) fn route_should_stop(&self) -> bool {
        self.expired()
    }

    pub(crate) fn cancel_routes(&self) {
        // `fetch_or` keeps a timeout flag set concurrently by another thread.
        self.state
            .fetch_or(DEADLINE_ROUTES_CANCELED, Ordering::Relaxed);
    }

    pub(crate) fn was_triggered(&self) -> bool {
        self.soft_expired();
        self.state.load(Ordering::Relaxed) & DEADLINE_TIMED_OUT != 0
    }

    pub(crate) fn hard_stop(&self) -> SearchStop<'_> {
        SearchStop::Deadline {
            deadline: self,
            stop_at_soft_deadline: false,
        }
    }

    pub(crate) fn bounded_stop(&self, stop_at_soft_deadline: bool) -> SearchStop<'_> {
        SearchStop::Deadline {
            deadline: self,
            stop_at_soft_deadline,
        }
    }
}

/// Cooperative stop boundary for one route phase. Reaching it does not mark
/// the file as timed out: the route passes its complete incumbent to the next
/// phase, which still starts if the soft deadline allows.
pub(crate) struct RouteWindow<'a> {
    deadline: &'a Deadline,
    stop_after: Option<Duration>,
}

impl<'a> RouteWindow<'a> {
    pub(crate) fn full(deadline: &'a Deadline) -> Self {
        Self {
            deadline,
            stop_after: None,
        }
    }

    /// Window that closes after 4/5 of the remaining time, leaving the rest
    /// for follow-up phases.
    pub(crate) fn reserving_follow_up(deadline: &'a Deadline) -> Self {
        let elapsed = deadline.started.elapsed();
        let initial_share = initial_bounded_phase_share(deadline.remaining());
        Self {
            deadline,
            stop_after: Some(elapsed.saturating_add(initial_share)),
        }
    }

    pub(crate) fn can_start_route(&self) -> bool {
        self.deadline.can_start_route()
            && self.stop_after.map_or(true, |stop_after| {
                self.deadline.started.elapsed() < stop_after
            })
    }

    pub(crate) fn stop(&self) -> SearchStop<'_> {
        SearchStop::Window {
            deadline: self.deadline,
            stop_after: self.stop_after,
        }
    }

    /// Hard stop that lets a dependent refinement, once started, run into the
    /// file's grace period.
    pub(crate) fn hard_stop(&self) -> SearchStop<'_> {
        self.deadline.hard_stop()
    }
}

/// Stop policy passed through all planners: an enum rather than a generic
/// closure, so each planner is compiled once and deadline checks stay direct
/// calls.
pub(crate) enum SearchStop<'a> {
    Deadline {
        deadline: &'a Deadline,
        stop_at_soft_deadline: bool,
    },
    Window {
        deadline: &'a Deadline,
        stop_after: Option<Duration>,
    },
    Never,
    Always,
    Callback(&'a mut dyn FnMut() -> bool),
}

impl<'a> SearchStop<'a> {
    pub(crate) fn never() -> Self {
        Self::Never
    }

    pub(crate) fn always() -> Self {
        Self::Always
    }

    pub(crate) fn callback(callback: &'a mut dyn FnMut() -> bool) -> Self {
        Self::Callback(callback)
    }

    /// Whether a started route may use its stream's grace period for one small,
    /// deterministic finalization step.
    ///
    /// Each raw optimizer call has its own grace, 1 second plus 10% of a nonzero
    /// timeout, except that ZIP members share the file's grace so that together
    /// their graces stay within it.
    pub(crate) fn permits_bounded_finalization(&self) -> bool {
        match self {
            Self::Deadline {
                deadline,
                stop_at_soft_deadline: false,
            } => {
                !deadline.grace.is_zero()
                    && deadline.state.load(Ordering::Relaxed) & DEADLINE_ROUTES_CANCELED == 0
            }
            Self::Always => true,
            Self::Deadline { .. } | Self::Window { .. } | Self::Never | Self::Callback(_) => false,
        }
    }

    /// Kept out of line like the deadline checks: a stop is polled at hundreds
    /// of call sites, and an inlined copy cost hundreds of bytes at each.
    #[inline(never)]
    pub(crate) fn reached(&mut self) -> bool {
        match self {
            Self::Deadline {
                deadline,
                stop_at_soft_deadline,
            } => {
                if *stop_at_soft_deadline {
                    deadline.soft_expired()
                } else {
                    deadline.expired()
                }
            }
            Self::Window {
                deadline,
                stop_after,
            } => {
                stop_after.is_some_and(|stop_after| deadline.started.elapsed() >= stop_after)
                    || deadline.route_should_stop()
            }
            Self::Never => false,
            Self::Always => true,
            Self::Callback(callback) => callback(),
        }
    }
}

#[cfg(test)]
mod tests;
