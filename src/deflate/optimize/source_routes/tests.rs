// SPDX-License-Identifier: MIT
// SPDX-FileCopyrightText: 2026 Andrew C.E. Dent <https://github.com/ace-dent>

use std::sync::Arc;

use super::super::test_support::comparison_candidate;
use super::super::NARROW_SOURCE_LIST_MAX_BLOCKS;
use super::*;
use crate::deflate::model::{PlannedBlock, Representation, SourceBlockType, Token};

#[test]
fn changed_parent_no_split_requires_a_new_best_topology() {
    let parent = comparison_candidate(12, 90, 1);
    let no_split = comparison_candidate(10, 80, 2);
    let refined = comparison_candidate(9, 79, 3);

    assert!(changed_parent_no_split_should_continue(
        true,
        &refined,
        &parent,
        Some(&no_split),
    ));
    assert!(!changed_parent_no_split_should_continue(
        false,
        &refined,
        &parent,
        Some(&no_split),
    ));
    assert!(!changed_parent_no_split_should_continue(
        true,
        &no_split,
        &parent,
        Some(&refined),
    ));
    assert!(!changed_parent_no_split_should_continue(
        true, &refined, &parent, None,
    ));
}

#[test]
fn changed_narrow_parent_continues_after_a_source_improvement() {
    let compressed = [0_u8; 12];
    let source = CandidateInput {
        compressed: &compressed,
        blocks: &[],
        meaningful_bits: 90,
        decoded_limit: 0,
        identity: StreamIdentity {
            decoded_size: 0,
            crc32: 0,
            adler32: 0,
        },
    };
    let narrow = comparison_candidate(10, 80, 1);

    assert!(changed_narrow_parent_should_continue(true, &narrow, source));
    assert!(!changed_narrow_parent_should_continue(
        false, &narrow, source
    ));

    let unchanged = comparison_candidate(12, 90, 5);
    assert!(!changed_narrow_parent_should_continue(
        true, &unchanged, source
    ));
}

#[test]
fn no_split_policy_prioritizes_cumulative_work_for_long_lists() {
    assert!(!cumulative_no_split_count_has_priority(0));
    assert!(!cumulative_no_split_count_has_priority(3));
    assert!(cumulative_no_split_count_has_priority(4));
    assert!(cumulative_no_split_count_has_priority(
        NARROW_SOURCE_LIST_MAX_BLOCKS,
    ));
}

#[test]
fn no_split_replay_requires_a_new_parent_state() {
    let compressed = [0_u8];
    let source = CandidateInput {
        compressed: &compressed,
        blocks: &[],
        meaningful_bits: 8,
        decoded_limit: 0,
        identity: StreamIdentity {
            decoded_size: 0,
            crc32: 0,
            adler32: 0,
        },
    };
    let mut candidate = comparison_candidate(1, 7, 1);

    // Different output bytes alone can be only a better Huffman header;
    // Max already priced that token state, so Default must not repeat it.
    assert!(!candidate_exposes_new_parent(&candidate, source));

    candidate.plans.push(PlannedBlock {
        tokens: Arc::new(vec![Token::Literal(0)]),
        plain: Arc::new(vec![0]),
        representation: Representation::Fixed,
        bits: 10,
        source_type: SourceBlockType::Fixed,
    });
    assert!(candidate_exposes_new_parent(&candidate, source));

    candidate.data.copy_from_slice(&compressed);
    assert!(!candidate_exposes_new_parent(&candidate, source));
}
