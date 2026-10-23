// SPDX-License-Identifier: MIT
// SPDX-FileCopyrightText: 2026 Andrew C.E. Dent <https://github.com/ace-dent>

//! Verbose optimization reports.
//!
//! Each optimizer call on a stream buffers one report instead of writing to
//! standard output from its worker thread. A file-level coordinator prints
//! each physical stream, in file order, as soon as all its producers have
//! finished and every earlier stream is printed. Quiet calls get disabled
//! reporters and never touch the shared report state.
//!
//! - `context`: the stream, and lineage of it, that a thread is optimizing.
//! - `stream` and `route`: the reporters that the optimizer calls.
//! - `buffer`: the bounded cache of reports not yet printed.
//! - `coordinator`: physical stream order, printing and the report spinner.

use std::fmt;
use std::io::{self, Write};
use std::sync::{Mutex, MutexGuard};

use crate::terminal::formatting::{plural, plural_u64};
use crate::{Format, Options};

mod buffer;
mod context;
mod coordinator;
mod route;
mod stream;

pub(crate) use context::{
    current_route_lineage, with_optional_route_lineage, with_route_lineage, with_stream_group,
    with_stream_reclaim, with_stream_slice, StreamNote,
};
pub(crate) use coordinator::{
    complete_all_stream_producers, complete_stream_group, complete_stream_producer,
    record_zip_store_changes, set_stream_producers, ZipStoreProgress, PRIMARY_STREAM_PRODUCER,
};
pub(crate) use route::RouteProgress;
pub(crate) use stream::{Progress, StreamProgress};

/// The most blocks a block report lists; it counts the rest.
pub(crate) const MAX_REPORTED_BLOCKS: usize = 64;

static REPORT_SESSION: Mutex<()> = Mutex::new(());

/// Whether a call reports. The optimizer also takes block-plan snapshots only
/// when it does.
pub(crate) fn reports_enabled(options: &Options) -> bool {
    options.verbose
}

/// Exclusive use of the static, per-file report state (stream IDs,
/// coordinator, report cache), so reporting calls run one at a time.
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

/// Begin a file's reports: print its format and Deflate stream count, then
/// order its streams from the first.
pub(crate) fn format_detected(options: &Options, format: Format, deflate_streams: Option<usize>) {
    if !reports_enabled(options) {
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
    stream::restart_stream_numbering();
    buffer::reset();
    let _ = write_format_summary(&mut io::stdout().lock(), label, deflate_streams);
    coordinator::begin_file(deflate_streams.unwrap_or(0), options.timeout);
}

/// Print any remaining stream reports once container work has finished.
/// Force-sealing keeps the reports of a container that returned early on an
/// error.
pub(crate) fn finish_file(options: &Options) {
    if !reports_enabled(options) {
        return;
    }
    let store_changes = coordinator::finish_file();
    buffer::flush();
    let _ = write_zip_store_changes(&mut io::stdout().lock(), &store_changes);
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

/// The first [`MAX_REPORTED_BLOCKS`] blocks of an encoding, and its block
/// count.
#[derive(Debug, Clone)]
pub(crate) struct BlockReport {
    pub(crate) blocks: Vec<BlockProgress>,
    pub(crate) total_blocks: usize,
}

#[derive(Debug, Clone)]
pub(crate) struct CandidateProgress {
    pub(crate) bytes: usize,
    pub(crate) bits: u64,
    pub(crate) reference_bits: u64,
    /// Whether this complete candidate is strictly smaller than its source.
    pub(crate) profitable: bool,
}

impl CandidateProgress {
    fn summary(&self) -> String {
        describe_size(self.bytes, self.bits, self.reference_bits)
    }
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

/// One open stream report, or none when reporting is disabled, which makes
/// every write a no-op.
#[derive(Clone, Copy)]
struct ReportLines {
    report_id: Option<usize>,
    color: bool,
}

impl ReportLines {
    const DISABLED: Self = Self {
        report_id: None,
        color: false,
    };

    fn enabled(self) -> bool {
        self.report_id.is_some()
    }

    /// Append a line after the stream tag (`  S3`). Leading spaces nest it:
    /// one for stream-level lines and route results, three for a route's
    /// other candidates, five for a phase of a long route and seven for that
    /// phase's progress.
    fn line(self, arguments: fmt::Arguments<'_>) {
        if let Some(report_id) = self.report_id {
            buffer::line(report_id, arguments);
        }
    }

    fn blank_line(self) {
        if let Some(report_id) = self.report_id {
            buffer::blank_line(report_id);
        }
    }

    /// Mark the report complete, so the coordinator can print it.
    fn finish(self) {
        if let Some(report_id) = self.report_id {
            buffer::finish(report_id);
        }
    }

    fn arrow(self) -> &'static str {
        if self.color {
            "\x1b[36m→\x1b[0m"
        } else {
            "→"
        }
    }

    fn success(self) -> &'static str {
        // The classic Windows console's default fonts, Consolas and Lucida
        // Console, have no check mark (U+2713) and draw an empty box; the
        // square root sign stands in, as it did in code page 437.
        match (cfg!(windows), self.color) {
            (false, true) => "\x1b[32m✓\x1b[0m",
            (false, false) => "✓",
            (true, true) => "\x1b[32m√\x1b[0m",
            (true, false) => "√",
        }
    }

    fn neutral(self) -> &'static str {
        if self.color {
            "\x1b[2m·\x1b[0m"
        } else {
            "·"
        }
    }

    fn warning(self) -> &'static str {
        if self.color {
            "\x1b[33m!\x1b[0m"
        } else {
            "!"
        }
    }

    fn candidate_marker(self, candidate: &CandidateProgress) -> &'static str {
        if candidate.profitable {
            self.success()
        } else {
            self.neutral()
        }
    }
}

/// For example `546 bytes / 4368 bits · saved 158 bits vs source`.
fn describe_size(bytes: usize, bits: u64, reference_bits: u64) -> String {
    format!(
        "{bytes} {} / {bits} {} · {}",
        plural(bytes, "byte", "bytes"),
        plural_u64(bits, "bit", "bits"),
        describe_bit_change(reference_bits, bits)
    )
}

fn describe_bit_change(reference_bits: u64, candidate_bits: u64) -> String {
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

#[cfg(test)]
mod tests;
