// SPDX-License-Identifier: MIT
// SPDX-FileCopyrightText: 2026 Andrew C.E. Dent <https://github.com/ace-dent>

//! The report of one optimizer call on one stream: its source, the routes it
//! tries, its final block plan and the selected result.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use crate::terminal;
use crate::terminal::formatting::{format_duration, plural, plural_u64, style, CYAN};
use crate::Options;

use super::buffer;
use super::context::{current_stream_group, report_order, StreamNote};
use super::coordinator::emit_ready_streams;
use super::route::{RouteProgress, RouteStep};
use super::{
    describe_size, BalancedTreeProgress, BlockReport, CandidateProgress, ReportLines,
    SameDistanceProgress,
};

static NEXT_STREAM_ID: AtomicUsize = AtomicUsize::new(1);
/// Report IDs are separate from displayed stream numbers, so concurrent
/// lineages of one stream never overwrite each other.
static NEXT_REPORT_ID: AtomicUsize = AtomicUsize::new(1);

/// Number a new file's streams from 1, for calls outside a stream context.
pub(super) fn restart_stream_numbering() {
    NEXT_STREAM_ID.store(1, Ordering::Relaxed);
}

/// The parsed source stream, as reported before any route runs.
pub(crate) struct StreamProgress {
    pub(crate) blocks: usize,
    pub(crate) compressed_bytes: usize,
    pub(crate) decoded_bytes: u64,
    pub(crate) empty_blocks: usize,
    pub(crate) meaningful_bits: u64,
    pub(crate) parse_elapsed: Duration,
}

/// The reporter of one optimizer call; every method is a no-op when
/// reporting is disabled.
#[derive(Clone, Copy)]
pub(crate) struct Progress {
    lines: ReportLines,
    optimizer_started: Instant,
    /// The stream is searched within a slice of the file's time budget.
    slice_budget: bool,
}

impl Progress {
    /// Open the stream's report with its source summary.
    pub(crate) fn begin(
        options: &Options,
        optimizer_started: Instant,
        stream: StreamProgress,
    ) -> Self {
        if !super::reports_enabled(options) {
            return Self {
                lines: ReportLines::DISABLED,
                optimizer_started,
                slice_budget: false,
            };
        }

        let group = current_stream_group();
        let stream_id = group.as_ref().map_or_else(
            || NEXT_STREAM_ID.fetch_add(1, Ordering::Relaxed),
            |group| group.id,
        );
        let note = group.as_ref().and_then(|group| group.note);
        let duplicates = group
            .as_ref()
            .map_or(&[][..], |group| group.duplicates.as_slice());
        let report_id = NEXT_REPORT_ID.fetch_add(1, Ordering::Relaxed);
        let color = terminal::stdout_color_enabled();
        let title = stream_title(stream_id, note, duplicates, color);
        buffer::begin(report_id, stream_id, report_order(note), title);
        let lines = ReportLines {
            report_id: Some(report_id),
            color,
        };

        let empty_blocks = if stream.empty_blocks == 0 {
            String::new()
        } else {
            format!(
                " · {} empty {}",
                stream.empty_blocks,
                plural(stream.empty_blocks, "block", "blocks")
            )
        };
        lines.line(format_args!(
            " Source · {} {} · {} meaningful {} · {} {}{empty_blocks}",
            stream.compressed_bytes,
            plural(stream.compressed_bytes, "byte", "bytes"),
            stream.meaningful_bits,
            plural_u64(stream.meaningful_bits, "bit", "bits"),
            stream.blocks,
            plural(stream.blocks, "block", "blocks"),
        ));
        lines.line(format_args!(
            " Parsed · {} · {} decoded {}",
            format_duration(stream.parse_elapsed),
            stream.decoded_bytes,
            plural_u64(stream.decoded_bytes, "byte", "bytes"),
        ));

        Self {
            lines,
            optimizer_started,
            slice_budget: group.is_some_and(|group| group.slice_budget),
        }
    }

    pub(crate) fn enabled(self) -> bool {
        self.lines.enabled()
    }

    pub(crate) fn normalization(self, blocks: usize, elapsed: Duration) {
        if !self.enabled() || blocks == 0 {
            return;
        }
        self.lines.line(format_args!(
            " {} Strict normalization · {} · canonicalized {blocks} {}",
            self.lines.success(),
            format_duration(elapsed),
            plural(blocks, "block", "blocks")
        ));
    }

    pub(crate) fn same_distance_opportunities(self, report: SameDistanceProgress) {
        if !self.enabled() {
            return;
        }
        if report.runs == 0 {
            self.lines.line(format_args!(
                " Matches · no adjacent same-distance runs in the joined source token stream"
            ));
            return;
        }
        self.lines.line(format_args!(
            " Matches · joined source token stream · {} {} / {} {} / {} decoded {} · {} direct {} · {} {} · up to {} removable {}",
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
        if !self.enabled() {
            return;
        }
        self.lines.line(format_args!(
            " Trees · {} dynamic {} · literal/length: {} pair + {} quad · distance: {} pair + {} quad · up to {} paired {}",
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

    /// Begin the list of routes.
    pub(crate) fn routes(self) {
        if self.enabled() {
            self.lines.blank_line();
            self.lines.line(format_args!(" Routes"));
        }
    }

    pub(crate) fn start(self, name: &'static str) -> RouteStep {
        RouteStep::start(self.lines, name)
    }

    /// Start a long route that reports its phases and periodic heartbeats.
    pub(crate) fn start_detailed(
        self,
        name: &'static str,
        reference_bits: u64,
        budget: Duration,
    ) -> (RouteStep, RouteProgress) {
        let step = self.start(name);
        (
            step,
            RouteProgress::start(self.lines, reference_bits, budget),
        )
    }

    /// Report a candidate that a route produced besides its result.
    pub(crate) fn candidate(self, name: &'static str, candidate: CandidateProgress) {
        if !self.enabled() {
            return;
        }
        self.lines.line(format_args!(
            "   {} {name} · {}",
            self.lines.candidate_marker(&candidate),
            candidate.summary()
        ));
    }

    pub(crate) fn skipped(self, name: &'static str, reason: &'static str) {
        if self.enabled() {
            self.lines.line(format_args!(
                "   {} {name} skipped · {reason}",
                self.lines.neutral()
            ));
        }
    }

    /// Report the selected encoding's blocks, up to
    /// [`MAX_REPORTED_BLOCKS`](super::MAX_REPORTED_BLOCKS) of them.
    pub(crate) fn blocks(self, report: Option<BlockReport>) {
        if !self.enabled() {
            return;
        }
        self.lines.blank_line();
        self.lines.line(format_args!(" Final block plan"));
        let Some(report) = report else {
            self.lines.line(format_args!("   · Details unavailable"));
            return;
        };

        for (index, block) in report.blocks.iter().enumerate() {
            self.lines.line(format_args!(
                "   #{:<3} {:>8} → {:<8} · {} decoded {} · {} {} · {} {} · starts at bit {}{}",
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
            self.lines.line(format_args!(
                "   … {omitted} additional {} omitted",
                plural(omitted, "block", "blocks"),
            ));
        }
    }

    /// Close the report with the selected result.
    pub(crate) fn finish(
        self,
        selected_route: &'static str,
        output_bytes: usize,
        output_bits: u64,
        source_bits: u64,
        timed_out: bool,
    ) {
        if !self.enabled() {
            return;
        }
        let stop = match (timed_out, self.slice_budget) {
            (false, _) => "",
            (true, true) => " · search slice reached",
            (true, false) => " · deadline reached",
        };
        self.lines.line(format_args!(
            " {} Selected {selected_route} · {} · {}{stop}",
            self.lines.success(),
            describe_size(output_bytes, output_bits, source_bits),
            format_duration(self.optimizer_started.elapsed()),
        ));
        self.lines.finish();
        // The scheduler may have sealed this stream before this final update
        // reached the cache, so recheck the next stream to print.
        emit_ready_streams();
    }
}

/// The report's opening lines: a blank line and the stream's title.
fn stream_title(
    stream_id: usize,
    note: Option<StreamNote>,
    duplicates: &[usize],
    color: bool,
) -> String {
    let (cyan, reset) = style(color, CYAN);
    let note = note.map_or_else(String::new, |note| format!(" · {}", note.label()));
    let duplicates = if duplicates.is_empty() {
        String::new()
    } else {
        let identifiers = duplicates
            .iter()
            .map(|stream| format!("{stream:02}"))
            .collect::<Vec<_>>()
            .join(", ");
        format!(" · duplicates {identifiers}")
    };
    format!("\n{cyan}Deflate stream {stream_id}{note}{duplicates}{reset}\n")
}
