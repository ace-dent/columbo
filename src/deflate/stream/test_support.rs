// SPDX-License-Identifier: MIT
// SPDX-FileCopyrightText: 2026 Andrew C.E. Dent <https://github.com/ace-dent>

//! Block fixtures and planner shortcuts shared by the stream planner's tests.

use super::{sequential_plan_with_compact_policy, AdjacentMergeSearch};
use crate::deflate::block::CanonicalPlanCache;
use crate::deflate::model::{
    canonical_length_encoding, count_frequencies, ParsedBlock, PlannedBlock, Representation,
    SourceBlockType, Token,
};
use crate::deflate::stop::SearchStop;
use crate::progress::RouteProgress;
use crate::Options;

/// Build the ordinary per-block result, joining consecutive fixed plans as
/// DeflOpt does (each join saves ten bits).
pub(super) fn sequential_plan(
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

pub(super) fn literal_block(bytes: &[u8], source_type: SourceBlockType) -> ParsedBlock {
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

pub(super) fn overlapping_match_block(length: u16) -> ParsedBlock {
    let (length_symbol, _, _) =
        canonical_length_encoding(length).expect("a legal Deflate match length");
    let tokens = vec![
        Token::Literal(b'a'),
        Token::Match {
            length,
            distance: 1,
            length_symbol,
            distance_symbol: 0,
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

pub(super) fn assert_same_plan(left: &PlannedBlock, right: &PlannedBlock) {
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
