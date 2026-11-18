// SPDX-License-Identifier: MIT
// SPDX-FileCopyrightText: 2026 Andrew C.E. Dent <https://github.com/ace-dent>

//! The complete Default floor that Max retains: Default's feedback routes (D1,
//! D2, D3) and tree floors, its finish and terminal header searches, D4's
//! source topology as a mandatory lineage, and the slid and first-route-cut
//! endpoints kept as final competitors. Also APNG's Default endpoint.

use std::time::{Duration, Instant};

use crate::progress::Progress;
use crate::{Error, Options, Result};

use super::gates::{
    compact_source_has_bounded_integrated_proven_feedback,
    compact_source_has_bounded_match_preserving_feedback, compact_strict_literal_tree_eligible,
    default_source_topology_eligible,
};
use super::proven_feedback::{
    build_compact_integrated_proven_feedback_candidate, build_compact_proven_feedback_candidate,
};
use super::source_routes::build_source_family_candidate;
use super::terminal::{
    improve_with_final_header_fit, improve_with_original_match_restoration,
    improve_with_repeated_tree_finish, improve_with_terminal_header_search,
    improve_with_terminal_merges, improve_with_terminal_searches, TerminalHeaderSearch,
    TerminalParseCache,
};
use super::tree_floor::{
    improve_with_terminal_tree_floors, refine_with_compact_balanced_tree_floor,
};
use super::{
    build_candidate, build_candidate_from_plans, candidate_progress, source_candidate, Candidate,
    CandidateInput, DefaultEndpoint, DefaultFloorWork, HeaderFit, ReplayPlanner,
    DEFAULT_RAW_REPLAY_LIMIT, TREE_FINISH_REPEATS,
};
use crate::deflate::stop::{Deadline, SearchStop};
use crate::deflate::stream::plan_stream;

const DEFAULT_STRICT_TREE_ROUNDS: usize = 4;

/// Complete the Default floor with D1, D2, D3 and the terminal tree floors.
/// Max's retained Default floor shares this sequence.
pub(super) fn improve_default_floor_with_feedback(
    source: CandidateInput<'_>,
    options: &Options,
    floor_work: DefaultFloorWork<'_>,
    progress: Progress,
    mut candidate: Candidate,
) -> Result<Candidate> {
    debug_assert!(!options.exhaustive);

    // D1: proven-before-feedback can reach a different fixed point.
    if compact_source_has_bounded_match_preserving_feedback(source) && floor_work.can_start_route()
    {
        let step = progress.start("Columbo match-preserving feedback");
        let contender =
            build_compact_proven_feedback_candidate(source, options, &mut floor_work.stop())?;
        step.finish(contender.as_ref().map(|candidate| {
            candidate_progress(
                candidate,
                source.meaningful_bits,
                candidate.is_strictly_smaller_than_source(source),
            )
        }));
        if let Some(contender) = contender {
            candidate.replace_if_smaller(contender);
        }
    }
    if compact_source_has_bounded_integrated_proven_feedback(source) && floor_work.can_start_route()
    {
        let step = progress.start("Columbo integrated proven feedback");
        let contender = build_compact_integrated_proven_feedback_candidate(
            source,
            options,
            &candidate,
            &mut floor_work.stop(),
        )?;
        step.finish(contender.as_ref().map(|candidate| {
            candidate_progress(
                candidate,
                source.meaningful_bits,
                candidate.is_strictly_smaller_than_source(source),
            )
        }));
        if let Some(contender) = contender {
            candidate.replace_if_smaller(contender);
        }
    }
    if options.strict
        && compact_strict_literal_tree_eligible(source.compressed.len(), source.blocks)
    {
        let tree_step = progress.start("Columbo strict literal-tree cleanup");
        for _ in 0..DEFAULT_STRICT_TREE_ROUNDS {
            let Some(next) = refine_with_compact_balanced_tree_floor(
                &candidate,
                options,
                source.decoded_limit,
                source.identity,
            )?
            else {
                break;
            };
            if !next.is_strictly_smaller_than(&candidate) {
                break;
            }
            candidate = next;
        }
        tree_step.finish(Some(candidate_progress(
            &candidate,
            source.meaningful_bits,
            candidate.is_strictly_smaller_than_source(source),
        )));
    }
    improve_with_terminal_tree_floors(source, options, floor_work, progress, candidate)
}

/// The exact Default result that a Max stream keeps before its Max routes.
pub(super) struct CompleteDefaultFloor {
    /// The Default route's candidate before feedback and terminal methods; it
    /// seeds Max's routes.
    pub(super) max_seed: Candidate,
    /// The complete Default endpoint, which R6–R9 may strengthen within the
    /// Max allowance.
    pub(super) complete: Candidate,
    /// Default's exact endpoint when it differs from `complete` (after its
    /// final R1c, R13–R18 and, for [`HeaderFit::AfterTreeFloors`], R2–R5, or
    /// with R1b), kept only as a final competitor; or Default's endpoint after
    /// a deadline during its first route, when that is smaller.
    pub(super) slid: Option<Candidate>,
    /// When asked for, a copy of the exact endpoint Default itself selects,
    /// taken before the Max-only R6–R9 strengthen `complete`.
    pub(super) exact: Option<DefaultEndpoint>,
}

/// Default's final R1c, R13–R18 and, for [`HeaderFit::AfterTreeFloors`], R2–R5
/// on `complete`, returned only when strictly smaller. It is only a final
/// competitor: boundaries fitted to the current trees can hide an improvement
/// that Max's own tree floors would find.
pub(super) fn slid_default_endpoint(
    source: CandidateInput<'_>,
    floor_options: &Options,
    progress: Progress,
    parse_cache: &mut TerminalParseCache,
    complete: &Candidate,
    header_fit: HeaderFit,
) -> Result<Option<Candidate>> {
    let slid = improve_with_terminal_header_search(
        TerminalHeaderSearch::BoundarySlide,
        source,
        floor_options,
        DefaultFloorWork::Mandatory,
        progress,
        parse_cache,
        complete.clone(),
    )?;
    let slid = improve_with_repeated_tree_finish(
        1 + TREE_FINISH_REPEATS,
        source,
        floor_options,
        DefaultFloorWork::Mandatory,
        progress,
        parse_cache,
        slid,
    )?;
    let slid = improve_with_terminal_merges(
        source,
        floor_options,
        DefaultFloorWork::Mandatory,
        progress,
        parse_cache,
        slid,
    )?;
    // R18, as Default's finish ends.
    let mut slid = improve_with_terminal_tree_floors(
        source,
        floor_options,
        DefaultFloorWork::Mandatory,
        progress,
        slid,
    )?;
    // `complete` ends with R2–R5 in the sweep, so Default's closing header fit
    // runs when the finish changed it.
    if header_fit == HeaderFit::AfterTreeFloors || slid.is_strictly_smaller_than(complete) {
        slid = improve_with_final_header_fit(
            source,
            floor_options,
            DefaultFloorWork::Mandatory,
            progress,
            parse_cache,
            slid,
        )?;
    }
    Ok(slid.is_strictly_smaller_than(complete).then_some(slid))
}

/// Default's R1b–R5 finish for one of Max's mandatory endpoints, returning
/// Max's parent and Default's exact endpoint as a final competitor.
///
/// R1b's uniform complete distance tree changes the header that R2–R9 start
/// from and can lead them to a worse fixed point. So when R1b wins, the parent
/// omits it (Max's sweep applies R1b later), and Default's endpoint with R1b
/// stays a final competitor, so Max never trails Default.
fn finish_mandatory_default_endpoint(
    source: CandidateInput<'_>,
    floor_options: &Options,
    progress: Progress,
    parse_cache: &mut TerminalParseCache,
    base: Candidate,
    header_fit: HeaderFit,
) -> Result<(Candidate, Option<Candidate>)> {
    let completed = improve_with_terminal_header_search(
        TerminalHeaderSearch::StrictDistanceCompletion,
        source,
        floor_options,
        DefaultFloorWork::Mandatory,
        progress,
        parse_cache,
        base.clone(),
    )?;
    let completion_won = completed.is_strictly_smaller_than(&base);
    if header_fit == HeaderFit::AfterTreeFloors {
        // Default's endpoint runs the finish and R18, then R2–R5; Max's parent
        // keeps R2–R5 in Max's sweep order.
        let exact = slid_default_endpoint(
            source,
            floor_options,
            progress,
            parse_cache,
            &completed,
            header_fit,
        )?
        .unwrap_or_else(|| completed.clone());
        let parent = finish_default_header_searches(
            source,
            floor_options,
            progress,
            parse_cache,
            if completion_won { base } else { completed },
        )?;
        let slid = exact.is_strictly_smaller_than(&parent).then_some(exact);
        return Ok((parent, slid));
    }
    let endpoint =
        finish_default_header_searches(source, floor_options, progress, parse_cache, completed)?;
    let slid = slid_default_endpoint(
        source,
        floor_options,
        progress,
        parse_cache,
        &endpoint,
        header_fit,
    )?;
    if !completion_won {
        return Ok((endpoint, slid));
    }
    let parent =
        finish_default_header_searches(source, floor_options, progress, parse_cache, base)?;
    Ok((parent, Some(slid.unwrap_or(endpoint))))
}

/// R1, then [`finish_mandatory_default_endpoint`], for one complete Default
/// lineage.
fn finish_complete_default_lineage(
    source: CandidateInput<'_>,
    floor_options: &Options,
    progress: Progress,
    parse_cache: &mut TerminalParseCache,
    base: Candidate,
    header_fit: HeaderFit,
) -> Result<(Candidate, Option<Candidate>)> {
    let restored = improve_with_original_match_restoration(
        source,
        floor_options,
        DefaultFloorWork::Mandatory,
        progress,
        base,
    )?;
    finish_mandatory_default_endpoint(
        source,
        floor_options,
        progress,
        parse_cache,
        restored,
        header_fit,
    )
}

/// [`finish_complete_default_lineage`] for `base` and, when D4 applies and its
/// source topology is strictly smaller than `base`, for that topology too,
/// keeping the lineage whose exact endpoint is smaller. D4 is mandatory here,
/// as in Default, so Max never ends above Default.
fn finish_complete_default_lineages(
    source: CandidateInput<'_>,
    floor_options: &Options,
    progress: Progress,
    parse_cache: &mut TerminalParseCache,
    base: Candidate,
    header_fit: HeaderFit,
) -> Result<(Candidate, Option<Candidate>)> {
    let topology = if default_source_topology_eligible(
        source.blocks,
        source.compressed.len(),
        source.identity.decoded_size,
    ) {
        build_source_family_candidate(source, floor_options, &mut SearchStop::never())?
            .filter(|topology| topology.is_strictly_smaller_than(&base))
    } else {
        None
    };
    let (complete, slid) = finish_complete_default_lineage(
        source,
        floor_options,
        progress,
        parse_cache,
        base,
        header_fit,
    )?;
    let Some(mut topology) = topology else {
        return Ok((complete, slid));
    };
    topology.plans.clear();
    let (topology, topology_slid) = finish_complete_default_lineage(
        source,
        floor_options,
        progress,
        parse_cache,
        topology,
        header_fit,
    )?;
    let endpoint = slid.as_ref().unwrap_or(&complete);
    Ok(
        if topology_slid
            .as_ref()
            .unwrap_or(&topology)
            .is_strictly_smaller_than(endpoint)
        {
            (topology, topology_slid)
        } else {
            (complete, slid)
        },
    )
}

/// Default's R2–R5 terminal header searches, in their sweep order.
fn finish_default_header_searches(
    source: CandidateInput<'_>,
    floor_options: &Options,
    progress: Progress,
    parse_cache: &mut TerminalParseCache,
    mut candidate: Candidate,
) -> Result<Candidate> {
    for search in [
        TerminalHeaderSearch::PayloadTradeoff,
        TerminalHeaderSearch::LiteralSpan,
        TerminalHeaderSearch::JointTreeRle,
        TerminalHeaderSearch::SymbolSets,
    ] {
        candidate = improve_with_terminal_header_search(
            search,
            source,
            floor_options,
            DefaultFloorWork::Mandatory,
            progress,
            parse_cache,
            candidate,
        )?;
    }
    Ok(candidate)
}

pub(super) fn build_complete_default_floor_candidate(
    source: CandidateInput<'_>,
    options: &Options,
    progress: Progress,
    terminal_work: DefaultFloorWork<'_>,
) -> Result<CompleteDefaultFloor> {
    build_complete_default_floor(source, options, progress, terminal_work, false)
}

/// [`build_complete_default_floor_candidate`], also keeping a copy of
/// Default's exact endpoint when `retain_exact` is set.
pub(super) fn build_complete_default_floor(
    source: CandidateInput<'_>,
    options: &Options,
    progress: Progress,
    terminal_work: DefaultFloorWork<'_>,
    retain_exact: bool,
) -> Result<CompleteDefaultFloor> {
    let mut floor_options = options.clone();
    floor_options.exhaustive = false;
    let candidate = build_candidate(
        source,
        &floor_options,
        DEFAULT_RAW_REPLAY_LIMIT,
        &mut SearchStop::never(),
    )?;
    let max_seed = candidate.clone().named("Normal floor");
    let first_route_cut =
        finish_first_route_cut(source, &floor_options, progress, candidate.clone())?;
    let complete = improve_default_floor_with_feedback(
        source,
        &floor_options,
        DefaultFloorWork::Mandatory,
        progress,
        candidate,
    )?;
    let mut parse_cache = TerminalParseCache::default();
    let (mut complete, mut slid) = finish_complete_default_lineages(
        source,
        &floor_options,
        progress,
        &mut parse_cache,
        complete,
        HeaderFit::InSweep,
    )?;
    // Default selects the slid endpoint when it is smaller, as Max's final
    // comparison does.
    let exact = if retain_exact {
        let endpoint = slid.as_ref().unwrap_or(&complete);
        let mut data = Vec::new();
        data.try_reserve_exact(endpoint.data.len())
            .map_err(|_| Error::allocation("Default endpoint"))?;
        data.extend_from_slice(&endpoint.data);
        Some(DefaultEndpoint {
            data,
            deflate_bits: endpoint.bits,
        })
    } else {
        None
    };
    // Default reaches the cut endpoint too when its deadline passes during
    // its first route, so it is a final competitor beside the slid endpoint.
    if first_route_cut.is_strictly_smaller_than(slid.as_ref().unwrap_or(&complete)) {
        slid = Some(first_route_cut);
    }
    // R6–R9 run under the caller's Max allowance, not as mandatory work.
    for search in [
        TerminalHeaderSearch::AlphabetBoundaries,
        TerminalHeaderSearch::HeaderTree,
        TerminalHeaderSearch::CodeLengthRotations,
        TerminalHeaderSearch::CoupledLengthSwaps,
    ] {
        complete = improve_with_terminal_header_search(
            search,
            source,
            &floor_options,
            terminal_work,
            progress,
            &mut parse_cache,
            complete,
        )?;
    }

    Ok(CompleteDefaultFloor {
        max_seed,
        complete,
        slid,
        exact,
    })
}

/// Default's endpoint when its deadline passes during its first route (the
/// ordinary planner): no later route starts, so D1, D2, R1–R5, D4 and the
/// route families are skipped, and D3, the tree floors and the finishing steps
/// (R1c, R13–R18) run on that route's candidate. The finishing steps are not
/// monotone in their starting size, so this endpoint can be smaller than the
/// complete route's, and a container that reclaims a member's time slice
/// keeps whichever of the two is smaller.
fn finish_first_route_cut(
    source: CandidateInput<'_>,
    floor_options: &Options,
    progress: Progress,
    candidate: Candidate,
) -> Result<Candidate> {
    // A soft deadline that has passed, with no hard stop: no route starts,
    // and linear finalization always finishes.
    let deadline = Deadline::with_grace(Instant::now(), Duration::ZERO, Duration::MAX);
    let work = DefaultFloorWork::Timed(&deadline);
    let candidate =
        improve_default_floor_with_feedback(source, floor_options, work, progress, candidate)?;
    // Default's closing sweep and R18, as its finish runs them.
    let candidate = improve_with_terminal_searches(
        source,
        floor_options,
        work,
        work,
        progress,
        HeaderFit::InSweep,
        false,
        candidate,
    )?;
    improve_with_terminal_tree_floors(source, floor_options, work, progress, candidate)
}

/// Build APNG's complete Default endpoint, with D4 as `ApngDefault` runs it, and
/// its slid form, then run R6–R9 on the endpoint within this frame's Max
/// allowance.
pub(super) fn build_complete_apng_default_floor_candidate(
    source: CandidateInput<'_>,
    options: &Options,
    progress: Progress,
    terminal_work: DefaultFloorWork<'_>,
) -> Result<(Candidate, Option<Candidate>)> {
    let floor_options = Options {
        exhaustive: false,
        ..options.clone()
    };
    let initial = build_apng_default_candidate(source, &floor_options, &mut SearchStop::never())?;
    let mut parse_cache = TerminalParseCache::default();
    let (mut complete, slid) = finish_complete_default_lineages(
        source,
        &floor_options,
        progress,
        &mut parse_cache,
        initial,
        HeaderFit::AfterTreeFloors,
    )?;
    for search in [
        TerminalHeaderSearch::AlphabetBoundaries,
        TerminalHeaderSearch::HeaderTree,
        TerminalHeaderSearch::CodeLengthRotations,
        TerminalHeaderSearch::CoupledLengthSwaps,
    ] {
        complete = improve_with_terminal_header_search(
            search,
            source,
            &floor_options,
            terminal_work,
            progress,
            &mut parse_cache,
            complete,
        )?;
    }
    Ok((complete, slid))
}

/// Run the initial stream planner once, without replay or the endpoint-proven
/// lineage (APNG Default).
pub(super) fn build_apng_default_candidate(
    source: CandidateInput<'_>,
    options: &Options,
    expired: &mut SearchStop<'_>,
) -> Result<Candidate> {
    let Some(plans) = plan_stream(source.blocks, source.compressed.len(), 0, options, expired)
    else {
        return source_candidate(source, options);
    };
    build_candidate_from_plans(source, plans, options, 0, ReplayPlanner::Full, expired)
}

#[cfg(test)]
mod tests;
