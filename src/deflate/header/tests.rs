// SPDX-License-Identifier: MIT

use super::test_support::{literal_span_test_block, payload_tradeoff_test_block};
use super::*;
use crate::deflate::huffman::Huffman;

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
    // Longer zero runs alone are not sufficient: finding this saving also
    // requires a different code-length tree. A fixed-CL shortcut misses it.
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
    // The first eight prices reach HLIT=277. Exhaustion retains that
    // complete winner without admitting any later span.
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
    // One price permits only the unchanged-tree control. Its fully priced
    // parent already ties, so no speculative permutation may escape.
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
    // Exhaustive short sequences include zero/nonzero runs, adjacent
    // swaps, endpoints and positions on either side of an alphabet seam.
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

/// Deliberately scan every legal edge and retain whole suffix spellings.
/// This slow oracle shares no sliding-window state with the production DP.
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
    // This histogram is a reduced header-only regression for a block where
    // Defluff's package-merge tree saves one bit over every heap tree. Lay
    // the symbols out without adjacent repeats so this test isolates the
    // nineteen-symbol tree choice rather than RLE packing.
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

    let rewritten = rewrite_rle_deflopt_local(&input, &code_lengths).unwrap();
    assert_eq!(rewritten[1].symbol, 16);
    assert!(repeat_rank(&rewritten) < repeat_rank(&input));
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

    let rewritten = rewrite_rle_deflopt_local(&input, &code_lengths).unwrap();
    assert_eq!(rewritten.len(), 11);
    assert!(rewritten.iter().all(|token| token.symbol == 0));

    // Ties are deliberately retained, matching the source optimizer's
    // deterministic strict-improvement rule.
    code_lengths[0] = 1;
    code_lengths[18] = 4;
    assert!(rewrite_rle_deflopt_local(&input, &code_lengths).is_none());
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

    // Three explicit zero codes and one repeat-17 code plus its three
    // extra bits both cost six. The ordinary strict finalizer retains the
    // repeat; deft4j's pre-rebuild prune deliberately expands the tie.
    assert!(rewrite_rle_deft4j_literals(&input, &code_lengths, false).is_none());
    assert_eq!(
        rewrite_rle_deft4j_literals(&input, &code_lengths, true).unwrap(),
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
fn strict_empty_dynamic_plan_uses_complete_huffman_codes() {
    // This is the shape that exposed libdeflate issue #323: an
    // all-literal block can otherwise advertise an empty distance code.
    // The EOB-only literal tree is another RFC edge case covered by
    // libdeflate's broader complete-code fix.
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
