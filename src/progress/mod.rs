// SPDX-License-Identifier: MIT
// SPDX-FileCopyrightText: 2026 Andrew C.E. Dent <https://github.com/ace-dent>

//! Verbose and visual optimization reports.
//!
//! Worker threads buffer events per stream instead of writing concurrently. A
//! file-level coordinator prints each physical stream, in file order, as soon
//! as all its producers have finished and every earlier stream is printed.

use std::cell::{Cell, RefCell};
use std::fmt::{self, Write as _};
use std::io::{self, Write};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Mutex, MutexGuard, OnceLock};
use std::time::{Duration, Instant};

pub(crate) use crate::terminal::formatting::format_duration;
use crate::terminal::formatting::{plural, plural_u64};
use crate::terminal::spinner::Spinner;
use crate::{Format, Options};

mod verbose;
mod visual;

pub(crate) const MAX_REPORTED_BLOCKS: usize = 64;

static NEXT_STREAM_ID: AtomicUsize = AtomicUsize::new(1);
/// Report IDs are separate from displayed stream numbers, so concurrent
/// lineages of one stream never overwrite each other.
static NEXT_REPORT_ID: AtomicUsize = AtomicUsize::new(1);
static REPORT_SPINNER: OnceLock<Mutex<Option<Spinner>>> = OnceLock::new();
static REPORT_COORDINATOR: OnceLock<Mutex<ReportCoordinator>> = OnceLock::new();
static REPORT_EMITTER: OnceLock<Mutex<()>> = OnceLock::new();
static REPORT_SESSION: Mutex<()> = Mutex::new(());
pub(crate) const PRIMARY_STREAM_PRODUCER: u8 = 0;
const FIRST_ROUTE_HEARTBEAT: Duration = Duration::from_secs(2);
const MIN_ROUTE_HEARTBEAT: Duration = Duration::from_secs(3);
const MAX_ROUTE_HEARTBEAT: Duration = Duration::from_secs(60);

/// Insertion sort for tiny report collections; keeps the general slice sort out
/// of cold code.
#[cold]
#[inline(never)]
fn sort_tiny_by<T>(values: &mut [T], mut compare: impl FnMut(&T, &T) -> std::cmp::Ordering) {
    for index in 1..values.len() {
        let mut current = index;
        while current != 0 && compare(&values[current], &values[current - 1]).is_lt() {
            values.swap(current, current - 1);
            current -= 1;
        }
    }
}

fn start_report_spinner(deadline: Instant) {
    stop_report_spinner();
    let spinner = Spinner::start(true, deadline);
    let slot = REPORT_SPINNER.get_or_init(|| Mutex::new(None));
    *slot.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(spinner);
}

fn stop_report_spinner() {
    let Some(slot) = REPORT_SPINNER.get() else {
        return;
    };
    let spinner = slot
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .take();
    if let Some(mut spinner) = spinner {
        spinner.stop();
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ProgressMode {
    Disabled,
    Verbose,
    Visual,
}

/// Report entries by report ID, sorted by ID. Callers sort what they print, so
/// a sorted vector replaces a `BTreeMap`, whose code is compiled again for
/// each value type.
struct IdMap<T>(Vec<(usize, T)>);

impl<T> Default for IdMap<T> {
    fn default() -> Self {
        Self(Vec::new())
    }
}

impl<T> IdMap<T> {
    fn position(&self, id: usize) -> Result<usize, usize> {
        self.0.binary_search_by_key(&id, |&(key, _)| key)
    }

    fn insert(&mut self, id: usize, value: T) {
        match self.position(id) {
            Ok(index) => self.0[index].1 = value,
            Err(index) => self.0.insert(index, (id, value)),
        }
    }

    fn get_mut(&mut self, id: usize) -> Option<&mut T> {
        let index = self.position(id).ok()?;
        Some(&mut self.0[index].1)
    }

    fn remove(&mut self, id: usize) -> Option<T> {
        let index = self.position(id).ok()?;
        Some(self.0.remove(index).1)
    }

    fn clear(&mut self) {
        self.0.clear();
    }

    fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    fn iter(&self) -> impl Iterator<Item = (usize, &T)> {
        self.0.iter().map(|(id, value)| (*id, value))
    }

    fn values(&self) -> impl Iterator<Item = &T> {
        self.0.iter().map(|(_, value)| value)
    }

    fn take(&mut self) -> Vec<(usize, T)> {
        std::mem::take(&mut self.0)
    }
}

impl<T> std::ops::Index<usize> for IdMap<T> {
    type Output = T;

    fn index(&self, id: usize) -> &T {
        let index = self.position(id).expect("report ID is present");
        &self.0[index].1
    }
}

/// A set of report producers, one bit per `u8` tag.
#[derive(Clone, Copy, Default)]
struct ProducerSet([u64; 4]);

impl ProducerSet {
    fn insert(&mut self, producer: u8) {
        self.0[usize::from(producer / 64)] |= 1 << (producer % 64);
    }

    fn contains_all(&self, other: &Self) -> bool {
        self.0
            .iter()
            .zip(&other.0)
            .all(|(&mine, &theirs)| theirs & !mine == 0)
    }
}

/// Report state of one stream: which producers have finished it, if any, and
/// whether every expected producer has.
#[derive(Clone, Copy, Default)]
struct StreamState {
    completed: Option<ProducerSet>,
    sealed: bool,
}

struct ReportCoordinator {
    active: bool,
    deadline: Option<Instant>,
    expected_producers: ProducerSet,
    mode: ProgressMode,
    next_stream_id: usize,
    /// Indexed by stream ID, which runs from 1 to `total_streams`; grown on
    /// demand.
    streams: Vec<StreamState>,
    zip_store_changes: Vec<ZipStoreProgress>,
    total_streams: usize,
}

impl Default for ReportCoordinator {
    fn default() -> Self {
        Self {
            active: false,
            deadline: None,
            expected_producers: ProducerSet::default(),
            mode: ProgressMode::Disabled,
            next_stream_id: 1,
            streams: Vec::new(),
            zip_store_changes: Vec::new(),
            total_streams: 0,
        }
    }
}

impl ReportCoordinator {
    fn reset(&mut self, mode: ProgressMode, total_streams: usize, timeout: Duration) {
        self.active = mode.enabled();
        self.deadline = Instant::now().checked_add(timeout);
        self.expected_producers = ProducerSet::default();
        self.expected_producers.insert(PRIMARY_STREAM_PRODUCER);
        self.mode = mode;
        self.next_stream_id = 1;
        self.streams.clear();
        self.zip_store_changes.clear();
        self.total_streams = total_streams;
    }

    fn set_expected_producers(&mut self, producers: &[u8]) {
        self.expected_producers = ProducerSet::default();
        for &producer in producers {
            self.expected_producers.insert(producer);
        }
        for stream_id in self.next_stream_id..=self.total_streams {
            self.update_sealed(stream_id);
        }
    }

    fn stream_mut(&mut self, stream_id: usize) -> &mut StreamState {
        if self.streams.len() <= stream_id {
            self.streams.resize(stream_id + 1, StreamState::default());
        }
        &mut self.streams[stream_id]
    }

    fn complete(&mut self, stream_id: usize, duplicates: &[usize], producer: u8) {
        for stream_id in std::iter::once(stream_id).chain(duplicates.iter().copied()) {
            if stream_id < self.next_stream_id || stream_id > self.total_streams {
                continue;
            }
            self.stream_mut(stream_id)
                .completed
                .get_or_insert_with(ProducerSet::default)
                .insert(producer);
            self.update_sealed(stream_id);
        }
    }

    fn complete_all(&mut self, producer: u8) {
        for stream_id in self.next_stream_id..=self.total_streams {
            self.stream_mut(stream_id)
                .completed
                .get_or_insert_with(ProducerSet::default)
                .insert(producer);
            self.update_sealed(stream_id);
        }
    }

    fn update_sealed(&mut self, stream_id: usize) {
        let expected = self.expected_producers;
        if let Some(stream) = self.streams.get_mut(stream_id) {
            stream.sealed = stream
                .completed
                .is_some_and(|done| done.contains_all(&expected));
        }
    }

    fn next_sealed(&self) -> Option<usize> {
        (self.next_stream_id <= self.total_streams
            && self
                .streams
                .get(self.next_stream_id)
                .is_some_and(|stream| stream.sealed))
        .then_some(self.next_stream_id)
    }

    fn advance(&mut self, stream_id: usize) {
        debug_assert_eq!(self.next_stream_id, stream_id);
        if let Some(stream) = self.streams.get_mut(stream_id) {
            *stream = StreamState::default();
        }
        self.next_stream_id = self.next_stream_id.saturating_add(1);
    }

    fn finish(&mut self) {
        self.active = false;
        for stream_id in self.next_stream_id..=self.total_streams {
            self.stream_mut(stream_id).sealed = true;
        }
    }
}

/// A ZIP entry rewritten from Deflate to Store after all Deflate work.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ZipStoreProgress {
    pub(crate) stream_id: usize,
    pub(crate) deflate_bytes: u32,
    pub(crate) stored_bytes: u32,
}

fn with_report_coordinator<T>(operation: impl FnOnce(&mut ReportCoordinator) -> T) -> T {
    let coordinator = REPORT_COORDINATOR.get_or_init(|| Mutex::new(ReportCoordinator::default()));
    let mut coordinator = coordinator
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    operation(&mut coordinator)
}

/// Declare the producers (such as ZIP Max's archive lineages) of every
/// physical stream; a stream prints only after all of them report completion.
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

/// Record the Store conversions of the selected ZIP lineage, after selection,
/// so reports never show a discarded lineage's changes.
pub(crate) fn zip_store_changes(options: &Options, changes: &[ZipStoreProgress]) {
    if changes.is_empty() || !ProgressMode::for_options(options).enabled() {
        return;
    }
    with_report_coordinator(|coordinator| {
        coordinator.zip_store_changes.clear();
        coordinator.zip_store_changes.extend_from_slice(changes);
    });
}

fn emit_ready_streams() {
    let emitter = REPORT_EMITTER.get_or_init(|| Mutex::new(()));
    let _emitter = emitter
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let mut verbose_reports = Vec::new();
    let mut visual_cards = Vec::new();

    while let Some((mode, stream_id)) = with_report_coordinator(|coordinator| {
        coordinator
            .next_sealed()
            .map(|stream_id| (coordinator.mode, stream_id))
    }) {
        let ready = match mode {
            ProgressMode::Verbose => {
                let reports = verbose::take_finished_stream(stream_id);
                reports.map(|reports| {
                    verbose_reports.extend(reports);
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

    if verbose_reports.is_empty() && visual_cards.is_empty() {
        return;
    }
    stop_report_spinner();
    verbose::write(verbose_reports);
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

/// Label every stream optimized by one concurrent container branch, so reports
/// can tell ZIP Max's Default and direct-Max archives apart.
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
/// Exact APNG `duplicates` share the work but keep their own identifiers.
pub(crate) fn with_stream_group<T>(
    id: usize,
    duplicates: &[usize],
    operation: impl FnOnce() -> T,
) -> T {
    with_stream_context(id, duplicates, None, false, operation)
}

/// Like [`with_stream_group`], for a stream searched within its proportional
/// slice of the file's time budget.
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
            verbose::begin(
                report_id,
                stream_id,
                report_order(group.as_ref().and_then(|group| group.note)),
                report,
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

    fn write_verbose(self, arguments: fmt::Arguments<'_>) {
        verbose::append(self.report_id, arguments);
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

    /// Start a long route that reports its phases and periodic heartbeats.
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
            verbose::finish(self.report_id);
        }
        // The scheduler may have sealed this stream before this final update
        // reached the cache, so recheck the next stream to print.
        emit_ready_streams();
    }
}

/// Buffered progress state for one long-running route; a no-op when reporting
/// is disabled. Interior mutability lets the deadline callback and planner
/// share it.
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
        verbose::append(self.report_id, arguments);
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

    /// Record the block being searched and its token count for the heartbeat.
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

    /// Refine the current activity shown by the next heartbeat. Unlike `phase`,
    /// this prints nothing and does not reset the heartbeat timer.
    pub(crate) fn activity(&self, name: &'static str) {
        if self.mode.enabled() {
            self.phase.set(name);
            if self.mode.visual() {
                visual::work(self.report_id, name, self.completed.get(), self.total.get());
            }
        }
    }

    /// Buffer a periodic status line; called from existing deadline checks.
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
    let mode = ProgressMode::for_options(options);
    if !mode.enabled() {
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
    match mode {
        ProgressMode::Visual => {
            NEXT_STREAM_ID.store(1, Ordering::Relaxed);
            visual::format_detected(label, deflate_streams);
        }
        ProgressMode::Verbose => {
            NEXT_STREAM_ID.store(1, Ordering::Relaxed);
            verbose::reset();
            let _ = write_format_summary(&mut io::stdout().lock(), label, deflate_streams);
        }
        ProgressMode::Disabled => {}
    }
    let deadline = with_report_coordinator(|coordinator| {
        coordinator.reset(mode, deflate_streams.unwrap_or(0), options.timeout);
        coordinator.deadline
    });
    if let Some(deadline) = deadline {
        start_report_spinner(deadline);
    }
}

/// Print any remaining stream reports once container work has finished.
/// Force-sealing keeps the reports of a container that returned early on an
/// error.
pub(crate) fn finish_file(options: &Options) {
    let mode = ProgressMode::for_options(options);
    if !mode.enabled() {
        return;
    }
    let store_changes = with_report_coordinator(|coordinator| {
        coordinator.finish();
        std::mem::take(&mut coordinator.zip_store_changes)
    });
    stop_report_spinner();
    emit_ready_streams();
    match mode {
        ProgressMode::Visual => {
            visual::finish_file();
            let _ = write_zip_store_changes(&mut io::stdout().lock(), &store_changes);
        }
        ProgressMode::Verbose => {
            verbose::flush();
            let _ = write_zip_store_changes(&mut io::stdout().lock(), &store_changes);
        }
        ProgressMode::Disabled => {}
    }
}

fn write_zip_store_changes(output: &mut dyn Write, changes: &[ZipStoreProgress]) -> io::Result<()> {
    if changes.is_empty() {
        return Ok(());
    }
    writeln!(output)?;
    writeln!(
        output,
        "ZIP method changes  {} {}",
        changes.len(),
        plural(changes.len(), "entry", "entries")
    )?;
    for change in changes {
        let saved = change.deflate_bytes - change.stored_bytes;
        let tenths = (u64::from(saved) * 1_000 + u64::from(change.deflate_bytes) / 2)
            / u64::from(change.deflate_bytes);
        writeln!(
            output,
            "  S{} Final wrapper · Deflate {} bytes → Store {} {} · saved {} {} ({}.{:01}%)",
            change.stream_id,
            change.deflate_bytes,
            change.stored_bytes,
            plural(change.stored_bytes as usize, "byte", "bytes"),
            saved,
            plural(saved as usize, "byte", "bytes"),
            tenths / 10,
            tenths % 10,
        )?;
    }
    output.flush()
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

/// Exclusive use of the static, per-file report state (stream IDs,
/// coordinator, caches, renderers), so reporting calls run one at a time.
/// Quiet calls never touch it.
pub(crate) struct ReportSession {
    _guard: MutexGuard<'static, ()>,
}

/// Begin a report session, waiting for any other reporting call to finish;
/// `None` without waiting when reporting is disabled.
pub(crate) fn begin_report_session(options: &Options) -> Option<ReportSession> {
    reports_enabled(options).then(|| ReportSession {
        // A panic in another reporting call leaves only presentation state,
        // which the next session resets; it is safe to continue.
        _guard: REPORT_SESSION
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()),
    })
}

/// Whether a progress renderer will consume block-plan snapshots.
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
    // The classic Windows console's default fonts, Consolas and Lucida Console,
    // have no check mark (U+2713) and draw an empty box; the square root sign
    // stands in, as it did in code page 437.
    match (cfg!(windows), color) {
        (false, true) => "\x1b[32m✓\x1b[0m",
        (false, false) => "✓",
        (true, true) => "\x1b[32m√\x1b[0m",
        (true, false) => "√",
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
mod tests;
