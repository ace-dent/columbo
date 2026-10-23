// SPDX-License-Identifier: MIT
// SPDX-FileCopyrightText: 2026 Andrew C.E. Dent <https://github.com/ace-dent>

//! Prints buffered stream reports in physical stream order.
//!
//! A stream is sealed once each of its producers (the container's optimizer
//! call, or every archive lineage in ZIP Max) has reported it complete. Sealed
//! streams print as soon as every earlier stream has printed, and the report
//! spinner pauses while they do.

use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use crate::terminal::spinner::Spinner;
use crate::Options;

use super::buffer;

/// The producer of every stream outside ZIP Max's archive lineages.
pub(crate) const PRIMARY_STREAM_PRODUCER: u8 = 0;

static REPORT_COORDINATOR: OnceLock<Mutex<ReportCoordinator>> = OnceLock::new();
static REPORT_EMITTER: OnceLock<Mutex<()>> = OnceLock::new();
static REPORT_SPINNER: OnceLock<Mutex<Option<Spinner>>> = OnceLock::new();

/// A ZIP entry rewritten from Deflate to Store after all Deflate work.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ZipStoreProgress {
    pub(crate) stream_id: usize,
    pub(crate) deflate_bytes: u32,
    pub(crate) stored_bytes: u32,
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

pub(super) struct ReportCoordinator {
    /// Whether the file's reports are still being produced, so the spinner
    /// resumes after each print.
    active: bool,
    deadline: Option<Instant>,
    expected_producers: ProducerSet,
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
            next_stream_id: 1,
            streams: Vec::new(),
            zip_store_changes: Vec::new(),
            total_streams: 0,
        }
    }
}

impl ReportCoordinator {
    pub(super) fn reset(&mut self, total_streams: usize, timeout: Duration) {
        self.active = true;
        self.deadline = Instant::now().checked_add(timeout);
        self.expected_producers = ProducerSet::default();
        self.expected_producers.insert(PRIMARY_STREAM_PRODUCER);
        self.next_stream_id = 1;
        self.streams.clear();
        self.zip_store_changes.clear();
        self.total_streams = total_streams;
    }

    pub(super) fn set_expected_producers(&mut self, producers: &[u8]) {
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

    pub(super) fn complete(&mut self, stream_id: usize, duplicates: &[usize], producer: u8) {
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

    /// The next stream to print, once it is sealed.
    pub(super) fn next_sealed(&self) -> Option<usize> {
        (self.next_stream_id <= self.total_streams
            && self
                .streams
                .get(self.next_stream_id)
                .is_some_and(|stream| stream.sealed))
        .then_some(self.next_stream_id)
    }

    pub(super) fn advance(&mut self, stream_id: usize) {
        debug_assert_eq!(self.next_stream_id, stream_id);
        if let Some(stream) = self.streams.get_mut(stream_id) {
            *stream = StreamState::default();
        }
        self.next_stream_id = self.next_stream_id.saturating_add(1);
    }

    /// Seal every remaining stream, so a container that returned early on an
    /// error still prints what it reported.
    fn finish(&mut self) {
        self.active = false;
        for stream_id in self.next_stream_id..=self.total_streams {
            self.stream_mut(stream_id).sealed = true;
        }
    }
}

fn with_report_coordinator<T>(operation: impl FnOnce(&mut ReportCoordinator) -> T) -> T {
    let coordinator = REPORT_COORDINATOR.get_or_init(|| Mutex::new(ReportCoordinator::default()));
    let mut coordinator = coordinator
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    operation(&mut coordinator)
}

/// Start ordering a file's `total_streams` reports and show the spinner.
pub(super) fn begin_file(total_streams: usize, timeout: Duration) {
    let deadline = with_report_coordinator(|coordinator| {
        coordinator.reset(total_streams, timeout);
        coordinator.deadline
    });
    if let Some(deadline) = deadline {
        start_report_spinner(deadline);
    }
}

/// Print every stream left and return the selected ZIP lineage's Store
/// conversions.
pub(super) fn finish_file() -> Vec<ZipStoreProgress> {
    let store_changes = with_report_coordinator(|coordinator| {
        coordinator.finish();
        std::mem::take(&mut coordinator.zip_store_changes)
    });
    stop_report_spinner();
    emit_ready_streams();
    store_changes
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
pub(crate) fn record_zip_store_changes(options: &Options, changes: &[ZipStoreProgress]) {
    if changes.is_empty() || !super::reports_enabled(options) {
        return;
    }
    with_report_coordinator(|coordinator| {
        coordinator.zip_store_changes.clear();
        coordinator.zip_store_changes.extend_from_slice(changes);
    });
}

/// Print every sealed stream that is next in order.
pub(super) fn emit_ready_streams() {
    let emitter = REPORT_EMITTER.get_or_init(|| Mutex::new(()));
    let _emitter = emitter
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let mut ready = Vec::new();
    while let Some(stream_id) = with_report_coordinator(|coordinator| coordinator.next_sealed()) {
        let Some(reports) = buffer::take_finished_stream(stream_id) else {
            break;
        };
        ready.extend(reports);
        with_report_coordinator(|coordinator| coordinator.advance(stream_id));
    }

    if ready.is_empty() {
        return;
    }
    stop_report_spinner();
    buffer::write(ready);
    let restart = with_report_coordinator(|coordinator| {
        coordinator.active.then_some(coordinator.deadline).flatten()
    });
    if let Some(deadline) = restart {
        start_report_spinner(deadline);
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

#[cfg(test)]
mod tests;
