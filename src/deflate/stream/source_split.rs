// SPDX-License-Identifier: MIT
// SPDX-FileCopyrightText: 2026 Andrew C.E. Dent <https://github.com/ace-dent>

//! Split source blocks during the sequential walk (the source split family).
//!
//! Small blocks get only the whole-block search. Larger ones price their
//! decoded-eighth cuts, and the cuts' inside-match siblings, structurally
//! first, because the whole-block Max search can use up the deadline before a
//! useful split is reached. Max adds 32-token cuts on compact blocks and the
//! adaptive probe's cut, then token-searches the split children, which a walk
//! over several blocks defers after the first cuts ([`DeferredSplits`]);
//! Default may split the larger child of a near-tied runner-up split again. A
//! split replaces the block only with strictly fewer bits.

use std::sync::Arc;

use crate::deflate::block::{lookup_block_cached, plan_block, CanonicalPlanCache};
use crate::deflate::model::{ParsedBlock, PlannedBlock, Representation, Token};
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

/// Split refinements a block's own search runs in a walk that defers the rest
/// ([`DeferredSplits`]). On 24 APNG frames at a 33-second allowance the first
/// two ranked cuts gave 690 of the 757 bits that refinement found.
const HEAD_SPLIT_REFINEMENTS: usize = 2;

/// The split refinements a Max walk has put off until every source block has
/// had its whole-block search.
///
/// Refining a block's ranked cuts takes most of a source block's search time
/// and finds little after the first cuts, while each block's own search may
/// use all the time left. Run in block order, the first blocks' refinements
/// therefore used up a short allowance, and later blocks kept their unsearched
/// plans. A first walk records each block's result and its unrefined cuts
/// here; a second walk resumes from them.
#[derive(Default)]
pub(super) struct DeferredSplits {
    resume: bool,
    blocks: Vec<DeferredSplit>,
}

struct DeferredSplit {
    /// Identifies the source block: both walks plan the same parsed blocks.
    tokens: Arc<Vec<Token>>,
    alignment: u8,
    best: Vec<PlannedBlock>,
    /// The ranked cuts not yet refined, in ranked order.
    unrefined: Vec<(u64, Cut)>,
}

impl DeferredSplits {
    /// Whether the first walk left any cut unrefined.
    pub(super) fn has_unrefined(&self) -> bool {
        self.blocks.iter().any(|block| !block.unrefined.is_empty())
    }

    /// Make the next walk resume from the recorded blocks.
    pub(super) fn resume(&mut self) {
        self.resume = true;
    }

    /// Take the record of `block`. A record made at another alignment is
    /// reused only when none of its plans is stored or original bits, whose
    /// cost or validity depends on the alignment.
    fn take(&mut self, block: &ParsedBlock, alignment: u8) -> Option<DeferredSplit> {
        let index = self.blocks.iter().position(|recorded| {
            Arc::ptr_eq(&recorded.tokens, &block.tokens)
                && (recorded.alignment == alignment
                    || recorded.best.iter().all(|plan| {
                        matches!(
                            plan.representation,
                            Representation::Fixed | Representation::Dynamic(_)
                        )
                    }))
        })?;
        Some(self.blocks.swap_remove(index))
    }

    fn record(
        &mut self,
        block: &ParsedBlock,
        alignment: u8,
        best: &[PlannedBlock],
        unrefined: Vec<(u64, Cut)>,
    ) {
        let mut copy = Vec::new();
        if copy.try_reserve_exact(best.len()).is_err() || self.blocks.try_reserve(1).is_err() {
            return;
        }
        for plan in best {
            let Some(plan) = try_clone_planned_block(plan) else {
                return;
            };
            copy.push(plan);
        }
        self.blocks.push(DeferredSplit {
            tokens: Arc::clone(&block.tokens),
            alignment,
            best: copy,
            unrefined,
        });
    }
}

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
    mut deferred: Option<&mut DeferredSplits>,
    stop: &mut SearchStop<'_>,
) -> Vec<PlannedBlock> {
    if let Some(recorded) = deferred
        .as_deref_mut()
        .filter(|deferred| deferred.resume)
        .and_then(|deferred| deferred.take(block, alignment))
    {
        let mut best = recorded.best;
        if recorded.unrefined.is_empty() || stop.reached() {
            return best;
        }
        let Some(composite) = Composite::new(std::slice::from_ref(block)) else {
            return best;
        };
        let end = Cut {
            token: block.tokens.len(),
            plain: block.plain.len(),
        };
        refine_ranked_splits(
            &composite,
            end,
            alignment,
            options,
            &recorded.unrefined,
            &mut best,
            stop,
        );
        return best;
    }
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
    let unrefined = ranked_splits.get(refined_splits..).unwrap_or_default();
    let Some(deferred) = deferred.filter(|deferred| !deferred.resume) else {
        refine_ranked_splits(
            &composite, end, alignment, options, unrefined, &mut best, stop,
        );
        return best;
    };
    let (head, rest) = unrefined.split_at(
        HEAD_SPLIT_REFINEMENTS
            .saturating_sub(refined_splits)
            .min(unrefined.len()),
    );
    refine_ranked_splits(&composite, end, alignment, options, head, &mut best, stop);
    if !stop.reached() {
        let mut unrefined = Vec::new();
        if unrefined.try_reserve_exact(rest.len()).is_ok() {
            unrefined.extend_from_slice(rest);
            deferred.record(block, alignment, &best, unrefined);
        }
    }
    best
}

/// Token-search the children of each ranked cut in turn, keeping a split that
/// is strictly smaller than `best`.
#[inline(never)]
fn refine_ranked_splits(
    composite: &Composite<'_>,
    end: Cut,
    alignment: u8,
    options: &Options,
    ranked_splits: &[(u64, Cut)],
    best: &mut Vec<PlannedBlock>,
    stop: &mut SearchStop<'_>,
) {
    let start = Cut { token: 0, plain: 0 };
    let mut best_bits = total_bits(best);
    for &(_, split) in ranked_splits {
        if stop.reached() {
            break;
        }
        let Some(candidate) =
            plan_searched_split(composite, start, split, end, alignment, options, stop)
        else {
            continue;
        };
        let candidate_bits = total_bits(&candidate);
        if candidate_bits < best_bits {
            best_bits = candidate_bits;
            *best = candidate;
        }
    }
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
