// SPDX-License-Identifier: MIT

//! Dependency-free human optimization reporting.
//!
//! Worker threads cache stream-labelled events instead of writing concurrently.
//! A file-level coordinator appends each completed physical stream as soon as
//! every one of its producers has finished and all earlier streams are ready.

use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, BTreeSet};
use std::env;
use std::fmt::{self, Write as _};
use std::io::{self, IsTerminal, Write};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

pub(crate) use crate::presentation::format_duration;
use crate::presentation::{countdown_seconds, plural, plural_u64, write_spinner_line};
use crate::{Format, Options};

mod visual;

pub(crate) const MAX_REPORTED_BLOCKS: usize = 64;

static NEXT_STREAM_ID: AtomicUsize = AtomicUsize::new(1);
// A physical stream can have more than one optimizer lineage in flight (for
// example ZIP's Default and direct-Max archive branches). Keep presentation
// identity separate from the human-facing stream number so their concurrent
// updates never overwrite each other.
static NEXT_REPORT_ID: AtomicUsize = AtomicUsize::new(1);
static VERBOSE_REPORTS: OnceLock<Mutex<VerboseReports>> = OnceLock::new();
static REPORT_SPINNER: OnceLock<Mutex<Option<ReportSpinner>>> = OnceLock::new();
static REPORT_COORDINATOR: OnceLock<Mutex<ReportCoordinator>> = OnceLock::new();
static REPORT_EMITTER: OnceLock<Mutex<()>> = OnceLock::new();
static SPINNER_PROGRESS_DONE: AtomicUsize = AtomicUsize::new(0);
static SPINNER_PROGRESS_TOTAL: AtomicUsize = AtomicUsize::new(0);
pub(crate) const PRIMARY_STREAM_PRODUCER: u8 = 0;
const MAX_CACHED_VERBOSE_BYTES: usize = 64 * 1_024 * 1_024;
const FIRST_ROUTE_HEARTBEAT: Duration = Duration::from_secs(2);
const MIN_ROUTE_HEARTBEAT: Duration = Duration::from_secs(3);
const MAX_ROUTE_HEARTBEAT: Duration = Duration::from_secs(60);
const SPINNER_DELAY: Duration = Duration::from_millis(300);
const SPINNER_INTERVAL: Duration = Duration::from_millis(500);

struct ReportSpinner {
    running: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
}

impl ReportSpinner {
    fn start(deadline: Instant) -> Option<Self> {
        if !io::stderr().is_terminal() || env::var_os("TERM").is_some_and(|term| term == "dumb") {
            return None;
        }

        let running = Arc::new(AtomicBool::new(true));
        let worker_running = Arc::clone(&running);
        let color = crate::terminal::stderr_color_enabled();
        let worker = thread::Builder::new()
            .name("columbo-report-spinner".into())
            .spawn(move || {
                const FRAMES: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
                let mut frame = 0;
                let mut drawn = false;
                thread::park_timeout(SPINNER_DELAY);
                while worker_running.load(Ordering::Relaxed) {
                    let done = SPINNER_PROGRESS_DONE.load(Ordering::Relaxed);
                    let total = SPINNER_PROGRESS_TOTAL.load(Ordering::Relaxed);
                    let seconds =
                        countdown_seconds(deadline.saturating_duration_since(Instant::now()));
                    {
                        let stderr = io::stderr();
                        let mut output = stderr.lock();
                        let checked = (total != 0).then_some((done, total));
                        let _ =
                            write_spinner_line(&mut output, FRAMES[frame], seconds, checked, color);
                        let _ = output.flush();
                    }
                    drawn = true;
                    frame = (frame + 1) % FRAMES.len();
                    thread::park_timeout(SPINNER_INTERVAL);
                }
                if drawn {
                    eprint!("\r\x1b[K");
                    let _ = io::stderr().flush();
                }
            })
            .ok()?;
        Some(Self {
            running,
            worker: Some(worker),
        })
    }

    fn stop(mut self) {
        self.running.store(false, Ordering::Relaxed);
        if let Some(worker) = self.worker.take() {
            worker.thread().unpark();
            let _ = worker.join();
        }
    }
}

fn start_report_spinner(deadline: Instant) {
    stop_report_spinner();
    let spinner = ReportSpinner::start(deadline);
    let slot = REPORT_SPINNER.get_or_init(|| Mutex::new(None));
    *slot.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) = spinner;
}

fn stop_report_spinner() {
    let Some(slot) = REPORT_SPINNER.get() else {
        return;
    };
    let spinner = slot
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .take();
    if let Some(spinner) = spinner {
        spinner.stop();
    }
}

#[derive(Default)]
struct VerboseReports {
    cached_bytes: usize,
    reports: BTreeMap<usize, VerboseReport>,
}

struct VerboseReport {
    finished: bool,
    order: u8,
    stream_id: usize,
    text: String,
    truncated: bool,
}

impl VerboseReports {
    fn reset(&mut self) {
        self.cached_bytes = 0;
        self.reports.clear();
    }

    fn insert(&mut self, report_id: usize, stream_id: usize, order: u8, mut text: String) {
        let truncated = self.cached_bytes.saturating_add(text.len()) > MAX_CACHED_VERBOSE_BYTES;
        if truncated {
            text.clear();
        } else {
            self.cached_bytes += text.len();
        }
        self.reports.insert(
            report_id,
            VerboseReport {
                finished: false,
                order,
                stream_id,
                text,
                truncated,
            },
        );
    }

    fn append(&mut self, report_id: usize, arguments: fmt::Arguments<'_>) {
        let Some(report) = self.reports.get_mut(&report_id) else {
            return;
        };
        if report.truncated {
            return;
        }
        let mut line = String::new();
        let _ = line.write_fmt(arguments);
        line.push('\n');
        if self.cached_bytes.saturating_add(line.len()) > MAX_CACHED_VERBOSE_BYTES {
            report.truncated = true;
            return;
        }
        self.cached_bytes += line.len();
        report.text.push_str(&line);
    }

    fn finish(&mut self, report_id: usize) {
        if let Some(report) = self.reports.get_mut(&report_id) {
            report.finished = true;
        }
    }

    fn take_finished_stream(&mut self, stream_id: usize) -> Option<Vec<VerboseReport>> {
        if self
            .reports
            .values()
            .any(|report| report.stream_id == stream_id && !report.finished)
        {
            return None;
        }
        let mut report_ids: Vec<_> = self
            .reports
            .iter()
            .filter_map(|(&report_id, report)| (report.stream_id == stream_id).then_some(report_id))
            .collect();
        report_ids.sort_unstable_by_key(|report_id| {
            let report = &self.reports[report_id];
            (report.order, *report_id)
        });
        let mut finished = Vec::with_capacity(report_ids.len());
        for report_id in report_ids {
            let report = self
                .reports
                .remove(&report_id)
                .expect("a collected verbose report remains cached");
            self.cached_bytes = self.cached_bytes.saturating_sub(report.text.len());
            finished.push(report);
        }
        Some(finished)
    }

    fn take_finished_in_stream_order(&mut self) -> Vec<VerboseReport> {
        let mut reports: Vec<_> = std::mem::take(&mut self.reports)
            .into_iter()
            .filter_map(|(report_id, report)| report.finished.then_some((report_id, report)))
            .collect();
        reports.sort_unstable_by_key(|(report_id, report)| {
            (report.stream_id, report.order, *report_id)
        });
        self.cached_bytes = 0;
        reports.into_iter().map(|(_, report)| report).collect()
    }
}

fn with_verbose_reports<T>(operation: impl FnOnce(&mut VerboseReports) -> T) -> T {
    let reports = VERBOSE_REPORTS.get_or_init(|| Mutex::new(VerboseReports::default()));
    let mut reports = reports
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    operation(&mut reports)
}

fn reset_verbose_reports() {
    with_verbose_reports(VerboseReports::reset);
}

fn begin_verbose_report(report_id: usize, stream_id: usize, order: u8, text: String) {
    with_verbose_reports(|reports| reports.insert(report_id, stream_id, order, text));
}

fn append_verbose_report(report_id: usize, arguments: fmt::Arguments<'_>) {
    with_verbose_reports(|reports| reports.append(report_id, arguments));
}

fn finish_verbose_report(report_id: usize) {
    with_verbose_reports(|reports| reports.finish(report_id));
}

fn write_verbose_reports(reports: Vec<VerboseReport>) {
    if reports.is_empty() {
        return;
    }
    let mut output = io::stdout().lock();
    for report in reports {
        let _ = output.write_all(report.text.as_bytes());
        if report.truncated {
            let _ = writeln!(
                output,
                "  S{} … remaining verbose details omitted; 64 MiB report cache reached",
                report.stream_id
            );
        }
    }
    let _ = output.flush();
}

fn flush_verbose_reports() {
    write_verbose_reports(with_verbose_reports(
        VerboseReports::take_finished_in_stream_order,
    ));
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ProgressMode {
    Disabled,
    Verbose,
    Visual,
}

struct ReportCoordinator {
    active: bool,
    checked_count: usize,
    checked_streams: Vec<bool>,
    completed_producers: BTreeMap<usize, BTreeSet<u8>>,
    deadline: Option<Instant>,
    expected_producers: BTreeSet<u8>,
    mode: ProgressMode,
    next_stream_id: usize,
    report_streams: BTreeMap<usize, ReportStreams>,
    sealed_streams: BTreeSet<usize>,
    total_streams: usize,
}

struct ReportStreams {
    duplicates: Vec<usize>,
    primary: usize,
}

impl Default for ReportCoordinator {
    fn default() -> Self {
        Self {
            active: false,
            checked_count: 0,
            checked_streams: Vec::new(),
            completed_producers: BTreeMap::new(),
            deadline: None,
            expected_producers: BTreeSet::new(),
            mode: ProgressMode::Disabled,
            next_stream_id: 1,
            report_streams: BTreeMap::new(),
            sealed_streams: BTreeSet::new(),
            total_streams: 0,
        }
    }
}

impl ReportCoordinator {
    fn reset(&mut self, mode: ProgressMode, total_streams: usize, timeout: Duration) {
        self.active = mode.enabled();
        self.checked_count = 0;
        self.checked_streams.clear();
        self.checked_streams
            .resize(total_streams.saturating_add(1), false);
        self.completed_producers.clear();
        self.deadline = Instant::now().checked_add(timeout);
        self.expected_producers.clear();
        self.expected_producers.insert(PRIMARY_STREAM_PRODUCER);
        self.mode = mode;
        self.next_stream_id = 1;
        self.report_streams.clear();
        self.sealed_streams.clear();
        self.total_streams = total_streams;
        self.publish_spinner_progress();
    }

    fn set_expected_producers(&mut self, producers: &[u8]) {
        self.expected_producers.clear();
        self.expected_producers.extend(producers.iter().copied());
        for stream_id in self.next_stream_id..=self.total_streams {
            self.update_sealed(stream_id);
        }
    }

    fn register_report(&mut self, report_id: usize, stream_id: usize, duplicates: &[usize]) {
        self.report_streams.insert(
            report_id,
            ReportStreams {
                duplicates: duplicates.to_vec(),
                primary: stream_id,
            },
        );
    }

    fn finish_report(&mut self, report_id: usize) {
        let Some(streams) = self.report_streams.remove(&report_id) else {
            return;
        };
        for stream_id in std::iter::once(streams.primary).chain(streams.duplicates) {
            let Some(checked) = self.checked_streams.get_mut(stream_id) else {
                continue;
            };
            if !*checked {
                *checked = true;
                self.checked_count = self.checked_count.saturating_add(1);
            }
        }
        self.publish_spinner_progress();
    }

    fn complete(&mut self, stream_id: usize, duplicates: &[usize], producer: u8) {
        for stream_id in std::iter::once(stream_id).chain(duplicates.iter().copied()) {
            if stream_id < self.next_stream_id || stream_id > self.total_streams {
                continue;
            }
            self.completed_producers
                .entry(stream_id)
                .or_default()
                .insert(producer);
            self.update_sealed(stream_id);
        }
    }

    fn complete_all(&mut self, producer: u8) {
        for stream_id in self.next_stream_id..=self.total_streams {
            self.completed_producers
                .entry(stream_id)
                .or_default()
                .insert(producer);
            self.update_sealed(stream_id);
        }
    }

    fn update_sealed(&mut self, stream_id: usize) {
        if self
            .completed_producers
            .get(&stream_id)
            .is_some_and(|done| {
                self.expected_producers
                    .iter()
                    .all(|producer| done.contains(producer))
            })
        {
            self.sealed_streams.insert(stream_id);
        } else {
            self.sealed_streams.remove(&stream_id);
        }
    }

    fn next_sealed(&self) -> Option<usize> {
        (self.next_stream_id <= self.total_streams
            && self.sealed_streams.contains(&self.next_stream_id))
        .then_some(self.next_stream_id)
    }

    fn advance(&mut self, stream_id: usize) {
        debug_assert_eq!(self.next_stream_id, stream_id);
        self.sealed_streams.remove(&stream_id);
        self.completed_producers.remove(&stream_id);
        self.next_stream_id = self.next_stream_id.saturating_add(1);
    }

    fn finish(&mut self) {
        self.active = false;
        for stream_id in self.next_stream_id..=self.total_streams {
            self.sealed_streams.insert(stream_id);
        }
    }

    fn publish_spinner_progress(&self) {
        SPINNER_PROGRESS_DONE.store(self.checked_count, Ordering::Relaxed);
        SPINNER_PROGRESS_TOTAL.store(self.total_streams, Ordering::Relaxed);
    }
}

fn with_report_coordinator<T>(operation: impl FnOnce(&mut ReportCoordinator) -> T) -> T {
    let coordinator = REPORT_COORDINATOR.get_or_init(|| Mutex::new(ReportCoordinator::default()));
    let mut coordinator = coordinator
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    operation(&mut coordinator)
}

/// Declare the complete set of independent producers for every physical stream.
///
/// ZIP Max registers its archive lineages before starting them. A stream is
/// released only after all registered lineages, including their reclaim
/// passes, have reported completion.
pub(crate) fn set_stream_producers(producers: &[u8]) {
    with_report_coordinator(|coordinator| coordinator.set_expected_producers(producers));
    emit_ready_streams();
}

pub(crate) fn complete_stream_group(id: usize, duplicates: &[usize]) {
    complete_stream_producer(id, duplicates, PRIMARY_STREAM_PRODUCER);
}

pub(crate) fn complete_stream_producer(id: usize, duplicates: &[usize], producer: u8) {
    with_report_coordinator(|coordinator| coordinator.complete(id, duplicates, producer));
    emit_ready_streams();
}

pub(crate) fn complete_all_stream_producers(producer: u8) {
    with_report_coordinator(|coordinator| coordinator.complete_all(producer));
    emit_ready_streams();
}

fn emit_ready_streams() {
    let emitter = REPORT_EMITTER.get_or_init(|| Mutex::new(()));
    let _emitter = emitter
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let mut verbose = Vec::new();
    let mut visual_cards = Vec::new();

    while let Some((mode, stream_id)) = with_report_coordinator(|coordinator| {
        coordinator
            .next_sealed()
            .map(|stream_id| (coordinator.mode, stream_id))
    }) {
        let ready = match mode {
            ProgressMode::Verbose => {
                let reports =
                    with_verbose_reports(|reports| reports.take_finished_stream(stream_id));
                reports.map(|reports| {
                    verbose.extend(reports);
                })
            }
            ProgressMode::Visual => visual::take_finished_stream(stream_id).map(|cards| {
                visual_cards.extend(cards);
            }),
            ProgressMode::Disabled => Some(()),
        };
        if ready.is_none() {
            break;
        }
        with_report_coordinator(|coordinator| coordinator.advance(stream_id));
    }

    if verbose.is_empty() && visual_cards.is_empty() {
        return;
    }
    stop_report_spinner();
    write_verbose_reports(verbose);
    visual::emit_cards(visual_cards);
    let restart = with_report_coordinator(|coordinator| {
        coordinator.active.then_some(coordinator.deadline).flatten()
    });
    if let Some(deadline) = restart {
        start_report_spinner(deadline);
    }
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

pub(crate) fn current_route_lineage() -> Option<&'static str> {
    ROUTE_LINEAGE.with(Cell::get)
}

pub(crate) fn with_optional_route_lineage<T>(
    note: Option<&'static str>,
    operation: impl FnOnce() -> T,
) -> T {
    match note {
        Some(note) => with_route_lineage(note, operation),
        None => operation(),
    }
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

#[derive(Debug, Clone, Copy)]
pub(crate) struct BalancedTreeProgress {
    pub(crate) dynamic_blocks: usize,
    pub(crate) literal_pair_moves: usize,
    pub(crate) literal_quad_moves: usize,
    pub(crate) distance_pair_moves: usize,
    pub(crate) distance_quad_moves: usize,
    pub(crate) paired_prices: usize,
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
            let mut report = String::new();
            let _ = writeln!(report);
            let _ = writeln!(
                report,
                "{}Deflate stream {stream_id}{note}{duplicates}{}",
                cyan(color),
                reset(color)
            );
            let _ = writeln!(
                report,
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
                report,
                "  S{stream_id} Parsed · {} · {} decoded {}",
                format_duration(stream.parse_elapsed),
                stream.decoded_bytes,
                plural_u64(stream.decoded_bytes, "byte", "bytes"),
            );
            begin_verbose_report(
                report_id,
                stream_id,
                report_order(group.as_ref().and_then(|group| group.note)),
                report,
            );
        }
        with_report_coordinator(|coordinator| {
            coordinator.register_report(
                report_id,
                stream_id,
                group
                    .as_ref()
                    .map_or(&[][..], |group| group.duplicates.as_slice()),
            )
        });

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

    fn write_verbose(self, arguments: fmt::Arguments<'_>) {
        append_verbose_report(self.report_id, arguments);
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
        self.write_verbose(format_args!(
            "  S{} {} Strict normalization · {} · canonicalized {} {}",
            self.stream_id,
            success(self.color),
            format_duration(elapsed),
            blocks,
            plural(blocks, "block", "blocks")
        ));
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
            self.write_verbose(format_args!(
                "  S{} Matches · no adjacent same-distance runs in the joined source token stream",
                self.stream_id
            ));
            return;
        }
        self.write_verbose(format_args!(
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
        ));
    }

    pub(crate) fn balanced_tree_opportunities(self, report: BalancedTreeProgress) {
        if !self.mode.enabled() {
            return;
        }
        let total_moves = report
            .literal_pair_moves
            .saturating_add(report.literal_quad_moves)
            .saturating_add(report.distance_pair_moves)
            .saturating_add(report.distance_quad_moves);
        if self.mode.visual() {
            visual::activity(
                self.report_id,
                &format!(
                    "Source trees · {total_moves} balanced moves · up to {} paired prices",
                    report.paired_prices
                ),
            );
        }
        if !self.mode.verbose() {
            return;
        }
        self.write_verbose(format_args!(
            "  S{} Trees · {} dynamic {} · literal/length: {} pair + {} quad · distance: {} pair + {} quad · up to {} paired {}",
            self.stream_id,
            report.dynamic_blocks,
            plural(report.dynamic_blocks, "block", "blocks"),
            report.literal_pair_moves,
            report.literal_quad_moves,
            report.distance_pair_moves,
            report.distance_quad_moves,
            report.paired_prices,
            plural(report.paired_prices, "price", "prices"),
        ));
    }

    pub(crate) fn routes(self) {
        if self.mode.visual() {
            visual::activity(self.report_id, "Choosing optimization routes");
        } else if self.mode.enabled() {
            self.write_verbose(format_args!(""));
            self.write_verbose(format_args!("  S{} Routes", self.stream_id));
        }
    }

    pub(crate) fn start(self, name: &'static str) -> RouteStep {
        if self.mode.enabled() {
            if self.mode.visual() {
                visual::route_started(self.report_id, name);
            } else {
                self.write_verbose(format_args!(
                    "  S{} {} {name}…",
                    self.stream_id,
                    arrow(self.color)
                ));
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
        self.write_verbose(format_args!(
            "  S{}   {} {name} · {} {} / {} {} · {}",
            self.stream_id,
            marker,
            candidate.bytes,
            plural(candidate.bytes, "byte", "bytes"),
            candidate.bits,
            plural_u64(candidate.bits, "bit", "bits"),
            describe_bit_change(candidate.reference_bits, candidate.bits)
        ));
    }

    pub(crate) fn skipped(self, name: &'static str, reason: &'static str) {
        if self.mode.visual() {
            visual::activity(self.report_id, &format!("{name} skipped · {reason}"));
        } else if self.mode.enabled() {
            self.write_verbose(format_args!(
                "  S{}   {} {name} skipped · {reason}",
                self.stream_id,
                neutral(self.color)
            ));
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
        self.write_verbose(format_args!(""));
        self.write_verbose(format_args!("  S{} Final block plan", self.stream_id));
        let Some(report) = report else {
            self.write_verbose(format_args!(
                "  S{}   · Details unavailable",
                self.stream_id
            ));
            return;
        };

        for (index, block) in report.blocks.iter().enumerate() {
            self.write_verbose(format_args!(
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
            ));
        }
        if report.total_blocks > report.blocks.len() {
            let omitted = report.total_blocks - report.blocks.len();
            self.write_verbose(format_args!(
                "  S{}   … {} additional {} omitted",
                self.stream_id,
                omitted,
                plural(omitted, "block", "blocks"),
            ));
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
                self.report_id,
                selected_route,
                output_bytes,
                output_bits,
                source_bits,
                self.optimizer_started.elapsed(),
                timed_out,
                self.slice_budget,
            );
        }
        if self.mode.verbose() {
            self.write_verbose(format_args!(
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
            ));
            finish_verbose_report(self.report_id);
        }
        with_report_coordinator(|coordinator| coordinator.finish_report(self.report_id));
        // A scheduler may already have sealed this stream while the final
        // report update was racing to the cache. Recheck the ordered head.
        emit_ready_streams();
    }
}

/// Cached context for one long-running route.
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

    fn write_verbose(&self, arguments: fmt::Arguments<'_>) {
        append_verbose_report(self.report_id, arguments);
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
        self.write_verbose(format_args!(
            "  S{}       · Soft deadline reached · finalizing active candidate · up to {} grace",
            self.stream_id,
            format_duration(grace),
        ));
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
        self.write_verbose(format_args!(
            "  S{}     {} {name} · {} {}",
            self.stream_id,
            arrow(self.color),
            total,
            self::plural(total, singular, plural)
        ));
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

    /// Cache a periodic status line from an existing deadline probe.
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
        self.write_verbose(format_args!(
            "  S{}       {} {} · {} route elapsed{work}{tokens}{best}",
            self.stream_id,
            neutral(self.color),
            self.phase.get(),
            format_duration(
                self.route_started
                    .expect("enabled route progress has a start time")
                    .elapsed()
            )
        ));
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
            self.write_verbose(format_args!(
                "  S{}       {} {name} · {} · {bits} {} in {blocks} {} · {}",
                self.stream_id,
                success(self.color),
                format_duration(elapsed),
                plural_u64(bits, "bit", "bits"),
                plural(blocks, "block", "blocks"),
                describe_bit_change(self.reference_bits, bits)
            ));
        } else {
            let behind = bits.saturating_sub(previous.unwrap_or(bits));
            if behind == 0 {
                self.write_verbose(format_args!(
                    "  S{}       {} {name} · {} · {bits} {} in {blocks} {} · matches current best",
                    self.stream_id,
                    neutral(self.color),
                    format_duration(elapsed),
                    plural_u64(bits, "bit", "bits"),
                    plural(blocks, "block", "blocks")
                ));
            } else {
                self.write_verbose(format_args!(
                    "  S{}       {} {name} · {} · {bits} {} in {blocks} {} · {} behind best",
                    self.stream_id,
                    neutral(self.color),
                    format_duration(elapsed),
                    plural_u64(bits, "bit", "bits"),
                    plural(blocks, "block", "blocks"),
                    format_bit_count(behind)
                ));
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
        self.write_verbose(format_args!(
            "  S{}     {} Replay {round}/{limit} · reparsing {blocks} {} at {bits} {}",
            self.stream_id,
            arrow(self.color),
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
            self.write_verbose(format_args!(
                "  S{}       {} Replay {round} · {} · {before_bits} → {after_bits} bits · saved {} · accepted as {blocks} {}",
                self.stream_id,
                success(self.color),
                format_duration(elapsed),
                format_bit_count(before_bits.saturating_sub(after_bits)),
                plural(blocks, "block", "blocks")
            ));
        } else {
            if self.mode.visual() {
                visual::activity(self.report_id, "Replay stable · best retained");
            }
            if !self.mode.verbose() {
                return;
            }
            self.write_verbose(format_args!(
                "  S{}       {} Replay {round} · {} · {before_bits} → {after_bits} bits · no strict improvement; route stabilized",
                self.stream_id,
                neutral(self.color),
                format_duration(elapsed)
            ));
        }
    }

    pub(crate) fn replay_stopped(&self, round: usize, elapsed: Duration, reason: &'static str) {
        if self.mode.visual() {
            visual::activity(
                self.report_id,
                &format!("Replay {round} stopped · {reason}"),
            );
        } else if self.mode.enabled() {
            self.write_verbose(format_args!(
                "  S{}       {} Replay {round} · {} · {reason}",
                self.stream_id,
                warning(self.color),
                format_duration(elapsed)
            ));
        }
    }

    pub(crate) fn stopped(&self, reason: &'static str) {
        if self.mode.visual() {
            visual::activity(self.report_id, reason);
        } else if self.mode.enabled() {
            self.write_verbose(format_args!(
                "  S{}       {} {reason} · {} route elapsed",
                self.stream_id,
                neutral(self.color),
                format_duration(
                    self.route_started
                        .expect("enabled route progress has a start time")
                        .elapsed()
                )
            ));
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
                self.progress.write_verbose(format_args!(
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
                ));
            }
            None => self.progress.write_verbose(format_args!(
                "  S{} {} {} · {} · no candidate",
                self.progress.stream_id,
                neutral(self.progress.color),
                self.name,
                format_duration(elapsed)
            )),
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
        self.progress.write_verbose(format_args!(
            "  S{} {} {} · {}",
            self.progress.stream_id,
            success(self.progress.color),
            self.name,
            format_duration(started.elapsed())
        ));
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
        self.progress.write_verbose(format_args!(
            "  S{} {} {} · {} · failed",
            self.progress.stream_id,
            warning(self.progress.color),
            self.name,
            format_duration(started.elapsed())
        ));
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
    let mode = ProgressMode::for_options(options);
    match mode {
        ProgressMode::Visual => {
            NEXT_STREAM_ID.store(1, Ordering::Relaxed);
            visual::format_detected(label, deflate_streams);
        }
        ProgressMode::Verbose => {
            NEXT_STREAM_ID.store(1, Ordering::Relaxed);
            reset_verbose_reports();
            let _ = write_format_summary(&mut io::stdout().lock(), label, deflate_streams);
        }
        ProgressMode::Disabled => {}
    }
    let deadline = with_report_coordinator(|coordinator| {
        coordinator.reset(mode, deflate_streams.unwrap_or(0), options.timeout);
        coordinator.deadline
    });
    if mode.enabled() {
        if let Some(deadline) = deadline {
            start_report_spinner(deadline);
        }
    }
}

/// Emit any remaining stream reports after container work has joined.
///
/// Ordinarily each sealed physical stream has already been appended in order.
/// Force-sealing here retains error-path reports when a container returned
/// before it could publish all of its normal scheduler completion events.
pub(crate) fn finish_file(options: &Options) {
    with_report_coordinator(ReportCoordinator::finish);
    stop_report_spinner();
    emit_ready_streams();
    match ProgressMode::for_options(options) {
        ProgressMode::Visual => visual::finish_file(),
        ProgressMode::Verbose => flush_verbose_reports(),
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

fn report_order(note: Option<&str>) -> u8 {
    match note {
        Some("metadata probe") => 0,
        Some("default floor") => 1,
        Some("direct max") => 2,
        None => 3,
        Some("refined default") => 4,
        Some("reclaimed time") => 5,
        Some(_) => 3,
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
    fn cached_verbose_reports_flush_in_physical_stream_order() {
        let mut reports = VerboseReports::default();
        reports.insert(30, 1, 0, "stream one\n".to_owned());
        reports.insert(10, 3, 0, "stream three\n".to_owned());
        reports.insert(20, 2, 0, "stream two\n".to_owned());

        // Completion order deliberately differs from source order.
        reports.finish(10);
        reports.finish(20);
        reports.finish(30);
        let ordered: Vec<_> = reports
            .take_finished_in_stream_order()
            .into_iter()
            .map(|report| (report.stream_id, report.text))
            .collect();

        assert_eq!(
            ordered,
            [
                (1, "stream one\n".to_owned()),
                (2, "stream two\n".to_owned()),
                (3, "stream three\n".to_owned()),
            ]
        );
    }

    #[test]
    fn completed_streams_release_only_the_contiguous_physical_prefix() {
        let mut coordinator = ReportCoordinator::default();
        coordinator.reset(ProgressMode::Verbose, 4, Duration::from_secs(30));

        coordinator.complete(3, &[], PRIMARY_STREAM_PRODUCER);
        coordinator.complete(1, &[2], PRIMARY_STREAM_PRODUCER);
        assert_eq!(coordinator.next_sealed(), Some(1));
        coordinator.advance(1);
        assert_eq!(coordinator.next_sealed(), Some(2));
        coordinator.advance(2);
        assert_eq!(coordinator.next_sealed(), Some(3));
        coordinator.advance(3);
        assert_eq!(coordinator.next_sealed(), None);

        coordinator.complete(4, &[], PRIMARY_STREAM_PRODUCER);
        assert_eq!(coordinator.next_sealed(), Some(4));
    }

    #[test]
    fn checked_progress_counts_each_physical_stream_once() {
        let mut coordinator = ReportCoordinator::default();
        coordinator.reset(ProgressMode::Verbose, 4, Duration::from_secs(30));

        coordinator.register_report(10, 3, &[]);
        coordinator.finish_report(10);
        assert_eq!(coordinator.checked_count, 1);

        coordinator.register_report(11, 1, &[2]);
        coordinator.finish_report(11);
        assert_eq!(coordinator.checked_count, 3);

        coordinator.register_report(12, 1, &[]);
        coordinator.finish_report(12);
        assert_eq!(coordinator.checked_count, 3);
    }

    #[test]
    fn every_registered_lineage_must_finish_before_a_stream_is_sealed() {
        let mut coordinator = ReportCoordinator::default();
        coordinator.reset(ProgressMode::Visual, 2, Duration::from_secs(30));
        coordinator.set_expected_producers(&[1, 2, 3]);

        coordinator.complete(1, &[], 2);
        coordinator.complete(1, &[], 1);
        assert_eq!(coordinator.next_sealed(), None);
        coordinator.complete(2, &[], 1);
        coordinator.complete(2, &[], 2);
        coordinator.complete(2, &[], 3);
        assert_eq!(coordinator.next_sealed(), None);
        coordinator.complete(1, &[], 3);
        assert_eq!(coordinator.next_sealed(), Some(1));
    }

    #[test]
    fn one_finished_stream_can_be_removed_without_draining_later_reports() {
        let mut reports = VerboseReports::default();
        reports.insert(1, 2, 0, "stream two\n".to_owned());
        reports.insert(2, 1, 1, "stream one second\n".to_owned());
        reports.insert(3, 1, 0, "stream one first\n".to_owned());
        reports.finish(1);
        reports.finish(2);
        reports.finish(3);

        let first: Vec<_> = reports
            .take_finished_stream(1)
            .unwrap()
            .into_iter()
            .map(|report| report.text)
            .collect();
        assert_eq!(first, ["stream one first\n", "stream one second\n"]);
        assert_eq!(reports.reports.len(), 1);
        assert_eq!(reports.reports[&1].stream_id, 2);
    }

    #[test]
    fn incomplete_verbose_reports_are_not_emitted() {
        let mut reports = VerboseReports::default();
        reports.insert(1, 1, 0, "complete\n".to_owned());
        reports.insert(2, 2, 0, "partial\n".to_owned());
        reports.finish(1);

        let ordered = reports.take_finished_in_stream_order();
        assert_eq!(ordered.len(), 1);
        assert_eq!(ordered[0].stream_id, 1);
        assert_eq!(ordered[0].text, "complete\n");
    }

    #[test]
    fn cached_lineages_have_a_stable_order_within_one_stream() {
        let mut reports = VerboseReports::default();
        reports.insert(
            1,
            4,
            report_order(Some("refined default")),
            "refined".to_owned(),
        );
        reports.insert(2, 4, report_order(Some("direct max")), "direct".to_owned());
        reports.insert(
            3,
            4,
            report_order(Some("default floor")),
            "floor".to_owned(),
        );
        reports.finish(1);
        reports.finish(2);
        reports.finish(3);

        let ordered: Vec<_> = reports
            .take_finished_in_stream_order()
            .into_iter()
            .map(|report| report.text)
            .collect();
        assert_eq!(ordered, ["floor", "direct", "refined"]);
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
