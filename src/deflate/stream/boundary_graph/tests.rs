// SPDX-License-Identifier: MIT
// SPDX-FileCopyrightText: 2026 Andrew C.E. Dent <https://github.com/ace-dent>

use super::*;
use crate::deflate::bitstream::BitWriter;
use crate::deflate::block::{emit_block, plan_block};
use crate::deflate::model::{canonical_length_encoding, count_frequencies};
use crate::deflate::stream::plan_stream;
use crate::deflate::stream::test_support::{literal_block, overlapping_match_block};

fn seven_inside_eighths_block() -> ParsedBlock {
    let (length_symbol, _, _) = canonical_length_encoding(15).unwrap();
    let mut tokens = vec![Token::Literal(b'a'), Token::Literal(b'a')];
    for match_index in 0..7 {
        tokens.push(Token::Match {
            length: 15,
            distance: 1,
            length_symbol,
            distance_symbol: 0,
        });
        if match_index != 6 {
            tokens.push(Token::Literal(b'a'));
        }
    }
    tokens.extend((0..15).map(|_| Token::Literal(b'a')));
    let (literal_frequencies, distance_frequencies) = count_frequencies(&tokens);
    ParsedBlock {
        tokens: tokens.into(),
        plain: vec![b'a'; 128].into(),
        literal_frequencies,
        distance_frequencies,
        original_literal_lengths: None,
        original_distance_lengths: None,
        original_dynamic: None,
        original: None,
        source_splits: Vec::new(),
        source_type: SourceBlockType::Dynamic,
    }
}

#[test]
fn interior_edge_templates_reconstruct_the_exact_priced_tokens() {
    let block = overlapping_match_block(258);
    let blocks = [block];
    let composite = Composite::new(&blocks).unwrap();
    let whole_start = Cut { token: 0, plain: 0 };
    let inside = composite.cut_at_plain(100).unwrap();
    let whole_end = Cut {
        token: 2,
        plain: 259,
    };
    let options = Options::default();
    let mut plan_cache = CanonicalPlanCache::new();

    for (start, end) in [(whole_start, inside), (inside, whole_end)] {
        let expected = make_range(&composite, start, end).unwrap();
        for alignment in [0, 3, 7] {
            let edge = prepare_edge(
                &blocks,
                &composite,
                start,
                end,
                &options,
                &mut plan_cache,
                &mut SearchStop::never(),
            )
            .unwrap();
            let template = edge.plan(alignment);
            let plan = template.instantiate(&composite, start, end).unwrap();
            assert_eq!(plan.tokens.as_slice(), expected.tokens.as_slice());
            assert_eq!(plan.plain.as_slice(), expected.plain.as_slice());
            assert_eq!(plan.bits, template.bits);

            let mut writer = BitWriter::default();
            writer.write(0, alignment).unwrap();
            emit_block(&mut writer, &[], &plan, true).unwrap();
            assert_eq!(writer.bit_position() - u64::from(alignment), template.bits);
        }
    }
}

#[test]
fn source_aligned_floor_prices_short_huffman_groups_before_search() {
    let blocks = [
        literal_block(&vec![b'a'; 800], SourceBlockType::Dynamic),
        literal_block(&vec![b'a'; 800], SourceBlockType::Dynamic),
        literal_block(&vec![b'a'; 800], SourceBlockType::Dynamic),
    ];
    let options = Options::default();
    let mut plan_cache = CanonicalPlanCache::new();
    let plans = source_aligned_huffman_floor(&blocks, 0, &options, &mut plan_cache).unwrap();

    assert_eq!(plans.len(), 1);
    assert_eq!(plans[0].plain.len(), 2_400);
    let separate_bits: u64 = blocks
        .iter()
        .map(|block| plan_block(block, 0, &options, &mut SearchStop::never()).bits)
        .sum();
    assert!(total_bits(&plans) < separate_bits);

    // Max mode allows regrouping regardless of encoded source size. Once
    // allowed at call entry, the complete structural floor remains available
    // even if the next deadline check ends optional search.
    let exhaustive = Options {
        exhaustive: true,
        ..Options::default()
    };
    let mut deadline_checks = 0;
    let mut expires = || {
        deadline_checks += 1;
        deadline_checks > 1
    };
    let deadline_safe = plan_stream(
        &blocks,
        0,
        0,
        &exhaustive,
        &mut SearchStop::callback(&mut expires),
    )
    .unwrap();
    assert_eq!(deadline_safe.len(), 1);
    assert_eq!(deadline_safe[0].plain.len(), 2_400);
}

fn boundary_priority_block(run_count: usize) -> ParsedBlock {
    let (length_symbol, _, _) = canonical_length_encoding(257).unwrap();
    let mut tokens = Vec::new();
    for _ in 0..run_count {
        tokens.push(Token::Literal(b'a'));
        tokens.extend((0..200).map(|_| Token::Match {
            length: 257,
            distance: 1,
            length_symbol,
            distance_symbol: 0,
        }));
    }
    let plain_len = tokens.iter().map(|token| token.decoded_len()).sum();
    let (literal_frequencies, distance_frequencies) = count_frequencies(&tokens);
    ParsedBlock {
        tokens: tokens.into(),
        plain: vec![b'a'; plain_len].into(),
        literal_frequencies,
        distance_frequencies,
        original_literal_lengths: None,
        original_distance_lengths: None,
        original_dynamic: None,
        original: None,
        source_splits: Vec::new(),
        source_type: SourceBlockType::Dynamic,
    }
}

#[test]
fn established_boundary_priority_covers_a_first_split_but_stays_bounded() {
    let one_block = [boundary_priority_block(2)];
    assert!(established_boundary_graph_priority(&one_block));

    let two_blocks = [boundary_priority_block(1), boundary_priority_block(1)];
    assert!(established_boundary_graph_priority(&two_blocks));

    let three_blocks = [
        boundary_priority_block(1),
        boundary_priority_block(1),
        literal_block(b"a", SourceBlockType::Dynamic),
    ];
    assert!(!established_boundary_graph_priority(&three_blocks));
}

#[test]
fn exact_siblings_do_not_displace_legacy_cuts_at_the_graph_limit() {
    let block = seven_inside_eighths_block();
    let blocks = vec![block; 128];
    let composite = Composite::new(&blocks).unwrap();
    let cuts = choose_cuts(&composite, true, false).unwrap();

    // Each source contributes seven distinct snapped and seven distinct
    // exact eighths, plus the 129 shared source endpoints. Counting raw
    // duplicate endpoint pushes would exceed the 2,048-anchor limit.
    assert_eq!(cuts.len(), 1_921);
    assert!(cuts.contains(&Cut {
        token: 3_812,
        plain: 16_258,
    }));
    assert!(cuts.contains(&Cut {
        token: 3_812,
        plain: 16_272,
    }));
}

#[test]
fn combined_run_adds_eighth_regroup_boundaries() {
    let left = literal_block(&vec![b'a'; 20_000], SourceBlockType::Dynamic);
    let right = literal_block(&vec![b'b'; 20_000], SourceBlockType::Dynamic);
    let blocks = [left, right];
    let composite = Composite::new(&blocks).unwrap();
    let cuts = choose_cuts(&composite, false, true).unwrap();
    assert!(cuts.contains(&Cut {
        token: 4_999,
        plain: 4_999,
    }));
    assert!(cuts.contains(&Cut {
        token: 20_000,
        plain: 20_000,
    }));
}

#[test]
fn compact_max_cut_set_reaches_late_32_token_probes() {
    let block = literal_block(&vec![b'x'; 384], SourceBlockType::Dynamic);
    let blocks = [block];
    let composite = Composite::new(&blocks).unwrap();
    let cuts = choose_cuts(&composite, true, true).unwrap();

    // This cut is near the end. Scoring all compact probes before deep
    // whole-block searches keeps it reachable under a finite Max deadline.
    assert!(cuts.contains(&Cut {
        token: 320,
        plain: 320,
    }));
}
