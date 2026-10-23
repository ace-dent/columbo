// SPDX-License-Identifier: MIT
// SPDX-FileCopyrightText: 2026 Andrew C.E. Dent <https://github.com/ace-dent>

//! The physical stream, and the lineage of it, that the current thread is
//! optimizing.
//!
//! Containers set this context around each optimizer call, and the stream
//! reporter reads it when the call begins, so stream identities never pass
//! through the optimizer.

use std::cell::{Cell, RefCell};

/// Labels the report of one lineage or pass of a stream, and orders it among
/// the stream's other reports.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum StreamNote {
    /// A quick, time-capped pass over a small compressed PNG metadata chunk.
    MetadataProbe,
    /// ZIP Max's Default archive lineage.
    DefaultFloor,
    /// ZIP Max's direct-Max archive lineage.
    DirectMax,
    /// ZIP Max's refinement of its Default lineage.
    RefinedDefault,
    /// A timed-out stream searched again with time that others left unused.
    ReclaimedTime,
}

impl StreamNote {
    pub(super) fn label(self) -> &'static str {
        match self {
            Self::MetadataProbe => "metadata probe",
            Self::DefaultFloor => "default floor",
            Self::DirectMax => "direct max",
            Self::RefinedDefault => "refined default",
            Self::ReclaimedTime => "reclaimed time",
        }
    }
}

/// The position of a report among one stream's reports: the probe, then ZIP
/// Max's lineages in the order they run around the unlabeled primary report,
/// then any retry.
pub(super) fn report_order(note: Option<StreamNote>) -> u8 {
    match note {
        Some(StreamNote::MetadataProbe) => 0,
        Some(StreamNote::DefaultFloor) => 1,
        Some(StreamNote::DirectMax) => 2,
        None => 3,
        Some(StreamNote::RefinedDefault) => 4,
        Some(StreamNote::ReclaimedTime) => 5,
    }
}

thread_local! {
    static STREAM_GROUP: RefCell<Option<StreamGroup>> = const { RefCell::new(None) };
    static ROUTE_LINEAGE: Cell<Option<StreamNote>> = const { Cell::new(None) };
}

#[derive(Clone)]
pub(super) struct StreamGroup {
    pub(super) id: usize,
    /// Exact APNG duplicates that share this stream's work.
    pub(super) duplicates: Vec<usize>,
    pub(super) note: Option<StreamNote>,
    /// Searched within a proportional slice of the file's time budget.
    pub(super) slice_budget: bool,
}

struct StreamGroupGuard(Option<StreamGroup>);

impl Drop for StreamGroupGuard {
    // Out of line: report paths drop many of these guards.
    #[inline(never)]
    fn drop(&mut self) {
        STREAM_GROUP.with(|group| {
            group.replace(self.0.take());
        });
    }
}

struct RouteLineageGuard(Option<StreamNote>);

impl Drop for RouteLineageGuard {
    fn drop(&mut self) {
        ROUTE_LINEAGE.with(|lineage| lineage.set(self.0.take()));
    }
}

/// Label every stream optimized by one concurrent container branch, so reports
/// can tell ZIP Max's Default and direct-Max archives apart.
pub(crate) fn with_route_lineage<T>(note: StreamNote, operation: impl FnOnce() -> T) -> T {
    let previous = ROUTE_LINEAGE.with(|lineage| lineage.replace(Some(note)));
    let _guard = RouteLineageGuard(previous);
    operation()
}

/// The current thread's lineage, for handing to worker threads.
pub(crate) fn current_route_lineage() -> Option<StreamNote> {
    ROUTE_LINEAGE.with(Cell::get)
}

pub(crate) fn with_optional_route_lineage<T>(
    note: Option<StreamNote>,
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
    note: Option<StreamNote>,
    operation: impl FnOnce() -> T,
) -> T {
    with_stream_context(id, duplicates, note, true, operation)
}

/// Like [`with_stream_group`], for a timed-out stream searched again.
pub(crate) fn with_stream_reclaim<T>(
    id: usize,
    duplicates: &[usize],
    slice_budget: bool,
    operation: impl FnOnce() -> T,
) -> T {
    with_stream_context(
        id,
        duplicates,
        Some(StreamNote::ReclaimedTime),
        slice_budget,
        operation,
    )
}

fn with_stream_context<T>(
    id: usize,
    duplicates: &[usize],
    note: Option<StreamNote>,
    slice_budget: bool,
    operation: impl FnOnce() -> T,
) -> T {
    let note = note.or_else(current_route_lineage);
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

pub(super) fn current_stream_group() -> Option<StreamGroup> {
    STREAM_GROUP.with(|group| group.borrow().clone())
}
