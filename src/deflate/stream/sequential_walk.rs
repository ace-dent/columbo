// SPDX-License-Identifier: MIT
// SPDX-FileCopyrightText: 2026 Andrew C.E. Dent <https://github.com/ace-dent>

//! Plan blocks in order with per-source search and adjacent merges.
//!
//! The walk behind the source-order search, the selected grouped layout, the
//! collected replay and the no-split and terminal merge routes holds a pending
//! block. It plans that block and the next at their running alignments with
//! the walk's per-source search, then weighs the pair against a fixed/fixed
//! join, a searched merge and the pair's shared source tree, as the merge mode
//! allows. The strict winner becomes the new pending block, which may absorb
//! its next neighbor too.

use std::sync::Arc;

use crate::deflate::block::{
    lookup_block_cached, plan_block, plan_block_cached, CanonicalPlanCache,
};
use crate::deflate::header::score_existing_dynamic;
use crate::deflate::model::{ParsedBlock, PlannedBlock, Representation, SourceBlockType};
use crate::deflate::search::{
    improve_plan_with_floor, plan_block_with_narrow_search, plan_block_with_seeded_narrow_search,
};
use crate::deflate::source_recode::plan_source_block;
use crate::deflate::stop::SearchStop;
use crate::progress::RouteProgress;
use crate::Options;

use super::source_split::{plan_source_with_splits, DeferredSplits};
use super::{
    append_output_plans, is_fixed_plan, total_bits, try_merge_parsed_blocks, AdjacentMergeSearch,
    PendingBlock, SourceBlockSearch, MAX_MERGED_PLAIN, MAX_MERGED_TOKENS,
};

/// Build the source-order result with full per-source search.
/// `integrated_compact_proven` selects the compact Max state order; it changes
/// search order only.
#[allow(clippy::too_many_arguments)]
pub(super) fn sequential_plan_with_compact_policy(
    blocks: &[ParsedBlock],
    start_alignment: u8,
    options: &Options,
    merge_search: AdjacentMergeSearch,
    integrated_compact_proven: bool,
    plan_cache: &mut CanonicalPlanCache,
    stop: &mut SearchStop<'_>,
    progress: Option<&RouteProgress>,
) -> Option<Vec<PlannedBlock>> {
    sequential_plan_with_source_search(
        blocks,
        start_alignment,
        options,
        merge_search,
        SourceBlockSearch::Full {
            integrated_compact_proven,
        },
        plan_cache,
        stop,
        progress,
    )
}

/// Walk the blocks in order. A Max walk with full per-source search over two
/// or more blocks walks twice: the first walk gives every block its
/// whole-block search and its first split refinements, and the second resumes
/// each block's remaining refinements ([`DeferredSplits`]), so a deadline
/// cuts the least valuable work instead of leaving later blocks unsearched.
/// The second walk replaces the first only with strictly fewer bits.
#[allow(clippy::too_many_arguments)]
pub(super) fn sequential_plan_with_source_search(
    blocks: &[ParsedBlock],
    start_alignment: u8,
    options: &Options,
    merge_search: AdjacentMergeSearch,
    source_search: SourceBlockSearch,
    plan_cache: &mut CanonicalPlanCache,
    stop: &mut SearchStop<'_>,
    progress: Option<&RouteProgress>,
) -> Option<Vec<PlannedBlock>> {
    let defers = options.exhaustive
        && blocks.len() >= 2
        && matches!(source_search, SourceBlockSearch::Full { .. });
    let mut deferred = DeferredSplits::default();
    let mut best: Option<Vec<PlannedBlock>> = None;
    loop {
        let walked = walk_source_blocks(
            blocks,
            start_alignment,
            options,
            merge_search,
            source_search,
            plan_cache,
            defers.then_some(&mut deferred),
            stop,
            progress,
        );
        let Some(first) = best.take() else {
            let first = walked?;
            if !defers || stop.reached() || !deferred.has_unrefined() {
                return Some(first);
            }
            deferred.resume();
            best = Some(first);
            continue;
        };
        return Some(match walked {
            Some(second) if total_bits(&second) < total_bits(&first) => second,
            _ => first,
        });
    }
}

#[inline(never)]
#[allow(clippy::too_many_arguments)]
fn walk_source_blocks(
    blocks: &[ParsedBlock],
    start_alignment: u8,
    options: &Options,
    merge_search: AdjacentMergeSearch,
    source_search: SourceBlockSearch,
    plan_cache: &mut CanonicalPlanCache,
    mut deferred: Option<&mut DeferredSplits>,
    stop: &mut SearchStop<'_>,
    progress: Option<&RouteProgress>,
) -> Option<Vec<PlannedBlock>> {
    let Some((first, rest)) = blocks.split_first() else {
        return Some(Vec::new());
    };
    let mut output = Vec::<PlannedBlock>::new();
    output.try_reserve_exact(blocks.len()).ok()?;
    let mut output_bits = 0_u64;
    let mut pending = PendingBlock::Borrowed(first);
    let mut pending_cache: Option<(u8, Vec<PlannedBlock>)> = None;

    for (index, current) in rest.iter().enumerate() {
        let pending_block = pending.as_block();
        let alignment = ((u64::from(start_alignment) + output_bits) & 7) as u8;
        if let Some(progress) = progress {
            progress.item(index + 1, pending_block.tokens.len());
            progress.activity("Columbo token and split search");
        }
        let pending_plans = match pending_cache.take() {
            Some((cached_alignment, plans)) if cached_alignment == alignment => plans,
            _ => plan_source_with_search(
                pending_block,
                alignment,
                options,
                source_search,
                plan_cache,
                deferred.as_deref_mut(),
                stop,
            ),
        };
        let pending_bits = total_bits(&pending_plans);
        let current_alignment = ((u64::from(alignment) + pending_bits) & 7) as u8;
        if let Some(progress) = progress {
            progress.item(index + 2, current.tokens.len());
            progress.activity("Columbo token and split search");
        }
        let current_plans = plan_source_with_search(
            current,
            current_alignment,
            options,
            source_search,
            plan_cache,
            deferred.as_deref_mut(),
            stop,
        );
        let separate_bits = pending_bits + total_bits(&current_plans);

        let shared_tree = blocks_share_dynamic_tree(pending_block, current);
        let small_merge = pending_block.plain.len() + current.plain.len() <= 512;
        let merge_search_enabled = merge_search != AdjacentMergeSearch::Disabled;
        let long_huffman_merge = merge_search == AdjacentMergeSearch::LongRun
            && pending_block.source_type != SourceBlockType::Stored
            && current.source_type != SourceBlockType::Stored;
        let can_search_merge = merge_search_enabled
            && (small_merge || options.exhaustive || long_huffman_merge)
            && pending_block.tokens.len() + current.tokens.len() <= MAX_MERGED_TOKENS
            && pending_block.plain.len() + current.plain.len() <= MAX_MERGED_PLAIN
            && !stop.reached();

        // A fixed/fixed join is exact and needs no Huffman search, so it
        // applies even when merge search is disabled.
        let fixed_join_eligible = pending_plans.len() == 1
            && current_plans.len() == 1
            && is_fixed_plan(&pending_plans[0])
            && is_fixed_plan(&current_plans[0]);
        let need_merged_block =
            can_search_merge || (merge_search_enabled && shared_tree) || fixed_join_eligible;
        let mut merged = need_merged_block
            .then(|| try_merge_parsed_blocks(pending_block, current))
            .flatten();
        let mut merged_winner = fixed_join_eligible
            .then(|| {
                let merged = merged.as_ref()?;
                Some(vec![PlannedBlock {
                    tokens: Arc::clone(&merged.tokens),
                    plain: Arc::clone(&merged.plain),
                    representation: Representation::Fixed,
                    bits: pending_plans[0]
                        .bits
                        .checked_add(current_plans[0].bits)?
                        .checked_sub(10)?,
                    source_type: merged.source_type,
                }])
            })
            .flatten();

        if let Some(merged_block) = can_search_merge.then_some(()).and(merged.as_ref()) {
            if let Some(progress) = progress {
                progress.item(index + 2, merged_block.tokens.len());
                progress.activity("Testing adjacent block merge");
            }
            let candidate = plan_source_with_search(
                merged_block,
                alignment,
                options,
                source_search,
                plan_cache,
                deferred.as_deref_mut(),
                stop,
            );
            if total_bits(&candidate) < separate_bits
                && merged_winner
                    .as_ref()
                    .map_or(true, |winner| total_bits(&candidate) < total_bits(winner))
            {
                merged_winner = Some(candidate);
            }
        }

        if let Some(merged_block) = (merge_search_enabled && shared_tree)
            .then_some(())
            .and(merged.as_ref())
        {
            if let Some(dynamic) = pending_block.original_dynamic.as_ref().and_then(|source| {
                score_existing_dynamic(&merged_block.tokens, source, options.strict)
            }) {
                if dynamic.bits < separate_bits
                    && merged_winner
                        .as_ref()
                        .map_or(true, |winner| dynamic.bits < total_bits(winner))
                {
                    merged_winner = Some(vec![PlannedBlock {
                        tokens: merged_block.tokens.clone(),
                        plain: merged_block.plain.clone(),
                        bits: dynamic.bits,
                        representation: Representation::Dynamic(dynamic),
                        source_type: merged_block.source_type,
                    }]);
                }
            }
        }

        if let Some(winner) = merged_winner {
            let mut merged = merged
                .take()
                .expect("every accepted adjacent candidate owns a merged block");
            if winner.len() == 1 {
                merged.replace_tokens(winner[0].tokens.clone());
            }
            pending = PendingBlock::Owned(merged);
            pending_cache = Some((alignment, winner));
        } else {
            append_output_plans(&mut output, &mut output_bits, pending_plans)?;
            pending = PendingBlock::Borrowed(current);
            pending_cache = Some((current_alignment, current_plans));
        }
    }

    let alignment = ((u64::from(start_alignment) + output_bits) & 7) as u8;
    if let Some(progress) = progress {
        progress.item(blocks.len(), pending.as_block().tokens.len());
        progress.activity("Columbo token and split search");
    }
    let pending_plans = match pending_cache {
        Some((cached_alignment, plans)) if cached_alignment == alignment => plans,
        _ => plan_source_with_search(
            pending.as_block(),
            alignment,
            options,
            source_search,
            plan_cache,
            deferred,
            stop,
        ),
    };
    append_output_plans(&mut output, &mut output_bits, pending_plans)?;
    Some(output)
}

fn plan_source_with_search(
    block: &ParsedBlock,
    alignment: u8,
    options: &Options,
    search: SourceBlockSearch,
    plan_cache: &mut CanonicalPlanCache,
    deferred: Option<&mut DeferredSplits>,
    stop: &mut SearchStop<'_>,
) -> Vec<PlannedBlock> {
    match search {
        SourceBlockSearch::Full {
            integrated_compact_proven,
        } => plan_source_with_splits(
            block,
            alignment,
            options,
            integrated_compact_proven,
            plan_cache,
            deferred,
            stop,
        ),
        SourceBlockSearch::Narrow { individual_prune } => {
            let plan = match plan_source_block(block, alignment, options, stop) {
                Some(seed) if !stop.reached() => plan_block_with_seeded_narrow_search(
                    block,
                    alignment,
                    options,
                    individual_prune,
                    seed,
                    stop,
                ),
                Some(seed) => seed,
                None => {
                    plan_block_with_narrow_search(block, alignment, options, individual_prune, stop)
                }
            };
            vec![plan]
        }
        SourceBlockSearch::Floor => {
            // After the deadline, use the ordinary table selector rather than
            // an extended floor on every remaining block.
            let plan = if stop.reached() {
                lookup_block_cached(block, alignment, options, plan_cache).unwrap_or_else(|| {
                    plan_block(block, alignment, options, &mut SearchStop::always())
                })
            } else {
                let mut floor_options = options.clone();
                floor_options.exhaustive = false;
                let base = plan_block_cached(block, alignment, &floor_options, plan_cache);
                improve_plan_with_floor(block, alignment, &floor_options, true, base)
            };
            vec![plan]
        }
    }
}

fn blocks_share_dynamic_tree(left: &ParsedBlock, right: &ParsedBlock) -> bool {
    match (&left.original_dynamic, &right.original_dynamic) {
        (Some(left), Some(right)) => {
            left.literal_lengths == right.literal_lengths
                && left.distance_lengths == right.distance_lengths
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests;
