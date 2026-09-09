// SPDX-License-Identifier: MIT

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
fn terminal_columns_accept_only_nonzero_decimal_digits() {
    assert_eq!(parse_terminal_columns("80"), Some(80));
    for invalid in ["", "0", "+80", " 80", "80 ", "8.0"] {
        assert_eq!(parse_terminal_columns(invalid), None);
    }
    assert_eq!(parse_terminal_columns(&format!("{}0", usize::MAX)), None);
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
