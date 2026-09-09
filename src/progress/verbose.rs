// SPDX-License-Identifier: MIT

//! Buffered verbose reports emitted in physical stream order.

use std::collections::BTreeMap;
use std::fmt::{self, Write as _};
use std::io::{self, Write};
use std::sync::{Mutex, OnceLock};

const MAX_CACHED_BYTES: usize = 64 * 1_024 * 1_024;

static REPORTS: OnceLock<Mutex<Reports>> = OnceLock::new();

#[derive(Default)]
struct Reports {
    cached_bytes: usize,
    reports: BTreeMap<usize, Report>,
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
        if self.cached_bytes.saturating_add(line.len()) > MAX_CACHED_BYTES {
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
            .filter_map(|(&report_id, report)| (report.stream_id == stream_id).then_some(report_id))
            .collect();
        super::sort_tiny_by(&mut report_ids, |left_id, right_id| {
            let left = &self.reports[left_id];
            let right = &self.reports[right_id];
            (left.order, left_id).cmp(&(right.order, right_id))
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

    fn take_finished_in_stream_order(&mut self) -> Vec<Report> {
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

pub(super) fn begin(report_id: usize, stream_id: usize, order: u8, text: String) {
    with_reports(|reports| reports.insert(report_id, stream_id, order, text));
}

pub(super) fn append(report_id: usize, arguments: fmt::Arguments<'_>) {
    with_reports(|reports| reports.append(report_id, arguments));
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

pub(super) fn flush() {
    write(with_reports(Reports::take_finished_in_stream_order));
}

#[cfg(test)]
mod tests;
