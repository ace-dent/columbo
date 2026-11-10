// SPDX-License-Identifier: MIT
// SPDX-FileCopyrightText: 2026 Andrew C.E. Dent <https://github.com/ace-dent>

use super::*;
use crate::deflate::model::DynamicPlan;
use crate::deflate::stream::test_support::{literal_block, sequential_plan};

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
fn long_run_search_rebuilds_profitable_dynamic_neighbors() {
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
fn long_huffman_route_does_not_join_large_stored_neighbors() {
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
