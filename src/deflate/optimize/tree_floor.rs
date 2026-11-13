// SPDX-License-Identifier: MIT
// SPDX-FileCopyrightText: 2026 Andrew C.E. Dent <https://github.com/ace-dent>

//! Terminal tree floors, which re-price the payload trees of a completed
//! candidate's blocks without changing tokens: the compact payload-tree floor
//! and its balanced closure, the bounded-depth floor and its rescue after the
//! hard stop, and the compact balanced-tree floor (D3, M4). R18 runs them
//! again at the end of the finish.

use crate::progress::Progress;
use crate::{Options, Result};

use super::{
    build_candidate_from_plans, candidate_progress, parse_validated_rewrite, rewritten_input,
    Candidate, CandidateInput, DefaultFloorWork, ReplayPlanner, StreamIdentity,
    COMPACT_TREE_MAX_COMPRESSED, COMPACT_TREE_MAX_DECODED, COMPACT_TREE_MAX_TOKENS,
    NARROW_SOURCE_LIST_MAX_BLOCKS,
};
use crate::deflate::block::reusable_original_bits;
use crate::deflate::header::{
    plan_bounded_depth_tree_candidate, plan_columbo_balanced_tree_candidate,
    plan_for_explicit_lengths, plan_rle_smoothed_tree_candidate,
};
use crate::deflate::model::{PlannedBlock, Representation, SourceBlockType};
use crate::deflate::stop::SearchStop;

const RLE_SMOOTHED_TREE_FLOOR_MAX_BLOCKS: usize = 8;
/// Limits for the bounded-depth tree rescue after the hard stop; its block
/// limit is [`NARROW_SOURCE_LIST_MAX_BLOCKS`].
const BOUNDED_DEPTH_RESCUE_MAX_COMPRESSED: usize = 1_024 * 1_024;
const BOUNDED_DEPTH_RESCUE_MAX_DECODED: u64 = 1_024 * 1_024;

/// Apply the terminal tree floors to one complete candidate, keeping the
/// candidates it replaced for Max's final close.
pub(super) fn improve_with_terminal_tree_floors(
    source: CandidateInput<'_>,
    options: &Options,
    floor_work: DefaultFloorWork<'_>,
    progress: Progress,
    mut candidate: Candidate,
) -> Result<Candidate> {
    let parents = std::mem::take(&mut candidate.replaced_parents);
    let mut closed =
        improve_with_terminal_tree_floors_inner(source, options, floor_work, progress, candidate)?;
    closed.replaced_parents.extend(parents);
    Ok(closed)
}

/// Apply the terminal tree floors to one complete candidate. Keep every
/// Default tree-only pass here, so Max cannot fall behind Default.
fn improve_with_terminal_tree_floors_inner(
    source: CandidateInput<'_>,
    options: &Options,
    floor_work: DefaultFloorWork<'_>,
    progress: Progress,
    mut candidate: Candidate,
) -> Result<Candidate> {
    // The compact tree size limits bound the smoothers' reparse; the completed
    // output's blocks, not the source's, decide whether the floor applies.
    let smoothed_tree_eligible = (floor_work.is_mandatory() || !options.timeout.is_zero())
        && source.compressed.len() <= COMPACT_TREE_MAX_COMPRESSED
        && source.identity.decoded_size <= COMPACT_TREE_MAX_DECODED;
    let mut bounded_depth_covered = false;
    if smoothed_tree_eligible {
        let tree_step = progress.start("Compact payload-tree floor");
        let (covered, tree) = refine_with_compact_payload_tree_floor(
            &candidate,
            options,
            source.decoded_limit,
            source.identity,
        )?;
        bounded_depth_covered = covered;
        tree_step.finish(tree.as_ref().map(|tree| {
            candidate_progress(
                tree,
                source.meaningful_bits,
                tree.is_strictly_smaller_than_source(source),
            )
        }));
        let compact_payload_tree_won = tree
            .map(|tree| candidate.replace_if_smaller(tree))
            .unwrap_or(false);
        if compact_payload_tree_won {
            let closure_step = progress.start("Compact payload-tree balanced closure");
            let closure = refine_with_compact_payload_tree_balanced_closure(
                &candidate,
                options,
                source.decoded_limit,
                source.identity,
            )?;
            closure_step.finish(closure.as_ref().map(|closure| {
                candidate_progress(
                    closure,
                    source.meaningful_bits,
                    closure.is_strictly_smaller_than_source(source),
                )
            }));
            if let Some(closure) = closure {
                candidate.replace_if_smaller(closure);
            }
        }
    }

    // Unless the compact pass covered them, price the bounded-depth trees; after
    // the hard stop, the bounded rescue finishes one fixed unit of work.
    let mut bounded_depth_stop = floor_work.stop();
    let bounded_depth_hard_expired = bounded_depth_stop.reached();
    if !bounded_depth_covered
        && (floor_work.is_mandatory() || !options.timeout.is_zero())
        && (!bounded_depth_hard_expired || bounded_depth_stop.permits_bounded_finalization())
    {
        let tree_step = progress.start("Bounded-depth tree floor");
        let tree = if bounded_depth_hard_expired {
            refine_with_bounded_depth_tree_rescue(
                &candidate,
                options,
                source.decoded_limit,
                source.identity,
            )?
        } else {
            refine_with_bounded_depth_tree_floor(
                &candidate,
                options,
                source.decoded_limit,
                source.identity,
                &mut bounded_depth_stop,
            )?
        };
        tree_step.finish(tree.as_ref().map(|tree| {
            candidate_progress(
                tree,
                source.meaningful_bits,
                tree.is_strictly_smaller_than_source(source),
            )
        }));
        if let Some(tree) = tree {
            candidate.replace_if_smaller(tree);
        }
    }
    Ok(candidate)
}

/// Apply Columbo's bounded pair/quad balanced-tree moves to one dynamic block
/// (D3 and M4), without changing tokens or replaying.
pub(super) fn refine_with_compact_balanced_tree_floor(
    candidate: &Candidate,
    options: &Options,
    decoded_limit: u64,
    identity: StreamIdentity,
) -> Result<Option<Candidate>> {
    let stream =
        parse_validated_rewrite(&candidate.data, decoded_limit, identity, &candidate.plans)?;
    let [block] = stream.blocks.as_slice() else {
        return Ok(None);
    };
    if block.source_type != SourceBlockType::Dynamic || block.tokens.len() > COMPACT_TREE_MAX_TOKENS
    {
        return Ok(None);
    }
    let Some(seed) = block.original_dynamic.as_ref() else {
        return Ok(None);
    };
    // Max omits the standalone literal pair move for streams with matches.
    let price_literal_pair = !options.exhaustive
        || block
            .distance_frequencies
            .iter()
            .all(|&frequency| frequency == 0);
    let dynamic = plan_columbo_balanced_tree_candidate(
        &block.tokens,
        &block.literal_frequencies,
        &block.distance_frequencies,
        seed,
        options.exhaustive,
        price_literal_pair,
    );
    let Some(mut dynamic) = dynamic else {
        return Ok(None);
    };
    // Default ranks the moves with its ordinary header grid, then gives only
    // the winning tree an exhaustive header finalization; Max already prices
    // every tree exhaustively.
    if !options.exhaustive {
        if let Some(finalized) = plan_for_explicit_lengths(
            &block.tokens,
            &dynamic.literal_lengths,
            &dynamic.distance_lengths,
            true,
        ) {
            if finalized.bits < dynamic.bits {
                dynamic = finalized;
            }
        }
    }
    if dynamic.bits >= candidate.bits {
        return Ok(None);
    }

    let plan = PlannedBlock {
        tokens: block.tokens.clone(),
        plain: block.plain.clone(),
        bits: dynamic.bits,
        representation: Representation::Dynamic(dynamic),
        source_type: block.source_type,
    };
    let mut plans = Vec::new();
    if plans.try_reserve_exact(1).is_err() {
        return Ok(None);
    }
    plans.push(plan);
    let source = rewritten_input(candidate, &stream, decoded_limit, identity);
    let mut never_expires = SearchStop::never();
    build_candidate_from_plans(
        source,
        plans,
        options,
        0,
        ReplayPlanner::Full,
        &mut never_expires,
    )
    .map(|candidate| Some(candidate.named("Columbo compact balanced-tree floor")))
}

/// Price every feasible bounded-depth payload tree for each dynamic block of a
/// completed stream. If the stop is reached during a block, that block keeps
/// its best completed tree and later blocks keep their original codes.
pub(super) fn refine_with_bounded_depth_tree_floor(
    candidate: &Candidate,
    options: &Options,
    decoded_limit: u64,
    identity: StreamIdentity,
    expired: &mut SearchStop<'_>,
) -> Result<Option<Candidate>> {
    let stream =
        parse_validated_rewrite(&candidate.data, decoded_limit, identity, &candidate.plans)?;
    if stream.blocks.is_empty() {
        return Ok(None);
    }

    let mut plans = Vec::new();
    if plans.try_reserve_exact(stream.blocks.len()).is_err() {
        return Ok(None);
    }
    let mut alignment = 0_u8;
    let mut improved = false;
    let mut search_open = true;
    for block in &stream.blocks {
        if search_open && expired.reached() {
            search_open = false;
        }
        let Some(original) = reusable_original_bits(block, alignment, options.strict) else {
            return Ok(None);
        };
        let bounded = (search_open && block.source_type == SourceBlockType::Dynamic)
            .then(|| {
                plan_bounded_depth_tree_candidate(
                    &block.tokens,
                    &block.literal_frequencies,
                    &block.distance_frequencies,
                    options.strict,
                    options.exhaustive,
                    options.exhaustive,
                    expired,
                )
            })
            .flatten()
            .filter(|dynamic| dynamic.bits < original.len);
        let (bits, representation) = if let Some(dynamic) = bounded {
            improved = true;
            (dynamic.bits, Representation::Dynamic(dynamic))
        } else {
            (original.len, Representation::Original(original))
        };
        plans.push(PlannedBlock {
            tokens: block.tokens.clone(),
            plain: block.plain.clone(),
            bits,
            representation,
            source_type: block.source_type,
        });
        alignment = ((u64::from(alignment) + bits) & 7) as u8;
    }
    if !improved {
        return Ok(None);
    }

    let source = rewritten_input(candidate, &stream, decoded_limit, identity);
    let rebuilt =
        build_candidate_from_plans(source, plans, options, 0, ReplayPlanner::Full, expired)?;
    Ok((rebuilt.bits < candidate.bits).then(|| rebuilt.named("Bounded-depth tree floor")))
}

/// Run the bounded-depth tree floor to completion after the hard stop (Max
/// only). Ignoring the stop makes the result independent of stop timing; the
/// size and block limits bound the work.
fn refine_with_bounded_depth_tree_rescue(
    candidate: &Candidate,
    options: &Options,
    decoded_limit: u64,
    identity: StreamIdentity,
) -> Result<Option<Candidate>> {
    if !options.exhaustive
        || !bounded_depth_rescue_sizes_are_bounded(candidate.data.len(), identity.decoded_size)
    {
        return Ok(None);
    }
    let stream =
        parse_validated_rewrite(&candidate.data, decoded_limit, identity, &candidate.plans)?;
    if stream.blocks.len() > NARROW_SOURCE_LIST_MAX_BLOCKS {
        return Ok(None);
    }
    if !stream
        .blocks
        .iter()
        .any(|block| block.source_type == SourceBlockType::Dynamic)
    {
        return Ok(None);
    }

    let mut plans = Vec::new();
    if plans.try_reserve_exact(stream.blocks.len()).is_err() {
        return Ok(None);
    }
    let mut alignment = 0_u8;
    let mut improved = false;
    let mut never_expires = SearchStop::never();
    for block in &stream.blocks {
        let Some(original) = reusable_original_bits(block, alignment, options.strict) else {
            return Ok(None);
        };
        let bounded = (block.source_type == SourceBlockType::Dynamic)
            .then(|| {
                plan_bounded_depth_tree_candidate(
                    &block.tokens,
                    &block.literal_frequencies,
                    &block.distance_frequencies,
                    options.strict,
                    true,
                    true,
                    &mut never_expires,
                )
            })
            .flatten()
            .filter(|dynamic| dynamic.bits < original.len);
        let (bits, representation) = if let Some(dynamic) = bounded {
            improved = true;
            (dynamic.bits, Representation::Dynamic(dynamic))
        } else {
            (original.len, Representation::Original(original))
        };
        plans.push(PlannedBlock {
            tokens: block.tokens.clone(),
            plain: block.plain.clone(),
            bits,
            representation,
            source_type: block.source_type,
        });
        alignment = ((u64::from(alignment) + bits) & 7) as u8;
    }
    if !improved {
        return Ok(None);
    }

    let source = rewritten_input(candidate, &stream, decoded_limit, identity);
    let rebuilt = build_candidate_from_plans(
        source,
        plans,
        options,
        0,
        ReplayPlanner::Full,
        &mut never_expires,
    )?;
    Ok((rebuilt.bits < candidate.bits).then(|| rebuilt.named("Bounded-depth tree rescue")))
}

fn bounded_depth_rescue_sizes_are_bounded(compressed_bytes: usize, decoded_bytes: u64) -> bool {
    compressed_bytes <= BOUNDED_DEPTH_RESCUE_MAX_COMPRESSED
        && decoded_bytes <= BOUNDED_DEPTH_RESCUE_MAX_DECODED
}

/// Apply bounded-depth and RLE-smoothed payload trees to a completed compact
/// Huffman stream. The first result says whether the blocks qualified, in
/// which case the bounded-depth trees are already priced.
///
/// This stays out of the block planner: tree prices steer later token
/// feedback, so replacing an intermediate winner can lose a better fixed point.
pub(super) fn refine_with_compact_payload_tree_floor(
    candidate: &Candidate,
    options: &Options,
    decoded_limit: u64,
    identity: StreamIdentity,
) -> Result<(bool, Option<Candidate>)> {
    let stream =
        parse_validated_rewrite(&candidate.data, decoded_limit, identity, &candidate.plans)?;
    if stream.blocks.is_empty()
        || stream.blocks.len() > RLE_SMOOTHED_TREE_FLOOR_MAX_BLOCKS
        || stream
            .blocks
            .iter()
            .any(|block| block.source_type == SourceBlockType::Stored)
        || stream
            .blocks
            .iter()
            .try_fold(0_usize, |count, block| {
                count.checked_add(block.tokens.len())
            })
            .map_or(true, |count| count > COMPACT_TREE_MAX_TOKENS)
    {
        return Ok((false, None));
    }
    let mut plans = Vec::new();
    if plans.try_reserve_exact(stream.blocks.len()).is_err() {
        return Ok((false, None));
    }
    let mut alignment = 0_u8;
    let mut improved = false;
    let mut never_expires = SearchStop::never();
    for block in &stream.blocks {
        let Some(original) = reusable_original_bits(block, alignment, options.strict) else {
            return Ok((false, None));
        };
        let tree_candidate = (block.source_type == SourceBlockType::Dynamic)
            .then(|| {
                let mut best = plan_bounded_depth_tree_candidate(
                    &block.tokens,
                    &block.literal_frequencies,
                    &block.distance_frequencies,
                    options.strict,
                    options.exhaustive,
                    options.exhaustive,
                    &mut never_expires,
                );
                if let Some(smoothed) = plan_rle_smoothed_tree_candidate(
                    &block.tokens,
                    &block.literal_frequencies,
                    &block.distance_frequencies,
                    options.strict,
                ) {
                    if best
                        .as_ref()
                        .map_or(true, |current| smoothed.bits < current.bits)
                    {
                        best = Some(smoothed);
                    }
                }
                best
            })
            .flatten()
            .filter(|dynamic| dynamic.bits < original.len);
        let (bits, representation) = if let Some(dynamic) = tree_candidate {
            improved = true;
            (dynamic.bits, Representation::Dynamic(dynamic))
        } else {
            (original.len, Representation::Original(original))
        };
        plans.push(PlannedBlock {
            tokens: block.tokens.clone(),
            plain: block.plain.clone(),
            bits,
            representation,
            source_type: block.source_type,
        });
        alignment = ((u64::from(alignment) + bits) & 7) as u8;
    }
    if !improved {
        return Ok((true, None));
    }
    let source = rewritten_input(candidate, &stream, decoded_limit, identity);
    let mut never_expires = SearchStop::never();
    let rebuilt = build_candidate_from_plans(
        source,
        plans,
        options,
        0,
        ReplayPlanner::Full,
        &mut never_expires,
    )?;
    Ok((
        true,
        (rebuilt.bits < candidate.bits).then(|| rebuilt.named("Compact payload-tree floor")),
    ))
}

/// After a winning payload-tree floor, price one balanced-tree pass (the
/// smoothed tree can allow a profitable pair/quad Kraft move), then smooth
/// again. More rounds slow Default for little gain.
fn refine_with_compact_payload_tree_balanced_closure(
    candidate: &Candidate,
    options: &Options,
    decoded_limit: u64,
    identity: StreamIdentity,
) -> Result<Option<Candidate>> {
    let Some(mut closed) =
        refine_with_compact_balanced_tree_floor(candidate, options, decoded_limit, identity)?
    else {
        return Ok(None);
    };
    if let Some(smoothed) =
        refine_with_compact_payload_tree_floor(&closed, options, decoded_limit, identity)?.1
    {
        closed.replace_if_smaller(smoothed);
    }
    Ok(Some(closed.named("Compact payload-tree balanced closure")))
}

#[cfg(test)]
mod tests;
