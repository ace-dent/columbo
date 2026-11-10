// SPDX-License-Identifier: MIT
// SPDX-FileCopyrightText: 2026 Andrew C.E. Dent <https://github.com/ace-dent>

//! Route gates: the size, topology and work-class rules that decide whether a
//! route runs, and whether it may share a phase with other routes. The
//! schedule and the route builders consult them before starting work.
//! `docs/routes-and-methods.md` gives their limits: M0–M10 in its route-gate
//! reference, and D1–D4 in each method's own entry.

use super::{
    CandidateInput, DefaultFloor, COMPACT_SPLIT_FLOOR_MAX_BLOCKS,
    COMPACT_SPLIT_FLOOR_MAX_COMPRESSED, COMPACT_SPLIT_FLOOR_MAX_DECODED,
    COMPACT_SPLIT_FLOOR_MAX_TOKENS, COMPACT_TREE_MAX_COMPRESSED, COMPACT_TREE_MAX_DECODED,
    COMPACT_TREE_MAX_TOKENS, CONCURRENT_BOUNDED_FLOOR_MAX_DECODED, NARROW_SOURCE_LIST_MAX_BLOCKS,
    NARROW_SOURCE_MAX_COMPRESSED, PREBUILD_BOUNDED_FLOOR_MAX_DECODED,
};
use crate::deflate::model::{same_tokens, ParsedBlock, PlannedBlock, SourceBlockType};
use crate::deflate::parse::parsed_model_bytes;
use crate::deflate::search::{compact_proven_submatch_route_eligible, same_distance_opportunities};

/// A multi-block direct deft4j gain below this (2%) counts as weak.
pub(super) const WEAK_DEFT4J_GAIN_BASIS_POINTS: u64 = 200;
/// Smallest decoded stream for Default's source-topology route (D4). Below
/// it, the build costs more than its rare gains, as on APNG frames.
const DEFAULT_SOURCE_TOPOLOGY_MIN_DECODED: u64 = 128 * 1024;
/// A large one-block source max overlaps the completed PNG floor only with at
/// least one repartition run per this many source tokens, since both compete
/// for cache and memory bandwidth.
const DENSE_REPARTITION_TOKENS_PER_RUN: usize = 16;
/// M9: up to this many tokens, source max also runs beside M3.
const COMPACT_COMPLEMENTARY_SOURCE_MAX_TOKENS: usize = 2_048;
/// M9: token band in which one source-max worker may replace two long workers
/// on the same graph.
const COMPACT_SINGLE_SOURCE_ROUTE_MAX_TOKENS: usize = 4_000;
const COMPACT_SINGLE_SOURCE_ROUTE_MIN_TOKENS: usize =
    COMPACT_SINGLE_SOURCE_ROUTE_MAX_TOKENS * 5 / 8;
/// Mean decoded bytes per token (7/8 of 256) at or above which a graph of at
/// most 4,000 tokens consists almost entirely of long matches.
const NEAR_MAX_MATCH_MEAN_DECODED_PER_TOKEN: u64 = 7 * 32;
/// Limits for running routes in parallel (M5), so peak memory does not grow
/// with every route's own budget.
const PARALLEL_ROUTE_MAX_COMPRESSED: usize = 8 * 1_024 * 1_024;
const PARALLEL_ROUTE_MAX_DECODED: u64 = 64 * 1_024 * 1_024;
pub(super) const PARALLEL_ROUTE_MAX_MODEL: usize = 64 * 1_024 * 1_024;

pub(super) fn deft4j_source_route_eligible(blocks: &[ParsedBlock]) -> bool {
    let mut nonempty = 0_usize;
    let mut huffman = 0_usize;
    for block in blocks.iter().filter(|block| !block.plain.is_empty()) {
        nonempty += 1;
        if matches!(
            block.source_type,
            SourceBlockType::Fixed | SourceBlockType::Dynamic
        ) {
            huffman += 1;
        }
    }
    // A lone stored block gives the deft4j graph nothing to improve.
    nonempty >= 2 || (nonempty == 1 && huffman == 1)
}

/// D4: Default prices the source's own block layout, each block searched from
/// the smallest of its ordinary plan, length-family chain and re-spell from
/// literals, for multi-block sources in the no-split route's class (M2) that
/// decode to at least [`DEFAULT_SOURCE_TOPOLOGY_MIN_DECODED`] bytes.
pub(super) fn default_source_topology_eligible(
    blocks: &[ParsedBlock],
    compressed_len: usize,
    decoded_size: u64,
) -> bool {
    decoded_size >= DEFAULT_SOURCE_TOPOLOGY_MIN_DECODED
        && narrow_source_route_eligible(blocks, compressed_len)
}

pub(super) fn narrow_source_route_eligible(blocks: &[ParsedBlock], compressed_len: usize) -> bool {
    compressed_len <= NARROW_SOURCE_MAX_COMPRESSED
        && (2..=NARROW_SOURCE_LIST_MAX_BLOCKS).contains(
            &blocks
                .iter()
                .filter(|block| !block.plain.is_empty())
                .count(),
        )
        && blocks.iter().all(|block| {
            block.plain.is_empty()
                || matches!(
                    block.source_type,
                    SourceBlockType::Fixed | SourceBlockType::Dynamic
                )
        })
}

pub(super) fn compact_balanced_tree_source_eligible(
    compressed_len: usize,
    decoded_size: u64,
    blocks: &[ParsedBlock],
) -> bool {
    if compressed_len > COMPACT_TREE_MAX_COMPRESSED || decoded_size > COMPACT_TREE_MAX_DECODED {
        return false;
    }
    let mut nonempty = blocks.iter().filter(|block| !block.plain.is_empty());
    let Some(block) = nonempty.next() else {
        return false;
    };
    nonempty.next().is_none()
        && block.source_type == SourceBlockType::Dynamic
        && block.tokens.len() <= COMPACT_TREE_MAX_TOKENS
}

pub(super) fn compact_strict_literal_tree_eligible(
    compressed_len: usize,
    blocks: &[ParsedBlock],
) -> bool {
    if compressed_len > COMPACT_TREE_MAX_COMPRESSED {
        return false;
    }
    let mut nonempty = blocks.iter().filter(|block| !block.plain.is_empty());
    let Some(block) = nonempty.next() else {
        return false;
    };
    nonempty.next().is_none()
        && block.source_type == SourceBlockType::Dynamic
        && block.tokens.len() <= COMPACT_TREE_MAX_TOKENS
        && block
            .distance_frequencies
            .iter()
            .all(|&frequency| frequency == 0)
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(super) enum BoundedPngMaxPolicy {
    #[default]
    Standard,
    FloorExpansion,
    GenericParallel,
}

/// Choose the bounded PNG route family (M6).
pub(super) fn bounded_png_max_policy(
    nonempty_blocks: usize,
    floor_exposes_new_states: bool,
    deft4j_eligible: bool,
    narrow_eligible: bool,
) -> BoundedPngMaxPolicy {
    if !deft4j_eligible && !narrow_eligible {
        BoundedPngMaxPolicy::GenericParallel
    } else if nonempty_blocks >= 2 || floor_exposes_new_states {
        BoundedPngMaxPolicy::FloorExpansion
    } else {
        BoundedPngMaxPolicy::Standard
    }
}

/// Whether the completed floor changed a nonempty block boundary or token
/// spelling, giving Max a seed that source-order search may not reach before
/// its deadline.
pub(super) fn floor_exposes_new_search_states(
    source: &[ParsedBlock],
    floor: &[PlannedBlock],
) -> bool {
    let mut source = source.iter().filter(|block| !block.plain.is_empty());
    let mut floor = floor.iter().filter(|block| !block.plain.is_empty());
    loop {
        match (source.next(), floor.next()) {
            (Some(source_block), Some(floor_block)) => {
                if source_block.plain.len() != floor_block.plain.len()
                    || !same_tokens(&source_block.tokens, &floor_block.tokens)
                {
                    return true;
                }
            }
            (None, None) => return false,
            _ => return true,
        }
    }
}

pub(super) fn has_multiple_nonempty_blocks(blocks: &[ParsedBlock]) -> bool {
    blocks
        .iter()
        .filter(|block| !block.plain.is_empty())
        .take(2)
        .count()
        == 2
}

pub(super) fn source_run_match_count_exceeds(blocks: &[ParsedBlock], limit: usize) -> bool {
    same_distance_opportunities(blocks).matches > limit
}

pub(super) fn prebuild_bounded_floor(nonempty_blocks: usize, decoded_size: u64) -> bool {
    nonempty_blocks <= 1
        || decoded_size <= PREBUILD_BOUNDED_FLOOR_MAX_DECODED
        || decoded_size > CONCURRENT_BOUNDED_FLOOR_MAX_DECODED
}

pub(super) fn gain_is_below(source_bits: u64, candidate_bits: u64, basis_points: u64) -> bool {
    let saved = source_bits.saturating_sub(candidate_bits);
    saved.saturating_mul(10_000) < source_bits.saturating_mul(basis_points)
}

/// Whether the floor-seeded continuation may run before a pending compact
/// split. If they cannot overlap, it goes first only when it saves at least 2%
/// over the floor, so it cannot starve the split.
pub(super) fn floor_seeded_priority_with_structural_sibling(
    floor_bits: u64,
    seeded_bits: u64,
    compact_split_pending: bool,
    compact_split_can_overlap: bool,
) -> bool {
    !compact_split_pending
        || compact_split_can_overlap
        || !gain_is_below(floor_bits, seeded_bits, WEAK_DEFT4J_GAIN_BASIS_POINTS)
}

/// Whether the deft4j refinement runs beside the floor-seeded continuation.
/// Only a stream with the file-level route window and bounded parallel work
/// can hold both expanded models.
pub(super) fn independent_deft4j_refinement_can_overlap(
    continuing_floor_seed: bool,
    owns_file_level_route_window: bool,
    parallel_work_is_bounded: bool,
    refinement_is_pending: bool,
) -> bool {
    continuing_floor_seed
        && owns_file_level_route_window
        && parallel_work_is_bounded
        && refinement_is_pending
}

/// Where original-source Max runs while the floor-seeded continuation (M7a)
/// may use the rest of the primary window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ContinuationSourceMax {
    /// Not beside the continuation; the follow-up or M12 may still run it.
    Later,
    /// Beside the continuation, in place of its other siblings (`ApngMax`).
    InsteadOfSiblings,
    /// Beside the continuation and its other siblings (`CompleteThenBounded`).
    BesideSiblings,
}

/// A continuation can use the rest of the primary window, and source max
/// (M12) would then never start, so a policy that already runs source max
/// beside other work starts it beside the continuation too. A single PNG image
/// does so only within G0t's search share: a source-max win replaces the
/// continued lineage, and only reserved terminal time lets M14 still finish
/// it. Outside that share only the incumbent gets the terminal stage, so the
/// win could move the endpoint either way. `pending` holds when the
/// continuation runs, M5 allows workers, no source-max candidate exists or is
/// marked as run, and a route may start.
pub(super) fn continuation_source_max(
    default_floor: DefaultFloor,
    terminal_search_share: bool,
    pending: bool,
) -> ContinuationSourceMax {
    if !pending || !default_floor.allows_parallel_source_follow_up() {
        ContinuationSourceMax::Later
    } else if default_floor == DefaultFloor::ApngMax {
        ContinuationSourceMax::InsteadOfSiblings
    } else if terminal_search_share {
        ContinuationSourceMax::BesideSiblings
    } else {
        ContinuationSourceMax::Later
    }
}

pub(super) fn compact_source_has_bounded_match_preserving_feedback(
    source: CandidateInput<'_>,
) -> bool {
    let [block] = source.blocks else {
        return false;
    };
    compact_proven_submatch_route_eligible(&block.tokens, block.plain.len())
}

pub(super) fn compact_source_has_bounded_integrated_proven_feedback(
    source: CandidateInput<'_>,
) -> bool {
    if !(2..=COMPACT_SPLIT_FLOOR_MAX_BLOCKS).contains(&source.blocks.len())
        || source.compressed.len() > COMPACT_SPLIT_FLOOR_MAX_COMPRESSED
        || source.identity.decoded_size > COMPACT_SPLIT_FLOOR_MAX_DECODED
    {
        return false;
    }
    let Some(token_count) = source.blocks.iter().try_fold(0_usize, |total, block| {
        total.checked_add(block.tokens.len())
    }) else {
        return false;
    };
    token_count <= COMPACT_SPLIT_FLOOR_MAX_TOKENS
        && source
            .blocks
            .iter()
            .any(|block| compact_proven_submatch_route_eligible(&block.tokens, block.plain.len()))
}

/// Whether original-source Max and floor-seeded Max can share the initial
/// phase. Within the compact split limits the overlap's memory and cache cost
/// is small; larger streams run serially so two beams pricing many payload
/// ranges do not starve the floor.
fn compact_parallel_source_max_work_class(source: CandidateInput<'_>) -> bool {
    if source.compressed.len() > COMPACT_SPLIT_FLOOR_MAX_COMPRESSED {
        return false;
    }
    let nonempty_blocks = source
        .blocks
        .iter()
        .filter(|block| !block.plain.is_empty())
        .count();
    if nonempty_blocks == 1 {
        return source.identity.decoded_size <= COMPACT_SPLIT_FLOOR_MAX_DECODED
            || (source_token_count(source)
                .is_some_and(|tokens| tokens <= COMPACT_SPLIT_FLOOR_MAX_TOKENS)
                && same_distance_opportunities(source.blocks).repartition_runs >= 2);
    }
    let decoded_limit = u64::try_from(nonempty_blocks)
        .ok()
        .and_then(|blocks| COMPACT_SPLIT_FLOOR_MAX_DECODED.checked_mul(blocks));
    (2..=COMPACT_SPLIT_FLOOR_MAX_BLOCKS).contains(&nonempty_blocks)
        && decoded_limit.is_some_and(|limit| source.identity.decoded_size <= limit)
        && source_token_count(source).is_some_and(|tokens| tokens <= COMPACT_SPLIT_FLOOR_MAX_TOKENS)
        && same_distance_opportunities(source.blocks).repartition_runs >= 2
}

/// Whether source max is cheap enough to overlap the bounded floor routes.
/// Only source max combines the choices of a dense repartition graph, and on
/// tiny graphs it cheaply tests original block merges that a rewritten floor
/// cannot reconstruct.
fn bounded_parallel_source_max_work_class(source: CandidateInput<'_>) -> bool {
    compact_complementary_source_max_is_cheap(source)
        || compact_parallel_source_max_work_class(source)
}

/// Whether a source-max beam should overlap the completed single-image PNG
/// floor (M8). A large one-block source overlaps only when its graph is cheap,
/// since otherwise both beams price many of the same payload ranges.
pub(super) fn complete_png_parallel_source_max_work_class(source: CandidateInput<'_>) -> bool {
    let nonempty_blocks = source
        .blocks
        .iter()
        .filter(|block| !block.plain.is_empty())
        .count();
    if nonempty_blocks != 1 || source.identity.decoded_size <= COMPACT_SPLIT_FLOOR_MAX_DECODED {
        return bounded_parallel_source_max_work_class(source);
    }

    let Some(token_count) = source_token_count(source) else {
        return false;
    };
    if token_count <= COMPACT_COMPLEMENTARY_SOURCE_MAX_TOKENS {
        return true;
    }

    source.compressed.len() <= COMPACT_SPLIT_FLOOR_MAX_COMPRESSED
        && token_count <= COMPACT_SPLIT_FLOOR_MAX_TOKENS
        && (dense_repartition_graph_justifies_parallel_source_max(
            token_count,
            same_distance_opportunities(source.blocks).repartition_runs,
        ) || near_max_match_source_graph_is_cheap(source.identity.decoded_size, token_count))
}

fn dense_repartition_graph_justifies_parallel_source_max(
    token_count: usize,
    repartition_runs: usize,
) -> bool {
    repartition_runs.saturating_mul(DENSE_REPARTITION_TOKENS_PER_RUN) >= token_count
}

fn near_max_match_source_graph_is_cheap(decoded_size: u64, token_count: usize) -> bool {
    token_count <= COMPACT_SINGLE_SOURCE_ROUTE_MAX_TOKENS
        && u64::try_from(token_count)
            .ok()
            .and_then(|tokens| tokens.checked_mul(NEAR_MAX_MATCH_MEAN_DECODED_PER_TOKEN))
            .is_some_and(|minimum_decoded| decoded_size >= minimum_decoded)
}

pub(super) fn compact_complementary_source_max_is_cheap(source: CandidateInput<'_>) -> bool {
    source_token_count(source)
        .is_some_and(|tokens| tokens <= COMPACT_COMPLEMENTARY_SOURCE_MAX_TOKENS)
}

pub(super) fn compact_single_source_route_work_class(source: CandidateInput<'_>) -> bool {
    source_token_count(source).is_some_and(|tokens| {
        (COMPACT_SINGLE_SOURCE_ROUTE_MIN_TOKENS..=COMPACT_SINGLE_SOURCE_ROUTE_MAX_TOKENS)
            .contains(&tokens)
    })
}

fn source_token_count(source: CandidateInput<'_>) -> Option<usize> {
    source.blocks.iter().try_fold(0_usize, |total, block| {
        total.checked_add(block.tokens.len())
    })
}

pub(super) fn repartition_graph_covers_source_blocks(
    blocks: &[ParsedBlock],
    repartition_runs: usize,
) -> bool {
    let mut nonempty_blocks = 0_usize;
    for block in blocks.iter().filter(|block| !block.plain.is_empty()) {
        nonempty_blocks += 1;
        // Source max combines choices across blocks only after finishing each
        // block, and a block above 4,000 tokens can use the whole phase.
        if block.tokens.len() > COMPACT_SINGLE_SOURCE_ROUTE_MAX_TOKENS {
            return false;
        }
    }
    nonempty_blocks != 0 && repartition_runs >= nonempty_blocks
}

/// Whether the direct deft4j parent can cheaply finish its dependent split.
pub(super) fn compact_dependent_deft4j_work_class(source: CandidateInput<'_>) -> bool {
    let Some(token_count) = source.blocks.iter().try_fold(0_usize, |total, block| {
        total.checked_add(block.tokens.len())
    }) else {
        return false;
    };
    source.compressed.len() <= COMPACT_SPLIT_FLOOR_MAX_COMPRESSED
        && source.identity.decoded_size <= COMPACT_SPLIT_FLOOR_MAX_DECODED
        && token_count <= COMPACT_SPLIT_FLOOR_MAX_TOKENS
        && has_multiple_nonempty_blocks(source.blocks)
}

pub(super) fn parallel_route_is_bounded(source: CandidateInput<'_>) -> bool {
    let Some(token_count) = source.blocks.iter().try_fold(0_usize, |total, block| {
        total.checked_add(block.tokens.len())
    }) else {
        return false;
    };
    parallel_route_sizes_are_bounded(
        source.compressed.len(),
        source.identity.decoded_size,
        token_count,
        source.blocks.len(),
    )
}

fn parallel_route_sizes_are_bounded(
    compressed_bytes: usize,
    decoded_bytes: u64,
    token_count: usize,
    block_count: usize,
) -> bool {
    if compressed_bytes > PARALLEL_ROUTE_MAX_COMPRESSED
        || decoded_bytes > PARALLEL_ROUTE_MAX_DECODED
    {
        return false;
    }
    usize::try_from(decoded_bytes)
        .ok()
        .and_then(|decoded| parsed_model_bytes(decoded, token_count, block_count))
        .is_some_and(|model| model <= PARALLEL_ROUTE_MAX_MODEL)
}

#[cfg(test)]
mod tests;
