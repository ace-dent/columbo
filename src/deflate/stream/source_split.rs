// SPDX-License-Identifier: MIT
// SPDX-FileCopyrightText: 2026 Andrew C.E. Dent <https://github.com/ace-dent>

//! Split source blocks during the sequential walk (the source split family).
//!
//! Small blocks get only the whole-block search. Larger ones price their
//! decoded-eighth cuts, and the cuts' inside-match siblings, structurally
//! first, because the whole-block Max search can use up the deadline before a
//! useful split is reached. Max adds 32-token cuts on compact blocks and the
//! adaptive probe's cut, then token-searches the split children; Default may
//! split the larger child of a near-tied runner-up split again. A split
//! replaces the block only with strictly fewer bits.

use crate::deflate::block::{lookup_block_cached, plan_block, CanonicalPlanCache};
use crate::deflate::model::{ParsedBlock, PlannedBlock};
use crate::deflate::search::{
    improve_plan_with_integrated_proven_floor, improve_plan_with_short_family_floor,
    plan_block_with_complete_base_search, plan_block_with_complete_integrated_proven_search,
    plan_block_with_search, try_clone_planned_block,
};
use crate::deflate::stop::SearchStop;
use crate::sort::SortShared;
use crate::Options;

use super::adaptive_split::add_adaptive_split_cut;
use super::{
    add_cut, add_eighth_cuts, include_inside_match_cuts, make_range, plan_structural_ranges,
    total_bits, Composite, Cut,
};

/// Holds every source-split cut: seven decoded-eighth cuts, their seven
/// inside-match siblings and at most fourteen 32-token anchors.
const MAX_SOURCE_SPLIT_CUTS: usize = 32;
/// Max mode adds a split anchor every 32 tokens to source blocks of at most
/// this many tokens.
pub(crate) const COMPACT_SOURCE_SPLIT_MAX_TOKENS: usize = 512;
/// Four bytes: a marginal adaptive split can trigger a whole-stream replay that
/// costs far more than it saves.
const ADAPTIVE_SPLIT_MIN_EXACT_SAVINGS_BITS: u64 = 32;
/// Only near-tied first splits justify an extra child refinement in Default
/// mode; one added header rarely recovers a wider gap.
const NESTED_RUNNER_UP_MARGIN_BITS: u64 = 64;

fn search_split_before_whole_block(
    exhaustive: bool,
    best_split_bits: Option<u64>,
    unsplit_floor_bits: u64,
) -> bool {
    exhaustive && best_split_bits.is_some_and(|bits| bits < unsplit_floor_bits)
}

/// Plan one source block, trying bounded single-boundary splits.
///
/// Default tries the seven decoded-eighth cuts. Max also tries 32-token cuts on
/// compact blocks and a Turtledeflate-inspired adaptive cut, then refines split
/// children with token search.
pub(super) fn plan_source_with_splits(
    block: &ParsedBlock,
    alignment: u8,
    options: &Options,
    integrated_compact_proven: bool,
    plan_cache: &mut CanonicalPlanCache,
    stop: &mut SearchStop<'_>,
) -> Vec<PlannedBlock> {
    if block.tokens.len() < 16
        || block.plain.len() < 128
        || (!options.exhaustive && block.plain.len() < 32_768)
    {
        let plan = match lookup_block_cached(block, alignment, options, plan_cache) {
            Some(base) => {
                plan_block_with_complete_base_search(block, alignment, options, base, stop)
            }
            None => plan_block_with_search(block, alignment, options, stop),
        };
        return vec![plan];
    }

    let base = if options.exhaustive {
        lookup_block_cached(block, alignment, options, plan_cache)
            .unwrap_or_else(|| plan_block(block, alignment, options, &mut *stop))
    } else {
        match lookup_block_cached(block, alignment, options, plan_cache) {
            Some(base) => {
                plan_block_with_complete_base_search(block, alignment, options, base, stop)
            }
            None => plan_block_with_search(block, alignment, options, stop),
        }
    };
    let complete_base = options
        .exhaustive
        .then(|| try_clone_planned_block(&base))
        .flatten();
    let mut best = vec![base];
    let mut best_bits = total_bits(&best);
    let unsplit_floor_bits = best_bits;
    if stop.reached() {
        return best;
    }

    let Some(composite) = Composite::new(std::slice::from_ref(block)) else {
        return best;
    };
    let source = composite.sources[0];
    let start = Cut { token: 0, plain: 0 };
    let end = Cut {
        token: block.tokens.len(),
        plain: block.plain.len(),
    };
    let mut cuts = Vec::new();
    if add_eighth_cuts(
        &mut cuts,
        &composite,
        source.token_start,
        source.token_end,
        source.plain_start,
        source.plain_end,
        include_inside_match_cuts(options.exhaustive, block.tokens.len()),
    )
    .is_none()
    {
        return best;
    }
    if options.exhaustive && block.tokens.len() <= COMPACT_SOURCE_SPLIT_MAX_TOKENS {
        for token in (32..block.tokens.len().saturating_sub(32)).step_by(32) {
            if add_cut(&mut cuts, &composite, token).is_none() {
                return best;
            }
        }
    }
    // `push_cut` keeps one cut per decoded offset, so the key breaks every tie.
    cuts.sort_shared_by_key(|&cut| {
        (
            usize::from(!composite.cut_is_token_boundary(cut)),
            cut.plain,
        )
    });
    cuts.dedup_by_key(|cut| cut.plain);
    cuts.truncate(MAX_SOURCE_SPLIT_CUTS);

    // Price every cut structurally first: the whole-block Max beam search can
    // use up the deadline before a useful split is reached.
    let mut ranked_splits = Vec::new();
    if ranked_splits.try_reserve_exact(cuts.len()).is_err() {
        return best;
    }
    for &split in &cuts {
        if stop.reached() {
            break;
        }
        let boundaries = [start, split, end];
        let Some(candidate) = plan_structural_ranges(
            &composite,
            &boundaries,
            alignment,
            options,
            plan_cache,
            stop,
        ) else {
            continue;
        };
        let candidate_bits = total_bits(&candidate);
        ranked_splits.push((candidate_bits, split));
        if candidate_bits < best_bits {
            best_bits = candidate_bits;
            best = candidate;
        }
    }

    ranked_splits.sort_shared_by_key(|&(bits, split)| (bits, split.plain, split.token));

    // The adaptive probe is cheap (at most 128 histogram samples, one priced
    // cut), so it runs before the costlier searches.
    if options.exhaustive && !stop.reached() {
        if let Some(candidate) = plan_adaptive_split(
            &composite, start, end, &cuts, alignment, options, plan_cache, stop,
        ) {
            let candidate_bits = total_bits(&candidate);
            if adaptive_split_is_worth_replay(candidate_bits, best_bits) {
                best_bits = candidate_bits;
                best = candidate;
            }
        }
    }

    let mut refined_splits = 0;
    if search_split_before_whole_block(
        options.exhaustive,
        ranked_splits.first().map(|&(bits, _)| bits),
        unsplit_floor_bits,
    ) && !stop.reached()
    {
        if let Some(&(_, split)) = ranked_splits.first() {
            if let Some(candidate) =
                plan_searched_split(&composite, start, split, end, alignment, options, stop)
            {
                let candidate_bits = total_bits(&candidate);
                if candidate_bits < best_bits {
                    best_bits = candidate_bits;
                    best = candidate;
                }
            }
            refined_splits = 1;
        }
    }

    // Compact Max blocks have a separate proven-first route, so unless
    // `integrated_compact_proven` is set they use the ordinary state order plus
    // the bounded proven and short-length floors. Larger blocks use the
    // integrated order: their completed comparison route covers the ordinary
    // one.
    if options.exhaustive && !stop.reached() {
        let searched_base = match complete_base {
            Some(base)
                if block.tokens.len() <= COMPACT_SOURCE_SPLIT_MAX_TOKENS
                    && !integrated_compact_proven =>
            {
                let base =
                    plan_block_with_complete_base_search(block, alignment, options, base, stop);
                let base = improve_plan_with_integrated_proven_floor(
                    block, alignment, options, true, base,
                );
                improve_plan_with_short_family_floor(block, options, base)
            }
            Some(base) => plan_block_with_complete_integrated_proven_search(
                block, alignment, options, base, stop,
            ),
            None => plan_block_with_search(block, alignment, options, stop),
        };
        if searched_base.bits < best_bits {
            best_bits = searched_base.bits;
            best = vec![searched_base];
        }
    }

    if !options.exhaustive {
        // A near-tied runner-up first split can win after its larger child is
        // split again. Whole-stream replay already does this for the winning
        // split.
        if ranked_splits.len() >= 2
            && ranked_splits[1].0
                <= ranked_splits[0]
                    .0
                    .saturating_add(NESTED_RUNNER_UP_MARGIN_BITS)
        {
            let outer = ranked_splits[1].1;
            let left_plain = outer.plain - start.plain;
            let right_plain = end.plain - outer.plain;
            let (child_start, child_end) = if left_plain >= right_plain {
                (start, outer)
            } else {
                (outer, end)
            };
            if stop.reached() {
                return best;
            }
            // Default tries the inside-match midpoint only on small sources;
            // pricing both cuts on larger ones could use up the deadline.
            for inner in midpoint_cuts(&composite, child_start, child_end)
                .into_iter()
                .take(if include_inside_match_cuts(false, block.tokens.len()) {
                    2
                } else {
                    1
                })
                .flatten()
            {
                if stop.reached() {
                    return best;
                }
                let boundaries = if inner.plain < outer.plain {
                    [start, inner, outer, end]
                } else {
                    [start, outer, inner, end]
                };
                let Some(candidate) = plan_structural_ranges(
                    &composite,
                    &boundaries,
                    alignment,
                    options,
                    plan_cache,
                    stop,
                ) else {
                    return best;
                };
                let candidate_bits = total_bits(&candidate);
                if candidate_bits < best_bits {
                    best_bits = candidate_bits;
                    best = candidate;
                }
            }
        }
        return best;
    }
    if stop.reached() {
        return best;
    }

    // Refine split children while time remains, cheapest direct price first;
    // the ranking keeps Max mode deterministic.
    for (_, split) in ranked_splits.into_iter().skip(refined_splits) {
        if stop.reached() {
            break;
        }
        let Some(candidate) =
            plan_searched_split(&composite, start, split, end, alignment, options, stop)
        else {
            continue;
        };
        let candidate_bits = total_bits(&candidate);
        if candidate_bits < best_bits {
            best_bits = candidate_bits;
            best = candidate;
        }
    }
    best
}

fn plan_searched_split(
    composite: &Composite<'_>,
    start: Cut,
    split: Cut,
    end: Cut,
    alignment: u8,
    options: &Options,
    stop: &mut SearchStop<'_>,
) -> Option<Vec<PlannedBlock>> {
    let left = make_range(composite, start, split)?;
    let left_plan = plan_block_with_search(&left, alignment, options, stop);
    if stop.reached() {
        return None;
    }
    let right_alignment = ((u64::from(alignment) + left_plan.bits) & 7) as u8;
    let right = make_range(composite, split, end)?;
    let right_plan = plan_block_with_search(&right, right_alignment, options, stop);
    Some(vec![left_plan, right_plan])
}

#[allow(clippy::too_many_arguments)]
fn plan_adaptive_split(
    composite: &Composite<'_>,
    start: Cut,
    end: Cut,
    established_cuts: &[Cut],
    alignment: u8,
    options: &Options,
    plan_cache: &mut CanonicalPlanCache,
    stop: &mut SearchStop<'_>,
) -> Option<Vec<PlannedBlock>> {
    let mut adaptive_cut = Vec::new();
    add_adaptive_split_cut(
        &mut adaptive_cut,
        composite,
        start,
        end,
        options.strict,
        stop,
    )?;
    let &split = adaptive_cut.first()?;
    if established_cuts.contains(&split) {
        return None;
    }

    let boundaries = [start, split, end];
    // Structural planning is enough to validate this cut; child token search
    // costs far more for little gain.
    plan_structural_ranges(composite, &boundaries, alignment, options, plan_cache, stop)
}

fn adaptive_split_is_worth_replay(candidate_bits: u64, established_bits: u64) -> bool {
    candidate_bits
        .checked_add(ADAPTIVE_SPLIT_MIN_EXACT_SAVINGS_BITS)
        .is_some_and(|required_bits| required_bits <= established_bits)
}

/// Return the decoded midpoint snapped down to a token boundary, and the exact
/// midpoint when it falls inside a match.
fn midpoint_cuts(composite: &Composite, start: Cut, end: Cut) -> [Option<Cut>; 2] {
    if end.plain <= start.plain + 1 {
        return [None, None];
    }
    let target = start.plain + (end.plain - start.plain) / 2;
    let token = composite
        .token_plain_offsets
        .partition_point(|&offset| offset <= target)
        .saturating_sub(1);
    let snapped = composite
        .token_plain_offsets
        .get(token)
        .copied()
        .map(|plain| Cut { token, plain })
        .filter(|cut| start.plain < cut.plain && cut.plain < end.plain);
    let exact = composite
        .cut_at_plain(target)
        .filter(|&cut| !composite.cut_is_token_boundary(cut))
        .filter(|cut| Some(*cut) != snapped)
        .filter(|cut| start.plain < cut.plain && cut.plain < end.plain);
    [snapped, exact]
}

#[cfg(test)]
mod tests;
