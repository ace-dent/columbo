// SPDX-License-Identifier: MIT
// SPDX-FileCopyrightText: 2026 Andrew C.E. Dent <https://github.com/ace-dent>

use super::*;
use crate::deflate::model::{count_frequencies, Token};
use crate::deflate::stream::test_support::{assert_same_plan, literal_block};
use crate::deflate::stream::WIDE_COLLECT_MIN_SOURCE_BLOCKS;

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

    // Serial and parallel decisions must match, whether or not a merge is
    // profitable.
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
