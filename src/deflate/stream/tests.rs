// SPDX-License-Identifier: MIT
// SPDX-FileCopyrightText: 2026 Andrew C.E. Dent <https://github.com/ace-dent>

use super::boundary_graph::{boundary_dp, exact_source};
use super::test_support::{
    assert_same_plan, literal_block, overlapping_match_block, sequential_plan,
};
use super::*;
use crate::deflate::bitstream::BitWriter;
use crate::deflate::block::{emit_block, stored_block_bits};
use crate::deflate::model::{token_extra_bits, OriginalBits};
use crate::deflate::parse::parse_stream;

#[test]
fn cheapest_grouped_layout_retains_the_selected_route_kind() {
    let blocks: &[ParsedBlock] = &[];
    let selected = cheapest_grouped_layout([
        Some((GroupedLayout::Greedy, blocks, 30)),
        Some((GroupedLayout::Bounded, blocks, 20)),
        Some((GroupedLayout::Collected, blocks, 10)),
    ]);

    let (kind, selected_blocks) = selected.expect("one grouped layout is available");
    assert_eq!(kind, GroupedLayout::Collected);
    assert!(selected_blocks.is_empty());
}

fn short_match_block() -> ParsedBlock {
    let mut tokens = vec![Token::Literal(b'a')];
    tokens.extend((0..100).map(|_| Token::Match {
        length: 6,
        distance: 1,
        length_symbol: 260,
        distance_symbol: 0,
    }));
    let (literal_frequencies, distance_frequencies) = count_frequencies(&tokens);
    ParsedBlock {
        tokens: tokens.into(),
        plain: vec![b'a'; 601].into(),
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

fn periodic_distance_three_block() -> ParsedBlock {
    let (length_symbol, _, _) = canonical_length_encoding(258).unwrap();
    let tokens = vec![
        Token::Literal(b'a'),
        Token::Literal(b'b'),
        Token::Literal(b'c'),
        Token::Match {
            length: 258,
            distance: 3,
            length_symbol,
            distance_symbol: 2,
        },
    ];
    let (literal_frequencies, distance_frequencies) = count_frequencies(&tokens);
    ParsedBlock {
        tokens: tokens.into(),
        plain: (0..261)
            .map(|index| b"abc"[index % 3])
            .collect::<Vec<_>>()
            .into(),
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
fn decoded_cuts_distinguish_token_boundaries_and_match_interiors() {
    let block = overlapping_match_block(10);
    let blocks = [block];
    let composite = Composite::new(&blocks).unwrap();

    assert_eq!(composite.cut_at_plain(0), Some(Cut { token: 0, plain: 0 }));
    assert_eq!(composite.cut_at_plain(1), Some(Cut { token: 1, plain: 1 }));
    assert_eq!(composite.cut_at_plain(4), Some(Cut { token: 1, plain: 4 }));
    assert_eq!(
        composite.cut_at_plain(11),
        Some(Cut {
            token: 2,
            plain: 11,
        })
    );
    assert!(composite.cut_at_plain(12).is_none());
    assert!(composite.cut_is_token_boundary(Cut { token: 1, plain: 1 }));
    assert!(!composite.cut_is_token_boundary(Cut { token: 1, plain: 4 }));
}

#[test]
fn every_inside_match_cut_uses_canonical_same_distance_fragments() {
    let block = overlapping_match_block(258);
    let blocks = [block];
    let composite = Composite::new(&blocks).unwrap();
    let start = Cut { token: 0, plain: 0 };
    let end = Cut {
        token: 2,
        plain: 259,
    };

    for offset in 1..258_usize {
        let cut = composite.cut_at_plain(1 + offset).expect("an interior cut");
        let left = composite.materialize_range_tokens(start, cut).unwrap();
        let right = composite.materialize_range_tokens(cut, end).unwrap();
        let decoded_len: usize = left
            .iter()
            .chain(right.iter())
            .map(|token| token.decoded_len())
            .sum();
        assert_eq!(decoded_len, 259, "offset {offset}");

        for token in left.iter().chain(right.iter()) {
            match *token {
                Token::Literal(value) => assert_eq!(value, b'a', "offset {offset}"),
                Token::Match {
                    length,
                    distance,
                    length_symbol,
                    distance_symbol,
                } => {
                    assert_eq!(distance, 1, "offset {offset}");
                    assert_eq!(distance_symbol, 0, "offset {offset}");
                    assert_eq!(
                        length_symbol,
                        canonical_length_encoding(length).unwrap().0,
                        "offset {offset}"
                    );
                }
            }
        }

        let left_fragment = offset;
        if left_fragment < 3 {
            assert!(left[1..]
                .iter()
                .all(|token| matches!(token, Token::Literal(b'a'))));
        } else {
            assert!(matches!(
                left.last(),
                Some(Token::Match { length, .. }) if usize::from(*length) == left_fragment
            ));
        }
        let right_fragment = 258 - offset;
        if right_fragment < 3 {
            assert!(right
                .iter()
                .all(|token| matches!(token, Token::Literal(b'a'))));
        } else {
            assert!(matches!(
                right.as_slice(),
                [Token::Match { length, .. }] if usize::from(*length) == right_fragment
            ));
        }
    }
}

#[test]
fn same_match_middle_ranges_materialize_literals_or_one_match() {
    let block = overlapping_match_block(20);
    let blocks = [block];
    let composite = Composite::new(&blocks).unwrap();
    let start = composite.cut_at_plain(5).unwrap();

    for (length, expected_tokens) in [(1, 1), (2, 2), (3, 1), (11, 1)] {
        let end = composite.cut_at_plain(5 + length).unwrap();
        let tokens = composite.materialize_range_tokens(start, end).unwrap();
        assert_eq!(tokens.len(), expected_tokens);
        assert_eq!(
            tokens
                .iter()
                .map(|token| token.decoded_len())
                .sum::<usize>(),
            length
        );
        if length < 3 {
            assert!(tokens
                .iter()
                .all(|token| matches!(token, Token::Literal(b'a'))));
        } else {
            assert!(matches!(
                tokens.as_slice(),
                [Token::Match {
                    length: match_length,
                    distance: 1,
                    ..
                }] if usize::from(*match_length) == length
            ));
        }
    }
}

#[test]
fn an_inside_overlap_cut_emits_two_valid_deflate_blocks() {
    let block = overlapping_match_block(258);
    let blocks = [block];
    let composite = Composite::new(&blocks).unwrap();
    let boundaries = [
        Cut { token: 0, plain: 0 },
        composite.cut_at_plain(100).unwrap(),
        Cut {
            token: 2,
            plain: 259,
        },
    ];
    let options = Options::default();
    let mut plan_cache = CanonicalPlanCache::new();
    let plans = plan_structural_ranges(
        &composite,
        &boundaries,
        0,
        &options,
        &mut plan_cache,
        &mut SearchStop::never(),
    )
    .unwrap();
    assert_eq!(plans.len(), 2);

    let mut writer = BitWriter::default();
    for (index, plan) in plans.iter().enumerate() {
        emit_block(&mut writer, &[], plan, index + 1 == plans.len()).unwrap();
    }
    assert_eq!(writer.bit_position(), total_bits(&plans));
    let encoded = writer.into_bytes();
    let reparsed = parse_stream(&encoded, 259).unwrap();
    assert_eq!(reparsed.decoded_size, 259);
    assert!(reparsed
        .blocks
        .iter()
        .flat_map(|block| block.plain.iter())
        .all(|&byte| byte == b'a'));
}

#[test]
fn inside_cuts_preserve_a_nonuniform_overlapping_distance() {
    let block = periodic_distance_three_block();
    let expected = Arc::clone(&block.plain);
    let blocks = [block];
    let composite = Composite::new(&blocks).unwrap();
    let boundaries = [
        Cut { token: 0, plain: 0 },
        composite.cut_at_plain(4).unwrap(),
        composite.cut_at_plain(259).unwrap(),
        Cut {
            token: 4,
            plain: 261,
        },
    ];
    let mut plan_cache = CanonicalPlanCache::new();
    let plans = plan_structural_ranges(
        &composite,
        &boundaries,
        0,
        &Options::default(),
        &mut plan_cache,
        &mut SearchStop::never(),
    )
    .unwrap();
    assert_eq!(plans.len(), 3);

    let mut writer = BitWriter::default();
    for (index, plan) in plans.iter().enumerate() {
        emit_block(&mut writer, &[], plan, index + 1 == plans.len()).unwrap();
    }
    assert_eq!(writer.bit_position(), total_bits(&plans));
    let encoded = writer.into_bytes();
    let reparsed = parse_stream(&encoded, expected.len() as u64).unwrap();
    let decoded: Vec<_> = reparsed
        .blocks
        .iter()
        .flat_map(|block| block.plain.iter().copied())
        .collect();
    assert_eq!(decoded, expected.as_slice());
}

#[test]
fn partial_multi_source_ranges_keep_relative_source_splits() {
    let left = overlapping_match_block(20);
    let (length_symbol, _, _) = canonical_length_encoding(10).unwrap();
    let right_tokens = vec![
        Token::Match {
            length: 10,
            distance: 1,
            length_symbol,
            distance_symbol: 0,
        },
        Token::Literal(b'a'),
    ];
    let (literal_frequencies, distance_frequencies) = count_frequencies(&right_tokens);
    let right = ParsedBlock {
        tokens: right_tokens.into(),
        plain: vec![b'a'; 11].into(),
        literal_frequencies,
        distance_frequencies,
        original_literal_lengths: None,
        original_distance_lengths: None,
        original_dynamic: None,
        original: None,
        source_splits: Vec::new(),
        source_type: SourceBlockType::Dynamic,
    };
    let blocks = [left, right];
    let composite = Composite::new(&blocks).unwrap();
    let left_inside = composite.cut_at_plain(10).unwrap();
    let right_inside = composite.cut_at_plain(25).unwrap();
    let stream_end = Cut {
        token: 4,
        plain: 32,
    };

    assert_eq!(
        overlapping_source_range(&composite, right_inside, stream_end),
        1..2
    );
    assert_eq!(exact_source(&composite, right_inside, stream_end), None);
    assert!(make_range(&composite, right_inside, stream_end)
        .unwrap()
        .source_splits
        .is_empty());

    assert_eq!(
        overlapping_source_range(&composite, left_inside, right_inside),
        0..2
    );
    let cross_source = make_range(&composite, left_inside, right_inside).unwrap();
    assert_eq!(cross_source.source_splits, [11]);
    assert_eq!(cross_source.plain.len(), 15);
}

#[test]
fn range_materialization_projects_inherited_source_splits() {
    let mut block = literal_block(b"abcdefghijkl", SourceBlockType::Dynamic);
    block.source_splits = vec![4, 9];
    let blocks = [block];
    let composite = Composite::new(&blocks).unwrap();
    let range = make_range(
        &composite,
        Cut { token: 2, plain: 2 },
        Cut {
            token: 12,
            plain: 12,
        },
    )
    .unwrap();

    assert_eq!(range.source_splits, [2, 7]);
}

#[test]
fn strided_range_histograms_match_direct_recounting() {
    let tokens: Vec<_> = (0..700)
        .map(|index| {
            if index % 7 == 0 {
                Token::Match {
                    length: 11,
                    distance: 5,
                    length_symbol: 265,
                    distance_symbol: 4,
                }
            } else {
                Token::Literal((index % 251) as u8)
            }
        })
        .collect();
    let plain_len = tokens.iter().map(|token| token.decoded_len()).sum();
    let (literal_frequencies, distance_frequencies) = count_frequencies(&tokens);
    let block = ParsedBlock {
        tokens: tokens.into(),
        plain: vec![0; plain_len].into(),
        literal_frequencies,
        distance_frequencies,
        original_literal_lengths: None,
        original_distance_lengths: None,
        original_dynamic: None,
        original: None,
        source_splits: Vec::new(),
        source_type: SourceBlockType::Dynamic,
    };
    let blocks = [block];
    let mut composite = Composite::new(&blocks).unwrap();

    for (start, end) in [
        (0, 0),
        (0, 1),
        (1, 50),
        (10, 20),
        (0, 255),
        (0, 256),
        (0, 257),
        (17, 257),
        (249, 251),
        (254, 256),
        (255, 256),
        (255, 257),
        (256, 257),
        (250, 520),
        (256, 512),
        (255, 511),
        (500, 511),
        (511, 513),
        (512, 512),
        (257, 513),
        (257, 699),
        (699, 700),
        (700, 700),
        (0, 700),
    ] {
        let indexed = composite.range_frequencies(start, end).unwrap();
        let direct = count_frequencies(&composite.tokens[start..end]);
        assert_eq!(indexed.literal, direct.0, "{start}..{end}");
        assert_eq!(indexed.distance, direct.1, "{start}..{end}");
        assert_eq!(
            indexed.extra_bits,
            token_extra_bits(&composite.tokens[start..end]),
            "{start}..{end}"
        );
    }
    assert!(composite.range_frequencies(2, 1).is_none());
    assert!(composite.range_frequencies(0, 701).is_none());
    assert!(composite.range_frequencies(701, 701).is_none());

    // Without the checkpoint index (dropped near the model memory limit),
    // range counts fall back to a direct scan.
    composite.frequency_checkpoints = None;
    let fallback = composite.range_frequencies(257, 699).unwrap();
    let direct = count_frequencies(&composite.tokens[257..699]);
    assert_eq!(fallback.literal, direct.0);
    assert_eq!(fallback.distance, direct.1);
    assert_eq!(
        fallback.extra_bits,
        token_extra_bits(&composite.tokens[257..699])
    );
}

#[test]
fn entropy_state_anchors_can_only_improve_the_exact_boundary_graph() {
    let mut bytes = vec![b'a'; 700];
    bytes.extend(std::iter::repeat(b'b').take(700));
    bytes.extend(std::iter::repeat(b'c').take(700));
    let blocks = [literal_block(&bytes, SourceBlockType::Dynamic)];
    let composite = Composite::new(&blocks).unwrap();
    let options = Options {
        exhaustive: true,
        ..Options::default()
    };
    let legacy_cuts = choose_cuts(&composite, true, true).unwrap();
    let mut augmented_cuts = legacy_cuts.clone();
    for cut in entropy_state_boundary_cuts(&composite, true, &mut SearchStop::never()).unwrap() {
        push_optional_cut(&mut augmented_cuts, &composite, cut).unwrap();
    }
    augmented_cuts.sort_unstable_by_key(|cut| cut.plain);

    let mut legacy_cache = CanonicalPlanCache::new();
    let legacy = boundary_dp(
        &blocks,
        &composite,
        &legacy_cuts,
        0,
        &options,
        true,
        &mut legacy_cache,
        &mut SearchStop::never(),
        None,
    )
    .unwrap();
    let mut augmented_cache = CanonicalPlanCache::new();
    let augmented = boundary_dp(
        &blocks,
        &composite,
        &augmented_cuts,
        0,
        &options,
        true,
        &mut augmented_cache,
        &mut SearchStop::never(),
        None,
    )
    .unwrap();

    assert!(
        total_bits(&augmented) < total_bits(&legacy),
        "the learned off-grid anchors should improve {} legacy bits, got {}",
        total_bits(&legacy),
        total_bits(&augmented)
    );
    let decoded: Vec<_> = augmented
        .iter()
        .flat_map(|plan| plan.plain.iter().copied())
        .collect();
    assert_eq!(decoded, bytes);
}

#[test]
fn redundant_empty_blocks_are_removed() {
    let empty = literal_block(&[], SourceBlockType::Fixed);
    let content = literal_block(b"one more thing", SourceBlockType::Fixed);
    let prepared = prepare_blocks(&[empty.clone(), content.clone(), empty]).unwrap();
    assert_eq!(prepared.len(), 1);
    assert_eq!(prepared[0].plain, content.plain);
}

#[test]
fn an_empty_stream_keeps_one_legal_block() {
    let empty = literal_block(&[], SourceBlockType::Fixed);
    let prepared = prepare_blocks(&[empty.clone(), empty]).unwrap();
    assert_eq!(prepared.len(), 1);
    assert!(prepared[0].plain.is_empty());
}

#[test]
fn adjacent_stored_chunks_are_accumulated() {
    let left = literal_block(&vec![1; 20_000], SourceBlockType::Stored);
    let right = literal_block(&vec![2; 30_000], SourceBlockType::Stored);
    let prepared = prepare_blocks(&[left, right]).unwrap();
    assert_eq!(prepared.len(), 1);
    assert_eq!(prepared[0].plain.len(), 50_000);
    assert_eq!(prepared[0].source_splits, [20_000]);
}

#[test]
fn ordinary_planning_shares_unchanged_payload_buffers() {
    let source = literal_block(b"immutable payload", SourceBlockType::Dynamic);
    assert!(prepare_blocks(std::slice::from_ref(&source)).is_none());

    let plan = plan_block(&source, 0, &Options::default(), &mut SearchStop::never());
    assert!(Arc::ptr_eq(&plan.tokens, &source.tokens));
    assert!(Arc::ptr_eq(&plan.plain, &source.plain));
}

#[test]
fn mandatory_floor_reuse_matches_independent_candidate_order() {
    let block = short_match_block();
    let options = Options {
        exhaustive: true,
        ..Options::default()
    };
    let mut expected_cache = CanonicalPlanCache::new();
    let floor = plan_block_with_floor_cached(&block, 3, &options, true, &mut expected_cache);
    let short = plan_block_with_short_family_floor_cached(&block, 3, &options, &mut expected_cache);
    let expected = if short.bits < floor.bits {
        short
    } else {
        floor
    };

    let mut actual_cache = CanonicalPlanCache::new();
    let actual =
        mandatory_token_floor_plan(std::slice::from_ref(&block), 3, &options, &mut actual_cache)
            .expect("one valid block always has a complete floor")
            .pop()
            .expect("one source block produces one plan");
    assert_same_plan(&actual, &expected);
}

#[test]
fn stored_accumulation_respects_the_wire_limit() {
    let left = literal_block(&vec![1; 40_000], SourceBlockType::Stored);
    let right = literal_block(&vec![2; 30_000], SourceBlockType::Stored);
    assert!(prepare_blocks(&[left, right]).is_none());
}

#[test]
fn stored_only_floor_repacks_without_copying_literal_tokens() {
    let blocks: Vec<_> = (0..5)
        .map(|value| literal_block(&vec![value; 16_000], SourceBlockType::Stored))
        .collect();
    let plans = repack_all_stored_blocks(&blocks, 0).unwrap();

    assert_eq!(plans.len(), 2);
    assert_eq!(plans[0].plain.len(), 65_535);
    assert_eq!(plans[1].plain.len(), 14_465);
    assert!(plans.iter().all(|plan| plan.tokens.is_empty()));
    let decoded: Vec<_> = plans
        .iter()
        .flat_map(|plan| plan.plain.iter().copied())
        .collect();
    let source: Vec<_> = blocks
        .iter()
        .flat_map(|block| block.plain.iter().copied())
        .collect();
    assert_eq!(decoded, source);

    let source_bits: u64 = blocks
        .iter()
        .map(|block| stored_block_bits(0, block.plain.len()))
        .sum();
    assert!(total_bits(&plans) < source_bits);

    // Repacking needs no Huffman or token search, so it still runs after a
    // file-wide search budget is spent.
    let relaxed = Options {
        strict: false,
        ..Options::default()
    };
    let after_deadline = plan_stream(&blocks, 0, 0, &relaxed, &mut SearchStop::always()).unwrap();
    assert_eq!(total_bits(&after_deadline), total_bits(&plans));
    assert_eq!(after_deadline.len(), plans.len());
}

#[test]
fn canonical_cache_reuses_source_intervals_across_routes() {
    let blocks = [
        literal_block(&vec![b'a'; 800], SourceBlockType::Dynamic),
        literal_block(&vec![b'b'; 800], SourceBlockType::Dynamic),
        literal_block(&vec![b'c'; 800], SourceBlockType::Dynamic),
    ];
    let options = Options::default();
    let mut plan_cache = CanonicalPlanCache::new();

    source_aligned_huffman_floor(&blocks, 0, &options, &mut plan_cache).unwrap();
    let after_source_aligned = plan_cache.stats();
    let mandatory = mandatory_token_floor_plan(&blocks, 0, &options, &mut plan_cache).unwrap();
    let after_mandatory = plan_cache.stats();

    assert_eq!(
        after_mandatory.hits - after_source_aligned.hits,
        blocks.len()
    );
    assert_eq!(after_mandatory.inserts, after_source_aligned.inserts);
    let decoded: Vec<_> = mandatory
        .iter()
        .flat_map(|plan| plan.plain.iter().copied())
        .collect();
    assert_eq!(
        decoded,
        blocks
            .iter()
            .flat_map(|block| block.plain.iter().copied())
            .collect::<Vec<_>>()
    );
}

#[test]
fn fixed_joins_reuse_owned_payloads_and_preserve_shared_sources() {
    for shared in [false, true] {
        let left = plan_block(
            &literal_block(b"a", SourceBlockType::Fixed),
            0,
            &Options::default(),
            &mut SearchStop::never(),
        );
        let source = shared.then(|| left.clone());
        let mut output_bits = left.bits;
        let mut output = vec![left];
        let right = plan_block(
            &literal_block(b"b", SourceBlockType::Dynamic),
            0,
            &Options::default(),
            &mut SearchStop::never(),
        );
        assert!(is_fixed_plan(&right));
        let right_tokens = Arc::clone(&right.tokens);
        let right_plain = Arc::clone(&right.plain);

        for count in 1..=128 {
            let tokens = Arc::as_ptr(&output[0].tokens);
            let plain = Arc::as_ptr(&output[0].plain);
            append_output_plan(&mut output, &mut output_bits, right.clone(), true).unwrap();
            assert_eq!(output.len(), 1);
            assert_eq!(output_bits, 10 + 8 * (count + 1));
            assert_eq!(output_bits, output[0].bits);
            assert_eq!(output[0].source_type, SourceBlockType::Dynamic);
            assert!(matches!(output[0].representation, Representation::Fixed));
            if shared && count == 1 {
                assert_ne!(tokens, Arc::as_ptr(&output[0].tokens));
                assert_ne!(plain, Arc::as_ptr(&output[0].plain));
            } else {
                // Keep the uniquely owned vectors through growth. An extra
                // Arc clone here would force a full payload copy on every join.
                assert_eq!(tokens, Arc::as_ptr(&output[0].tokens));
                assert_eq!(plain, Arc::as_ptr(&output[0].plain));
            }
        }
        assert_eq!(right_tokens.as_slice(), &[Token::Literal(b'b')]);
        assert_eq!(right_plain.as_slice(), b"b");
        if let Some(source) = source {
            assert_eq!(source.tokens.as_slice(), &[Token::Literal(b'a')]);
            assert_eq!(source.plain.as_slice(), b"a");
        }

        let mut expected = vec![b'b'; 129];
        expected[0] = b'a';
        assert_eq!(output[0].plain.as_slice(), expected);
        let mut writer = BitWriter::default();
        emit_block(&mut writer, &[], &output[0], true).unwrap();
        assert_eq!(writer.bit_position(), output_bits);
        let parsed = parse_stream(&writer.into_bytes(), expected.len() as u64).unwrap();
        assert_eq!(parsed.blocks[0].plain.as_slice(), expected);
    }
}

#[test]
fn fixed_join_does_not_shift_a_later_stored_plan() {
    let options = Options::default();
    let left_block = literal_block(b"a", SourceBlockType::Fixed);
    let right_block = literal_block(b"b", SourceBlockType::Fixed);
    let left = plan_block(&left_block, 0, &options, &mut SearchStop::never());
    let right = plan_block(
        &right_block,
        (left.bits & 7) as u8,
        &options,
        &mut SearchStop::never(),
    );
    assert!(is_fixed_plan(&left));
    assert!(is_fixed_plan(&right));

    let stored_alignment = ((left.bits + right.bits) & 7) as u8;
    let stored = PlannedBlock {
        tokens: vec![Token::Literal(b'c')].into(),
        plain: vec![b'c'].into(),
        representation: Representation::Stored,
        bits: stored_block_bits(stored_alignment, 1),
        source_type: SourceBlockType::Stored,
    };
    let expected_bits = left.bits + right.bits + stored.bits;
    let mut output_bits = left.bits;
    let mut output = vec![left];
    append_output_plans(&mut output, &mut output_bits, vec![right, stored]).unwrap();

    // Joining the two fixed blocks would save ten bits, but it would also
    // move the stored block away from the alignment used to price its
    // padding. A later copied stored block could then be emitted invalidly.
    assert_eq!(output.len(), 3);
    assert_eq!(output_bits, expected_bits);

    let mut writer = BitWriter::default();
    for (index, plan) in output.iter().enumerate() {
        emit_block(&mut writer, &[], plan, index + 1 == output.len()).unwrap();
    }
    assert_eq!(writer.bit_position(), output_bits);
    let encoded = writer.into_bytes();
    let reparsed = parse_stream(&encoded, 3).unwrap();
    assert_eq!(reparsed.decoded_size, 3);
}

#[test]
fn collected_floor_is_not_hidden_by_an_unrelated_strong_adjacent_win() {
    let options = Options::default();
    let mut source_start = 0_u64;
    let blocks: Vec<_> = (0..9)
        .map(|index| {
            let value = if index & 1 == 0 { b'a' } else { b'b' };
            let mut block = literal_block(&[value; 100], SourceBlockType::Dynamic);
            // Expensive source encodings make the adjacent route a
            // >512-byte win. That saving is independent of the alternating
            // run's better collect-first grouping.
            block.original = Some(OriginalBits {
                start: source_start,
                len: 16_000,
                alignment: (source_start & 7) as u8,
                block_type: SourceBlockType::Dynamic,
            });
            source_start += 16_000;
            block
        })
        .collect();

    let mut plan_cache = CanonicalPlanCache::new();
    let adjacent = sequential_plan(
        &blocks,
        0,
        &options,
        AdjacentMergeSearch::LongRun,
        &mut plan_cache,
        &mut SearchStop::never(),
        None,
    )
    .unwrap();
    assert!(
        encoded_source_bytes(&blocks, 18_000)
            > total_bits(&adjacent).div_ceil(8).saturating_add(512)
    );
    let collected_blocks = collect_huffman_runs(&blocks, false).unwrap();
    let collected = sequential_plan(
        &collected_blocks,
        0,
        &options,
        AdjacentMergeSearch::Disabled,
        &mut plan_cache,
        &mut SearchStop::never(),
        None,
    )
    .unwrap();
    assert!(total_bits(&collected) < total_bits(&adjacent));

    let planned = plan_stream(&blocks, 18_000, 0, &options, &mut SearchStop::never()).unwrap();
    assert_eq!(total_bits(&planned), total_bits(&collected));
}

#[test]
fn eighth_target_keeps_snap_and_adds_exact_inside_match_cut() {
    let mut block = literal_block(&[0; 128], SourceBlockType::Dynamic);
    block.tokens = vec![
        Token::Literal(0),
        Token::Match {
            length: 60,
            distance: 1,
            length_symbol: 276,
            distance_symbol: 0,
        },
    ]
    .into();
    Arc::make_mut(&mut block.tokens).extend((0..67).map(|_| Token::Literal(0)));
    block.recount_frequencies();
    let blocks = [block];
    let composite = Composite::new(&blocks).unwrap();
    let mut cuts = Vec::new();
    add_eighth_cuts(&mut cuts, &composite, 0, 69, 0, 128, true).unwrap();

    // The first target is decoded offset 16, inside the 60-byte match. Keep
    // the token boundary after the first literal and add the exact
    // inside-match cut.
    assert!(cuts.contains(&Cut { token: 1, plain: 1 }));
    assert!(cuts.contains(&Cut {
        token: 1,
        plain: 16,
    }));
}

#[test]
fn default_inside_match_search_obeys_its_token_work_budget() {
    let maximum_tokens = DEFAULT_INSIDE_MATCH_TOKEN_WORK / SOURCE_EIGHTH_SPLITS;
    assert!(include_inside_match_cuts(false, maximum_tokens));
    assert!(!include_inside_match_cuts(false, maximum_tokens + 1));
    assert!(include_inside_match_cuts(true, usize::MAX));
}

#[test]
fn eighth_target_keeps_the_strictly_preceding_exact_boundary() {
    let block = literal_block(&[0; 128], SourceBlockType::Dynamic);
    let blocks = [block];
    let composite = Composite::new(&blocks).unwrap();
    let mut cuts = Vec::new();
    add_eighth_cuts(&mut cuts, &composite, 0, 128, 0, 128, true).unwrap();

    // The probe treats decoded offset 16 as the inclusive end of the token
    // starting at offset 15, so the cut stays before that token.
    assert!(cuts.contains(&Cut {
        token: 15,
        plain: 15,
    }));
    assert!(!cuts.contains(&Cut {
        token: 16,
        plain: 16,
    }));
}

#[test]
fn combined_histograms_match_the_joined_token_stream() {
    let left = literal_block(b"left", SourceBlockType::Dynamic);
    let right = short_match_block();

    let merged = try_merge_parsed_blocks(&left, &right).unwrap();
    let expected = count_frequencies(&merged.tokens);
    assert_eq!(merged.literal_frequencies, expected.0);
    assert_eq!(merged.distance_frequencies, expected.1);

    let mut appended = left;
    assert!(try_append_parsed_block(&mut appended, &right));
    assert_eq!(appended.literal_frequencies, expected.0);
    assert_eq!(appended.distance_frequencies, expected.1);
}

#[test]
fn terminal_layout_work_scales_with_stream_volume() {
    assert_eq!(terminal_layout_work(&[]), MIN_TERMINAL_LAYOUT_WORK);
    let block = |tokens: usize, plain: usize| {
        let tokens: Vec<_> = vec![Token::Literal(0); tokens];
        let (literal_frequencies, distance_frequencies) = count_frequencies(&tokens);
        ParsedBlock {
            tokens: tokens.into(),
            plain: vec![0; plain].into(),
            literal_frequencies,
            distance_frequencies,
            original_literal_lengths: None,
            original_distance_lengths: None,
            original_dynamic: None,
            original: None,
            source_splits: Vec::new(),
            source_type: SourceBlockType::Dynamic,
        }
    };
    // Small streams keep the floor; large ones get 64 units per token and
    // decoded byte, summed over blocks.
    assert_eq!(
        terminal_layout_work(&[block(1_000, 4_000)]),
        MIN_TERMINAL_LAYOUT_WORK
    );
    let large = [block(3_000_000, 4_000_000), block(1_000_000, 1_000_000)];
    assert_eq!(terminal_layout_work(&large), 64 * 9_000_000);
}
