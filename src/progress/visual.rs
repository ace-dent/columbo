// SPDX-License-Identifier: MIT

//! Ordered Deflate stream map.
//!
//! Optimizer workers update cached stream views and never write terminal
//! control sequences. The reporting coordinator emits immutable cards as
//! complete physical streams become available in order. This preserves
//! readable scrollback without weakening multi-stream parallelism.

use std::collections::BTreeMap;
use std::env;
#[cfg(unix)]
use std::fs::File;
use std::io::{self, IsTerminal, Write};
#[cfg(unix)]
use std::process::{Command, Stdio};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use crate::{terminal, Options};

use super::{write_format_summary, BlockEncoding, BlockReport, CandidateProgress, StreamProgress};

const DEFAULT_COLUMNS: usize = 80;
const MAX_TERMINAL_COLUMNS: usize = 4_096;
const CARD_WIDTH_NUMERATOR: usize = 9;
const CARD_WIDTH_DENOMINATOR: usize = 10;
const MIN_CARD_COLUMNS: usize = 24;
const CHANGE_HIGHLIGHT_DRAWS: u8 = 1;
const STORED_COLOR: &str = "\x1b[33m";
const FIXED_COLOR: &str = "\x1b[36m";
const DYNAMIC_COLOR: &str = "\x1b[96m";

static RENDERER: OnceLock<Mutex<Renderer>> = OnceLock::new();

pub(super) fn enabled(options: &Options) -> bool {
    options.visual
        && io::stderr().is_terminal()
        && env::var_os("TERM").map_or(true, |term| term != "dumb")
}

pub(super) fn format_detected(format: &'static str, deflate_streams: Option<usize>) {
    with_renderer(|renderer| {
        renderer.reset();
        renderer.format = Some(format);
        renderer.deflate_streams = deflate_streams;
        renderer.print_header();
    });
}

pub(super) fn begin(
    report_id: usize,
    stream_id: usize,
    duplicates: &[usize],
    note: Option<&'static str>,
    stream: &StreamProgress,
    source_report: Option<BlockReport>,
) {
    with_renderer(|renderer| {
        renderer.print_header();
        renderer.views.insert(
            report_id,
            StreamView {
                id: stream_id,
                duplicates: duplicates.to_vec(),
                note,
                source_bits: stream.meaningful_bits,
                source_blocks: stream.blocks,
                source_bytes: stream.compressed_bytes,
                source_report: source_report.clone(),
                output_bits: stream.meaningful_bits,
                output_bytes: stream.compressed_bytes,
                output_report: source_report,
                change_highlight_draws: 0,
                finished: false,
                pulse: 0,
                status: format!(
                    "Parsed {} decoded · {}",
                    format_bytes_u64(stream.decoded_bytes),
                    super::format_duration(stream.parse_elapsed)
                ),
                work: None,
            },
        );
    });
}

pub(super) fn route_started(report_id: usize, name: &str) {
    with_renderer(|renderer| {
        let Some(view) = renderer.views.get_mut(&report_id) else {
            return;
        };
        view.status.clear();
        view.status.push_str(name);
        view.work = Some(WorkPosition {
            completed: 0,
            total: 0,
        });
    });
}

pub(super) fn activity(report_id: usize, status: &str) {
    with_renderer(|renderer| {
        let Some(view) = renderer.views.get_mut(&report_id) else {
            return;
        };
        view.status.clear();
        view.status.push_str(status);
        view.work = None;
    });
}

pub(super) fn work(report_id: usize, status: &str, completed: usize, total: usize) {
    with_renderer(|renderer| {
        let Some(view) = renderer.views.get_mut(&report_id) else {
            return;
        };
        view.status.clear();
        view.status.push_str(status);
        view.work = Some(WorkPosition { completed, total });
    });
}

pub(super) fn checkpoint(report_id: usize, name: &str, bits: u64, blocks: usize, improved: bool) {
    with_renderer(|renderer| {
        let Some(view) = renderer.views.get_mut(&report_id) else {
            return;
        };
        view.status = if improved {
            format!(
                "{name} · {blocks} blocks · {}",
                bit_change(view.source_bits, bits)
            )
        } else {
            format!("{name} checked · best retained")
        };
        view.work = None;
    });
}

pub(super) fn candidate(report_id: usize, name: &str, candidate: &CandidateProgress) {
    with_renderer(|renderer| {
        let Some(view) = renderer.views.get_mut(&report_id) else {
            return;
        };
        let improved = is_smaller(
            candidate.bytes,
            candidate.bits,
            view.output_bytes,
            view.output_bits,
        );
        if improved {
            view.output_bytes = candidate.bytes;
            view.output_bits = candidate.bits;
            if let Some(report) = &candidate.report {
                view.output_report = Some(report.clone());
            }
            let structure =
                structure_change(view.source_report.as_ref(), view.output_report.as_ref());
            if !structure.is_empty() {
                view.change_highlight_draws = CHANGE_HIGHLIGHT_DRAWS;
            }
            view.status = if structure.is_empty() {
                format!("{name} · new best")
            } else {
                format!("{name} · {structure}")
            };
        } else {
            view.status = format!("{name} checked · best retained");
        }
        view.work = None;
    });
}

pub(super) fn final_plan(report_id: usize, report: Option<&BlockReport>) {
    with_renderer(|renderer| {
        let Some(view) = renderer.views.get_mut(&report_id) else {
            return;
        };
        if let Some(report) = report {
            view.output_bits = report.total_bits;
            view.output_bytes =
                usize::try_from(report.total_bits.saturating_add(7) / 8).unwrap_or(usize::MAX);
            view.output_report = Some(report.clone());
            let structure =
                structure_change(view.source_report.as_ref(), view.output_report.as_ref());
            if !structure.is_empty() {
                view.change_highlight_draws = CHANGE_HIGHLIGHT_DRAWS;
            }
            view.status = if structure.is_empty() {
                "Final block plan".to_owned()
            } else {
                format!("Final plan · {structure}")
            };
        } else {
            view.status = "Final block plan · details unavailable".to_owned();
        }
        view.work = None;
    });
}

#[allow(clippy::too_many_arguments)]
pub(super) fn finish(
    report_id: usize,
    route: &str,
    output_bytes: usize,
    output_bits: u64,
    source_bits: u64,
    elapsed: Duration,
    timed_out: bool,
    slice_budget: bool,
) {
    with_renderer(|renderer| {
        let Some(view) = renderer.views.get_mut(&report_id) else {
            return;
        };
        view.output_bytes = output_bytes;
        view.output_bits = output_bits;
        view.change_highlight_draws = 0;
        view.finished = true;
        view.work = None;
        view.status = format!(
            "✓ {route} · {} · {}{}",
            bit_change(source_bits, output_bits),
            super::format_duration(elapsed),
            if timed_out {
                if slice_budget {
                    " · search slice"
                } else {
                    " · deadline"
                }
            } else {
                ""
            }
        );
    });
}

pub(super) fn finish_file() {
    with_renderer(Renderer::emit_finished);
}

pub(super) fn take_finished_stream(stream_id: usize) -> Option<Vec<[String; 4]>> {
    with_renderer(|renderer| renderer.take_finished_stream(stream_id))
}

pub(super) fn emit_cards(cards: Vec<[String; 4]>) {
    if cards.is_empty() {
        return;
    }
    let mut terminal = io::stderr().lock();
    for lines in cards {
        for line in lines {
            let _ = writeln!(terminal, "{line}");
        }
    }
    let _ = terminal.flush();
}

fn with_renderer<T>(operation: impl FnOnce(&mut Renderer) -> T) -> T {
    let renderer = RENDERER.get_or_init(|| Mutex::new(Renderer::default()));
    let mut renderer = renderer
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    operation(&mut renderer)
}

#[derive(Default)]
struct Renderer {
    columns: usize,
    deflate_streams: Option<usize>,
    format: Option<&'static str>,
    header_printed: bool,
    unicode: bool,
    views: BTreeMap<usize, StreamView>,
}

#[derive(Clone, Copy)]
struct Glyphs {
    boundary: char,
    changed: char,
    dynamic: char,
    fixed: char,
    original: char,
    saved: char,
    stored: char,
    unknown: char,
    working: char,
    working_alternate: char,
    unicode: bool,
}

impl Glyphs {
    fn detect() -> Self {
        Self::for_unicode(unicode_enabled())
    }

    fn for_unicode(unicode: bool) -> Self {
        if unicode {
            Self {
                boundary: '│',
                changed: '▲',
                dynamic: '▓',
                fixed: '▒',
                original: '·',
                saved: '·',
                stored: '░',
                unknown: '?',
                working: '◆',
                working_alternate: '◇',
                unicode: true,
            }
        } else {
            Self {
                boundary: '|',
                changed: '^',
                dynamic: 'D',
                fixed: 'F',
                original: '.',
                saved: '.',
                stored: 'S',
                unknown: '?',
                working: '*',
                working_alternate: '+',
                unicode: false,
            }
        }
    }
}

impl Renderer {
    fn reset(&mut self) {
        self.columns = 0;
        self.deflate_streams = None;
        self.format = None;
        self.header_printed = false;
        self.unicode = false;
        self.views.clear();
    }

    fn ordered_views(&self) -> Vec<(usize, &StreamView)> {
        let mut views: Vec<_> = self
            .views
            .iter()
            .map(|(&report_id, view)| (report_id, view))
            .collect();
        views.sort_unstable_by_key(|(report_id, view)| {
            (view.id, super::report_order(view.note), *report_id)
        });
        views
    }

    fn take_finished_stream(&mut self, stream_id: usize) -> Option<Vec<[String; 4]>> {
        if self
            .views
            .values()
            .any(|view| view.id == stream_id && !view.finished)
        {
            return None;
        }
        let mut report_ids: Vec<_> = self
            .views
            .iter()
            .filter_map(|(&report_id, view)| (view.id == stream_id).then_some(report_id))
            .collect();
        report_ids.sort_unstable_by_key(|report_id| {
            let view = &self.views[report_id];
            (super::report_order(view.note), *report_id)
        });
        let color = color_enabled();
        let glyphs = Glyphs::for_unicode(self.unicode);
        Some(
            report_ids
                .into_iter()
                .filter_map(|report_id| self.views.remove(&report_id))
                .map(|view| render_card(&view, self.columns, color, glyphs))
                .collect(),
        )
    }

    fn emit_finished(&mut self) {
        if self.views.is_empty() {
            self.reset();
            return;
        }
        // Keep the width captured with the header. Earlier immutable cards may
        // already be in scrollback, so resampling after a resize would make
        // later cards visually inconsistent with the same file.
        let glyphs = Glyphs::for_unicode(self.unicode);
        let cards: Vec<_> = self
            .ordered_views()
            .into_iter()
            .filter(|(_, view)| view.finished)
            .map(|(_, view)| render_card(view, self.columns, color_enabled(), glyphs))
            .collect();
        emit_cards(cards);
        self.reset();
    }

    fn print_header(&mut self) {
        if self.header_printed {
            return;
        }
        let color = color_enabled();
        let glyphs = Glyphs::detect();
        self.unicode = glyphs.unicode;
        let format = self.format.unwrap_or("Deflate");
        self.columns = terminal_columns();
        let (margin_width, card_width) = card_dimensions(self.columns);
        let margin = " ".repeat(margin_width);
        let mut output = io::stderr().lock();
        let _ = write_format_summary(&mut output, format, self.deflate_streams);
        let _ = writeln!(output);
        let full_legend = format!(
            "{} stored  {} fixed  {} dynamic  {} boundary  {} saved",
            glyphs.stored, glyphs.fixed, glyphs.dynamic, glyphs.boundary, glyphs.saved
        );
        let compact_legend = format!(
            "{} stored  {} fixed  {} dynamic  {} edge  {} saved",
            glyphs.stored, glyphs.fixed, glyphs.dynamic, glyphs.boundary, glyphs.saved
        );
        let (legend, boundary_label) = if visible_width(&full_legend) <= card_width {
            (&full_legend, "boundary")
        } else {
            (&compact_legend, "edge")
        };
        if visible_width(legend) <= card_width {
            let _ = writeln!(
                output,
                "{margin}{}{stored}{} stored  {}{fixed}{} fixed  {}{dynamic}{} dynamic  \
                 {boundary} {boundary_label}  {saved} saved",
                style(color, STORED_COLOR),
                reset(color),
                style(color, FIXED_COLOR),
                reset(color),
                style(color, DYNAMIC_COLOR),
                reset(color),
                stored = glyphs.stored,
                fixed = glyphs.fixed,
                dynamic = glyphs.dynamic,
                boundary = glyphs.boundary,
                saved = glyphs.saved,
            );
        } else {
            let _ = writeln!(
                output,
                "{margin}{}",
                fit_text(&compact_legend, card_width, glyphs)
            );
        }
        let _ = writeln!(output);
        let _ = output.flush();
        self.header_printed = true;
    }
}

struct StreamView {
    id: usize,
    duplicates: Vec<usize>,
    note: Option<&'static str>,
    source_bits: u64,
    source_blocks: usize,
    source_bytes: usize,
    source_report: Option<BlockReport>,
    output_bits: u64,
    output_bytes: usize,
    output_report: Option<BlockReport>,
    change_highlight_draws: u8,
    finished: bool,
    pulse: usize,
    status: String,
    work: Option<WorkPosition>,
}

#[derive(Clone, Copy)]
struct WorkPosition {
    completed: usize,
    total: usize,
}

fn render_card(
    view: &StreamView,
    terminal_width: usize,
    color: bool,
    glyphs: Glyphs,
) -> [String; 4] {
    let (margin_width, card_width) = card_dimensions(terminal_width);
    let margin = " ".repeat(margin_width);
    let heading_text = fit_text(&stream_title(view), card_width, glyphs);
    let heading = format!(
        "{margin}{}{heading_text}{}",
        style(color, "\x1b[1;36m"),
        reset(color)
    );
    let source_prefix = "  in  ";
    let output_prefix = "  out ";
    let prefix_width = visible_width(source_prefix).max(visible_width(output_prefix));
    let bar_width = bar_columns(card_width, prefix_width);

    let source_layout = layout_cells(view.source_report.as_ref(), bar_width, true);
    let source_cells = render_layout(&source_layout, None, false, None, color, glyphs);
    let source_bar = format!("[{source_cells}]");
    let source = format!("{margin}{source_prefix}{source_bar}");

    let output_layout = output_layout(view, bar_width);
    let work_column = view
        .work
        .and_then(|work| work_column(&output_layout, work, view.pulse));
    let changed = (view.change_highlight_draws != 0)
        .then(|| changed_columns(&source_layout.cells, &output_layout.cells));
    let output_cells = render_layout(
        &output_layout,
        work_column,
        view.pulse & 1 != 0,
        changed.as_deref(),
        color,
        glyphs,
    );
    let output_bar = format!("[{output_cells}]");
    let output = format!("{margin}{output_prefix}{output_bar}");

    let output_blocks = view
        .output_report
        .as_ref()
        .map_or(view.source_blocks, |report| report.total_blocks);
    let summary = format!(
        "{} → {} · {} · {} · {}",
        format_bytes(view.source_bytes),
        format_bytes(view.output_bytes),
        block_count_change(
            view.source_blocks,
            output_blocks,
            view.output_report
                .as_ref()
                .map(|_| output_layout.visible_blocks),
        ),
        compact_change(view.source_bits, view.output_bits),
        view.status
    );
    let information_width = card_width.saturating_sub(prefix_width);
    let information = format!(
        "{margin}{:prefix_width$}{}",
        "",
        fit_text(&summary, information_width, glyphs)
    );

    [heading, source, output, information]
}

fn stream_title(view: &StreamView) -> String {
    let mut title = format!("Stream {:02}", view.id);
    if let Some(note) = view.note {
        title.push_str(" · ");
        title.push_str(note);
    }
    if !view.duplicates.is_empty() {
        let duplicates = view
            .duplicates
            .iter()
            .map(|stream| format!("{stream:02}"))
            .collect::<Vec<_>>()
            .join(", ");
        title.push_str(" · duplicates ");
        title.push_str(&duplicates);
    }
    title
}

struct CellLayout {
    cells: Vec<VisualCell>,
    visible_blocks: usize,
}

fn layout_cells(report: Option<&BlockReport>, width: usize, source: bool) -> CellLayout {
    if width == 0 {
        return CellLayout {
            cells: Vec::new(),
            visible_blocks: 0,
        };
    }
    let Some(report) = report.filter(|report| report.total_bits != 0) else {
        return CellLayout {
            cells: vec![VisualCell::Unknown; width],
            visible_blocks: 0,
        };
    };
    segmented_cells(report, width, source).map_or_else(
        || dense_cells(report, width, source),
        |cells| CellLayout {
            cells,
            visible_blocks: report.blocks.len(),
        },
    )
}

fn output_layout(view: &StreamView, width: usize) -> CellLayout {
    if let (Some(source), Some(output)) = (view.source_report.as_ref(), view.output_report.as_ref())
    {
        if let Some(layout) = aligned_output_layout(source, output, width) {
            return layout;
        }
        if let Some(layout) = localized_output_layout(source, output, width) {
            return layout;
        }
    }

    let used = scaled_columns(view.output_bits, view.source_bits, width);
    let mut layout = layout_cells(view.output_report.as_ref(), used, false);
    layout.cells.resize(width, VisualCell::Saved);
    layout
}

fn aligned_output_layout(
    source: &BlockReport,
    output: &BlockReport,
    width: usize,
) -> Option<CellLayout> {
    if source.total_blocks != source.blocks.len()
        || output.total_blocks != output.blocks.len()
        || source.blocks.is_empty()
        || output.blocks.is_empty()
    {
        return None;
    }
    let source_widths = complete_segment_widths(source, width)?;
    let source_equivalent_bits = source_bits_by_output_block(source, output)?;
    let mut cells = Vec::with_capacity(width);

    let mut source_start = 0_usize;
    let mut output_start = 0_usize;
    let mut common_decoded = 0_usize;
    while source_start < source.blocks.len() && output_start < output.blocks.len() {
        let mut source_end = source_start + 1;
        let mut output_end = output_start + 1;
        let mut source_decoded =
            common_decoded.checked_add(source.blocks[source_start].decoded_bytes)?;
        let mut output_decoded =
            common_decoded.checked_add(output.blocks[output_start].decoded_bytes)?;

        while source_decoded != output_decoded {
            if source_decoded < output_decoded {
                let block = source.blocks.get(source_end)?;
                source_decoded = source_decoded.checked_add(block.decoded_bytes)?;
                source_end += 1;
            } else {
                let block = output.blocks.get(output_end)?;
                output_decoded = output_decoded.checked_add(block.decoded_bytes)?;
                output_end += 1;
            }
        }

        let source_count = source_end - source_start;
        let output_count = output_end - output_start;
        let source_span = source_widths[source_start..source_end]
            .iter()
            .try_fold(source_count.saturating_sub(1), |span, &block_width| {
                span.checked_add(block_width)
            })?;
        let output_boundaries = output_count.saturating_sub(1);
        let output_fill = source_span.checked_sub(output_boundaries)?;
        let block_widths = proportional_widths(
            &source_equivalent_bits[output_start..output_end],
            output_fill,
        )?;

        for (position, (&block_width, output_block)) in block_widths
            .iter()
            .zip(&output.blocks[output_start..output_end])
            .enumerate()
        {
            if position != 0 {
                cells.push(VisualCell::Boundary);
            }
            let reference_bits = source_equivalent_bits[output_start + position];
            let data_width = scaled_columns(output_block.output_bits, reference_bits, block_width);
            let encoding = effective_encoding(output_block.input, output_block.output, false);
            cells.extend(std::iter::repeat(VisualCell::Block(encoding)).take(data_width));
            cells.extend(
                std::iter::repeat(VisualCell::Saved).take(block_width.saturating_sub(data_width)),
            );
        }

        source_start = source_end;
        output_start = output_end;
        common_decoded = source_decoded;
        if source_start != source.blocks.len() {
            cells.push(VisualCell::Boundary);
        }
    }

    if source_start != source.blocks.len()
        || output_start != output.blocks.len()
        || cells.len() != width
    {
        return None;
    }
    Some(CellLayout {
        cells,
        visible_blocks: output.blocks.len(),
    })
}

/// Keep savings beside representative output blocks when the terminal is too
/// narrow to preserve every source and output boundary simultaneously.
fn localized_output_layout(
    source: &BlockReport,
    output: &BlockReport,
    width: usize,
) -> Option<CellLayout> {
    if width == 0
        || source.total_blocks != source.blocks.len()
        || output.total_blocks != output.blocks.len()
        || source.blocks.is_empty()
        || output.blocks.is_empty()
    {
        return None;
    }
    let source_equivalent_bits = source_bits_by_output_block(source, output)?;
    let target = output.blocks.len().min(width.div_ceil(2)).max(1);
    let selected = select_dense_blocks(output, target);
    let boundary_columns = selected.len().saturating_sub(1);
    let fill_columns = width.checked_sub(boundary_columns)?;
    let weights: Vec<u64> = selected
        .iter()
        .map(|&index| source_equivalent_bits[index])
        .collect();
    let block_widths = proportional_widths(&weights, fill_columns)?;
    let mut cells = Vec::with_capacity(width);
    for (position, (&index, &block_width)) in selected.iter().zip(&block_widths).enumerate() {
        if position != 0 {
            cells.push(VisualCell::Boundary);
        }
        let block = &output.blocks[index];
        let data_width = scaled_columns(
            block.output_bits,
            source_equivalent_bits[index],
            block_width,
        );
        let encoding = effective_encoding(block.input, block.output, false);
        cells.extend(std::iter::repeat(VisualCell::Block(encoding)).take(data_width));
        cells.extend(
            std::iter::repeat(VisualCell::Saved).take(block_width.saturating_sub(data_width)),
        );
    }
    if cells.len() != width {
        return None;
    }
    Some(CellLayout {
        cells,
        visible_blocks: selected.len(),
    })
}

/// Apportion source bits to output blocks by decoded overlap.
///
/// A changed plan can split, merge, or move Deflate boundaries. Mapping both
/// reports through their shared decoded stream gives each output block a
/// source-sized visual region, allowing savings to remain beside the data
/// that produced them instead of collecting at the right edge.
fn source_bits_by_output_block(source: &BlockReport, output: &BlockReport) -> Option<Vec<u64>> {
    let source_ranges = decoded_ranges(source)?;
    let output_ranges = decoded_ranges(output)?;
    if source_ranges.last().map(|range| range.1) != output_ranges.last().map(|range| range.1) {
        return None;
    }

    let mut apportioned = vec![0_u64; output.blocks.len()];
    for (source_index, &(source_start, source_end)) in source_ranges.iter().enumerate() {
        let source_bits = source.blocks[source_index].output_bits;
        let source_decoded = source_end.saturating_sub(source_start);
        if source_decoded == 0 {
            let output_index = output_ranges
                .iter()
                .position(|&(start, end)| start <= source_start && source_start < end)
                .or_else(|| {
                    output_ranges
                        .iter()
                        .position(|&(start, _)| start == source_start)
                })
                .unwrap_or_else(|| output_ranges.len().saturating_sub(1));
            apportioned[output_index] = apportioned[output_index].checked_add(source_bits)?;
            continue;
        }

        let mut shares = Vec::new();
        let mut covered = 0_u128;
        let mut assigned = 0_u64;
        for (output_index, &(output_start, output_end)) in output_ranges.iter().enumerate() {
            let overlap_start = source_start.max(output_start);
            let overlap_end = source_end.min(output_end);
            let overlap = overlap_end.saturating_sub(overlap_start);
            if overlap == 0 {
                continue;
            }
            covered = covered.checked_add(overlap)?;
            let scaled = u128::from(source_bits).checked_mul(overlap)?;
            let quotient = u64::try_from(scaled / source_decoded).ok()?;
            let remainder = scaled % source_decoded;
            assigned = assigned.checked_add(quotient)?;
            shares.push((remainder, output_index, quotient));
        }
        if covered != source_decoded || shares.is_empty() {
            return None;
        }
        shares.sort_unstable_by(|left, right| {
            right.0.cmp(&left.0).then_with(|| left.1.cmp(&right.1))
        });
        let residual = usize::try_from(source_bits.checked_sub(assigned)?).ok()?;
        if residual > shares.len() {
            return None;
        }
        for (position, &(_, output_index, quotient)) in shares.iter().enumerate() {
            let share = quotient + u64::from(position < residual);
            apportioned[output_index] = apportioned[output_index].checked_add(share)?;
        }
    }
    Some(apportioned)
}

fn decoded_ranges(report: &BlockReport) -> Option<Vec<(u128, u128)>> {
    let mut ranges = Vec::with_capacity(report.blocks.len());
    let mut start = 0_u128;
    for block in &report.blocks {
        let end = start.checked_add(block.decoded_bytes as u128)?;
        ranges.push((start, end));
        start = end;
    }
    Some(ranges)
}

fn proportional_widths(weights: &[u64], width: usize) -> Option<Vec<usize>> {
    if weights.is_empty() || width < weights.len() {
        return None;
    }
    let extra_columns = width - weights.len();
    let total_weight = weights
        .iter()
        .fold(0_u128, |total, &weight| total + u128::from(weight));
    let mut widths = vec![1_usize; weights.len()];
    if total_weight == 0 {
        for position in 0..extra_columns {
            widths[position % weights.len()] += 1;
        }
        return Some(widths);
    }

    let mut assigned_extra = 0_usize;
    let mut remainders = Vec::with_capacity(weights.len());
    for (index, &weight) in weights.iter().enumerate() {
        let scaled = extra_columns as u128 * u128::from(weight);
        let quotient = usize::try_from(scaled / total_weight).ok()?;
        widths[index] = widths[index].checked_add(quotient)?;
        assigned_extra = assigned_extra.checked_add(quotient)?;
        remainders.push((scaled % total_weight, weight, index));
    }
    remainders.sort_unstable_by(|left, right| right.cmp(left));
    for &(_, _, index) in remainders
        .iter()
        .take(extra_columns.checked_sub(assigned_extra)?)
    {
        widths[index] = widths[index].checked_add(1)?;
    }
    Some(widths)
}

fn work_column(layout: &CellLayout, work: WorkPosition, pulse: usize) -> Option<usize> {
    let data_columns = layout
        .cells
        .iter()
        .filter(|cell| matches!(cell, VisualCell::Block(_) | VisualCell::Unknown))
        .count();
    if data_columns == 0 {
        return None;
    }
    let ordinal = if work.total == 0 {
        pulse % data_columns
    } else {
        work.completed
            .saturating_mul(data_columns.saturating_sub(1))
            / work.total.max(1)
    };
    layout
        .cells
        .iter()
        .enumerate()
        .filter(|(_, cell)| matches!(cell, VisualCell::Block(_) | VisualCell::Unknown))
        .nth(ordinal)
        .map(|(column, _)| column)
}

fn render_layout(
    layout: &CellLayout,
    work_column: Option<usize>,
    work_alternate: bool,
    changed: Option<&[bool]>,
    color: bool,
    glyphs: Glyphs,
) -> String {
    let mut output = String::new();
    for (column, &cell) in layout.cells.iter().enumerate() {
        if work_column == Some(column) {
            let working = if work_alternate {
                glyphs.working_alternate
            } else {
                glyphs.working
            };
            if color {
                output.push_str("\x1b[1;97;7m");
                output.push(working);
                output.push_str("\x1b[0m");
            } else {
                output.push(working);
            }
            continue;
        }
        if changed.is_some_and(|columns| columns.get(column) == Some(&true)) {
            if color {
                output.push_str("\x1b[1;93;7m");
                output.push(glyphs.changed);
                output.push_str("\x1b[0m");
            } else {
                output.push(glyphs.changed);
            }
            continue;
        }

        match cell {
            VisualCell::Block(encoding) if color => {
                output.push_str(encoding_color(encoding));
                output.push(encoding_glyph(encoding, glyphs));
                output.push_str("\x1b[0m");
            }
            VisualCell::Block(encoding) => output.push(encoding_glyph(encoding, glyphs)),
            VisualCell::Boundary => output.push(glyphs.boundary),
            VisualCell::Saved => output.push(glyphs.saved),
            VisualCell::Unknown => output.push(glyphs.unknown),
        }
    }
    output
}

fn changed_columns(source: &[VisualCell], output: &[VisualCell]) -> Vec<bool> {
    output
        .iter()
        .enumerate()
        .map(|(index, cell)| source.get(index) != Some(cell))
        .collect()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum VisualCell {
    Block(BlockEncoding),
    Boundary,
    Saved,
    Unknown,
}

/// Lay out complete, reasonably small block lists as explicit segments.
///
/// Boundary markers consume their own columns, and every block receives at
/// least one type cell. A one-cell block therefore remains visible instead of
/// being replaced by its leading boundary. Very fragmented or truncated
/// reports fall back to a dense representative layout below.
fn segmented_cells(report: &BlockReport, width: usize, source: bool) -> Option<Vec<VisualCell>> {
    let widths = complete_segment_widths(report, width)?;

    let mut cells = Vec::with_capacity(width);
    for (index, (block, &block_width)) in report.blocks.iter().zip(&widths).enumerate() {
        if index != 0 {
            cells.push(VisualCell::Boundary);
        }
        let encoding = effective_encoding(block.input, block.output, source);
        cells.extend(std::iter::repeat(VisualCell::Block(encoding)).take(block_width));
    }
    debug_assert_eq!(cells.len(), width);
    Some(cells)
}

fn complete_segment_widths(report: &BlockReport, width: usize) -> Option<Vec<usize>> {
    let block_count = report.blocks.len();
    if block_count == 0
        || report.total_blocks != block_count
        || block_count.saturating_mul(2).saturating_sub(1) > width
    {
        return None;
    }

    let boundary_columns = block_count.saturating_sub(1);
    let fill_columns = width.checked_sub(boundary_columns)?;
    let extra_columns = fill_columns.checked_sub(block_count)?;
    let total_bits = std::num::NonZeroU128::new(u128::from(report.total_bits))?.get();
    let mut widths = vec![1_usize; block_count];
    let mut remainders = Vec::with_capacity(block_count);
    let mut assigned_extra = 0_usize;

    for (index, block) in report.blocks.iter().enumerate() {
        let scaled = extra_columns as u128 * u128::from(block.output_bits);
        let quotient = usize::try_from(scaled / total_bits).ok()?;
        widths[index] = widths[index].saturating_add(quotient);
        assigned_extra = assigned_extra.saturating_add(quotient);
        remainders.push((scaled % total_bits, block.output_bits, index));
    }

    remainders.sort_unstable_by(|left, right| right.cmp(left));
    for &(_, _, index) in remainders
        .iter()
        .take(extra_columns.saturating_sub(assigned_extra))
    {
        widths[index] = widths[index].saturating_add(1);
    }
    Some(widths)
}

/// Select a bounded schematic of a fragmented stream.
///
/// Changed encodings and unusually small blocks are selected first, then the
/// remaining cells are spread across the largest gaps. This makes the blocks
/// most likely to disappear under midpoint sampling explicitly visible.
fn dense_cells(report: &BlockReport, width: usize, source: bool) -> CellLayout {
    if width == 0 || report.blocks.is_empty() {
        return CellLayout {
            cells: vec![VisualCell::Unknown; width],
            visible_blocks: 0,
        };
    }
    let target = report.blocks.len().min(width.div_ceil(2)).max(1);
    let selected = select_dense_blocks(report, target);
    let cells = selected_cells(report, &selected, width, source);
    CellLayout {
        cells,
        visible_blocks: selected.len(),
    }
}

fn select_dense_blocks(report: &BlockReport, target: usize) -> Vec<usize> {
    let mut chosen = vec![false; report.blocks.len()];
    let mut count = 0_usize;
    choose_block(&mut chosen, &mut count, target, 0);
    choose_block(
        &mut chosen,
        &mut count,
        target,
        report.blocks.len().saturating_sub(1),
    );

    for (index, block) in report.blocks.iter().enumerate() {
        if block.output != BlockEncoding::Original && block.output != block.input {
            choose_block(&mut chosen, &mut count, target, index);
        }
    }

    let mut by_size: Vec<(u64, usize)> = report
        .blocks
        .iter()
        .enumerate()
        .map(|(index, block)| (block.output_bits, index))
        .collect();
    by_size.sort_unstable();
    let tiny_budget = (target / 4).max(1);
    for &(_, index) in by_size.iter().take(tiny_budget) {
        choose_block(&mut chosen, &mut count, target, index);
    }

    while count < target {
        let mut previous = None;
        let mut largest_gap = None;
        for (index, &is_chosen) in chosen.iter().enumerate() {
            if !is_chosen {
                continue;
            }
            if let Some(start) = previous {
                let gap = index.saturating_sub(start + 1);
                if gap != 0 && largest_gap.map_or(true, |(_, best)| gap > best) {
                    largest_gap = Some((start + 1 + gap / 2, gap));
                }
            }
            previous = Some(index);
        }
        let index = largest_gap
            .map(|(index, _)| index)
            .or_else(|| chosen.iter().position(|is_chosen| !is_chosen));
        let Some(index) = index else {
            break;
        };
        choose_block(&mut chosen, &mut count, target, index);
    }

    chosen
        .iter()
        .enumerate()
        .filter_map(|(index, &is_chosen)| is_chosen.then_some(index))
        .collect()
}

fn choose_block(chosen: &mut [bool], count: &mut usize, target: usize, index: usize) {
    if *count < target && chosen.get(index) == Some(&false) {
        chosen[index] = true;
        *count += 1;
    }
}

fn selected_cells(
    report: &BlockReport,
    selected: &[usize],
    width: usize,
    source: bool,
) -> Vec<VisualCell> {
    let boundary_columns = selected.len().saturating_sub(1);
    let fill_columns = width.saturating_sub(boundary_columns);
    let extra_columns = fill_columns.saturating_sub(selected.len());
    let total_bits = selected.iter().fold(0_u128, |total, &index| {
        total + u128::from(report.blocks[index].output_bits)
    });
    let mut widths = vec![1_usize; selected.len()];
    let mut assigned_extra = 0_usize;
    let mut remainders = Vec::with_capacity(selected.len());
    if let Some(total_bits) = std::num::NonZeroU128::new(total_bits) {
        let total_bits = total_bits.get();
        for (position, &index) in selected.iter().enumerate() {
            let bits = report.blocks[index].output_bits;
            let scaled = extra_columns as u128 * u128::from(bits);
            let quotient = usize::try_from(scaled / total_bits).unwrap_or(0);
            widths[position] = widths[position].saturating_add(quotient);
            assigned_extra = assigned_extra.saturating_add(quotient);
            remainders.push((scaled % total_bits, bits, position));
        }
        remainders.sort_unstable_by(|left, right| right.cmp(left));
        for &(_, _, position) in remainders
            .iter()
            .take(extra_columns.saturating_sub(assigned_extra))
        {
            widths[position] = widths[position].saturating_add(1);
        }
    } else if !widths.is_empty() {
        for position in 0..extra_columns {
            widths[position % selected.len()] += 1;
        }
    }

    let mut cells = Vec::with_capacity(width);
    for (position, (&index, &block_width)) in selected.iter().zip(&widths).enumerate() {
        if position != 0 {
            cells.push(VisualCell::Boundary);
        }
        let block = &report.blocks[index];
        let encoding = effective_encoding(block.input, block.output, source);
        cells.extend(std::iter::repeat(VisualCell::Block(encoding)).take(block_width));
    }
    cells.resize(width, VisualCell::Unknown);
    cells
}

fn effective_encoding(input: BlockEncoding, output: BlockEncoding, source: bool) -> BlockEncoding {
    if source || output == BlockEncoding::Original {
        input
    } else {
        output
    }
}

fn encoding_glyph(encoding: BlockEncoding, glyphs: Glyphs) -> char {
    match encoding {
        BlockEncoding::Stored => glyphs.stored,
        BlockEncoding::Fixed => glyphs.fixed,
        BlockEncoding::Dynamic => glyphs.dynamic,
        BlockEncoding::Original => glyphs.original,
    }
}

fn encoding_color(encoding: BlockEncoding) -> &'static str {
    match encoding {
        BlockEncoding::Stored => STORED_COLOR,
        BlockEncoding::Fixed => FIXED_COLOR,
        BlockEncoding::Dynamic => DYNAMIC_COLOR,
        BlockEncoding::Original => "\x1b[2m",
    }
}

fn structure_change(source: Option<&BlockReport>, output: Option<&BlockReport>) -> String {
    let (Some(source), Some(output)) = (source, output) else {
        return String::new();
    };
    let mut changes = Vec::new();
    match output.total_blocks.cmp(&source.total_blocks) {
        std::cmp::Ordering::Less => changes.push(format!(
            "merged {}→{} blocks",
            source.total_blocks, output.total_blocks
        )),
        std::cmp::Ordering::Greater => changes.push(format!(
            "split {}→{} blocks",
            source.total_blocks, output.total_blocks
        )),
        std::cmp::Ordering::Equal => {
            if decoded_boundaries(source) != decoded_boundaries(output) {
                changes.push("boundaries moved".to_owned());
            }
        }
    }

    let changed_types = source
        .blocks
        .iter()
        .zip(&output.blocks)
        .filter(|(before, after)| {
            let after = if after.output == BlockEncoding::Original {
                after.input
            } else {
                after.output
            };
            before.input != after
        })
        .count();
    if changed_types != 0 {
        changes.push(format!(
            "{changed_types} type {}",
            if changed_types == 1 {
                "change"
            } else {
                "changes"
            }
        ));
    }
    changes.join(" · ")
}

fn decoded_boundaries(report: &BlockReport) -> Vec<usize> {
    let mut offset = 0_usize;
    report
        .blocks
        .iter()
        .take(report.blocks.len().saturating_sub(1))
        .map(|block| {
            offset = offset.saturating_add(block.decoded_bytes);
            offset
        })
        .collect()
}

fn scaled_columns(output_bits: u64, source_bits: u64, width: usize) -> usize {
    if width == 0 || output_bits == 0 {
        return 0;
    }
    if source_bits == 0 || output_bits >= source_bits {
        return width;
    }
    // A strict floor makes even a sub-cell saving visible. The percentage and
    // exact byte count below the bar keep that intentionally eager visual
    // contraction quantitatively honest.
    let scaled = width as u128 * u128::from(output_bits) / u128::from(source_bits);
    usize::try_from(scaled).unwrap_or(width).clamp(1, width)
}

fn is_smaller(bytes: usize, bits: u64, best_bytes: usize, best_bits: u64) -> bool {
    bytes < best_bytes || (bytes == best_bytes && bits < best_bits)
}

fn bit_change(source_bits: u64, output_bits: u64) -> String {
    match source_bits.cmp(&output_bits) {
        std::cmp::Ordering::Greater => {
            format!("saved {} bits", source_bits - output_bits)
        }
        std::cmp::Ordering::Less => format!("added {} bits", output_bits - source_bits),
        std::cmp::Ordering::Equal => "unchanged".to_owned(),
    }
}

fn compact_change(source_bits: u64, output_bits: u64) -> String {
    if source_bits == output_bits {
        return "—".to_owned();
    }
    let (glyph, difference) = if source_bits > output_bits {
        ('↓', source_bits - output_bits)
    } else {
        ('↑', output_bits - source_bits)
    };
    if source_bits == 0 {
        return format!("{glyph}{difference}b");
    }
    let hundredths =
        (u128::from(difference) * 10_000 + u128::from(source_bits) / 2) / u128::from(source_bits);
    format!("{glyph}{}.{:02}%", hundredths / 100, hundredths % 100)
}

fn block_count_change(
    source_blocks: usize,
    output_blocks: usize,
    visible_blocks: Option<usize>,
) -> String {
    let mut summary = if source_blocks == output_blocks {
        format!(
            "{source_blocks} {}",
            if source_blocks == 1 {
                "block"
            } else {
                "blocks"
            }
        )
    } else {
        format!("{source_blocks}→{output_blocks} blocks")
    };
    if let Some(visible) = visible_blocks.filter(|&visible| visible < output_blocks) {
        summary.push_str(&format!(" · {visible} visible"));
    }
    summary
}

fn format_bytes(bytes: usize) -> String {
    format_bytes_u64(u64::try_from(bytes).unwrap_or(u64::MAX))
}

fn format_bytes_u64(bytes: u64) -> String {
    const KIB: u64 = 1024;
    const MIB: u64 = 1024 * KIB;
    const GIB: u64 = 1024 * MIB;
    if bytes < KIB {
        format!("{bytes} B")
    } else {
        let (unit, divisor) = if bytes < MIB {
            ("KiB", KIB)
        } else if bytes < GIB {
            ("MiB", MIB)
        } else {
            ("GiB", GIB)
        };
        let tenths = (u128::from(bytes) * 10 + u128::from(divisor) / 2) / u128::from(divisor);
        format!("{}.{:01} {unit}", tenths / 10, tenths % 10)
    }
}

fn terminal_columns() -> usize {
    query_terminal_columns()
        .or_else(|| {
            env::var("COLUMNS")
                .ok()
                .and_then(|value| value.parse::<usize>().ok())
                .filter(|&columns| columns != 0)
        })
        .unwrap_or(DEFAULT_COLUMNS)
        .min(MAX_TERMINAL_COLUMNS)
}

#[cfg(unix)]
fn query_terminal_columns() -> Option<usize> {
    let terminal = File::open("/dev/tty").ok()?;
    let output = Command::new("stty")
        .arg("size")
        .stdin(Stdio::from(terminal))
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    std::str::from_utf8(&output.stdout)
        .ok()?
        .split_whitespace()
        .next_back()?
        .parse()
        .ok()
}

#[cfg(not(unix))]
fn query_terminal_columns() -> Option<usize> {
    None
}

fn card_dimensions(terminal_width: usize) -> (usize, usize) {
    let terminal_width = terminal_width.clamp(1, MAX_TERMINAL_COLUMNS);
    let proportional = terminal_width.saturating_mul(CARD_WIDTH_NUMERATOR) / CARD_WIDTH_DENOMINATOR;
    let card_width = proportional
        .max(MIN_CARD_COLUMNS.min(terminal_width))
        .min(terminal_width);
    (0, card_width)
}

fn bar_columns(card_width: usize, prefix_width: usize) -> usize {
    card_width.saturating_sub(prefix_width.saturating_add(2))
}

fn fit_text(text: &str, width: usize, glyphs: Glyphs) -> String {
    if width == 0 {
        return String::new();
    }
    let text = display_text(text, glyphs);
    if visible_width(&text) <= width {
        return text;
    }
    let ellipsis = if glyphs.unicode { "…" } else { "..." };
    let ellipsis_width = visible_width(ellipsis);
    if width <= ellipsis_width {
        return ".".repeat(width);
    }
    let mut result: String = text.chars().take(width - ellipsis_width).collect();
    result.push_str(ellipsis);
    result
}

fn display_text(text: &str, glyphs: Glyphs) -> String {
    if glyphs.unicode {
        return text.to_owned();
    }
    let mut output = String::with_capacity(text.len());
    for character in text.chars() {
        match character {
            '░' => output.push('S'),
            '▒' => output.push('F'),
            '▓' => output.push('D'),
            '│' => output.push('|'),
            '◆' => output.push('*'),
            '▲' => output.push('^'),
            '·' => output.push('|'),
            '→' => output.push_str("->"),
            '↓' => output.push('-'),
            '↑' => output.push('+'),
            '—' | '–' => output.push('-'),
            '✓' => output.push_str("OK"),
            'µ' => output.push('u'),
            '…' => output.push_str("..."),
            other if other.is_ascii() => output.push(other),
            _ => output.push('?'),
        }
    }
    output
}

fn visible_width(text: &str) -> usize {
    // Every visual glyph is deliberately one cell wide. Locales that commonly
    // render box/block characters as double-width automatically use ASCII.
    text.chars().count()
}

fn color_enabled() -> bool {
    terminal::stderr_color_enabled()
}

fn unicode_enabled() -> bool {
    if env::var_os("COLUMBO_ASCII").is_some() {
        return false;
    }
    let locale = ["LC_ALL", "LC_CTYPE", "LANG"]
        .into_iter()
        .find_map(|name| env::var(name).ok().filter(|value| !value.is_empty()));
    unicode_enabled_for(locale.as_deref())
}

fn unicode_enabled_for(locale: Option<&str>) -> bool {
    let Some(locale) = locale else {
        return true;
    };
    let locale = locale.to_ascii_lowercase();
    if matches!(locale.as_str(), "c" | "posix") {
        return false;
    }
    let utf8 = locale.contains("utf-8") || locale.contains("utf8");
    let ambiguous_width_locale =
        locale.starts_with("ja") || locale.starts_with("ko") || locale.starts_with("zh");
    utf8 && !ambiguous_width_locale
}

fn style(enabled: bool, sequence: &'static str) -> &'static str {
    if enabled {
        sequence
    } else {
        ""
    }
}

fn reset(enabled: bool) -> &'static str {
    if enabled {
        "\x1b[0m"
    } else {
        ""
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::progress::BlockProgress;

    fn report(types: &[BlockEncoding], bits: &[u64], decoded: &[usize]) -> BlockReport {
        BlockReport {
            blocks: types
                .iter()
                .zip(bits)
                .zip(decoded)
                .enumerate()
                .map(|(index, ((encoding, bits), decoded))| BlockProgress {
                    alignment: 0,
                    decoded_bytes: *decoded,
                    final_block: index + 1 == types.len(),
                    input: *encoding,
                    output: *encoding,
                    output_bits: *bits,
                    tokens: 0,
                })
                .collect(),
            total_blocks: types.len(),
            total_bits: bits.iter().sum(),
        }
    }

    fn idle_view(stream_id: usize) -> StreamView {
        StreamView {
            id: stream_id,
            duplicates: Vec::new(),
            note: None,
            source_bits: 8,
            source_blocks: 1,
            source_bytes: 1,
            source_report: None,
            output_bits: 8,
            output_bytes: 1,
            output_report: None,
            change_highlight_draws: 0,
            finished: false,
            pulse: 0,
            status: String::new(),
            work: None,
        }
    }

    #[test]
    fn concurrent_trials_remain_distinct_and_render_in_physical_stream_order() {
        let mut renderer = Renderer::default();
        renderer.views.insert(20, idle_view(2));
        renderer.views.insert(10, idle_view(2));
        renderer.views.insert(30, idle_view(1));

        let order: Vec<_> = renderer
            .ordered_views()
            .into_iter()
            .map(|(report_id, view)| (view.id, report_id))
            .collect();
        assert_eq!(order, [(1, 30), (2, 10), (2, 20)]);
    }

    #[test]
    fn card_is_left_aligned_bounded_and_shows_savings_inside_aligned_blocks() {
        let source = report(
            &[BlockEncoding::Dynamic, BlockEncoding::Fixed],
            &[600, 400],
            &[100, 50],
        );
        let mut output = source.clone();
        output.total_bits = 750;
        output.blocks[0].output_bits = 450;
        output.blocks[1].output_bits = 300;
        let view = StreamView {
            id: 1,
            duplicates: Vec::new(),
            note: None,
            source_bits: 1_000,
            source_blocks: 2,
            source_bytes: 125,
            source_report: Some(source),
            output_bits: 750,
            output_bytes: 94,
            output_report: Some(output),
            change_highlight_draws: 0,
            finished: false,
            pulse: 0,
            status: "candidate".to_owned(),
            work: None,
        };

        let [heading, input, output, information] =
            render_card(&view, 80, false, Glyphs::for_unicode(true));
        assert!(visible_width(&heading) <= 80);
        assert!(visible_width(&input) <= 80);
        assert!(visible_width(&output) <= 80);
        assert!(visible_width(&information) <= 80);
        assert_eq!(heading, "Stream 01");
        assert!(input.starts_with("  in  ["));
        assert_eq!(input.find('['), output.find('['));
        assert_eq!(visible_width(&input), visible_width(&output));
        assert!(input.contains('▓'));
        assert!(input.contains('│'));
        assert!(output.contains("··"));
        let input_boundary = input.chars().position(|cell| cell == '│').unwrap();
        let output_boundary = output.chars().position(|cell| cell == '│').unwrap();
        assert_eq!(input_boundary, output_boundary);
        let output_cells: Vec<char> = output.chars().collect();
        assert!(output_cells[..output_boundary].contains(&'·'));
        assert!(output_cells[output_boundary + 1..].contains(&'·'));
        assert!(information.contains("125 B → 94 B"));
        assert!(information.contains("2 blocks"));
        assert!(information.contains("↓25.00%"));
        assert!(information.contains("candidate"));
        assert!(!input.contains('\x1b'));
        assert!(!output.contains('\x1b'));
        assert!(!information.contains('\x1b'));
    }

    #[test]
    fn stream_title_identifies_physical_duplicates_sharing_work() {
        let view = StreamView {
            id: 2,
            duplicates: vec![5, 8],
            note: None,
            source_bits: 8,
            source_blocks: 1,
            source_bytes: 1,
            source_report: None,
            output_bits: 8,
            output_bytes: 1,
            output_report: None,
            change_highlight_draws: 0,
            finished: false,
            pulse: 0,
            status: String::new(),
            work: None,
        };

        assert_eq!(stream_title(&view), "Stream 02 · duplicates 05, 08");
        let [heading, ..] = render_card(&view, 80, false, Glyphs::for_unicode(true));
        assert_eq!(heading, "Stream 02 · duplicates 05, 08");

        let mut reclaimed = view;
        reclaimed.note = Some("reclaimed time");
        assert_eq!(
            stream_title(&reclaimed),
            "Stream 02 · reclaimed time · duplicates 05, 08"
        );
    }

    #[test]
    fn split_layout_keeps_shared_boundaries_and_places_savings_at_their_source() {
        let source = report(
            &[
                BlockEncoding::Dynamic,
                BlockEncoding::Dynamic,
                BlockEncoding::Dynamic,
            ],
            &[600, 300, 100],
            &[100, 100, 100],
        );
        let output = report(
            &[
                BlockEncoding::Dynamic,
                BlockEncoding::Dynamic,
                BlockEncoding::Dynamic,
                BlockEncoding::Dynamic,
            ],
            &[240, 240, 300, 100],
            &[50, 50, 100, 100],
        );

        assert_eq!(
            source_bits_by_output_block(&source, &output).unwrap(),
            [300, 300, 300, 100]
        );
        let source_layout = layout_cells(Some(&source), 25, true);
        let output_layout = aligned_output_layout(&source, &output, 25).unwrap();
        let source_boundaries: Vec<_> = source_layout
            .cells
            .iter()
            .enumerate()
            .filter_map(|(column, cell)| (*cell == VisualCell::Boundary).then_some(column))
            .collect();
        let output_boundaries: Vec<_> = output_layout
            .cells
            .iter()
            .enumerate()
            .filter_map(|(column, cell)| (*cell == VisualCell::Boundary).then_some(column))
            .collect();
        let saved: Vec<_> = output_layout
            .cells
            .iter()
            .enumerate()
            .filter_map(|(column, cell)| (*cell == VisualCell::Saved).then_some(column))
            .collect();

        assert!(source_boundaries
            .iter()
            .all(|boundary| output_boundaries.contains(boundary)));
        assert_eq!(output_boundaries.len(), source_boundaries.len() + 1);
        assert!(!saved.is_empty());
        assert!(saved.iter().all(|&column| column < source_boundaries[0]));
        assert_ne!(output_layout.cells.last(), Some(&VisualCell::Saved));
    }

    #[test]
    fn narrow_split_layout_does_not_collect_savings_at_the_right_edge() {
        let source = report(
            &[BlockEncoding::Dynamic; 6],
            &[1_200, 500, 400, 300, 200, 100],
            &[100; 6],
        );
        let output = report(
            &[BlockEncoding::Dynamic; 7],
            &[240, 240, 500, 400, 300, 200, 100],
            &[50, 50, 100, 100, 100, 100, 100],
        );

        assert!(aligned_output_layout(&source, &output, 10).is_none());
        let layout = localized_output_layout(&source, &output, 10).unwrap();
        let saved: Vec<_> = layout
            .cells
            .iter()
            .enumerate()
            .filter_map(|(column, cell)| (*cell == VisualCell::Saved).then_some(column))
            .collect();
        assert!(!saved.is_empty());
        assert!(saved.iter().all(|&column| column < 4));
        assert_ne!(layout.cells.last(), Some(&VisualCell::Saved));
    }

    #[test]
    fn cards_are_left_aligned_and_use_nine_tenths_of_common_terminal_widths() {
        assert_eq!(card_dimensions(80), (0, 72));
        assert_eq!(card_dimensions(100), (0, 90));
        assert_eq!(card_dimensions(120), (0, 108));
        assert_eq!(card_dimensions(200), (0, 180));
    }

    #[test]
    fn huffman_blocks_use_the_teal_palette() {
        assert_eq!(encoding_color(BlockEncoding::Fixed), "\x1b[36m");
        assert_eq!(encoding_color(BlockEncoding::Dynamic), "\x1b[96m");
    }

    #[test]
    fn a_one_cell_middle_block_keeps_its_type_glyph() {
        let blocks = report(
            &[
                BlockEncoding::Dynamic,
                BlockEncoding::Dynamic,
                BlockEncoding::Dynamic,
            ],
            &[100, 1, 100],
            &[100, 1, 100],
        );

        let layout = layout_cells(Some(&blocks), 7, true);
        assert_eq!(
            render_layout(&layout, None, false, None, false, Glyphs::for_unicode(true)),
            "▓▓│▓│▓▓"
        );
    }

    #[test]
    fn dense_layout_preserves_tiny_blocks_and_reports_visibility() {
        let mut types = vec![BlockEncoding::Dynamic; 10];
        types[4] = BlockEncoding::Fixed;
        let mut bits = vec![100; 10];
        bits[4] = 1;
        let decoded = vec![10; 10];
        let blocks = report(&types, &bits, &decoded);
        let layout = layout_cells(Some(&blocks), 7, true);
        let rendered = render_layout(&layout, None, false, None, false, Glyphs::for_unicode(true));
        assert_eq!(layout.visible_blocks, 4);
        assert!(rendered.contains('▒'));
        assert_eq!(
            block_count_change(10, 10, Some(layout.visible_blocks)),
            "10 blocks · 4 visible"
        );
    }

    #[test]
    fn changed_cells_have_a_non_color_marker() {
        let source = report(&[BlockEncoding::Dynamic], &[100], &[10]);
        let output = report(&[BlockEncoding::Fixed], &[90], &[10]);
        let source = layout_cells(Some(&source), 5, true);
        let output = layout_cells(Some(&output), 5, false);
        let changed = changed_columns(&source.cells, &output.cells);
        assert_eq!(
            render_layout(
                &output,
                None,
                false,
                Some(&changed),
                false,
                Glyphs::for_unicode(true),
            ),
            "▲▲▲▲▲"
        );
    }

    #[test]
    fn indeterminate_work_cursor_moves_and_pulses() {
        let blocks = report(&[BlockEncoding::Dynamic], &[100], &[10]);
        let layout = layout_cells(Some(&blocks), 5, false);
        let work = WorkPosition {
            completed: 0,
            total: 0,
        };
        let first = work_column(&layout, work, 0).unwrap();
        let second = work_column(&layout, work, 1).unwrap();
        assert_eq!((first, second), (0, 1));
        assert_eq!(
            render_layout(
                &layout,
                Some(first),
                false,
                None,
                false,
                Glyphs::for_unicode(true),
            ),
            "◆▓▓▓▓"
        );
        assert_eq!(
            render_layout(
                &layout,
                Some(second),
                true,
                None,
                false,
                Glyphs::for_unicode(true),
            ),
            "▓◇▓▓▓"
        );
    }

    #[test]
    fn incompatible_locales_use_an_ascii_one_cell_fallback() {
        assert!(!unicode_enabled_for(Some("C")));
        assert!(!unicode_enabled_for(Some("ja_JP.UTF-8")));
        assert!(unicode_enabled_for(Some("en_GB.UTF-8")));
        assert_eq!(
            display_text("▓│◆▲·→↓✓µs", Glyphs::for_unicode(false)),
            "D|*^|->-OKus"
        );
    }

    #[test]
    fn structural_summary_names_merges_splits_boundaries_and_types() {
        let source = report(
            &[BlockEncoding::Dynamic, BlockEncoding::Dynamic],
            &[50, 50],
            &[10, 10],
        );
        let merged = report(&[BlockEncoding::Fixed], &[90], &[20]);
        assert_eq!(
            structure_change(Some(&source), Some(&merged)),
            "merged 2→1 blocks · 1 type change"
        );

        let moved = report(
            &[BlockEncoding::Dynamic, BlockEncoding::Dynamic],
            &[45, 45],
            &[12, 8],
        );
        assert_eq!(
            structure_change(Some(&source), Some(&moved)),
            "boundaries moved"
        );
    }

    #[test]
    fn scaling_is_safe_and_keeps_nonempty_streams_visible() {
        assert_eq!(scaled_columns(1, u64::MAX, 55), 1);
        assert_eq!(scaled_columns(500, 1_000, 60), 30);
        assert_eq!(scaled_columns(2_000, 1_000, 60), 60);
    }
}
