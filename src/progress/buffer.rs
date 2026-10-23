// SPDX-License-Identifier: MIT
// SPDX-FileCopyrightText: 2026 Andrew C.E. Dent <https://github.com/ace-dent>

//! Buffered stream reports, printed in physical stream order.
//!
//! Each optimizer call buffers its report here under a report ID, and the
//! coordinator takes a stream's reports once all of them are finished. The
//! cache is bounded: past 64 MiB, a report stops growing and ends with a note.

use std::fmt::{self, Write as _};
use std::io::{self, Write};
use std::sync::{Mutex, OnceLock};

use crate::sort::SortShared;

const MAX_CACHED_BYTES: usize = 64 * 1_024 * 1_024;

static REPORTS: OnceLock<Mutex<Reports>> = OnceLock::new();

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
    // Out of line: each call searches the map, from many report paths.
    #[inline(never)]
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

/// Insertion sort for the few reports of one stream; keeps the general slice
/// sort out of cold code.
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

#[derive(Default)]
struct Reports {
    cached_bytes: usize,
    reports: IdMap<Report>,
}

pub(super) struct Report {
    finished: bool,
    order: u8,
    stream_id: usize,
    text: String,
    truncated: bool,
}

impl Reports {
    fn reset(&mut self) {
        self.cached_bytes = 0;
        self.reports.clear();
    }

    fn insert(&mut self, report_id: usize, stream_id: usize, order: u8, mut text: String) {
        let truncated = self.cached_bytes.saturating_add(text.len()) > MAX_CACHED_BYTES;
        if truncated {
            text.clear();
        } else {
            self.cached_bytes += text.len();
        }
        self.reports.insert(
            report_id,
            Report {
                finished: false,
                order,
                stream_id,
                text,
                truncated,
            },
        );
    }

    /// Append one line, tagged with the stream (`  S3`) unless it is blank.
    fn append(&mut self, report_id: usize, arguments: Option<fmt::Arguments<'_>>) {
        let Some(report) = self.reports.get_mut(report_id) else {
            return;
        };
        if report.truncated {
            return;
        }
        let mut line = String::new();
        if let Some(arguments) = arguments {
            let _ = write!(line, "  S{}", report.stream_id);
            let _ = line.write_fmt(arguments);
        }
        line.push('\n');
        if self.cached_bytes.saturating_add(line.len()) > MAX_CACHED_BYTES {
            report.truncated = true;
            return;
        }
        self.cached_bytes += line.len();
        report.text.push_str(&line);
    }

    fn finish(&mut self, report_id: usize) {
        if let Some(report) = self.reports.get_mut(report_id) {
            report.finished = true;
        }
    }

    /// Remove every report of a stream once all of them are finished, in
    /// report order; `None` while any is still open.
    fn take_finished_stream(&mut self, stream_id: usize) -> Option<Vec<Report>> {
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
            .filter_map(|(report_id, report)| (report.stream_id == stream_id).then_some(report_id))
            .collect();
        sort_tiny_by(&mut report_ids, |left_id, right_id| {
            let left = &self.reports[*left_id];
            let right = &self.reports[*right_id];
            (left.order, left_id).cmp(&(right.order, right_id))
        });
        let mut finished = Vec::with_capacity(report_ids.len());
        for report_id in report_ids {
            let report = self
                .reports
                .remove(report_id)
                .expect("a collected report remains cached");
            self.cached_bytes = self.cached_bytes.saturating_sub(report.text.len());
            finished.push(report);
        }
        Some(finished)
    }

    /// Remove every finished report, in stream and report order; unfinished
    /// reports are dropped.
    fn take_finished_in_stream_order(&mut self) -> Vec<Report> {
        let mut reports: Vec<_> = self
            .reports
            .take()
            .into_iter()
            .filter_map(|(report_id, report)| report.finished.then_some((report_id, report)))
            .collect();
        reports
            .sort_shared_by_key(|(report_id, report)| (report.stream_id, report.order, *report_id));
        self.cached_bytes = 0;
        reports.into_iter().map(|(_, report)| report).collect()
    }
}

fn with_reports<T>(operation: impl FnOnce(&mut Reports) -> T) -> T {
    let reports = REPORTS.get_or_init(|| Mutex::new(Reports::default()));
    let mut reports = reports
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    operation(&mut reports)
}

pub(super) fn reset() {
    with_reports(Reports::reset);
}

/// Open a report whose text starts with `header`.
pub(super) fn begin(report_id: usize, stream_id: usize, order: u8, header: String) {
    with_reports(|reports| reports.insert(report_id, stream_id, order, header));
}

pub(super) fn line(report_id: usize, arguments: fmt::Arguments<'_>) {
    with_reports(|reports| reports.append(report_id, Some(arguments)));
}

pub(super) fn blank_line(report_id: usize) {
    with_reports(|reports| reports.append(report_id, None));
}

pub(super) fn finish(report_id: usize) {
    with_reports(|reports| reports.finish(report_id));
}

pub(super) fn take_finished_stream(stream_id: usize) -> Option<Vec<Report>> {
    with_reports(|reports| reports.take_finished_stream(stream_id))
}

pub(super) fn write(reports: Vec<Report>) {
    if reports.is_empty() {
        return;
    }
    let stdout = io::stdout();
    let mut output = stdout.lock();
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

/// Print every finished report that is still buffered.
pub(super) fn flush() {
    write(with_reports(Reports::take_finished_in_stream_order));
}

#[cfg(test)]
mod tests;
