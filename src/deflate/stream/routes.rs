// SPDX-License-Identifier: MIT
// SPDX-FileCopyrightText: 2026 Andrew C.E. Dent <https://github.com/ace-dent>

//! Whole-stream routes built from the planner's floors and walks.
//!
//! Each route returns a complete candidate that the optimizer keeps as its own
//! lineage. The no-split source routes (M2, M2c) keep source boundaries apart
//! from source-order merges and give each block a narrow search. The
//! proven-submatch and integrated-proven routes (D1, D2) keep resegmentation
//! from displacing a lineage whose replay ends better. The terminal merge route
//! merges a selected seed's neighbors with deterministic floors, and the
//! fragmented collection and replay (M11) serve highly fragmented streams.

use crate::deflate::block::{plan_block, plan_block_cached, CanonicalPlanCache};
use crate::deflate::model::{ParsedBlock, PlannedBlock, SourceBlockType};
use crate::deflate::search::{
    improve_plan_with_integrated_proven_floor, improve_plan_with_proven_submatches,
    improve_plan_with_short_family_floor, plan_block_with_complete_integrated_proven_search,
    proven_submatch_route_eligible,
};
use crate::deflate::stop::SearchStop;
use crate::Options;

use super::boundary_graph::source_aligned_huffman_floor_with_limit;
use super::compact_split::plan_compact_source_split_floor_cached;
use super::grouping::collect_huffman_runs_with_limits;
use super::sequential_walk::sequential_plan_with_source_search;
use super::{
    append_output_plan, direct_structural_plan, encoded_source_bytes, finish_plan,
    mandatory_token_floor_plan, prepare_blocks, total_bits, AdjacentMergeSearch, SourceBlockSearch,
    COLLECTED_RUN_MAX_PLAIN, DEFAULT_LONG_MERGE_MAX, DEFAULT_LONG_MERGE_MIN,
    FRAGMENTED_COLLECT_MAX_TOKENS, FRAGMENTED_COLLECT_MIN_SOURCE_BLOCKS,
    MAX_FRAGMENTED_REPLAY_BLOCKS, MAX_GREEDY_SOURCE_BLOCKS, MAX_REGROUP_SOURCE_BLOCKS,
};

/// Run the no-split source route (M2) with cumulative pruning.
pub(crate) fn plan_source_no_split_route(
    blocks: &[ParsedBlock],
    start_alignment: u8,
    options: &Options,
    stop: &mut SearchStop<'_>,
) -> Option<Vec<PlannedBlock>> {
    plan_source_no_split_route_with_pruning(blocks, start_alignment, options, false, stop)
}

/// Run [`plan_source_no_split_route`] with individual pruning, which changes
/// alignments and adjacent-merge prices, so the two are separate routes.
pub(crate) fn plan_source_individual_no_split_route(
    blocks: &[ParsedBlock],
    start_alignment: u8,
    options: &Options,
    stop: &mut SearchStop<'_>,
) -> Option<Vec<PlannedBlock>> {
    plan_source_no_split_route_with_pruning(blocks, start_alignment, options, true, stop)
}

fn plan_source_no_split_route_with_pruning(
    blocks: &[ParsedBlock],
    start_alignment: u8,
    options: &Options,
    individual_prune: bool,
    stop: &mut SearchStop<'_>,
) -> Option<Vec<PlannedBlock>> {
    let prepared = prepare_blocks(blocks);
    let blocks = prepared.as_deref().unwrap_or(blocks);
    let mut plan_cache = CanonicalPlanCache::new();
    let fallback = direct_structural_plan(blocks, start_alignment, options, &mut plan_cache)?;
    if stop.reached() {
        return Some(finish_plan(fallback, options));
    }

    let searched = sequential_plan_with_source_search(
        blocks,
        start_alignment,
        options,
        AdjacentMergeSearch::LongRun,
        SourceBlockSearch::Narrow { individual_prune },
        &mut plan_cache,
        stop,
        None,
    );
    match searched {
        Some(candidate) if total_bits(&candidate) < total_bits(&fallback) => {
            Some(finish_plan(candidate, options))
        }
        _ => Some(finish_plan(fallback, options)),
    }
}

/// Build the proven-submatch candidate as a separate lineage, so a locally
/// smaller resegmentation cannot displace a route whose replay ends better.
/// Returns `None` when no block improves.
pub(crate) fn plan_proven_submatch_route(
    blocks: &[ParsedBlock],
    start_alignment: u8,
    options: &Options,
    stop: &mut SearchStop<'_>,
) -> Option<Vec<PlannedBlock>> {
    if !blocks.iter().any(|block| {
        proven_submatch_route_eligible(&block.tokens, block.plain.len(), options.exhaustive)
    }) {
        return None;
    }

    let mut plans = Vec::new();
    plans.try_reserve_exact(blocks.len()).ok()?;
    let mut output_bits = 0_u64;
    let mut changed = false;
    let mut floor_options = options.clone();
    floor_options.exhaustive = false;

    for block in blocks {
        let alignment = ((u64::from(start_alignment) + output_bits) & 7) as u8;
        // The caller reparses the completed ordinary route first, so this cheap
        // plan branches from the selected table without repeating the ordinary
        // floor.
        let base = plan_block(block, alignment, &floor_options, &mut *stop);
        if !proven_submatch_route_eligible(&base.tokens, block.plain.len(), options.exhaustive) {
            append_output_plan(&mut plans, &mut output_bits, base, true)?;
            continue;
        }
        let base_bits = base.bits;
        let plan = improve_plan_with_proven_submatches(block, alignment, options, stop, base);
        let block_changed = plan.bits < base_bits;
        changed |= block_changed;
        let plan = if block_changed {
            improve_plan_with_short_family_floor(block, &floor_options, plan)
        } else {
            plan
        };
        append_output_plan(&mut plans, &mut output_bits, plan, true)?;
    }

    changed.then(|| finish_plan(plans, options))
}

/// Build the proven-before-feedback floor as a separate route, so its locally
/// smaller spelling cannot hide the ordinary replay's fixed point.
pub(crate) fn plan_integrated_proven_source_route(
    blocks: &[ParsedBlock],
    start_alignment: u8,
    options: &Options,
    stop: &mut SearchStop<'_>,
) -> Option<Vec<PlannedBlock>> {
    let mut plans = Vec::new();
    plans.try_reserve_exact(blocks.len()).ok()?;
    let mut output_bits = 0_u64;
    let mut floor_options = options.clone();
    floor_options.exhaustive = false;
    let mut plan_cache = CanonicalPlanCache::new();

    for block in blocks {
        if stop.reached() {
            return None;
        }
        let alignment = ((u64::from(start_alignment) + output_bits) & 7) as u8;
        let base = plan_block_cached(block, alignment, &floor_options, &mut plan_cache);
        let base =
            improve_plan_with_integrated_proven_floor(block, alignment, &floor_options, true, base);
        let base = improve_plan_with_short_family_floor(block, &floor_options, base);
        let plan = plan_block_with_complete_integrated_proven_search(
            block,
            alignment,
            &floor_options,
            base,
            &mut *stop,
        );
        append_output_plan(&mut plans, &mut output_bits, plan, true)?;
    }

    Some(finish_plan(plans, options))
}

/// Greedily merge adjacent blocks of a selected Huffman seed using
/// deterministic floors. The work is linear in the block count, so it may
/// finish after the main deadline.
pub(crate) fn plan_terminal_merge_route(
    blocks: &[ParsedBlock],
    start_alignment: u8,
    options: &Options,
    stop: &mut SearchStop<'_>,
) -> Option<Vec<PlannedBlock>> {
    if blocks.len() < 2 || blocks.len() > MAX_GREEDY_SOURCE_BLOCKS || stop.reached() {
        return None;
    }
    let mut plan_cache = CanonicalPlanCache::new();
    let fallback = direct_structural_plan(blocks, start_alignment, options, &mut plan_cache)?;
    let candidate = sequential_plan_with_source_search(
        blocks,
        start_alignment,
        options,
        AdjacentMergeSearch::LongRun,
        SourceBlockSearch::Floor,
        &mut plan_cache,
        stop,
        None,
    )?;
    (total_bits(&candidate) < total_bits(&fallback)).then(|| finish_plan(candidate, options))
}

/// Bounded replay planner for the fragmented collection (M11): recode each
/// block, price source-boundary merges and test decoded-eighth splits. Repeated
/// replays reach nested cuts.
pub(crate) fn plan_fragmented_replay(
    blocks: &[ParsedBlock],
    start_alignment: u8,
    options: &Options,
) -> Option<Vec<PlannedBlock>> {
    if blocks.is_empty()
        || blocks.len() > MAX_FRAGMENTED_REPLAY_BLOCKS
        || blocks
            .iter()
            .any(|block| block.source_type == SourceBlockType::Stored)
    {
        return None;
    }

    let mut plan_cache = CanonicalPlanCache::new();
    let mut best = mandatory_token_floor_plan(blocks, start_alignment, options, &mut plan_cache)?;
    if let Some(grouped) = source_aligned_huffman_floor_with_limit(
        blocks,
        start_alignment,
        options,
        MAX_FRAGMENTED_REPLAY_BLOCKS,
        &mut plan_cache,
    ) {
        if total_bits(&grouped) < total_bits(&best) {
            best = grouped;
        }
    }
    if let Some(split) = plan_compact_source_split_floor_cached(
        blocks,
        start_alignment,
        options,
        &mut plan_cache,
        false,
        &mut SearchStop::never(),
    ) {
        if split.len() <= MAX_FRAGMENTED_REPLAY_BLOCKS && total_bits(&split) < total_bits(&best) {
            best = split;
        }
    }
    Some(best)
}

/// Build the 4,096-token collected candidate (M11). It is replayed as its own
/// lineage because on highly fragmented streams it can overtake the 8,192-token
/// floor after several replays. `stream_bytes` is the encoded length of the
/// stream `blocks` came from.
pub(crate) fn fragmented_collect_seed(
    blocks: &[ParsedBlock],
    stream_bytes: usize,
    start_alignment: u8,
    options: &Options,
) -> Option<Vec<PlannedBlock>> {
    if !options.exhaustive
        || blocks.len() < FRAGMENTED_COLLECT_MIN_SOURCE_BLOCKS
        || !(DEFAULT_LONG_MERGE_MIN..=DEFAULT_LONG_MERGE_MAX)
            .contains(&encoded_source_bytes(blocks, stream_bytes))
        || blocks
            .iter()
            .any(|block| block.source_type == SourceBlockType::Stored)
    {
        return None;
    }

    let collected = collect_huffman_runs_with_limits(
        blocks,
        FRAGMENTED_COLLECT_MAX_TOKENS,
        COLLECTED_RUN_MAX_PLAIN,
    )?;
    if collected.len() >= blocks.len()
        || !(2..=MAX_REGROUP_SOURCE_BLOCKS).contains(&collected.len())
    {
        return None;
    }
    let mut plan_cache = CanonicalPlanCache::new();
    direct_structural_plan(&collected, start_alignment, options, &mut plan_cache)
}

#[cfg(test)]
mod tests;
