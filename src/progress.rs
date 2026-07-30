// SPDX-License-Identifier: MIT

//! Lightweight, line-oriented progress reporting for human test runs.
//!
//! The renderer writes the verbose report to standard output and is
//! intentionally dependency-free and append-only. Redirected output therefore
//! remains readable, while terminals receive restrained colour without cursor
//! movement or a competing spinner.

use std::cell::Cell;
use std::env;
use std::io::{self, IsTerminal};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use crate::{Format, Options};

pub(crate) const MAX_REPORTED_BLOCKS: usize = 64;

static NEXT_STREAM_ID: AtomicUsize = AtomicUsize::new(1);
const FIRST_ROUTE_HEARTBEAT: Duration = Duration::from_secs(2);
const MIN_ROUTE_HEARTBEAT: Duration = Duration::from_secs(3);
const MAX_ROUTE_HEARTBEAT: Duration = Duration::from_secs(60);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BlockEncoding {
    Original,
    Stored,
    Fixed,
    Dynamic,
}

impl BlockEncoding {
    fn label(self) -> &'static str {
        match self {
            Self::Original => "original",
            Self::Stored => "stored",
            Self::Fixed => "fixed",
            Self::Dynamic => "dynamic",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct BlockProgress {
    pub(crate) alignment: u8,
    pub(crate) decoded_bytes: usize,
    pub(crate) final_block: bool,
    pub(crate) input: BlockEncoding,
    pub(crate) output: BlockEncoding,
    pub(crate) output_bits: u64,
    pub(crate) tokens: usize,
}

#[derive(Debug, Clone)]
pub(crate) struct BlockReport {
    pub(crate) blocks: Vec<BlockProgress>,
    pub(crate) total_blocks: usize,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct CandidateProgress {
    pub(crate) bytes: usize,
    pub(crate) bits: u64,
    pub(crate) reference_bits: u64,
    /// Whether this complete candidate is strictly smaller than its source.
    ///
    /// Several routes can be profitable at once. The final stream line, rather
    /// than this marker, identifies the candidate Columbo ultimately selected.
    pub(crate) profitable: bool,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct SameDistanceProgress {
    pub(crate) runs: usize,
    pub(crate) matches: usize,
    pub(crate) decoded_bytes: usize,
    pub(crate) coalescible_runs: usize,
    pub(crate) repartition_runs: usize,
    pub(crate) tokens_removable: usize,
}

pub(crate) struct StreamProgress {
    pub(crate) blocks: usize,
    pub(crate) compressed_bytes: usize,
    pub(crate) decoded_bytes: u64,
    pub(crate) empty_blocks: usize,
    pub(crate) meaningful_bits: u64,
    pub(crate) parse_elapsed: Duration,
}

#[derive(Clone, Copy)]
pub(crate) struct Progress {
    color: bool,
    enabled: bool,
    optimizer_started: Instant,
    stream_id: usize,
}

impl Progress {
    pub(crate) fn begin(
        options: &Options,
        optimizer_started: Instant,
        stream: StreamProgress,
    ) -> Self {
        if !options.verbose {
            return Self {
                color: false,
                enabled: false,
                optimizer_started,
                stream_id: 0,
            };
        }

        let stream_id = NEXT_STREAM_ID.fetch_add(1, Ordering::Relaxed);
        let color = terminal_color_enabled();
        println!();
        println!("{}Deflate stream {stream_id}{}", cyan(color), reset(color));
        println!(
            "  Source  {} {} · {} meaningful {} · {} {}{}",
            stream.compressed_bytes,
            plural(stream.compressed_bytes, "byte", "bytes"),
            stream.meaningful_bits,
            plural_u64(stream.meaningful_bits, "bit", "bits"),
            stream.blocks,
            plural(stream.blocks, "block", "blocks"),
            if stream.empty_blocks == 0 {
                String::new()
            } else {
                format!(
                    " · {} empty {}",
                    stream.empty_blocks,
                    plural(stream.empty_blocks, "block", "blocks")
                )
            }
        );
        println!(
            "  Parsed  {} · {} decoded {}",
            format_duration(stream.parse_elapsed),
            stream.decoded_bytes,
            plural_u64(stream.decoded_bytes, "byte", "bytes"),
        );

        Self {
            color,
            enabled: true,
            optimizer_started,
            stream_id,
        }
    }

    pub(crate) fn enabled(self) -> bool {
        self.enabled
    }

    pub(crate) fn normalization(self, blocks: usize, elapsed: Duration) {
        if !self.enabled || blocks == 0 {
            return;
        }
        println!(
            "  S{} {} Strict normalization · {} · canonicalized {} {}",
            self.stream_id,
            success(self.color),
            format_duration(elapsed),
            blocks,
            plural(blocks, "block", "blocks")
        );
    }

    pub(crate) fn same_distance_opportunities(self, report: SameDistanceProgress) {
        if !self.enabled {
            return;
        }
        if report.runs == 0 {
            println!(
                "  S{} Matches · no adjacent same-distance runs in the joined source token stream",
                self.stream_id
            );
            return;
        }
        println!(
            "  S{} Matches · joined source token stream · {} {} / {} {} / {} decoded {} · {} direct {} · {} {} · up to {} removable {}",
            self.stream_id,
            report.runs,
            plural(report.runs, "run", "runs"),
            report.matches,
            plural(report.matches, "match", "matches"),
            report.decoded_bytes,
            plural(report.decoded_bytes, "byte", "bytes"),
            report.coalescible_runs,
            plural(report.coalescible_runs, "coalesce", "coalesces"),
            report.repartition_runs,
            plural(report.repartition_runs, "repartition", "repartitions"),
            report.tokens_removable,
            plural(report.tokens_removable, "token", "tokens"),
        );
    }

    pub(crate) fn routes(self) {
        if self.enabled {
            println!();
            println!("  S{} Routes", self.stream_id);
        }
    }

    pub(crate) fn start(self, name: &'static str) -> RouteStep {
        if self.enabled {
            println!("  S{} {} {name}…", self.stream_id, arrow(self.color));
            RouteStep {
                name,
                progress: self,
                started: Some(Instant::now()),
            }
        } else {
            RouteStep {
                name,
                progress: self,
                started: None,
            }
        }
    }

    /// Start a route whose nested work can take long enough to need context.
    ///
    /// The returned reporter is deliberately single-threaded. Long searches
    /// already check their deadline frequently, so those checks also provide a
    /// natural, low-overhead opportunity to print a heartbeat.
    pub(crate) fn start_detailed(
        self,
        name: &'static str,
        reference_bits: u64,
        budget: Duration,
    ) -> (RouteStep, RouteProgress) {
        let step = self.start(name);
        let heartbeat_interval = route_heartbeat_interval(budget);
        let route_started = self.enabled.then(Instant::now);
        let details = RouteProgress {
            best_bits: Cell::new(None),
            best_blocks: Cell::new(0),
            color: self.color,
            completed: Cell::new(0),
            current_tokens: Cell::new(None),
            deadline_reached: Cell::new(false),
            enabled: self.enabled,
            finalizing_after_soft_deadline: Cell::new(false),
            heartbeat_emitted: Cell::new(false),
            heartbeat_interval,
            last_reported: Cell::new(route_started),
            phase: Cell::new("Starting"),
            phase_started: Cell::new(route_started),
            reference_bits,
            route_started,
            stream_id: self.stream_id,
            total: Cell::new(0),
            unit_plural: Cell::new("items"),
            unit_singular: Cell::new("item"),
        };
        (step, details)
    }

    pub(crate) fn candidate(self, name: &'static str, candidate: CandidateProgress) {
        if !self.enabled {
            return;
        }
        let marker = if candidate.profitable {
            success(self.color)
        } else {
            neutral(self.color)
        };
        println!(
            "  S{}   {} {name} · {} {} / {} {} · {}",
            self.stream_id,
            marker,
            candidate.bytes,
            plural(candidate.bytes, "byte", "bytes"),
            candidate.bits,
            plural_u64(candidate.bits, "bit", "bits"),
            describe_bit_change(candidate.reference_bits, candidate.bits)
        );
    }

    pub(crate) fn skipped(self, name: &'static str, reason: &'static str) {
        if self.enabled {
            println!(
                "  S{}   {} {name} skipped · {reason}",
                self.stream_id,
                neutral(self.color)
            );
        }
    }

    pub(crate) fn blocks(self, report: Option<BlockReport>) {
        if !self.enabled {
            return;
        }
        println!();
        println!("  S{} Final block plan", self.stream_id);
        let Some(report) = report else {
            println!("  S{}   · Details unavailable", self.stream_id);
            return;
        };

        for (index, block) in report.blocks.iter().enumerate() {
            println!(
                "  S{}   #{:<3} {:>8} → {:<8} · {} decoded {} · {} {} · {} {} · starts at bit {}{}",
                self.stream_id,
                index + 1,
                block.input.label(),
                block.output.label(),
                block.decoded_bytes,
                plural(block.decoded_bytes, "byte", "bytes"),
                block.tokens,
                plural(block.tokens, "token", "tokens"),
                block.output_bits,
                plural_u64(block.output_bits, "bit", "bits"),
                block.alignment,
                if block.final_block { " · final" } else { "" }
            );
        }
        if report.total_blocks > report.blocks.len() {
            let omitted = report.total_blocks - report.blocks.len();
            println!(
                "  S{}   … {} additional {} omitted",
                self.stream_id,
                omitted,
                plural(omitted, "block", "blocks"),
            );
        }
    }

    pub(crate) fn finish(
        self,
        selected_route: &'static str,
        output_bytes: usize,
        output_bits: u64,
        source_bits: u64,
        timed_out: bool,
    ) {
        if !self.enabled {
            return;
        }
        println!(
            "  S{} {} Selected {selected_route} · {} {} / {} {} · {} · {}{}",
            self.stream_id,
            success(self.color),
            output_bytes,
            plural(output_bytes, "byte", "bytes"),
            output_bits,
            plural_u64(output_bits, "bit", "bits"),
            describe_bit_change(source_bits, output_bits),
            format_duration(self.optimizer_started.elapsed()),
            if timed_out {
                " · deadline reached"
            } else {
                ""
            }
        );
    }
}

/// Live context for one long-running route.
///
/// Every method is a no-op when verbose reporting is disabled. Interior
/// mutability lets the deadline callback and planner share this reporter
/// without changing the optimizer's ownership or cancellation model.
pub(crate) struct RouteProgress {
    best_bits: Cell<Option<u64>>,
    best_blocks: Cell<usize>,
    color: bool,
    completed: Cell<usize>,
    current_tokens: Cell<Option<usize>>,
    deadline_reached: Cell<bool>,
    enabled: bool,
    finalizing_after_soft_deadline: Cell<bool>,
    heartbeat_emitted: Cell<bool>,
    heartbeat_interval: Duration,
    last_reported: Cell<Option<Instant>>,
    phase: Cell<&'static str>,
    phase_started: Cell<Option<Instant>>,
    reference_bits: u64,
    route_started: Option<Instant>,
    stream_id: usize,
    total: Cell<usize>,
    unit_plural: Cell<&'static str>,
    unit_singular: Cell<&'static str>,
}

impl RouteProgress {
    pub(crate) fn disabled() -> Self {
        Self {
            best_bits: Cell::new(None),
            best_blocks: Cell::new(0),
            color: false,
            completed: Cell::new(0),
            current_tokens: Cell::new(None),
            deadline_reached: Cell::new(false),
            enabled: false,
            finalizing_after_soft_deadline: Cell::new(false),
            heartbeat_emitted: Cell::new(false),
            heartbeat_interval: MAX_ROUTE_HEARTBEAT,
            last_reported: Cell::new(None),
            phase: Cell::new(""),
            phase_started: Cell::new(None),
            reference_bits: 0,
            route_started: None,
            stream_id: 0,
            total: Cell::new(0),
            unit_plural: Cell::new("items"),
            unit_singular: Cell::new("item"),
        }
    }

    pub(crate) fn enabled(&self) -> bool {
        self.enabled
    }

    pub(crate) fn deadline_reached(&self) {
        if self.enabled {
            self.deadline_reached.set(true);
        }
    }

    pub(crate) fn finalizing_after_soft_deadline(&self, grace: Duration) {
        if !self.enabled || self.finalizing_after_soft_deadline.replace(true) {
            return;
        }
        println!(
            "  S{}       · Soft deadline reached · finalizing active candidate · up to {} grace",
            self.stream_id,
            format_duration(grace),
        );
    }

    pub(crate) fn deadline_was_reached(&self) -> bool {
        self.enabled && self.deadline_reached.get()
    }

    pub(crate) fn phase(
        &self,
        name: &'static str,
        total: usize,
        singular: &'static str,
        plural: &'static str,
    ) {
        if !self.enabled {
            return;
        }
        self.phase.set(name);
        self.completed.set(0);
        self.current_tokens.set(None);
        self.total.set(total);
        self.unit_singular.set(singular);
        self.unit_plural.set(plural);
        let now = Instant::now();
        self.last_reported.set(Some(now));
        self.phase_started.set(Some(now));
        println!(
            "  S{}     {} {name} · {} {}",
            self.stream_id,
            arrow(self.color),
            total,
            self::plural(total, singular, plural)
        );
    }

    /// Update the current work position without printing a line per item.
    pub(crate) fn advance(&self, completed: usize) {
        if self.enabled {
            self.completed.set(completed.min(self.total.get()));
        }
    }

    /// Identify the block currently being searched for heartbeat context.
    pub(crate) fn item(&self, current: usize, tokens: usize) {
        if self.enabled {
            self.completed.set(current.min(self.total.get()));
            self.current_tokens.set(Some(tokens));
        }
    }

    /// Refine the current activity shown by the next heartbeat.
    ///
    /// Unlike `phase`, this emits no immediate line and does not reset the
    /// silence timer. It is safe to call at existing block/merge boundaries.
    pub(crate) fn activity(&self, name: &'static str) {
        if self.enabled {
            self.phase.set(name);
        }
    }

    /// Print a periodic status line from an existing deadline probe.
    pub(crate) fn heartbeat(&self) {
        if !self.enabled {
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
                self::plural(total, self.unit_singular.get(), self.unit_plural.get())
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
        println!(
            "  S{}       {} {} · {} route elapsed{work}{tokens}{best}",
            self.stream_id,
            neutral(self.color),
            self.phase.get(),
            format_duration(
                self.route_started
                    .expect("enabled route progress has a start time")
                    .elapsed()
            )
        );
    }

    /// Report one complete internal candidate and retain the best bit count.
    pub(crate) fn checkpoint(&self, name: &'static str, bits: u64, blocks: usize) {
        if !self.enabled {
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

        if improved {
            println!(
                "  S{}       {} {name} · {} · {bits} {} in {blocks} {} · {}",
                self.stream_id,
                success(self.color),
                format_duration(elapsed),
                plural_u64(bits, "bit", "bits"),
                plural(blocks, "block", "blocks"),
                describe_bit_change(self.reference_bits, bits)
            );
        } else {
            let behind = bits.saturating_sub(previous.unwrap_or(bits));
            if behind == 0 {
                println!(
                    "  S{}       {} {name} · {} · {bits} {} in {blocks} {} · matches current best",
                    self.stream_id,
                    neutral(self.color),
                    format_duration(elapsed),
                    plural_u64(bits, "bit", "bits"),
                    plural(blocks, "block", "blocks")
                );
            } else {
                println!(
                    "  S{}       {} {name} · {} · {bits} {} in {blocks} {} · {} behind best",
                    self.stream_id,
                    neutral(self.color),
                    format_duration(elapsed),
                    plural_u64(bits, "bit", "bits"),
                    plural(blocks, "block", "blocks"),
                    format_bit_count(behind)
                );
            }
        }
    }

    pub(crate) fn replay_started(&self, round: usize, limit: usize, blocks: usize, bits: u64) {
        if !self.enabled {
            return;
        }
        self.phase.set("Replanning emitted stream");
        self.completed.set(round.saturating_sub(1));
        self.current_tokens.set(None);
        self.total.set(limit);
        self.unit_singular.set("replay");
        self.unit_plural.set("replays");
        let now = Instant::now();
        self.last_reported.set(Some(now));
        self.phase_started.set(Some(now));
        println!(
            "  S{}     {} Replay {round}/{limit} · reparsing {blocks} {} at {bits} {}",
            self.stream_id,
            arrow(self.color),
            plural(blocks, "block", "blocks"),
            plural_u64(bits, "bit", "bits")
        );
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
        if !self.enabled {
            return;
        }
        self.completed.set(round);
        self.last_reported.set(Some(Instant::now()));
        if accepted {
            self.best_bits.set(Some(after_bits));
            self.best_blocks.set(blocks);
            println!(
                "  S{}       {} Replay {round} · {} · {before_bits} → {after_bits} bits · saved {} · accepted as {blocks} {}",
                self.stream_id,
                success(self.color),
                format_duration(elapsed),
                format_bit_count(before_bits.saturating_sub(after_bits)),
                plural(blocks, "block", "blocks")
            );
        } else {
            println!(
                "  S{}       {} Replay {round} · {} · {before_bits} → {after_bits} bits · no strict improvement; route stabilized",
                self.stream_id,
                neutral(self.color),
                format_duration(elapsed)
            );
        }
    }

    pub(crate) fn replay_stopped(&self, round: usize, elapsed: Duration, reason: &'static str) {
        if self.enabled {
            println!(
                "  S{}       {} Replay {round} · {} · {reason}",
                self.stream_id,
                warning(self.color),
                format_duration(elapsed)
            );
        }
    }

    pub(crate) fn stopped(&self, reason: &'static str) {
        if self.enabled {
            println!(
                "  S{}       {} {reason} · {} route elapsed",
                self.stream_id,
                neutral(self.color),
                format_duration(
                    self.route_started
                        .expect("enabled route progress has a start time")
                        .elapsed()
                )
            );
        }
    }
}

pub(crate) struct RouteStep {
    name: &'static str,
    progress: Progress,
    started: Option<Instant>,
}

impl RouteStep {
    pub(crate) fn finish(self, candidate: Option<CandidateProgress>) {
        let Some(started) = self.started else {
            return;
        };
        let elapsed = started.elapsed();
        match candidate {
            Some(candidate) => {
                let marker = if candidate.profitable {
                    success(self.progress.color)
                } else {
                    neutral(self.progress.color)
                };
                println!(
                    "  S{} {} {} · {} · {} {} / {} {} · {}",
                    self.progress.stream_id,
                    marker,
                    self.name,
                    format_duration(elapsed),
                    candidate.bytes,
                    plural(candidate.bytes, "byte", "bytes"),
                    candidate.bits,
                    plural_u64(candidate.bits, "bit", "bits"),
                    describe_bit_change(candidate.reference_bits, candidate.bits)
                );
            }
            None => println!(
                "  S{} {} {} · {} · no candidate",
                self.progress.stream_id,
                neutral(self.progress.color),
                self.name,
                format_duration(elapsed)
            ),
        }
    }

    pub(crate) fn finish_phase(self) {
        let Some(started) = self.started else {
            return;
        };
        println!(
            "  S{} {} {} · {}",
            self.progress.stream_id,
            success(self.progress.color),
            self.name,
            format_duration(started.elapsed())
        );
    }

    pub(crate) fn fail(self) {
        let Some(started) = self.started else {
            return;
        };
        println!(
            "  S{} {} {} · {} · failed",
            self.progress.stream_id,
            warning(self.progress.color),
            self.name,
            format_duration(started.elapsed())
        );
    }
}

fn route_heartbeat_interval(budget: Duration) -> Duration {
    (budget / 30).clamp(MIN_ROUTE_HEARTBEAT, MAX_ROUTE_HEARTBEAT)
}

pub(crate) fn format_detected(options: &Options, format: Format) {
    if !options.verbose {
        return;
    }
    let label = match format {
        Format::Auto => "raw Deflate",
        Format::Raw => "raw Deflate",
        Format::Png => "PNG",
        Format::Zlib => "zlib",
        Format::Gzip => "GZIP",
        Format::Zip => "ZIP",
    };
    println!("Format   {label}");
}

pub(crate) fn format_duration(duration: Duration) -> String {
    if duration < Duration::from_millis(1) {
        format!("{} µs", duration.subsec_micros())
    } else if duration < Duration::from_secs(1) {
        let centimilliseconds = (duration.subsec_micros() + 5) / 10;
        format!(
            "{}.{:02} ms",
            centimilliseconds / 100,
            centimilliseconds % 100
        )
    } else if duration < Duration::from_secs(10) {
        let mut seconds = duration.as_secs();
        let mut centiseconds = (duration.subsec_micros() + 5_000) / 10_000;
        if centiseconds == 100 {
            seconds += 1;
            centiseconds = 0;
        }
        format!("{seconds}.{centiseconds:02} s")
    } else {
        let mut seconds = duration.as_secs();
        let mut deciseconds = (duration.subsec_millis() + 50) / 100;
        if deciseconds == 10 {
            seconds = seconds.saturating_add(1);
            deciseconds = 0;
        }
        format!("{seconds}.{deciseconds} s")
    }
}

pub(crate) fn describe_bit_change(reference_bits: u64, candidate_bits: u64) -> String {
    match reference_bits.cmp(&candidate_bits) {
        std::cmp::Ordering::Greater => {
            let saved = reference_bits - candidate_bits;
            format!(
                "saved {saved} {} vs source",
                plural_u64(saved, "bit", "bits")
            )
        }
        std::cmp::Ordering::Less => {
            let added = candidate_bits - reference_bits;
            format!(
                "added {added} {} vs source",
                plural_u64(added, "bit", "bits")
            )
        }
        std::cmp::Ordering::Equal => "same bit length as source".to_owned(),
    }
}

fn format_bit_count(bits: u64) -> String {
    format!("{bits} {}", plural_u64(bits, "bit", "bits"))
}

fn terminal_color_enabled() -> bool {
    io::stdout().is_terminal()
        && env::var_os("NO_COLOR").is_none()
        && env::var_os("TERM").map_or(true, |term| term != "dumb")
}

fn plural<'a>(count: usize, singular: &'a str, plural: &'a str) -> &'a str {
    if count == 1 {
        singular
    } else {
        plural
    }
}

fn plural_u64<'a>(count: u64, singular: &'a str, plural: &'a str) -> &'a str {
    if count == 1 {
        singular
    } else {
        plural
    }
}

fn arrow(color: bool) -> &'static str {
    if color {
        "\x1b[36m→\x1b[0m"
    } else {
        "→"
    }
}

fn success(color: bool) -> &'static str {
    if color {
        "\x1b[32m✓\x1b[0m"
    } else {
        "✓"
    }
}

fn neutral(color: bool) -> &'static str {
    if color {
        "\x1b[2m·\x1b[0m"
    } else {
        "·"
    }
}

fn warning(color: bool) -> &'static str {
    if color {
        "\x1b[33m!\x1b[0m"
    } else {
        "!"
    }
}

fn cyan(color: bool) -> &'static str {
    if color {
        "\x1b[36m"
    } else {
        ""
    }
}

fn reset(color: bool) -> &'static str {
    if color {
        "\x1b[0m"
    } else {
        ""
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn duration_units_are_stable_at_boundaries() {
        assert_eq!(format_duration(Duration::ZERO), "0 µs");
        assert_eq!(format_duration(Duration::from_micros(999)), "999 µs");
        assert_eq!(format_duration(Duration::from_millis(1)), "1.00 ms");
        assert_eq!(format_duration(Duration::from_millis(999)), "999.00 ms");
        assert_eq!(format_duration(Duration::from_secs(1)), "1.00 s");
        assert_eq!(format_duration(Duration::from_secs(10)), "10.0 s");
    }

    #[test]
    fn bit_change_wording_handles_savings_ties_and_growth() {
        assert_eq!(describe_bit_change(100, 99), "saved 1 bit vs source");
        assert_eq!(describe_bit_change(100, 90), "saved 10 bits vs source");
        assert_eq!(describe_bit_change(100, 100), "same bit length as source");
        assert_eq!(describe_bit_change(100, 101), "added 1 bit vs source");
        assert_eq!(describe_bit_change(100, 111), "added 11 bits vs source");
    }

    #[test]
    fn long_route_heartbeat_cadence_is_bounded() {
        assert_eq!(
            route_heartbeat_interval(Duration::from_secs(10)),
            Duration::from_secs(3)
        );
        assert_eq!(
            route_heartbeat_interval(Duration::from_secs(180)),
            Duration::from_secs(6)
        );
        assert_eq!(
            route_heartbeat_interval(Duration::from_secs(4_000)),
            Duration::from_secs(60)
        );
    }
}
