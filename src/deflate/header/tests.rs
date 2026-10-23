// SPDX-License-Identifier: MIT
// SPDX-FileCopyrightText: 2026 Andrew C.E. Dent <https://github.com/ace-dent>

use super::test_support::{literal_span_test_block, payload_tradeoff_test_block};
use super::*;
use crate::deflate::huffman::{huffman_tree_shape_is_complete, make_lengths, Huffman};
use crate::deflate::model::count_frequencies;

fn best_dynamic_plan(
    tokens: &[Token],
    literal_frequencies: &[u32; 286],
    distance_frequencies: &[u32; 30],
    original: Option<&DynamicPlan>,
    strict: bool,
    exhaustive: bool,
    stop: &mut SearchStop<'_>,
) -> Option<DynamicPlan> {
    best_dynamic_plan_cached(
        tokens,
        literal_frequencies,
        distance_frequencies,
        original,
        strict,
        exhaustive,
        stop,
        &mut HeaderPlanCache::new(),
    )
}

#[test]
fn literal_span_padding_saves_a_bit_without_changing_payload_codes() {
    let block = literal_span_test_block();
    let parent = block.original_dynamic.as_ref().unwrap();
    let result = plan_literal_span(&block, true, &mut 1024, &mut SearchStop::never()).unwrap();
    assert_eq!(parent.hlit, 269);
    assert_eq!((result.hlit, result.bits), (277, 1293));
    assert_eq!(block.original.unwrap().len, 1294);
    assert_eq!(
        &result.literal_lengths[..parent.hlit],
        parent.literal_lengths
    );
    assert!(result.literal_lengths[parent.hlit..]
        .iter()
        .all(|&n| n == 0));
    assert_eq!(result.distance_lengths, parent.distance_lengths);
    assert!(result.has_strictly_compatible_huffman_codes());
    let data_bits = token_bits(
        &block.tokens,
        &parent.literal_lengths,
        &parent.distance_lengths,
    )
    .unwrap();
    assert_eq!(
        token_bits(
            &block.tokens,
            &result.literal_lengths,
            &result.distance_lengths
        ),
        Some(data_bits)
    );
    let control = plan_for_trimmed_lengths_uncached(
        &parent.literal_lengths,
        &parent.distance_lengths,
        data_bits,
        true,
        0xff,
    )
    .unwrap();
    assert_eq!(control.bits, 1294);
    // Longer zero runs alone are not enough: this saving also needs a
    // different code-length code, so a search with that code fixed misses it.
    let lengths: Vec<_> = result
        .literal_lengths
        .iter()
        .chain(&result.distance_lengths)
        .copied()
        .collect();
    if let Some(rle) = shortest_rle(&lengths, &parent.code_length_lengths) {
        let mut frozen = result.clone();
        frozen.code_length_lengths = parent.code_length_lengths;
        frozen.hclen = parent.hclen;
        frozen.rle = rle;
        assert!(dynamic_bits(data_bits, &frozen).unwrap() >= 1294);
    }
}

#[test]
fn literal_span_search_covers_shorter_and_longer_advertised_counts() {
    let base = literal_span_test_block();
    let original = base.original_dynamic.as_ref().unwrap();
    let data_bits = token_bits(
        &base.tokens,
        &original.literal_lengths,
        &original.distance_lengths,
    )
    .unwrap();
    for advertised in [269, 275, 286] {
        let mut block = base.clone();
        let mut literal = original.literal_lengths.clone();
        literal.resize(advertised, 0);
        let mut distance = original.distance_lengths.clone();
        distance.resize(32, 0); // Keep even a non-minimal distance span fixed.
        let parent =
            plan_for_trimmed_lengths_uncached(&literal, &distance, data_bits, true, 0xff).unwrap();
        let parent_bits = parent.bits;
        block.original_dynamic = Some(parent);
        let mut oracle = parent_bits;
        literal.resize(286, 0);
        for count in 269..=286 {
            oracle = oracle.min(
                plan_for_trimmed_lengths_uncached(
                    &literal[..count],
                    &distance,
                    data_bits,
                    true,
                    0xff,
                )
                .unwrap()
                .bits,
            );
        }
        let result = plan_literal_span(&block, true, &mut 1024, &mut SearchStop::never());
        assert_eq!(result.as_ref().map_or(parent_bits, |p| p.bits), oracle);
        if let Some(result) = result {
            assert_eq!(result.distance_lengths, distance);
            assert_eq!(&result.literal_lengths[..269], original.literal_lengths);
            assert!(result.has_strictly_compatible_huffman_codes());
        }
    }
}

#[test]
fn literal_span_search_respects_limits_and_retains_finished_prices() {
    let block = literal_span_test_block();
    assert!(plan_literal_span(&block, true, &mut 0, &mut SearchStop::never()).is_none());
    assert!(plan_literal_span(&block, true, &mut 1024, &mut SearchStop::always()).is_none());
    let mut one = 1;
    assert!(plan_literal_span(&block, true, &mut one, &mut SearchStop::never()).is_none());
    assert_eq!(one, 0);
    // The first eight prices reach 277 literal/length codes. Running out of
    // prices keeps that complete winner and tries no later count.
    let mut eight = 8;
    let result = plan_literal_span(&block, true, &mut eight, &mut SearchStop::never()).unwrap();
    assert_eq!((result.hlit, result.bits, eight), (277, 1293, 0));
    let mut polls = 0;
    let mut stop = || {
        polls += 1;
        polls > 10
    };
    let result = plan_literal_span(
        &block,
        true,
        &mut 1024,
        &mut SearchStop::callback(&mut stop),
    )
    .unwrap();
    assert_eq!((result.hlit, result.bits), (277, 1293));
    // A used symbol 285 leaves no legal larger count. Keep all payload
    // trees complete so this exercises the format bound, not rejection.
    let mut full = block.clone();
    let mut literal = [0_u8; 286];
    literal[0] = 1;
    literal[256] = 2;
    literal[285] = 2;
    full.tokens = vec![Token::Literal(0)].into();
    full.recount_frequencies();
    full.original_dynamic =
        Some(plan_for_explicit_lengths(&full.tokens, &literal, &[1, 1], true).unwrap());
    let mut budget = 1024;
    assert!(plan_literal_span(&full, true, &mut budget, &mut SearchStop::never()).is_none());
    assert_eq!(budget, 1024);
}

#[test]
fn positive_payload_swap_saves_more_in_the_header() {
    let block = payload_tradeoff_test_block();
    let parent = block.original_dynamic.as_ref().unwrap();
    let mut budget = 1024;
    let result =
        plan_payload_header_tradeoff(&block, true, &mut budget, &mut SearchStop::never()).unwrap();
    let payload = |plan: &DynamicPlan| {
        token_bits(&block.tokens, &plan.literal_lengths, &plan.distance_lengths).unwrap()
    };
    assert_eq!((parent.bits, payload(parent)), (582, 441));
    assert_eq!((result.bits, payload(&result)), (581, 444));
    assert!(result.has_strictly_compatible_huffman_codes());
    assert_eq!(parent.distance_lengths, result.distance_lengths);
    let mut old_lengths = parent.literal_lengths.clone();
    let mut new_lengths = result.literal_lengths.clone();
    old_lengths.sort_unstable();
    new_lengths.sort_unstable();
    assert_eq!(old_lengths, new_lengths);
    for (old, new) in parent.literal_lengths.iter().zip(&result.literal_lengths) {
        assert_eq!(*old == 0, *new == 0);
    }
}

#[test]
fn payload_swap_search_respects_stop_and_shared_price_budget() {
    let block = payload_tradeoff_test_block();
    assert!(plan_payload_header_tradeoff(&block, true, &mut 0, &mut SearchStop::never()).is_none());
    assert!(
        plan_payload_header_tradeoff(&block, true, &mut 1024, &mut SearchStop::always()).is_none()
    );
    // One price covers only the unchanged-tree repack, which ties the fully
    // priced parent, so no swap result may be returned.
    let mut budget = 1;
    assert!(
        plan_payload_header_tradeoff(&block, true, &mut budget, &mut SearchStop::never()).is_none()
    );
    assert_eq!(budget, 0);
    let mut polls = 0;
    let mut callback = || {
        polls += 1;
        polls > 4
    };
    assert!(plan_payload_header_tradeoff(
        &block,
        true,
        &mut 1024,
        &mut SearchStop::callback(&mut callback)
    )
    .is_none());
    assert!(polls > 4);
}

#[test]
fn local_swap_transition_delta_matches_full_sequence_oracle() {
    // Exhaustive short sequences include zero and nonzero runs, adjacent
    // swaps, endpoints and positions on either side of an alphabet boundary.
    for mut code in 0..3_usize.pow(6) {
        let mut lengths = [0_u8; 6];
        for length in &mut lengths {
            *length = (code % 3) as u8;
            code /= 3;
        }
        let transitions = |v: &[u8]| v.windows(2).filter(|p| p[0] != p[1]).count() as i64;
        for a in 0..lengths.len() {
            for b in a + 1..lengths.len() {
                let mut swapped = lengths;
                swapped.swap(a, b);
                assert_eq!(
                    swap_transitions_removed(&lengths, a, b),
                    transitions(&lengths) - transitions(&swapped)
                );
            }
        }
    }
}

/// Produce a dense, irregular frequency table without carrying a large
/// opaque array literal in the regression test below.
fn reduced_depth_fixture(mut state: u32) -> ([u32; 286], [u32; 30]) {
    let mut next = || {
        state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        state
    };
    let mut literal_frequencies = [0_u32; 286];
    for frequency in &mut literal_frequencies[..256] {
        let sample = next();
        *frequency = if sample % 13 == 0 { 0 } else { 1 + sample % 32 };
    }
    literal_frequencies[0] = 100 + next() % 4_000;
    literal_frequencies[256] = 1;
    for frequency in &mut literal_frequencies[257..283] {
        let sample = next();
        *frequency = if sample % 5 != 0 { 1 + sample % 128 } else { 0 };
    }
    let mut distance_frequencies = [0_u32; 30];
    for frequency in &mut distance_frequencies[..25] {
        let sample = next();
        *frequency = if sample % 6 != 0 { 1 + sample % 64 } else { 0 };
    }
    (literal_frequencies, distance_frequencies)
}

#[test]
fn minimum_complete_depth_follows_prefix_code_capacity() {
    assert_eq!(minimum_complete_tree_depth(&[]), 1);
    assert_eq!(minimum_complete_tree_depth(&[1]), 1);
    assert_eq!(minimum_complete_tree_depth(&[1, 1]), 1);
    assert_eq!(minimum_complete_tree_depth(&[1, 1, 1]), 2);
    assert_eq!(minimum_complete_tree_depth(&[1; 256]), 8);
    assert_eq!(minimum_complete_tree_depth(&[1; 257]), 9);
}

#[test]
fn bounded_depth_frontier_matches_every_feasible_restricted_ceiling() {
    let (literal_frequencies, distance_frequencies) = reduced_depth_fixture(0x9e37_79b9);
    let actual = plan_bounded_depth_tree_candidate(
        &[],
        &literal_frequencies,
        &distance_frequencies,
        false,
        false,
        false,
        &mut SearchStop::never(),
    )
    .unwrap();
    let minimum_depth = minimum_complete_tree_depth(&literal_frequencies)
        .max(minimum_complete_tree_depth(&distance_frequencies));
    let mut cache = HeaderPlanCache::new();
    let mut expected = None;
    for max_bits in minimum_depth..=MAX_RESTRICTED_PAYLOAD_TREE_DEPTH {
        let literal_defluff = make_lengths_defluff_exact(&literal_frequencies, max_bits, 0);
        let distance_defluff = make_lengths_defluff_exact(&distance_frequencies, max_bits, 0);
        let literal_candidates = [
            literal_defluff.clone(),
            make_lengths_zopfli_package_from(&literal_frequencies, &literal_defluff, max_bits),
        ];
        let distance_candidates = [
            distance_defluff.clone(),
            make_lengths_zopfli_package_from(&distance_frequencies, &distance_defluff, max_bits),
        ];
        for literal_lengths in &literal_candidates {
            for distance_lengths in &distance_candidates {
                if let Some(candidate) = explicit_exact_tree_candidate(
                    &literal_frequencies,
                    &distance_frequencies,
                    literal_lengths,
                    distance_lengths,
                    0,
                    false,
                    &mut cache,
                ) {
                    keep_better(&mut expected, candidate);
                }
            }
        }
    }
    assert_eq!(actual, expected.unwrap());
}

#[test]
fn independent_depth_frontier_matches_the_exact_alphabet_cross_product() {
    let (literal_frequencies, distance_frequencies) = reduced_depth_fixture(0xee81_ca19);
    let actual = plan_bounded_depth_tree_candidate(
        &[],
        &literal_frequencies,
        &distance_frequencies,
        false,
        true,
        true,
        &mut SearchStop::never(),
    )
    .unwrap();

    let mut build_literal_frequencies = literal_frequencies;
    ensure_code_symbols(&mut build_literal_frequencies, false);
    let mut build_distance_frequencies = distance_frequencies;
    ensure_distance_symbols(&mut build_distance_frequencies, false);
    let mut literal_candidates = Vec::new();
    for max_bits in
        minimum_complete_tree_depth(&build_literal_frequencies)..=MAX_RESTRICTED_PAYLOAD_TREE_DEPTH
    {
        let defluff = make_lengths_defluff_exact(&build_literal_frequencies, max_bits, 0);
        let package_first =
            make_lengths_zopfli_package_from(&build_literal_frequencies, &defluff, max_bits);
        push_unique(&mut literal_candidates, defluff);
        push_unique(&mut literal_candidates, package_first);
    }
    let mut distance_candidates = Vec::new();
    for max_bits in
        minimum_complete_tree_depth(&build_distance_frequencies)..=MAX_RESTRICTED_PAYLOAD_TREE_DEPTH
    {
        let defluff = make_lengths_defluff_exact(&build_distance_frequencies, max_bits, 0);
        let package_first =
            make_lengths_zopfli_package_from(&build_distance_frequencies, &defluff, max_bits);
        push_unique(&mut distance_candidates, defluff);
        push_unique(&mut distance_candidates, package_first);
    }

    let mut cache = HeaderPlanCache::new();
    let mut expected = None;
    for literal_lengths in &literal_candidates {
        for distance_lengths in &distance_candidates {
            if let Some(candidate) = explicit_exact_tree_candidate(
                &literal_frequencies,
                &distance_frequencies,
                literal_lengths,
                distance_lengths,
                0,
                true,
                &mut cache,
            ) {
                keep_better(&mut expected, candidate);
            }
        }
    }
    assert_eq!(actual, expected.unwrap());
}

#[test]
fn independent_depth_frontier_returns_its_best_complete_plan_on_stop() {
    let (literal_frequencies, distance_frequencies) = reduced_depth_fixture(0xee81_ca19);
    let complete = plan_bounded_depth_tree_candidate(
        &[],
        &literal_frequencies,
        &distance_frequencies,
        false,
        true,
        true,
        &mut SearchStop::never(),
    )
    .unwrap();

    let mut polls = 0;
    let mut stop_after_two = || {
        polls += 1;
        polls > 2
    };
    let partial = plan_bounded_depth_tree_candidate(
        &[],
        &literal_frequencies,
        &distance_frequencies,
        false,
        true,
        true,
        &mut SearchStop::callback(&mut stop_after_two),
    )
    .unwrap();

    assert_eq!(polls, 3);
    assert!(partial.bits >= complete.bits);
    assert!(plan_bounded_depth_tree_candidate(
        &[],
        &literal_frequencies,
        &distance_frequencies,
        false,
        true,
        true,
        &mut SearchStop::always(),
    )
    .is_none());
}

#[test]
fn reduced_depth_candidates_beat_full_depth_tree_families() {
    for (seed, max_bits, expected_bits) in [
        (0x9e37_79b9_u32, 10_u8, (48_667, 48_693)),
        (0xee81_ca19, 9, (46_568, 46_579)),
    ] {
        let (literal_frequencies, distance_frequencies) = reduced_depth_fixture(seed);
        let mut cache = HeaderPlanCache::new();
        let reduced = exact_tree_candidate(
            &literal_frequencies,
            &distance_frequencies,
            &literal_frequencies,
            &distance_frequencies,
            0,
            max_bits,
            true,
            &mut cache,
        )
        .unwrap();

        let mut ordinary = None;
        let literal_candidates = tree_candidates(&literal_frequencies, 15, true);
        let distance_candidates = tree_candidates(&distance_frequencies, 15, true);
        for literal in &literal_candidates {
            for distance in &distance_candidates {
                let Some(data_bits) = token_bits_from_frequencies(
                    &literal_frequencies,
                    &distance_frequencies,
                    literal,
                    distance,
                    0,
                ) else {
                    continue;
                };
                if let Some(candidate) = plan_for_explicit_lengths_with_cost_cached(
                    literal, distance, data_bits, true, &mut cache,
                ) {
                    keep_better(&mut ordinary, candidate);
                }
            }
        }
        let ordinary = ordinary.unwrap();
        assert_eq!((reduced.bits, ordinary.bits), expected_bits);
        assert!(reduced.bits < ordinary.bits);
        assert!(reduced
            .literal_lengths
            .iter()
            .all(|&length| length <= max_bits));
        assert!(reduced
            .distance_lengths
            .iter()
            .all(|&length| length <= max_bits));
        assert!(payload_tree_shape_is_valid(&reduced.literal_lengths, false));
        assert!(payload_tree_shape_is_valid(&reduced.distance_lengths, true));
    }
}

#[test]
fn header_plan_cache_reuses_only_the_header_kernel() {
    let mut literal_lengths = [0_u8; 257];
    literal_lengths[0] = 1;
    literal_lengths[256] = 1;
    let distance_lengths = [1_u8];
    let mut cache = HeaderPlanCache::with_limit(2);

    let first = plan_for_explicit_lengths_with_cost_cached(
        &literal_lengths,
        &distance_lengths,
        10,
        false,
        &mut cache,
    )
    .expect("the first header kernel is valid");
    let second = plan_for_explicit_lengths_with_cost_cached(
        &literal_lengths,
        &distance_lengths,
        25,
        false,
        &mut cache,
    )
    .expect("the cached header kernel is valid");

    assert_eq!(second.bits, first.bits + 15);
    assert_eq!(
        cache.stats(),
        HeaderPlanCacheStats {
            lookups: 2,
            hits: 1,
            misses: 1,
            inserts: 1,
            collision_checks: 1,
            saturated: 0,
        }
    );

    let exhaustive = plan_for_explicit_lengths_with_cost_cached(
        &literal_lengths,
        &distance_lengths,
        25,
        true,
        &mut cache,
    )
    .expect("header policy produces an independent valid kernel");
    assert_eq!(exhaustive.bits, second.bits);
    assert_eq!(cache.stats().hits, 1);
    assert_eq!(cache.stats().misses, 2);
    assert_eq!(cache.stats().inserts, 2);
}

#[test]
fn saturated_header_cache_returns_exact_plans_without_retaining_misses() {
    let mut literal = [0_u8; 257];
    literal[0] = 1;
    literal[256] = 1;
    let distance = [1_u8];
    for limit in [0, 1] {
        let mut cache = HeaderPlanCache::with_limit(limit);
        for data_bits in [0, 10, 25] {
            let actual = cache.price(&literal, &distance, data_bits, false, 0xff);
            let expected =
                plan_for_trimmed_lengths_uncached(&literal, &distance, data_bits, false, 0xff);
            assert_eq!(actual, expected);
        }
        // An exhaustive miss has a different cache key even if its plan ties.
        let actual = cache.price(&literal, &distance, 17, true, 0xff);
        let expected = plan_for_trimmed_lengths_uncached(&literal, &distance, 17, true, 0xff);
        assert_eq!(actual, expected);
        assert_eq!(cache.entries.len(), limit);
        assert_eq!(cache.stats().inserts, limit);
        assert_eq!(cache.stats().hits, 2 * limit);
        assert_eq!(cache.stats().saturated, 4 - 3 * limit);
        let before = cache.stats();
        assert!(cache
            .price(&literal, &distance, u64::MAX, true, 0xff)
            .is_none());
        assert_eq!(cache.stats().inserts, before.inserts);
        assert_eq!(cache.stats().saturated, before.saturated);
    }
}

#[test]
fn header_plan_cache_verifies_lengths_after_a_hash_collision() {
    let mut first_literal = [0_u8; 257];
    first_literal[0] = 1;
    first_literal[256] = 1;
    let mut second_literal = [0_u8; 257];
    second_literal[0] = 2;
    second_literal[1] = 2;
    second_literal[2] = 2;
    second_literal[256] = 2;
    let distance = [1_u8];
    let mut cache = HeaderPlanCache::new();
    plan_for_explicit_lengths_with_cost_cached(&first_literal, &distance, 10, false, &mut cache)
        .expect("the first header kernel is valid");

    let fingerprint = header_plan_fingerprint(&second_literal, &distance, false, 0xff);
    cache.entries[0].fingerprint = fingerprint;
    cache.first_by_hash.clear();
    cache.first_by_hash.insert(fingerprint, 0);
    let second = plan_for_explicit_lengths_with_cost_cached(
        &second_literal,
        &distance,
        20,
        false,
        &mut cache,
    )
    .expect("the colliding header kernel is independently planned");

    assert_eq!(second.literal_lengths, second_literal);
    assert_eq!(cache.entries.len(), 2);
    assert_eq!(cache.entries[1].next_same_hash, Some(0));
    assert_eq!(cache.stats().hits, 0);
    assert_eq!(cache.stats().misses, 2);
    assert_eq!(cache.stats().collision_checks, 1);
}

#[test]
fn zopfli_rle_candidate_is_distinct_and_scores_original_counts() {
    let mut literal_frequencies = [0_u32; 286];
    literal_frequencies[..40].copy_from_slice(&[
        24, 23, 22, 21, 18, 17, 16, 19, 32, 31, 34, 33, 10, 13, 12, 11, 36, 35, 34, 33, 6, 5, 4, 7,
        4, 3, 6, 5, 22, 25, 24, 23, 16, 15, 14, 13, 26, 25, 24, 27,
    ]);
    literal_frequencies[256] = 1;
    let mut distance_frequencies = [0_u32; 30];
    distance_frequencies[..8].copy_from_slice(&[12, 11, 14, 13, 6, 9, 8, 7]);

    let zopfli =
        zopfli_rle_tree_candidate(&literal_frequencies, &distance_frequencies, 0, true, true)
            .expect("Zopfli smoothing changes this tree");
    let columbo =
        columbo_rle_tree_candidate(&literal_frequencies, &distance_frequencies, 0, true, true)
            .expect("Columbo quantization changes this tree");
    assert_eq!(zopfli.bits, 4_366);
    assert_eq!(columbo.bits, 4_376);

    let data_bits = token_bits_from_frequencies(
        &literal_frequencies,
        &distance_frequencies,
        &zopfli.literal_lengths,
        &zopfli.distance_lengths,
        0,
    )
    .expect("candidate covers every source symbol");
    let exactly_priced = plan_for_explicit_lengths_with_cost(
        &zopfli.literal_lengths,
        &zopfli.distance_lengths,
        data_bits,
        true,
    )
    .expect("candidate trees form a valid dynamic block");
    assert_eq!(zopfli, exactly_priced);
}

fn enumerate_rle_paths(
    lengths: &[u8],
    index: usize,
    current: &mut Vec<RleToken>,
    output: &mut Vec<Vec<RleToken>>,
) {
    if index == lengths.len() {
        output.push(current.clone());
        return;
    }
    let mut run = 1;
    while index + run < lengths.len() && lengths[index + run] == lengths[index] {
        run += 1;
    }
    let mut visit = |count: usize, symbol: u8, extra: u8| {
        current.push(RleToken { symbol, extra });
        enumerate_rle_paths(lengths, index + count, current, output);
        current.pop();
    };

    visit(1, lengths[index], 0);
    if index > 0 && lengths[index] == lengths[index - 1] {
        for count in 3..=run.min(6) {
            visit(count, 16, (count - 3) as u8);
        }
    }
    if lengths[index] == 0 {
        for count in 3..=run.min(10) {
            visit(count, 17, (count - 3) as u8);
        }
        for count in 11..=run.min(138) {
            visit(count, 18, (count - 11) as u8);
        }
    }
}

fn exhaustive_shortest_rle(lengths: &[u8], costs: &[u8; 19]) -> Option<Vec<RleToken>> {
    let mut spellings = Vec::new();
    enumerate_rle_paths(lengths, 0, &mut Vec::new(), &mut spellings);
    spellings.retain(|rle| {
        rle.iter()
            .all(|token| costs[usize::from(token.symbol)] != 0)
    });
    spellings.sort_by_key(|rle| rle_cost(rle, costs));
    spellings.into_iter().next()
}

#[test]
fn token_bits_counts_huffman_codes_and_match_extras_together() {
    let tokens = [
        Token::Literal(b'A'),
        Token::Match {
            length: 12,
            distance: 5,
            length_symbol: 265,
            distance_symbol: 4,
            length_extra: 1,
            distance_extra: 0,
            length_extra_bits: 1,
            distance_extra_bits: 1,
        },
    ];
    let mut literal_lengths = [0_u8; 286];
    literal_lengths[usize::from(b'A')] = 8;
    literal_lengths[256] = 7;
    literal_lengths[265] = 7;
    let mut distance_lengths = [0_u8; 30];
    distance_lengths[4] = 5;

    // literal + length code + distance code + both extras + end code
    assert_eq!(
        token_bits(&tokens, &literal_lengths, &distance_lengths),
        Some(8 + 7 + 5 + 1 + 1 + 7)
    );
}

#[test]
fn shortest_rle_matches_exhaustive_short_sequence_oracle() {
    let mut cost_sets = Vec::new();
    let mut balanced = [3_u8; 19];
    balanced[16] = 2;
    balanced[17] = 3;
    balanced[18] = 4;
    cost_sets.push(balanced);
    let mut literal_friendly = [5_u8; 19];
    literal_friendly[0] = 1;
    literal_friendly[3] = 1;
    literal_friendly[16] = 7;
    literal_friendly[17] = 7;
    literal_friendly[18] = 7;
    cost_sets.push(literal_friendly);
    let mut repeat_friendly = [6_u8; 19];
    repeat_friendly[0] = 4;
    repeat_friendly[3] = 4;
    repeat_friendly[16] = 1;
    repeat_friendly[17] = 1;
    repeat_friendly[18] = 1;
    cost_sets.push(repeat_friendly);

    for length in 1..=8 {
        for mask in 0..1_usize << length {
            let sequence: Vec<_> = (0..length)
                .map(|bit| if mask & (1 << bit) == 0 { 0 } else { 3 })
                .collect();
            for costs in &cost_sets {
                assert_eq!(
                    shortest_rle(&sequence, costs),
                    exhaustive_shortest_rle(&sequence, costs),
                    "sequence {sequence:?}, costs {costs:?}",
                );
            }
        }
    }

    let long_zero_run = [0_u8; 12];
    for costs in &cost_sets {
        assert_eq!(
            shortest_rle(&long_zero_run, costs),
            exhaustive_shortest_rle(&long_zero_run, costs),
        );
    }
}

/// Slow oracle: scan every legal edge and keep whole suffix spellings. It
/// shares no sliding-window state with `shortest_rle`.
fn scanned_shortest_rle(lengths: &[u8], costs: &[u8; 19]) -> Option<Vec<RleToken>> {
    let mut suffixes = vec![None; lengths.len() + 1];
    suffixes[lengths.len()] = Some((0_u64, Vec::<RleToken>::new()));
    for start in (0..lengths.len()).rev() {
        let run = lengths[start..]
            .iter()
            .take_while(|&&value| value == lengths[start])
            .count();
        let mut edges = vec![(1, lengths[start], 0)];
        for (symbol, minimum, maximum, allowed) in [
            (16, 3, 6, start > 0 && lengths[start - 1] == lengths[start]),
            (17, 3, 10, lengths[start] == 0),
            (18, 11, 138, lengths[start] == 0),
        ] {
            if allowed {
                edges.extend(
                    (minimum..=run.min(maximum))
                        .map(|count| (count, symbol, (count - minimum) as u8)),
                );
            }
        }
        for (count, symbol, extra) in edges {
            if costs[usize::from(symbol)] == 0 {
                continue;
            }
            let Some((suffix_cost, suffix)) = &suffixes[start + count] else {
                continue;
            };
            let cost = suffix_cost + u64::from(costs[usize::from(symbol)]) + rle_extra_bits(symbol);
            if suffixes[start]
                .as_ref()
                .map_or(true, |(best, _)| cost < *best)
            {
                let mut path = vec![RleToken { symbol, extra }];
                path.extend_from_slice(suffix);
                suffixes[start] = Some((cost, path));
            }
        }
    }
    suffixes[0].take().map(|(_, path)| path)
}

#[test]
fn shortest_rle_preserves_long_run_windows_missing_codes_and_ties() {
    let mut state = 0x736f_6d65_7073_6575_u64;
    for n in [0, 3, 6, 10, 11, 12, 137, 138, 139, 276, 277, 318] {
        let mut separated = vec![0; n / 2];
        separated.extend([3; 7]);
        separated.resize(n, 0);
        for sequence in [vec![0; n], vec![3; n], separated] {
            for case in 0..24 {
                let mut costs = [1; 19];
                if case != 0 {
                    for cost in &mut costs {
                        state ^= state << 13;
                        state ^= state >> 7;
                        state ^= state << 17;
                        *cost = (state % 8) as u8;
                    }
                }
                assert_eq!(
                    shortest_rle(&sequence, &costs),
                    scanned_shortest_rle(&sequence, &costs),
                    "sequence {sequence:?}, costs {costs:?}",
                );
            }
        }
    }
}

#[test]
fn rle_round_trip_shape() {
    let lengths = [0, 0, 0, 0, 3, 3, 3, 3, 3, 0, 0, 0];
    let rle = greedy_rle(&lengths, false, false, false);
    assert!(rle.iter().any(|token| token.symbol == 16));
    assert!(rle.iter().any(|token| token.symbol == 17));
}

fn unpruned_rle_seed_candidates(lengths: &[u8], rle_mask: u8) -> Vec<Vec<RleToken>> {
    let mut candidates = Vec::new();
    for mask in 0..8 {
        if rle_mask & (1 << mask) == 0 {
            continue;
        }
        let no_16 = mask & 1 != 0;
        let no_17 = mask & 2 != 0;
        let no_18 = mask & 4 != 0;
        push_unique(&mut candidates, greedy_rle(lengths, no_16, no_17, no_18));
        if !no_16 {
            push_unique(&mut candidates, balanced_repeat_rle(lengths, no_17, no_18));
            push_unique(
                &mut candidates,
                columbo_zero_repeat_rle(lengths, no_17, no_18),
            );
        }
    }
    candidates
}

#[test]
fn rle_opportunity_gates_preserve_every_distinct_seed() {
    let masks = [0x01, 0x55, 0xaa, 0xff];
    for encoded in 0..4_u32.pow(6) {
        let mut encoded = encoded;
        let mut lengths = [0_u8; 6];
        for length in &mut lengths {
            *length = (encoded % 4) as u8;
            encoded /= 4;
        }
        for mask in masks {
            assert_eq!(
                rle_seed_candidates(&lengths, mask),
                unpruned_rle_seed_candidates(&lengths, mask),
            );
        }
    }

    for lengths in [
        vec![3; 8],
        vec![3; 9],
        vec![3; 14],
        vec![3; 15],
        vec![0; 11],
        vec![0; 138],
        vec![0; 139],
        vec![0; 141],
        [vec![2; 15], vec![0; 149], vec![4; 8]].concat(),
    ] {
        assert_eq!(
            rle_seed_candidates(&lengths, 0xff),
            unpruned_rle_seed_candidates(&lengths, 0xff),
        );
    }
}

#[test]
fn frequency_header_score_matches_token_scan() {
    let rle = vec![
        RleToken {
            symbol: 0,
            extra: 0,
        },
        RleToken {
            symbol: 16,
            extra: 2,
        },
        RleToken {
            symbol: 17,
            extra: 4,
        },
        RleToken {
            symbol: 18,
            extra: 40,
        },
        RleToken {
            symbol: 0,
            extra: 0,
        },
    ];
    let mut code_length_lengths = [0_u8; 19];
    for symbol in [0, 16, 17, 18] {
        code_length_lengths[symbol] = 2;
    }
    let hclen = trim_code_lengths(&code_length_lengths);
    let plan = DynamicPlan {
        literal_lengths: Vec::new(),
        distance_lengths: Vec::new(),
        code_length_lengths,
        rle: rle.clone(),
        hlit: 0,
        hdist: 0,
        hclen,
        bits: 0,
    };

    assert_eq!(
        dynamic_bits_from_rle_frequencies(73, hclen, &rle_frequencies(&rle), &code_length_lengths,),
        dynamic_bits(73, &plan),
    );
}

#[test]
fn distance_symbol_one_is_not_trimmed_away() {
    let mut lengths = [0_u8; 30];
    lengths[1] = 1;
    assert_eq!(trim_distance(&lengths), 2);
}

#[test]
fn columbo_quad_move_preserves_a_complete_tree() {
    let mut tokens = vec![Token::Literal(b'A'); 100];
    tokens.extend(b"bcde".iter().copied().map(Token::Literal));
    let mut literal_frequencies = [0_u32; 286];
    literal_frequencies[usize::from(b'A')] = 100;
    for symbol in b"bcde" {
        literal_frequencies[usize::from(*symbol)] = 1;
    }
    literal_frequencies[256] = 1;
    let distance_frequencies = [0_u32; 30];

    // Two length-two leaves and four length-three leaves form a complete
    // tree. The quad move shortens the frequent leaf and lengthens all
    // four rare leaves while preserving that Kraft sum.
    let mut literal_lengths = vec![0_u8; 257];
    literal_lengths[usize::from(b'A')] = 2;
    literal_lengths[256] = 2;
    for symbol in b"bcde" {
        literal_lengths[usize::from(*symbol)] = 3;
    }
    let seed = DynamicPlan {
        literal_lengths,
        distance_lengths: vec![1],
        code_length_lengths: [0; 19],
        rle: Vec::new(),
        hlit: 257,
        hdist: 1,
        hclen: 4,
        bits: 0,
    };

    let candidate = plan_columbo_balanced_tree_candidate(
        &tokens,
        &literal_frequencies,
        &distance_frequencies,
        &seed,
        true,
        false,
    )
    .expect("the Kraft-preserving quad move is legal");

    assert_eq!(candidate.literal_lengths[usize::from(b'A')], 1);
    for symbol in b"bcde" {
        assert_eq!(candidate.literal_lengths[usize::from(*symbol)], 4);
    }
    assert!(Huffman::build(&candidate.literal_lengths).is_some());
}

#[test]
fn columbo_pair_move_preserves_a_complete_tree() {
    let mut tokens = vec![Token::Literal(b'A'); 100];
    tokens.extend(b"bc".iter().copied().map(Token::Literal));
    let mut literal_frequencies = [0_u32; 286];
    literal_frequencies[usize::from(b'A')] = 100;
    for symbol in b"bc" {
        literal_frequencies[usize::from(*symbol)] = 1;
    }
    literal_frequencies[256] = 1;
    let distance_frequencies = [0_u32; 30];

    // Four length-two leaves form a complete tree. Shortening the frequent
    // leaf and lengthening two rare leaves preserves the Kraft sum.
    let mut literal_lengths = vec![0_u8; 257];
    for symbol in b"Abc" {
        literal_lengths[usize::from(*symbol)] = 2;
    }
    literal_lengths[256] = 2;
    let seed = DynamicPlan {
        literal_lengths,
        distance_lengths: vec![1],
        code_length_lengths: [0; 19],
        rle: Vec::new(),
        hlit: 257,
        hdist: 1,
        hclen: 4,
        bits: 0,
    };

    let candidate = plan_columbo_balanced_tree_candidate(
        &tokens,
        &literal_frequencies,
        &distance_frequencies,
        &seed,
        true,
        true,
    )
    .expect("the Kraft-preserving pair move is legal");

    assert_eq!(candidate.literal_lengths[usize::from(b'A')], 1);
    assert_eq!(candidate.literal_lengths[usize::from(b'b')], 3);
    assert_eq!(candidate.literal_lengths[usize::from(b'c')], 3);
    assert!(Huffman::build(&candidate.literal_lengths).is_some());
}

#[test]
fn columbo_pair_move_also_optimizes_the_distance_tree() {
    let mut tokens = Vec::new();
    for (symbol, count) in [(0_u8, 100_usize), (1, 1), (2, 1), (3, 1)] {
        tokens.extend((0..count).map(|_| Token::Match {
            length: 3,
            distance: u16::from(symbol) + 1,
            length_symbol: 257,
            distance_symbol: symbol,
            length_extra: 0,
            distance_extra: 0,
            length_extra_bits: 0,
            distance_extra_bits: 0,
        }));
    }
    let mut literal_frequencies = [0_u32; 286];
    literal_frequencies[256] = 1;
    literal_frequencies[257] = tokens.len() as u32;
    let mut distance_frequencies = [0_u32; 30];
    distance_frequencies[0] = 100;
    distance_frequencies[1..4].fill(1);
    let mut literal_lengths = vec![0_u8; 258];
    literal_lengths[256] = 1;
    literal_lengths[257] = 1;
    let seed = DynamicPlan {
        literal_lengths,
        distance_lengths: vec![2; 4],
        code_length_lengths: [0; 19],
        rle: Vec::new(),
        hlit: 258,
        hdist: 4,
        hclen: 4,
        bits: 0,
    };

    let candidate = plan_columbo_balanced_tree_candidate(
        &tokens,
        &literal_frequencies,
        &distance_frequencies,
        &seed,
        true,
        false,
    )
    .expect("the distance-side pair move is legal");

    assert_eq!(candidate.distance_lengths[0], 1);
    assert_eq!(
        candidate
            .distance_lengths
            .iter()
            .filter(|&&length| length == 3)
            .count(),
        2
    );
    assert!(Huffman::build(&candidate.distance_lengths).is_some());
}

#[test]
fn columbo_quad_move_also_optimizes_the_distance_tree() {
    let tokens = Vec::new();
    let mut literal_frequencies = [0_u32; 286];
    literal_frequencies[256] = 1;
    let mut distance_frequencies = [0_u32; 30];
    distance_frequencies[0] = 100;
    distance_frequencies[1..6].fill(1);
    let mut literal_lengths = vec![0_u8; 257];
    literal_lengths[256] = 1;
    let seed = DynamicPlan {
        literal_lengths,
        distance_lengths: vec![2, 2, 3, 3, 3, 3],
        code_length_lengths: [0; 19],
        rle: Vec::new(),
        hlit: 257,
        hdist: 6,
        hclen: 4,
        bits: 0,
    };

    let candidate = plan_columbo_balanced_tree_candidate(
        &tokens,
        &literal_frequencies,
        &distance_frequencies,
        &seed,
        true,
        false,
    )
    .expect("the distance-side quad move is legal");

    assert_eq!(candidate.distance_lengths[0], 1);
    assert!(candidate.distance_lengths[2..6]
        .iter()
        .all(|&length| length == 4));
    assert!(Huffman::build(&candidate.distance_lengths).is_some());
}

#[test]
fn columbo_balanced_tree_search_prices_paired_alphabet_moves() {
    let tokens = Vec::new();
    let mut literal_frequencies = [0_u32; 286];
    literal_frequencies[usize::from(b'A')] = 100;
    literal_frequencies[usize::from(b'b')] = 1;
    literal_frequencies[usize::from(b'c')] = 1;
    literal_frequencies[256] = 1;
    let mut distance_frequencies = [0_u32; 30];
    distance_frequencies[0] = 100;
    distance_frequencies[1..4].fill(1);
    let mut literal_lengths = vec![0_u8; 257];
    for symbol in b"Abc" {
        literal_lengths[usize::from(*symbol)] = 2;
    }
    literal_lengths[256] = 2;
    let seed = DynamicPlan {
        literal_lengths,
        distance_lengths: vec![2; 4],
        code_length_lengths: [0; 19],
        rle: Vec::new(),
        hlit: 257,
        hdist: 4,
        hclen: 4,
        bits: 0,
    };

    let opportunities =
        balanced_tree_opportunities(&literal_frequencies, &distance_frequencies, &seed)
            .expect("opportunity counting should remain bounded");
    assert!(opportunities.literal_pair_moves > 0);
    assert!(opportunities.distance_pair_moves > 0);
    assert_eq!(opportunities.paired_prices, 16);

    let candidate = plan_columbo_balanced_tree_candidate(
        &tokens,
        &literal_frequencies,
        &distance_frequencies,
        &seed,
        true,
        true,
    )
    .expect("the paired balanced-tree move is legal");

    assert_eq!(candidate.literal_lengths[usize::from(b'A')], 1);
    assert_eq!(candidate.distance_lengths[0], 1);
    assert!(Huffman::build(&candidate.literal_lengths).is_some());
    assert!(Huffman::build(&candidate.distance_lengths).is_some());
}

#[test]
fn balanced_repeat_avoids_a_two_literal_tail() {
    let lengths = [5_u8; 9];
    let greedy = greedy_rle(&lengths, false, false, false);
    let balanced = balanced_repeat_rle(&lengths, false, false);

    assert_eq!(greedy.len(), 4);
    assert_eq!(balanced.len(), 3);
    assert_eq!(
        balanced[0],
        RleToken {
            symbol: 5,
            extra: 0
        }
    );
    assert_eq!(
        balanced[1],
        RleToken {
            symbol: 16,
            extra: 1
        }
    );
    assert_eq!(
        balanced[2],
        RleToken {
            symbol: 16,
            extra: 1
        }
    );
}

#[test]
fn columbo_zero_repeat_can_continue_a_literal_zero() {
    let lengths = [0_u8; 5];
    let ordinary = greedy_rle(&lengths, false, true, false);
    let source_shaped = columbo_zero_repeat_rle(&lengths, true, false);

    assert_eq!(
        ordinary,
        vec![
            RleToken {
                symbol: 0,
                extra: 0
            };
            5
        ]
    );
    assert_eq!(
        source_shaped,
        [
            RleToken {
                symbol: 0,
                extra: 0,
            },
            RleToken {
                symbol: 16,
                extra: 1,
            },
        ]
    );
}

#[test]
fn package_merge_can_win_the_code_length_alphabet() {
    // On this histogram Defluff's package-merge tree saves one bit over every
    // heap tree. No adjacent repeats, so the test isolates the 19-symbol tree
    // choice from RLE packing.
    let mut remaining = [134_u32, 2, 0, 1, 1, 6, 5, 39, 33, 21, 74];
    let mut lengths = Vec::new();
    lengths.try_reserve_exact(316).unwrap();
    while lengths.len() < 316 {
        let previous = lengths.last().copied();
        let symbol = remaining
            .iter()
            .enumerate()
            .filter(|&(symbol, &count)| count != 0 && Some(symbol as u8) != previous)
            .max_by_key(|&(symbol, &count)| (count, std::cmp::Reverse(symbol)))
            .map(|(symbol, _)| symbol)
            .unwrap();
        remaining[symbol] -= 1;
        lengths.push(symbol as u8);
    }
    assert!(remaining.iter().all(|&count| count == 0));
    assert!(lengths.windows(2).all(|pair| pair[0] != pair[1]));

    let rle: Vec<_> = lengths
        .iter()
        .copied()
        .map(|symbol| RleToken { symbol, extra: 0 })
        .collect();
    let mut search = HeaderPlanSearch::default();
    consider_rle(
        4_496,
        &lengths[..286],
        &lengths[286..],
        &lengths,
        rle,
        false,
        &mut search,
    );

    let best = search.best.unwrap();
    assert_eq!(
        best.code_length_lengths,
        [1, 6, 0, 7, 7, 6, 6, 4, 4, 4, 2, 0, 0, 0, 0, 0, 0, 0, 0]
    );
}

#[test]
fn generated_plans_reject_incomplete_multi_symbol_payload_trees() {
    let mut literal_lengths = vec![0_u8; 257];
    literal_lengths[0] = 15;
    literal_lengths[256] = 15;

    assert!(Huffman::build(&literal_lengths).is_some());
    assert!(plan_for_explicit_lengths(&[], &literal_lengths, &[1], false).is_none());
}

#[test]
fn deflopt_local_rewrite_has_a_strictly_decreasing_repeat_rank() {
    let input = [
        RleToken {
            symbol: 0,
            extra: 0,
        },
        RleToken {
            symbol: 17,
            extra: 1,
        },
    ];
    let mut code_lengths = [0_u8; 19];
    code_lengths[0] = 3;
    code_lengths[16] = 2;
    code_lengths[17] = 2;

    let (rewritten, frequencies) =
        rewrite_rle_deflopt_local(&input, &rle_frequencies(&input), &code_lengths).unwrap();
    assert_eq!(rewritten[1].symbol, 16);
    assert_eq!(frequencies, rle_frequencies(&rewritten));
    assert!(repeat_rank(&frequencies) < repeat_rank(&rle_frequencies(&input)));
}

#[test]
fn deflopt_local_rewrite_expands_only_a_strictly_dearer_repeat() {
    let input = [RleToken {
        symbol: 18,
        extra: 0, // eleven zero lengths
    }];
    let mut code_lengths = [0_u8; 19];
    code_lengths[0] = 1;
    code_lengths[18] = 7;

    let (rewritten, frequencies) =
        rewrite_rle_deflopt_local(&input, &rle_frequencies(&input), &code_lengths).unwrap();
    assert_eq!(frequencies, rle_frequencies(&rewritten));
    assert_eq!(rewritten.len(), 11);
    assert!(rewritten.iter().all(|token| token.symbol == 0));

    // A tie keeps the repeat code, matching DeflOpt's strict-improvement
    // rule.
    code_lengths[0] = 1;
    code_lengths[18] = 4;
    assert!(rewrite_rle_deflopt_local(&input, &rle_frequencies(&input), &code_lengths).is_none());
}

#[test]
fn deft4j_prune_can_rebuild_after_an_equal_cost_rewrite() {
    let input = [RleToken {
        symbol: 17,
        extra: 0, // three zero lengths
    }];
    let mut code_lengths = [0_u8; 19];
    code_lengths[0] = 2;
    code_lengths[17] = 3;

    // Three explicit zero codes and one code 17 plus its three extra bits
    // both cost six. The strict final pass keeps the repeat; deft4j's
    // pre-rebuild prune expands the tie.
    let frequencies = rle_frequencies(&input);
    assert!(rewrite_rle_deft4j_literals(&input, &frequencies, &code_lengths, false).is_none());
    assert_eq!(
        rewrite_rle_deft4j_literals(&input, &frequencies, &code_lengths, true)
            .unwrap()
            .0,
        vec![
            RleToken {
                symbol: 0,
                extra: 0
            };
            3
        ]
    );
}

#[test]
fn unused_distance_alphabet_can_remain_empty() {
    let frequencies = [0_u32; 30];
    let candidates = tree_candidates(&frequencies, 15, false);
    assert!(candidates
        .iter()
        .any(|lengths| lengths.iter().all(|&length| length == 0)));
}

#[test]
fn strict_distance_completions_are_complete_uniform_and_cover_the_used_symbol() {
    let empty = strict_distance_completions(&[0; 30]);
    assert_eq!(empty.len(), 4);
    for (depth, lengths) in (1..=4_u8).zip(&empty) {
        assert!(huffman_tree_shape_is_complete(lengths));
        assert_eq!(&lengths[..1 << depth], &vec![depth; 1 << depth][..]);
        assert!(lengths[1 << depth..].iter().all(|&length| length == 0));
    }

    for used in [0, 7, 29] {
        let mut frequencies = [0_u32; 30];
        frequencies[used] = 5;
        let completions = strict_distance_completions(&frequencies);
        assert_eq!(completions.len(), 4);
        for lengths in &completions {
            assert!(huffman_tree_shape_is_complete(lengths));
            assert_ne!(lengths[used], 0);
        }
    }

    let mut two_used = [0_u32; 30];
    two_used[3] = 1;
    two_used[4] = 1;
    assert!(strict_distance_completions(&two_used).is_empty());
}

#[test]
fn strict_distance_completion_replaces_a_costly_one_bit_pair() {
    // A literal-only block whose planned `[1, 1]` distance completion is the
    // only reason code-length symbol 1 is sent, and so 18 code-length code
    // lengths.
    let block = payload_tradeoff_test_block();
    let original = block.original_dynamic.as_ref().unwrap();
    assert_eq!(&original.distance_lengths[..2], &[1, 1]);
    assert_eq!(original.hclen, 18);

    let mut prices = 4;
    let completed =
        plan_strict_distance_completion(&block, true, &mut prices, &mut SearchStop::never())
            .unwrap();
    assert!(completed.bits < original.bits);
    assert!(completed.has_strictly_compatible_huffman_codes());
    assert_eq!(completed.literal_lengths, original.literal_lengths);
    assert!(completed.hclen < original.hclen);
    assert_eq!(prices, 0);

    let mut prices = 4;
    assert!(
        plan_strict_distance_completion(&block, false, &mut prices, &mut SearchStop::never())
            .is_none()
    );
    assert_eq!(prices, 4);
}

#[test]
fn strict_completion_payload_adjustment_matches_a_full_token_scan() {
    let mut tokens: Vec<Token> = (0..40_u8).map(|byte| Token::Literal(byte % 7)).collect();
    tokens.extend(
        std::iter::repeat(Token::Match {
            length: 3,
            distance: 4,
            length_symbol: 257,
            distance_symbol: 3,
            length_extra: 0,
            distance_extra: 0,
            length_extra_bits: 0,
            distance_extra_bits: 0,
        })
        .take(5),
    );
    let (literal_frequencies, distance_frequencies) = count_frequencies(&tokens);
    let literal = make_lengths(&literal_frequencies, 15, 0);
    let literal = &literal[..trim_literal(&literal)];
    for original in [&[0, 0, 1, 1][..], &[0, 0, 0, 1, 1][..]] {
        let base = token_bits(&tokens, literal, original).unwrap();
        for completion in strict_distance_completions(&distance_frequencies) {
            let completion = &completion[..trim_distance(&completion)];
            assert_eq!(
                completed_distance_data_bits(base, &distance_frequencies, original, completion),
                token_bits(&tokens, literal, completion),
            );
        }
    }

    let literal_only: Vec<Token> = (0..20_u8).map(Token::Literal).collect();
    let (literal_frequencies, distance_frequencies) = count_frequencies(&literal_only);
    let literal = make_lengths(&literal_frequencies, 15, 0);
    let literal = &literal[..trim_literal(&literal)];
    let base = token_bits(&literal_only, literal, &[1, 1]).unwrap();
    for completion in strict_distance_completions(&distance_frequencies) {
        let completion = &completion[..trim_distance(&completion)];
        assert_eq!(
            completed_distance_data_bits(base, &distance_frequencies, &[1, 1], completion),
            token_bits(&literal_only, literal, completion),
        );
    }
}

#[test]
fn strict_empty_dynamic_plan_uses_complete_huffman_codes() {
    // An end-of-block-only literal/length code and an empty distance code, as
    // in libdeflate issue #323. Strict output completes both codes.
    let mut literal_frequencies = [0_u32; 286];
    literal_frequencies[256] = 1;
    let distance_frequencies = [0_u32; 30];

    let strict = best_dynamic_plan(
        &[],
        &literal_frequencies,
        &distance_frequencies,
        None,
        true,
        false,
        &mut SearchStop::never(),
    )
    .unwrap();
    assert!(strict.has_strictly_compatible_huffman_codes());
    assert_eq!(
        strict
            .literal_lengths
            .iter()
            .filter(|&&length| length != 0)
            .count(),
        2
    );
    assert_eq!(
        strict
            .distance_lengths
            .iter()
            .take(30)
            .filter(|&&length| length != 0)
            .count(),
        2
    );

    let relaxed = best_dynamic_plan(
        &[],
        &literal_frequencies,
        &distance_frequencies,
        None,
        false,
        false,
        &mut SearchStop::never(),
    )
    .unwrap();
    assert!(!relaxed.has_strictly_compatible_huffman_codes());
}

#[test]
fn equal_frequency_arrangement_preserves_payload_cost_and_histogram() {
    let frequencies = [7, 2, 7, 7, 2, 1];
    let original = [4, 3, 2, 3, 5, 2];
    let original_cost: u32 = frequencies
        .iter()
        .zip(original)
        .map(|(&frequency, length)| frequency * u32::from(length))
        .sum();

    let mut ascending = original;
    arrange_equal_frequency_lengths(&frequencies, &mut ascending, false);
    assert_eq!(ascending, [2, 3, 3, 4, 5, 2]);

    let mut descending = original;
    arrange_equal_frequency_lengths(&frequencies, &mut descending, true);
    assert_eq!(descending, [4, 5, 3, 2, 3, 2]);

    for arranged in [ascending, descending] {
        let arranged_cost: u32 = frequencies
            .iter()
            .zip(arranged)
            .map(|(&frequency, length)| frequency * u32::from(length))
            .sum();
        let mut histogram = arranged;
        histogram.sort_unstable();
        let mut original_histogram = original;
        original_histogram.sort_unstable();
        assert_eq!(arranged_cost, original_cost);
        assert_eq!(histogram, original_histogram);
    }
}

#[test]
fn deft4j_zero_runs_use_maximal_repeat_chunks() {
    let chunks = |tokens: &[RleToken], symbol: u8, base: usize| -> Vec<usize> {
        tokens
            .iter()
            .filter(|token| token.symbol == symbol)
            .map(|token| usize::from(token.extra) + base)
            .collect()
    };
    for run in 0..=316_usize {
        for (no_long, no_short) in [(false, false), (true, false), (false, true), (true, true)] {
            // Literal remainders keep the repeat-16 path out of this check.
            let options = Deft4jPackOptions {
                special_repeat: false,
                use_eight: false,
                use_seven: false,
                no_repeat: true,
                no_zero_repeat: no_short,
                no_long_zero_repeat: no_long,
                no_repeat_zeros: false,
            };
            let Some(tokens) = deft4j_pack_code_lengths(&vec![0; run], options) else {
                continue;
            };
            let long = chunks(&tokens, 18, 11);
            let short = chunks(&tokens, 17, 3);
            let literal = tokens.iter().filter(|token| token.symbol == 0).count();
            assert_eq!(
                long.iter().sum::<usize>() + short.iter().sum::<usize>() + literal,
                run
            );
            if let Some((last, full)) = long.split_last() {
                assert!(full.iter().all(|&count| count == 138));
                assert!((11..=138).contains(last));
            }
            if let Some((last, full)) = short.split_last() {
                assert!(full.iter().all(|&count| count == 10));
                assert!((3..=10).contains(last));
            }
            let after_long = run - long.iter().sum::<usize>();
            assert!(no_long || after_long < 11, "{run} {options:?}");
            assert!(no_short || literal < 3, "{run} {options:?}");
        }
    }
}

/// `reorder_code_length_lengths` before its linear early exit.
fn reference_reorder_code_length_lengths(lengths: &mut [u8; 19], frequencies: &[u32; 19]) -> bool {
    let mut changed = false;
    for left in 0..18 {
        if lengths[left] == 0 {
            continue;
        }
        for right in (left + 1)..19 {
            if lengths[right] == 0 {
                continue;
            }
            let should_swap = if lengths[right] < lengths[left] {
                frequencies[right] < frequencies[left]
            } else if lengths[right] > lengths[left] {
                frequencies[right] > frequencies[left]
            } else {
                false
            };
            if should_swap {
                lengths.swap(left, right);
                changed = true;
            }
        }
    }
    changed
}

#[test]
fn code_length_reorder_early_exit_matches_the_full_pass() {
    let mut state = 0x2545_f491_4f6c_dd1d_u64;
    let mut next = move || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state
    };
    let mut early_exits = 0;
    for case in 0..200_000 {
        let mut frequencies = [0_u32; 19];
        let mut lengths = [0_u8; 19];
        for frequency in frequencies.iter_mut() {
            if next() % 4 != 0 {
                *frequency = (next() % [3, 8, 60][case % 3]) as u32;
            }
        }
        if case % 2 == 0 {
            // Trees from the builders themselves, which mostly pass the check.
            make_lengths_into(&frequencies, &mut lengths, 7, (case % 4) as u32);
        } else {
            for length in lengths.iter_mut() {
                *length = (next() % 8) as u8;
            }
        }
        let mut expected = lengths;
        let mut actual = lengths;
        let expected_changed = reference_reorder_code_length_lengths(&mut expected, &frequencies);
        let actual_changed = reorder_code_length_lengths(&mut actual, &frequencies);
        assert_eq!(
            (actual_changed, actual),
            (expected_changed, expected),
            "case {case}"
        );
        early_exits += usize::from(code_lengths_follow_frequencies(&lengths, &frequencies));
    }
    assert!(early_exits > 50_000);
}

/// Trimmed literal/length and distance code lengths from random Huffman trees,
/// with zero runs long enough for codes 17 and 18.
fn random_trimmed_lengths(next: &mut impl FnMut() -> u64) -> (Vec<u8>, Vec<u8>) {
    let mut literal_frequencies = [0_u32; 286];
    for (symbol, frequency) in literal_frequencies.iter_mut().enumerate() {
        let gap = (symbol / 8) % 3 == usize::try_from(next() % 3).unwrap();
        if !gap && next() % 3 != 0 {
            *frequency = u32::try_from(next() % 200).unwrap();
        }
    }
    literal_frequencies[256] = 1;
    let mut distance_frequencies = [0_u32; 30];
    for frequency in distance_frequencies
        .iter_mut()
        .take(1 + (next() % 30) as usize)
    {
        *frequency = u32::try_from(next() % 50).unwrap();
    }
    let literal = make_lengths(&literal_frequencies, 15, 0);
    let distance = make_lengths(&distance_frequencies, 15, 0);
    let literal = literal[..trim_literal(&literal)].to_vec();
    let distance = distance[..trim_distance(&distance)].to_vec();
    (literal, distance)
}

#[test]
fn stored_header_plans_round_trip_through_generation_turnover() {
    let mut state = 0x9e37_79b9_7f4a_7c15_u64;
    let mut next = move || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state
    };
    let cases: Vec<_> = (0..48).map(|_| random_trimmed_lengths(&mut next)).collect();
    let plans: Vec<_> = cases
        .iter()
        .map(|(literal, distance)| {
            plan_for_trimmed_lengths_uncached(literal, distance, 0, false, 0xff)
                .expect("random trees give a valid header")
        })
        .collect();
    // About six plans per generation, so lookups cross both generations and
    // evicted plans.
    let mut store = HeaderPlanStore::with_budget(8 << 10);
    let mut inserted = [false; 48];
    let (mut hits, mut evictions) = (0, 0);
    for round in 0..400 {
        let index = (next() % if round % 2 == 0 { 8 } else { 48 }) as usize;
        let (literal, distance) = &cases[index];
        let fingerprint = header_plan_fingerprint(literal, distance, false, 0xff);
        match store.get(fingerprint, literal, distance, false, 0xff) {
            Some(plan) => {
                assert_eq!(plan, plans[index]);
                hits += 1;
            }
            None => {
                evictions += usize::from(inserted[index]);
                inserted[index] = true;
                store.insert(
                    fingerprint,
                    StoredHeaderPlan::pack(false, 0xff, &plans[index]).unwrap(),
                );
            }
        }
        assert!(store.current.bytes < store.max_bytes / 2);
    }
    assert!(
        hits > 0 && evictions > 0,
        "{hits} hits, {evictions} evictions"
    );
}

#[test]
fn stored_header_plan_rejects_a_fingerprint_collision() {
    let mut state = 0x2545_f491_4f6c_dd1d_u64;
    let mut next = move || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state
    };
    let (literal, distance) = random_trimmed_lengths(&mut next);
    let (other_literal, other_distance) = random_trimmed_lengths(&mut next);
    let plan = plan_for_trimmed_lengths_uncached(&literal, &distance, 0, false, 0xff).unwrap();
    for move_to_previous in [false, true] {
        let mut store = HeaderPlanStore::with_budget(usize::MAX);
        store.insert(7, StoredHeaderPlan::pack(false, 0xff, &plan).unwrap());
        if move_to_previous {
            store.previous = std::mem::take(&mut store.current);
        }
        assert_eq!(
            store.get(7, &other_literal, &other_distance, false, 0xff),
            None
        );
        assert_eq!(store.get(7, &literal, &distance, true, 0xff), None);
        let expected = (!move_to_previous).then(|| plan.clone());
        // A rejected entry in the previous generation is dropped.
        assert_eq!(store.get(7, &literal, &distance, false, 0xff), expected);
    }
}

#[test]
fn thread_header_plans_match_uncached_planning() {
    let mut state = 0x6a09_e667_f3bc_c909_u64;
    let mut next = move || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state
    };
    release_thread_header_plans();
    for case in 0..40 {
        let (literal, distance) = random_trimmed_lengths(&mut next);
        let exhaustive = case % 8 == 0;
        let rle_mask = [0xff, 0x01, 0x11][case % 3];
        let data_bits = next() % 100_000;
        let expected =
            plan_for_trimmed_lengths_uncached(&literal, &distance, data_bits, exhaustive, rle_mask);
        for _ in 0..2 {
            let planned =
                plan_for_trimmed_lengths(&literal, &distance, data_bits, exhaustive, rle_mask);
            assert_eq!(planned, expected);
        }
    }
    release_thread_header_plans();
    assert!(HEADER_PLAN_STORE.with(|store| store.borrow().current.plans.is_empty()));
}

/// The local rewrite before it counted frequencies and allocated on the first
/// change.
fn rewrite_rle_deflopt_local_reference(
    input: &[RleToken],
    code_length_lengths: &[u8; 19],
) -> Option<Vec<RleToken>> {
    let mut output = Vec::new();
    let mut previous = None;
    let mut changed = false;
    for &token in input {
        match token.symbol {
            value @ 0..=15 => {
                output.push(token);
                previous = Some(value);
            }
            18 => {
                let count = usize::from(token.extra) + 11;
                if explicit_rle_cost(code_length_lengths, 0, count)
                    < rle_symbol_cost(code_length_lengths, 18)
                {
                    output.extend(
                        std::iter::repeat(RleToken {
                            symbol: 0,
                            extra: 0,
                        })
                        .take(count),
                    );
                    changed = true;
                } else {
                    output.push(token);
                }
                previous = Some(0);
            }
            17 => {
                let count = usize::from(token.extra) + 3;
                if previous == Some(0)
                    && token.extra <= 3
                    && code_length_lengths[16] != 0
                    && code_length_lengths[17] != 0
                    && code_length_lengths[16] <= code_length_lengths[17]
                {
                    output.push(RleToken {
                        symbol: 16,
                        extra: token.extra,
                    });
                    changed = true;
                } else if explicit_rle_cost(code_length_lengths, 0, count)
                    < rle_symbol_cost(code_length_lengths, 17)
                {
                    output.extend(
                        std::iter::repeat(RleToken {
                            symbol: 0,
                            extra: 0,
                        })
                        .take(count),
                    );
                    changed = true;
                } else {
                    output.push(token);
                }
                previous = Some(0);
            }
            16 => {
                let previous = previous?;
                let count = usize::from(token.extra) + 3;
                if explicit_rle_cost(code_length_lengths, previous, count)
                    < rle_symbol_cost(code_length_lengths, 16)
                {
                    output.extend(
                        std::iter::repeat(RleToken {
                            symbol: previous,
                            extra: 0,
                        })
                        .take(count),
                    );
                    changed = true;
                } else {
                    output.push(token);
                }
            }
            _ => return None,
        }
    }
    changed.then_some(output)
}

#[test]
fn deflopt_local_rewrite_matches_the_reference_and_counts_its_symbols() {
    let mut random = 0x2545_f491_4f6c_dd1d_u64;
    let mut next = move || {
        random ^= random << 13;
        random ^= random >> 7;
        random ^= random << 17;
        random
    };
    for case in 0..20_000 {
        let len = 1 + next() as usize % 40;
        let input: Vec<RleToken> = (0..len)
            .map(|_| {
                let symbol = (next() % 19) as u8;
                let extra = match symbol {
                    16 => next() % 4,
                    17 => next() % 8,
                    18 => next() % 128,
                    _ => 0,
                } as u8;
                RleToken { symbol, extra }
            })
            .collect();
        let mut code_lengths = [0_u8; 19];
        for length in &mut code_lengths {
            *length = (next() % 8) as u8;
        }
        let expected = rewrite_rle_deflopt_local_reference(&input, &code_lengths);
        let actual = rewrite_rle_deflopt_local(&input, &rle_frequencies(&input), &code_lengths);
        assert_eq!(
            actual.as_ref().map(|(rle, _)| rle),
            expected.as_ref(),
            "case {case}"
        );
        if let Some((rle, frequencies)) = actual {
            assert_eq!(frequencies, rle_frequencies(&rle), "case {case}");
        }
    }
}

/// `rewrite_rle_deft4j_literals` before it took and counted frequencies.
fn rewrite_rle_deft4j_literals_reference(
    input: &[RleToken],
    code_length_lengths: &[u8; 19],
    include_equal: bool,
) -> Option<Vec<RleToken>> {
    let mut output = Vec::new();
    let mut previous = None;
    let mut changed = false;
    for &token in input {
        let (value, count) = match token.symbol {
            value @ 0..=15 => {
                output.push(token);
                previous = Some(value);
                continue;
            }
            16 => (previous?, usize::from(token.extra) + 3),
            17 => (0, usize::from(token.extra) + 3),
            18 => (0, usize::from(token.extra) + 11),
            _ => return None,
        };
        let explicit = explicit_rle_cost(code_length_lengths, value, count);
        let repeat = rle_symbol_cost(code_length_lengths, token.symbol);
        if explicit < repeat || (include_equal && explicit == repeat) {
            if output.len() + count > MAX_DYNAMIC_CODE_LENGTH_COUNT {
                return None;
            }
            output.extend(
                std::iter::repeat(RleToken {
                    symbol: value,
                    extra: 0,
                })
                .take(count),
            );
            changed = true;
        } else {
            output.push(token);
        }
        previous = Some(value);
    }
    changed.then_some(output)
}

#[test]
fn deft4j_rewrite_matches_the_reference_and_counts_its_symbols() {
    let mut random = 0x9e37_79b9_7f4a_7c15_u64;
    let mut next = move || {
        random ^= random << 13;
        random ^= random >> 7;
        random ^= random << 17;
        random
    };
    for case in 0..20_000 {
        let len = 1 + next() as usize % 40;
        let input: Vec<RleToken> = (0..len)
            .map(|_| {
                let symbol = (next() % 19) as u8;
                let extra = match symbol {
                    16 => next() % 4,
                    17 => next() % 8,
                    18 => next() % 128,
                    _ => 0,
                } as u8;
                RleToken { symbol, extra }
            })
            .collect();
        let mut code_lengths = [0_u8; 19];
        for length in &mut code_lengths {
            *length = (next() % 8) as u8;
        }
        for include_equal in [false, true] {
            let expected =
                rewrite_rle_deft4j_literals_reference(&input, &code_lengths, include_equal);
            let actual = rewrite_rle_deft4j_literals(
                &input,
                &rle_frequencies(&input),
                &code_lengths,
                include_equal,
            );
            assert_eq!(
                actual.as_ref().map(|(rle, _)| rle),
                expected.as_ref(),
                "case {case}"
            );
            if let Some((rle, frequencies)) = actual {
                assert_eq!(frequencies, rle_frequencies(&rle), "case {case}");
            }
        }
    }
}

/// `shortest_rle` before it used fixed buffers and counted frequencies.
fn shortest_rle_reference(lengths: &[u8], costs: &[u8; 19]) -> Option<Vec<RleToken>> {
    #[derive(Clone, Copy)]
    struct Step {
        next: usize,
        token: RleToken,
    }

    let mut best = vec![INF; lengths.len() + 1];
    let mut step = vec![None; lengths.len()];
    let mut run = 0;
    let mut zero_repeats = std::collections::VecDeque::<usize>::new();
    best[lengths.len()] = 0;
    for index in (0..lengths.len()).rev() {
        if index + 1 < lengths.len() && lengths[index] == lengths[index + 1] {
            run += 1;
        } else {
            run = 1;
            zero_repeats.clear();
        }

        // Code 18 costs the same for every repeat from 11 to 138, so a
        // monotonic deque keeps the cheapest suffix of that sliding window.
        // Newer ends are smaller, so dropping equal costs keeps the preference
        // for the shortest repeat.
        let zero_repeat = if lengths[index] == 0 && costs[18] != 0 && run >= 11 {
            let first_end = index + 11;
            let last_end = index + run.min(138);
            while zero_repeats.front().is_some_and(|&end| end > last_end) {
                zero_repeats.pop_front();
            }
            while zero_repeats
                .back()
                .is_some_and(|&end| best[end] >= best[first_end])
            {
                zero_repeats.pop_back();
            }
            zero_repeats.push_back(first_end);
            zero_repeats.front().map(|&end| end - index)
        } else {
            None
        };

        let mut consider = |count: usize, symbol: u8, extra: u8| {
            let code = costs[usize::from(symbol)];
            if code == 0 || index + count > lengths.len() {
                return;
            }
            let cost = u64::from(code)
                .saturating_add(rle_extra_bits(symbol))
                .saturating_add(best[index + count]);
            // Replace only on a strict improvement, so a tie keeps the option
            // considered first: an explicit length, then codes 16, 17 and 18.
            if cost < best[index] {
                best[index] = cost;
                step[index] = Some(Step {
                    next: index + count,
                    token: RleToken { symbol, extra },
                });
            }
        };

        consider(1, lengths[index], 0);
        if index > 0 && lengths[index] == lengths[index - 1] {
            for count in 3..=run.min(6) {
                consider(count, 16, (count - 3) as u8);
            }
        }
        if lengths[index] == 0 {
            for count in 3..=run.min(10) {
                consider(count, 17, (count - 3) as u8);
            }
            if let Some(count) = zero_repeat {
                consider(count, 18, (count - 11) as u8);
            }
        }
    }
    if best[0] == INF {
        return None;
    }
    let mut output = Vec::new();
    let mut index = 0;
    while index < lengths.len() {
        let selected = step[index]?;
        output.push(selected.token);
        index = selected.next;
    }
    Some(output)
}

#[test]
fn shortest_rle_matches_the_reference_and_counts_its_symbols() {
    // One scratch for every case, as a header search reuses it.
    let mut scratch = ShortestRleScratch::default();
    let mut random = 0x9e37_79b9_7f4a_7c15_u64;
    let mut next = move || {
        random ^= random << 13;
        random ^= random >> 7;
        random ^= random << 17;
        random
    };
    for case in 0..20_000 {
        let len = 1 + next() as usize % MAX_DYNAMIC_CODE_LENGTH_COUNT;
        // Long runs of zeros and repeats exercise codes 16, 17 and 18.
        let mut lengths = Vec::with_capacity(len);
        while lengths.len() < len {
            let value = if next() % 3 == 0 {
                0
            } else {
                (next() % 16) as u8
            };
            let run = 1 + next() as usize % 150;
            lengths.extend(std::iter::repeat(value).take(run.min(len - lengths.len())));
        }
        let mut costs = [0_u8; 19];
        for cost in &mut costs {
            *cost = (next() % 8) as u8;
        }
        let expected = shortest_rle_reference(&lengths, &costs);
        let actual = shortest_rle_with_frequencies(&lengths, &costs, &mut scratch);
        assert_eq!(
            actual.as_ref().map(|(rle, _)| rle),
            expected.as_ref(),
            "case {case}"
        );
        assert_eq!(shortest_rle(&lengths, &costs), expected, "case {case}");
        if let Some((rle, frequencies)) = actual {
            assert_eq!(frequencies, rle_frequencies(&rle), "case {case}");
        }
    }
}
