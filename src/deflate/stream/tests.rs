// SPDX-License-Identifier: MIT

use super::*;
use crate::deflate::bitstream::BitWriter;
use crate::deflate::block::{emit_block, stored_block_bits};
use crate::deflate::model::OriginalBits;
use crate::deflate::parse::parse_stream;

/// Build the ordinary per-block result, joining consecutive fixed output plans
/// as DeflOpt does. Removing the first block's seven-bit end code and the next
/// block's three-bit header saves exactly ten bits.
fn sequential_plan(
    blocks: &[ParsedBlock],
    start_alignment: u8,
    options: &Options,
    merge_search: AdjacentMergeSearch,
    plan_cache: &mut CanonicalPlanCache,
    stop: &mut SearchStop<'_>,
    progress: Option<&RouteProgress>,
) -> Option<Vec<PlannedBlock>> {
    sequential_plan_with_compact_policy(
        blocks,
        start_alignment,
        options,
        merge_search,
        false,
        plan_cache,
        stop,
        progress,
    )
}

/// Search kernel for `add_adaptive_split_cut`.
///
/// This independent Columbo implementation is substantially different from
/// Turtledeflate's `turtledeflate_best_block_split`; it is not a translation
/// or exact recreation.
fn coarse_to_fine_split<S>(
    start: usize,
    end: usize,
    score: &mut S,
    stop: &mut SearchStop<'_>,
) -> Option<AdaptiveSplit>
where
    S: FnMut(usize) -> Option<u64>,
{
    Some(coarse_to_fine_split_search(start, end, score, stop)?.best)
}

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

fn literal_block(bytes: &[u8], source_type: SourceBlockType) -> ParsedBlock {
    let tokens: Vec<_> = bytes.iter().copied().map(Token::Literal).collect();
    let (literal_frequencies, distance_frequencies) = count_frequencies(&tokens);
    ParsedBlock {
        tokens: tokens.into(),
        plain: bytes.to_vec().into(),
        literal_frequencies,
        distance_frequencies,
        original_literal_lengths: None,
        original_distance_lengths: None,
        original_dynamic: None,
        original: None,
        source_splits: Vec::new(),
        source_type,
    }
}

fn short_match_block() -> ParsedBlock {
    let mut tokens = vec![Token::Literal(b'a')];
    tokens.extend((0..100).map(|_| Token::Match {
        length: 6,
        distance: 1,
        length_symbol: 260,
        distance_symbol: 0,
        length_extra: 0,
        distance_extra: 0,
        length_extra_bits: 0,
        distance_extra_bits: 0,
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

fn overlapping_match_block(length: u16) -> ParsedBlock {
    let (length_symbol, length_extra, length_extra_bits) =
        canonical_length_encoding(length).expect("a legal Deflate match length");
    let tokens = vec![
        Token::Literal(b'a'),
        Token::Match {
            length,
            distance: 1,
            length_symbol,
            distance_symbol: 0,
            length_extra,
            distance_extra: 0,
            length_extra_bits,
            distance_extra_bits: 0,
        },
    ];
    let (literal_frequencies, distance_frequencies) = count_frequencies(&tokens);
    ParsedBlock {
        tokens: tokens.into(),
        plain: vec![b'a'; usize::from(length) + 1].into(),
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

fn seven_inside_eighths_block() -> ParsedBlock {
    let (length_symbol, length_extra, length_extra_bits) = canonical_length_encoding(15).unwrap();
    let mut tokens = vec![Token::Literal(b'a'), Token::Literal(b'a')];
    for match_index in 0..7 {
        tokens.push(Token::Match {
            length: 15,
            distance: 1,
            length_symbol,
            distance_symbol: 0,
            length_extra,
            distance_extra: 0,
            length_extra_bits,
            distance_extra_bits: 0,
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

fn periodic_distance_three_block() -> ParsedBlock {
    let (length_symbol, length_extra, length_extra_bits) = canonical_length_encoding(258).unwrap();
    let tokens = vec![
        Token::Literal(b'a'),
        Token::Literal(b'b'),
        Token::Literal(b'c'),
        Token::Match {
            length: 258,
            distance: 3,
            length_symbol,
            distance_symbol: 2,
            length_extra,
            distance_extra: 0,
            length_extra_bits,
            distance_extra_bits: 0,
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
                    length_extra,
                    distance_extra,
                    length_extra_bits,
                    distance_extra_bits,
                } => {
                    assert_eq!(distance, 1, "offset {offset}");
                    assert_eq!(distance_symbol, 0, "offset {offset}");
                    assert_eq!(distance_extra, 0, "offset {offset}");
                    assert_eq!(distance_extra_bits, 0, "offset {offset}");
                    assert_eq!(
                        (length_symbol, length_extra, length_extra_bits),
                        canonical_length_encoding(length).unwrap(),
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
fn partial_multi_source_ranges_keep_relative_source_splits() {
    let left = overlapping_match_block(20);
    let (length_symbol, length_extra, length_extra_bits) = canonical_length_encoding(10).unwrap();
    let right_tokens = vec![
        Token::Match {
            length: 10,
            distance: 1,
            length_symbol,
            distance_symbol: 0,
            length_extra,
            distance_extra: 0,
            length_extra_bits,
            distance_extra_bits: 0,
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
                    length_extra: 0,
                    distance_extra: 0,
                    length_extra_bits: 1,
                    distance_extra_bits: 1,
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
        (250, 520),
        (256, 512),
        (255, 511),
        (256, 512),
        (257, 513),
        (257, 699),
        (699, 700),
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

    // Near the optional-model ceiling, Columbo preserves the established
    // structural route without the index and falls back to a direct scan.
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
fn entropy_state_assignment_matches_an_exhaustive_oracle() {
    let left_tokens = [Token::Literal(b'a'); 32];
    let right_tokens = [Token::Literal(b'b'); 32];
    let mut left = EntropyScoutHistogram::zero();
    let mut right = EntropyScoutHistogram::zero();
    for token in left_tokens {
        left.add_token(token);
    }
    for token in right_tokens {
        right.add_token(token);
    }
    let codes = [
        EntropyScoutCode::from_histogram(&left, true),
        EntropyScoutCode::from_histogram(&right, true),
    ];
    let tokens = [
        Token::Literal(b'a'),
        Token::Literal(b'a'),
        Token::Literal(b'b'),
        Token::Literal(b'b'),
        Token::Literal(b'a'),
    ];
    let switch_bits = 2;
    let states =
        assign_entropy_states(&tokens, &codes, switch_bits, &mut SearchStop::never()).unwrap();
    let path_cost = |states: &[u8]| {
        let payload: u64 = tokens
            .iter()
            .zip(states)
            .map(|(&token, &state)| codes[usize::from(state)].token_bits(token))
            .sum();
        let switches = states.windows(2).filter(|pair| pair[0] != pair[1]).count() as u64;
        payload + switches * switch_bits
    };
    let oracle = (0_u8..1 << tokens.len())
        .map(|mask| {
            let candidate: Vec<_> = (0..tokens.len())
                .map(|position| (mask >> position) & 1)
                .collect();
            path_cost(&candidate)
        })
        .min()
        .unwrap();

    assert_eq!(path_cost(&states), oracle);
}

#[test]
fn entropy_state_scout_finds_multiple_off_grid_transitions_deterministically() {
    let mut bytes = vec![b'a'; 700];
    bytes.extend(std::iter::repeat(b'b').take(700));
    bytes.extend(std::iter::repeat(b'c').take(700));
    let block = literal_block(&bytes, SourceBlockType::Dynamic);
    let blocks = [block];
    let composite = Composite::new(&blocks).unwrap();

    let first = entropy_state_boundary_cuts(&composite, true, &mut SearchStop::never())
        .expect("the learned states expose both literal transitions");
    let second = entropy_state_boundary_cuts(&composite, true, &mut SearchStop::never())
        .expect("the scout is deterministic");

    assert_eq!(first, second);
    assert!(first.contains(&Cut {
        token: 700,
        plain: 700,
    }));
    assert!(first.contains(&Cut {
        token: 1_400,
        plain: 1_400,
    }));
}

#[test]
fn entropy_state_scout_is_bounded_and_skips_ineligible_work() {
    let small = literal_block(&vec![b'a'; 512], SourceBlockType::Dynamic);
    let small_blocks = [small];
    let small_composite = Composite::new(&small_blocks).unwrap();
    assert!(
        entropy_state_boundary_cuts(&small_composite, true, &mut SearchStop::never(),).is_none()
    );

    let stored = literal_block(&vec![b'a'; 1_024], SourceBlockType::Stored);
    let stored_blocks = [stored];
    let stored_composite = Composite::new(&stored_blocks).unwrap();
    assert!(
        entropy_state_boundary_cuts(&stored_composite, true, &mut SearchStop::never(),).is_none()
    );

    let eligible = literal_block(&vec![b'a'; 1_024], SourceBlockType::Dynamic);
    let eligible_blocks = [eligible];
    let eligible_composite = Composite::new(&eligible_blocks).unwrap();
    assert!(
        entropy_state_boundary_cuts(&eligible_composite, true, &mut SearchStop::always(),)
            .is_none()
    );

    let mut bytes = Vec::new();
    for chunk in 0..40 {
        bytes.extend(std::iter::repeat(if chunk % 2 == 0 { b'a' } else { b'b' }).take(32));
    }
    let block = literal_block(&bytes, SourceBlockType::Dynamic);
    let blocks = [block];
    let composite = Composite::new(&blocks).unwrap();
    let mut left = EntropyScoutHistogram::zero();
    let mut right = EntropyScoutHistogram::zero();
    for _ in 0..32 {
        left.add_token(Token::Literal(b'a'));
        right.add_token(Token::Literal(b'b'));
    }
    let codes = [
        EntropyScoutCode::from_histogram(&left, true),
        EntropyScoutCode::from_histogram(&right, true),
    ];
    let states: Vec<_> = (0..bytes.len())
        .map(|position| (position / 32 % 2) as u8)
        .collect();
    let cuts = ranked_entropy_state_cuts(&composite, &codes, &states).unwrap();
    assert_eq!(cuts.len(), ENTROPY_SCOUT_MAX_CUTS);
    assert!(cuts.windows(2).all(|pair| pair[0].plain < pair[1].plain));
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
fn coarse_to_fine_split_finds_an_off_grid_minimum_under_its_probe_cap() {
    let mut probes = 0_usize;
    let mut score = |token: usize| {
        probes += 1;
        let distance = token.abs_diff(733) as u64;
        Some(distance * distance)
    };
    let candidate =
        coarse_to_fine_split(0, 2_048, &mut score, &mut SearchStop::never()).expect("a legal cut");

    assert_eq!(candidate.token, 733);
    assert_eq!(candidate.bits, 0);
    assert!(probes <= ADAPTIVE_SPLIT_MAX_PROBES);
}

#[test]
fn coarse_to_fine_split_centres_flat_ties() {
    let candidate = coarse_to_fine_split(0, 2_048, &mut |_| Some(1), &mut SearchStop::never())
        .expect("a legal cut");

    assert_eq!(candidate.token, 1_024);
}

#[test]
fn coarse_to_fine_split_retains_one_well_separated_secondary_basin() {
    let mut score = |token: usize| {
        let primary = token.abs_diff(420) as u64;
        let secondary = token.abs_diff(1_600) as u64;
        Some((primary * primary).min(100 + secondary * secondary))
    };
    let search = coarse_to_fine_split_search(0, 2_048, &mut score, &mut SearchStop::never())
        .expect("two sampled basins");

    assert_eq!(search.best.token, 420);
    let secondary = search
        .secondary
        .expect("the distant local minimum is retained");
    assert!(secondary.token > 1_000);
    assert!(secondary.token.abs_diff(search.best.token) >= 2_048 / ADAPTIVE_SPLIT_INTERVALS);
}

#[test]
fn adaptive_histogram_cut_finds_a_literal_transition() {
    let mut bytes = vec![b'a'; 733];
    bytes.extend(std::iter::repeat(b'z').take(1_315));
    let block = literal_block(&bytes, SourceBlockType::Dynamic);
    let blocks = [block];
    let composite = Composite::new(&blocks).unwrap();
    let mut cuts = Vec::new();
    let start = Cut { token: 0, plain: 0 };
    let end = Cut {
        token: bytes.len(),
        plain: bytes.len(),
    };

    add_adaptive_split_cut(
        &mut cuts,
        &composite,
        start,
        end,
        false,
        &mut SearchStop::never(),
    )
    .unwrap();

    assert_eq!(
        cuts,
        [Cut {
            token: 733,
            plain: 733,
        }]
    );
}

#[test]
fn adaptive_split_skips_small_ranges_and_respects_an_expired_deadline() {
    let small = literal_block(&vec![b'x'; 512], SourceBlockType::Dynamic);
    let small_blocks = [small];
    let small_composite = Composite::new(&small_blocks).unwrap();
    let mut cuts = Vec::new();
    add_adaptive_split_cut(
        &mut cuts,
        &small_composite,
        Cut { token: 0, plain: 0 },
        Cut {
            token: 512,
            plain: 512,
        },
        false,
        &mut SearchStop::never(),
    )
    .unwrap();
    assert!(cuts.is_empty());

    let mut scorer_called = false;
    let candidate = coarse_to_fine_split(
        0,
        2_048,
        &mut |_| {
            scorer_called = true;
            Some(0)
        },
        &mut SearchStop::always(),
    );
    assert!(candidate.is_none());
    assert!(!scorer_called);
}

#[test]
fn adaptive_split_requires_a_material_exact_saving() {
    assert!(!adaptive_split_is_worth_replay(969, 1_000));
    assert!(adaptive_split_is_worth_replay(968, 1_000));
    assert!(!adaptive_split_is_worth_replay(u64::MAX, u64::MAX));
}

fn assert_same_plan(left: &PlannedBlock, right: &PlannedBlock) {
    assert_eq!(left.bits, right.bits);
    assert_eq!(left.tokens, right.tokens);
    assert_eq!(left.plain, right.plain);
    assert_eq!(left.source_type, right.source_type);
    match (&left.representation, &right.representation) {
        (Representation::Original(left), Representation::Original(right)) => {
            assert_eq!(left, right);
        }
        (Representation::Stored, Representation::Stored)
        | (Representation::Fixed, Representation::Fixed) => {}
        (Representation::Dynamic(left), Representation::Dynamic(right)) => {
            assert_eq!(left, right);
        }
        pair => panic!("different representations: {pair:?}"),
    }
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

    // Repacking does not consult Huffman tables or alter LZ77 tokens, so
    // it remains safe after a ZIP/APNG file-wide search budget is spent.
    let relaxed = Options {
        strict: false,
        ..Options::default()
    };
    let after_deadline = plan_stream(&blocks, 0, &relaxed, &mut SearchStop::always()).unwrap();
    assert_eq!(total_bits(&after_deadline), total_bits(&plans));
    assert_eq!(after_deadline.len(), plans.len());
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

    // Exhaustive mode admits regrouping regardless of encoded source size.
    // Once admitted at call entry, the complete structural floor remains
    // available even if the next deadline check expires optional search.
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
        &exhaustive,
        &mut SearchStop::callback(&mut expires),
    )
    .unwrap();
    assert_eq!(deadline_safe.len(), 1);
    assert_eq!(deadline_safe[0].plain.len(), 2_400);
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
fn floor_search_caches_only_the_bounded_header_policy() {
    let block = literal_block(&vec![b'a'; 800], SourceBlockType::Dynamic);
    let exhaustive = Options {
        exhaustive: true,
        ..Options::default()
    };
    let mut plan_cache = CanonicalPlanCache::new();
    let plans = plan_source_with_search(
        &block,
        0,
        &exhaustive,
        SourceBlockSearch::Floor,
        &mut plan_cache,
        &mut SearchStop::never(),
    );
    assert_eq!(plans.len(), 1);

    let bounded = Options::default();
    let before = plan_cache.stats();
    assert!(lookup_block_cached(&block, 0, &bounded, &mut plan_cache).is_some());
    let after = plan_cache.stats();
    assert_eq!(after.hits, before.hits + 1);
    assert_eq!(after.inserts, before.inserts);
}

#[test]
fn bounded_segmentation_keeps_a_later_combined_win() {
    let mut prices = vec![[None; MAX_BOUNDED_GROUP_SPAN + 1]; 4];
    for price in &mut prices {
        price[1] = Some(10);
    }
    // The largest immediate saving at source zero is the three-block
    // range (30 -> 18). Taking it would hide the much stronger range at
    // source two. The complete optimum is [0..2] + [2..4] = 12 bits.
    prices[0][2] = Some(11);
    prices[0][3] = Some(18);
    prices[2][2] = Some(1);

    let boundaries = choose_bounded_boundaries(&prices).unwrap();
    assert_eq!(boundaries, [2, 2, 4, 4]);
}

#[test]
fn parallel_bounded_grouping_matches_the_serial_candidate() {
    let blocks: Vec<_> = (0..20)
        .map(|_| {
            let mut tokens = vec![Token::Literal(b'a')];
            tokens.extend((0..100).map(|_| Token::Match {
                length: 11,
                distance: 1,
                length_symbol: 265,
                distance_symbol: 0,
                length_extra: 0,
                distance_extra: 0,
                length_extra_bits: 1,
                distance_extra_bits: 0,
            }));
            let (literal_frequencies, distance_frequencies) = count_frequencies(&tokens);
            ParsedBlock {
                tokens: tokens.into(),
                plain: vec![b'a'; 1_101].into(),
                literal_frequencies,
                distance_frequencies,
                original_literal_lengths: None,
                original_distance_lengths: None,
                original_dynamic: None,
                original: None,
                source_splits: Vec::new(),
                source_type: SourceBlockType::Dynamic,
            }
        })
        .collect();
    let options = Options::default();
    let mut serial_cache = CanonicalPlanCache::new();
    let serial = bounded_huffman_grouping(
        &blocks,
        &options,
        BoundedRangePricing::Serial,
        &mut serial_cache,
    )
    .unwrap();
    let mut parallel_cache = CanonicalPlanCache::new();
    let parallel = bounded_huffman_grouping(
        &blocks,
        &options,
        BoundedRangePricing::Parallel,
        &mut parallel_cache,
    )
    .unwrap();

    // The same-distance floor can make each synthetic source block as cheap as
    // its grouped form. This test requires identical serial and parallel
    // decisions, whether or not a merge remains profitable.
    assert_eq!(parallel.len(), serial.len());
    for (parallel, serial) in parallel.iter().zip(&serial) {
        assert_eq!(parallel.tokens, serial.tokens);
        assert_eq!(parallel.plain, serial.plain);
        assert_eq!(parallel.literal_frequencies, serial.literal_frequencies);
        assert_eq!(parallel.distance_frequencies, serial.distance_frequencies);
        assert_eq!(parallel.source_splits, serial.source_splits);
        assert_eq!(parallel.source_type, serial.source_type);
    }

    let serial_plans = finish_plan(
        direct_structural_plan(&serial, 0, &options, &mut serial_cache).unwrap(),
        &options,
    );
    let parallel_plans = plan_columbo_floor_seeded_bounded_grouping(&blocks, 0, &options).unwrap();
    assert_eq!(parallel_plans.len(), serial_plans.len());
    for (parallel, serial) in parallel_plans.iter().zip(&serial_plans) {
        assert_same_plan(parallel, serial);
    }
}

#[test]
fn long_huffman_collection_is_linear_and_bounded() {
    let blocks: Vec<_> = (0..10)
        .map(|value| literal_block(&vec![value; 1_000], SourceBlockType::Dynamic))
        .collect();
    let collected = collect_huffman_runs(&blocks, false).unwrap();

    assert_eq!(collected.len(), 2);
    assert_eq!(collected[0].tokens.len(), 8_000);
    assert_eq!(collected[0].plain.len(), 8_000);
    assert_eq!(collected[0].source_splits.len(), 7);
    assert_eq!(collected[1].tokens.len(), 2_000);
    let (literal, distance) = count_frequencies(&collected[0].tokens);
    assert_eq!(collected[0].literal_frequencies, literal);
    assert_eq!(collected[0].distance_frequencies, distance);
}

#[test]
fn fragmented_collection_uses_the_independent_4096_token_limit() {
    let mut source_start = 0_u64;
    let blocks: Vec<_> = (0..FRAGMENTED_COLLECT_MIN_SOURCE_BLOCKS)
        .map(|index| {
            let mut block = literal_block(&[index as u8; 100], SourceBlockType::Dynamic);
            block.original = Some(OriginalBits {
                start: source_start,
                len: 2_048,
                alignment: (source_start & 7) as u8,
                block_type: SourceBlockType::Dynamic,
            });
            source_start += 2_048;
            block
        })
        .collect();
    let options = Options {
        exhaustive: true,
        ..Options::default()
    };

    let seed = fragmented_collect_seed(&blocks, 0, &options).unwrap();
    assert_eq!(seed.len(), 2);
    assert_eq!(seed[0].tokens.len(), 4_000);
    assert_eq!(seed[1].tokens.len(), 2_400);
    let decoded: Vec<_> = seed
        .iter()
        .flat_map(|plan| plan.plain.iter().copied())
        .collect();
    let source: Vec<_> = blocks
        .iter()
        .flat_map(|block| block.plain.iter().copied())
        .collect();
    assert_eq!(decoded, source);
}

#[test]
fn fragmented_replay_is_bounded_and_preserves_decoded_bytes() {
    let blocks: Vec<_> = (0..4)
        .map(|_| literal_block(&vec![b'a'; 512], SourceBlockType::Dynamic))
        .collect();
    let mut plan_cache = CanonicalPlanCache::new();
    let mandatory =
        mandatory_token_floor_plan(&blocks, 0, &Options::default(), &mut plan_cache).unwrap();
    let plans = plan_fragmented_replay(&blocks, 0, &Options::default()).unwrap();

    assert_eq!(plans.len(), 1);
    assert!(total_bits(&plans) < total_bits(&mandatory));
    let decoded: Vec<_> = plans
        .iter()
        .flat_map(|plan| plan.plain.iter().copied())
        .collect();
    let source: Vec<_> = blocks
        .iter()
        .flat_map(|block| block.plain.iter().copied())
        .collect();
    assert_eq!(decoded, source);

    let too_many = vec![blocks[0].clone(); MAX_FRAGMENTED_REPLAY_BLOCKS + 1];
    assert!(plan_fragmented_replay(&too_many, 0, &Options::default()).is_none());
}

#[test]
fn wide_collection_remains_an_additive_candidate() {
    let blocks: Vec<_> = (0..10)
        .map(|value| literal_block(&vec![value; 1_000], SourceBlockType::Dynamic))
        .collect();
    let ordinary = collect_huffman_runs(&blocks, false).unwrap();
    let exhaustive = collect_huffman_runs(&blocks, true).unwrap();

    assert_eq!(ordinary.len(), 2);
    assert_eq!(exhaustive.len(), 1);
    let mut plan_cache = CanonicalPlanCache::new();
    let plans =
        direct_structural_plan(&exhaustive, 0, &Options::default(), &mut plan_cache).unwrap();
    assert_eq!(plans.len(), 1);
}

#[test]
fn very_long_flush_runs_can_use_the_bounded_wide_collection() {
    let blocks: Vec<_> = (0..WIDE_COLLECT_MIN_SOURCE_BLOCKS)
        .map(|value| literal_block(&[value as u8; 100], SourceBlockType::Dynamic))
        .collect();
    let collected = collect_huffman_runs(&blocks, true).unwrap();

    assert_eq!(collected.len(), 1);
    assert_eq!(collected[0].tokens.len(), 12_800);
    assert_eq!(collected[0].plain.len(), 12_800);
}

#[test]
fn stored_block_breaks_huffman_collection() {
    let left = literal_block(&vec![1; 1_000], SourceBlockType::Dynamic);
    let stored = literal_block(&vec![2; 1_000], SourceBlockType::Stored);
    let right = literal_block(&vec![3; 1_000], SourceBlockType::Dynamic);

    let collected = collect_huffman_runs(&[left, stored, right], false).unwrap();
    assert_eq!(collected.len(), 3);
    assert_eq!(collected[1].source_type, SourceBlockType::Stored);
}

#[test]
fn consecutive_fixed_plans_remove_ten_bits() {
    let options = Options::default();
    let blocks = [
        literal_block(b"a", SourceBlockType::Fixed),
        literal_block(b"b", SourceBlockType::Fixed),
    ];
    let separate_left = plan_block(&blocks[0], 0, &options, &mut SearchStop::never());
    let separate_right = plan_block(&blocks[1], 0, &options, &mut SearchStop::never());
    assert!(is_fixed_plan(&separate_left));
    assert!(is_fixed_plan(&separate_right));

    let mut plan_cache = CanonicalPlanCache::new();
    let plans = sequential_plan(
        &blocks,
        0,
        &options,
        AdjacentMergeSearch::Local,
        &mut plan_cache,
        &mut SearchStop::never(),
        None,
    )
    .unwrap();
    assert_eq!(plans.len(), 1);
    assert!(matches!(plans[0].representation, Representation::Fixed));
    assert_eq!(plans[0].bits, separate_left.bits + separate_right.bits - 10);
    assert_eq!(plans[0].plain.as_slice(), b"ab");
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
            // Deliberately expensive source serializations make the
            // adjacent route a >512-byte win. That saving is independent of
            // the alternating run's better collect-first grouping.
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
    assert!(encoded_source_bytes(&blocks) > total_bits(&adjacent).div_ceil(8).saturating_add(512));
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

    let planned = plan_stream(&blocks, 0, &options, &mut SearchStop::never()).unwrap();
    assert_eq!(total_bits(&planned), total_bits(&collected));
}

#[test]
fn long_run_search_rebuilds_profitable_dynamic_neighbours() {
    let options = Options::default();
    let blocks = [
        literal_block(&vec![b'a'; 1_000], SourceBlockType::Dynamic),
        literal_block(&vec![b'a'; 1_000], SourceBlockType::Dynamic),
    ];
    let mut plan_cache = CanonicalPlanCache::new();
    let separate = sequential_plan(
        &blocks,
        0,
        &options,
        AdjacentMergeSearch::Disabled,
        &mut plan_cache,
        &mut SearchStop::never(),
        None,
    )
    .unwrap();
    let merged = sequential_plan(
        &blocks,
        0,
        &options,
        AdjacentMergeSearch::LongRun,
        &mut plan_cache,
        &mut SearchStop::never(),
        None,
    )
    .unwrap();

    assert!(total_bits(&merged) < total_bits(&separate));
    assert_eq!(merged.len(), 1);
    assert_eq!(merged[0].plain.len(), 2_000);
}

#[test]
fn direct_no_split_route_is_additive_and_preserves_decoded_bytes() {
    let options = Options {
        exhaustive: true,
        ..Options::default()
    };
    let blocks = [
        literal_block(&vec![b'a'; 800], SourceBlockType::Dynamic),
        literal_block(&vec![b'a'; 800], SourceBlockType::Dynamic),
        literal_block(&vec![b'a'; 800], SourceBlockType::Dynamic),
    ];
    let mut plan_cache = CanonicalPlanCache::new();
    let fallback = direct_structural_plan(&blocks, 0, &options, &mut plan_cache).unwrap();
    let route = plan_source_no_split_route(&blocks, 0, &options, &mut SearchStop::never())
        .expect("the direct route retains a complete fallback");

    assert!(total_bits(&route) <= total_bits(&fallback));
    let decoded: Vec<_> = route
        .iter()
        .flat_map(|plan| plan.plain.iter().copied())
        .collect();
    assert_eq!(decoded, vec![b'a'; 2_400]);
}

#[test]
fn compact_split_coarse_to_fine_never_loses_the_unlimited_exact_route() {
    let options = Options {
        exhaustive: true,
        ..Options::default()
    };
    let mut first = vec![b'a'; 512];
    first.extend(std::iter::repeat(b'b').take(512));
    let mut second = vec![b'c'; 512];
    second.extend(std::iter::repeat(b'd').take(512));
    let blocks = [
        literal_block(&first, SourceBlockType::Dynamic),
        literal_block(&second, SourceBlockType::Dynamic),
    ];

    let exact = plan_compact_source_split_floor(&blocks, 0, &options).unwrap();
    let timed =
        plan_compact_source_split_floor_until(&blocks, 0, &options, &mut SearchStop::never())
            .unwrap();
    assert!(total_bits(&timed) <= total_bits(&exact));

    let stopped =
        plan_compact_source_split_floor_until(&blocks, 0, &options, &mut SearchStop::always())
            .expect("an expired route still forwards a complete structural parent");
    let decoded: Vec<_> = stopped
        .iter()
        .flat_map(|plan| plan.plain.iter().copied())
        .collect();
    assert_eq!(decoded, [first, second].concat());
}

#[test]
fn expired_compact_split_rescue_keeps_a_complete_parent_or_better() {
    let options = Options {
        exhaustive: true,
        ..Options::default()
    };
    let mut plain = vec![b'a'; 8_192];
    plain.extend((0..8_192).map(|index| (index % 251) as u8));
    let blocks = [literal_block(&plain, SourceBlockType::Dynamic)];
    let mut plan_cache = CanonicalPlanCache::new();
    let fallback = direct_structural_plan(&blocks, 0, &options, &mut plan_cache).unwrap();
    let rescued =
        plan_compact_source_split_floor_until(&blocks, 0, &options, &mut SearchStop::always())
            .expect("the bounded rescue retains a complete parent");

    assert!(total_bits(&rescued) <= total_bits(&fallback));
    let decoded: Vec<_> = rescued
        .iter()
        .flat_map(|plan| plan.plain.iter().copied())
        .collect();
    assert_eq!(decoded, plain);
}

#[test]
fn terminal_merge_returns_only_a_strict_complete_stream_win() {
    let options = Options::default();
    let blocks = [
        literal_block(&vec![b'a'; 800], SourceBlockType::Dynamic),
        literal_block(&vec![b'a'; 800], SourceBlockType::Dynamic),
    ];
    let mut plan_cache = CanonicalPlanCache::new();
    let fallback = direct_structural_plan(&blocks, 0, &options, &mut plan_cache).unwrap();
    let merged = plan_terminal_merge_route(&blocks, 0, &options, &mut SearchStop::never())
        .expect("equal neighbours have a profitable deterministic merge");

    assert!(total_bits(&merged) < total_bits(&fallback));
    assert_eq!(merged.len(), 1);
    assert_eq!(merged[0].plain.as_slice(), &[b'a'; 1_600]);

    assert!(plan_terminal_merge_route(&blocks, 0, &options, &mut SearchStop::always()).is_none());
}

#[test]
fn selected_plan_merge_cleanup_reuses_finished_token_spellings() {
    let options = Options::default();
    let blocks = [
        literal_block(&vec![b'a'; 800], SourceBlockType::Dynamic),
        literal_block(&vec![b'a'; 800], SourceBlockType::Dynamic),
    ];
    let mut plan_cache = CanonicalPlanCache::new();
    let selected = direct_structural_plan(&blocks, 0, &options, &mut plan_cache).unwrap();
    let merged = plan_selected_huffman_merge_floor(&selected, 0, &options, &mut plan_cache)
        .expect("the selected token spellings admit a cheaper adjacent merge");

    assert!(total_bits(&merged) < total_bits(&selected));
    assert_eq!(merged.len(), 1);
    assert_eq!(merged[0].plain.as_slice(), &[b'a'; 1_600]);
}

#[test]
fn selected_plan_boundary_reseat_finds_a_better_transition() {
    let options = Options {
        exhaustive: true,
        ..Options::default()
    };
    let mut left_bytes = vec![b'a'; 733];
    left_bytes.extend(std::iter::repeat(b'z').take(291));
    let blocks = [
        literal_block(&left_bytes, SourceBlockType::Dynamic),
        literal_block(&vec![b'z'; 1_024], SourceBlockType::Dynamic),
    ];
    let mut plan_cache = CanonicalPlanCache::new();
    let selected = direct_structural_plan(&blocks, 0, &options, &mut plan_cache).unwrap();
    let reseated = plan_selected_huffman_boundary_reseat(
        &selected,
        0,
        &options,
        &mut plan_cache,
        &mut SearchStop::never(),
    )
    .expect("the literal transition is a cheaper boundary");

    assert!(total_bits(&reseated) < total_bits(&selected));
    assert_eq!(reseated.len(), 2);
    assert_eq!(reseated[0].plain.len(), 733);
    let decoded: Vec<_> = reseated
        .iter()
        .flat_map(|plan| plan.plain.iter().copied())
        .collect();
    assert_eq!(decoded, [vec![b'a'; 733], vec![b'z'; 1_315]].concat());

    assert!(plan_selected_huffman_boundary_reseat(
        &selected,
        0,
        &Options::default(),
        &mut plan_cache,
        &mut SearchStop::never(),
    )
    .is_none());
    assert!(plan_selected_huffman_boundary_reseat(
        &selected,
        0,
        &options,
        &mut plan_cache,
        &mut SearchStop::always(),
    )
    .is_none());
}

#[test]
fn forced_split_escape_is_max_only_and_deadline_bounded() {
    let blocks = [
        literal_block(&vec![b'a'; 1_024], SourceBlockType::Dynamic),
        literal_block(&vec![b'z'; 1_024], SourceBlockType::Dynamic),
    ];
    let mut plan_cache = CanonicalPlanCache::new();
    let selected =
        direct_structural_plan(&blocks, 0, &Options::default(), &mut plan_cache).unwrap();

    assert!(plan_forced_split_boundary_escape(
        &selected,
        0,
        &Options::default(),
        &mut plan_cache,
        &mut SearchStop::never(),
    )
    .is_none());
    assert!(plan_forced_split_boundary_escape(
        &selected,
        0,
        &Options {
            exhaustive: true,
            ..Options::default()
        },
        &mut plan_cache,
        &mut SearchStop::always(),
    )
    .is_none());
}

#[test]
fn long_huffman_route_does_not_join_large_stored_neighbours() {
    let options = Options::default();
    let blocks = [
        literal_block(&vec![b'a'; 1_000], SourceBlockType::Stored),
        literal_block(&vec![b'a'; 1_000], SourceBlockType::Stored),
    ];
    let mut plan_cache = CanonicalPlanCache::new();
    let separate = sequential_plan(
        &blocks,
        0,
        &options,
        AdjacentMergeSearch::Disabled,
        &mut plan_cache,
        &mut SearchStop::never(),
        None,
    )
    .unwrap();
    let long_route = sequential_plan(
        &blocks,
        0,
        &options,
        AdjacentMergeSearch::LongRun,
        &mut plan_cache,
        &mut SearchStop::never(),
        None,
    )
    .unwrap();

    assert_eq!(total_bits(&long_route), total_bits(&separate));
    assert_eq!(long_route.len(), separate.len());
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
            length_extra: 1,
            distance_extra: 0,
            length_extra_bits: 4,
            distance_extra_bits: 0,
        },
    ]
    .into();
    Arc::make_mut(&mut block.tokens).extend((0..67).map(|_| Token::Literal(0)));
    block.recount_frequencies();
    let blocks = [block];
    let composite = Composite::new(&blocks).unwrap();
    let mut cuts = Vec::new();
    add_eighth_cuts(&mut cuts, &composite, 0, 69, 0, 128, true).unwrap();

    // The first target is decoded offset 16, inside the 60-byte match.
    // Retain the legacy boundary after the first literal and add an exact
    // sibling that materializes only the selected edge fragments.
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

fn boundary_priority_block(run_count: usize) -> ParsedBlock {
    let (length_symbol, length_extra, length_extra_bits) = canonical_length_encoding(257).unwrap();
    let mut tokens = Vec::new();
    for _ in 0..run_count {
        tokens.push(Token::Literal(b'a'));
        tokens.extend((0..200).map(|_| Token::Match {
            length: 257,
            distance: 1,
            length_symbol,
            distance_symbol: 0,
            length_extra,
            distance_extra: 0,
            length_extra_bits,
            distance_extra_bits: 0,
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
fn split_first_route_requires_measured_structural_dominance() {
    assert!(!search_split_before_whole_block(true, None, 1_000,));
    assert!(!search_split_before_whole_block(false, Some(999), 1_000,));
    assert!(search_split_before_whole_block(true, Some(999), 1_000,));
    assert!(!search_split_before_whole_block(true, Some(1_000), 1_000,));
}

#[test]
fn eighth_target_keeps_the_strictly_preceding_exact_boundary() {
    let block = literal_block(&[0; 128], SourceBlockType::Dynamic);
    let blocks = [block];
    let composite = Composite::new(&blocks).unwrap();
    let mut cuts = Vec::new();
    add_eighth_cuts(&mut cuts, &composite, 0, 128, 0, 128, true).unwrap();

    // The original Columbo C probe treats decoded offset 16 as the
    // inclusive end of the token beginning at offset 15, so the split
    // stays before it.
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
fn midpoint_refinement_keeps_snap_and_adds_exact_inside_match_cut() {
    let mut block = literal_block(&[0; 128], SourceBlockType::Dynamic);
    block.tokens = Arc::new((0..10).map(|_| Token::Literal(0)).collect());
    Arc::make_mut(&mut block.tokens).push(Token::Match {
        length: 100,
        distance: 1,
        length_symbol: 279,
        distance_symbol: 0,
        length_extra: 1,
        distance_extra: 0,
        length_extra_bits: 4,
        distance_extra_bits: 0,
    });
    Arc::make_mut(&mut block.tokens).extend((0..18).map(|_| Token::Literal(0)));
    block.recount_frequencies();

    let blocks = [block];
    let composite = Composite::new(&blocks).unwrap();
    let midpoints = midpoint_cuts(
        &composite,
        Cut { token: 0, plain: 0 },
        Cut {
            token: 29,
            plain: 128,
        },
    );

    // Decoded offset 64 lies inside the 100-byte match. Keep the prior
    // token boundary and add the exact proven-submatch sibling.
    assert_eq!(
        midpoints,
        [
            Some(Cut {
                token: 10,
                plain: 10,
            }),
            Some(Cut {
                token: 10,
                plain: 64,
            }),
        ]
    );
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

    // This boundary is deliberately near the end. Scoring all compact
    // probes before deep whole-block searches keeps it reachable under a
    // finite max-mode deadline.
    assert!(cuts.contains(&Cut {
        token: 320,
        plain: 320,
    }));
}

#[test]
fn identical_dynamic_trees_survive_a_merge() {
    let shared = DynamicPlan {
        literal_lengths: vec![1; 257],
        distance_lengths: vec![1],
        code_length_lengths: [0; 19],
        rle: Vec::new(),
        hlit: 257,
        hdist: 1,
        hclen: 4,
        bits: 100,
    };
    let mut left = literal_block(b"left", SourceBlockType::Dynamic);
    let mut right = literal_block(b"right", SourceBlockType::Dynamic);
    left.original_dynamic = Some(shared.clone());
    right.original_dynamic = Some(shared);

    assert!(blocks_share_dynamic_tree(&left, &right));
    let merged = try_merge_parsed_blocks(&left, &right).unwrap();
    assert!(merged.original_dynamic.is_some());
    assert_eq!(merged.plain.as_slice(), b"leftright");
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
