// SPDX-License-Identifier: MIT
// SPDX-FileCopyrightText: 2026 Andrew C.E. Dent <https://github.com/ace-dent>

//! Refine the layout of a selected complete plan.
//!
//! These Max steps work from the plans themselves. The selected-plan merge
//! cleanup greedily merges neighbors of a compact Huffman plan with the bounded
//! floor. The comparison-floor boundary reseat moves one boundary to the
//! adaptive probe's cut of its two blocks' union. The forced-split boundary
//! escape forces one locally losing split, then reseats, so it can pass through
//! a worse layout to a better one. Each result counts only if its total is
//! strictly smaller.

use std::sync::Arc;

use crate::deflate::block::CanonicalPlanCache;
use crate::deflate::model::{count_frequencies, ParsedBlock, PlannedBlock};
use crate::deflate::search::try_clone_planned_block;
use crate::deflate::stop::SearchStop;
use crate::Options;

use super::adaptive_split::{
    add_adaptive_split_cut, secondary_adaptive_split_cut, ADAPTIVE_SPLIT_MIN_TOKENS,
};
use super::{
    append_output_plan, is_fixed_plan, plan_block_with_floor_cached, plan_is_alignment_independent,
    plan_structural_ranges, total_bits, try_merge_parsed_blocks, Composite, Cut,
    COLLECTED_RUN_MAX_PLAIN, COLLECTED_RUN_MAX_TOKENS, MAX_MERGED_TOKENS,
    MAX_REGROUP_SOURCE_BLOCKS,
};

/// The forced split adds one block, which must stay within the boundary-reseat
/// limit.
const FORCED_SPLIT_MAX_PARENT_BLOCKS: usize = MAX_REGROUP_SOURCE_BLOCKS - 1;

/// Greedily merge adjacent blocks of a selected compact Huffman plan with the
/// bounded floor. Returns `None` unless the result is smaller.
pub(super) fn plan_selected_huffman_merge_floor(
    plans: &[PlannedBlock],
    start_alignment: u8,
    options: &Options,
    plan_cache: &mut CanonicalPlanCache,
) -> Option<Vec<PlannedBlock>> {
    if !(2..=MAX_REGROUP_SOURCE_BLOCKS).contains(&plans.len())
        || plans
            .iter()
            .any(|plan| !plan_is_alignment_independent(plan))
    {
        return None;
    }
    let token_count = plans
        .iter()
        .try_fold(0_usize, |total, plan| total.checked_add(plan.tokens.len()))?;
    let plain_count = plans
        .iter()
        .try_fold(0_usize, |total, plan| total.checked_add(plan.plain.len()))?;
    if token_count > COLLECTED_RUN_MAX_TOKENS || plain_count > COLLECTED_RUN_MAX_PLAIN {
        return None;
    }

    let (first, rest) = plans.split_first()?;
    let mut pending_plan = try_clone_planned_block(first)?;
    let mut pending_block = parsed_from_selected_plan(first);
    let mut output = Vec::new();
    output.try_reserve_exact(plans.len()).ok()?;
    let mut output_bits = 0_u64;
    let mut floor_options = options.clone();
    floor_options.exhaustive = false;

    for current in rest {
        let alignment = ((u64::from(start_alignment) + output_bits) & 7) as u8;
        let current_plan = try_clone_planned_block(current)?;
        let current_block = parsed_from_selected_plan(current);
        let mut separate_bits = pending_plan.bits.checked_add(current_plan.bits)?;
        if is_fixed_plan(&pending_plan) && is_fixed_plan(&current_plan) {
            separate_bits = separate_bits.checked_sub(10)?;
        }

        if let Some(mut merged) = try_merge_parsed_blocks(&pending_block, &current_block) {
            let merged_plan =
                plan_block_with_floor_cached(&merged, alignment, &floor_options, false, plan_cache);
            if merged_plan.bits < separate_bits {
                merged.replace_tokens(merged_plan.tokens.clone());
                pending_block = merged;
                pending_plan = merged_plan;
                continue;
            }
        }

        append_output_plan(&mut output, &mut output_bits, pending_plan, true)?;
        pending_block = current_block;
        pending_plan = current_plan;
    }
    append_output_plan(&mut output, &mut output_bits, pending_plan, true)?;

    (output_bits < total_bits(plans)).then_some(output)
}

/// Move one boundary of a selected Max plan to the adaptive histogram cut with
/// the largest exact saving. All blocks are alignment-independent, so later
/// prices stay valid.
pub(super) fn plan_selected_huffman_boundary_reseat(
    plans: &[PlannedBlock],
    start_alignment: u8,
    options: &Options,
    plan_cache: &mut CanonicalPlanCache,
    stop: &mut SearchStop<'_>,
) -> Option<Vec<PlannedBlock>> {
    if !options.exhaustive
        || !(2..=MAX_REGROUP_SOURCE_BLOCKS).contains(&plans.len())
        || plans
            .iter()
            .any(|plan| !plan_is_alignment_independent(plan))
    {
        return None;
    }
    let token_count = plans
        .iter()
        .try_fold(0_usize, |total, plan| total.checked_add(plan.tokens.len()))?;
    let plain_count = plans
        .iter()
        .try_fold(0_usize, |total, plan| total.checked_add(plan.plain.len()))?;
    if token_count > COLLECTED_RUN_MAX_TOKENS || plain_count > COLLECTED_RUN_MAX_PLAIN {
        return None;
    }

    let mut prefix_bits = 0_u64;
    let mut best_savings = 0_u64;
    let mut best_reseat = None;
    for pair_index in 0..plans.len() - 1 {
        if stop.reached() {
            break;
        }
        let pair = [
            parsed_from_selected_plan(&plans[pair_index]),
            parsed_from_selected_plan(&plans[pair_index + 1]),
        ];
        let Some(composite) = Composite::new(&pair) else {
            break;
        };
        let start = Cut { token: 0, plain: 0 };
        let end = Cut {
            token: composite.tokens.len(),
            plain: composite.plain.len(),
        };
        let existing = Cut {
            token: pair[0].tokens.len(),
            plain: pair[0].plain.len(),
        };
        let mut cuts = Vec::new();
        let Some(()) =
            add_adaptive_split_cut(&mut cuts, &composite, start, end, options.strict, stop)
        else {
            break;
        };
        let Some(&split) = cuts.first().filter(|&&split| split != existing) else {
            prefix_bits = prefix_bits.checked_add(plans[pair_index].bits)?;
            continue;
        };
        let alignment = ((u64::from(start_alignment) + prefix_bits) & 7) as u8;
        let Some(replacement) = plan_structural_ranges(
            &composite,
            &[start, split, end],
            alignment,
            options,
            plan_cache,
            stop,
        ) else {
            break;
        };
        let old_bits = plans[pair_index]
            .bits
            .checked_add(plans[pair_index + 1].bits)?;
        let replacement_bits = total_bits(&replacement);
        let savings = old_bits.saturating_sub(replacement_bits);
        if savings > best_savings {
            best_savings = savings;
            best_reseat = Some((pair_index, replacement));
        }
        prefix_bits = prefix_bits.checked_add(plans[pair_index].bits)?;
    }

    let (pair_index, replacement) = best_reseat?;
    let mut output = Vec::new();
    output.try_reserve_exact(plans.len()).ok()?;
    for plan in &plans[..pair_index] {
        output.push(try_clone_planned_block(plan)?);
    }
    output.extend(replacement);
    for plan in &plans[pair_index + 2..] {
        output.push(try_clone_planned_block(plan)?);
    }
    (total_bits(&output) < total_bits(plans)).then_some(output)
}

/// Try one forced split, then one boundary reseat: a bounded form of
/// Turtledeflate's pushed split, which may pass through a worse state to reach
/// a better partition.
pub(super) fn plan_forced_split_boundary_escape(
    plans: &[PlannedBlock],
    start_alignment: u8,
    options: &Options,
    plan_cache: &mut CanonicalPlanCache,
    stop: &mut SearchStop<'_>,
) -> Option<Vec<PlannedBlock>> {
    if !options.exhaustive
        || !(2..=FORCED_SPLIT_MAX_PARENT_BLOCKS).contains(&plans.len())
        || plans
            .iter()
            .any(|plan| !plan_is_alignment_independent(plan))
    {
        return None;
    }
    let token_count = plans
        .iter()
        .try_fold(0_usize, |total, plan| total.checked_add(plan.tokens.len()))?;
    let plain_count = plans
        .iter()
        .try_fold(0_usize, |total, plan| total.checked_add(plan.plain.len()))?;
    if token_count > COLLECTED_RUN_MAX_TOKENS || plain_count > COLLECTED_RUN_MAX_PLAIN {
        return None;
    }

    let (block_index, selected) = plans
        .iter()
        .enumerate()
        .filter(|(_, plan)| {
            (ADAPTIVE_SPLIT_MIN_TOKENS..=MAX_MERGED_TOKENS).contains(&plan.tokens.len())
                && plan.plain.len() >= 128
        })
        .max_by_key(|(index, plan)| (plan.tokens.len(), plan.plain.len(), usize::MAX - index))?;
    if stop.reached() {
        return None;
    }

    let block = parsed_from_selected_plan(selected);
    let composite = Composite::new(std::slice::from_ref(&block))?;
    let start = Cut { token: 0, plain: 0 };
    let end = Cut {
        token: block.tokens.len(),
        plain: block.plain.len(),
    };
    let split = secondary_adaptive_split_cut(&composite, start, end, options.strict, stop)?;
    let prefix_bits = plans[..block_index]
        .iter()
        .try_fold(0_u64, |bits, plan| bits.checked_add(plan.bits))?;
    let alignment = ((u64::from(start_alignment) + prefix_bits) & 7) as u8;
    let replacement = plan_structural_ranges(
        &composite,
        &[start, split, end],
        alignment,
        options,
        plan_cache,
        stop,
    )?;

    let mut forced = Vec::new();
    forced.try_reserve_exact(plans.len().checked_add(1)?).ok()?;
    for plan in &plans[..block_index] {
        forced.push(try_clone_planned_block(plan)?);
    }
    forced.extend(replacement);
    for plan in &plans[block_index + 1..] {
        forced.push(try_clone_planned_block(plan)?);
    }

    let floor_bits = total_bits(plans);
    let forced_bits = total_bits(&forced);
    if !stop.reached() {
        if let Some(reseated) = plan_selected_huffman_boundary_reseat(
            &forced,
            start_alignment,
            options,
            plan_cache,
            stop,
        ) {
            if total_bits(&reseated) < floor_bits && total_bits(&reseated) < forced_bits {
                return Some(reseated);
            }
        }
    }
    (forced_bits < floor_bits).then_some(forced)
}

fn parsed_from_selected_plan(plan: &PlannedBlock) -> ParsedBlock {
    let (literal_frequencies, distance_frequencies) = count_frequencies(&plan.tokens);
    ParsedBlock {
        tokens: Arc::clone(&plan.tokens),
        plain: Arc::clone(&plan.plain),
        literal_frequencies,
        distance_frequencies,
        original_literal_lengths: None,
        original_distance_lengths: None,
        original_dynamic: None,
        original: None,
        source_splits: Vec::new(),
        source_type: plan.source_type,
    }
}

#[cfg(test)]
mod tests;
