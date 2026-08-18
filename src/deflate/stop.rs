// SPDX-License-Identifier: MIT

//! Cooperative stopping shared by Deflate search planners.
//!
//! Search functions used to be generic over each caller's deadline closure.
//! That made a distinct copy of the complete planner for every closure type.
//! `SearchStop` keeps the policy explicit and concrete, so production deadline
//! probes remain direct calls while the optimizer emits one planner body.

use std::sync::atomic::{AtomicU8, Ordering};
use std::time::{Duration, Instant};

const DEADLINE_TIMED_OUT: u8 = 1;
const DEADLINE_ROUTES_CANCELLED: u8 = 2;
pub(crate) const TIMEOUT_GRACE_DIVISOR: u32 = 10;
pub(crate) const TIMEOUT_GRACE_BASE: Duration = Duration::from_secs(1);
const INITIAL_BOUNDED_PHASE_NUMERATOR: u32 = 4;
const INITIAL_BOUNDED_PHASE_DENOMINATOR: u32 = 5;

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
    // Timeout and route-cancellation flags share one atomic so a hot search
    // probe performs only one synchronization load. The flags carry no data,
    // so relaxed ordering is sufficient.
    state: AtomicU8,
}

impl Deadline {
    #[cfg(test)]
    pub(crate) fn new(started: Instant, duration: Duration) -> Self {
        Self::with_grace(started, duration, timeout_grace(duration))
    }

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

    /// Whether another independent route may begin.
    pub(crate) fn can_start_route(&self) -> bool {
        if self.soft_expired() {
            return false;
        }
        self.state.load(Ordering::Relaxed) & DEADLINE_ROUTES_CANCELLED == 0
    }

    #[inline(always)]
    pub(crate) fn soft_expired(&self) -> bool {
        if self.started.elapsed() < self.duration {
            return false;
        }
        self.state.fetch_or(DEADLINE_TIMED_OUT, Ordering::Relaxed);
        true
    }

    /// Hard stop polled by an active route while it finalizes a candidate.
    #[inline(always)]
    pub(crate) fn expired(&self) -> bool {
        if self.state.load(Ordering::Relaxed) & DEADLINE_ROUTES_CANCELLED != 0 {
            return true;
        }
        let hard_duration = self.duration.saturating_add(self.grace);
        if self.started.elapsed() < hard_duration {
            return false;
        }
        self.state.fetch_or(DEADLINE_TIMED_OUT, Ordering::Relaxed);
        true
    }

    #[inline(always)]
    pub(crate) fn route_should_stop(&self) -> bool {
        self.expired()
    }

    pub(crate) fn cancel_routes(&self) {
        // `fetch_or` cannot erase a timeout racing with this route failure.
        self.state
            .fetch_or(DEADLINE_ROUTES_CANCELLED, Ordering::Relaxed);
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

/// Cooperative stop boundary for one route phase.
///
/// A phase yield does not mark the file as timed out: it merely forwards each
/// route's complete incumbent to the next stage while the global soft budget
/// still permits that stage to start.
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

    /// Let an admitted dependent refinement finish within the file grace.
    pub(crate) fn hard_stop(&self) -> SearchStop<'_> {
        self.deadline.hard_stop()
    }
}

/// One concrete stop policy for the complete planner hierarchy.
///
/// Normal execution uses the deadline and window variants, which make direct
/// calls and avoid virtual dispatch. The callback variant remains available
/// for progress instrumentation and focused tests without multiplying the hot
/// planner for every closure type.
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

    #[inline(always)]
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
