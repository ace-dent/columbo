// SPDX-License-Identifier: MIT

//! Dependency-free human progress reporting.
//!
//! Verbose mode is append-only on standard output, so redirected reports remain
//! readable. Visual mode delegates to a bounded-width terminal stream card on
//! standard error. Both consume the same low-frequency optimizer checkpoints.

use std::cell::{Cell, RefCell};
use std::io::{self, Write};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

pub(crate) use crate::presentation::format_duration;
use crate::presentation::{plural, plural_u64};
use crate::{Format, Options};

mod visual;

pub(crate) const MAX_REPORTED_BLOCKS: usize = 64;

static NEXT_STREAM_ID: AtomicUsize = AtomicUsize::new(1);
// A physical stream can have more than one optimizer lineage in flight (for
// example ZIP's Default and direct-Max archive branches). Keep presentation
// identity separate from the human-facing stream number so their concurrent
// updates never overwrite each other.
static NEXT_REPORT_ID: AtomicUsize = AtomicUsize::new(1);
const FIRST_ROUTE_HEARTBEAT: Duration = Duration::from_secs(2);
const MIN_ROUTE_HEARTBEAT: Duration = Duration::from_secs(3);
const MAX_ROUTE_HEARTBEAT: Duration = Duration::from_secs(60);

#[derive(Clone, Copy, PartialEq, Eq)]
enum ProgressMode {
    Disabled,
    Verbose,
    Visual,
}

impl ProgressMode {
    fn for_options(options: &Options) -> Self {
        if visual::enabled(options) {
            Self::Visual
        } else if options.verbose {
            Self::Verbose
        } else {
            Self::Disabled
        }
    }

    fn enabled(self) -> bool {
        !matches!(self, Self::Disabled)
    }

    fn verbose(self) -> bool {
        matches!(self, Self::Verbose)
    }

    fn visual(self) -> bool {
        matches!(self, Self::Visual)
    }
}

thread_local! {
    static STREAM_GROUP: RefCell<Option<StreamGroup>> = const { RefCell::new(None) };
    static ROUTE_LINEAGE: Cell<Option<&'static str>> = const { Cell::new(None) };
}

#[derive(Clone)]
struct StreamGroup {
    id: usize,
    duplicates: Vec<usize>,
    note: Option<&'static str>,
    slice_budget: bool,
}

struct StreamGroupGuard(Option<StreamGroup>);

impl Drop for StreamGroupGuard {
    fn drop(&mut self) {
        STREAM_GROUP.with(|group| {
            group.replace(self.0.take());
        });
    }
}

struct RouteLineageGuard(Option<&'static str>);

impl Drop for RouteLineageGuard {
    fn drop(&mut self) {
        ROUTE_LINEAGE.with(|lineage| lineage.set(self.0.take()));
    }
}

/// Add context to every stream optimized by one concurrent container branch.
///
/// ZIP Max can evaluate a Default floor and a direct-Max archive at the same
/// time. Both operate on the same physical stream numbers, so this label lets
/// human reports distinguish the two trials without changing optimizer data.
pub(crate) fn with_route_lineage<T>(note: &'static str, operation: impl FnOnce() -> T) -> T {
    let previous = ROUTE_LINEAGE.with(|lineage| lineage.replace(Some(note)));
    let _guard = RouteLineageGuard(previous);
    operation()
}

/// Associate one optimizer invocation with its physical container stream.
///
/// Exact APNG duplicates retain their own file-order identifiers while sharing
/// one optimizer invocation. The visual title names those duplicate streams
/// instead of presenting shared work as missing cards.
pub(crate) fn with_stream_group<T>(
    id: usize,
    duplicates: &[usize],
    operation: impl FnOnce() -> T,
) -> T {
    with_stream_context(id, duplicates, None, false, operation)
}

/// Associate an optimizer invocation with a proportional stream search slice.
pub(crate) fn with_stream_slice<T>(
    id: usize,
    duplicates: &[usize],
    note: Option<&'static str>,
    operation: impl FnOnce() -> T,
) -> T {
    with_stream_context(id, duplicates, note, true, operation)
}

pub(crate) fn with_stream_reclaim<T>(
    id: usize,
    duplicates: &[usize],
    slice_budget: bool,
    operation: impl FnOnce() -> T,
) -> T {
    with_stream_context(
        id,
        duplicates,
        Some("reclaimed time"),
        slice_budget,
        operation,
    )
}

fn with_stream_context<T>(
    id: usize,
    duplicates: &[usize],
    note: Option<&'static str>,
    slice_budget: bool,
    operation: impl FnOnce() -> T,
) -> T {
    let note = note.or_else(|| ROUTE_LINEAGE.with(Cell::get));
    let previous = STREAM_GROUP.with(|group| {
        group.replace(Some(StreamGroup {
            id,
            duplicates: duplicates.to_vec(),
            note,
            slice_budget,
        }))
    });
    let _guard = StreamGroupGuard(previous);
    operation()
}

fn current_stream_group() -> Option<StreamGroup> {
    STREAM_GROUP.with(|group| group.borrow().clone())
}

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
    pub(crate) total_bits: u64,
}

#[derive(Debug, Clone)]
pub(crate) struct CandidateProgress {
    pub(crate) bytes: usize,
    pub(crate) bits: u64,
    pub(crate) report: Option<BlockReport>,
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
    mode: ProgressMode,
    optimizer_started: Instant,
    report_id: usize,
    slice_budget: bool,
    stream_id: usize,
}

impl Progress {
    pub(crate) fn begin(
        options: &Options,
        optimizer_started: Instant,
        stream: StreamProgress,
        source_report: Option<BlockReport>,
    ) -> Self {
        let mode = ProgressMode::for_options(options);
        if !mode.enabled() {
            return Self {
                color: false,
                mode,
                optimizer_started,
                report_id: 0,
                slice_budget: false,
                stream_id: 0,
            };
        }

        let group = current_stream_group();
        let stream_id = group.as_ref().map_or_else(
            || NEXT_STREAM_ID.fetch_add(1, Ordering::Relaxed),
            |group| group.id,
        );
        let color = mode.verbose() && terminal_color_enabled();
        let report_id = NEXT_REPORT_ID.fetch_add(1, Ordering::Relaxed);
        if mode.visual() {
            visual::begin(
                report_id,
                stream_id,
                group
                    .as_ref()
                    .map_or(&[][..], |group| group.duplicates.as_slice()),
                group.as_ref().and_then(|group| group.note),
                &stream,
                source_report,
            );
        } else {
            let duplicates = group.as_ref().map_or_else(String::new, |group| {
                duplicate_stream_suffix(&group.duplicates)
            });
            let note = group
                .as_ref()
                .and_then(|group| group.note)
                .map_or_else(String::new, |note| format!(" · {note}"));
            // One stdout lock keeps another worker from inserting its stream
            // header between these related lines. Every detail line also
            // carries the physical stream number for later interleaved work.
            let mut output = io::stdout().lock();
            let _ = writeln!(output);
            let _ = writeln!(
                output,
                "{}Deflate stream {stream_id}{note}{duplicates}{}",
                cyan(color),
                reset(color)
            );
            let _ = writeln!(
                output,
                "  S{stream_id} Source · {} {} · {} meaningful {} · {} {}{}",
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
            let _ = writeln!(
                output,
                "  S{stream_id} Parsed · {} · {} decoded {}",
                format_duration(stream.parse_elapsed),
                stream.decoded_bytes,
                plural_u64(stream.decoded_bytes, "byte", "bytes"),
            );
        }

        Self {
            color,
            mode,
            optimizer_started,
            report_id,
            slice_budget: group.is_some_and(|group| group.slice_budget),
            stream_id,
        }
    }

    pub(crate) fn enabled(self) -> bool {
        self.mode.enabled()
    }

    pub(crate) fn normalization(self, blocks: usize, elapsed: Duration) {
        if !self.mode.enabled() || blocks == 0 {
            return;
        }
        if self.mode.visual() {
            visual::activity(
                self.report_id,
                &format!("Strict normalization · {blocks} blocks"),
            );
        }
        if !self.mode.verbose() {
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
        if !self.mode.enabled() {
            return;
        }
        if self.mode.visual() {
            let status = if report.runs == 0 {
                "Source parsed · no adjacent match runs".to_owned()
            } else {
                format!(
                    "Source parsed · {} match runs · up to {} removable tokens",
                    report.runs, report.tokens_removable
                )
            };
            visual::activity(self.report_id, &status);
        }
        if !self.mode.verbose() {
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
        if self.mode.visual() {
            visual::activity(self.report_id, "Choosing optimization routes");
        } else if self.mode.enabled() {
            println!();
            println!("  S{} Routes", self.stream_id);
        }
    }

    pub(crate) fn start(self, name: &'static str) -> RouteStep {
        if self.mode.enabled() {
            if self.mode.visual() {
                visual::route_started(self.report_id, name);
            } else {
                println!("  S{} {} {name}…", self.stream_id, arrow(self.color));
            }
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
        let route_started = self.mode.enabled().then(Instant::now);
        let details = RouteProgress {
            best_bits: Cell::new(None),
            best_blocks: Cell::new(0),
            color: self.color,
            completed: Cell::new(0),
            current_tokens: Cell::new(None),
            deadline_reached: Cell::new(false),
            mode: self.mode,
            finalizing_after_soft_deadline: Cell::new(false),
            heartbeat_emitted: Cell::new(false),
            heartbeat_interval,
            last_reported: Cell::new(route_started),
            phase: Cell::new("Starting"),
            phase_started: Cell::new(route_started),
            reference_bits,
            report_id: self.report_id,
            route_started,
            stream_id: self.stream_id,
            total: Cell::new(0),
            unit_plural: Cell::new("items"),
            unit_singular: Cell::new("item"),
        };
        (step, details)
    }

    pub(crate) fn candidate(self, name: &'static str, candidate: CandidateProgress) {
        if !self.mode.enabled() {
            return;
        }
        if self.mode.visual() {
            visual::candidate(self.report_id, name, &candidate);
        }
        if !self.mode.verbose() {
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
        if self.mode.visual() {
            visual::activity(self.report_id, &format!("{name} skipped · {reason}"));
        } else if self.mode.enabled() {
            println!(
                "  S{}   {} {name} skipped · {reason}",
                self.stream_id,
                neutral(self.color)
            );
        }
    }

    pub(crate) fn blocks(self, report: Option<BlockReport>) {
        if !self.mode.enabled() {
            return;
        }
        if self.mode.visual() {
            visual::final_plan(self.report_id, report.as_ref());
        }
        if !self.mode.verbose() {
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
        if !self.mode.enabled() {
            return;
        }
        if self.mode.visual() {
            visual::finish(
                self.stream_id,
                selected_route,
                output_bytes,
                output_bits,
                source_bits,
                self.optimizer_started.elapsed(),
                timed_out,
                self.slice_budget,
            );
        }
        if !self.mode.verbose() {
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
                if self.slice_budget {
                    " · search slice reached"
                } else {
                    " · deadline reached"
                }
            } else {
                ""
            }
        );
    }
}

/// Live context for one long-running route.
///
/// Every method is a no-op when progress reporting is disabled. Interior
/// mutability lets the deadline callback and planner share this reporter
/// without changing the optimizer's ownership or cancellation model.
pub(crate) struct RouteProgress {
    best_bits: Cell<Option<u64>>,
    best_blocks: Cell<usize>,
    color: bool,
    completed: Cell<usize>,
    current_tokens: Cell<Option<usize>>,
    deadline_reached: Cell<bool>,
    mode: ProgressMode,
    finalizing_after_soft_deadline: Cell<bool>,
    heartbeat_emitted: Cell<bool>,
    heartbeat_interval: Duration,
    last_reported: Cell<Option<Instant>>,
    phase: Cell<&'static str>,
    phase_started: Cell<Option<Instant>>,
    reference_bits: u64,
    report_id: usize,
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
            mode: ProgressMode::Disabled,
            finalizing_after_soft_deadline: Cell::new(false),
            heartbeat_emitted: Cell::new(false),
            heartbeat_interval: MAX_ROUTE_HEARTBEAT,
            last_reported: Cell::new(None),
            phase: Cell::new(""),
            phase_started: Cell::new(None),
            reference_bits: 0,
            report_id: 0,
            route_started: None,
            stream_id: 0,
            total: Cell::new(0),
            unit_plural: Cell::new("items"),
            unit_singular: Cell::new("item"),
        }
    }

    pub(crate) fn enabled(&self) -> bool {
        self.mode.enabled()
    }

    pub(crate) fn deadline_reached(&self) {
        if self.mode.enabled() {
            self.deadline_reached.set(true);
        }
    }

    pub(crate) fn finalizing_after_soft_deadline(&self, grace: Duration) {
        if !self.mode.enabled() || self.finalizing_after_soft_deadline.replace(true) {
            return;
        }
        if self.mode.visual() {
            visual::activity(
                self.report_id,
                &format!(
                    "Soft deadline · finalizing with {} grace",
                    format_duration(grace)
                ),
            );
        }
        if !self.mode.verbose() {
            return;
        }
        println!(
            "  S{}       · Soft deadline reached · finalizing active candidate · up to {} grace",
            self.stream_id,
            format_duration(grace),
        );
    }

    pub(crate) fn deadline_was_reached(&self) -> bool {
        self.mode.enabled() && self.deadline_reached.get()
    }

    pub(crate) fn phase(
        &self,
        name: &'static str,
        total: usize,
        singular: &'static str,
        plural: &'static str,
    ) {
        if !self.mode.enabled() {
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
        if self.mode.visual() {
            visual::work(self.report_id, name, 0, total);
        }
        if !self.mode.verbose() {
            return;
        }
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
        if self.mode.enabled() {
            self.completed.set(completed.min(self.total.get()));
            if self.mode.visual() {
                visual::work(
                    self.report_id,
                    self.phase.get(),
                    self.completed.get(),
                    self.total.get(),
                );
            }
        }
    }

    /// Identify the block currently being searched for heartbeat context.
    pub(crate) fn item(&self, current: usize, tokens: usize) {
        if self.mode.enabled() {
            self.completed.set(current.min(self.total.get()));
            self.current_tokens.set(Some(tokens));
            if self.mode.visual() {
                visual::work(
                    self.report_id,
                    self.phase.get(),
                    self.completed.get(),
                    self.total.get(),
                );
            }
        }
    }

    /// Refine the current activity shown by the next heartbeat.
    ///
    /// Unlike `phase`, this emits no immediate line and does not reset the
    /// silence timer. It is safe to call at existing block/merge boundaries.
    pub(crate) fn activity(&self, name: &'static str) {
        if self.mode.enabled() {
            self.phase.set(name);
            if self.mode.visual() {
                visual::work(self.report_id, name, self.completed.get(), self.total.get());
            }
        }
    }

    /// Print a periodic status line from an existing deadline probe.
    pub(crate) fn heartbeat(&self) {
        if !self.mode.enabled() {
            return;
        }
        if self.mode.visual() {
            visual::work(
                self.report_id,
                self.phase.get(),
                self.completed.get(),
                self.total.get(),
            );
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
        if !self.mode.enabled() {
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

        if self.mode.visual() {
            visual::checkpoint(self.report_id, name, bits, blocks, improved);
        }
        if !self.mode.verbose() {
            return;
        }

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
        if !self.mode.enabled() {
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
        if self.mode.visual() {
            visual::work(
                self.report_id,
                "Replanning emitted stream",
                round.saturating_sub(1),
                limit,
            );
        }
        if !self.mode.verbose() {
            return;
        }
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
        if !self.mode.enabled() {
            return;
        }
        self.completed.set(round);
        self.last_reported.set(Some(Instant::now()));
        if accepted {
            self.best_bits.set(Some(after_bits));
            self.best_blocks.set(blocks);
            if self.mode.visual() {
                visual::checkpoint(self.report_id, "Replay accepted", after_bits, blocks, true);
            }
            if !self.mode.verbose() {
                return;
            }
            println!(
                "  S{}       {} Replay {round} · {} · {before_bits} → {after_bits} bits · saved {} · accepted as {blocks} {}",
                self.stream_id,
                success(self.color),
                format_duration(elapsed),
                format_bit_count(before_bits.saturating_sub(after_bits)),
                plural(blocks, "block", "blocks")
            );
        } else {
            if self.mode.visual() {
                visual::activity(self.report_id, "Replay stable · best retained");
            }
            if !self.mode.verbose() {
                return;
            }
            println!(
                "  S{}       {} Replay {round} · {} · {before_bits} → {after_bits} bits · no strict improvement; route stabilized",
                self.stream_id,
                neutral(self.color),
                format_duration(elapsed)
            );
        }
    }

    pub(crate) fn replay_stopped(&self, round: usize, elapsed: Duration, reason: &'static str) {
        if self.mode.visual() {
            visual::activity(
                self.report_id,
                &format!("Replay {round} stopped · {reason}"),
            );
        } else if self.mode.enabled() {
            println!(
                "  S{}       {} Replay {round} · {} · {reason}",
                self.stream_id,
                warning(self.color),
                format_duration(elapsed)
            );
        }
    }

    pub(crate) fn stopped(&self, reason: &'static str) {
        if self.mode.visual() {
            visual::activity(self.report_id, reason);
        } else if self.mode.enabled() {
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
        if self.progress.mode.visual() {
            match candidate.as_ref() {
                Some(candidate) => visual::candidate(self.progress.report_id, self.name, candidate),
                None => visual::activity(
                    self.progress.report_id,
                    &format!("{} · no candidate", self.name),
                ),
            }
        }
        if !self.progress.mode.verbose() {
            return;
        }
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
        if self.progress.mode.visual() {
            visual::activity(self.progress.report_id, &format!("{} complete", self.name));
        }
        if !self.progress.mode.verbose() {
            return;
        }
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
        if self.progress.mode.visual() {
            visual::activity(self.progress.report_id, &format!("{} failed", self.name));
        }
        if !self.progress.mode.verbose() {
            return;
        }
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

pub(crate) fn format_detected(options: &Options, format: Format, deflate_streams: Option<usize>) {
    let label = match format {
        Format::Auto => "raw Deflate",
        Format::Raw => "raw Deflate",
        Format::Png => "PNG",
        Format::Zlib => "zlib",
        Format::Gzip => "GZIP",
        Format::Zip => "ZIP",
    };
    match ProgressMode::for_options(options) {
        ProgressMode::Visual => visual::format_detected(label, deflate_streams),
        ProgressMode::Verbose => {
            let _ = write_format_summary(&mut io::stdout().lock(), label, deflate_streams);
        }
        ProgressMode::Disabled => {}
    }
}

fn write_format_summary(
    output: &mut dyn Write,
    format: &str,
    deflate_streams: Option<usize>,
) -> io::Result<()> {
    writeln!(output, "Format   {format}")?;
    if let Some(deflate_streams) = deflate_streams {
        writeln!(output, "Deflate streams  {deflate_streams}")?;
    }
    Ok(())
}

/// Whether block-plan snapshots will be consumed by a progress renderer.
pub(crate) fn reports_enabled(options: &Options) -> bool {
    ProgressMode::for_options(options).enabled()
}

fn duplicate_stream_suffix(duplicates: &[usize]) -> String {
    if duplicates.is_empty() {
        return String::new();
    }
    let identifiers = duplicates
        .iter()
        .map(|stream| format!("{stream:02}"))
        .collect::<Vec<_>>()
        .join(", ");
    format!(" · duplicates {identifiers}")
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
    crate::terminal::stdout_color_enabled()
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
    fn verbose_and_visual_share_the_format_summary_rows() {
        let mut summary = Vec::new();
        write_format_summary(&mut summary, "ZIP", Some(3)).unwrap();
        assert_eq!(
            String::from_utf8(summary).unwrap(),
            "Format   ZIP\nDeflate streams  3\n"
        );
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
