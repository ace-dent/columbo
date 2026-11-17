// SPDX-License-Identifier: MIT
// SPDX-FileCopyrightText: 2026 Andrew C.E. Dent <https://github.com/ace-dent>

use std::sync::Arc;

use super::super::StreamIdentity;
use super::*;
use crate::deflate::model::{Representation, Token, TOKEN_CHARGE_BYTES};
use crate::deflate::parse::parse_stream;
use crate::deflate::search::PROVEN_SUBMATCH_FULL_MATCH_LIMIT;

#[test]
fn every_potentially_improvable_source_topology_admits_the_deft4j_route() {
    // A one-literal fixed stream represents the single-block members used
    // by GZIP, ZIP, and compressed PNG metadata. Container floor policy
    // must not make its direct deft4j endpoint unreachable.
    let parsed = parse_stream(&[0x4b, 0x04, 0x00], 1).unwrap();
    assert_eq!(parsed.decoded_size, 1);
    assert!(deft4j_source_route_eligible(&parsed.blocks));

    // Route memory accounting, rather than an arbitrary source-list cap,
    // controls very fragmented streams.
    let many = (0..129)
        .map(|_| parsed.blocks[0].try_clone_shared().unwrap())
        .collect::<Vec<_>>();
    assert!(deft4j_source_route_eligible(&many));
}

#[test]
fn no_split_route_uses_bounded_source_size_and_topology() {
    let parsed = parse_stream(&[0x4b, 0x04, 0x00], 1).unwrap();
    let blocks = vec![
        parsed.blocks[0].try_clone_shared().unwrap(),
        parsed.blocks[0].try_clone_shared().unwrap(),
    ];

    assert!(narrow_source_route_eligible(
        &blocks,
        NARROW_SOURCE_MAX_COMPRESSED
    ));
    assert!(!narrow_source_route_eligible(
        &blocks,
        NARROW_SOURCE_MAX_COMPRESSED + 1
    ));

    let too_many = (0..=NARROW_SOURCE_LIST_MAX_BLOCKS)
        .map(|_| parsed.blocks[0].try_clone_shared().unwrap())
        .collect::<Vec<_>>();
    assert!(!narrow_source_route_eligible(&too_many, 1));

    let mut stored = blocks;
    stored[0].source_type = SourceBlockType::Stored;
    assert!(!narrow_source_route_eligible(&stored, 1));
}

#[test]
fn default_source_topology_uses_the_no_split_class_above_a_decoded_floor() {
    let parsed = parse_stream(&[0x4b, 0x04, 0x00], 1).unwrap();
    let blocks = vec![
        parsed.blocks[0].try_clone_shared().unwrap(),
        parsed.blocks[0].try_clone_shared().unwrap(),
    ];
    let floor = DEFAULT_SOURCE_TOPOLOGY_MIN_DECODED;
    assert!(default_source_topology_eligible(&blocks, 1, floor));
    // Small streams such as APNG frames are left to the planner.
    assert!(!default_source_topology_eligible(&blocks, 1, floor - 1));
    // The no-split route's size and topology bounds still apply.
    assert!(!default_source_topology_eligible(
        &blocks,
        NARROW_SOURCE_MAX_COMPRESSED + 1,
        floor
    ));
    assert!(!default_source_topology_eligible(&blocks[..1], 1, floor));
    let mut stored = blocks;
    stored[0].source_type = SourceBlockType::Stored;
    assert!(!default_source_topology_eligible(&stored, 1, floor));
}

#[test]
fn parallel_routes_require_small_input_and_model_sizes() {
    assert!(parallel_route_sizes_are_bounded(
        PARALLEL_ROUTE_MAX_COMPRESSED,
        1,
        1,
        1,
    ));
    assert!(!parallel_route_sizes_are_bounded(
        PARALLEL_ROUTE_MAX_COMPRESSED + 1,
        1,
        1,
        1,
    ));
    assert!(!parallel_route_sizes_are_bounded(
        1,
        PARALLEL_ROUTE_MAX_DECODED + 1,
        1,
        1,
    ));

    let oversized_token_count = PARALLEL_ROUTE_MAX_MODEL / TOKEN_CHARGE_BYTES + 1;
    assert!(!parallel_route_sizes_are_bounded(
        1,
        1,
        oversized_token_count,
        1,
    ));
}

#[test]
fn bounded_floor_prebuild_uses_topology_and_route_work() {
    assert!(prebuild_bounded_floor(1, 1));
    assert!(prebuild_bounded_floor(
        2,
        PREBUILD_BOUNDED_FLOOR_MAX_DECODED
    ));
    assert!(!prebuild_bounded_floor(
        2,
        PREBUILD_BOUNDED_FLOOR_MAX_DECODED + 1
    ));
    assert!(!prebuild_bounded_floor(
        2,
        CONCURRENT_BOUNDED_FLOOR_MAX_DECODED
    ));
    assert!(prebuild_bounded_floor(
        2,
        CONCURRENT_BOUNDED_FLOOR_MAX_DECODED + 1
    ));

    let source_match = Token::Match {
        length: 3,
        distance: 1,
        length_symbol: 257,
        distance_symbol: 0,
    };
    let block = ParsedBlock {
        tokens: Arc::new(vec![source_match; PROVEN_SUBMATCH_FULL_MATCH_LIMIT + 1]),
        plain: Arc::new(Vec::new()),
        literal_frequencies: [0; 286],
        distance_frequencies: [0; 30],
        original_literal_lengths: None,
        original_distance_lengths: None,
        original_dynamic: None,
        original: None,
        source_splits: Vec::new(),
        source_type: SourceBlockType::Dynamic,
    };
    assert!(source_run_match_count_exceeds(
        std::slice::from_ref(&block),
        PROVEN_SUBMATCH_FULL_MATCH_LIMIT
    ));
}

#[test]
fn weak_deft4j_threshold_is_strictly_below_two_percent() {
    assert!(gain_is_below(10_000, 9_801, 200));
    assert!(!gain_is_below(10_000, 9_800, 200));
    assert!(!gain_is_below(10_000, 9_000, 200));
}

#[test]
fn best_first_floor_seed_respects_the_structural_split_signal() {
    assert!(!floor_seeded_priority_with_structural_sibling(
        10_000, 9_801, true, false,
    ));
    assert!(floor_seeded_priority_with_structural_sibling(
        10_000, 9_800, true, false,
    ));
    assert!(floor_seeded_priority_with_structural_sibling(
        10_000, 9_999, false, false,
    ));
    assert!(floor_seeded_priority_with_structural_sibling(
        10_000, 9_999, true, true,
    ));
}

#[test]
fn independent_refinement_overlaps_only_inside_the_bounded_parallel_class() {
    assert!(independent_deft4j_refinement_can_overlap(
        true, true, true, true
    ));
    assert!(!independent_deft4j_refinement_can_overlap(
        false, true, true, true
    ));
    assert!(!independent_deft4j_refinement_can_overlap(
        true, false, true, true
    ));
    assert!(!independent_deft4j_refinement_can_overlap(
        true, true, false, true
    ));
    assert!(!independent_deft4j_refinement_can_overlap(
        true, true, true, false
    ));
}

#[test]
fn continuation_runs_source_max_beside_it_where_the_policy_runs_it_in_parallel() {
    // An APNG child runs it in place of the other siblings in either share.
    for search_share in [false, true] {
        assert_eq!(
            continuation_source_max(DefaultFloor::ApngMax, search_share, true),
            ContinuationSourceMax::InsteadOfSiblings
        );
    }
    // A single PNG image runs it beside them only within the search share.
    assert_eq!(
        continuation_source_max(DefaultFloor::CompleteThenBounded, true, true),
        ContinuationSourceMax::BesideSiblings
    );
    assert_eq!(
        continuation_source_max(DefaultFloor::CompleteThenBounded, false, true),
        ContinuationSourceMax::Later
    );
    for floor in [
        DefaultFloor::Complete,
        DefaultFloor::CompleteThenBounded,
        DefaultFloor::Shared,
        DefaultFloor::SharedExact,
        DefaultFloor::ApngDefault,
        DefaultFloor::ApngMax,
        DefaultFloor::Established,
        DefaultFloor::MandatoryComplete,
    ] {
        for search_share in [false, true] {
            assert_eq!(
                continuation_source_max(floor, search_share, false),
                ContinuationSourceMax::Later
            );
            if !floor.allows_parallel_source_follow_up() {
                assert_eq!(
                    continuation_source_max(floor, search_share, true),
                    ContinuationSourceMax::Later
                );
            }
        }
    }
}

#[test]
fn bounded_png_max_policy_follows_available_route_families() {
    assert_eq!(
        bounded_png_max_policy(1, false, true, true),
        BoundedPngMaxPolicy::Standard
    );
    assert_eq!(
        bounded_png_max_policy(1, true, true, true),
        BoundedPngMaxPolicy::FloorExpansion
    );
    for blocks in [2, 4, 13, 32, 33] {
        assert_eq!(
            bounded_png_max_policy(blocks, false, true, true),
            BoundedPngMaxPolicy::FloorExpansion
        );
    }

    // Generic-only streams keep source max beside the floor lineage,
    // regardless of source block count.
    for blocks in [2, 13] {
        assert_eq!(
            bounded_png_max_policy(blocks, false, false, false),
            BoundedPngMaxPolicy::GenericParallel
        );
    }
}

#[test]
fn floor_state_probe_detects_token_and_boundary_changes() {
    fn parsed(tokens: Vec<Token>, plain: Vec<u8>) -> ParsedBlock {
        ParsedBlock {
            tokens: Arc::new(tokens),
            plain: Arc::new(plain),
            literal_frequencies: [0; 286],
            distance_frequencies: [0; 30],
            original_literal_lengths: None,
            original_distance_lengths: None,
            original_dynamic: None,
            original: None,
            source_splits: Vec::new(),
            source_type: SourceBlockType::Dynamic,
        }
    }
    fn planned(tokens: Vec<Token>, plain: Vec<u8>) -> PlannedBlock {
        PlannedBlock {
            tokens: Arc::new(tokens),
            plain: Arc::new(plain),
            representation: Representation::Fixed,
            bits: 0,
            source_type: SourceBlockType::Dynamic,
        }
    }

    let source = [parsed(
        vec![Token::Literal(b'a'), Token::Literal(b'b')],
        b"ab".to_vec(),
    )];
    let same = [planned(
        vec![Token::Literal(b'a'), Token::Literal(b'b')],
        b"ab".to_vec(),
    )];
    assert!(!floor_exposes_new_search_states(&source, &same));

    let changed_tokens = [planned(
        vec![Token::Literal(b'a'), Token::Literal(b'c')],
        b"ab".to_vec(),
    )];
    assert!(floor_exposes_new_search_states(&source, &changed_tokens));

    let changed_boundaries = [
        planned(vec![Token::Literal(b'a')], b"a".to_vec()),
        planned(vec![Token::Literal(b'b')], b"b".to_vec()),
    ];
    assert!(floor_exposes_new_search_states(
        &source,
        &changed_boundaries
    ));
}

#[test]
fn compact_max_scheduling_follows_dependency_topology() {
    assert!(dense_repartition_graph_justifies_parallel_source_max(
        DENSE_REPARTITION_TOKENS_PER_RUN,
        1,
    ));
    assert!(!dense_repartition_graph_justifies_parallel_source_max(
        DENSE_REPARTITION_TOKENS_PER_RUN + 1,
        1,
    ));
    assert!(near_max_match_source_graph_is_cheap(
        NEAR_MAX_MATCH_MEAN_DECODED_PER_TOKEN
            * u64::try_from(COMPACT_SINGLE_SOURCE_ROUTE_MAX_TOKENS).unwrap(),
        COMPACT_SINGLE_SOURCE_ROUTE_MAX_TOKENS,
    ));
    assert!(!near_max_match_source_graph_is_cheap(
        NEAR_MAX_MATCH_MEAN_DECODED_PER_TOKEN
            * u64::try_from(COMPACT_SINGLE_SOURCE_ROUTE_MAX_TOKENS).unwrap()
            - 1,
        COMPACT_SINGLE_SOURCE_ROUTE_MAX_TOKENS,
    ));
    assert!(!near_max_match_source_graph_is_cheap(
        u64::MAX,
        COMPACT_SINGLE_SOURCE_ROUTE_MAX_TOKENS + 1,
    ));

    assert!(DefaultFloor::Complete.owns_terminal_stream_time());
    assert!(DefaultFloor::CompleteThenBounded.owns_terminal_stream_time());
    for shared in [
        DefaultFloor::Shared,
        DefaultFloor::SharedExact,
        DefaultFloor::ApngDefault,
        DefaultFloor::ApngMax,
        DefaultFloor::Established,
        DefaultFloor::MandatoryComplete,
    ] {
        assert!(!shared.owns_terminal_stream_time());
    }
    assert!(DefaultFloor::CompleteThenBounded.allows_parallel_source_follow_up());
    assert!(DefaultFloor::ApngMax.allows_parallel_source_follow_up());
    for serial in [
        DefaultFloor::Complete,
        DefaultFloor::Shared,
        DefaultFloor::SharedExact,
        DefaultFloor::ApngDefault,
        DefaultFloor::Established,
        DefaultFloor::MandatoryComplete,
    ] {
        assert!(!serial.allows_parallel_source_follow_up());
    }

    let block = ParsedBlock {
        tokens: Arc::new(vec![Token::Literal(0)]),
        plain: Arc::new(vec![0]),
        literal_frequencies: [0; 286],
        distance_frequencies: [0; 30],
        original_literal_lengths: None,
        original_distance_lengths: None,
        original_dynamic: None,
        original: None,
        source_splits: Vec::new(),
        source_type: SourceBlockType::Dynamic,
    };
    let compressed = [0_u8];
    let identity = StreamIdentity {
        decoded_size: 2,
        crc32: 0,
        adler32: 0,
    };
    let one_block = [block.clone()];
    let one_block_source = CandidateInput {
        compressed: &compressed,
        blocks: &one_block,
        meaningful_bits: 1,
        decoded_limit: 2,
        identity,
    };
    assert!(compact_parallel_source_max_work_class(one_block_source));
    assert!(compact_complementary_source_max_is_cheap(one_block_source));
    assert!(bounded_parallel_source_max_work_class(one_block_source));
    assert!(complete_png_parallel_source_max_work_class(
        one_block_source
    ));
    assert!(!compact_dependent_deft4j_work_class(one_block_source));

    let two_blocks = [block.clone(), block];
    let two_block_source = CandidateInput {
        compressed: &compressed,
        blocks: &two_blocks,
        meaningful_bits: 1,
        decoded_limit: 2,
        identity,
    };
    assert!(!compact_parallel_source_max_work_class(two_block_source));
    assert!(compact_complementary_source_max_is_cheap(two_block_source));
    assert!(bounded_parallel_source_max_work_class(two_block_source));
    assert!(complete_png_parallel_source_max_work_class(
        two_block_source
    ));
    assert!(compact_dependent_deft4j_work_class(two_block_source));
    assert!(!repartition_graph_covers_source_blocks(&two_blocks, 1));
    assert!(repartition_graph_covers_source_blocks(&two_blocks, 2));

    let large_token_block = ParsedBlock {
        tokens: Arc::new(vec![
            Token::Literal(0);
            COMPACT_COMPLEMENTARY_SOURCE_MAX_TOKENS + 1
        ]),
        ..two_blocks[0].clone()
    };
    let large_token_blocks = [large_token_block];
    let large_token_source = CandidateInput {
        compressed: &compressed,
        blocks: &large_token_blocks,
        meaningful_bits: 1,
        decoded_limit: 2,
        identity,
    };
    assert!(!compact_complementary_source_max_is_cheap(
        large_token_source
    ));
    let large_decoded_source = CandidateInput {
        identity: StreamIdentity {
            decoded_size: COMPACT_SPLIT_FLOOR_MAX_DECODED + 1,
            ..identity
        },
        ..large_token_source
    };
    assert!(!compact_parallel_source_max_work_class(
        large_decoded_source
    ));
    assert!(!bounded_parallel_source_max_work_class(
        large_decoded_source
    ));
    assert!(!complete_png_parallel_source_max_work_class(
        large_decoded_source
    ));

    let repartition_match = Token::Match {
        length: 130,
        distance: 1,
        length_symbol: 281,
        distance_symbol: 0,
    };
    let mut repartition_tokens = vec![Token::Literal(0); 2_049];
    repartition_tokens.extend([
        repartition_match,
        repartition_match,
        Token::Literal(0),
        repartition_match,
        repartition_match,
    ]);
    let repartition_block = ParsedBlock {
        tokens: Arc::new(repartition_tokens),
        ..large_token_blocks[0].clone()
    };
    let repartition_blocks = [repartition_block];
    let large_decoded_repartition_source = CandidateInput {
        blocks: &repartition_blocks,
        ..large_decoded_source
    };
    assert!(compact_parallel_source_max_work_class(
        large_decoded_repartition_source
    ));
    assert!(bounded_parallel_source_max_work_class(
        large_decoded_repartition_source
    ));
    assert!(!complete_png_parallel_source_max_work_class(
        large_decoded_repartition_source
    ));

    let mut dense_repartition_tokens = Vec::with_capacity(2_100);
    for _ in 0..700 {
        dense_repartition_tokens.extend([repartition_match, repartition_match, Token::Literal(0)]);
    }
    let dense_repartition_block = ParsedBlock {
        tokens: Arc::new(dense_repartition_tokens),
        ..large_token_blocks[0].clone()
    };
    let dense_repartition_blocks = [dense_repartition_block];
    let dense_repartition_source = CandidateInput {
        blocks: &dense_repartition_blocks,
        ..large_decoded_source
    };
    assert!(compact_parallel_source_max_work_class(
        dense_repartition_source
    ));
    assert!(complete_png_parallel_source_max_work_class(
        dense_repartition_source
    ));
    assert!(!compact_single_source_route_work_class(large_token_source));
    assert!(repartition_graph_covers_source_blocks(
        &large_token_blocks,
        1
    ));

    let single_route_token_block = ParsedBlock {
        tokens: Arc::new(vec![
            Token::Literal(0);
            COMPACT_SINGLE_SOURCE_ROUTE_MIN_TOKENS
        ]),
        ..large_token_blocks[0].clone()
    };
    let single_route_token_blocks = [single_route_token_block];
    let single_route_token_source = CandidateInput {
        compressed: &compressed,
        blocks: &single_route_token_blocks,
        meaningful_bits: 1,
        decoded_limit: 2,
        identity,
    };
    assert!(compact_single_source_route_work_class(
        single_route_token_source
    ));

    let oversized_token_block = ParsedBlock {
        tokens: Arc::new(vec![
            Token::Literal(0);
            COMPACT_SINGLE_SOURCE_ROUTE_MAX_TOKENS + 1
        ]),
        ..large_token_blocks[0].clone()
    };
    let oversized_token_blocks = [oversized_token_block];
    let oversized_token_source = CandidateInput {
        compressed: &compressed,
        blocks: &oversized_token_blocks,
        meaningful_bits: 1,
        decoded_limit: 2,
        identity,
    };
    assert!(!compact_single_source_route_work_class(
        oversized_token_source
    ));
    assert!(!repartition_graph_covers_source_blocks(
        &oversized_token_blocks,
        1
    ));
}
