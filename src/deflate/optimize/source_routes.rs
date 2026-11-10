// SPDX-License-Identifier: MIT
// SPDX-FileCopyrightText: 2026 Andrew C.E. Dent <https://github.com/ace-dent>

//! Whole-stream route builders on the source and on rewritten lineages: the
//! deft4j source route (M1), the no-split route and its continuation (M2,
//! M2c), D4's source-layout candidate and original-source Max (source max),
//! with the Default, Max and terminal-merge replans that continue a lineage.

use crate::progress::{Progress, RouteProgress};
use crate::{Options, Result};

use super::gates::{deft4j_source_route_eligible, narrow_source_route_eligible};
use super::{
    build_candidate, build_candidate_from_established_floor, build_candidate_from_plans,
    build_candidate_from_plans_with_progress, candidate_progress, parse_validated_rewrite,
    rewritten_input, source_candidate, Candidate, CandidateInput, ReplayPlanner, StreamIdentity,
    CUMULATIVE_NO_SPLIT_MIN_SOURCE_BLOCKS, DEFAULT_RAW_REPLAY_LIMIT, MAX_RAW_REPLAY_LIMIT,
};
use crate::deflate::block::plan_block;
use crate::deflate::model::{same_tokens, ParsedBlock};
use crate::deflate::search::{
    plan_block_with_seeded_narrow_search, plan_length_family_chain, plan_respell_from_literals,
};
use crate::deflate::source_recode::plan_source_blocks;
use crate::deflate::stop::{Deadline, SearchStop};
use crate::deflate::stream::{
    plan_source_individual_no_split_route, plan_source_no_split_route, plan_stream_with_progress,
    plan_terminal_merge_route,
};

/// Run one original-source Max search, with progress heartbeats only when
/// reports are enabled.
pub(super) fn build_source_max_candidate(
    source: CandidateInput<'_>,
    options: &Options,
    progress: Progress,
    deadline: &Deadline,
    integrated_compact_proven: bool,
    expired: &mut SearchStop<'_>,
) -> Result<Candidate> {
    if !progress.enabled() {
        let candidate = build_candidate_from_established_floor(
            source,
            options,
            MAX_RAW_REPLAY_LIMIT,
            integrated_compact_proven,
            true,
            expired,
        )?;
        return Ok(if candidate.route == "Original source" {
            candidate
        } else {
            candidate.named("Columbo source max route")
        });
    }

    build_source_max_candidate_verbose(
        source,
        options,
        progress,
        deadline,
        integrated_compact_proven,
        expired,
    )
}

fn build_source_max_candidate_verbose(
    source: CandidateInput<'_>,
    options: &Options,
    progress: Progress,
    deadline: &Deadline,
    integrated_compact_proven: bool,
    expired: &mut SearchStop<'_>,
) -> Result<Candidate> {
    let (step, details) = progress.start_detailed(
        "Columbo source max route",
        source.meaningful_bits,
        deadline.remaining(),
    );
    let mut monitored_expired = || {
        details.heartbeat();
        if deadline.soft_expired() {
            details.finalizing_after_soft_deadline(deadline.grace);
        }
        let should_stop = expired.reached();
        if should_stop && deadline.was_triggered() {
            details.deadline_reached();
        }
        should_stop
    };
    let mut monitored_stop = SearchStop::callback(&mut monitored_expired);
    match build_candidate_with_progress(
        source,
        options,
        MAX_RAW_REPLAY_LIMIT,
        integrated_compact_proven,
        &mut monitored_stop,
        &details,
    ) {
        Ok(candidate) if candidate.route == "Original source" => {
            details.stopped("No complete source-max plan was available");
            step.finish(None);
            Ok(candidate)
        }
        Ok(candidate) => {
            let candidate = candidate.named("Columbo source max route");
            step.finish(Some(candidate_progress(
                &candidate,
                source.meaningful_bits,
                candidate.is_strictly_smaller_than_source(source),
            )));
            Ok(candidate)
        }
        Err(error) => {
            step.fail();
            Err(error)
        }
    }
}

/// Build the direct deft4j-derived source candidate (M1) without replay.
pub(super) fn build_deft4j_source_candidate(
    source: CandidateInput<'_>,
    options: &Options,
    expired: &mut SearchStop<'_>,
) -> Result<Option<Candidate>> {
    let Some(plans) = plan_source_blocks(source.blocks, 0, options, &mut *expired) else {
        return Ok(None);
    };
    build_candidate_from_plans(source, plans, options, 0, ReplayPlanner::Full, expired)
        .map(|candidate| Some(candidate.named("deft4j-derived source")))
}

/// Build D4's candidate in the source's own block layout. Each nonempty block
/// is seeded with the smallest of its ordinary plan, its length-family chain
/// and its re-spell from literals, then searched from that seed with the
/// no-split route's narrow whole-block search, which adds the large-source
/// length bands. A stop reached before a block starts leaves no candidate; a
/// block whose search the stop cuts short keeps its best plan so far.
pub(super) fn build_source_family_candidate(
    source: CandidateInput<'_>,
    options: &Options,
    stop: &mut SearchStop<'_>,
) -> Result<Option<Candidate>> {
    let mut plans = Vec::new();
    if plans.try_reserve_exact(source.blocks.len()).is_err() {
        return Ok(None);
    }
    let mut alignment = 0_u8;
    for block in source.blocks.iter().filter(|block| !block.plain.is_empty()) {
        if stop.reached() {
            return Ok(None);
        }
        let ordinary = plan_block(block, alignment, options, &mut *stop);
        let mut seed = plan_length_family_chain(block, alignment, options, ordinary.bits, stop)
            .unwrap_or(ordinary);
        if let Some(respelled) =
            plan_respell_from_literals(block, alignment, options, seed.bits, stop)
        {
            seed = respelled;
        }
        let plan =
            plan_block_with_seeded_narrow_search(block, alignment, options, false, seed, stop);
        alignment = ((u64::from(alignment) + plan.bits) & 7) as u8;
        plans.push(plan);
    }
    build_candidate_from_plans(source, plans, options, 0, ReplayPlanner::Full, stop)
        .map(|candidate| Some(candidate.named("source layout")))
}

/// Build the no-split source candidate (M2) with the pruning policy chosen by
/// block count.
pub(super) fn build_narrow_source_candidate(
    source: CandidateInput<'_>,
    options: &Options,
    expired: &mut SearchStop<'_>,
    refinement_stop: &mut SearchStop<'_>,
) -> Result<Option<Candidate>> {
    let individual_prune = !cumulative_no_split_has_priority(source.blocks);
    build_narrow_source_candidate_with_policy(
        source,
        options,
        individual_prune,
        expired,
        refinement_stop,
    )
}

pub(super) fn build_complementary_narrow_source_candidate(
    source: CandidateInput<'_>,
    options: &Options,
    expired: &mut SearchStop<'_>,
    refinement_stop: &mut SearchStop<'_>,
) -> Result<Option<Candidate>> {
    let individual_prune = cumulative_no_split_has_priority(source.blocks);
    build_narrow_source_candidate_with_policy(
        source,
        options,
        individual_prune,
        expired,
        refinement_stop,
    )
}

fn cumulative_no_split_has_priority(blocks: &[ParsedBlock]) -> bool {
    let nonempty_blocks = blocks
        .iter()
        .filter(|block| !block.plain.is_empty())
        .count();
    cumulative_no_split_count_has_priority(nonempty_blocks)
}

fn cumulative_no_split_count_has_priority(nonempty_blocks: usize) -> bool {
    nonempty_blocks >= CUMULATIVE_NO_SPLIT_MIN_SOURCE_BLOCKS
}

fn build_narrow_source_candidate_with_policy(
    source: CandidateInput<'_>,
    options: &Options,
    individual_prune: bool,
    expired: &mut SearchStop<'_>,
    refinement_stop: &mut SearchStop<'_>,
) -> Result<Option<Candidate>> {
    let plans = if individual_prune {
        plan_source_individual_no_split_route(source.blocks, 0, options, &mut *expired)
    } else {
        plan_source_no_split_route(source.blocks, 0, options, &mut *expired)
    };
    let Some(plans) = plans else {
        return Ok(None);
    };
    let mut candidate =
        build_candidate_from_plans(source, plans, options, 0, ReplayPlanner::Full, expired)?;
    if candidate_exposes_new_parent(&candidate, source) && !refinement_stop.reached() {
        let refined = refine_with_default_planner(
            &candidate,
            options,
            source.decoded_limit,
            source.identity,
            refinement_stop,
        )?;
        // A deadline can stop the refinement, so Max keeps the walk it
        // replaces.
        candidate.replace_keeping_parent(refined, options.exhaustive);
    }
    Ok(Some(candidate.named(if individual_prune {
        "Individual no-split source"
    } else {
        "No-split source"
    })))
}

/// Whether a route changed block boundaries or token spellings, giving the
/// Default planner a new input worth one replay. A new Huffman header alone
/// does not: Max has already priced those tokens with a superset of headers.
pub(super) fn candidate_exposes_new_parent(
    candidate: &Candidate,
    source: CandidateInput<'_>,
) -> bool {
    candidate.data.as_slice() != source.compressed
        && (candidate.plans.len() != source.blocks.len()
            || candidate
                .plans
                .iter()
                .zip(source.blocks)
                .any(|(plan, block)| {
                    plan.plain.len() != block.plain.len()
                        || (!std::sync::Arc::ptr_eq(&plan.tokens, &block.tokens)
                            && !same_tokens(&plan.tokens, &block.tokens))
                }))
}

/// Run the deft4j route on a rewritten floor, for when the direct deft4j gain
/// is weak.
pub(super) fn build_deft4j_seed_candidate(
    candidate: &Candidate,
    options: &Options,
    decoded_limit: u64,
    identity: StreamIdentity,
    expired: &mut SearchStop<'_>,
) -> Result<Option<Candidate>> {
    let stream = parse_validated_rewrite(&candidate.data, decoded_limit, identity)?;
    if !deft4j_source_route_eligible(&stream.blocks) {
        return Ok(None);
    }
    let source = rewritten_input(candidate, &stream, decoded_limit, identity);
    build_deft4j_source_candidate(source, options, expired)
}

pub(super) fn refine_with_default_planner(
    candidate: &Candidate,
    options: &Options,
    decoded_limit: u64,
    identity: StreamIdentity,
    expired: &mut SearchStop<'_>,
) -> Result<Candidate> {
    refine_with_default_planner_and_change(candidate, options, decoded_limit, identity, expired)
        .map(|(candidate, _)| candidate)
}

/// Replan a rewritten candidate with the Default planner and report whether the
/// result changed blocks or tokens.
pub(super) fn refine_with_default_planner_and_change(
    candidate: &Candidate,
    options: &Options,
    decoded_limit: u64,
    identity: StreamIdentity,
    expired: &mut SearchStop<'_>,
) -> Result<(Candidate, bool)> {
    let stream = parse_validated_rewrite(&candidate.data, decoded_limit, identity)?;
    let source = rewritten_input(candidate, &stream, decoded_limit, identity);
    let mut floor_options = options.clone();
    floor_options.exhaustive = false;
    let refined = build_candidate(source, &floor_options, DEFAULT_RAW_REPLAY_LIMIT, expired)?
        .named("Default refinement");
    let exposes_new_parent = candidate_exposes_new_parent(&refined, source);
    Ok((refined, exposes_new_parent))
}

pub(super) fn refine_with_no_split_route(
    candidate: &Candidate,
    options: &Options,
    decoded_limit: u64,
    identity: StreamIdentity,
    route_stop: &mut SearchStop<'_>,
    refinement_stop: &mut SearchStop<'_>,
) -> Result<Option<Candidate>> {
    let stream = parse_validated_rewrite(&candidate.data, decoded_limit, identity)?;
    if !narrow_source_route_eligible(&stream.blocks, candidate.data.len()) {
        return Ok(None);
    }
    let source = rewritten_input(candidate, &stream, decoded_limit, identity);
    build_narrow_source_candidate(source, options, route_stop, refinement_stop)
}

pub(super) fn changed_parent_no_split_should_continue(
    exposes_new_parent: bool,
    refined: &Candidate,
    parent: &Candidate,
    completed_no_split: Option<&Candidate>,
) -> bool {
    exposes_new_parent
        && refined.is_strictly_smaller_than(parent)
        && completed_no_split.is_some_and(|no_split| refined.is_strictly_smaller_than(no_split))
}

/// Whether a completed no-split candidate gets one no-split continuation
/// (M2c). Sibling sizes do not decide this, since a smaller sibling may have a
/// different topology.
pub(super) fn changed_narrow_parent_should_continue(
    exposes_new_parent: bool,
    narrow: &Candidate,
    source: CandidateInput<'_>,
) -> bool {
    exposes_new_parent && narrow.is_strictly_smaller_than_source(source)
}

pub(super) fn refine_with_max_planner(
    candidate: &Candidate,
    options: &Options,
    decoded_limit: u64,
    identity: StreamIdentity,
    expired: &mut SearchStop<'_>,
) -> Result<Candidate> {
    let stream = parse_validated_rewrite(&candidate.data, decoded_limit, identity)?;
    let source = rewritten_input(candidate, &stream, decoded_limit, identity);
    build_candidate(source, options, MAX_RAW_REPLAY_LIMIT, expired)
        .map(|candidate| candidate.named("Columbo max refinement"))
}

/// Apply one linear merge cleanup to a selected seed: price only adjacent
/// merges with bounded table floors, then settle the smaller list with at most
/// two Default replays while time remains.
pub(super) fn refine_with_terminal_merge(
    candidate: &Candidate,
    options: &Options,
    decoded_limit: u64,
    identity: StreamIdentity,
    expired: &mut SearchStop<'_>,
) -> Result<Option<Candidate>> {
    if expired.reached() {
        return Ok(None);
    }
    let stream = parse_validated_rewrite(&candidate.data, decoded_limit, identity)?;
    let mut floor_options = options.clone();
    floor_options.exhaustive = false;
    let Some(plans) = plan_terminal_merge_route(&stream.blocks, 0, &floor_options, expired) else {
        return Ok(None);
    };
    let source = rewritten_input(candidate, &stream, decoded_limit, identity);
    build_candidate_from_plans(
        source,
        plans,
        &floor_options,
        2,
        ReplayPlanner::Full,
        expired,
    )
    .map(|candidate| Some(candidate.named("Columbo terminal merge")))
}

/// Build the source-max candidate with progress reporting. It is separate so
/// quiet runs do not read clocks or update progress state in hot loops.
fn build_candidate_with_progress(
    source: CandidateInput<'_>,
    options: &Options,
    replay_limit: usize,
    integrated_compact_proven: bool,
    expired: &mut SearchStop<'_>,
    progress: &RouteProgress,
) -> Result<Candidate> {
    let Some(plans) = plan_stream_with_progress(
        source.blocks,
        source.compressed.len(),
        0,
        options,
        true,
        integrated_compact_proven,
        &mut *expired,
        progress,
    ) else {
        return source_candidate(source, options);
    };
    build_candidate_from_plans_with_progress(
        source,
        plans,
        options,
        replay_limit,
        ReplayPlanner::Full,
        expired,
        Some(progress),
        true,
    )
}

#[cfg(test)]
mod tests;
