// SPDX-License-Identifier: MIT
// SPDX-FileCopyrightText: 2026 Andrew C.E. Dent <https://github.com/ace-dent>

//! Route scheduling for one raw Deflate stream.
//!
//! [`optimize_raw_prefix_with_floor_and_grace`] validates the source, then
//! [`select_candidate`] runs the route phases in a fixed order. Every phase
//! reads the immutable [`RouteContext`] and updates the complete lineages in
//! [`RouteState`]. The order is part of the output contract: equal candidates
//! keep the earlier incumbent, and each optional route checks its deadline
//! where it would start.
//!
//! Route algorithms, eligibility rules and candidate builders live in the
//! parent module; this module decides which run, when, on which worker, and
//! what each lineage retains.

use std::thread;
use std::time::{Duration, Instant};

use crate::progress::{
    reports_enabled, BalancedTreeProgress, CandidateProgress, Progress, SameDistanceProgress,
    StreamProgress,
};
use crate::{Error, Options, Result};

use super::{
    bounded_parallel_source_max_work_class, bounded_png_max_policy, build_apng_default_candidate,
    build_bounded_floor_candidate, build_bounded_generic_max_candidates,
    build_bounded_phase_candidates, build_candidate, build_candidate_from_plans,
    build_compact_proven_feedback_candidate, build_complementary_narrow_source_candidate,
    build_complete_apng_default_floor_candidate, build_complete_default_floor_candidate,
    build_deft4j_source_candidate, build_prepared_compact_source_split_floors,
    build_source_family_candidate, build_source_max_candidate, candidate_exposes_new_parent,
    candidate_progress, capture_source_block_report, changed_narrow_parent_should_continue,
    compact_balanced_tree_source_eligible, compact_dependent_deft4j_work_class,
    compact_split_parent_is_completed, complete_png_parallel_source_max_work_class,
    default_source_topology_eligible, deft4j_source_route_eligible, established_floor_candidate,
    floor_exposes_new_search_states, floor_seeded_priority_with_structural_sibling, gain_is_below,
    has_multiple_nonempty_blocks, improve_default_floor_with_feedback,
    improve_with_final_header_fit, improve_with_terminal_searches,
    improve_with_terminal_tree_floors, independent_deft4j_refinement_can_overlap,
    is_strictly_better, join_route, narrow_source_route_eligible, normalize_258_aliases,
    parallel_route_is_bounded, parse_validated_rewrite, prebuild_bounded_floor,
    prepare_compact_source_split_seed, refine_bounded_deft4j_lineage,
    refine_with_compact_balanced_tree_floor, refine_with_compact_proven_feedback,
    refine_with_compact_source_split_floor, refine_with_compact_source_split_floor_until,
    refine_with_max_planner, refine_with_no_split_route,
    refine_with_terminal_source_split_floor_until, replace_optional_if_smaller,
    source_run_match_count_exceeds, spawn_route, BoundedFollowUpCandidates, BoundedPhaseCandidates,
    BoundedPngMaxPolicy, BoundedRoutes, Candidate, CandidateInput, CompactSplitSeed, DefaultFloor,
    DefaultFloorWork, HeaderFit, RawInfo, RawOptimization, ReplayPlanner, StreamIdentity,
    TerminalParseCache, TerminalShare, COMPACT_SPLIT_FLOOR_MAX_COMPRESSED,
    DEFAULT_RAW_REPLAY_LIMIT, MAX_RAW_REPLAY_LIMIT, WEAK_DEFT4J_GAIN_BASIS_POINTS,
};
use crate::deflate::block::release_thread_block_plans;
use crate::deflate::header::{balanced_tree_opportunities, BalancedTreeOpportunities};
use crate::deflate::model::{ParsedBlock, ParsedStream};
use crate::deflate::parse::parse_stream;
use crate::deflate::search::{
    compact_proven_submatch_route_eligible, same_distance_opportunities,
    PROVEN_SUBMATCH_FULL_MATCH_LIMIT,
};
use crate::deflate::stop::{initial_bounded_phase_share, Deadline, SearchStop};
use crate::deflate::stream::fragmented_collect_seed;

pub(crate) fn optimize_raw_prefix_with_floor_and_grace(
    input: &[u8],
    options: &Options,
    decoded_limit: u64,
    default_floor: DefaultFloor,
    grace: Duration,
) -> Result<RawOptimization> {
    let result = optimize_stream(input, options, decoded_limit, default_floor, grace);
    // Block plans repeat within a stream, rarely across streams, and APNG
    // frames and ZIP members reuse worker threads: release this thread's.
    release_thread_block_plans();
    result
}

fn optimize_stream(
    input: &[u8],
    options: &Options,
    decoded_limit: u64,
    default_floor: DefaultFloor,
    grace: Duration,
) -> Result<RawOptimization> {
    if input.len() as u64 > options.max_input_bytes {
        return Err(Error::resource_limit(
            "input exceeds configured safety limit",
        ));
    }

    let started = Instant::now();
    let mut parsed = parse_stream(input, decoded_limit.min(options.max_decoded_bytes))?;
    let reporting = reports_enabled(options);
    let parse_elapsed = if reporting {
        started.elapsed()
    } else {
        std::time::Duration::ZERO
    };
    let mut blocks = std::mem::take(&mut parsed.blocks);
    let progress = Progress::begin(
        options,
        started,
        StreamProgress {
            blocks: parsed.source_block_count,
            compressed_bytes: parsed.consumed,
            decoded_bytes: parsed.decoded_size,
            empty_blocks: parsed.source_empty_block_count,
            meaningful_bits: parsed.meaningful_bits,
            parse_elapsed,
        },
    );
    prepare_source_blocks(options, progress, &mut blocks)?;
    progress.routes();
    let deadlines = StreamDeadlines::new(started, grace, options, default_floor, &parsed);

    // Unused high bits in the final byte belong to the source bytes and are
    // retained when the source wins.
    let original = &input[..parsed.consumed];
    let decoded_limit = decoded_limit.min(options.max_decoded_bytes);
    let identity = StreamIdentity {
        decoded_size: parsed.decoded_size,
        crc32: parsed.crc32,
        adler32: parsed.adler32,
    };
    let source = CandidateInput {
        compressed: original,
        blocks: &blocks,
        meaningful_bits: parsed.meaningful_bits,
        decoded_limit,
        identity,
    };
    let context = RouteContext {
        source,
        options,
        default_floor,
        deadline: &deadlines.primary,
        progress,
    };
    let mut candidate = select_candidate(&context, &deadlines)?;

    let keep_original = !options.strict && !candidate.is_strictly_smaller_than_source(source);
    let deflate_bits = if keep_original {
        parsed.meaningful_bits
    } else {
        candidate.bits
    };
    let final_report = if keep_original {
        capture_source_block_report(&blocks, parsed.source_block_count, reporting)
    } else {
        candidate.block_report.take()
    };
    let selected_route = if keep_original {
        "Original source"
    } else {
        candidate.route
    };
    let output_bytes = if keep_original {
        original.len()
    } else {
        candidate.data.len()
    };
    let output_max_distance = if keep_original {
        parsed.max_distance
    } else if let Some(max_distance) = candidate.output_max_distance {
        max_distance
    } else {
        // A known-losing child skipped its validation parse, but an outer
        // route selected its bytes, so validate them now.
        parse_validated_rewrite(&candidate.data, decoded_limit, identity)?.max_distance
    };
    debug_assert!(output_max_distance <= parsed.max_distance);
    let timed_out = default_floor != DefaultFloor::MandatoryComplete
        && deadlines.terminal_work().was_triggered();
    progress.blocks(final_report);
    progress.finish(
        selected_route,
        output_bytes,
        deflate_bits,
        parsed.meaningful_bits,
        timed_out,
    );

    // Drop the model and reuse the output allocation, so keeping the source
    // never holds two source-sized outputs plus the parsed model at once.
    drop(blocks);
    drop(std::mem::take(&mut candidate.plans));
    if keep_original {
        candidate.data.clear();
        candidate
            .data
            .try_reserve_exact(original.len())
            .map_err(|_| Error::allocation("Deflate output"))?;
        candidate.data.extend_from_slice(original);
    }
    let mut data = candidate.data;
    if options.strip_metadata {
        crate::deflate::parse::normalize_padding(&mut data, decoded_limit)?;
    }

    Ok(RawOptimization {
        data,
        consumed: parsed.consumed,
        info: RawInfo {
            crc32: parsed.crc32,
            adler32: parsed.adler32,
            size: parsed.decoded_size,
            max_distance: parsed.max_distance,
            source_deflate_bits: parsed.meaningful_bits,
            deflate_bits,
            source_block_count: parsed.source_block_count,
            source_empty_block_count: parsed.source_empty_block_count,
        },
        output_max_distance,
        timed_out,
    })
}

#[derive(Clone, Copy)]
struct RouteContext<'a> {
    source: CandidateInput<'a>,
    options: &'a Options,
    default_floor: DefaultFloor,
    /// Primary optional-route deadline; see [`StreamDeadlines`].
    deadline: &'a Deadline,
    progress: Progress,
}

// The report helpers run once per candidate from dozens of call sites, so
// they stay out of line with the report clones they make.
impl<'a> RouteContext<'a> {
    #[inline(never)]
    fn report(&self, candidate: &Candidate) -> CandidateProgress {
        candidate_progress(
            candidate,
            self.source.meaningful_bits,
            candidate.is_strictly_smaller_than_source(self.source),
        )
    }

    #[inline(never)]
    fn report_optional(&self, candidate: Option<&Candidate>) -> Option<CandidateProgress> {
        candidate.map(|candidate| self.report(candidate))
    }

    fn timed(&self) -> DefaultFloorWork<'a> {
        DefaultFloorWork::Timed(self.deadline)
    }
}

/// The primary route deadline and the complete file allowance. When a stream
/// reserves terminal time (G0t), optional routes use a shorter primary window
/// and the full allowance governs only terminal work and timeout reporting.
struct StreamDeadlines {
    primary: Deadline,
    terminal: Deadline,
    reserve_terminal: bool,
}

impl StreamDeadlines {
    fn new(
        started: Instant,
        grace: Duration,
        options: &Options,
        default_floor: DefaultFloor,
        parsed: &ParsedStream,
    ) -> Self {
        let terminal = Deadline::with_grace(started, options.timeout, grace);
        let share = default_floor.terminal_share(
            options,
            parsed.consumed,
            parsed.decoded_size,
            parsed.source_block_count,
        );
        let reserve_terminal = share != TerminalShare::None;
        // The search share gives primary work no grace, so it cannot eat into
        // the terminal share. The finalization share keeps it, since primary
        // work still ends one twentieth before the full hard stop.
        let primary = match share {
            TerminalShare::None => Deadline::with_grace(started, options.timeout, grace),
            TerminalShare::Finalization => {
                Deadline::with_grace(started, options.timeout.saturating_mul(19) / 20, grace)
            }
            TerminalShare::Search => {
                let primary_share = if default_floor == DefaultFloor::ApngMax {
                    options.timeout.saturating_mul(19) / 20
                } else {
                    initial_bounded_phase_share(options.timeout)
                };
                Deadline::with_grace(started, primary_share, Duration::ZERO)
            }
        };
        Self {
            primary,
            terminal,
            reserve_terminal,
        }
    }

    /// The deadline governing terminal work and final timeout reporting; a
    /// primary-phase yield does not mark the file timed out.
    fn terminal_work(&self) -> &Deadline {
        if self.reserve_terminal {
            &self.terminal
        } else {
            &self.primary
        }
    }
}

/// Route families eligible for the parsed source and floor policy. Only
/// topology and size count; deadlines are checked when each route starts.
struct RouteEligibility {
    compact_tree: bool,
    compact_proven_feedback: bool,
    deft4j: bool,
    /// D4; Max runs it in its complete Default floor instead.
    default_source_topology: bool,
    narrow_source: bool,
    parallel_routes: bool,
}

impl RouteEligibility {
    fn new(context: &RouteContext<'_>) -> Self {
        let RouteContext {
            source,
            options,
            default_floor,
            ..
        } = *context;
        let bounded_png_max = options.exhaustive && default_floor.uses_bounded_png_routes();
        Self {
            compact_tree: bounded_png_max
                && compact_balanced_tree_source_eligible(
                    source.compressed.len(),
                    source.identity.decoded_size,
                    source.blocks,
                ),
            compact_proven_feedback: bounded_png_max
                && source.blocks.len() == 1
                && compact_proven_submatch_route_eligible(
                    &source.blocks[0].tokens,
                    source.blocks[0].plain.len(),
                ),
            // M1 applies under every floor policy, so a longer timeout can
            // always reach the deft4j endpoint.
            deft4j: options.exhaustive && deft4j_source_route_eligible(source.blocks),
            default_source_topology: !options.exhaustive
                && default_source_topology_eligible(
                    source.blocks,
                    source.compressed.len(),
                    source.identity.decoded_size,
                ),
            narrow_source: bounded_png_max
                && narrow_source_route_eligible(source.blocks, source.compressed.len()),
            parallel_routes: options.exhaustive
                && default_floor.is_bounded()
                && parallel_route_is_bounded(source),
        }
    }
}

/// Complete candidates and scheduling facts carried between route phases.
///
/// Each candidate field is an independently retained lineage. Phases replace
/// a lineage only with a strictly smaller complete candidate unless a comment
/// at the assignment says otherwise.
#[derive(Default)]
struct RouteState {
    /// The exact Default endpoint that Max must never lose.
    complete_default: Option<Candidate>,
    /// Default's exact endpoint where it differs from `complete_default`
    /// (after its final R1c, R13 and R14, or with R1b). It is compared only
    /// after Max's terminal searches, so it never becomes their parent.
    slid_default: Option<Candidate>,
    /// The Default comparison floor, which also seeds Max's routes.
    floor: Option<Candidate>,
    floor_seeded: Option<Candidate>,
    deft4j: Option<Candidate>,
    narrow: Option<Candidate>,
    source_max: Option<Candidate>,
    /// Consumed by [`run_proven_feedback_floor`], which merges it into `floor`.
    proven_feedback: Option<Candidate>,
    compact_split: Option<Candidate>,
    split_seeds: CompactSplitSeeds,
    completed_compact_split_parent: Option<Vec<u8>>,
    /// The direct deft4j route saved less than 2% on a multi-block
    /// single-image PNG, enabling the deft4j floor seed and compact split.
    weak_deft4j: bool,
    integrated_compact_source_max: bool,
    suppress_later_source_max: bool,
    suppress_later_optional_routes: bool,
    compact_split_attempted: bool,
    deft4j_refinement_completed: bool,
}

/// Prepared compact-split parents, one per distinct structural lineage.
#[derive(Default)]
struct CompactSplitSeeds {
    normal: Option<CompactSplitSeed>,
    seeded: Option<CompactSplitSeed>,
    deft4j: Option<CompactSplitSeed>,
}

impl CompactSplitSeeds {
    fn as_refs(&self) -> [Option<&CompactSplitSeed>; 3] {
        [
            self.normal.as_ref(),
            self.seeded.as_ref(),
            self.deft4j.as_ref(),
        ]
    }

    fn any(&self) -> bool {
        self.as_refs().into_iter().any(|seed| seed.is_some())
    }

    fn clear(&mut self) {
        *self = Self::default();
    }

    fn build(&self, context: &RouteContext<'_>) -> Result<Option<Candidate>> {
        build_prepared_compact_source_split_floors(
            self.as_refs(),
            context.options,
            context.source.decoded_limit,
            context.source.identity,
            context.deadline,
        )
    }
}

fn nonempty_block_count(blocks: &[ParsedBlock]) -> usize {
    blocks
        .iter()
        .filter(|block| !block.plain.is_empty())
        .count()
}

/// Normalize strict-mode aliases and report source opportunities.
fn prepare_source_blocks(
    options: &Options,
    progress: Progress,
    blocks: &mut [ParsedBlock],
) -> Result<()> {
    if options.strict {
        let normalization_started = progress.enabled().then(Instant::now);
        let normalized_blocks = normalize_258_aliases(blocks)?;
        if let Some(normalization_started) = normalization_started {
            progress.normalization(normalized_blocks, normalization_started.elapsed());
        }
    }
    if progress.enabled() {
        let opportunities = same_distance_opportunities(blocks);
        progress.same_distance_opportunities(SameDistanceProgress {
            runs: opportunities.runs,
            matches: opportunities.matches,
            decoded_bytes: opportunities.decoded_bytes,
            coalescible_runs: opportunities.coalescible_runs,
            repartition_runs: opportunities.repartition_runs,
            tokens_removable: opportunities.tokens_removable,
        });
        let mut tree_opportunities = BalancedTreeOpportunities::default();
        for block in blocks.iter() {
            let Some(seed) = block.original_dynamic.as_ref() else {
                continue;
            };
            if let Some(opportunities) = balanced_tree_opportunities(
                &block.literal_frequencies,
                &block.distance_frequencies,
                seed,
            ) {
                tree_opportunities.add_assign(opportunities);
            }
        }
        progress.balanced_tree_opportunities(BalancedTreeProgress {
            dynamic_blocks: tree_opportunities.dynamic_blocks,
            literal_pair_moves: tree_opportunities.literal_pair_moves,
            literal_quad_moves: tree_opportunities.literal_quad_moves,
            distance_pair_moves: tree_opportunities.distance_pair_moves,
            distance_quad_moves: tree_opportunities.distance_quad_moves,
            paired_prices: tree_opportunities.paired_prices,
        });
    }
    Ok(())
}

/// Run every eligible route family and return the selected complete candidate.
///
/// The bounded phases (through [`close_no_split_lineage`]) build independent
/// Max lineages beside the ordinary floor. [`establish_comparison_floor`] then
/// fixes the incumbent, which each later phase may only replace with a
/// strictly smaller complete candidate.
fn select_candidate(context: &RouteContext<'_>, deadlines: &StreamDeadlines) -> Result<Candidate> {
    let mut state = RouteState::default();
    let guaranteed_floor = build_guaranteed_floor(context, &mut state)?;
    let eligibility = RouteEligibility::new(context);
    run_bounded_phase(context, &eligibility, guaranteed_floor, &mut state)?;
    run_proven_feedback_floor(context, &eligibility, &mut state)?;
    prepare_compact_split_seeds(context, &mut state)?;
    finish_floor_seeded_tree(context, &eligibility, &mut state)?;
    continue_floor_seeded(context, &eligibility, &mut state)?;
    run_bounded_refinement(context, &eligibility, &mut state)?;
    close_no_split_lineage(context, &mut state)?;

    let mut candidate = establish_comparison_floor(context, &mut state)?;
    merge_independent_topologies(context, &eligibility, &mut state, &mut candidate)?;
    run_fragmented_collection(context, &state, &mut candidate)?;
    let deferred_split_parent = if context.options.exhaustive {
        run_max_source_routes(context, &eligibility, &mut state, &mut candidate)?
    } else {
        None
    };
    run_complementary_no_split(context, &eligibility, &mut candidate)?;
    run_compact_balanced_tree(context, &eligibility, &mut candidate)?;

    // Compare `Complete`'s retained Default endpoint only now, so it cannot
    // redirect the rewritten-seed refinement.
    if let Some(complete_default) = state.complete_default.take() {
        candidate.replace_if_smaller(complete_default);
    }
    if let Some(parent) = deferred_split_parent {
        run_deferred_source_max_split(context, deadlines, &parent, &mut candidate)?;
    }
    let source_topology = build_default_source_topology(context, &eligibility, &candidate)?;
    let mut candidate = finish_terminal_searches(context, deadlines, candidate)?;
    if let Some(source_topology) = source_topology {
        let finished = finish_terminal_searches(context, deadlines, source_topology)?;
        candidate.replace_if_smaller(finished);
    }
    if let Some(slid_default) = state.slid_default.take() {
        candidate.replace_if_smaller(slid_default);
    }
    Ok(candidate)
}

/// Build Default's source topology (D4), returned only when it is already
/// strictly smaller than the selected candidate. The source's own blocks,
/// each re-spelled by a length-family chain and searched from it, can reach a
/// different basin from the planner's, whose match-by-match expansion stops
/// where every remaining match is cheaper than its literals alone. Both
/// endpoints are then finished and the smaller kept, so the planner's endpoint
/// is never lost.
fn build_default_source_topology(
    context: &RouteContext<'_>,
    eligibility: &RouteEligibility,
    candidate: &Candidate,
) -> Result<Option<Candidate>> {
    if !(eligibility.default_source_topology && context.deadline.can_start_route()) {
        return Ok(None);
    }
    let step = context.progress.start("Source-layout search");
    let topology = build_source_family_candidate(
        context.source,
        context.options,
        &mut context.deadline.hard_stop(),
    )?;
    step.finish(context.report_optional(topology.as_ref()));
    Ok(topology
        .filter(|topology| topology.is_strictly_smaller_than(candidate))
        .map(|mut topology| {
            topology.plans.clear();
            topology
        }))
}

/// Build the comparison floor that some policies need before the bounded
/// routes (M0). `Established` copies the caller's stream; for
/// `CompleteThenBounded`, see [`super::PREBUILD_BOUNDED_FLOOR_MAX_DECODED`].
fn build_guaranteed_floor(
    context: &RouteContext<'_>,
    state: &mut RouteState,
) -> Result<Option<Candidate>> {
    let RouteContext {
        source,
        options,
        default_floor,
        deadline,
        progress,
    } = *context;
    let prebuild_floor_first = options.exhaustive
        && match default_floor {
            DefaultFloor::CompleteThenBounded => {
                prebuild_bounded_floor(
                    nonempty_block_count(source.blocks),
                    source.identity.decoded_size,
                ) || source_run_match_count_exceeds(source.blocks, PROVEN_SUBMATCH_FULL_MATCH_LIMIT)
            }
            DefaultFloor::Shared
            | DefaultFloor::SharedExact
            | DefaultFloor::ApngDefault
            | DefaultFloor::ApngMax => true,
            DefaultFloor::Established => false,
            DefaultFloor::Complete | DefaultFloor::MandatoryComplete => false,
        };
    let step = prebuild_floor_first.then(|| progress.start("Normal comparison floor"));
    let floor = if default_floor == DefaultFloor::Established {
        Some(established_floor_candidate(source)?)
    } else if prebuild_floor_first {
        Some(match default_floor {
            DefaultFloor::CompleteThenBounded | DefaultFloor::SharedExact => {
                let floors = build_complete_default_floor_candidate(
                    source,
                    options,
                    progress,
                    context.timed(),
                )?;
                state.complete_default = Some(floors.complete);
                state.slid_default = floors.slid;
                floors.max_seed
            }
            // APNG's initial planner and terminal methods define its exact
            // Default endpoint. The replay-bounded seed is a separate lineage:
            // a smaller seed does not dominate its terminal children.
            DefaultFloor::ApngMax => {
                let (complete, slid) = build_complete_apng_default_floor_candidate(
                    source,
                    options,
                    progress,
                    context.timed(),
                )?;
                state.complete_default = Some(complete);
                state.slid_default = slid;
                build_bounded_floor_candidate(source, options, &mut SearchStop::never())?
            }
            _ => build_bounded_floor_candidate(source, options, &mut deadline.hard_stop())?,
        })
    } else {
        None
    };
    if let Some(step) = step {
        let reported_floor = state.complete_default.as_ref().or(floor.as_ref());
        step.finish(context.report_optional(reported_floor));
    }
    Ok(floor)
}

// Out of line: inlined, it keeps `select_candidate` from inlining into its
// one caller, which costs several KiB of code.
#[inline(never)]
fn run_bounded_phase(
    context: &RouteContext<'_>,
    eligibility: &RouteEligibility,
    guaranteed_floor: Option<Candidate>,
    state: &mut RouteState,
) -> Result<()> {
    let RouteContext {
        source,
        options,
        default_floor,
        deadline,
        progress,
    } = *context;
    let png_policy = if default_floor.uses_bounded_png_routes() && eligibility.parallel_routes {
        let floor_exposes_new_states = guaranteed_floor
            .as_ref()
            .is_some_and(|floor| floor_exposes_new_search_states(source.blocks, &floor.plans));
        bounded_png_max_policy(
            nonempty_block_count(source.blocks),
            floor_exposes_new_states,
            eligibility.deft4j,
            eligibility.narrow_source,
        )
    } else {
        BoundedPngMaxPolicy::default()
    };
    let bounded = options.exhaustive && default_floor.is_bounded();
    let step = bounded.then(|| progress.start("Bounded comparison routes"));
    let candidates = if bounded {
        match png_policy {
            BoundedPngMaxPolicy::GenericParallel => build_bounded_generic_max_candidates(
                source,
                options,
                deadline,
                progress,
                guaranteed_floor,
            )?,
            BoundedPngMaxPolicy::Standard | BoundedPngMaxPolicy::FloorExpansion => {
                let run_deft4j = eligibility.deft4j && deadline.can_start_route();
                let floor_expansion = png_policy == BoundedPngMaxPolicy::FloorExpansion;
                let run_source_max = eligibility.parallel_routes
                    && floor_expansion
                    && if default_floor.uses_bounded_png_routes() {
                        complete_png_parallel_source_max_work_class(source)
                    } else {
                        bounded_parallel_source_max_work_class(source)
                    };
                let routes = BoundedRoutes {
                    preserve_complete_default: default_floor == DefaultFloor::CompleteThenBounded,
                    seeded_max: floor_expansion,
                    deft4j: run_deft4j,
                    narrow: eligibility.narrow_source,
                    source_max: run_source_max,
                    proven_feedback: run_source_max && eligibility.compact_proven_feedback,
                    parallel: eligibility.parallel_routes,
                };
                build_bounded_phase_candidates(
                    source,
                    options,
                    routes,
                    deadline,
                    progress,
                    guaranteed_floor,
                )?
            }
        }
    } else {
        // Every policy that prebuilds a floor is bounded.
        debug_assert!(guaranteed_floor.is_none());
        BoundedPhaseCandidates::default()
    };
    state.floor = candidates.floor;
    // The bounded phase builds a slid copy only when no endpoint was prebuilt.
    state.slid_default = candidates.slid_default.or(state.slid_default.take());
    state.floor_seeded = candidates.floor_seeded;
    state.deft4j = candidates.deft4j;
    state.narrow = candidates.narrow;
    state.source_max = candidates.source_max;
    state.proven_feedback = candidates.proven_feedback;
    state.suppress_later_source_max = candidates.suppress_later_source_max;
    state.suppress_later_optional_routes = candidates.suppress_later_optional_routes;
    state.completed_compact_split_parent = candidates.completed_compact_split_parent;
    if let Some(step) = step {
        step.finish_phase();
        for (name, candidate) in [
            ("Normal floor", state.floor.as_ref()),
            ("Columbo floor-seeded", state.floor_seeded.as_ref()),
            ("deft4j-derived source", state.deft4j.as_ref()),
            ("No-split source", state.narrow.as_ref()),
            ("Columbo source max", state.source_max.as_ref()),
            ("Columbo proven-feedback", state.proven_feedback.as_ref()),
        ] {
            if let Some(candidate) = candidate {
                progress.candidate(name, context.report(candidate));
            }
        }
    }
    Ok(())
}

/// Price the compact proven-feedback candidate (M3) if the bounded phase did
/// not, and merge it into the floor before source max runs.
fn run_proven_feedback_floor(
    context: &RouteContext<'_>,
    eligibility: &RouteEligibility,
    state: &mut RouteState,
) -> Result<()> {
    let RouteContext {
        source,
        options,
        deadline,
        progress,
        ..
    } = *context;
    let run = eligibility.compact_proven_feedback
        && state.proven_feedback.is_none()
        && deadline.can_start_route();
    let step = run.then(|| progress.start("Columbo proven-feedback floor"));
    if run {
        state.proven_feedback =
            build_compact_proven_feedback_candidate(source, options, &mut deadline.hard_stop())?;
    }
    if let Some(step) = step {
        step.finish(context.report_optional(state.proven_feedback.as_ref()));
    }
    // S0: only when proven feedback ties the floor's bytes but wins bits does
    // later source max use the integrated proven order to look for the next
    // byte. Larger blocks use that order inside the stream planner.
    state.integrated_compact_source_max = state
        .proven_feedback
        .as_ref()
        .zip(state.floor.as_ref())
        .is_some_and(|(proven, normal)| {
            proven.data.len() == normal.data.len() && proven.bits < normal.bits
        });
    if let Some(proven_feedback) = state.proven_feedback.take() {
        replace_optional_if_smaller(&mut state.floor, proven_feedback);
    }
    Ok(())
}

/// Decide whether compact split (M10c) applies and prepare its distinct
/// parents. This precedes choosing which route gets the remaining time, since
/// only a prepared parent shows that it meets the split's limits.
fn prepare_compact_split_seeds(context: &RouteContext<'_>, state: &mut RouteState) -> Result<()> {
    let source = context.source;
    let (decoded_limit, identity) = (source.decoded_limit, source.identity);
    state.weak_deft4j = context.default_floor.uses_bounded_png_routes()
        && state.deft4j.as_ref().is_some_and(|deft4j| {
            has_multiple_nonempty_blocks(source.blocks)
                && gain_is_below(
                    source.meaningful_bits,
                    deft4j.bits,
                    WEAK_DEFT4J_GAIN_BASIS_POINTS,
                )
        });
    if state.weak_deft4j {
        // Split pricing is not monotone in the parent's encoded size, so keep
        // each distinct parent.
        let completed = state.completed_compact_split_parent.as_deref();
        let normal_parent = state
            .floor
            .as_ref()
            .filter(|candidate| !compact_split_parent_is_completed(candidate, completed));
        let seeded_parent = state.floor_seeded.as_ref().filter(|seeded| {
            !compact_split_parent_is_completed(seeded, completed)
                && normal_parent.map_or(true, |normal| normal.data != seeded.data)
        });
        let deft4j_parent = state.deft4j.as_ref().filter(|deft4j| {
            !compact_split_parent_is_completed(deft4j, completed)
                && [normal_parent, seeded_parent]
                    .into_iter()
                    .flatten()
                    .all(|seed| seed.data != deft4j.data)
        });
        let prepare = |parent: Option<&Candidate>| {
            parent
                .map(|candidate| {
                    prepare_compact_source_split_seed(candidate, decoded_limit, identity)
                })
                .transpose()
                .map(Option::flatten)
        };
        state.split_seeds = CompactSplitSeeds {
            normal: prepare(normal_parent)?,
            seeded: prepare(seeded_parent)?,
            deft4j: prepare(deft4j_parent)?,
        };
    }
    if let Some(floor) = &mut state.floor {
        // Later phases need only the floor's bytes.
        floor.plans.clear();
    }
    Ok(())
}

/// Run compact split and balanced-tree cleanup (M10c, M4) on the completed
/// floor-seeded Max parent. The cleanup is deterministic, so it may run after
/// the soft deadline.
fn finish_floor_seeded_tree(
    context: &RouteContext<'_>,
    eligibility: &RouteEligibility,
    state: &mut RouteState,
) -> Result<()> {
    let RouteContext {
        source,
        options,
        deadline,
        ..
    } = *context;
    let (decoded_limit, identity) = (source.decoded_limit, source.identity);
    if eligibility.compact_tree {
        if let Some(seeded) = &mut state.floor_seeded {
            // A one-block source may have become a multi-block floor, which
            // the compact split can use but the one-block routes cannot.
            if let Some(split) = refine_with_compact_source_split_floor_until(
                seeded,
                options,
                decoded_limit,
                identity,
                &mut deadline.hard_stop(),
            )? {
                seeded.replace_if_smaller(split);
            }
            if let Some(mut tree) =
                refine_with_compact_balanced_tree_floor(seeded, options, decoded_limit, identity)?
            {
                if let Some(feedback) =
                    refine_with_compact_proven_feedback(&tree, options, decoded_limit, identity)?
                {
                    tree.replace_if_smaller(feedback);
                }
                seeded.replace_if_smaller(tree);
            }
        }
    }
    if let Some(seeded) = &mut state.floor_seeded {
        seeded.plans.clear();
    }
    Ok(())
}

/// Continue the floor-seeded lineage before weaker parents when it leads and is
/// not a Max fixed point (M7a).
fn continue_floor_seeded(
    context: &RouteContext<'_>,
    eligibility: &RouteEligibility,
    state: &mut RouteState,
) -> Result<()> {
    let RouteContext {
        source,
        options,
        default_floor,
        deadline,
        progress,
    } = *context;
    let (decoded_limit, identity) = (source.decoded_limit, source.identity);
    // With parallel routes, run a pending compact split beside this
    // continuation so neither starves the other.
    let compact_split_pending = state.split_seeds.any();
    let overlap_compact_split = compact_split_pending && eligibility.parallel_routes;
    let continue_best = state.floor_seeded.as_ref().is_some_and(|seeded| {
        !seeded.max_planner_is_stable
            && state.floor.as_ref().is_some_and(|floor| {
                seeded.is_strictly_smaller_than(floor)
                    && floor_seeded_priority_with_structural_sibling(
                        floor.bits,
                        seeded.bits,
                        compact_split_pending,
                        overlap_compact_split,
                    )
            })
            && [
                state.deft4j.as_ref(),
                state.narrow.as_ref(),
                state.source_max.as_ref(),
            ]
            .into_iter()
            .flatten()
            .all(|other| !other.is_strictly_smaller_than(seeded))
    }) && deadline.can_start_route();
    // Run the deft4j refinement beside the continuation, which could otherwise
    // use up any longer deadline. This stays within three workers.
    let overlap_deft4j = independent_deft4j_refinement_can_overlap(
        continue_best,
        default_floor.uses_bounded_png_routes(),
        eligibility.parallel_routes,
        state.weak_deft4j || state.deft4j.is_some(),
    );
    // An APNG child has no time left after a continuation that uses its
    // whole window, so run original-source Max beside it.
    let overlap_source_max = continue_best
        && default_floor == DefaultFloor::ApngMax
        && eligibility.parallel_routes
        && state.source_max.is_none()
        && !state.suppress_later_source_max
        && deadline.can_start_route();
    let floor_seeded_step =
        continue_best.then(|| progress.start("Columbo floor-seeded continuation"));
    let split_step = (continue_best && overlap_compact_split)
        .then(|| progress.start("Columbo compact split floor"));
    let deft4j_step = overlap_deft4j.then(|| progress.start("deft4j-derived refinement"));
    let mut floor_seeded_changed = false;
    if continue_best {
        let refined = if overlap_source_max {
            continue_beside_source_max(context, state)?
        } else if overlap_deft4j {
            continue_beside_deft4j_refinement(context, state, overlap_compact_split)?
        } else if overlap_compact_split {
            continue_beside_compact_split(context, state)?
        } else {
            let seeded = floor_seeded_parent(state);
            Some(refine_with_max_planner(
                seeded,
                options,
                decoded_limit,
                identity,
                &mut deadline.hard_stop(),
            )?)
        };
        if let Some(refined) = refined {
            let seeded = state
                .floor_seeded
                .as_mut()
                .expect("continuation requires a floor-seeded candidate");
            floor_seeded_changed = seeded.replace_keeping_parent(refined, options.exhaustive);
        }
    }
    if let Some(step) = floor_seeded_step {
        step.finish(context.report_optional(state.floor_seeded.as_ref()));
    }
    if let Some(step) = split_step {
        step.finish(context.report_optional(state.compact_split.as_ref()));
    }
    if let Some(step) = deft4j_step {
        step.finish(context.report_optional(state.deft4j.as_ref()));
    }
    if floor_seeded_changed && state.weak_deft4j {
        // Prepare the changed parent unless it matches the normal or deft4j
        // seed.
        let seeds = &state.split_seeds;
        let completed = state.completed_compact_split_parent.as_deref();
        let refreshed = state
            .floor_seeded
            .as_ref()
            .filter(|seeded| {
                !compact_split_parent_is_completed(seeded, completed)
                    && [seeds.normal.as_ref(), seeds.deft4j.as_ref()]
                        .into_iter()
                        .flatten()
                        .all(|seed| seed.data != seeded.data)
            })
            .map(|seeded| prepare_compact_source_split_seed(seeded, decoded_limit, identity))
            .transpose()?
            .flatten();
        state.split_seeds.seeded = refreshed;
    }
    Ok(())
}

fn floor_seeded_parent(state: &RouteState) -> &Candidate {
    state
        .floor_seeded
        .as_ref()
        .expect("continuation requires a floor-seeded candidate")
}

/// Continue the floor-seeded parent beside an original-source Max worker.
fn continue_beside_source_max(
    context: &RouteContext<'_>,
    state: &mut RouteState,
) -> Result<Option<Candidate>> {
    let RouteContext {
        source,
        options,
        deadline,
        progress,
        ..
    } = *context;
    let (decoded_limit, identity) = (source.decoded_limit, source.identity);
    let seeded = floor_seeded_parent(state);
    let integrated_compact_source_max = state.integrated_compact_source_max;
    let (refined, source_max) =
        thread::scope(|scope| -> Result<(Option<Candidate>, Option<Candidate>)> {
            let source_worker =
                spawn_route(scope, "columbo-source-max-continuation", deadline, || {
                    build_source_max_candidate(
                        source,
                        options,
                        progress,
                        deadline,
                        integrated_compact_source_max,
                        &mut deadline.hard_stop(),
                    )
                });
            let Some(source_worker) = source_worker else {
                // Source max can still run serially later.
                let refined = refine_with_max_planner(
                    seeded,
                    options,
                    decoded_limit,
                    identity,
                    &mut deadline.hard_stop(),
                )?;
                return Ok((Some(refined), None));
            };
            let refined = refine_with_max_planner(
                seeded,
                options,
                decoded_limit,
                identity,
                &mut deadline.hard_stop(),
            );
            if refined.is_err() {
                deadline.cancel_routes();
            }
            let source_max = join_route(source_worker)?;
            Ok((Some(refined?), Some(source_max)))
        })?;
    if let Some(source_max) = source_max {
        state.source_max = Some(source_max);
        state.suppress_later_source_max = true;
    }
    Ok(refined)
}

/// Continue the floor-seeded parent beside the deft4j-derived refinement,
/// and beside compact split when that sibling can also overlap.
fn continue_beside_deft4j_refinement(
    context: &RouteContext<'_>,
    state: &mut RouteState,
    overlap_compact_split: bool,
) -> Result<Option<Candidate>> {
    let RouteContext {
        source,
        options,
        deadline,
        ..
    } = *context;
    let (decoded_limit, identity) = (source.decoded_limit, source.identity);
    let RouteState {
        floor,
        floor_seeded,
        deft4j,
        narrow,
        split_seeds,
        weak_deft4j,
        ..
    } = state;
    let seeded = floor_seeded
        .as_ref()
        .expect("continuation requires a floor-seeded candidate");
    let (floor, narrow, split_seeds, weak_deft4j) =
        (floor.as_ref(), narrow.as_ref(), &*split_seeds, *weak_deft4j);
    let (refined, split, refinement_completed) = thread::scope(
        |scope| -> Result<(Option<Candidate>, Option<Candidate>, bool)> {
            let refinement_worker =
                spawn_route(scope, "columbo-deft4j-continuation", deadline, || {
                    refine_bounded_deft4j_lineage(
                        source,
                        options,
                        decoded_limit,
                        identity,
                        deadline,
                        floor,
                        narrow,
                        weak_deft4j,
                        false,
                        deft4j,
                    )
                });
            let Some(refinement_worker) = refinement_worker else {
                // The serial phase below runs the refinement.
                return Ok((None, None, false));
            };
            let split_worker = overlap_compact_split
                .then(|| {
                    spawn_route(
                        scope,
                        "columbo-compact-split-continuation",
                        deadline,
                        || split_seeds.build(context),
                    )
                })
                .flatten();
            let refined = refine_with_max_planner(
                seeded,
                options,
                decoded_limit,
                identity,
                &mut deadline.hard_stop(),
            );
            if refined.is_err() {
                deadline.cancel_routes();
            }
            join_route(refinement_worker)?;
            let split = match split_worker {
                Some(worker) => join_route(worker)?,
                None if overlap_compact_split => split_seeds.build(context)?,
                None => None,
            };
            Ok((Some(refined?), split, true))
        },
    )?;
    state.deft4j_refinement_completed = refinement_completed;
    if refinement_completed && overlap_compact_split {
        state.compact_split_attempted = true;
        state.split_seeds.clear();
    }
    if let Some(split) = split {
        replace_optional_if_smaller(&mut state.compact_split, split);
    }
    Ok(refined)
}

/// Continue the floor-seeded parent beside the compact split sibling.
fn continue_beside_compact_split(
    context: &RouteContext<'_>,
    state: &mut RouteState,
) -> Result<Option<Candidate>> {
    let RouteContext {
        source,
        options,
        deadline,
        ..
    } = *context;
    let (decoded_limit, identity) = (source.decoded_limit, source.identity);
    let seeded = floor_seeded_parent(state);
    let split_seeds = &state.split_seeds;
    let (refined, split) =
        thread::scope(|scope| -> Result<(Option<Candidate>, Option<Candidate>)> {
            let split_worker = spawn_route(
                scope,
                "columbo-compact-split-continuation",
                deadline,
                || split_seeds.build(context),
            );
            let Some(split_worker) = split_worker else {
                return split_seeds.build(context).map(|split| (None, split));
            };
            let refined = refine_with_max_planner(
                seeded,
                options,
                decoded_limit,
                identity,
                &mut deadline.hard_stop(),
            );
            if refined.is_err() {
                deadline.cancel_routes();
            }
            let split = join_route(split_worker)?;
            Ok((Some(refined?), split))
        })?;
    state.compact_split_attempted = true;
    state.split_seeds.clear();
    if let Some(split) = split {
        replace_optional_if_smaller(&mut state.compact_split, split);
    }
    Ok(refined)
}

/// Run the bounded follow-up routes (M10b, M2c), then finish compact split
/// serially if no concurrent route attempted it.
fn run_bounded_refinement(
    context: &RouteContext<'_>,
    eligibility: &RouteEligibility,
    state: &mut RouteState,
) -> Result<()> {
    let RouteContext {
        source,
        options,
        default_floor,
        deadline,
        progress,
    } = *context;
    let (decoded_limit, identity) = (source.decoded_limit, source.identity);
    let run_bounded_refinement = default_floor.is_bounded() && deadline.can_start_route();
    let compact_split_work_possible = run_bounded_refinement
        && (state.split_seeds.any()
            // Refinement can expose an eligible parent even when no early
            // candidate qualifies.
            || (state.weak_deft4j && compact_dependent_deft4j_work_class(source)))
        // The hard-deadline fallback below may still finish one parent.
        || (state.weak_deft4j && !state.compact_split_attempted && !deadline.expired());
    let compact_split_step =
        compact_split_work_possible.then(|| progress.start("Columbo compact split floor"));
    // M2c takes the source-max worker's slot, keeping the bounded phase within
    // three workers; source max remains eligible later.
    let continue_best_narrow = run_bounded_refinement
        && state.narrow.as_ref().is_some_and(|narrow| {
            changed_narrow_parent_should_continue(
                candidate_exposes_new_parent(narrow, source),
                narrow,
                source,
            )
        })
        && deadline.can_start_route();
    let narrow_step = continue_best_narrow.then(|| progress.start("Columbo no-split continuation"));
    // Release the token plans before reparsing, so the models do not overlap
    // at peak memory.
    for candidate in [&mut state.deft4j, &mut state.narrow, &mut state.source_max]
        .into_iter()
        .flatten()
    {
        candidate.plans.clear();
    }
    let refinement_step = (run_bounded_refinement
        && !state.deft4j_refinement_completed
        && (state.weak_deft4j || state.deft4j.is_some()))
    .then(|| progress.start("deft4j-derived refinement"));
    if run_bounded_refinement {
        let routes = FollowUpRoutes {
            source_max: options.exhaustive
                && default_floor.allows_parallel_source_follow_up()
                && eligibility.parallel_routes
                && state.source_max.is_none()
                && !state.suppress_later_source_max
                && !continue_best_narrow
                && deadline.can_start_route(),
            narrow: continue_best_narrow,
            compact_split: state.split_seeds.any(),
        };
        let follow_up = if routes.source_max || routes.narrow || routes.compact_split {
            run_concurrent_follow_up(context, state, routes)?
        } else {
            if !state.deft4j_refinement_completed {
                refine_bounded_deft4j_lineage(
                    source,
                    options,
                    decoded_limit,
                    identity,
                    deadline,
                    state.floor.as_ref(),
                    state.narrow.as_ref(),
                    state.weak_deft4j,
                    false,
                    &mut state.deft4j,
                )?;
            }
            BoundedFollowUpCandidates::default()
        };
        state.compact_split_attempted |= follow_up.attempted_compact_split;
        if let Some(compact_split) = follow_up.compact_split {
            replace_optional_if_smaller(&mut state.compact_split, compact_split);
        }
        if let Some(source_max) = follow_up.source_max {
            state.source_max = Some(source_max);
            state.suppress_later_source_max = true;
        }
        if let Some(continued) = follow_up.narrow {
            match state.narrow.as_mut() {
                Some(narrow) => {
                    narrow.replace_keeping_parent(continued, options.exhaustive);
                }
                None => state.narrow = Some(continued),
            }
        }
    }
    if let Some(step) = narrow_step {
        step.finish(context.report_optional(state.narrow.as_ref()));
    }
    if let Some(step) = refinement_step {
        step.finish(context.report_optional(state.deft4j.as_ref()));
    }
    // If no concurrent phase ran compact split, finish it serially before the
    // hard deadline; the `_until` variant keeps the parent if a split trial
    // runs out of grace.
    if state.weak_deft4j && !state.compact_split_attempted && !deadline.expired() {
        let completed = state.completed_compact_split_parent.as_deref();
        state.compact_split = match state.deft4j.as_ref() {
            Some(deft4j) if !compact_split_parent_is_completed(deft4j, completed) => {
                refine_with_compact_source_split_floor_until(
                    deft4j,
                    options,
                    decoded_limit,
                    identity,
                    &mut deadline.hard_stop(),
                )?
            }
            None => None,
            Some(_) => None,
        };
    }
    if let Some(step) = compact_split_step {
        step.finish(context.report_optional(state.compact_split.as_ref()));
    }
    if let Some(split) = state.compact_split.take() {
        replace_optional_if_smaller(&mut state.deft4j, split);
    }
    Ok(())
}

/// Workers that run beside the deft4j-derived refinement.
#[derive(Clone, Copy)]
struct FollowUpRoutes {
    source_max: bool,
    narrow: bool,
    compact_split: bool,
}

/// Refine the deft4j lineage on this thread while the requested siblings run
/// on scoped workers. Every started worker joins before any error is returned.
fn run_concurrent_follow_up(
    context: &RouteContext<'_>,
    state: &mut RouteState,
    routes: FollowUpRoutes,
) -> Result<BoundedFollowUpCandidates> {
    let RouteContext {
        source,
        options,
        deadline,
        progress,
        ..
    } = *context;
    let (decoded_limit, identity) = (source.decoded_limit, source.identity);
    let RouteState {
        floor,
        deft4j,
        narrow,
        split_seeds,
        completed_compact_split_parent,
        weak_deft4j,
        integrated_compact_source_max,
        deft4j_refinement_completed,
        ..
    } = state;
    let (floor, narrow, split_seeds) = (floor.as_ref(), narrow.as_ref(), &*split_seeds);
    let completed_compact_split_parent = completed_compact_split_parent.as_deref();
    let (weak_deft4j, integrated_compact_source_max, deft4j_refinement_completed) = (
        *weak_deft4j,
        *integrated_compact_source_max,
        *deft4j_refinement_completed,
    );
    thread::scope(|scope| -> Result<BoundedFollowUpCandidates> {
        let source_worker = routes
            .source_max
            .then(|| {
                spawn_route(scope, "columbo-source-max-follow-up", deadline, || {
                    build_source_max_candidate(
                        source,
                        options,
                        progress,
                        deadline,
                        integrated_compact_source_max,
                        &mut deadline.hard_stop(),
                    )
                })
            })
            .flatten();
        let compact_worker = routes
            .compact_split
            .then(|| {
                spawn_route(scope, "columbo-compact-split-lineages", deadline, || {
                    split_seeds.build(context)
                })
            })
            .flatten();
        let narrow_worker = routes
            .narrow
            .then(|| {
                spawn_route(scope, "columbo-no-split-continuation", deadline, || {
                    continue_no_split(context, narrow)
                })
            })
            .flatten();
        let refinement = if deft4j_refinement_completed {
            Ok(())
        } else {
            refine_bounded_deft4j_lineage(
                source,
                options,
                decoded_limit,
                identity,
                deadline,
                floor,
                narrow,
                weak_deft4j,
                routes.compact_split,
                deft4j,
            )
        };
        if refinement.is_err() {
            deadline.cancel_routes();
        }
        // A different refined topology is another split parent. Price it
        // while source max runs, if soft time remains or it strictly beats
        // every early parent.
        let dependent_split = if refinement.is_ok() && weak_deft4j {
            deft4j.as_ref().and_then(|deft4j| {
                let duplicates_early_seed = split_seeds
                    .as_refs()
                    .into_iter()
                    .flatten()
                    .any(|seed| seed.data == deft4j.data)
                    || compact_split_parent_is_completed(deft4j, completed_compact_split_parent);
                let improves_early_seeds =
                    split_seeds.as_refs().into_iter().flatten().all(|seed| {
                        is_strictly_better(
                            deft4j.data.len(),
                            deft4j.bits,
                            seed.data.len(),
                            seed.bits,
                        )
                    });
                (!duplicates_early_seed && (improves_early_seeds || deadline.can_start_route()))
                    .then(|| {
                        refine_with_compact_source_split_floor(
                            deft4j,
                            options,
                            decoded_limit,
                            identity,
                        )
                    })
            })
        } else {
            None
        };
        let source_max = match source_worker {
            Some(worker) => Some(join_route(worker)?),
            None => None,
        };
        let narrow_continuation = match narrow_worker {
            Some(worker) => join_route(worker),
            None if routes.narrow => continue_no_split(context, narrow),
            None => Ok(None),
        }?;
        let attempted_compact_split = routes.compact_split || dependent_split.is_some();
        let early_split = match compact_worker {
            Some(worker) => join_route(worker),
            None => split_seeds.build(context),
        };
        refinement?;
        let mut compact_split = early_split?;
        if let Some(result) = dependent_split {
            if let Some(candidate) = result? {
                replace_optional_if_smaller(&mut compact_split, candidate);
            }
        }
        Ok(BoundedFollowUpCandidates {
            source_max,
            attempted_compact_split,
            compact_split,
            narrow: narrow_continuation,
        })
    })
}

/// Continue a changed no-split parent once with the same no-split route.
fn continue_no_split(
    context: &RouteContext<'_>,
    narrow: Option<&Candidate>,
) -> Result<Option<Candidate>> {
    let narrow = narrow.expect("scheduled no-split continuation");
    let deadline = context.deadline;
    let mut route_stop = deadline.hard_stop();
    let mut refinement_stop = deadline.hard_stop();
    refine_with_no_split_route(
        narrow,
        context.options,
        context.source.decoded_limit,
        context.source.identity,
        &mut route_stop,
        &mut refinement_stop,
    )
}

/// Fold the no-split candidate into the deft4j lineage, first running the
/// terminal tree floors on a changed topology that would otherwise lose.
fn close_no_split_lineage(context: &RouteContext<'_>, state: &mut RouteState) -> Result<()> {
    let RouteContext {
        source,
        options,
        progress,
        ..
    } = *context;
    if let Some(deft4j) = &mut state.deft4j {
        deft4j.plans.clear();
    }
    if let Some(mut narrow) = state.narrow.take() {
        // A winner gets the tree floors later, at Max's terminal stage.
        let narrow_would_be_retained = [
            state.floor.as_ref(),
            state.floor_seeded.as_ref(),
            state.deft4j.as_ref(),
            state.source_max.as_ref(),
            state.complete_default.as_ref(),
        ]
        .into_iter()
        .flatten()
        .all(|other| narrow.is_strictly_smaller_than(other));
        if candidate_exposes_new_parent(&narrow, source) && !narrow_would_be_retained {
            narrow = improve_with_terminal_tree_floors(
                source,
                options,
                context.timed(),
                progress,
                narrow,
            )?;
        }
        replace_optional_if_smaller(&mut state.deft4j, narrow);
    }
    Ok(())
}

/// Select the incumbent every later route must strictly improve. The complete
/// Default endpoint joins the comparison here, after the bounded Max routes,
/// because seeding them from it can lead to a different local optimum.
fn establish_comparison_floor(
    context: &RouteContext<'_>,
    state: &mut RouteState,
) -> Result<Candidate> {
    let RouteContext {
        source,
        options,
        default_floor,
        deadline,
        progress,
    } = *context;
    if let Some(complete_default) = state.complete_default.take() {
        replace_optional_if_smaller(&mut state.floor, complete_default);
    }

    let initial_step = state.floor.is_none().then(|| {
        progress.start(if options.exhaustive {
            "Normal comparison floor"
        } else {
            "Normal route"
        })
    });
    let mut candidate = if let Some(floor) = state.floor.take() {
        floor
    } else if options.exhaustive {
        // Max finishes the Default route first; it stays the floor even when
        // the deadline cuts optional search.
        match default_floor {
            DefaultFloor::Complete => {
                let floors = build_complete_default_floor_candidate(
                    source,
                    options,
                    progress,
                    context.timed(),
                )?;
                state.complete_default = Some(floors.complete);
                state.slid_default = floors.slid;
                floors.max_seed
            }
            DefaultFloor::MandatoryComplete => build_candidate(
                source,
                options,
                DEFAULT_RAW_REPLAY_LIMIT,
                &mut SearchStop::never(),
            )?,
            DefaultFloor::CompleteThenBounded
            | DefaultFloor::Shared
            | DefaultFloor::SharedExact
            | DefaultFloor::ApngDefault
            | DefaultFloor::ApngMax
            | DefaultFloor::Established => {
                build_bounded_floor_candidate(source, options, &mut deadline.hard_stop())?
            }
        }
    } else if default_floor == DefaultFloor::ApngDefault {
        // Leave replay and the endpoint-proven lineage to Max, so APNG
        // Default cost does not grow with every frame's route count.
        build_apng_default_candidate(source, options, &mut deadline.hard_stop())?
    } else if default_floor == DefaultFloor::MandatoryComplete {
        build_candidate(
            source,
            options,
            DEFAULT_RAW_REPLAY_LIMIT,
            &mut SearchStop::never(),
        )?
    } else {
        build_candidate(
            source,
            options,
            DEFAULT_RAW_REPLAY_LIMIT,
            &mut deadline.hard_stop(),
        )?
    };
    if let Some(step) = initial_step {
        step.finish(Some(context.report(&candidate)));
    }
    // Default adds D1–D3 and the terminal tree floors, except for
    // `ApngDefault`, for the same per-frame reason; Max runs them for APNG.
    if !options.exhaustive && default_floor != DefaultFloor::ApngDefault {
        let floor_work = if default_floor == DefaultFloor::MandatoryComplete {
            DefaultFloorWork::Mandatory
        } else {
            context.timed()
        };
        candidate =
            improve_default_floor_with_feedback(source, options, floor_work, progress, candidate)?;
    }
    Ok(candidate)
}

/// Run the terminal tree floors on a losing contender before comparing it,
/// since a different topology may win only after them. A winner gets the same
/// work at the end of Max.
fn close_losing_topology(
    context: &RouteContext<'_>,
    incumbent: &Candidate,
    contender: Candidate,
) -> Result<Candidate> {
    if contender.is_strictly_smaller_than(incumbent) {
        return Ok(contender);
    }
    improve_with_terminal_tree_floors(
        context.source,
        context.options,
        context.timed(),
        context.progress,
        contender,
    )
}

/// Compare the floor-seeded and deft4j-derived topologies with the incumbent,
/// first building the deft4j candidate (M1) for `Complete`, which has no
/// bounded phase.
fn merge_independent_topologies(
    context: &RouteContext<'_>,
    eligibility: &RouteEligibility,
    state: &mut RouteState,
    candidate: &mut Candidate,
) -> Result<()> {
    let RouteContext {
        source,
        options,
        default_floor,
        deadline,
        progress,
    } = *context;
    if eligibility.deft4j && default_floor == DefaultFloor::Complete && deadline.can_start_route() {
        let deft4j_step = progress.start("deft4j-derived source route");
        state.deft4j = build_deft4j_source_candidate(source, options, &mut deadline.hard_stop())?;
        deft4j_step.finish(context.report_optional(state.deft4j.as_ref()));
    }

    if let Some(seeded) = state.floor_seeded.take() {
        let seeded = close_losing_topology(context, candidate, seeded)?;
        candidate.replace_keeping_parent(seeded, options.exhaustive);
    }

    if let Some(mut deft4j) = state.deft4j.take() {
        deft4j.plans.clear();
        let deft4j = close_losing_topology(context, candidate, deft4j)?;
        candidate.replace_keeping_parent(deft4j, options.exhaustive);
    }
    Ok(())
}

/// Run the fragmented collection route (M11). A 4,096-token collection can
/// start slightly larger but reach a better layout after strict replays.
fn run_fragmented_collection(
    context: &RouteContext<'_>,
    state: &RouteState,
    candidate: &mut Candidate,
) -> Result<()> {
    let RouteContext {
        source,
        options,
        deadline,
        progress,
        ..
    } = *context;
    let run =
        options.exhaustive && !state.suppress_later_optional_routes && deadline.can_start_route();
    let step = run.then(|| progress.start("Columbo fragmented collection"));
    let mut fragmented = if run {
        fragmented_collect_seed(source.blocks, 0, options)
            .map(|plans| {
                let mut replay_options = options.clone();
                replay_options.exhaustive = false;
                build_candidate_from_plans(
                    source,
                    plans,
                    &replay_options,
                    MAX_RAW_REPLAY_LIMIT,
                    ReplayPlanner::Fragmented,
                    &mut deadline.hard_stop(),
                )
                .map(|candidate| candidate.named("Columbo fragmented collection"))
            })
            .transpose()?
    } else {
        None
    };
    if let Some(step) = step {
        step.finish(context.report_optional(fragmented.as_ref()));
    }
    if let Some(fragmented) = &mut fragmented {
        fragmented.plans.clear();
    }
    if let Some(fragmented) = fragmented {
        candidate.replace_keeping_parent(fragmented, options.exhaustive);
    }
    Ok(())
}

/// Run original-source Max (M12) and the rewritten-seed refinement (M13).
/// Returns a compact source-max parent whose split (M10d) is deferred.
fn run_max_source_routes(
    context: &RouteContext<'_>,
    eligibility: &RouteEligibility,
    state: &mut RouteState,
    candidate: &mut Candidate,
) -> Result<Option<Candidate>> {
    // Comparison and the replay seed need only the encoded bytes.
    candidate.plans.clear();
    let (deferred_split_parent, source_max_stabilized_incumbent) =
        merge_source_max(context, eligibility, state, candidate)?;
    candidate.plans.clear();
    refine_rewritten_seed(context, state, candidate, source_max_stabilized_incumbent)?;
    Ok(deferred_split_parent)
}

/// Compare the original-source Max lineage with the incumbent (M12). It runs
/// before a rewritten seed can spend the remaining time on one large merged
/// block. Returns the deferred split parent and whether source max proved the
/// incumbent's exact encoding stable.
fn merge_source_max(
    context: &RouteContext<'_>,
    eligibility: &RouteEligibility,
    state: &mut RouteState,
    candidate: &mut Candidate,
) -> Result<(Option<Candidate>, bool)> {
    let RouteContext {
        source,
        options,
        default_floor,
        deadline,
        progress,
    } = *context;
    let source_max = if let Some(max_candidate) = state.source_max.take() {
        Some(max_candidate)
    } else {
        let run_source_max = !state.suppress_later_source_max && deadline.can_start_route();
        if run_source_max {
            Some(build_source_max_candidate(
                source,
                options,
                progress,
                deadline,
                state.integrated_compact_source_max,
                &mut deadline.hard_stop(),
            )?)
        } else {
            None
        }
    };
    let Some(mut max_candidate) = source_max else {
        return Ok((None, false));
    };
    let replaced_parents = std::mem::take(&mut max_candidate.replaced_parents);
    // M10d: save a compact parent to split after the timed routes, so the
    // split cannot take their time.
    let mut deferred_split_parent = None;
    if default_floor.owns_terminal_stream_time()
        && max_candidate.data.len() <= COMPACT_SPLIT_FLOOR_MAX_COMPRESSED
        && !compact_split_parent_is_completed(
            &max_candidate,
            state.completed_compact_split_parent.as_deref(),
        )
    {
        let mut parent = max_candidate.clone();
        parent.plans.clear();
        parent.block_report = None;
        deferred_split_parent = Some(parent);
    }

    let max_candidate = close_source_max_topology(context, eligibility, candidate, max_candidate)?;
    let mut stabilized = candidate.is_encoding_stabilized_by(&max_candidate);
    candidate.replace_keeping_parent(max_candidate, options.exhaustive);

    // A deadline can stop source max after any replay and hand over that
    // parent, and a better parent can finish worse. Closing each parent the
    // replays replaced keeps a longer search from ending above a shorter one.
    // A winning parent is a new encoding, which no Max fixed point covers.
    for parent in replaced_parents {
        if !deadline.can_start_route() {
            break;
        }
        let parent = close_source_max_topology(context, eligibility, candidate, parent)?;
        if candidate.replace_if_smaller(parent) {
            stabilized = false;
        }
    }
    Ok((deferred_split_parent, stabilized))
}

/// Close one source-max candidate against the incumbent, after the compact
/// balanced-tree cleanup (M4): a smaller proven-feedback endpoint can hide the
/// balanced-tree win reachable from source max.
fn close_source_max_topology(
    context: &RouteContext<'_>,
    eligibility: &RouteEligibility,
    incumbent: &Candidate,
    mut max_candidate: Candidate,
) -> Result<Candidate> {
    let RouteContext {
        source,
        options,
        progress,
        ..
    } = *context;
    let (decoded_limit, identity) = (source.decoded_limit, source.identity);
    if eligibility.compact_tree {
        let tree_step = progress.start("Columbo source-max balanced-tree floor");
        let mut tree = refine_with_compact_balanced_tree_floor(
            &max_candidate,
            options,
            decoded_limit,
            identity,
        )?;
        if let Some(candidate) = tree.as_mut() {
            if let Some(feedback) =
                refine_with_compact_proven_feedback(candidate, options, decoded_limit, identity)?
            {
                candidate.replace_if_smaller(feedback);
            }
        }
        tree_step.finish(context.report_optional(tree.as_ref()));
        if let Some(tree) = tree {
            max_candidate.replace_if_smaller(tree);
        }
    }
    close_losing_topology(context, incumbent, max_candidate)
}

/// Run one Max pass from the selected rewritten seed (M13), whose match choices
/// and boundaries can expose further Max transformations.
fn refine_rewritten_seed(
    context: &RouteContext<'_>,
    state: &RouteState,
    candidate: &mut Candidate,
    source_max_stabilized_incumbent: bool,
) -> Result<()> {
    let RouteContext {
        source,
        options,
        deadline,
        progress,
        ..
    } = *context;
    let seed_selected = options.strict || candidate.is_strictly_smaller_than_source(source);
    let max_seed_is_stable = candidate.max_planner_is_stable || source_max_stabilized_incumbent;
    if seed_selected && max_seed_is_stable {
        progress.skipped(
            "Columbo rewritten-seed refinement",
            "exact max-planner fixed point already established",
        );
    }
    let run_seeded = seed_selected
        && !max_seed_is_stable
        && !state.suppress_later_optional_routes
        && deadline.can_start_route();
    if !run_seeded {
        return Ok(());
    }
    let step = progress.start("Columbo rewritten-seed refinement");
    let seeded = refine_with_max_planner(
        candidate,
        options,
        source.decoded_limit,
        source.identity,
        &mut deadline.hard_stop(),
    )?;
    step.finish(Some(context.report(&seeded)));
    candidate.replace_keeping_parent(seeded, options.exhaustive);
    Ok(())
}

/// Price the complementary no-split pruning policy (M2 late; see
/// [`super::CUMULATIVE_NO_SPLIT_MIN_SOURCE_BLOCKS`]) as its own topology.
fn run_complementary_no_split(
    context: &RouteContext<'_>,
    eligibility: &RouteEligibility,
    candidate: &mut Candidate,
) -> Result<()> {
    let RouteContext {
        source,
        options,
        default_floor,
        deadline,
        progress,
    } = *context;
    let run = options.exhaustive
        && default_floor.uses_bounded_png_routes()
        && eligibility.narrow_source
        && deadline.can_start_route();
    if !run {
        return Ok(());
    }
    let step = progress.start("Columbo complementary no-split route");
    let mut route_stop = deadline.hard_stop();
    let mut refinement_stop = deadline.hard_stop();
    let complementary = build_complementary_narrow_source_candidate(
        source,
        options,
        &mut route_stop,
        &mut refinement_stop,
    )?;
    let Some(contender) = complementary else {
        step.finish(None);
        return Ok(());
    };
    let contender = close_losing_topology(context, candidate, contender)?;
    step.finish(Some(context.report(&contender)));
    candidate.replace_keeping_parent(contender, options.exhaustive);
    Ok(())
}

/// Run the final balanced-tree cleanup (M4) on the selected candidate.
fn run_compact_balanced_tree(
    context: &RouteContext<'_>,
    eligibility: &RouteEligibility,
    candidate: &mut Candidate,
) -> Result<()> {
    if !(eligibility.compact_tree && context.deadline.can_start_route()) {
        return Ok(());
    }
    let tree_step = context
        .progress
        .start("Columbo compact balanced-tree floor");
    let tree = refine_with_compact_balanced_tree_floor(
        candidate,
        context.options,
        context.source.decoded_limit,
        context.source.identity,
    )?;
    tree_step.finish(context.report_optional(tree.as_ref()));
    if let Some(tree) = tree {
        candidate.replace_if_smaller(tree);
    }
    Ok(())
}

/// Price the split of the saved compact source-max parent (M10d) after every
/// ordinary timed route. Source max can end at a different endpoint from the
/// deft4j parent split earlier. The `_until` variant uses any remaining
/// allowance, or its deterministic one-block rescue after the hard stop.
fn run_deferred_source_max_split(
    context: &RouteContext<'_>,
    deadlines: &StreamDeadlines,
    parent: &Candidate,
    candidate: &mut Candidate,
) -> Result<()> {
    let deadline = context.deadline;
    let split_step = context
        .progress
        .start("Columbo source-max compact split floor");
    // After a primary-phase yield, allow only the cheap coarse rescue, so
    // exhaustive split pricing cannot spend the terminal share.
    let mut split_stop =
        if deadlines.reserve_terminal && deadline.expired() && deadlines.terminal.can_start_route()
        {
            SearchStop::always()
        } else {
            deadline.hard_stop()
        };
    let split = refine_with_terminal_source_split_floor_until(
        parent,
        context.options,
        context.source.decoded_limit,
        context.source.identity,
        &mut split_stop,
    )?;
    split_step.finish(context.report_optional(split.as_ref()));
    if let Some(split) = split {
        candidate.replace_if_smaller(split);
    }
    Ok(())
}

/// Apply the terminal tree floors (Max only), then the terminal methods, then
/// the tree floors once more (R18), then, for `ApngDefault`, R2–R5. Max then
/// closes with the tree floors each candidate the terminal sweeps replaced.
fn finish_terminal_searches(
    context: &RouteContext<'_>,
    deadlines: &StreamDeadlines,
    mut candidate: Candidate,
) -> Result<Candidate> {
    let RouteContext {
        source,
        options,
        progress,
        deadline,
        ..
    } = *context;
    // Default runs these floors inside `improve_default_floor_with_feedback`.
    if !options.exhaustive {
        return finish_closed_candidate(context, deadlines, candidate);
    }
    let terminal_work = DefaultFloorWork::Timed(deadlines.terminal_work());

    // Max applies them again to its final incumbent and, while routes may
    // start, to each distinct complete candidate it replaced: a deadline could
    // have handed any of those to the terminal searches, and a smaller
    // candidate can close larger. The terminal searches can also end lower
    // from a larger start, so the closed incumbent is finished first and the
    // smallest closed parent that beats it is finished too, as time allows,
    // keeping the smaller endpoint.
    let mut parents: Vec<Candidate> = Vec::new();
    for parent in std::mem::take(&mut candidate.replaced_parents) {
        if parent.data != candidate.data && parents.iter().all(|kept| kept.data != parent.data) {
            parents.push(parent);
        }
    }
    let candidate =
        improve_with_terminal_tree_floors(source, options, terminal_work, progress, candidate)?;
    let mut best_parent: Option<Candidate> = None;
    for parent in parents {
        if !deadline.can_start_route() {
            break;
        }
        let parent =
            improve_with_terminal_tree_floors(source, options, terminal_work, progress, parent)?;
        if parent.is_strictly_smaller_than(best_parent.as_ref().unwrap_or(&candidate)) {
            best_parent = Some(parent);
        }
    }
    let mut finished = finish_closed_candidate(context, deadlines, candidate)?;
    let mut swept = std::mem::take(&mut finished.replaced_parents);
    if let Some(parent) = best_parent.filter(|_| deadline.can_start_route()) {
        let mut parent = finish_closed_candidate(context, deadlines, parent)?;
        swept.append(&mut parent.replaced_parents);
        finished.replace_if_smaller(parent);
    }
    // The terminal sweeps keep each candidate a method that needs a route
    // start replaced: a deadline just before that method would have handed it
    // to the finishing steps, which end with the tree floors, and those are
    // not monotone in their starting size. Both endpoints are complete, so
    // closing these with the floors, as linear finalization until the hard
    // stop, can only make the result smaller; a tie keeps the endpoint.
    let mut distinct: Vec<Candidate> = Vec::new();
    for replaced in swept {
        if replaced.data != finished.data && distinct.iter().all(|kept| kept.data != replaced.data)
        {
            distinct.push(replaced);
        }
    }
    for replaced in distinct {
        if !terminal_work.can_finalize() {
            break;
        }
        let replaced =
            improve_with_terminal_tree_floors(source, options, terminal_work, progress, replaced)?;
        finished.replace_if_smaller(replaced);
    }
    finished.replaced_parents.clear();
    Ok(finished)
}

/// The terminal methods, then the tree floors once more (R18), then, for
/// `ApngDefault`, R2–R5.
fn finish_closed_candidate(
    context: &RouteContext<'_>,
    deadlines: &StreamDeadlines,
    candidate: Candidate,
) -> Result<Candidate> {
    let RouteContext {
        source,
        options,
        default_floor,
        progress,
        ..
    } = *context;
    let terminal_work = DefaultFloorWork::Timed(deadlines.terminal_work());
    let restoration_work = if default_floor == DefaultFloor::MandatoryComplete {
        DefaultFloorWork::Mandatory
    } else {
        terminal_work
    };
    let header_fit = if !options.exhaustive && default_floor == DefaultFloor::ApngDefault {
        HeaderFit::AfterTreeFloors
    } else {
        HeaderFit::InSweep
    };
    // Max keeps what the sweeps replace for `finish_terminal_searches`.
    let candidate = improve_with_terminal_searches(
        source,
        options,
        restoration_work,
        terminal_work,
        progress,
        header_fit,
        options.exhaustive,
        candidate,
    )?;
    // R18: the blocks R13–R17 cut, merged or re-spelled have not had the
    // bounded-depth and compact tree families, nor have APNG Default frames,
    // which skip the first floors.
    let candidate =
        improve_with_terminal_tree_floors(source, options, terminal_work, progress, candidate)?;
    if header_fit == HeaderFit::InSweep {
        return Ok(candidate);
    }
    improve_with_final_header_fit(
        source,
        options,
        restoration_work,
        progress,
        &mut TerminalParseCache::default(),
        candidate,
    )
}

#[cfg(test)]
mod tests;
