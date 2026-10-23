// SPDX-License-Identifier: MIT
// SPDX-FileCopyrightText: 2026 Andrew C.E. Dent <https://github.com/ace-dent>

//! Report lines for one optimization route, and the phases and heartbeats of
//! a long route.

use std::cell::Cell;
use std::time::{Duration, Instant};

use crate::terminal::formatting::{format_duration, plural, plural_u64};

use super::{describe_bit_change, format_bit_count, CandidateProgress, ReportLines};

const FIRST_ROUTE_HEARTBEAT: Duration = Duration::from_secs(2);
const MIN_ROUTE_HEARTBEAT: Duration = Duration::from_secs(3);
const MAX_ROUTE_HEARTBEAT: Duration = Duration::from_secs(60);

/// One route of a stream report, from its start line to its result line.
pub(crate) struct RouteStep {
    lines: ReportLines,
    name: &'static str,
    /// Set only when reporting is enabled.
    started: Option<Instant>,
}

impl RouteStep {
    pub(super) fn start(lines: ReportLines, name: &'static str) -> Self {
        let started = lines.enabled().then(|| {
            lines.line(format_args!(" {} {name}…", lines.arrow()));
            Instant::now()
        });
        Self {
            lines,
            name,
            started,
        }
    }

    pub(crate) fn finish(self, candidate: Option<CandidateProgress>) {
        let Some(started) = self.started else {
            return;
        };
        let elapsed = format_duration(started.elapsed());
        let name = self.name;
        match candidate {
            Some(candidate) => self.lines.line(format_args!(
                " {} {name} · {elapsed} · {}",
                self.lines.candidate_marker(&candidate),
                candidate.summary()
            )),
            None => self.lines.line(format_args!(
                " {} {name} · {elapsed} · no candidate",
                self.lines.neutral()
            )),
        }
    }

    pub(crate) fn finish_phase(self) {
        let Some(started) = self.started else {
            return;
        };
        self.lines.line(format_args!(
            " {} {} · {}",
            self.lines.success(),
            self.name,
            format_duration(started.elapsed())
        ));
    }

    pub(crate) fn fail(self) {
        let Some(started) = self.started else {
            return;
        };
        self.lines.line(format_args!(
            " {} {} · {} · failed",
            self.lines.warning(),
            self.name,
            format_duration(started.elapsed())
        ));
    }
}

/// Buffered progress state for one long-running route; a no-op when reporting
/// is disabled. Interior mutability lets the deadline callback and planner
/// share it.
///
/// Phases print nested under the route; work positions update silently and
/// appear in the next heartbeat, which deadline checks request.
pub(crate) struct RouteProgress {
    lines: ReportLines,
    reference_bits: u64,
    heartbeat_interval: Duration,
    /// Set only when reporting is enabled, as are the times below.
    route_started: Option<Instant>,
    last_reported: Cell<Option<Instant>>,
    heartbeat_emitted: Cell<bool>,

    phase: Cell<&'static str>,
    phase_started: Cell<Option<Instant>>,
    completed: Cell<usize>,
    total: Cell<usize>,
    unit_singular: Cell<&'static str>,
    unit_plural: Cell<&'static str>,
    current_tokens: Cell<Option<usize>>,

    best_bits: Cell<Option<u64>>,
    best_blocks: Cell<usize>,
    deadline_reached: Cell<bool>,
    finalizing_after_soft_deadline: Cell<bool>,
}

impl RouteProgress {
    pub(crate) fn disabled() -> Self {
        Self::new(ReportLines::DISABLED, 0, MAX_ROUTE_HEARTBEAT)
    }

    /// Progress for a route searched within `budget`, which sets the
    /// heartbeat interval.
    pub(super) fn start(lines: ReportLines, reference_bits: u64, budget: Duration) -> Self {
        Self::new(lines, reference_bits, route_heartbeat_interval(budget))
    }

    fn new(lines: ReportLines, reference_bits: u64, heartbeat_interval: Duration) -> Self {
        let route_started = lines.enabled().then(Instant::now);
        Self {
            lines,
            reference_bits,
            heartbeat_interval,
            route_started,
            last_reported: Cell::new(route_started),
            heartbeat_emitted: Cell::new(false),
            phase: Cell::new("Starting"),
            phase_started: Cell::new(route_started),
            completed: Cell::new(0),
            total: Cell::new(0),
            unit_singular: Cell::new("item"),
            unit_plural: Cell::new("items"),
            current_tokens: Cell::new(None),
            best_bits: Cell::new(None),
            best_blocks: Cell::new(0),
            deadline_reached: Cell::new(false),
            finalizing_after_soft_deadline: Cell::new(false),
        }
    }

    pub(crate) fn enabled(&self) -> bool {
        self.lines.enabled()
    }

    fn route_elapsed(&self) -> String {
        format_duration(
            self.route_started
                .expect("enabled route progress has a start time")
                .elapsed(),
        )
    }

    pub(crate) fn deadline_reached(&self) {
        if self.enabled() {
            self.deadline_reached.set(true);
        }
    }

    pub(crate) fn deadline_was_reached(&self) -> bool {
        self.enabled() && self.deadline_reached.get()
    }

    pub(crate) fn finalizing_after_soft_deadline(&self, grace: Duration) {
        if !self.enabled() || self.finalizing_after_soft_deadline.replace(true) {
            return;
        }
        self.lines.line(format_args!(
            "       · Soft deadline reached · finalizing active candidate · up to {} grace",
            format_duration(grace),
        ));
    }

    /// Start a phase of `total` work units and print it.
    pub(crate) fn phase(
        &self,
        name: &'static str,
        total: usize,
        singular: &'static str,
        plural: &'static str,
    ) {
        if !self.enabled() {
            return;
        }
        self.begin_phase(name, 0, total, singular, plural);
        self.lines.line(format_args!(
            "     {} {name} · {total} {}",
            self.lines.arrow(),
            self::plural(total, singular, plural)
        ));
    }

    fn begin_phase(
        &self,
        name: &'static str,
        completed: usize,
        total: usize,
        singular: &'static str,
        plural: &'static str,
    ) {
        self.phase.set(name);
        self.completed.set(completed);
        self.current_tokens.set(None);
        self.total.set(total);
        self.unit_singular.set(singular);
        self.unit_plural.set(plural);
        let now = Instant::now();
        self.last_reported.set(Some(now));
        self.phase_started.set(Some(now));
    }

    /// Update the current work position without printing a line per item.
    pub(crate) fn advance(&self, completed: usize) {
        if self.enabled() {
            self.completed.set(completed.min(self.total.get()));
        }
    }

    /// Record the block being searched and its token count for the heartbeat.
    pub(crate) fn item(&self, current: usize, tokens: usize) {
        if self.enabled() {
            self.completed.set(current.min(self.total.get()));
            self.current_tokens.set(Some(tokens));
        }
    }

    /// Refine the current activity shown by the next heartbeat. Unlike `phase`,
    /// this prints nothing and does not reset the heartbeat timer.
    pub(crate) fn activity(&self, name: &'static str) {
        if self.enabled() {
            self.phase.set(name);
        }
    }

    /// Buffer a periodic status line; called from existing deadline checks.
    pub(crate) fn heartbeat(&self) {
        if !self.enabled() {
            return;
        }
        let now = Instant::now();
        let last_reported = self
            .last_reported
            .get()
            .expect("enabled route progress has a report time");
        let interval = if self.heartbeat_emitted.get() {
            self.heartbeat_interval
        } else {
            FIRST_ROUTE_HEARTBEAT
        };
        if now.duration_since(last_reported) < interval {
            return;
        }
        self.heartbeat_emitted.set(true);
        self.last_reported.set(Some(now));

        let completed = self.completed.get();
        let total = self.total.get();
        let work = if total == 0 {
            String::new()
        } else {
            format!(
                " · {completed}/{total} {}",
                plural(total, self.unit_singular.get(), self.unit_plural.get())
            )
        };
        let tokens = self
            .current_tokens
            .get()
            .map_or_else(String::new, |tokens| {
                format!(" · {tokens} {}", plural(tokens, "token", "tokens"))
            });
        let best = self.best_bits.get().map_or_else(String::new, |bits| {
            format!(
                " · best route: {bits} {} in {} {}",
                plural_u64(bits, "bit", "bits"),
                self.best_blocks.get(),
                plural(self.best_blocks.get(), "block", "blocks")
            )
        });
        self.lines.line(format_args!(
            "       {} {} · {} route elapsed{work}{tokens}{best}",
            self.lines.neutral(),
            self.phase.get(),
            self.route_elapsed()
        ));
    }

    /// Report one complete internal candidate and retain the best bit count.
    pub(crate) fn checkpoint(&self, name: &'static str, bits: u64, blocks: usize) {
        if !self.enabled() {
            return;
        }
        let previous = self.best_bits.get();
        let improved = previous.map_or(true, |best| bits < best);
        let elapsed = self
            .phase_started
            .get()
            .map_or(Duration::ZERO, |started| started.elapsed());
        if improved {
            self.best_bits.set(Some(bits));
            self.best_blocks.set(blocks);
        }
        self.last_reported.set(Some(Instant::now()));

        let (marker, standing) = if improved {
            (
                self.lines.success(),
                describe_bit_change(self.reference_bits, bits),
            )
        } else {
            let standing = match bits.saturating_sub(previous.unwrap_or(bits)) {
                0 => "matches current best".to_owned(),
                behind => format!("{} behind best", format_bit_count(behind)),
            };
            (self.lines.neutral(), standing)
        };
        self.lines.line(format_args!(
            "       {marker} {name} · {} · {bits} {} in {blocks} {} · {standing}",
            format_duration(elapsed),
            plural_u64(bits, "bit", "bits"),
            plural(blocks, "block", "blocks"),
        ));
    }

    pub(crate) fn replay_started(&self, round: usize, limit: usize, blocks: usize, bits: u64) {
        if !self.enabled() {
            return;
        }
        self.begin_phase(
            "Replanning emitted stream",
            round.saturating_sub(1),
            limit,
            "replay",
            "replays",
        );
        self.lines.line(format_args!(
            "     {} Replay {round}/{limit} · reparsing {blocks} {} at {bits} {}",
            self.lines.arrow(),
            plural(blocks, "block", "blocks"),
            plural_u64(bits, "bit", "bits")
        ));
    }

    pub(crate) fn replay_finished(
        &self,
        round: usize,
        before_bits: u64,
        after_bits: u64,
        blocks: usize,
        elapsed: Duration,
        accepted: bool,
    ) {
        if !self.enabled() {
            return;
        }
        self.completed.set(round);
        self.last_reported.set(Some(Instant::now()));
        let elapsed = format_duration(elapsed);
        if accepted {
            self.best_bits.set(Some(after_bits));
            self.best_blocks.set(blocks);
            self.lines.line(format_args!(
                "       {} Replay {round} · {elapsed} · {before_bits} → {after_bits} bits · saved {} · accepted as {blocks} {}",
                self.lines.success(),
                format_bit_count(before_bits.saturating_sub(after_bits)),
                plural(blocks, "block", "blocks")
            ));
        } else {
            self.lines.line(format_args!(
                "       {} Replay {round} · {elapsed} · {before_bits} → {after_bits} bits · no strict improvement; route stabilized",
                self.lines.neutral(),
            ));
        }
    }

    pub(crate) fn replay_stopped(&self, round: usize, elapsed: Duration, reason: &'static str) {
        if self.enabled() {
            self.lines.line(format_args!(
                "       {} Replay {round} · {} · {reason}",
                self.lines.warning(),
                format_duration(elapsed)
            ));
        }
    }

    pub(crate) fn stopped(&self, reason: &'static str) {
        if self.enabled() {
            self.lines.line(format_args!(
                "       {} {reason} · {} route elapsed",
                self.lines.neutral(),
                self.route_elapsed()
            ));
        }
    }
}

/// About 30 heartbeats over the route's budget, from 3 seconds to 1 minute
/// apart.
fn route_heartbeat_interval(budget: Duration) -> Duration {
    (budget / 30).clamp(MIN_ROUTE_HEARTBEAT, MAX_ROUTE_HEARTBEAT)
}

#[cfg(test)]
mod tests;
