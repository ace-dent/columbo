// SPDX-License-Identifier: MIT
// SPDX-FileCopyrightText: 2026 Andrew C.E. Dent <https://github.com/ace-dent>

use super::*;

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
            &mut ExactTreeBuilder::new(&literal_frequencies, &distance_frequencies),
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

#[test]
fn unused_distance_alphabet_can_remain_empty() {
    let frequencies = [0_u32; 30];
    let candidates = tree_candidates(&frequencies, 15, false);
    assert!(candidates
        .iter()
        .any(|lengths| lengths.iter().all(|&length| length == 0)));
}
