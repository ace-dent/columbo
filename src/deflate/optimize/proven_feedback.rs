// SPDX-License-Identifier: MIT
// SPDX-FileCopyrightText: 2026 Andrew C.E. Dent <https://github.com/ace-dent>

//! Compact proven-before-feedback candidates (D1, D2, M3): proven-submatch
//! resegmentation runs before table feedback on one compact block, or inside
//! each block's floor and search on a compact multi-block stream.

use crate::{Options, Result};

use super::gates::compact_source_has_bounded_integrated_proven_feedback;
use super::tree_floor::refine_with_compact_balanced_tree_floor;
use super::{
    build_candidate_from_plans, parse_validated_rewrite, rewritten_input, Candidate,
    CandidateInput, ReplayPlanner, StreamIdentity, DEFAULT_RAW_REPLAY_LIMIT,
};
use crate::deflate::block::plan_block;
use crate::deflate::model::PlannedBlock;
use crate::deflate::search::{
    compact_proven_submatch_route_eligible, improve_plan_with_header_aware_proven_composition,
    improve_plan_with_integrated_proven_floor, improve_plan_with_short_family_floor,
    plan_block_with_integrated_proven_search,
};
use crate::deflate::stop::SearchStop;
use crate::deflate::stream::plan_integrated_proven_source_route;

/// Run compact proven feedback on one balanced-tree rewrite, whose new header
/// changes match and literal prices.
pub(super) fn refine_with_compact_proven_feedback(
    candidate: &Candidate,
    options: &Options,
    decoded_limit: u64,
    identity: StreamIdentity,
) -> Result<Option<Candidate>> {
    let stream =
        parse_validated_rewrite(&candidate.data, decoded_limit, identity, &candidate.plans)?;
    let source = rewritten_input(candidate, &stream, decoded_limit, identity);
    let mut never_expires = SearchStop::never();
    build_compact_proven_feedback_candidate(source, options, &mut never_expires)
}

/// Build the compact proven-before-feedback candidate (D1, M3). Default runs
/// both the bounded floor and the full search; Max runs only the floor,
/// because source max covers the full search.
pub(super) fn build_compact_proven_feedback_candidate(
    source: CandidateInput<'_>,
    options: &Options,
    expired: &mut SearchStop<'_>,
) -> Result<Option<Candidate>> {
    let [block] = source.blocks else {
        return Ok(None);
    };
    if !compact_proven_submatch_route_eligible(&block.tokens, block.plain.len())
        || expired.reached()
    {
        return Ok(None);
    }
    let mut route_options = options.clone();
    route_options.exhaustive = false;
    let floor_base = plan_block(block, 0, &route_options, &mut *expired);
    let mut floor_plan =
        improve_plan_with_integrated_proven_floor(block, 0, &route_options, true, floor_base);
    let floor_bits_before_composition = floor_plan.bits;
    // Max's header-aware beam starts from the complete compact floor, which
    // supplies its initial code costs and stays as the fallback.
    if options.exhaustive && !expired.reached() {
        floor_plan = improve_plan_with_header_aware_proven_composition(
            block, 0, options, expired, floor_plan,
        );
    }
    let composition_improved = floor_plan.bits < floor_bits_before_composition;
    let composition_bits = floor_plan.bits;
    let floor_plan = improve_plan_with_short_family_floor(block, &route_options, floor_plan);
    let floor_route = if composition_improved && floor_plan.bits == composition_bits {
        "Columbo header-aware proven feedback"
    } else {
        "Columbo proven-feedback floor"
    };
    // Settle the cheap floor first, so the full search cannot use the floor's
    // time.
    let mut candidate = build_compact_proven_seed_candidate(
        source,
        floor_plan,
        &route_options,
        options,
        floor_route,
        expired,
    )?;
    if options.exhaustive || expired.reached() {
        return Ok(Some(candidate));
    }
    // The full search starts from its own ordinary price, which keeps its tie
    // order.
    let searched_plan =
        plan_block_with_integrated_proven_search(block, 0, &route_options, &mut *expired);
    let searched = build_compact_proven_seed_candidate(
        source,
        searched_plan,
        &route_options,
        options,
        "Columbo proven-feedback floor",
        expired,
    )?;
    candidate.replace_if_smaller(searched);
    Ok(Some(candidate))
}

/// Settle both replay endpoints (ordinary and proven-before-feedback) reachable
/// from one compact seed.
fn build_compact_proven_seed_candidate(
    source: CandidateInput<'_>,
    plan: PlannedBlock,
    route_options: &Options,
    options: &Options,
    route: &'static str,
    expired: &mut SearchStop<'_>,
) -> Result<Candidate> {
    let mut candidate = build_candidate_from_plans(
        source,
        vec![plan.clone()],
        route_options,
        DEFAULT_RAW_REPLAY_LIMIT,
        ReplayPlanner::Full,
        expired,
    )?
    .named(route);
    let integrated = build_candidate_from_plans(
        source,
        vec![plan],
        route_options,
        DEFAULT_RAW_REPLAY_LIMIT,
        ReplayPlanner::IntegratedProven,
        expired,
    )?
    .named(route);
    candidate.replace_if_smaller(integrated);
    // Comparing before the cleanup can hide a locally dearer seed whose table
    // leads to a smaller tree endpoint.
    if let Some(tree) = refine_with_compact_balanced_tree_floor(
        &candidate,
        options,
        source.decoded_limit,
        source.identity,
    )? {
        candidate.replace_if_smaller(tree);
    }
    Ok(candidate)
}

/// Build the compact multi-block integrated proven-feedback candidate (D2):
/// proven resegmentation runs inside each block's floor and search, before
/// table feedback, rather than as a separate endpoint lineage.
pub(super) fn build_compact_integrated_proven_feedback_candidate(
    source: CandidateInput<'_>,
    options: &Options,
    incumbent: &Candidate,
    expired: &mut SearchStop<'_>,
) -> Result<Option<Candidate>> {
    if !compact_source_has_bounded_integrated_proven_feedback(source) || expired.reached() {
        return Ok(None);
    }
    let mut route_options = options.clone();
    route_options.exhaustive = false;
    let Some(plans) =
        plan_integrated_proven_source_route(source.blocks, 0, &route_options, &mut *expired)
    else {
        return Ok(None);
    };
    let integrated_replay_plans = plans.clone();
    let mut candidate = build_candidate_from_plans(
        source,
        plans,
        &route_options,
        DEFAULT_RAW_REPLAY_LIMIT,
        ReplayPlanner::Full,
        expired,
    )?
    .named("Columbo integrated proven feedback");
    // Ordinary replay is cheaper and finds many compact multi-block wins, so
    // try integrated replay only when it did not beat the incumbent.
    if !candidate.is_strictly_smaller_than(incumbent) && !expired.reached() {
        let integrated = build_candidate_from_plans(
            source,
            integrated_replay_plans,
            &route_options,
            DEFAULT_RAW_REPLAY_LIMIT,
            ReplayPlanner::IntegratedProven,
            expired,
        )?
        .named("Columbo integrated proven feedback");
        candidate.replace_if_smaller(integrated);
    }
    Ok(Some(candidate))
}
