// SPDX-License-Identifier: MIT
// SPDX-FileCopyrightText: 2026 Andrew C.E. Dent <https://github.com/ace-dent>

//! Max's bounded phase: the bounded floor and its descendants (M0, M7), the
//! independent routes run beside them under one deadline (M5, M6, M9, M10a),
//! and the refinement of the deft4j lineage that follows (M10b).

use std::thread;

use crate::progress::Progress;
use crate::{Error, Options, Result};

use super::compact_split::{
    build_prepared_compact_source_split_floor, compact_split_preserves_source_blocks,
    prepare_compact_source_split_seed,
};
use super::default_floor::build_complete_default_floor_candidate;
use super::gates::{
    compact_complementary_source_max_is_cheap, compact_dependent_deft4j_work_class,
    compact_single_source_route_work_class, has_multiple_nonempty_blocks,
    repartition_graph_covers_source_blocks,
};
use super::proven_feedback::build_compact_proven_feedback_candidate;
use super::source_routes::{
    build_deft4j_seed_candidate, build_deft4j_source_candidate, build_narrow_source_candidate,
    build_source_max_candidate, changed_parent_no_split_should_continue,
    refine_with_default_planner, refine_with_default_planner_and_change, refine_with_max_planner,
    refine_with_no_split_route, refine_with_terminal_merge,
};
use super::{
    build_candidate, build_candidate_from_established_floor, build_candidate_from_plans,
    join_route, parse_validated_rewrite, replace_optional_if_smaller, rewritten_input,
    run_route_with_cancellation, spawn_route, Candidate, CandidateInput, DefaultFloorWork,
    ReplayPlanner, StreamIdentity, DEFAULT_RAW_REPLAY_LIMIT, MAX_RAW_REPLAY_LIMIT,
    NARROW_SOURCE_MAX_COMPRESSED,
};
use crate::deflate::block::reusable_original_bits;
use crate::deflate::search::same_distance_opportunities;
use crate::deflate::stop::{Deadline, RouteWindow, SearchStop};
use crate::deflate::stream::plan_columbo_floor_seeded_bounded_grouping;

const DEFLATE_MAX_STORED_BLOCK_PLAIN: u64 = 65_535;

#[derive(Default)]
pub(super) struct BoundedFollowUpCandidates {
    pub(super) source_max: Option<Candidate>,
    pub(super) attempted_compact_split: bool,
    pub(super) compact_split: Option<Candidate>,
    pub(super) narrow: Option<Candidate>,
}

/// Refine the bounded phase's best deft4j lineage in place (M10b).
///
/// A concurrent original-source Max shares only the deadline. Set
/// `stop_at_soft_deadline` when compact split siblings already cover this
/// lineage's parents, so it does not use the grace.
#[allow(clippy::too_many_arguments)]
pub(super) fn refine_bounded_deft4j_lineage(
    source: CandidateInput<'_>,
    options: &Options,
    decoded_limit: u64,
    identity: StreamIdentity,
    deadline: &Deadline,
    bounded_floor: Option<&Candidate>,
    narrow_candidate: Option<&Candidate>,
    seed_weak_deft4j: bool,
    stop_at_soft_deadline: bool,
    deft4j_candidate: &mut Option<Candidate>,
) -> Result<()> {
    if seed_weak_deft4j {
        if let Some(floor) =
            bounded_floor.filter(|floor| floor.is_strictly_smaller_than_source(source))
        {
            if let Some(mut seeded) = build_deft4j_seed_candidate(
                floor,
                options,
                decoded_limit,
                identity,
                &mut deadline.bounded_stop(stop_at_soft_deadline),
            )? {
                let mut continue_no_split = false;
                if deadline.can_start_route() {
                    let (refined, exposes_new_parent) = refine_with_default_planner_and_change(
                        &seeded,
                        options,
                        decoded_limit,
                        identity,
                        &mut deadline.bounded_stop(stop_at_soft_deadline),
                    )?;
                    continue_no_split = changed_parent_no_split_should_continue(
                        exposes_new_parent,
                        &refined,
                        &seeded,
                        narrow_candidate,
                    );
                    seeded.replace_keeping_parent(refined, options.exhaustive);
                }
                if continue_no_split && deadline.can_start_route() {
                    let mut route_stop = deadline.hard_stop();
                    let mut refinement_stop = deadline.hard_stop();
                    if let Some(continued) = refine_with_no_split_route(
                        &seeded,
                        options,
                        decoded_limit,
                        identity,
                        &mut route_stop,
                        &mut refinement_stop,
                    )? {
                        seeded.replace_keeping_parent(continued, options.exhaustive);
                    }
                }
                let narrow_already_wins =
                    narrow_candidate.is_some_and(|narrow| narrow.is_strictly_smaller_than(&seeded));
                if !narrow_already_wins
                    && deadline.can_start_route()
                    && seeded.data.len() <= NARROW_SOURCE_MAX_COMPRESSED
                {
                    if let Some(terminal) = refine_with_terminal_merge(
                        &seeded,
                        options,
                        decoded_limit,
                        identity,
                        &mut deadline.bounded_stop(stop_at_soft_deadline),
                    )? {
                        seeded.replace_keeping_parent(terminal, options.exhaustive);
                    }
                }
                match deft4j_candidate {
                    Some(deft4j) => {
                        deft4j.replace_keeping_parent(seeded, options.exhaustive);
                    }
                    None => *deft4j_candidate = Some(seeded),
                }
            }
        }
    } else if let Some(deft4j) = deft4j_candidate.as_ref() {
        let (mut refined, exposes_new_parent) = refine_with_default_planner_and_change(
            deft4j,
            options,
            decoded_limit,
            identity,
            &mut deadline.bounded_stop(stop_at_soft_deadline),
        )?;
        // M2c: a smaller new topology from Default may give the no-split route
        // another fixed point.
        let continue_no_split = changed_parent_no_split_should_continue(
            exposes_new_parent,
            &refined,
            deft4j,
            narrow_candidate,
        ) && deadline.can_start_route();
        if continue_no_split {
            // No compact split sibling covers this changed parent, so it may
            // use the grace even when the rest of this lineage may not.
            let mut route_stop = deadline.hard_stop();
            let mut refinement_stop = deadline.hard_stop();
            if let Some(continued) = refine_with_no_split_route(
                &refined,
                options,
                decoded_limit,
                identity,
                &mut route_stop,
                &mut refinement_stop,
            )? {
                refined.replace_keeping_parent(continued, options.exhaustive);
            }
        }
        // Neither source max nor Max on the unrefined parent covers a new
        // topology from Default; header-only rewrites stop at Default.
        if exposes_new_parent
            && refined.is_strictly_smaller_than(deft4j)
            && deadline.can_start_route()
        {
            let continued = refine_with_max_planner(
                &refined,
                options,
                decoded_limit,
                identity,
                &mut deadline.bounded_stop(stop_at_soft_deadline),
            )?;
            refined.replace_keeping_parent(continued, options.exhaustive);
        }
        if let Some(deft4j) = deft4j_candidate.as_mut() {
            deft4j.replace_keeping_parent(refined, options.exhaustive);
        }
    }
    Ok(())
}

/// Completed candidates produced by the bounded first phase.
///
/// `suppress_later_source_max` marks source max as already run or omitted by
/// policy.
#[derive(Default)]
pub(super) struct BoundedPhaseCandidates {
    pub(super) floor: Option<Candidate>,
    /// Compared only after the terminal searches.
    pub(super) slid_default: Option<Candidate>,
    pub(super) floor_seeded: Option<Candidate>,
    pub(super) deft4j: Option<Candidate>,
    pub(super) narrow: Option<Candidate>,
    pub(super) source_max: Option<Candidate>,
    pub(super) proven_feedback: Option<Candidate>,
    pub(super) suppress_later_source_max: bool,
    pub(super) completed_compact_split_parent: Option<Vec<u8>>,
}

/// Route families requested for the bounded comparison phase.
#[derive(Clone, Copy, Default)]
pub(super) struct BoundedRoutes {
    /// Finish the exact Default endpoint before optional routes can use time.
    pub(super) preserve_complete_default: bool,
    /// Continue the ordinary floor with the Max planner.
    pub(super) seeded_max: bool,
    pub(super) deft4j: bool,
    pub(super) narrow: bool,
    pub(super) source_max: bool,
    pub(super) proven_feedback: bool,
    pub(super) parallel: bool,
}

/// Run the independent bounded comparison routes under one deadline, on
/// scoped workers within the parallel limits (M5) and serially otherwise.
pub(super) fn build_bounded_phase_candidates(
    source: CandidateInput<'_>,
    options: &Options,
    routes: BoundedRoutes,
    deadline: &Deadline,
    progress: Progress,
    completed_floor: Option<Candidate>,
) -> Result<BoundedPhaseCandidates> {
    let BoundedRoutes {
        preserve_complete_default,
        seeded_max: run_seeded_max,
        deft4j: run_deft4j,
        narrow: run_narrow,
        source_max: run_source_max,
        proven_feedback: run_proven_feedback,
        parallel: parallel_routes,
    } = routes;
    let run_source_max = run_source_max && parallel_routes;
    let run_proven_feedback = run_proven_feedback && parallel_routes;
    // A one-block floor descendant gets the full window; multi-block work
    // keeps the follow-up reservation for its compact routes.
    let unique_one_block_floor_descendant = run_seeded_max
        && source
            .blocks
            .iter()
            .filter(|block| !block.plain.is_empty())
            .count()
            == 1;
    // M10a: finish the cheap direct deft4j candidate, its Default replay and
    // (for a compact multi-block lineage) its dependent split before the long
    // beams start.
    let repartition_runs = same_distance_opportunities(source.blocks).repartition_runs;
    // When repartition runs cover every nonempty block, only source max can
    // combine them, so start it at once and defer the split.
    let source_repartition_graph_is_dense =
        repartition_graph_covers_source_blocks(source.blocks, repartition_runs);
    let prebuild_compact_split = run_seeded_max
        && compact_dependent_deft4j_work_class(source)
        && !source_repartition_graph_is_dense;
    let prebuild_deft4j = (unique_one_block_floor_descendant || prebuild_compact_split)
        && run_deft4j
        && deadline.can_start_route();
    let mut prebuilt_deft4j = if prebuild_deft4j {
        build_deft4j_source_candidate(source, options, &mut deadline.hard_stop())?
    } else {
        None
    };
    let mut completed_compact_split_parent = None;
    if let Some(deft4j) = &mut prebuilt_deft4j {
        if deadline.can_start_route() {
            let refined = refine_with_default_planner(
                deft4j,
                options,
                source.decoded_limit,
                source.identity,
                &mut deadline.hard_stop(),
            )?;
            // The replay waits on G0, so a deadline could have handed over the
            // direct candidate it replaces, which Max keeps.
            deft4j.replace_keeping_parent(refined, options.exhaustive);
        }
        if prebuild_compact_split {
            if let Some(seed) =
                prepare_compact_source_split_seed(deft4j, source.decoded_limit, source.identity)?
            {
                let split = build_prepared_compact_source_split_floor(
                    &seed,
                    options,
                    source.decoded_limit,
                    source.identity,
                )?;
                let preserves_source_blocks = split.as_ref().is_some_and(|candidate| {
                    compact_split_preserves_source_blocks(&seed.stream.blocks, &candidate.plans)
                });
                if let Some(split) = split {
                    deft4j.replace_if_smaller(split);
                }
                // Record the completed parent so later phases skip it. If a
                // cut won, only the input parent is complete and its split
                // descendant stays eligible.
                completed_compact_split_parent = if preserves_source_blocks {
                    Some(deft4j.data.clone())
                } else {
                    Some(seed.data)
                };
            }
        }
    }
    // Open the phase window after the prerequisite, so its reserved fifth is
    // measured from the time that remains.
    let route_window = if (run_deft4j || run_narrow || run_source_max || run_proven_feedback)
        && !unique_one_block_floor_descendant
    {
        RouteWindow::reserving_follow_up(deadline)
    } else {
        RouteWindow::full(deadline)
    };
    let run_deft4j = run_deft4j && !prebuild_deft4j && route_window.can_start_route();
    let run_narrow = run_narrow && route_window.can_start_route();
    // M9: choose source max, proven feedback or both by token count.
    let run_complementary_source_max = compact_complementary_source_max_is_cheap(source);
    let source_restart_has_distinct_topology =
        repartition_runs != 0 || source.identity.decoded_size > DEFLATE_MAX_STORED_BLOCK_PLAIN;
    let run_single_source_max = compact_single_source_route_work_class(source)
        && !run_complementary_source_max
        && source_restart_has_distinct_topology
        && run_proven_feedback;
    let run_source_max = run_source_max
        && (run_complementary_source_max
            || run_single_source_max
            || repartition_runs >= 2
            || !run_proven_feedback)
        && route_window.can_start_route();
    let run_proven_feedback = run_proven_feedback
        && (!run_single_source_max || !run_source_max)
        && route_window.can_start_route();
    if !run_deft4j && !run_narrow && !run_source_max && !run_proven_feedback {
        let (floor, floor_seeded, slid_default) =
            build_bounded_floor_descendants_preserving_default(
                source,
                options,
                preserve_complete_default,
                run_seeded_max,
                &route_window,
                completed_floor,
                progress,
            )?;
        return Ok(BoundedPhaseCandidates {
            floor: Some(floor),
            slid_default,
            floor_seeded,
            deft4j: prebuilt_deft4j,
            completed_compact_split_parent,
            ..BoundedPhaseCandidates::default()
        });
    }

    if !parallel_routes {
        let mut candidates = build_bounded_phase_candidates_sequential(
            source,
            options,
            preserve_complete_default,
            run_seeded_max,
            run_deft4j,
            run_narrow,
            &route_window,
            completed_floor,
            progress,
        )?;
        if let Some(prebuilt) = prebuilt_deft4j {
            replace_optional_if_smaller(&mut candidates.deft4j, prebuilt);
        }
        candidates.completed_compact_split_parent = completed_compact_split_parent;
        return Ok(candidates);
    }

    thread::scope(|scope| {
        let deft4j_worker = run_deft4j
            .then(|| {
                spawn_route(scope, "columbo-deft4j-derived", deadline, || {
                    build_deft4j_source_candidate(source, options, &mut route_window.stop())
                })
            })
            .flatten();
        let narrow_worker = run_narrow
            .then(|| {
                spawn_route(scope, "columbo-no-split", deadline, || {
                    build_narrow_source_candidate(
                        source,
                        options,
                        &mut route_window.stop(),
                        &mut route_window.stop(),
                    )
                })
            })
            .flatten();
        let source_max_worker = run_source_max
            .then(|| {
                spawn_route(scope, "columbo-source-max-initial", deadline, || {
                    build_source_max_candidate(
                        source,
                        options,
                        progress,
                        deadline,
                        false,
                        &mut route_window.stop(),
                    )
                })
            })
            .flatten();
        let proven_feedback_worker = run_proven_feedback
            .then(|| {
                spawn_route(scope, "columbo-proven-feedback-initial", deadline, || {
                    build_compact_proven_feedback_candidate(
                        source,
                        options,
                        &mut route_window.stop(),
                    )
                })
            })
            .flatten();

        // Join every spawned worker before applying the fixed error order
        // below.
        let floor = run_route_with_cancellation(deadline, || {
            build_bounded_floor_descendants_preserving_default(
                source,
                options,
                preserve_complete_default,
                run_seeded_max,
                &route_window,
                completed_floor,
                progress,
            )
        });
        let deft4j = match deft4j_worker {
            Some(worker) => worker.join(),
            None if run_deft4j => Ok(run_route_with_cancellation(deadline, || {
                build_deft4j_source_candidate(source, options, &mut route_window.stop())
            })),
            None => Ok(Ok(None)),
        };
        let narrow = match narrow_worker {
            Some(worker) => worker.join(),
            None if run_narrow => Ok(run_route_with_cancellation(deadline, || {
                build_narrow_source_candidate(
                    source,
                    options,
                    &mut route_window.stop(),
                    &mut route_window.stop(),
                )
            })),
            None => Ok(Ok(None)),
        };
        let source_max = match source_max_worker {
            Some(worker) => join_route(worker).map(Some),
            None if run_source_max => run_route_with_cancellation(deadline, || {
                build_source_max_candidate(
                    source,
                    options,
                    progress,
                    deadline,
                    false,
                    &mut route_window.stop(),
                )
            })
            .map(Some),
            None => Ok(None),
        };
        let proven_feedback = match proven_feedback_worker {
            Some(worker) => join_route(worker),
            None if run_proven_feedback => run_route_with_cancellation(deadline, || {
                build_compact_proven_feedback_candidate(source, options, &mut route_window.stop())
            }),
            None => Ok(None),
        };

        // All workers have joined, so resuming a panic cannot strand a sibling.
        let worker_deft4j = match deft4j {
            Ok(result) => result,
            Err(payload) => std::panic::resume_unwind(payload),
        }?;
        let narrow = match narrow {
            Ok(result) => result,
            Err(payload) => std::panic::resume_unwind(payload),
        }?;
        let source_max = source_max?;
        let proven_feedback = proven_feedback?;
        let (floor, floor_seeded, slid_default) = floor?;
        if let Some(worker_deft4j) = worker_deft4j {
            replace_optional_if_smaller(&mut prebuilt_deft4j, worker_deft4j);
        }
        Ok(BoundedPhaseCandidates {
            floor: Some(floor),
            slid_default,
            floor_seeded,
            deft4j: prebuilt_deft4j,
            narrow,
            source_max,
            proven_feedback,
            suppress_later_source_max: run_source_max,
            completed_compact_split_parent,
        })
    })
}

/// Run the bounded phase serially, so route arenas never overlap.
#[allow(clippy::too_many_arguments)]
fn build_bounded_phase_candidates_sequential(
    source: CandidateInput<'_>,
    options: &Options,
    preserve_complete_default: bool,
    run_seeded_max: bool,
    run_deft4j: bool,
    run_narrow: bool,
    route_window: &RouteWindow<'_>,
    completed_floor: Option<Candidate>,
    progress: Progress,
) -> Result<BoundedPhaseCandidates> {
    // Build the mandatory Default floor first so optional routes cannot use up
    // its time.
    let mut completed_floor = completed_floor;
    let preserved_floor = preserve_complete_default
        .then(|| {
            build_bounded_floor_descendants_preserving_default(
                source,
                options,
                true,
                run_seeded_max,
                route_window,
                completed_floor.take(),
                progress,
            )
        })
        .transpose()?;
    let deft4j = if run_deft4j && route_window.can_start_route() {
        build_deft4j_source_candidate(source, options, &mut route_window.stop())?
    } else {
        None
    };
    let narrow = if run_narrow && route_window.can_start_route() {
        build_narrow_source_candidate(
            source,
            options,
            &mut route_window.stop(),
            &mut route_window.hard_stop(),
        )?
    } else {
        None
    };
    let (floor, floor_seeded, slid_default) = match preserved_floor {
        Some(floor) => floor,
        None => {
            let (floor, floor_seeded) = build_bounded_floor_descendants(
                source,
                options,
                run_seeded_max,
                route_window,
                completed_floor,
            )?;
            (floor, floor_seeded, None)
        }
    };
    Ok(BoundedPhaseCandidates {
        floor: Some(floor),
        slid_default,
        floor_seeded,
        deft4j,
        narrow,
        ..BoundedPhaseCandidates::default()
    })
}

pub(super) fn build_bounded_floor_candidate(
    source: CandidateInput<'_>,
    options: &Options,
    expired: &mut SearchStop<'_>,
) -> Result<Candidate> {
    let mut floor_options = options.clone();
    floor_options.exhaustive = false;
    build_candidate(source, &floor_options, DEFAULT_RAW_REPLAY_LIMIT, expired)
        .map(|candidate| candidate.named("Normal floor"))
}

/// Copy a caller-retained `Established` stream into the local candidate set.
/// Descendants reparse it, so the normal identity checks still apply. Strict
/// mode returns `None` unless the copy is strictly compatible, so the routes
/// build a bounded floor instead.
pub(super) fn established_floor_candidate(
    source: CandidateInput<'_>,
    strict: bool,
) -> Result<Option<Candidate>> {
    if strict && !source_is_strictly_reusable(source) {
        return Ok(None);
    }
    let mut data = Vec::new();
    data.try_reserve_exact(source.compressed.len())
        .map_err(|_| Error::allocation("Deflate output"))?;
    data.extend_from_slice(source.compressed);
    Ok(Some(Candidate {
        data,
        bits: source.meaningful_bits,
        output_max_distance: None,
        plans: Vec::new(),
        block_report: None,
        route: "Normal floor",
        max_planner_is_stable: false,
        replaced_parents: Vec::new(),
    }))
}

/// Whether every source block may be copied verbatim in strict mode, with no
/// gap where the parser dropped an empty block. Strict normalization clears
/// `original` from blocks that used the 258 alias.
fn source_is_strictly_reusable(source: CandidateInput<'_>) -> bool {
    let mut position = 0;
    source.blocks.iter().all(|block| {
        block.original.is_some_and(|original| {
            let contiguous = original.start == position;
            position = original.start + original.len;
            contiguous && reusable_original_bits(block, original.alignment, true).is_some()
        })
    }) && position == source.meaningful_bits
}

fn completed_or_bounded_floor(
    source: CandidateInput<'_>,
    options: &Options,
    completed_floor: Option<Candidate>,
    expired: &mut SearchStop<'_>,
) -> Result<Candidate> {
    match completed_floor {
        Some(floor) => Ok(floor.named("Normal floor")),
        None => build_bounded_floor_candidate(source, options, expired),
    }
}

/// Build the bounded Default floor, then run the Max planner seeded from it.
/// Both candidates are kept, so a timeout or a non-improving Max run cannot
/// discard the floor.
fn build_bounded_floor_lineage(
    source: CandidateInput<'_>,
    options: &Options,
    route_window: &RouteWindow<'_>,
    completed_floor: Option<Candidate>,
) -> Result<(Candidate, Option<Candidate>)> {
    let floor =
        completed_or_bounded_floor(source, options, completed_floor, &mut route_window.stop())?;
    continue_bounded_floor_lineage(source, floor, options, route_window, true)
}

/// Continue the selected floor into Max search when requested, or else, for a
/// multi-block floor, into standalone bounded grouping.
fn build_bounded_floor_descendants(
    source: CandidateInput<'_>,
    options: &Options,
    run_seeded_max: bool,
    route_window: &RouteWindow<'_>,
    completed_floor: Option<Candidate>,
) -> Result<(Candidate, Option<Candidate>)> {
    let floor =
        completed_or_bounded_floor(source, options, completed_floor, &mut route_window.stop())?;
    if !run_seeded_max {
        return continue_bounded_floor_lineage(source, floor, options, route_window, false);
    }

    continue_bounded_floor_lineage(source, floor, options, route_window, true)
}

/// Build the floor lineage, keeping the exact Default endpoint separate from
/// the Max seed. Returns the floor, its descendant, and the slid Default
/// endpoint.
#[allow(clippy::too_many_arguments)]
fn build_bounded_floor_descendants_preserving_default(
    source: CandidateInput<'_>,
    options: &Options,
    preserve_complete_default: bool,
    run_seeded_max: bool,
    route_window: &RouteWindow<'_>,
    completed_floor: Option<Candidate>,
    progress: Progress,
) -> Result<(Candidate, Option<Candidate>, Option<Candidate>)> {
    if !preserve_complete_default || completed_floor.is_some() {
        let (floor, descendant) = build_bounded_floor_descendants(
            source,
            options,
            run_seeded_max,
            route_window,
            completed_floor,
        )?;
        return Ok((floor, descendant, None));
    }

    let floors = build_complete_default_floor_candidate(
        source,
        options,
        progress,
        DefaultFloorWork::Window(route_window),
    )?;
    let (_, descendant) = build_bounded_floor_descendants(
        source,
        options,
        run_seeded_max,
        route_window,
        Some(floors.max_seed),
    )?;
    Ok((floors.complete, descendant, floors.slid))
}

fn continue_bounded_floor_lineage(
    source: CandidateInput<'_>,
    mut floor: Candidate,
    options: &Options,
    route_window: &RouteWindow<'_>,
    run_seeded_max: bool,
) -> Result<(Candidate, Option<Candidate>)> {
    let floor_selected = options.strict || floor.is_strictly_smaller_than_source(source);
    if !floor_selected || !route_window.can_start_route() {
        return Ok((floor, None));
    }

    // Release the floor's plans before parsing it.
    floor.plans.clear();
    let stream = parse_validated_rewrite(&floor.data, source.decoded_limit, source.identity)?;
    let rewritten = rewritten_input(&floor, &stream, source.decoded_limit, source.identity);
    let mut descendant = None;

    if run_seeded_max && route_window.can_start_route() {
        let seeded = build_candidate_from_established_floor(
            rewritten,
            options,
            MAX_RAW_REPLAY_LIMIT,
            false,
            false,
            &mut route_window.stop(),
        )?
        .named("Columbo max refinement");
        descendant = Some(seeded);
    } else if has_multiple_nonempty_blocks(&stream.blocks) && route_window.can_start_route() {
        // The Max planner starts from the same grouping; running both repeats
        // range pricing and can starve the Max route.
        if let Some(plans) = plan_columbo_floor_seeded_bounded_grouping(&stream.blocks, 0, options)
        {
            let grouped = build_candidate_from_plans(
                rewritten,
                plans,
                options,
                0,
                ReplayPlanner::Full,
                &mut route_window.stop(),
            )?
            .named("Columbo floor-seeded grouping");
            descendant = Some(grouped);
        }
    }
    Ok((floor, descendant))
}

/// `GenericParallel` (M6): run source max beside the floor lineage.
pub(super) fn build_bounded_generic_max_candidates(
    source: CandidateInput<'_>,
    options: &Options,
    deadline: &Deadline,
    progress: Progress,
    completed_floor: Option<Candidate>,
) -> Result<BoundedPhaseCandidates> {
    let route_window = RouteWindow::full(deadline);
    thread::scope(|scope| {
        let run_source_max = deadline.can_start_route();
        let source_worker = run_source_max
            .then(|| {
                spawn_route(scope, "columbo-source-max", deadline, || {
                    build_source_max_candidate(
                        source,
                        options,
                        progress,
                        deadline,
                        false,
                        &mut deadline.hard_stop(),
                    )
                })
            })
            .flatten();

        let floor = run_route_with_cancellation(deadline, || {
            build_bounded_floor_lineage(source, options, &route_window, completed_floor)
        });
        let source_max = match source_worker {
            Some(worker) => match worker.join() {
                Ok(result) => result.map(Some),
                Err(payload) => std::panic::resume_unwind(payload),
            },
            None if run_source_max && deadline.can_start_route() => {
                run_route_with_cancellation(deadline, || {
                    build_source_max_candidate(
                        source,
                        options,
                        progress,
                        deadline,
                        false,
                        &mut deadline.hard_stop(),
                    )
                })
                .map(Some)
            }
            None => Ok(None),
        };

        // A floor error takes precedence over a source-max error.
        let (floor, floor_seeded) = floor?;
        let source_max = source_max?;
        Ok(BoundedPhaseCandidates {
            floor: Some(floor),
            floor_seeded,
            source_max,
            suppress_later_source_max: true,
            ..BoundedPhaseCandidates::default()
        })
    })
}

#[cfg(test)]
mod tests;
