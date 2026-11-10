// SPDX-License-Identifier: MIT
// SPDX-FileCopyrightText: 2026 Andrew C.E. Dent <https://github.com/ace-dent>

//! Group adjacent Huffman source blocks into cheap floor layouts.
//!
//! The greedy walk merges each pending block with the next when the bounded
//! floor prices the merge strictly cheaper. Bounded grouping finds groups the
//! walk misses, whose first pair is neutral but whose three or more blocks
//! profit from one shared table: it scores every group of up to 16 contiguous
//! blocks from additive summaries and picks the cheapest segmentation; M7's
//! floor-seeded route runs it alone. Collection folds runs of adjacent Huffman
//! blocks within token and decoded-byte limits without pricing. The ordinary
//! planner plans each layout structurally as a floor.

use std::thread;

use crate::deflate::block::CanonicalPlanCache;
use crate::deflate::model::{
    token_extra_bits, ParsedBlock, PlannedBlock, Representation, SourceBlockType,
};
use crate::deflate::search::{score_short_family_frequencies, ShortFamilyStats};
use crate::Options;

use super::{
    direct_structural_plan, finish_plan, is_fixed_plan, payload_storage_bytes,
    plan_block_with_floor_cached, plan_block_with_short_family_floor_cached, prepare_blocks,
    try_append_parsed_block, try_merge_parsed_blocks, PendingBlock, COLLECTED_RUN_MAX_PLAIN,
    COLLECTED_RUN_MAX_TOKENS, MAX_BOUNDARY_DP_CUTS, MAX_GREEDY_SOURCE_BLOCKS,
    MAX_GROUPED_MODEL_BYTES, MAX_MERGED_PLAIN, MAX_MERGED_TOKENS,
};

const MAX_BOUNDED_GROUP_SPAN: usize = 16;
/// Keeps the optional bounded-grouping route from taking every hardware thread.
const MAX_BOUNDED_GROUP_WORKERS: usize = 4;

/// Group adjacent Huffman source blocks with a greedy walk priced by the
/// bounded floor. Unlike the source-order walk, it adds no fixed-join or
/// shared-tree merge candidates.
pub(super) fn greedy_huffman_blocklist(
    blocks: &[ParsedBlock],
    start_alignment: u8,
    options: &Options,
    plan_cache: &mut CanonicalPlanCache,
) -> Option<Vec<ParsedBlock>> {
    let (first, rest) = blocks.split_first()?;
    if rest.is_empty()
        || blocks
            .iter()
            .any(|block| block.source_type == SourceBlockType::Stored)
        // The walk keeps the parsed source and at most one grouped copy.
        || payload_storage_bytes(blocks)?.checked_mul(2)? > MAX_GROUPED_MODEL_BYTES
    {
        return None;
    }

    // A structural floor, not a second Max search: the exhaustive header
    // search runs when the chosen groups are replayed.
    let mut structural_options = options.clone();
    structural_options.exhaustive = false;

    let mut grouped = Vec::new();
    grouped.try_reserve_exact(blocks.len()).ok()?;
    let mut output_bits = 0_u64;
    let mut pending = PendingBlock::Borrowed(first);
    let mut pending_cache: Option<(u8, PlannedBlock)> = None;

    for current in rest {
        let alignment = ((u64::from(start_alignment) + output_bits) & 7) as u8;
        let pending_plan = match pending_cache.take() {
            Some((cached_alignment, plan)) if cached_alignment == alignment => plan,
            _ => plan_block_with_floor_cached(
                pending.as_block(),
                alignment,
                &structural_options,
                false,
                plan_cache,
            ),
        };
        let current_alignment = ((u64::from(alignment) + pending_plan.bits) & 7) as u8;
        let current_plan = plan_block_with_floor_cached(
            current,
            current_alignment,
            &structural_options,
            false,
            plan_cache,
        );
        let mut separate_bits = pending_plan.bits.checked_add(current_plan.bits)?;
        // Adjacent fixed plans are later joined, 10 bits shorter
        // (`try_join_fixed_plans`); a merge must beat that total.
        if is_fixed_plan(&pending_plan) && is_fixed_plan(&current_plan) {
            separate_bits = separate_bits.checked_sub(10)?;
        }

        let pending_block = pending.as_block();
        let can_merge = pending_block
            .tokens
            .len()
            .checked_add(current.tokens.len())
            .is_some_and(|tokens| tokens <= MAX_MERGED_TOKENS)
            && pending_block
                .plain
                .len()
                .checked_add(current.plain.len())
                .is_some_and(|plain| plain <= MAX_MERGED_PLAIN);
        if can_merge {
            if let Some(merged) = try_merge_parsed_blocks(pending_block, current) {
                let merged_plan = plan_block_with_floor_cached(
                    &merged,
                    alignment,
                    &structural_options,
                    false,
                    plan_cache,
                );
                if merged_plan.bits < separate_bits {
                    let mut merged = merged;
                    merged.replace_tokens(merged_plan.tokens.clone());
                    pending = PendingBlock::Owned(merged);
                    pending_cache = Some((alignment, merged_plan));
                    continue;
                }
            }
        }

        grouped.try_reserve(1).ok()?;
        grouped.push(pending_block.try_clone_shared()?);
        output_bits = output_bits.checked_add(pending_plan.bits)?;
        pending = PendingBlock::Borrowed(current);
        pending_cache = Some((current_alignment, current_plan));
    }

    grouped.try_reserve(1).ok()?;
    grouped.push(pending.as_block().try_clone_shared()?);
    Some(grouped)
}

/// Replan a rewritten floor candidate with bounded Huffman grouping only. Range
/// pricing runs in parallel; the result is deterministic.
pub(crate) fn plan_columbo_floor_seeded_bounded_grouping(
    blocks: &[ParsedBlock],
    start_alignment: u8,
    options: &Options,
) -> Option<Vec<PlannedBlock>> {
    let prepared = prepare_blocks(blocks);
    let blocks = prepared.as_deref().unwrap_or(blocks);
    let mut plan_cache = CanonicalPlanCache::new();
    let grouped = bounded_huffman_grouping(
        blocks,
        options,
        BoundedRangePricing::Parallel,
        &mut plan_cache,
    )?;
    let plans = direct_structural_plan(&grouped, start_alignment, options, &mut plan_cache)?;
    Some(finish_plan(plans, options))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum BoundedRangePricing {
    Serial,
    Parallel,
}

/// Group source blocks with a bounded lookahead, which finds groups a greedy
/// walk misses: a neutral first pair whose three or more blocks profit from one
/// shared table.
pub(super) fn bounded_huffman_grouping(
    blocks: &[ParsedBlock],
    options: &Options,
    pricing: BoundedRangePricing,
    plan_cache: &mut CanonicalPlanCache,
) -> Option<Vec<ParsedBlock>> {
    if blocks.len() < 2
        || blocks.len() > MAX_GREEDY_SOURCE_BLOCKS
        || blocks
            .iter()
            .any(|block| block.source_type == SourceBlockType::Stored)
        // Pricing is transient; only the source and one grouped copy are kept.
        || payload_storage_bytes(blocks)?.checked_mul(2)? > MAX_GROUPED_MODEL_BYTES
    {
        return None;
    }

    let mut structural_options = options.clone();
    structural_options.exhaustive = false;
    let mut source_plans = Vec::new();
    source_plans.try_reserve_exact(blocks.len()).ok()?;
    for block in blocks {
        let plan = plan_block_with_floor_cached(block, 0, &structural_options, false, plan_cache);
        if matches!(plan.representation, Representation::Stored) {
            return None;
        }
        source_plans.push(plan);
    }

    let mut source_stats = Vec::new();
    source_stats.try_reserve_exact(blocks.len()).ok()?;
    for block in blocks {
        source_stats.push(ShortFamilyStats::from_block(block)?);
    }
    let mut source_extra_bits = Vec::new();
    source_extra_bits.try_reserve_exact(blocks.len()).ok()?;
    for block in blocks {
        source_extra_bits.push(token_extra_bits(&block.tokens));
    }

    // All candidates are Huffman blocks, so a range's price does not depend on
    // its neighbors.
    let range_bits = match pricing {
        BoundedRangePricing::Serial => bounded_range_prices(
            blocks,
            &source_plans,
            &source_stats,
            &source_extra_bits,
            structural_options.strict,
            0,
            blocks.len(),
        )?,
        BoundedRangePricing::Parallel => parallel_bounded_range_prices(
            blocks,
            &source_plans,
            &source_stats,
            &source_extra_bits,
            structural_options.strict,
        )?,
    };

    let next_boundary = choose_bounded_boundaries(&range_bits)?;

    let mut grouped = Vec::new();
    grouped.try_reserve_exact(blocks.len()).ok()?;
    let mut start = 0_usize;
    while start < blocks.len() {
        let end = next_boundary[start];
        if end > start + 1 {
            let mut winner = blocks[start].try_clone_shared()?;
            for block in &blocks[start + 1..end] {
                if !try_append_parsed_block(&mut winner, block) {
                    return None;
                }
            }
            let plan = plan_block_with_short_family_floor_cached(
                &winner,
                0,
                &structural_options,
                plan_cache,
            );
            let predicted = range_bits[start][end - start]?;
            // The score prices a concrete paired-tree candidate, so a costlier
            // plan means building it failed under memory pressure; abandon this
            // optional layout.
            if plan.bits > predicted {
                return None;
            }
            winner.replace_tokens(plan.tokens);
            grouped.push(winner);
            start = end;
        } else {
            let mut single = blocks[start].try_clone_shared()?;
            single.replace_tokens(source_plans[start].tokens.clone());
            grouped.push(single);
            start += 1;
        }
    }
    Some(grouped)
}

type BoundedRangePrices = [Option<u64>; MAX_BOUNDED_GROUP_SPAN + 1];

/// Price independent start rows concurrently, preserving source-row order.
///
/// A failed spawn runs its rows on the calling thread. Every started worker is
/// joined before an allocation failure is returned or the first worker panic
/// is resumed.
fn parallel_bounded_range_prices(
    blocks: &[ParsedBlock],
    source_plans: &[PlannedBlock],
    source_stats: &[ShortFamilyStats],
    source_extra_bits: &[u64],
    min_distance_codes: bool,
) -> Option<Vec<BoundedRangePrices>> {
    let worker_count = thread::available_parallelism()
        .map(usize::from)
        .unwrap_or(1)
        .min(MAX_BOUNDED_GROUP_WORKERS)
        .min(blocks.len());
    if worker_count <= 1 {
        return bounded_range_prices(
            blocks,
            source_plans,
            source_stats,
            source_extra_bits,
            min_distance_codes,
            0,
            blocks.len(),
        );
    }

    let rows_per_worker = blocks.len().div_ceil(worker_count);
    let chunk_count = blocks.len().div_ceil(rows_per_worker);
    thread::scope(|scope| {
        let mut completed = Vec::new();
        completed.try_reserve_exact(chunk_count).ok()?;
        completed.resize_with(chunk_count, || None);

        let mut handles = Vec::new();
        if handles.try_reserve_exact(chunk_count).is_err() {
            return bounded_range_prices(
                blocks,
                source_plans,
                source_stats,
                source_extra_bits,
                min_distance_codes,
                0,
                blocks.len(),
            );
        }

        let mut pricing_failed = false;
        for (chunk_index, start) in (0..blocks.len()).step_by(rows_per_worker).enumerate() {
            let end = blocks.len().min(start + rows_per_worker);
            let worker = thread::Builder::new().spawn_scoped(scope, move || {
                bounded_range_prices(
                    blocks,
                    source_plans,
                    source_stats,
                    source_extra_bits,
                    min_distance_codes,
                    start,
                    end,
                )
            });
            match worker {
                Ok(handle) => handles.push((chunk_index, handle)),
                Err(_) => match bounded_range_prices(
                    blocks,
                    source_plans,
                    source_stats,
                    source_extra_bits,
                    min_distance_codes,
                    start,
                    end,
                ) {
                    Some(rows) => completed[chunk_index] = Some(rows),
                    None => pricing_failed = true,
                },
            }
        }

        let mut first_panic = None;
        for (chunk_index, handle) in handles {
            match handle.join() {
                Ok(Some(rows)) => completed[chunk_index] = Some(rows),
                Ok(None) => pricing_failed = true,
                Err(payload) => {
                    if first_panic.is_none() {
                        first_panic = Some(payload);
                    }
                }
            }
        }
        if let Some(payload) = first_panic {
            std::panic::resume_unwind(payload);
        }
        if pricing_failed {
            return None;
        }

        let mut prices = Vec::new();
        prices.try_reserve_exact(blocks.len()).ok()?;
        for chunk in completed {
            let rows = chunk?;
            let combined_len = prices.len().checked_add(rows.len())?;
            if combined_len > blocks.len() {
                return None;
            }
            prices.extend(rows);
        }
        (prices.len() == blocks.len()).then_some(prices)
    })
}

#[allow(clippy::too_many_arguments)]
fn bounded_range_prices(
    blocks: &[ParsedBlock],
    source_plans: &[PlannedBlock],
    source_stats: &[ShortFamilyStats],
    source_extra_bits: &[u64],
    min_distance_codes: bool,
    start_index: usize,
    end_index: usize,
) -> Option<Vec<BoundedRangePrices>> {
    let mut range_bits = Vec::new();
    range_bits
        .try_reserve_exact(end_index.checked_sub(start_index)?)
        .ok()?;
    for start in start_index..end_index {
        let mut prices = [None; MAX_BOUNDED_GROUP_SPAN + 1];
        prices[1] = Some(source_plans.get(start)?.bits);
        let mut literal_frequencies = [0_u32; 286];
        let mut distance_frequencies = [0_u32; 30];
        let mut range_extra_bits = 0_u64;
        let mut range_tokens = 0_usize;
        let mut range_plain = 0_usize;
        let mut range_stats = source_stats.get(start)?.clone();

        for end in start + 1..=blocks.len().min(start + MAX_BOUNDED_GROUP_SPAN) {
            let block = &blocks[end - 1];
            range_tokens = range_tokens.checked_add(block.tokens.len())?;
            range_plain = range_plain.checked_add(block.plain.len())?;
            range_extra_bits = range_extra_bits.checked_add(*source_extra_bits.get(end - 1)?)?;
            for (total, &frequency) in literal_frequencies
                .iter_mut()
                .zip(&block.literal_frequencies)
            {
                *total = total.checked_add(frequency)?;
            }
            for (total, &frequency) in distance_frequencies
                .iter_mut()
                .zip(&block.distance_frequencies)
            {
                *total = total.checked_add(frequency)?;
            }

            if end > start + 1 {
                // Each source histogram counts its own end-of-block symbol; a
                // group emits only one.
                literal_frequencies[256] = literal_frequencies[256].checked_sub(1)?;
                range_stats.add_assign(source_stats.get(end - 1)?)?;
            }

            if range_tokens > MAX_MERGED_TOKENS || range_plain > MAX_MERGED_PLAIN {
                break;
            }
            if end == start + 1 {
                continue;
            }

            // Short-family effects are additive, so score ranges from these
            // summaries and fully plan only the selected ones.
            prices[end - start] = score_short_family_frequencies(
                &literal_frequencies,
                &distance_frequencies,
                range_extra_bits,
                &range_stats,
                min_distance_codes,
            );
        }
        range_bits.push(prices);
    }
    Some(range_bits)
}

/// Choose the cheapest segmentation from `prices[start][span]` (`None`: group
/// not allowed), returning each start's group end. Ties keep the shorter group.
fn choose_bounded_boundaries(
    prices: &[[Option<u64>; MAX_BOUNDED_GROUP_SPAN + 1]],
) -> Option<Vec<usize>> {
    let mut suffix_bits = Vec::new();
    suffix_bits
        .try_reserve_exact(prices.len().checked_add(1)?)
        .ok()?;
    suffix_bits.resize(prices.len() + 1, u64::MAX);
    suffix_bits[prices.len()] = 0;
    let mut next_boundary = Vec::new();
    next_boundary.try_reserve_exact(prices.len()).ok()?;
    next_boundary.resize(prices.len(), 0_usize);

    for start in (0..prices.len()).rev() {
        let mut best_bits = prices[start][1]?.checked_add(suffix_bits[start + 1])?;
        let mut best_end = start + 1;
        for span in 2..=MAX_BOUNDED_GROUP_SPAN.min(prices.len() - start) {
            let Some(bits) = prices[start][span] else {
                continue;
            };
            let total = bits.checked_add(suffix_bits[start + span])?;
            if total < best_bits {
                best_bits = total;
                best_end = start + span;
            }
        }
        suffix_bits[start] = best_bits;
        next_boundary[start] = best_end;
    }
    Some(next_boundary)
}

/// Collect adjacent Huffman blocks into runs within the token and decoded-byte
/// limits, in one source-order pass; stored blocks end a run.
pub(super) fn collect_huffman_runs(blocks: &[ParsedBlock], wide: bool) -> Option<Vec<ParsedBlock>> {
    let (token_limit, plain_limit) = if wide {
        (MAX_MERGED_TOKENS, MAX_MERGED_PLAIN)
    } else {
        (COLLECTED_RUN_MAX_TOKENS, COLLECTED_RUN_MAX_PLAIN)
    };
    collect_huffman_runs_with_limits(blocks, token_limit, plain_limit)
}

pub(super) fn collect_huffman_runs_with_limits(
    blocks: &[ParsedBlock],
    token_limit: usize,
    plain_limit: usize,
) -> Option<Vec<ParsedBlock>> {
    if token_limit == 0
        || plain_limit == 0
        || payload_storage_bytes(blocks)?.checked_mul(3)? > MAX_GROUPED_MODEL_BYTES
    {
        return None;
    }
    let mut collected = Vec::<ParsedBlock>::new();

    for block in blocks {
        let can_append = collected.last().is_some_and(|pending| {
            pending.source_type != SourceBlockType::Stored
                && block.source_type != SourceBlockType::Stored
                && pending
                    .tokens
                    .len()
                    .checked_add(block.tokens.len())
                    .is_some_and(|tokens| tokens <= token_limit)
                && pending
                    .plain
                    .len()
                    .checked_add(block.plain.len())
                    .is_some_and(|plain| plain <= plain_limit)
        });
        if can_append {
            let pending = collected
                .last_mut()
                .expect("the append predicate observed a pending block");
            if !try_append_parsed_block(pending, block) {
                // On allocation failure, keep the block unmerged.
                if collected.len() >= MAX_BOUNDARY_DP_CUTS {
                    return None;
                }
                collected.try_reserve(1).ok()?;
                collected.push(block.try_clone_shared()?);
            }
        } else {
            if collected.len() >= MAX_BOUNDARY_DP_CUTS {
                return None;
            }
            collected.try_reserve(1).ok()?;
            collected.push(block.try_clone_shared()?);
        }
    }

    Some(collected)
}

#[cfg(test)]
mod tests;
