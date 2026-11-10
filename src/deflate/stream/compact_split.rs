// SPDX-License-Identifier: MIT
// SPDX-FileCopyrightText: 2026 Andrew C.E. Dent <https://github.com/ace-dent>

//! Split blocks of a compact completed parent once each (M10c, M10d).
//!
//! The compact split floor adds at most one cut per block, at a decoded eighth
//! or its inside-match sibling, with no token, merge or replay search, so its
//! work is small and deterministic and it can run late. The timed form prices
//! every cut with ordinary headers first, nearest the 7/8 point first, and
//! refines with exhaustive headers while time remains. Blocks not yet reached at
//! the deadline keep their base plan, so the result is still a complete
//! candidate.

use std::sync::Arc;

use crate::deflate::block::{plan_block_cached, reusable_original_bits, CanonicalPlanCache};
use crate::deflate::model::{
    count_frequencies, ParsedBlock, PlannedBlock, Representation, SourceBlockType,
};
use crate::deflate::stop::SearchStop;
use crate::Options;

use super::{
    add_eighth_cuts, append_output_plans, include_inside_match_cuts, make_range,
    sort_cuts_by_plain, total_bits, Composite, Cut, MAX_FRAGMENTED_REPLAY_BLOCKS,
};

/// Price the best direct decoded-eighth split of each block, without deep token
/// search.
pub(crate) fn plan_compact_source_split_floor(
    blocks: &[ParsedBlock],
    start_alignment: u8,
    options: &Options,
) -> Option<Vec<PlannedBlock>> {
    let mut plan_cache = CanonicalPlanCache::new();
    plan_compact_source_split_floor_cached(
        blocks,
        start_alignment,
        options,
        &mut plan_cache,
        false,
        &mut SearchStop::never(),
    )
}

/// Plan the compact split floor until the deadline; untouched blocks then keep
/// their base plan, so the result is still a complete candidate.
pub(crate) fn plan_compact_source_split_floor_until(
    blocks: &[ParsedBlock],
    start_alignment: u8,
    options: &Options,
    stop: &mut SearchStop<'_>,
) -> Option<Vec<PlannedBlock>> {
    let mut plan_cache = CanonicalPlanCache::new();
    if stop.reached() && stop.permits_bounded_finalization() {
        return plan_compact_source_split_rescue(blocks, start_alignment, options, &mut plan_cache);
    }
    // Coarse to fine: price every cut non-exhaustively, then exhaustively while
    // time remains.
    let mut quick_options = options.clone();
    quick_options.exhaustive = false;
    let mut priority = plan_compact_source_split_floor_cached(
        blocks,
        start_alignment,
        &quick_options,
        &mut plan_cache,
        true,
        stop,
    )?;
    finalize_compact_split_header_suffix(
        &mut priority,
        start_alignment,
        options,
        &mut plan_cache,
        stop,
    );
    if stop.reached() {
        return Some(priority);
    }
    let complete = plan_compact_source_split_floor_cached(
        blocks,
        start_alignment,
        options,
        &mut plan_cache,
        true,
        stop,
    )?;
    if total_bits(&complete) < total_bits(&priority) {
        Some(complete)
    } else {
        Some(priority)
    }
}

/// Run one bounded split probe when a compact route reaches its hard deadline
/// before trying any cut.
fn plan_compact_source_split_rescue(
    blocks: &[ParsedBlock],
    start_alignment: u8,
    options: &Options,
    plan_cache: &mut CanonicalPlanCache,
) -> Option<Vec<PlannedBlock>> {
    let priority_block = blocks
        .iter()
        .enumerate()
        .filter(|(_, block)| block.tokens.len() >= 16 && block.plain.len() >= 128)
        .max_by_key(|(_, block)| block.plain.len())
        .map(|(index, _)| index)?;
    let mut quick_options = options.clone();
    quick_options.exhaustive = false;
    let mut output = Vec::new();
    output
        .try_reserve_exact(blocks.len().saturating_add(1))
        .ok()?;
    let mut output_bits = 0_u64;

    for (block_index, block) in blocks.iter().enumerate() {
        let alignment = ((u64::from(start_alignment) + output_bits) & 7) as u8;
        let base = reusable_original_bits(block, alignment, options.strict)
            .map(|original| PlannedBlock {
                tokens: Arc::clone(&block.tokens),
                plain: Arc::clone(&block.plain),
                representation: Representation::Original(original),
                bits: original.len,
                source_type: block.source_type,
            })
            .unwrap_or_else(|| plan_block_cached(block, alignment, &quick_options, plan_cache));
        let mut winner = vec![base];
        let mut winner_bits = total_bits(&winner);

        if block_index == priority_block {
            let composite = Composite::new(std::slice::from_ref(block))?;
            let source = composite.sources[0];
            let mut cuts = Vec::new();
            add_eighth_cuts(
                &mut cuts,
                &composite,
                source.token_start,
                source.token_end,
                source.plain_start,
                source.plain_end,
                include_inside_match_cuts(options.exhaustive, block.tokens.len()),
            )?;
            sort_cuts_by_plain(&mut cuts);
            cuts.dedup_by_key(|cut| cut.plain);

            let mut best_probe = None;
            for split in cuts {
                let left = make_range(&composite, Cut { token: 0, plain: 0 }, split)?;
                let left_plan = plan_block_cached(&left, alignment, &quick_options, plan_cache);
                let right_alignment = ((u64::from(alignment) + left_plan.bits) & 7) as u8;
                let right = make_range(
                    &composite,
                    split,
                    Cut {
                        token: block.tokens.len(),
                        plain: block.plain.len(),
                    },
                )?;
                let right_plan =
                    plan_block_cached(&right, right_alignment, &quick_options, plan_cache);
                let bits = left_plan.bits.checked_add(right_plan.bits)?;
                if best_probe.map_or(true, |(best_bits, _)| bits < best_bits) {
                    best_probe = Some((bits, split));
                }
            }

            if let Some((_, split)) = best_probe {
                let left = make_range(&composite, Cut { token: 0, plain: 0 }, split)?;
                let left_plan = plan_block_cached(&left, alignment, options, plan_cache);
                let right_alignment = ((u64::from(alignment) + left_plan.bits) & 7) as u8;
                let right = make_range(
                    &composite,
                    split,
                    Cut {
                        token: block.tokens.len(),
                        plain: block.plain.len(),
                    },
                )?;
                let right_plan = plan_block_cached(&right, right_alignment, options, plan_cache);
                let bits = left_plan.bits.checked_add(right_plan.bits)?;
                if bits < winner_bits {
                    winner_bits = bits;
                    winner.clear();
                    winner.push(left_plan);
                    winner.push(right_plan);
                }
            }
        }

        if output.len().checked_add(winner.len())? > MAX_FRAGMENTED_REPLAY_BLOCKS {
            return None;
        }
        append_output_plans(&mut output, &mut output_bits, winner)?;
    }
    Some(output)
}

/// Exhaustively replan the alignment-independent suffix of the fast sweep's
/// winning layout, after its last stored or exact-original block.
fn finalize_compact_split_header_suffix(
    plans: &mut [PlannedBlock],
    start_alignment: u8,
    options: &Options,
    plan_cache: &mut CanonicalPlanCache,
    stop: &mut SearchStop<'_>,
) {
    let first_safe = plans
        .iter()
        .rposition(|plan| {
            matches!(
                plan.representation,
                Representation::Original(_) | Representation::Stored
            )
        })
        .map_or(0, |index| index + 1);
    let mut output_bits = 0_u64;
    for (index, plan) in plans.iter_mut().enumerate() {
        let alignment = ((u64::from(start_alignment) + output_bits) & 7) as u8;
        if index >= first_safe && !stop.reached() {
            if let Some(block) = parsed_from_generated_huffman_plan(plan) {
                let refined = plan_block_cached(&block, alignment, options, plan_cache);
                if refined.bits < plan.bits {
                    *plan = refined;
                }
            }
        }
        output_bits = output_bits.saturating_add(plan.bits);
    }
}

fn parsed_from_generated_huffman_plan(plan: &PlannedBlock) -> Option<ParsedBlock> {
    let original_dynamic = match &plan.representation {
        Representation::Dynamic(dynamic) => Some(dynamic.try_clone()?),
        Representation::Fixed => None,
        Representation::Original(_) | Representation::Stored => return None,
    };
    let source_type = if original_dynamic.is_some() {
        SourceBlockType::Dynamic
    } else {
        SourceBlockType::Fixed
    };
    let (literal_frequencies, distance_frequencies) = count_frequencies(&plan.tokens);
    Some(ParsedBlock {
        tokens: Arc::clone(&plan.tokens),
        plain: Arc::clone(&plan.plain),
        literal_frequencies,
        distance_frequencies,
        original_literal_lengths: None,
        original_distance_lengths: None,
        original_dynamic,
        original: None,
        source_splits: Vec::new(),
        source_type,
    })
}

/// `timed` selects the deadline form: each block starts from its original bits
/// when they can be reused, and token-boundary cuts, then inside-match cuts,
/// are tried nearest the 7/8 point first.
pub(super) fn plan_compact_source_split_floor_cached(
    blocks: &[ParsedBlock],
    start_alignment: u8,
    options: &Options,
    plan_cache: &mut CanonicalPlanCache,
    timed: bool,
    stop: &mut SearchStop<'_>,
) -> Option<Vec<PlannedBlock>> {
    let mut output = Vec::new();
    output.try_reserve_exact(blocks.len()).ok()?;
    let mut output_bits = 0_u64;
    for block in blocks {
        let alignment = ((u64::from(start_alignment) + output_bits) & 7) as u8;
        // Timed finalization reuses the parent's valid original bits: repeating
        // the Max route's table search could use the whole grace period before
        // the first split.
        let base = if timed {
            reusable_original_bits(block, alignment, options.strict).map(|original| PlannedBlock {
                tokens: Arc::clone(&block.tokens),
                plain: Arc::clone(&block.plain),
                representation: Representation::Original(original),
                bits: original.len,
                source_type: block.source_type,
            })
        } else {
            None
        }
        .unwrap_or_else(|| plan_block_cached(block, alignment, options, plan_cache));
        let mut winner = Vec::new();
        winner.try_reserve_exact(2).ok()?;
        winner.push(base);
        let mut winner_bits = total_bits(&winner);

        if block.tokens.len() >= 16 && block.plain.len() >= 128 && !stop.reached() {
            let composite = Composite::new(std::slice::from_ref(block))?;
            let source = composite.sources[0];
            let mut cuts = Vec::new();
            add_eighth_cuts(
                &mut cuts,
                &composite,
                source.token_start,
                source.token_end,
                source.plain_start,
                source.plain_end,
                include_inside_match_cuts(options.exhaustive, block.tokens.len()),
            )?;
            cuts.sort_unstable_by_key(|&cut| {
                let token_boundary = composite.cut_is_token_boundary(cut);
                if timed {
                    (
                        usize::from(!token_boundary),
                        cut.plain.abs_diff(block.plain.len().saturating_mul(7) / 8),
                    )
                } else {
                    (usize::from(!token_boundary), cut.plain)
                }
            });
            cuts.dedup_by_key(|cut| cut.plain);
            for split in cuts {
                if stop.reached() {
                    break;
                }
                let left = make_range(&composite, Cut { token: 0, plain: 0 }, split)?;
                let left_plan = plan_block_cached(&left, alignment, options, plan_cache);
                let right_alignment = ((u64::from(alignment) + left_plan.bits) & 7) as u8;
                let right = make_range(
                    &composite,
                    split,
                    Cut {
                        token: block.tokens.len(),
                        plain: block.plain.len(),
                    },
                )?;
                let right_plan = plan_block_cached(&right, right_alignment, options, plan_cache);
                let bits = left_plan.bits.checked_add(right_plan.bits)?;
                if bits < winner_bits {
                    winner_bits = bits;
                    winner.clear();
                    winner.push(left_plan);
                    winner.push(right_plan);
                }
            }
        }

        if output.len().checked_add(winner.len())? > MAX_FRAGMENTED_REPLAY_BLOCKS {
            return None;
        }
        append_output_plans(&mut output, &mut output_bits, winner)?;
    }
    Some(output)
}

#[cfg(test)]
mod tests;
