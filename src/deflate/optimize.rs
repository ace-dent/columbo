// SPDX-License-Identifier: MIT

use std::sync::atomic::{AtomicU8, Ordering};
use std::thread;
use std::time::Instant;

use crate::progress::{
    BlockEncoding, BlockProgress, BlockReport, CandidateProgress, Progress, RouteProgress,
    SameDistanceProgress, StreamProgress, MAX_REPORTED_BLOCKS,
};
use crate::{Error, Options, Result};

use super::bitstream::BitWriter;
use super::block::emit_block;
use super::deft4j::plan_source_blocks;
use super::header::plan_columbo_quad_lengthen_candidate;
use super::model::{
    ParsedBlock, ParsedStream, PlannedBlock, Representation, SourceBlockType, Token,
};
use super::parse::{parse_stream, parsed_model_bytes};
use super::search::{rewrite_258_symbols, same_distance_opportunities};
use super::stream::{
    fragmented_collect_seed, plan_columbo_floor_seeded_bounded_grouping,
    plan_compact_source_split_floor, plan_fragmented_replay, plan_source_no_split_route,
    plan_stream, plan_stream_with_progress, plan_terminal_merge_route,
};

// Long source-block chains can need one pass to establish profitable adjacent
// groups, then two inexpensive passes over that much simpler block layout to
// settle their boundaries and tables. Every round below must strictly improve
// the complete stream, so the extra slot cannot oscillate or grow the output.
const DEFAULT_RAW_REPLAY_LIMIT: usize = 3;
const MAX_RAW_REPLAY_LIMIT: usize = 8;
// A tiny single Huffman block can still benefit from the source-ordered deft4j
// candidate graph: its selected token spelling can expose a second win to
// ordinary refinement. Keep that extra route within the same 4 KiB
// compact-stream band used by the original Columbo C implementation. On larger
// one-block streams it can consume the deadline before broader routes that
// already have stronger incumbents. These thresholds are Columbo scheduling
// policy, not limits in deft4j itself.
const DEFT4J_SINGLE_BLOCK_MAX_COMPRESSED: usize = 4_096;
const DEFT4J_MULTI_BLOCK_MIN_HUFFMAN_BLOCKS: usize = 2;
const DEFT4J_SOURCE_LIST_MAX_BLOCKS: usize = 128;
const WEAK_DEFT4J_GAIN_BASIS_POINTS: u64 = 200;
const NARROW_SOURCE_MIN_COMPRESSED: usize = 32 * 1_024;
const NARROW_SOURCE_MAX_COMPRESSED: usize = 512 * 1_024;
const TERMINAL_MERGE_MIN_COMPRESSED: usize = 100_001;
const LEGACY_GROUPING_MIN_SOURCE_BLOCKS: usize = 13;
const LEGACY_GROUPING_MAX_SOURCE_BLOCKS: usize = 32;
// A completed compact deft4j-derived seed may expose useful eighth-position
// child splits after the timed route has settled its tokens and source joins.
// Keep this deterministic Columbo floor tightly bounded: it finishes at most
// seven structural prices per block and never starts another token search.
const COMPACT_SPLIT_FLOOR_MAX_COMPRESSED: usize = 16 * 1024;
const COMPACT_SPLIT_FLOOR_MAX_DECODED: u64 = 128 * 1024;
const COMPACT_SPLIT_FLOOR_MAX_BLOCKS: usize = 4;
const COMPACT_SPLIT_FLOOR_MAX_TOKENS: usize = 16 * 1024;
// The original Columbo C quad-lengthening move fills the one-block gap between
// the tiny deft4j route and larger generic max work. Keep its post-route trial
// band narrow so normal mode and broader streams pay no finished-tree cost.
const COMPACT_QUAD_MIN_COMPRESSED: usize = 4_097;
const COMPACT_QUAD_MAX_COMPRESSED: usize = 8 * 1_024;
const COMPACT_QUAD_MAX_DECODED: u64 = 128 * 1_024;
const COMPACT_QUAD_MIN_TOKENS: usize = 701;
const COMPACT_QUAD_MAX_TOKENS: usize = 4_096;
// Parallel routes shorten a container's wall-clock search without making its
// peak memory proportional to every individually valid route budget. Larger
// streams retain the same candidates, but evaluate them serially.
const PARALLEL_ROUTE_MAX_COMPRESSED: usize = 8 * 1_024 * 1_024;
const PARALLEL_ROUTE_MAX_DECODED: u64 = 64 * 1_024 * 1_024;
const PARALLEL_ROUTE_MAX_MODEL: usize = 64 * 1_024 * 1_024;

/// Facts collected while decoding the source stream. Container handlers use
/// these values to validate their checksums without inflating a second time.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct RawInfo {
    pub(crate) crc32: u32,
    pub(crate) adler32: u32,
    pub(crate) size: u64,
    pub(crate) max_distance: u16,
    pub(crate) deflate_bits: u64,
    pub(crate) source_block_count: usize,
    pub(crate) source_empty_block_count: usize,
}

pub(crate) struct RawOptimization {
    pub(crate) data: Vec<u8>,
    pub(crate) consumed: usize,
    pub(crate) info: RawInfo,
    pub(crate) timed_out: bool,
}

/// Decide whether max mode must finish its ordinary-mode comparison floor.
///
/// A standalone stream uses [`DefaultFloor::Complete`] so `--max` cannot lose
/// an optimization found by normal mode. A single scheduled PNG image uses
/// [`DefaultFloor::CompleteThenBounded`]: it establishes that same incumbent,
/// then spends only the remaining file budget on PNG's bounded max routes.
/// Multi-stream containers use [`DefaultFloor::Shared`], which also prevents a
/// tiny single-block deft4j route from consuming time needed by later members.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DefaultFloor {
    Complete,
    CompleteThenBounded,
    Shared,
}

impl DefaultFloor {
    fn is_bounded(self) -> bool {
        !matches!(self, Self::Complete)
    }

    fn uses_bounded_png_routes(self) -> bool {
        matches!(self, Self::CompleteThenBounded)
    }

    fn allows_single_block_deft4j(self) -> bool {
        !matches!(self, Self::Shared)
    }
}

pub(crate) fn optimize_raw(input: &[u8], options: &Options) -> Result<RawOptimization> {
    let optimized = optimize_raw_prefix(input, options, options.max_decoded_bytes)?;
    if optimized.consumed != input.len() {
        return Err(Error::new("trailing data after Deflate stream"));
    }
    Ok(optimized)
}

/// Canonicalize Defluff's non-standard length-258 spelling before planning.
///
/// Merely disabling the relaxed rewrite candidate is insufficient: an alias
/// already present in the input could otherwise survive through exact source
/// reuse or an ordinary header rewrite. Clearing source representations makes
/// every strict candidate encode the canonical symbol 285 token.
fn normalize_258_aliases(blocks: &mut [ParsedBlock]) -> Result<usize> {
    let mut normalized_blocks = 0_usize;
    for block in blocks {
        let has_alias = block.tokens.iter().any(|token| {
            matches!(
                token,
                Token::Match {
                    length: 258,
                    length_symbol: 284,
                    ..
                }
            )
        });
        if !has_alias {
            continue;
        }

        normalized_blocks += 1;
        let tokens = rewrite_258_symbols(&block.tokens, block.plain.len(), false)
            .ok_or_else(|| Error::new("could not allocate strict Deflate token normalization"))?;
        block.tokens = tokens.into();
        block.recount_frequencies();
        block.original = None;
        block.original_literal_lengths = None;
        block.original_distance_lengths = None;
        block.original_dynamic = None;
    }
    Ok(normalized_blocks)
}

/// Optimize the first complete Deflate stream in `input`.
///
/// Prefix decoding is essential for concatenated GZIP members, whose trailer
/// immediately follows a non-byte-length-delimited raw Deflate stream.
pub(crate) fn optimize_raw_prefix(
    input: &[u8],
    options: &Options,
    decoded_limit: u64,
) -> Result<RawOptimization> {
    optimize_raw_prefix_with_floor(input, options, decoded_limit, DefaultFloor::Complete)
}

/// Optimize one raw stream with an explicit max-mode default-floor policy.
///
/// This is kept crate-private because the distinction belongs to container
/// scheduling, not to Columbo's public optimization options.
pub(crate) fn optimize_raw_prefix_with_floor(
    input: &[u8],
    options: &Options,
    decoded_limit: u64,
    default_floor: DefaultFloor,
) -> Result<RawOptimization> {
    if input.len() as u64 > options.max_input_bytes {
        return Err(Error::new("input exceeds configured safety limit"));
    }

    let started = Instant::now();
    let parsed = parse_stream(input, decoded_limit.min(options.max_decoded_bytes))?;
    // Avoid even an extra clock read in ordinary speed-first runs.
    let parse_elapsed = if options.verbose {
        started.elapsed()
    } else {
        std::time::Duration::ZERO
    };
    let mut blocks = parsed.blocks;
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
    if options.strict {
        let normalization_started = progress.enabled().then(Instant::now);
        let normalized_blocks = normalize_258_aliases(&mut blocks)?;
        if let Some(normalization_started) = normalization_started {
            progress.normalization(normalized_blocks, normalization_started.elapsed());
        }
    }
    if progress.enabled() {
        let opportunities = same_distance_opportunities(&blocks);
        progress.same_distance_opportunities(SameDistanceProgress {
            runs: opportunities.runs,
            matches: opportunities.matches,
            decoded_bytes: opportunities.decoded_bytes,
            coalescible_runs: opportunities.coalescible_runs,
            repartition_runs: opportunities.repartition_runs,
            tokens_removable: opportunities.tokens_removable,
        });
    }
    progress.routes();
    let deadline = Deadline {
        started,
        duration: options.timeout,
        state: AtomicU8::new(0),
    };

    // Prefix callers need the exact bytes occupied by the first stream. Any
    // unused high bits in its final byte belong to that stream's byte-level
    // representation and are retained when the source wins.
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
    // A standalone PNG must retain the genuine normal-mode result while still
    // allowing the historical bounded max routes to use the remaining file
    // budget. Establishing this incumbent first keeps the complete-floor
    // guarantee without letting that floor starve all max-only work.
    let guaranteed_floor_step = (options.exhaustive
        && default_floor == DefaultFloor::CompleteThenBounded)
        .then(|| progress.start("Normal comparison floor"));
    let mut guaranteed_floor_candidate =
        if options.exhaustive && default_floor == DefaultFloor::CompleteThenBounded {
            let mut floor_options = options.clone();
            floor_options.exhaustive = false;
            let mut never_expires = || false;
            Some(build_candidate(
                source,
                &floor_options,
                DEFAULT_RAW_REPLAY_LIMIT,
                &mut never_expires,
            )?)
        } else {
            None
        };
    if let Some(step) = guaranteed_floor_step {
        step.finish(guaranteed_floor_candidate.as_ref().map(|candidate| {
            candidate_progress(
                candidate,
                source.meaningful_bits,
                candidate.is_strictly_smaller_than_source(source),
            )
        }));
    }
    let compact_quad_eligible = options.exhaustive
        && default_floor.uses_bounded_png_routes()
        && compact_quad_source_eligible(original.len(), parsed.decoded_size, &blocks);
    let deft4j_eligible = options.exhaustive
        && deft4j_source_route_eligible(
            &blocks,
            original.len(),
            default_floor.allows_single_block_deft4j(),
        );

    // Bounded PNG routes normally share the parsed stream. The audited legacy
    // block-count band instead follows its floor with one deterministic
    // Columbo grouping, avoiding duplicate work that cannot finish before the
    // same deadline. Generic-only streams run source max beside their full
    // floor lineage so current gains remain selectable. Complete standalone
    // floors keep their unbounded order.
    let run_narrow_source = options.exhaustive
        && default_floor.uses_bounded_png_routes()
        && narrow_source_route_eligible(&blocks, original.len());
    let parallel_routes =
        options.exhaustive && default_floor.is_bounded() && parallel_route_is_bounded(source);
    let png_policy = if default_floor.uses_bounded_png_routes() && parallel_routes {
        bounded_png_max_policy(
            blocks
                .iter()
                .filter(|block| !block.plain.is_empty())
                .count(),
            parsed.source_empty_block_count,
            parsed.source_trailing_empty_block_count,
            deft4j_eligible,
            run_narrow_source,
        )
    } else {
        BoundedPngMaxPolicy::default()
    };
    let bounded_step = (options.exhaustive && default_floor.is_bounded())
        .then(|| progress.start("Bounded comparison routes"));
    let bounded_candidates = if options.exhaustive && default_floor.is_bounded() {
        // A single-image PNG has already finished this exact ordinary-mode
        // floor above. Continue that candidate's retained plans instead of
        // rebuilding them inside the bounded phase. Apart from avoiding
        // duplicate work, this gives the max-only descendants the same wall
        // time that historical bounded PNG scheduling made available.
        let completed_floor = guaranteed_floor_candidate.take();
        match png_policy {
            BoundedPngMaxPolicy::GenericParallel => build_bounded_generic_max_candidates(
                source,
                options,
                &deadline,
                progress,
                completed_floor,
            )?,
            BoundedPngMaxPolicy::LegacyGrouping => {
                let (floor, floor_seeded) = build_bounded_floor_grouping_lineage(
                    source,
                    options,
                    &deadline,
                    completed_floor,
                )?;
                let suppress_later_source_max = floor_seeded.is_some();
                BoundedPhaseCandidates {
                    floor: Some(floor),
                    floor_seeded,
                    suppress_later_source_max,
                    suppress_later_optional_routes: suppress_later_source_max,
                    ..BoundedPhaseCandidates::default()
                }
            }
            BoundedPngMaxPolicy::Standard | BoundedPngMaxPolicy::CoarseExpansion => {
                let run_deft4j = deft4j_eligible && !deadline.expired();
                build_bounded_phase_candidates(
                    source,
                    options,
                    png_policy == BoundedPngMaxPolicy::CoarseExpansion,
                    run_deft4j,
                    run_narrow_source,
                    parallel_routes,
                    &deadline,
                    completed_floor,
                )?
            }
        }
    } else {
        BoundedPhaseCandidates::default()
    };
    let mut bounded_floor_candidate = bounded_candidates.floor;
    let mut floor_seeded_candidate = bounded_candidates.floor_seeded;
    let mut deft4j_candidate = bounded_candidates.deft4j;
    let mut narrow_candidate = bounded_candidates.narrow;
    let mut source_max_candidate = bounded_candidates.source_max;
    let suppress_later_source_max = bounded_candidates.suppress_later_source_max;
    let suppress_later_optional_routes = bounded_candidates.suppress_later_optional_routes;
    if let Some(step) = bounded_step {
        step.finish_phase();
        for (name, candidate) in [
            ("Normal floor", bounded_floor_candidate.as_ref()),
            ("Columbo floor-seeded", floor_seeded_candidate.as_ref()),
            ("deft4j-derived source", deft4j_candidate.as_ref()),
            ("No-split source", narrow_candidate.as_ref()),
            ("Columbo source max", source_max_candidate.as_ref()),
        ] {
            if let Some(candidate) = candidate {
                progress.candidate(
                    name,
                    candidate_progress(
                        candidate,
                        source.meaningful_bits,
                        candidate.is_strictly_smaller_than_source(source),
                    ),
                );
            }
        }
    }
    let seed_weak_deft4j = default_floor.uses_bounded_png_routes()
        && deft4j_candidate.as_ref().is_some_and(|deft4j| {
            has_multiple_nonempty_blocks(&blocks)
                && deft4j_gain_is_below(
                    parsed.meaningful_bits,
                    deft4j.bits,
                    WEAK_DEFT4J_GAIN_BASIS_POINTS,
                )
        });
    if let Some(floor) = &mut bounded_floor_candidate {
        // The later deft4j refinement needs only the encoded floor for its
        // strict comparison. Release transformed floor plans before it
        // reparses another complete candidate.
        floor.plans.clear();
    }
    if let Some(seeded) = &mut floor_seeded_candidate {
        seeded.plans.clear();
    }
    if let Some(deft4j) = &mut deft4j_candidate {
        // Refinement needs only the encoded stream. Release retained
        // deft4j-route token plans before reparsing it so the models do not
        // overlap at peak memory.
        deft4j.plans.clear();
    }
    if let Some(narrow) = &mut narrow_candidate {
        narrow.plans.clear();
    }
    if let Some(source_max) = &mut source_max_candidate {
        source_max.plans.clear();
    }
    let run_bounded_refinement = default_floor.is_bounded() && !deadline.expired();
    let refinement_step = (run_bounded_refinement
        && (seed_weak_deft4j || deft4j_candidate.is_some()))
    .then(|| progress.start("deft4j-derived refinement"));
    if run_bounded_refinement {
        if seed_weak_deft4j {
            if let Some(floor) = bounded_floor_candidate
                .as_ref()
                .filter(|floor| floor.is_strictly_smaller_than_source(source))
            {
                if let Some(mut seeded) = build_deft4j_seed_candidate(
                    floor,
                    options,
                    decoded_limit,
                    identity,
                    default_floor.allows_single_block_deft4j(),
                    &mut || deadline.expired(),
                )? {
                    if !deadline.expired() {
                        let refined = refine_with_default_planner(
                            &seeded,
                            options,
                            decoded_limit,
                            identity,
                            &mut || deadline.expired(),
                        )?;
                        seeded.replace_if_smaller(refined);
                    }
                    let narrow_already_wins = narrow_candidate
                        .as_ref()
                        .is_some_and(|narrow| narrow.is_strictly_smaller_than(&seeded));
                    if !narrow_already_wins
                        && !deadline.expired()
                        && (TERMINAL_MERGE_MIN_COMPRESSED..=NARROW_SOURCE_MAX_COMPRESSED)
                            .contains(&seeded.data.len())
                    {
                        if let Some(terminal) = refine_with_terminal_merge(
                            &seeded,
                            options,
                            decoded_limit,
                            identity,
                            &mut || deadline.expired(),
                        )? {
                            seeded.replace_if_smaller(terminal);
                        }
                    }
                    replace_optional_if_smaller(&mut deft4j_candidate, seeded);
                }
            }
        } else if let Some(deft4j) = deft4j_candidate.as_ref() {
            let refined =
                refine_with_default_planner(deft4j, options, decoded_limit, identity, &mut || {
                    deadline.expired()
                })?;
            replace_optional_if_smaller(&mut deft4j_candidate, refined);
        }
    }
    if let Some(step) = refinement_step {
        step.finish(deft4j_candidate.as_ref().map(|candidate| {
            candidate_progress(
                candidate,
                source.meaningful_bits,
                candidate.is_strictly_smaller_than_source(source),
            )
        }));
    }
    if default_floor.uses_bounded_png_routes() && seed_weak_deft4j {
        let compact_split_step = progress.start("Columbo compact split floor");
        let compact_split = match deft4j_candidate.as_ref() {
            Some(deft4j) => {
                refine_with_compact_source_split_floor(deft4j, options, decoded_limit, identity)?
            }
            None => None,
        };
        compact_split_step.finish(compact_split.as_ref().map(|candidate| {
            candidate_progress(
                candidate,
                source.meaningful_bits,
                candidate.is_strictly_smaller_than_source(source),
            )
        }));
        if let Some(split) = compact_split {
            replace_optional_if_smaller(&mut deft4j_candidate, split);
        }
    }
    if let Some(deft4j) = &mut deft4j_candidate {
        // Keep only the encoded incumbent for comparison and later routes;
        // refinement can otherwise retain another expanded token graph.
        deft4j.plans.clear();
    }
    if let Some(narrow) = narrow_candidate {
        replace_optional_if_smaller(&mut deft4j_candidate, narrow);
    }

    let needs_initial_floor = bounded_floor_candidate.is_none();
    let initial_step = needs_initial_floor.then(|| {
        progress.start(if options.exhaustive {
            "Normal comparison floor"
        } else {
            "Normal route"
        })
    });
    let mut candidate = if let Some(floor) = bounded_floor_candidate {
        floor
    } else if options.exhaustive {
        // Max mode starts from the genuine normal-mode result. Standalone
        // streams finish that floor so --max cannot lose to normal mode;
        // locally scheduled container streams keep it within their allotted
        // time. Either way, elapsed floor time advances the same deadline.
        let mut floor_options = options.clone();
        floor_options.exhaustive = false;
        match default_floor {
            DefaultFloor::Complete => {
                let mut never_expires = || false;
                build_candidate(
                    source,
                    &floor_options,
                    DEFAULT_RAW_REPLAY_LIMIT,
                    &mut never_expires,
                )?
            }
            DefaultFloor::CompleteThenBounded | DefaultFloor::Shared => {
                build_bounded_floor_candidate(source, options, &mut || deadline.expired())?
            }
        }
    } else {
        build_candidate(source, options, DEFAULT_RAW_REPLAY_LIMIT, &mut || {
            deadline.expired()
        })?
    };
    if let Some(step) = initial_step {
        step.finish(Some(candidate_progress(
            &candidate,
            source.meaningful_bits,
            candidate.is_strictly_smaller_than_source(source),
        )));
    }
    // The complete standalone PNG floor is consumed by the bounded phase and
    // returned as `bounded_floor_candidate`; other policies never create it.
    debug_assert!(guaranteed_floor_candidate.is_none());
    if deft4j_eligible && default_floor == DefaultFloor::Complete && !deadline.expired() {
        let deft4j_step = progress.start("deft4j-derived source route");
        deft4j_candidate =
            build_deft4j_source_candidate(source, options, &mut || deadline.expired())?;
        deft4j_step.finish(deft4j_candidate.as_ref().map(|deft4j| {
            candidate_progress(
                deft4j,
                source.meaningful_bits,
                deft4j.is_strictly_smaller_than_source(source),
            )
        }));
    }

    if let Some(seeded) = floor_seeded_candidate {
        candidate.replace_if_smaller(seeded);
    }

    if let Some(deft4j) = &mut deft4j_candidate {
        deft4j.plans.clear();
    }
    let mut deft4j_lineage = false;
    if let Some(deft4j) = deft4j_candidate {
        deft4j_lineage = candidate.replace_if_smaller(deft4j);
    }

    // A 4,096-token collection can start slightly larger but converge to a
    // better fragmented-stream layout after strict replays. It is additive to
    // the normal-mode comparison floor above: a short container slice must not
    // spend its whole budget here before securing that floor. The collection
    // seed itself remains deadline-independent and can still win when there is
    // no time left for its optional replay rounds. Its 64-source-block minimum
    // also keeps it outside the audited 13-to-32-block legacy corpus band.
    let run_fragmented = options.exhaustive
        && !suppress_later_optional_routes
        // Once the source-ordered deft4j route has won and spent the deadline,
        // another source-derived seed cannot receive any replay work. Avoid
        // that duplicate post-deadline collection.
        && (!deft4j_lineage || !deadline.expired());
    let fragmented_step = run_fragmented.then(|| progress.start("Columbo fragmented collection"));
    let mut fragmented_candidate = if run_fragmented {
        fragmented_collect_seed(&blocks, 0, options)
            .map(|plans| {
                let mut replay_options = options.clone();
                replay_options.exhaustive = false;
                build_candidate_from_plans(
                    source,
                    plans,
                    &replay_options,
                    MAX_RAW_REPLAY_LIMIT,
                    ReplayPlanner::Fragmented,
                    &mut || deadline.expired(),
                )
                .map(|candidate| candidate.named("Columbo fragmented collection"))
            })
            .transpose()?
    } else {
        None
    };
    if let Some(step) = fragmented_step {
        step.finish(fragmented_candidate.as_ref().map(|fragmented| {
            candidate_progress(
                fragmented,
                source.meaningful_bits,
                fragmented.is_strictly_smaller_than_source(source),
            )
        }));
    }
    if let Some(fragmented) = &mut fragmented_candidate {
        fragmented.plans.clear();
    }

    if let Some(fragmented) = fragmented_candidate {
        candidate.replace_if_smaller(fragmented);
    }

    if options.exhaustive {
        // The encoded floor is all we need for comparison. Releasing its
        // copied tokens before max search keeps peak memory predictable.
        candidate.plans.clear();
        let mut source_max_stabilized_incumbent = false;

        // Generic max also explores source-boundary and table families outside
        // deft4j's source-ordered graph. Run it before a rewritten seed can
        // spend the remainder on one large merged block.
        if let Some(max_candidate) = source_max_candidate {
            source_max_stabilized_incumbent = candidate.is_encoding_stabilized_by(&max_candidate);
            candidate.replace_if_smaller(max_candidate);
        } else {
            let run_source_max = !suppress_later_source_max && !deadline.expired();
            if run_source_max {
                let max_candidate =
                    build_source_max_candidate(source, options, progress, &deadline, &mut || {
                        deadline.expired()
                    })?;
                source_max_stabilized_incumbent =
                    candidate.is_encoding_stabilized_by(&max_candidate);
                candidate.replace_if_smaller(max_candidate);
            }
        }

        // A winning source restart can own another full plan graph. Only its
        // encoded bytes are needed as the optional replay seed.
        candidate.plans.clear();

        let seed_selected = options.strict || candidate.is_strictly_smaller_than_source(source);
        // `build_candidate` has already run the same max planner on every
        // accepted rewrite until it either stopped improving or hit its replay
        // cap. If source max proved these exact bytes stable, reparsing them
        // here can only repeat that final no-improvement round.
        let max_seed_is_stable = candidate.max_planner_is_stable || source_max_stabilized_incumbent;
        if seed_selected && max_seed_is_stable {
            progress.skipped(
                "Columbo rewritten-seed refinement",
                "exact max-planner fixed point already established",
            );
        }
        let run_seeded = seed_selected
            && !max_seed_is_stable
            && !suppress_later_optional_routes
            && !deadline.expired();
        let seeded_step = run_seeded.then(|| progress.start("Columbo rewritten-seed refinement"));
        if run_seeded {
            // Rewritten match choices and boundaries can expose later max
            // transformations, so retain one additive seeded pass after both
            // source-shaped routes. Its incumbent remains available if this
            // final route times out or fails to improve it.
            let seeded_candidate =
                refine_with_max_planner(&candidate, options, decoded_limit, identity, &mut || {
                    deadline.expired()
                })?;
            if let Some(step) = seeded_step {
                step.finish(Some(candidate_progress(
                    &seeded_candidate,
                    source.meaningful_bits,
                    seeded_candidate.is_strictly_smaller_than_source(source),
                )));
            }
            candidate.replace_if_smaller(seeded_candidate);
        }
    }

    if compact_quad_eligible {
        let quad_step = progress.start("Columbo compact quad floor");
        let quad = refine_with_compact_quad_floor(&candidate, options, decoded_limit, identity)?;
        quad_step.finish(quad.as_ref().map(|quad| {
            candidate_progress(
                quad,
                source.meaningful_bits,
                quad.is_strictly_smaller_than_source(source),
            )
        }));
        if let Some(quad) = quad {
            candidate.replace_if_smaller(quad);
        }
    }

    let keep_original = !options.strict && !candidate.is_strictly_smaller_than_source(source);
    let deflate_bits = if keep_original {
        parsed.meaningful_bits
    } else {
        candidate.bits
    };
    let final_report = if keep_original {
        capture_source_block_report(&blocks, parsed.source_block_count, options.verbose)
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
    let timed_out = deadline.was_triggered();
    progress.blocks(final_report);
    progress.finish(
        selected_route,
        output_bytes,
        deflate_bits,
        parsed.meaningful_bits,
        timed_out,
    );

    // Planning is complete. Drop model storage before copying a winning source
    // stream, then reuse the generated output allocation where possible. This
    // keeps the no-growth guarantee from briefly requiring two source-sized
    // outputs plus the full parsed model.
    drop(blocks);
    drop(std::mem::take(&mut candidate.plans));
    if keep_original {
        candidate.data.clear();
        candidate
            .data
            .try_reserve_exact(original.len())
            .map_err(|_| Error::new("could not allocate Deflate output"))?;
        candidate.data.extend_from_slice(original);
    }
    let data = candidate.data;

    Ok(RawOptimization {
        data,
        consumed: parsed.consumed,
        info: RawInfo {
            crc32: parsed.crc32,
            adler32: parsed.adler32,
            size: parsed.decoded_size,
            max_distance: parsed.max_distance,
            deflate_bits,
            source_block_count: parsed.source_block_count,
            source_empty_block_count: parsed.source_empty_block_count,
        },
        timed_out,
    })
}

fn deft4j_source_route_eligible(
    blocks: &[ParsedBlock],
    compressed_len: usize,
    allow_single_block: bool,
) -> bool {
    let mut nonempty = 0_usize;
    let mut huffman = 0_usize;
    for block in blocks.iter().filter(|block| !block.plain.is_empty()) {
        nonempty += 1;
        if matches!(
            block.source_type,
            SourceBlockType::Fixed | SourceBlockType::Dynamic
        ) {
            huffman += 1;
        }
    }
    match nonempty {
        1 => {
            allow_single_block
                && huffman == 1
                && compressed_len <= DEFT4J_SINGLE_BLOCK_MAX_COMPRESSED
        }
        2..=DEFT4J_SOURCE_LIST_MAX_BLOCKS => huffman >= DEFT4J_MULTI_BLOCK_MIN_HUFFMAN_BLOCKS,
        _ => false,
    }
}

fn narrow_source_route_eligible(blocks: &[ParsedBlock], compressed_len: usize) -> bool {
    (NARROW_SOURCE_MIN_COMPRESSED..=NARROW_SOURCE_MAX_COMPRESSED).contains(&compressed_len)
        && (2..=DEFT4J_SOURCE_LIST_MAX_BLOCKS).contains(
            &blocks
                .iter()
                .filter(|block| !block.plain.is_empty())
                .count(),
        )
        && blocks.iter().all(|block| {
            block.plain.is_empty()
                || matches!(
                    block.source_type,
                    SourceBlockType::Fixed | SourceBlockType::Dynamic
                )
        })
}

fn compact_quad_source_eligible(
    compressed_len: usize,
    decoded_size: u64,
    blocks: &[ParsedBlock],
) -> bool {
    if !(COMPACT_QUAD_MIN_COMPRESSED..=COMPACT_QUAD_MAX_COMPRESSED).contains(&compressed_len)
        || decoded_size > COMPACT_QUAD_MAX_DECODED
    {
        return false;
    }
    let mut nonempty = blocks.iter().filter(|block| !block.plain.is_empty());
    let Some(block) = nonempty.next() else {
        return false;
    };
    nonempty.next().is_none()
        && block.source_type == SourceBlockType::Dynamic
        && (COMPACT_QUAD_MIN_TOKENS..=COMPACT_QUAD_MAX_TOKENS).contains(&block.tokens.len())
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
enum BoundedPngMaxPolicy {
    #[default]
    Standard,
    CoarseExpansion,
    LegacyGrouping,
    GenericParallel,
}

/// Choose the bounded PNG route family from the audited source topology.
///
/// Generic streams have no specialized source sibling, so source max remains
/// available even when their block count overlaps the legacy band.
fn bounded_png_max_policy(
    nonempty_blocks: usize,
    source_empty_blocks: usize,
    source_trailing_empty_blocks: usize,
    deft4j_eligible: bool,
    narrow_eligible: bool,
) -> BoundedPngMaxPolicy {
    let has_trailing_empty_pair_only =
        source_empty_blocks == 2 && source_trailing_empty_blocks == 2;
    if !deft4j_eligible && !narrow_eligible {
        BoundedPngMaxPolicy::GenericParallel
    } else if (LEGACY_GROUPING_MIN_SOURCE_BLOCKS..=LEGACY_GROUPING_MAX_SOURCE_BLOCKS)
        .contains(&nonempty_blocks)
        && !has_trailing_empty_pair_only
    {
        BoundedPngMaxPolicy::LegacyGrouping
    } else if (2..=4).contains(&nonempty_blocks) {
        BoundedPngMaxPolicy::CoarseExpansion
    } else {
        BoundedPngMaxPolicy::Standard
    }
}

fn has_multiple_nonempty_blocks(blocks: &[ParsedBlock]) -> bool {
    blocks
        .iter()
        .filter(|block| !block.plain.is_empty())
        .take(2)
        .count()
        == 2
}

fn deft4j_gain_is_below(source_bits: u64, candidate_bits: u64, basis_points: u64) -> bool {
    let saved = source_bits.saturating_sub(candidate_bits);
    saved.saturating_mul(10_000) < source_bits.saturating_mul(basis_points)
}

#[derive(Clone, Copy)]
struct StreamIdentity {
    decoded_size: u64,
    crc32: u32,
    adler32: u32,
}

struct Candidate {
    data: Vec<u8>,
    bits: u64,
    plans: Vec<PlannedBlock>,
    block_report: Option<BlockReport>,
    route: &'static str,
    /// The exhaustive full-stream planner reparsed these exact bytes and
    /// reached a strict fixed point without exhausting its search allowance.
    ///
    /// This is deliberately attached to the encoded candidate rather than to
    /// a route name: an earlier route may have emitted byte-for-byte identical
    /// output and won the stable tie order.
    max_planner_is_stable: bool,
}

impl Candidate {
    fn named(mut self, route: &'static str) -> Self {
        self.route = route;
        self
    }

    /// Compare complete encodings by bytes, then by meaningful Deflate bits.
    ///
    /// The comparison is deliberately strict: equal candidates retain the
    /// incumbent selected by the earlier route, preserving deterministic
    /// routing and output bytes.
    fn is_strictly_smaller_than(&self, incumbent: &Self) -> bool {
        is_strictly_better(
            self.data.len(),
            self.bits,
            incumbent.data.len(),
            incumbent.bits,
        )
    }

    /// Compare a generated candidate with the parsed source stream.
    fn is_strictly_smaller_than_source(&self, source: CandidateInput<'_>) -> bool {
        is_strictly_better(
            self.data.len(),
            self.bits,
            source.compressed.len(),
            source.meaningful_bits,
        )
    }

    /// Replace this incumbent only when `contender` is strictly smaller.
    ///
    /// Returning whether the replacement happened lets callers retain route
    /// lineage without repeating the comparison and assignment.
    fn replace_if_smaller(&mut self, contender: Self) -> bool {
        if contender.is_strictly_smaller_than(self) {
            *self = contender;
            true
        } else {
            false
        }
    }

    /// Whether another candidate proved this exact encoding to be a max-plan
    /// fixed point.
    fn is_encoding_stabilized_by(&self, contender: &Self) -> bool {
        contender.max_planner_is_stable
            && contender.bits == self.bits
            && contender.data == self.data
    }
}

fn candidate_progress(
    candidate: &Candidate,
    reference_bits: u64,
    profitable: bool,
) -> CandidateProgress {
    CandidateProgress {
        bytes: candidate.data.len(),
        bits: candidate.bits,
        reference_bits,
        profitable,
    }
}

/// Run one original-source max search, with nested telemetry only in verbose
/// mode and the historical hot path otherwise.
fn build_source_max_candidate<F>(
    source: CandidateInput<'_>,
    options: &Options,
    progress: Progress,
    deadline: &Deadline,
    expired: &mut F,
) -> Result<Candidate>
where
    F: FnMut() -> bool,
{
    if !progress.enabled() {
        let candidate = build_candidate(source, options, MAX_RAW_REPLAY_LIMIT, expired)?;
        return Ok(if candidate.route == "Original source" {
            candidate
        } else {
            candidate.named("Columbo source max route")
        });
    }

    build_source_max_candidate_verbose(source, options, progress, deadline, expired)
}

/// Keep the telemetry-heavy planner monomorphized only once. The generic
/// wrapper above preserves inlined deadline checks for production quiet mode,
/// while verbose mode can afford one indirect call at its existing probes.
fn build_source_max_candidate_verbose(
    source: CandidateInput<'_>,
    options: &Options,
    progress: Progress,
    deadline: &Deadline,
    expired: &mut dyn FnMut() -> bool,
) -> Result<Candidate> {
    let (step, details) = progress.start_detailed(
        "Columbo source max route",
        source.meaningful_bits,
        deadline.remaining(),
    );
    let mut monitored_expired = || {
        details.heartbeat();
        let should_stop = expired();
        if should_stop && deadline.was_triggered() {
            details.deadline_reached();
        }
        should_stop
    };
    match build_candidate_with_progress(
        source,
        options,
        MAX_RAW_REPLAY_LIMIT,
        &mut monitored_expired,
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

/// Insert a first candidate or replace an existing one only on a strict win.
fn replace_optional_if_smaller(incumbent: &mut Option<Candidate>, contender: Candidate) -> bool {
    let wins = incumbent
        .as_ref()
        .map_or(true, |current| contender.is_strictly_smaller_than(current));
    if wins {
        *incumbent = Some(contender);
    }
    wins
}

/// Completed candidates produced by the bounded first phase.
///
/// Optional fields make the later comparison order explicit without relying
/// on positional tuples. `suppress_later_source_max` distinguishes a route
/// intentionally omitted by policy from one that has not run yet. The separate
/// optional-route flag lets generic routes keep their final rewritten-candidate
/// pass while the completed legacy grouping ends its route family.
#[derive(Default)]
struct BoundedPhaseCandidates {
    floor: Option<Candidate>,
    floor_seeded: Option<Candidate>,
    deft4j: Option<Candidate>,
    narrow: Option<Candidate>,
    source_max: Option<Candidate>,
    suppress_later_source_max: bool,
    suppress_later_optional_routes: bool,
}

/// One complete compressed stream together with the facts needed to verify
/// every accepted reparse/replan round.
#[derive(Clone, Copy)]
struct CandidateInput<'a> {
    compressed: &'a [u8],
    blocks: &'a [ParsedBlock],
    meaningful_bits: u64,
    decoded_limit: u64,
    identity: StreamIdentity,
}

/// Borrow a validated rewrite in the same shape as the original source.
///
/// `candidate.bits` remains authoritative for comparisons: reparsing proves
/// stream identity, while the emitter's exact meaningful-bit count is what
/// the route originally priced and selected.
fn rewritten_input<'a>(
    candidate: &'a Candidate,
    stream: &'a ParsedStream,
    decoded_limit: u64,
    identity: StreamIdentity,
) -> CandidateInput<'a> {
    CandidateInput {
        compressed: &candidate.data,
        blocks: &stream.blocks,
        meaningful_bits: candidate.bits,
        decoded_limit,
        identity,
    }
}

/// Run the independent bounded comparison routes under one wall clock.
///
/// Small inputs can safely share their immutable parsed blocks across worker
/// threads. Larger inputs use the same fixed route order serially, preventing
/// otherwise bounded per-route arenas from adding up to an excessive peak.
#[allow(clippy::too_many_arguments)]
fn build_bounded_phase_candidates(
    source: CandidateInput<'_>,
    options: &Options,
    probe_floor_expansion: bool,
    run_deft4j: bool,
    run_narrow: bool,
    parallel_routes: bool,
    deadline: &Deadline,
    completed_floor: Option<Candidate>,
) -> Result<BoundedPhaseCandidates> {
    if !run_deft4j && !run_narrow {
        let (floor, floor_seeded) = build_bounded_expanding_floor_lineage(
            source,
            options,
            probe_floor_expansion,
            deadline,
            completed_floor,
        )?;
        return Ok(BoundedPhaseCandidates {
            floor: Some(floor),
            floor_seeded,
            ..BoundedPhaseCandidates::default()
        });
    }

    if !parallel_routes {
        return build_bounded_phase_candidates_sequential(
            source,
            options,
            run_deft4j,
            run_narrow,
            deadline,
            completed_floor,
        );
    }

    thread::scope(|scope| {
        let deft4j_worker = run_deft4j.then(|| {
            thread::Builder::new()
                .name("columbo-deft4j-derived".into())
                .spawn_scoped(scope, || {
                    run_route_with_cancellation(deadline, || {
                        build_deft4j_source_candidate(source, options, &mut || {
                            deadline.route_should_stop()
                        })
                    })
                })
                .ok()
        });
        let narrow_worker = run_narrow.then(|| {
            thread::Builder::new()
                .name("columbo-no-split".into())
                .spawn_scoped(scope, || {
                    run_route_with_cancellation(deadline, || {
                        build_narrow_source_candidate(source, options, &mut || {
                            deadline.route_should_stop()
                        })
                    })
                })
                .ok()
        });

        // A route error (or unwind) asks its siblings to stop at their next
        // ordinary deadline check. Join every successfully spawned worker
        // before choosing the fixed deft4j/narrow/floor error order below.
        let floor = run_route_with_cancellation(deadline, || {
            build_bounded_expanding_floor_lineage(
                source,
                options,
                probe_floor_expansion,
                deadline,
                completed_floor,
            )
        });
        let deft4j = match deft4j_worker.flatten() {
            Some(worker) => worker.join(),
            None if run_deft4j => Ok(run_route_with_cancellation(deadline, || {
                build_deft4j_source_candidate(source, options, &mut || deadline.route_should_stop())
            })),
            None => Ok(Ok(None)),
        };
        let narrow = match narrow_worker.flatten() {
            Some(worker) => worker.join(),
            None if run_narrow => Ok(run_route_with_cancellation(deadline, || {
                build_narrow_source_candidate(source, options, &mut || deadline.route_should_stop())
            })),
            None => Ok(Ok(None)),
        };

        // A panic still denotes an internal invariant failure. Both joins are
        // complete now, so resuming it cannot strand a sibling worker.
        let deft4j = match deft4j {
            Ok(result) => result,
            Err(payload) => std::panic::resume_unwind(payload),
        }?;
        let narrow = match narrow {
            Ok(result) => result,
            Err(payload) => std::panic::resume_unwind(payload),
        }?;
        let (floor, floor_seeded) = floor?;
        Ok(BoundedPhaseCandidates {
            floor: Some(floor),
            floor_seeded,
            deft4j,
            narrow,
            ..BoundedPhaseCandidates::default()
        })
    })
}

/// Retain the deft4j/narrow/floor deadline order without overlapping arenas.
fn build_bounded_phase_candidates_sequential(
    source: CandidateInput<'_>,
    options: &Options,
    run_deft4j: bool,
    run_narrow: bool,
    deadline: &Deadline,
    completed_floor: Option<Candidate>,
) -> Result<BoundedPhaseCandidates> {
    let deft4j = if run_deft4j {
        build_deft4j_source_candidate(source, options, &mut || deadline.expired())?
    } else {
        None
    };
    let narrow = if run_narrow {
        build_narrow_source_candidate(source, options, &mut || deadline.expired())?
    } else {
        None
    };
    let floor =
        completed_or_bounded_floor(source, options, completed_floor, &mut || deadline.expired())?;
    Ok(BoundedPhaseCandidates {
        floor: Some(floor),
        deft4j,
        narrow,
        ..BoundedPhaseCandidates::default()
    })
}

fn parallel_route_is_bounded(source: CandidateInput<'_>) -> bool {
    let Some(token_count) = source.blocks.iter().try_fold(0_usize, |total, block| {
        total.checked_add(block.tokens.len())
    }) else {
        return false;
    };
    parallel_route_sizes_are_bounded(
        source.compressed.len(),
        source.identity.decoded_size,
        token_count,
        source.blocks.len(),
    )
}

fn parallel_route_sizes_are_bounded(
    compressed_bytes: usize,
    decoded_bytes: u64,
    token_count: usize,
    block_count: usize,
) -> bool {
    if compressed_bytes > PARALLEL_ROUTE_MAX_COMPRESSED
        || decoded_bytes > PARALLEL_ROUTE_MAX_DECODED
    {
        return false;
    }
    usize::try_from(decoded_bytes)
        .ok()
        .and_then(|decoded| parsed_model_bytes(decoded, token_count, block_count))
        .is_some_and(|model| model <= PARALLEL_ROUTE_MAX_MODEL)
}

/// Request sibling cancellation when a route returns an error or unwinds.
fn run_route_with_cancellation<T>(
    deadline: &Deadline,
    route: impl FnOnce() -> Result<T>,
) -> Result<T> {
    struct CancelOnFailure<'a> {
        deadline: &'a Deadline,
        succeeded: bool,
    }

    impl Drop for CancelOnFailure<'_> {
        fn drop(&mut self) {
            if !self.succeeded {
                self.deadline.cancel_routes();
            }
        }
    }

    let mut guard = CancelOnFailure {
        deadline,
        succeeded: false,
    };
    let result = route();
    guard.succeeded = result.is_ok();
    result
}

fn build_bounded_floor_candidate<F>(
    source: CandidateInput<'_>,
    options: &Options,
    expired: &mut F,
) -> Result<Candidate>
where
    F: FnMut() -> bool,
{
    let mut floor_options = options.clone();
    floor_options.exhaustive = false;
    build_candidate(source, &floor_options, DEFAULT_RAW_REPLAY_LIMIT, expired)
        .map(|candidate| candidate.named("Normal floor"))
}

/// Reuse a completed ordinary-mode floor when PNG scheduling already made it.
///
/// Its retained plans have the same shape needed by each bounded continuation,
/// so rebuilding the candidate would only consume deadline and memory.
fn completed_or_bounded_floor<F>(
    source: CandidateInput<'_>,
    options: &Options,
    completed_floor: Option<Candidate>,
    expired: &mut F,
) -> Result<Candidate>
where
    F: FnMut() -> bool,
{
    match completed_floor {
        Some(floor) => Ok(floor.named("Normal floor")),
        None => build_bounded_floor_candidate(source, options, expired),
    }
}

/// Build the ordinary bounded floor, then continue through the historical
/// Columbo max route seeded from that rewritten floor.
///
/// Both complete candidates remain independent, so timeout or non-improvement
/// cannot discard the normal-mode floor. The caller applies the parallel model
/// cap before selecting this route.
fn build_bounded_floor_lineage(
    source: CandidateInput<'_>,
    options: &Options,
    deadline: &Deadline,
    completed_floor: Option<Candidate>,
) -> Result<(Candidate, Option<Candidate>)> {
    let floor = completed_or_bounded_floor(source, options, completed_floor, &mut || {
        deadline.route_should_stop()
    })?;
    continue_bounded_floor_lineage(source, floor, options, deadline)
}

/// Build the ordinary bounded floor, then apply only Columbo's deterministic
/// bounded grouping to that rewritten topology.
///
/// The legacy corpus band benefits from this exact structural continuation;
/// skipping unrelated max routes avoids both their duplicate work and their
/// competition for the same PNG deadline.
fn build_bounded_floor_grouping_lineage(
    source: CandidateInput<'_>,
    options: &Options,
    deadline: &Deadline,
    completed_floor: Option<Candidate>,
) -> Result<(Candidate, Option<Candidate>)> {
    let mut floor = completed_or_bounded_floor(source, options, completed_floor, &mut || {
        deadline.route_should_stop()
    })?;
    let floor_selected = options.strict || floor.is_strictly_smaller_than_source(source);
    if !floor_selected || deadline.route_should_stop() {
        return Ok((floor, None));
    }

    // The grouping route reparses the encoded floor and owns its grouped
    // tokens, so the earlier token plans need not overlap that model.
    floor.plans.clear();
    let seeded =
        refine_with_bounded_grouping_floor(&floor, options, source.decoded_limit, source.identity)?;
    Ok((floor, seeded))
}

/// Continue the historical floor seed only when ordinary planning exposes
/// additional nonempty blocks from a coarse two-to-four-block source.
fn build_bounded_expanding_floor_lineage(
    source: CandidateInput<'_>,
    options: &Options,
    probe_expansion: bool,
    deadline: &Deadline,
    completed_floor: Option<Candidate>,
) -> Result<(Candidate, Option<Candidate>)> {
    let floor = completed_or_bounded_floor(source, options, completed_floor, &mut || {
        deadline.route_should_stop()
    })?;
    if !probe_expansion {
        return Ok((floor, None));
    }

    // Inspect the plans already retained by the floor. Re-parsing merely to
    // rediscover this topology would spend time and allocate another model.
    let source_blocks = source
        .blocks
        .iter()
        .filter(|block| !block.plain.is_empty())
        .count();
    let floor_blocks = floor
        .plans
        .iter()
        .filter(|plan| !plan.plain.is_empty())
        .count();
    if floor_blocks <= source_blocks {
        return Ok((floor, None));
    }
    continue_bounded_floor_lineage(source, floor, options, deadline)
}

fn continue_bounded_floor_lineage(
    source: CandidateInput<'_>,
    mut floor: Candidate,
    options: &Options,
    deadline: &Deadline,
) -> Result<(Candidate, Option<Candidate>)> {
    let floor_selected = options.strict || floor.is_strictly_smaller_than_source(source);
    if !floor_selected || deadline.route_should_stop() {
        return Ok((floor, None));
    }

    // The seeded parse owns its rewritten tokens. Release the floor plans
    // before allocating that model so this remains one route arena.
    floor.plans.clear();
    let seeded = refine_with_max_planner(
        &floor,
        options,
        source.decoded_limit,
        source.identity,
        &mut || deadline.route_should_stop(),
    )?;
    Ok((floor, Some(seeded)))
}

/// Preserve source max when no specialized source route is eligible.
///
/// The original source and floor-seeded candidates share one deadline and at
/// most two route arenas. The caller has already applied the parallel model
/// cap before selecting this route.
fn build_bounded_generic_max_candidates(
    source: CandidateInput<'_>,
    options: &Options,
    deadline: &Deadline,
    progress: Progress,
    completed_floor: Option<Candidate>,
) -> Result<BoundedPhaseCandidates> {
    thread::scope(|scope| {
        let source_worker = thread::Builder::new()
            .name("columbo-source-max".into())
            .spawn_scoped(scope, || {
                run_route_with_cancellation(deadline, || {
                    build_source_max_candidate(source, options, progress, deadline, &mut || {
                        deadline.route_should_stop()
                    })
                })
            })
            .ok();

        let floor = run_route_with_cancellation(deadline, || {
            build_bounded_floor_lineage(source, options, deadline, completed_floor)
        });
        let source_max = match source_worker {
            Some(worker) => match worker.join() {
                Ok(result) => result.map(Some),
                Err(payload) => std::panic::resume_unwind(payload),
            },
            None if !deadline.route_should_stop() => run_route_with_cancellation(deadline, || {
                build_source_max_candidate(source, options, progress, deadline, &mut || {
                    deadline.route_should_stop()
                })
            })
            .map(Some),
            None => Ok(None),
        };

        // Both routes have rejoined. Preserve floor-before-source error
        // precedence when independent failures happen at the same time.
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

/// Build a bounded, source-ordered deft4j candidate without replaying it
/// through the broader planner.
///
/// Its fixed-point work remains subject to the caller's Columbo deadline and
/// memory policies. Any later cross-route replay is scheduled explicitly by
/// the caller.
fn build_deft4j_source_candidate<F>(
    source: CandidateInput<'_>,
    options: &Options,
    expired: &mut F,
) -> Result<Option<Candidate>>
where
    F: FnMut() -> bool,
{
    let Some(plans) = plan_source_blocks(source.blocks, 0, options, &mut *expired) else {
        return Ok(None);
    };
    build_candidate_from_plans(source, plans, options, 0, ReplayPlanner::Full, expired)
        .map(|candidate| Some(candidate.named("deft4j-derived source")))
}

/// Build one direct source-order candidate without the broader split graph.
///
/// This mirrors the useful no-split route family while sharing the caller's
/// parsed payloads and deadline. It is intentionally not replayed through
/// `plan_stream`; doing so would immediately repeat the grouping and split
/// work this sibling exists to bypass.
fn build_narrow_source_candidate<F>(
    source: CandidateInput<'_>,
    options: &Options,
    expired: &mut F,
) -> Result<Option<Candidate>>
where
    F: FnMut() -> bool,
{
    let Some(plans) = plan_source_no_split_route(source.blocks, 0, options, true, &mut *expired)
    else {
        return Ok(None);
    };
    build_candidate_from_plans(source, plans, options, 0, ReplayPlanner::Full, expired)
        .map(|candidate| Some(candidate.named("No-split source")))
}

/// Apply the source-ordered deft4j route to a genuinely rewritten floor state.
///
/// Weak gains from the original deft4j route can indicate that Columbo's
/// ordinary bounded floor is the more useful parent in its route graph.
/// Reparse that encoded incumbent once, validate its identity, and keep the
/// seeded deft4j walk additive to the original deft4j candidate rather than
/// replaying duplicate work on the original blocks.
fn build_deft4j_seed_candidate<F>(
    candidate: &Candidate,
    options: &Options,
    decoded_limit: u64,
    identity: StreamIdentity,
    allow_single_block: bool,
    expired: &mut F,
) -> Result<Option<Candidate>>
where
    F: FnMut() -> bool,
{
    let stream = parse_validated_rewrite(&candidate.data, decoded_limit, identity)?;
    if !deft4j_source_route_eligible(&stream.blocks, candidate.data.len(), allow_single_block) {
        return Ok(None);
    }
    let source = rewritten_input(candidate, &stream, decoded_limit, identity);
    build_deft4j_source_candidate(source, options, expired)
}

/// Apply the ordinary planner to boundaries and token spellings produced by
/// the source-ordered deft4j route while the same container deadline has room.
///
/// This is not duplicate source work: the reparsed deft4j candidate contains
/// merged blocks and expanded token states that the original normal floor
/// cannot see. The original floor remains an independent comparison below.
fn refine_with_default_planner<F>(
    candidate: &Candidate,
    options: &Options,
    decoded_limit: u64,
    identity: StreamIdentity,
    expired: &mut F,
) -> Result<Candidate>
where
    F: FnMut() -> bool,
{
    let stream = parse_validated_rewrite(&candidate.data, decoded_limit, identity)?;
    let source = rewritten_input(candidate, &stream, decoded_limit, identity);
    let mut floor_options = options.clone();
    floor_options.exhaustive = false;
    build_candidate(source, &floor_options, DEFAULT_RAW_REPLAY_LIMIT, expired)
        .map(|candidate| candidate.named("Default refinement"))
}

/// Finish a small deft4j-derived seed with Columbo's structural split floor.
///
/// The timed source route can settle its token spellings just before the
/// deadline, leaving no opportunity to price the seven inexpensive
/// eighth-position cuts already used elsewhere by Columbo. This route performs
/// no token search, merge search, or replay. Its explicit size, topology, and
/// token limits make it safe to finish deterministically after the shared
/// deadline, while the caller keeps the unsplit candidate as a fallback.
fn refine_with_compact_source_split_floor(
    candidate: &Candidate,
    options: &Options,
    decoded_limit: u64,
    identity: StreamIdentity,
) -> Result<Option<Candidate>> {
    if candidate.data.len() > COMPACT_SPLIT_FLOOR_MAX_COMPRESSED {
        return Ok(None);
    }

    let stream = parse_validated_rewrite(&candidate.data, decoded_limit, identity)?;
    if !compact_source_split_floor_eligible(identity.decoded_size, &stream.blocks) {
        return Ok(None);
    }

    let Some(plans) = plan_compact_source_split_floor(&stream.blocks, 0, options) else {
        return Ok(None);
    };
    let source = rewritten_input(candidate, &stream, decoded_limit, identity);
    let mut never_expires = || false;
    build_candidate_from_plans(
        source,
        plans,
        options,
        0,
        ReplayPlanner::Full,
        &mut never_expires,
    )
    .map(|candidate| Some(candidate.named("Columbo compact split floor")))
}

fn compact_source_split_floor_eligible(decoded_size: u64, blocks: &[ParsedBlock]) -> bool {
    if decoded_size > COMPACT_SPLIT_FLOOR_MAX_DECODED
        || !(2..=COMPACT_SPLIT_FLOOR_MAX_BLOCKS).contains(&blocks.len())
        || blocks
            .iter()
            .any(|block| block.plain.is_empty() || block.source_type == SourceBlockType::Stored)
        || !blocks
            .iter()
            .any(|block| block.tokens.len() >= 16 && block.plain.len() >= 128)
    {
        return false;
    }
    blocks
        .iter()
        .try_fold(0_usize, |count, block| {
            count.checked_add(block.tokens.len())
        })
        .is_some_and(|tokens| tokens <= COMPACT_SPLIT_FLOOR_MAX_TOKENS)
}

/// Apply Columbo's bounded quad-lengthening move to one finished dynamic block.
///
/// The source-level gate admits only the narrow 4–8 KiB gap between the tiny
/// deft4j route and larger generic work. This post-route helper neither changes
/// tokens nor replays the result; the encoded incumbent remains an independent
/// fallback if every legal tree move loses.
fn refine_with_compact_quad_floor(
    candidate: &Candidate,
    options: &Options,
    decoded_limit: u64,
    identity: StreamIdentity,
) -> Result<Option<Candidate>> {
    let stream = parse_validated_rewrite(&candidate.data, decoded_limit, identity)?;
    let [block] = stream.blocks.as_slice() else {
        return Ok(None);
    };
    if block.source_type != SourceBlockType::Dynamic
        || !(COMPACT_QUAD_MIN_TOKENS..=COMPACT_QUAD_MAX_TOKENS).contains(&block.tokens.len())
    {
        return Ok(None);
    }
    let Some(seed) = block.original_dynamic.as_ref() else {
        return Ok(None);
    };
    let Some(dynamic) = plan_columbo_quad_lengthen_candidate(
        &block.tokens,
        &block.literal_frequencies,
        &block.distance_frequencies,
        seed,
    ) else {
        return Ok(None);
    };
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
    let mut never_expires = || false;
    build_candidate_from_plans(
        source,
        plans,
        options,
        0,
        ReplayPlanner::Full,
        &mut never_expires,
    )
    .map(|candidate| Some(candidate.named("Columbo compact quad floor")))
}

/// Apply the full Columbo max planner to a complete rewritten candidate.
fn refine_with_max_planner<F>(
    candidate: &Candidate,
    options: &Options,
    decoded_limit: u64,
    identity: StreamIdentity,
    expired: &mut F,
) -> Result<Candidate>
where
    F: FnMut() -> bool,
{
    let stream = parse_validated_rewrite(&candidate.data, decoded_limit, identity)?;
    let source = rewritten_input(candidate, &stream, decoded_limit, identity);
    build_candidate(source, options, MAX_RAW_REPLAY_LIMIT, expired)
        .map(|candidate| candidate.named("Columbo max refinement"))
}

/// Apply Columbo's bounded source-grouping floor to a complete rewritten
/// candidate without entering the broader max planner or replaying its result.
fn refine_with_bounded_grouping_floor(
    candidate: &Candidate,
    options: &Options,
    decoded_limit: u64,
    identity: StreamIdentity,
) -> Result<Option<Candidate>> {
    let stream = parse_validated_rewrite(&candidate.data, decoded_limit, identity)?;
    let Some(plans) = plan_columbo_floor_seeded_bounded_grouping(&stream.blocks, 0, options) else {
        return Ok(None);
    };
    let source = rewritten_input(candidate, &stream, decoded_limit, identity);
    let mut never_expires = || false;
    build_candidate_from_plans(
        source,
        plans,
        options,
        0,
        ReplayPlanner::Full,
        &mut never_expires,
    )
    .map(|candidate| Some(candidate.named("Columbo floor-seeded grouping")))
}

/// Apply one linear, deterministic merge cleanup to a selected max seed.
///
/// The timed route may end immediately after producing a substantially better
/// block list. Reparse that completed stream and price only adjacent merges
/// with bounded table floors. If time remains, at most two ordinary replays
/// may stabilize the smaller list. Optional probes observe the caller's
/// deadline; complete candidate emission and validation still finish.
fn refine_with_terminal_merge<F>(
    candidate: &Candidate,
    options: &Options,
    decoded_limit: u64,
    identity: StreamIdentity,
    expired: &mut F,
) -> Result<Option<Candidate>>
where
    F: FnMut() -> bool,
{
    if expired() {
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

/// Build one complete-stream candidate and follow strict improvements through
/// a small number of reparse/replan rounds.
fn build_candidate<F>(
    source: CandidateInput<'_>,
    options: &Options,
    replay_limit: usize,
    expired: &mut F,
) -> Result<Candidate>
where
    F: FnMut() -> bool,
{
    let Some(plans) = plan_stream(source.blocks, 0, options, &mut *expired) else {
        return source_candidate(source, options);
    };
    build_candidate_from_plans(
        source,
        plans,
        options,
        replay_limit,
        ReplayPlanner::Full,
        expired,
    )
}

/// Build the direct source-max candidate with nested human-facing telemetry.
///
/// Keeping this separate from `build_candidate` means ordinary and quiet
/// routes do not take clocks or maintain progress state in their hot loops.
fn build_candidate_with_progress<F>(
    source: CandidateInput<'_>,
    options: &Options,
    replay_limit: usize,
    expired: &mut F,
    progress: &RouteProgress,
) -> Result<Candidate>
where
    F: FnMut() -> bool,
{
    let Some(plans) = plan_stream_with_progress(source.blocks, 0, options, &mut *expired, progress)
    else {
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
    )
}

#[derive(Clone, Copy)]
enum ReplayPlanner {
    Full,
    Fragmented,
}

/// Emit an explicitly selected structural seed and stabilize strict replays.
///
/// Most callers obtain their first plans from [`plan_stream`]. Alternate
/// collection strategies intentionally need to preserve a locally dearer seed,
/// so accepting the initial plan list here prevents the ordinary planner from
/// replacing it before its new block boundaries have been reparsed.
fn build_candidate_from_plans<F>(
    source: CandidateInput<'_>,
    plans: Vec<PlannedBlock>,
    options: &Options,
    replay_limit: usize,
    replay_planner: ReplayPlanner,
    expired: &mut F,
) -> Result<Candidate>
where
    F: FnMut() -> bool,
{
    build_candidate_from_plans_with_progress(
        source,
        plans,
        options,
        replay_limit,
        replay_planner,
        expired,
        None,
    )
}

#[allow(clippy::too_many_arguments)]
fn build_candidate_from_plans_with_progress<F>(
    source: CandidateInput<'_>,
    mut plans: Vec<PlannedBlock>,
    options: &Options,
    replay_limit: usize,
    replay_planner: ReplayPlanner,
    expired: &mut F,
    progress: Option<&RouteProgress>,
) -> Result<Candidate>
where
    F: FnMut() -> bool,
{
    let (mut data, mut bits) = emit_plans(source.compressed, &plans, options.strict)?;
    let initial_loses = !is_strictly_better(
        data.len(),
        bits,
        source.compressed.len(),
        source.meaningful_bits,
    );

    // A merged stream has token and block boundaries which did not exist in
    // its input. Re-parsing makes those boundaries available to the next
    // planning round. Each accepted round must improve the complete stream,
    // so this cannot oscillate even before the explicit cap is reached.
    let mut data_is_validated = false;
    let mut max_planner_is_stable = false;
    for round in 1..=replay_limit {
        if initial_loses && !options.strict {
            if let Some(progress) = progress {
                progress.stopped("Replays skipped because the initial route is not smaller");
            }
            break;
        }
        if expired() {
            if let Some(progress) = progress {
                progress.deadline_reached();
                progress.stopped("File-wide deadline reached before the next replay");
            }
            break;
        }

        let replay_started = progress.map(|progress| {
            progress.replay_started(round, replay_limit, plans.len(), bits);
            Instant::now()
        });
        let replayed = parse_validated_rewrite(&data, source.decoded_limit, source.identity)?;
        data_is_validated = true;

        let replay_plans = match replay_planner {
            ReplayPlanner::Full => match progress {
                Some(progress) => {
                    plan_stream_with_progress(&replayed.blocks, 0, options, &mut *expired, progress)
                }
                None => plan_stream(&replayed.blocks, 0, options, &mut *expired),
            },
            ReplayPlanner::Fragmented => plan_fragmented_replay(&replayed.blocks, 0, options),
        };
        let Some(replay_plans) = replay_plans else {
            if let Some(progress) = progress {
                progress.stopped("Replay planner produced no complete candidate");
            }
            break;
        };
        let (replay_data, replay_bits) = emit_plans(&data, &replay_plans, options.strict)?;
        if !is_strictly_better(replay_data.len(), replay_bits, data.len(), bits) {
            // A completed exhaustive full replay from the current bytes is
            // precisely the work performed by the later rewritten-seed route.
            // Record the fixed point only while the allowance is still live;
            // a deadline-truncated planner has not proved stability.
            max_planner_is_stable = matches!(replay_planner, ReplayPlanner::Full)
                && options.exhaustive
                && replay_bits == bits
                && replay_data == data
                && !expired();
            if let (Some(progress), Some(started)) = (progress, replay_started) {
                if progress.deadline_was_reached() {
                    progress.replay_stopped(
                        round,
                        started.elapsed(),
                        "file-wide deadline reached; retained the best completed plans",
                    );
                } else {
                    progress.replay_finished(
                        round,
                        bits,
                        replay_bits,
                        replay_plans.len(),
                        started.elapsed(),
                        false,
                    );
                }
            }
            break;
        }
        if let (Some(progress), Some(started)) = (progress, replay_started) {
            progress.replay_finished(
                round,
                bits,
                replay_bits,
                replay_plans.len(),
                started.elapsed(),
                true,
            );
        }
        data = replay_data;
        bits = replay_bits;
        plans = replay_plans;
        // The accepted rewrite has not itself been parsed yet. The next loop
        // iteration or the final check below must validate this new stream.
        data_is_validated = false;
    }

    // The last accepted replay is not necessarily followed by another loop
    // iteration: the replay cap or deadline may stop immediately after it.
    // Validate that final selectable stream as well. A known-losing generated
    // stream is skipped because the caller will retain the already-validated
    // source bytes instead.
    if (!initial_loses || options.strict) && !data_is_validated {
        parse_validated_rewrite(&data, source.decoded_limit, source.identity)?;
    }

    let block_report = capture_planned_block_report(&plans, options.verbose);
    Ok(Candidate {
        data,
        bits,
        plans,
        block_report,
        route: if options.exhaustive {
            "Columbo max route"
        } else {
            "Normal route"
        },
        max_planner_is_stable,
    })
}

/// Retain a parsed source when the mandatory plan list cannot be allocated.
/// Strict mode must rewrite incompatible Huffman alphabets, so it reports an
/// allocation failure rather than silently ignoring the requested transform.
fn source_candidate(source: CandidateInput<'_>, options: &Options) -> Result<Candidate> {
    if options.strict {
        return Err(Error::new("could not allocate Deflate plan"));
    }
    let mut data = Vec::new();
    data.try_reserve_exact(source.compressed.len())
        .map_err(|_| Error::new("could not allocate Deflate output"))?;
    data.extend_from_slice(source.compressed);
    Ok(Candidate {
        data,
        bits: source.meaningful_bits,
        plans: Vec::new(),
        block_report: None,
        route: "Original source",
        max_planner_is_stable: false,
    })
}

/// Parse a generated stream and fail closed if it changed the decoded data.
fn parse_validated_rewrite(
    data: &[u8],
    decoded_limit: u64,
    identity: StreamIdentity,
) -> Result<ParsedStream> {
    let stream = parse_stream(data, decoded_limit)?;
    validate_replayed_stream(&stream, data.len(), identity)?;
    Ok(stream)
}

/// Verify every generated stream before using it as input to another round.
///
/// These checks used to be debug assertions. Keeping them in release builds is
/// inexpensive—the parser has already computed every value—and makes an
/// emitter or planner bug fail closed instead of allowing changed decoded data
/// to become the next optimization seed.
fn validate_replayed_stream(
    stream: &ParsedStream,
    expected_bytes: usize,
    identity: StreamIdentity,
) -> Result<()> {
    if stream.consumed != expected_bytes
        || stream.decoded_size != identity.decoded_size
        || stream.crc32 != identity.crc32
        || stream.adler32 != identity.adler32
    {
        return Err(Error::new(
            "internal error: rewritten Deflate stream changed decoded data",
        ));
    }
    Ok(())
}

fn emit_plans(input: &[u8], plans: &[PlannedBlock], strict: bool) -> Result<(Vec<u8>, u64)> {
    if strict
        && plans.iter().any(|plan| {
            matches!(
                &plan.representation,
                Representation::Dynamic(dynamic)
                    if !dynamic.has_strictly_compatible_huffman_codes()
            )
        })
    {
        return Err(Error::new(
            "internal strict Deflate plan has an incomplete Huffman code",
        ));
    }

    let mut writer = BitWriter::default();
    for (index, plan) in plans.iter().enumerate() {
        emit_block(&mut writer, input, plan, index + 1 == plans.len())?;
    }
    let bits = writer.bit_position();
    Ok((writer.into_bytes(), bits))
}

fn is_strictly_better(
    candidate_bytes: usize,
    candidate_bits: u64,
    reference_bytes: usize,
    reference_bits: u64,
) -> bool {
    candidate_bytes < reference_bytes
        || (candidate_bytes == reference_bytes && candidate_bits < reference_bits)
}

fn capture_planned_block_report(plans: &[PlannedBlock], enabled: bool) -> Option<BlockReport> {
    if !enabled {
        return None;
    }
    let shown = plans.len().min(MAX_REPORTED_BLOCKS);
    let mut blocks = Vec::new();
    blocks.try_reserve_exact(shown).ok()?;
    let mut alignment = 0_u8;
    for (index, plan) in plans.iter().take(shown).enumerate() {
        blocks.push(block_progress(
            alignment,
            index + 1 == plans.len(),
            plan.source_type,
            &plan.representation,
            plan.plain.len(),
            plan.tokens.len(),
            plan.bits,
        ));
        alignment = ((u64::from(alignment) + plan.bits) & 7) as u8;
    }
    Some(BlockReport {
        blocks,
        total_blocks: plans.len(),
    })
}

fn capture_source_block_report(
    blocks: &[ParsedBlock],
    source_block_count: usize,
    enabled: bool,
) -> Option<BlockReport> {
    // The parser intentionally collapses pathological runs of empty blocks.
    // In that case it no longer has an exact block-for-block view of source
    // bytes, so omit details instead of presenting the compact model as the
    // original stream.
    if !enabled || blocks.len() != source_block_count {
        return None;
    }
    let shown = blocks.len().min(MAX_REPORTED_BLOCKS);
    let mut report = Vec::new();
    report.try_reserve_exact(shown).ok()?;
    for (index, block) in blocks.iter().take(shown).enumerate() {
        let original = block.original?;
        report.push(block_progress(
            original.alignment,
            index + 1 == blocks.len(),
            block.source_type,
            &Representation::Original(original),
            block.plain.len(),
            block.tokens.len(),
            original.len,
        ));
    }
    Some(BlockReport {
        blocks: report,
        total_blocks: source_block_count,
    })
}

fn block_progress(
    alignment: u8,
    final_block: bool,
    source_type: SourceBlockType,
    representation: &Representation,
    decoded_bytes: usize,
    tokens: usize,
    output_bits: u64,
) -> BlockProgress {
    let output = match representation {
        Representation::Original(_) => BlockEncoding::Original,
        Representation::Stored => BlockEncoding::Stored,
        Representation::Fixed => BlockEncoding::Fixed,
        Representation::Dynamic(_) => BlockEncoding::Dynamic,
    };
    let input = match source_type {
        SourceBlockType::Stored => BlockEncoding::Stored,
        SourceBlockType::Fixed => BlockEncoding::Fixed,
        SourceBlockType::Dynamic => BlockEncoding::Dynamic,
    };
    BlockProgress {
        alignment,
        decoded_bytes,
        final_block,
        input,
        output,
        output_bits,
        tokens,
    }
}

const DEADLINE_TIMED_OUT: u8 = 1;
const DEADLINE_ROUTES_CANCELLED: u8 = 2;

struct Deadline {
    started: Instant,
    duration: std::time::Duration,
    // Timeout and route-cancellation flags share one atomic so a hot search
    // probe performs only one synchronization load. The flags carry no data,
    // so relaxed ordering is sufficient.
    state: AtomicU8,
}

impl Deadline {
    fn remaining(&self) -> std::time::Duration {
        self.duration.saturating_sub(self.started.elapsed())
    }

    fn expired(&self) -> bool {
        if self.state.load(Ordering::Relaxed) & DEADLINE_TIMED_OUT != 0
            || self.started.elapsed() >= self.duration
        {
            self.state.fetch_or(DEADLINE_TIMED_OUT, Ordering::Relaxed);
            true
        } else {
            false
        }
    }

    fn route_should_stop(&self) -> bool {
        if self.state.load(Ordering::Relaxed) != 0 {
            return true;
        }
        if self.started.elapsed() >= self.duration {
            self.state.fetch_or(DEADLINE_TIMED_OUT, Ordering::Relaxed);
            return true;
        }
        false
    }

    fn cancel_routes(&self) {
        // `fetch_or` cannot erase a timeout racing with this route failure.
        self.state
            .fetch_or(DEADLINE_ROUTES_CANCELLED, Ordering::Relaxed);
    }

    fn was_triggered(&self) -> bool {
        self.state.load(Ordering::Relaxed) & DEADLINE_TIMED_OUT != 0
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;
    use crate::deflate::bitstream::BitWriter;
    use crate::deflate::huffman::{fixed_trees, huffman_tree_shape_is_complete};
    use crate::deflate::model::Token;

    fn comparison_candidate(bytes: usize, bits: u64, marker: u8) -> Candidate {
        Candidate {
            data: vec![marker; bytes],
            bits,
            plans: Vec::new(),
            block_report: None,
            route: "test",
            max_planner_is_stable: false,
        }
    }

    #[test]
    fn candidate_replacement_is_byte_first_strict_and_stable_on_ties() {
        let mut incumbent = comparison_candidate(2, 8, 1);

        assert!(incumbent.replace_if_smaller(comparison_candidate(2, 7, 2)));
        assert_eq!(incumbent.data, vec![2; 2]);
        assert_eq!(incumbent.bits, 7);

        // An exact size tie retains the earlier route's byte spelling.
        assert!(!incumbent.replace_if_smaller(comparison_candidate(2, 7, 3)));
        assert_eq!(incumbent.data, vec![2; 2]);

        assert!(!incumbent.replace_if_smaller(comparison_candidate(2, 8, 4)));
        // Byte count is authoritative; bits only break equal-byte ties.
        assert!(incumbent.replace_if_smaller(comparison_candidate(1, 8, 5)));
        assert_eq!(incumbent.data, vec![5]);

        let mut optional = None;
        assert!(replace_optional_if_smaller(
            &mut optional,
            comparison_candidate(1, 8, 6)
        ));
        assert!(!replace_optional_if_smaller(
            &mut optional,
            comparison_candidate(1, 8, 7)
        ));
        assert_eq!(optional.as_ref().unwrap().data, vec![6]);
        assert!(replace_optional_if_smaller(
            &mut optional,
            comparison_candidate(1, 7, 8)
        ));
        assert_eq!(optional.unwrap().data, vec![8]);

        let incumbent = comparison_candidate(2, 7, 9);
        let mut stable = comparison_candidate(2, 7, 9);
        stable.max_planner_is_stable = true;
        assert!(incumbent.is_encoding_stabilized_by(&stable));
        stable.data[0] = 10;
        assert!(!incumbent.is_encoding_stabilized_by(&stable));

        let source_bytes = [0_u8; 2];
        let source = CandidateInput {
            compressed: &source_bytes,
            blocks: &[],
            meaningful_bits: 7,
            decoded_limit: 0,
            identity: StreamIdentity {
                decoded_size: 0,
                crc32: 0,
                adler32: 0,
            },
        };
        assert!(comparison_candidate(1, 8, 9).is_strictly_smaller_than_source(source));
        assert!(!comparison_candidate(2, 7, 10).is_strictly_smaller_than_source(source));
    }

    #[test]
    fn one_pass_closes_repeated_deflopt_defluff_feedback() {
        // This small synthetic dynamic block witnesses the interaction between
        // the recovered DeflOpt and Defluff methods. DeflOpt expands one
        // length-three match into literals and rebuilds a stream that is five
        // bits smaller overall. On that new frequency state, Defluff swaps the
        // five- and six-bit assignments of equal-frequency length symbols 259
        // and 260. The payload remains 196 bits while the dynamic header
        // shrinks from 130 to 129 bits. The original tools therefore need two
        // programs/passes: 331 -> 326 -> 325 meaningful bits.
        //
        // Columbo's broader feedback route must close the same state graph in
        // one invocation. A second invocation must not find another saving.
        let input = [
            0x25, 0xc0, 0x01, 0x01, 0xc0, 0x30, 0x0c, 0xc3, 0x30, 0x6c, 0xb5, 0x9b, 0xf0, 0x87,
            0xf4, 0x7d, 0xd3, 0xcc, 0xcc, 0xcc, 0xcc, 0x01, 0x00, 0x00, 0xc0, 0x71, 0x5d, 0xaa,
            0xaa, 0xaa, 0xfe, 0x76, 0x77, 0x93, 0x24, 0x49, 0x9e, 0xa7, 0x6d, 0xdb, 0xf6, 0x03,
        ];
        let source = parse_stream(&input, 86).unwrap();
        assert_eq!(source.meaningful_bits, 331);
        assert_eq!(source.decoded_size, 86);

        let options = Options {
            strict: false,
            ..Options::default()
        };
        let first = optimize_raw(&input, &options).unwrap();
        // Later additive structural routes may beat the recovered 325-bit
        // fixed point, but must never lose it.
        assert!(first.info.deflate_bits <= 325);
        let second = optimize_raw(&first.data, &options).unwrap();
        assert_eq!(second.info.deflate_bits, first.info.deflate_bits);
        assert_eq!(second.data, first.data);

        // Max mode always completes the default floor before optional timed
        // routes. A spent optional budget must therefore retain the same
        // fixed point without requiring a second executable invocation.
        let max_options = Options {
            exhaustive: true,
            strict: false,
            timeout: Duration::ZERO,
            ..Options::default()
        };
        let maximum = optimize_raw(&input, &max_options).unwrap();
        assert!(maximum.timed_out);
        assert_eq!(maximum.info.deflate_bits, first.info.deflate_bits);
        assert_eq!(maximum.data, first.data);
        let repeated_max = optimize_raw(&maximum.data, &max_options).unwrap();
        assert_eq!(repeated_max.data, maximum.data);
    }

    #[test]
    fn optimizes_a_dynamic_header_with_all_32_distance_code_lengths() {
        // This empty dynamic block advertises the full RFC 1951 HDIST range
        // with complete literal/length and distance trees. Reserved distance
        // symbol 31 participates in tree construction but is never decoded.
        let input = [
            0x05, 0xdf, 0x01, 0x04, 0x00, 0x00, 0x00, 0x00, 0x90, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x80, 0x01,
            0x00, 0x00, 0x80, 0x01,
        ];

        let optimized = optimize_raw(&input, &Options::default()).unwrap();
        let reparsed = parse_stream(&optimized.data, 0).unwrap();
        assert_eq!(reparsed.decoded_size, 0);
        assert!(optimized.data.len() <= input.len());
    }

    #[test]
    fn strict_mode_never_creates_the_non_rfc_258_alias() {
        let (literal, distance) = fixed_trees();
        let mut writer = BitWriter::default();
        writer.write(1, 1).unwrap(); // Final block.
        writer.write(1, 2).unwrap(); // Fixed Huffman block.
        for code in [
            literal.code(usize::from(b'A')).unwrap(),
            literal.code(285).unwrap(), // RFC length-258 spelling.
            distance.code(0).unwrap(),
            literal.code(256).unwrap(),
        ] {
            writer.write(u32::from(code.code), code.length).unwrap();
        }
        let input = writer.into_bytes();

        let optimized = optimize_raw(&input, &Options::default()).unwrap();
        let reparsed = parse_stream(&optimized.data, 259).unwrap();
        assert_eq!(reparsed.decoded_size, 259);
        assert!(reparsed.blocks.iter().all(|block| {
            block.tokens.iter().all(|token| {
                !matches!(
                    token,
                    Token::Match {
                        length_symbol: 284,
                        length_extra: 31,
                        ..
                    }
                )
            })
        }));
    }

    #[test]
    fn default_route_coalesces_adjacent_same_distance_matches() {
        let (literal, distance) = fixed_trees();
        let mut writer = BitWriter::default();
        writer.write(1, 1).unwrap(); // Final block.
        writer.write(1, 2).unwrap(); // Fixed Huffman block.
        let literal_a = literal.code(usize::from(b'A')).unwrap();
        writer
            .write(u32::from(literal_a.code), literal_a.length)
            .unwrap();
        for length_symbol in [257, 258] {
            let length = literal.code(length_symbol).unwrap();
            writer.write(u32::from(length.code), length.length).unwrap();
            let distance_one = distance.code(0).unwrap();
            writer
                .write(u32::from(distance_one.code), distance_one.length)
                .unwrap();
        }
        let end = literal.code(256).unwrap();
        writer.write(u32::from(end.code), end.length).unwrap();
        let input = writer.into_bytes();

        for strict in [true, false] {
            let options = Options {
                strict,
                ..Options::default()
            };
            let optimized = optimize_raw(&input, &options).unwrap();
            let reparsed = parse_stream(&optimized.data, 8).unwrap();
            assert_eq!(reparsed.decoded_size, 8);
            let tokens = reparsed
                .blocks
                .iter()
                .flat_map(|block| block.tokens.iter())
                .copied()
                .collect::<Vec<_>>();
            assert!(matches!(
                tokens.as_slice(),
                [
                    Token::Literal(b'A'),
                    Token::Match {
                        length: 7,
                        distance: 1,
                        ..
                    }
                ]
            ));
            assert!(optimized.data.len() <= input.len());
            let repeated = optimize_raw(&optimized.data, &options).unwrap();
            assert_eq!(repeated.data, optimized.data);
        }

        // A coalesced length 258 must use canonical symbol 285 in strict mode,
        // even though symbol 284 plus extra value 31 decodes to the same size
        // in Columbo's relaxed compatibility extension.
        let mut writer = BitWriter::default();
        writer.write(1, 1).unwrap();
        writer.write(1, 2).unwrap();
        writer
            .write(u32::from(literal_a.code), literal_a.length)
            .unwrap();
        for (length_symbol, extra, extra_bits) in [(279, 1, 4), (281, 27, 5)] {
            let length = literal.code(length_symbol).unwrap();
            writer.write(u32::from(length.code), length.length).unwrap();
            writer.write(extra, extra_bits).unwrap();
            let distance_one = distance.code(0).unwrap();
            writer
                .write(u32::from(distance_one.code), distance_one.length)
                .unwrap();
        }
        writer.write(u32::from(end.code), end.length).unwrap();
        let input = writer.into_bytes();
        let optimized = optimize_raw(&input, &Options::default()).unwrap();
        let reparsed = parse_stream(&optimized.data, 259).unwrap();
        assert!(matches!(
            reparsed.blocks.as_slice(),
            [ParsedBlock { tokens, .. }]
                if matches!(
                    tokens.as_slice(),
                    [
                        Token::Literal(b'A'),
                        Token::Match {
                            length: 258,
                            length_symbol: 285,
                            length_extra: 0,
                            length_extra_bits: 0,
                            ..
                        }
                    ]
                )
        ));
    }

    #[test]
    fn default_route_resegments_a_proven_match_without_changing_its_distance() {
        let input = [
            0x65, 0xc1, 0x31, 0x01, 0x00, 0x00, 0x00, 0xc2, 0xa0, 0x6c, 0xf4, 0x2f, 0xe5, 0x3f,
            0x41, 0x29, 0xa5, 0x94, 0x72, 0x06,
        ];
        let source = parse_stream(&input, 120).unwrap();
        assert_eq!(source.meaningful_bits, 156);
        let source_matches = source
            .blocks
            .iter()
            .flat_map(|block| block.tokens.iter())
            .filter_map(|token| match *token {
                Token::Match {
                    length, distance, ..
                } => Some((length, distance)),
                Token::Literal(_) => None,
            })
            .collect::<Vec<_>>();
        let mut expected_source_matches = vec![(16, 1); 6];
        expected_source_matches.push((17, 1));
        assert_eq!(source_matches, expected_source_matches);

        for strict in [true, false] {
            let options = Options {
                strict,
                ..Options::default()
            };
            let optimized = optimize_raw(&input, &options).unwrap();
            assert!(optimized.info.deflate_bits <= 151);
            assert!(optimized.info.deflate_bits < source.meaningful_bits);
            assert!(optimized.data.len() < input.len());

            let reparsed = parse_stream(&optimized.data, 120).unwrap();
            assert_eq!(reparsed.decoded_size, source.decoded_size);
            assert_eq!(reparsed.crc32, source.crc32);
            assert_eq!(reparsed.adler32, source.adler32);
            let tokens = reparsed
                .blocks
                .iter()
                .flat_map(|block| block.tokens.iter())
                .copied()
                .collect::<Vec<_>>();
            assert_eq!(
                tokens
                    .iter()
                    .filter(|token| matches!(token, Token::Literal(b'A')))
                    .count(),
                8
            );
            assert_eq!(
                tokens
                    .iter()
                    .filter(|token| {
                        matches!(
                            token,
                            Token::Match {
                                length: 16,
                                distance: 1,
                                length_symbol: 267,
                                ..
                            }
                        )
                    })
                    .count(),
                7
            );
            assert!(matches!(
                tokens.as_slice(),
                [
                    ..,
                    Token::Literal(b'A'),
                    Token::Match {
                        length: 16,
                        distance: 1,
                        length_symbol: 267,
                        ..
                    }
                ]
            ));

            let repeated = optimize_raw(&optimized.data, &options).unwrap();
            assert_eq!(repeated.info.deflate_bits, optimized.info.deflate_bits);
            assert_eq!(repeated.data, optimized.data);
        }
    }

    #[test]
    fn default_route_exact_prices_a_nonhighest_source_symbol_tie() {
        let input = [
            0x75, 0xc1, 0x41, 0x0d, 0x00, 0x00, 0x0c, 0x03, 0x21, 0x6d, 0xf8, 0x37, 0xb5, 0x7f,
            0x97, 0x03, 0xcb, 0xb2, 0x3c, 0x82, 0x20, 0x08, 0x0e,
        ];
        let source = parse_stream(&input, 166).unwrap();
        assert_eq!(source.meaningful_bits, 181);
        assert_eq!(source.blocks.len(), 1);
        assert_eq!(source.blocks[0].literal_frequencies[268], 1);
        assert_eq!(source.blocks[0].literal_frequencies[270], 4);
        assert_eq!(
            source.blocks[0]
                .tokens
                .iter()
                .filter(|token| {
                    matches!(
                        token,
                        Token::Match {
                            length: 16,
                            distance: 1,
                            ..
                        }
                    )
                })
                .count(),
            3
        );

        let optimized = optimize_raw(&input, &Options::default()).unwrap();
        assert_eq!(optimized.info.deflate_bits, 178);
        assert_eq!(optimized.data.len(), input.len());
        let reparsed = parse_stream(&optimized.data, 166).unwrap();
        assert_eq!(reparsed.decoded_size, source.decoded_size);
        assert_eq!(reparsed.crc32, source.crc32);
        assert_eq!(reparsed.adler32, source.adler32);
        assert_eq!(reparsed.blocks.len(), 1);
        assert_eq!(reparsed.blocks[0].literal_frequencies[268], 0);
        assert_eq!(reparsed.blocks[0].literal_frequencies[270], 4);
        assert_eq!(
            reparsed.blocks[0]
                .tokens
                .iter()
                .filter(|token| matches!(token, Token::Literal(b'A')))
                .count(),
            10
        );
        assert_eq!(
            reparsed.blocks[0]
                .tokens
                .iter()
                .filter(|token| {
                    matches!(
                        token,
                        Token::Match {
                            length: 16,
                            distance: 1,
                            length_symbol: 267,
                            length_extra: 1,
                            length_extra_bits: 1,
                            distance_symbol: 0,
                            distance_extra: 0,
                            distance_extra_bits: 0,
                        }
                    )
                })
                .count(),
            4
        );

        let repeated = optimize_raw(&optimized.data, &Options::default()).unwrap();
        assert_eq!(repeated.info.deflate_bits, optimized.info.deflate_bits);
        assert_eq!(repeated.data, optimized.data);
    }

    #[test]
    fn strict_mode_repairs_alias_and_single_distance_code_inputs() {
        // This valid compatibility-extension input is a relaxed fixed point:
        // its dynamic tree has one usable distance code and its final
        // length-258 match uses Defluff's non-standard symbol-284 alias.
        // Canonicalizing either detail costs bits, so this catches accidental
        // source reuse in strict mode.
        let input = [
            0xe5, 0xc0, 0x81, 0x00, 0x00, 0x00, 0x00, 0x80, 0x20, 0xb6, 0xfd, 0xa5, 0x06, 0xa9,
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0xbe, 0x01,
        ];
        let source = parse_stream(&input, 2_302).unwrap();
        assert!(stream_uses_258_alias(&source));
        assert!(source.blocks.iter().any(|block| {
            block
                .original_dynamic
                .as_ref()
                .is_some_and(|dynamic| !dynamic.has_two_usable_distance_codes())
        }));

        let strict = optimize_raw(&input, &Options::default()).unwrap();
        let strict_stream = parse_stream(&strict.data, 2_302).unwrap();
        assert_eq!(strict_stream.decoded_size, source.decoded_size);
        assert!(!stream_uses_258_alias(&strict_stream));
        let strict_dynamic: Vec<_> = strict_stream
            .blocks
            .iter()
            .filter_map(|block| block.original_dynamic.as_ref())
            .collect();
        // A cheaper fixed rewrite is also strictly compatible. If the
        // selected stream remains dynamic, all three alphabets must be
        // complete and its distance tree must contain two usable symbols.
        for dynamic in strict_dynamic {
            assert!(dynamic.has_strictly_compatible_huffman_codes());
            assert!(huffman_tree_shape_is_complete(&dynamic.literal_lengths));
            assert!(huffman_tree_shape_is_complete(&dynamic.distance_lengths));
            assert!(huffman_tree_shape_is_complete(&dynamic.code_length_lengths));
        }

        let relaxed_options = Options {
            strict: false,
            ..Options::default()
        };
        let relaxed = optimize_raw(&input, &relaxed_options).unwrap();
        let relaxed_stream = parse_stream(&relaxed.data, 2_302).unwrap();
        assert_eq!(relaxed_stream.decoded_size, source.decoded_size);
        assert!(relaxed.data.len() <= input.len());
        // Relaxed mode permits the source exceptions but may still select an
        // independently cheaper canonical fixed or dynamic representation.
        // Parsing and explicit alias-rewrite tests cover retention of those
        // extensions when they remain the winning spelling.
    }

    fn stream_uses_258_alias(stream: &ParsedStream) -> bool {
        stream.blocks.iter().any(|block| {
            block.tokens.iter().any(|token| {
                matches!(
                    token,
                    Token::Match {
                        length: 258,
                        length_symbol: 284,
                        length_extra: 31,
                        length_extra_bits: 5,
                        ..
                    }
                )
            })
        })
    }

    #[test]
    fn optimization_never_discovers_new_lz77_matches() {
        // A stored block has only literal source tokens. Even though this
        // deliberately repetitive payload would be easy to recompress with
        // new matches, Columbo may only choose a cheaper serialization of the
        // existing parse.
        let plain = vec![b'A'; 1_024];
        let length = plain.len() as u16;
        let mut input = vec![0x01]; // Final stored block, then byte alignment.
        input.extend_from_slice(&length.to_le_bytes());
        input.extend_from_slice(&(!length).to_le_bytes());
        input.extend_from_slice(&plain);

        let optimized = optimize_raw(&input, &Options::default()).unwrap();
        let reparsed = parse_stream(&optimized.data, plain.len() as u64).unwrap();

        assert_eq!(reparsed.decoded_size, plain.len() as u64);
        assert!(reparsed
            .blocks
            .iter()
            .flat_map(|block| block.tokens.iter())
            .all(|token| matches!(token, Token::Literal(_))));
    }

    #[test]
    fn timeout_still_returns_one_complete_stream() {
        // A final empty fixed block occupies ten meaningful bits. A zero
        // deadline skips optional searches, but must never truncate parsing
        // or emission halfway through that block.
        let input = [0x03, 0x00];
        let options = Options {
            timeout: Duration::ZERO,
            ..Options::default()
        };

        let optimized = optimize_raw(&input, &options).unwrap();
        assert_eq!(optimized.consumed, input.len());
        assert!(optimized.timed_out);

        let reparsed = parse_stream(&optimized.data, 1).unwrap();
        assert_eq!(reparsed.consumed, optimized.data.len());
        assert_eq!(reparsed.decoded_size, 0);
    }

    #[test]
    fn max_keeps_the_finished_default_floor_after_timeout() {
        let input = [0x03, 0x00];
        let default = optimize_raw(&input, &Options::default()).unwrap();
        let max_options = Options {
            exhaustive: true,
            timeout: Duration::ZERO,
            ..Options::default()
        };

        let maximum = optimize_raw(&input, &max_options).unwrap();
        assert!(maximum.timed_out);
        assert!(
            maximum.data.len() < default.data.len()
                || (maximum.data.len() == default.data.len()
                    && maximum.info.deflate_bits <= default.info.deflate_bits)
        );
    }

    #[test]
    fn shared_max_floor_still_returns_a_complete_stream() {
        let input = [0x03, 0x00];
        let options = Options {
            exhaustive: true,
            timeout: Duration::ZERO,
            ..Options::default()
        };

        let optimized = optimize_raw_prefix_with_floor(
            &input,
            &options,
            options.max_decoded_bytes,
            DefaultFloor::Shared,
        )
        .unwrap();
        assert!(optimized.timed_out);
        assert_eq!(optimized.consumed, input.len());

        let reparsed = parse_stream(&optimized.data, 1).unwrap();
        assert_eq!(reparsed.consumed, optimized.data.len());
        assert_eq!(reparsed.decoded_size, 0);
    }

    #[test]
    fn complete_then_bounded_floor_keeps_the_finished_default_at_zero_timeout() {
        let input = [0x03, 0x00];
        let default = optimize_raw(&input, &Options::default()).unwrap();
        let options = Options {
            exhaustive: true,
            timeout: Duration::ZERO,
            ..Options::default()
        };

        let maximum = optimize_raw_prefix_with_floor(
            &input,
            &options,
            options.max_decoded_bytes,
            DefaultFloor::CompleteThenBounded,
        )
        .unwrap();

        assert!(maximum.timed_out);
        assert!(
            maximum.data.len() < default.data.len()
                || (maximum.data.len() == default.data.len()
                    && maximum.info.deflate_bits <= default.info.deflate_bits)
        );
    }

    #[test]
    fn completed_png_floor_is_reused_without_rebuilding() {
        let input = [0x03, 0x00];
        let parsed = parse_stream(&input, 1).unwrap();
        let source = CandidateInput {
            compressed: &input,
            blocks: &parsed.blocks,
            meaningful_bits: parsed.meaningful_bits,
            decoded_limit: 1,
            identity: StreamIdentity {
                decoded_size: parsed.decoded_size,
                crc32: parsed.crc32,
                adler32: parsed.adler32,
            },
        };
        let options = Options::default();
        let completed =
            build_candidate(source, &options, DEFAULT_RAW_REPLAY_LIMIT, &mut || false).unwrap();
        let expected_data = completed.data.clone();
        let expected_bits = completed.bits;

        let reused = completed_or_bounded_floor(source, &options, Some(completed), &mut || {
            panic!("reusing a completed floor must not start another build")
        })
        .unwrap();

        assert_eq!(reused.data, expected_data);
        assert_eq!(reused.bits, expected_bits);
        assert_eq!(reused.route, "Normal floor");
    }

    #[test]
    fn bounded_parallel_floors_preserve_a_complete_stream() {
        // Two nonempty Huffman blocks activate both independent phase-one
        // routes. They share the parsed token/plain buffers and wall clock,
        // then rejoin before any deft4j refinement is considered.
        let (literal, _) = fixed_trees();
        let end = literal.code(256).unwrap();
        let mut writer = BitWriter::default();
        for (index, value) in b"ab".iter().copied().enumerate() {
            let code = literal.code(usize::from(value)).unwrap();
            writer.write(u32::from(index == 1), 1).unwrap();
            writer.write(1, 2).unwrap();
            writer.write(u32::from(code.code), code.length).unwrap();
            writer.write(u32::from(end.code), end.length).unwrap();
        }
        let input = writer.into_bytes();
        let source = parse_stream(&input, 2).unwrap();
        assert_eq!(source.source_block_count, 2);

        let options = Options {
            exhaustive: true,
            timeout: Duration::from_millis(100),
            ..Options::default()
        };
        let optimized = optimize_raw_prefix_with_floor(
            &input,
            &options,
            options.max_decoded_bytes,
            DefaultFloor::CompleteThenBounded,
        )
        .unwrap();
        let reparsed = parse_stream(&optimized.data, 2).unwrap();
        let plain: Vec<_> = reparsed
            .blocks
            .iter()
            .flat_map(|block| block.plain.iter().copied())
            .collect();

        assert_eq!(plain, b"ab");
        assert_eq!(optimized.consumed, input.len());
        assert!(
            optimized.data.len() < input.len()
                || (optimized.data.len() == input.len()
                    && optimized.info.deflate_bits <= source.meaningful_bits)
        );
    }

    #[test]
    fn compact_split_floor_gate_is_tightly_bounded() {
        let (literal, _) = fixed_trees();
        let value = literal.code(usize::from(b'a')).unwrap();
        let end = literal.code(256).unwrap();
        let mut writer = BitWriter::default();
        for block_index in 0..2 {
            writer.write(u32::from(block_index == 1), 1).unwrap();
            writer.write(1, 2).unwrap();
            for _ in 0..128 {
                writer.write(u32::from(value.code), value.length).unwrap();
            }
            writer.write(u32::from(end.code), end.length).unwrap();
        }
        let parsed = parse_stream(&writer.into_bytes(), 256).unwrap();
        assert!(compact_source_split_floor_eligible(
            parsed.decoded_size,
            &parsed.blocks
        ));
        assert!(!compact_source_split_floor_eligible(
            COMPACT_SPLIT_FLOOR_MAX_DECODED + 1,
            &parsed.blocks
        ));
        assert!(!compact_source_split_floor_eligible(
            parsed.decoded_size,
            &parsed.blocks[..1]
        ));

        let mut stored = parsed.blocks.clone();
        stored[0].source_type = SourceBlockType::Stored;
        assert!(!compact_source_split_floor_eligible(
            parsed.decoded_size,
            &stored
        ));

        let mut too_many_tokens = parsed.blocks.clone();
        too_many_tokens[0].tokens =
            vec![Token::Literal(b'a'); COMPACT_SPLIT_FLOOR_MAX_TOKENS].into();
        assert!(!compact_source_split_floor_eligible(
            parsed.decoded_size,
            &too_many_tokens
        ));

        let mut quad_block = parsed.blocks[0].clone();
        quad_block.source_type = SourceBlockType::Dynamic;
        quad_block.tokens = vec![Token::Literal(b'a'); COMPACT_QUAD_MIN_TOKENS].into();
        assert!(compact_quad_source_eligible(
            COMPACT_QUAD_MIN_COMPRESSED,
            quad_block.plain.len() as u64,
            std::slice::from_ref(&quad_block),
        ));
        assert!(!compact_quad_source_eligible(
            COMPACT_QUAD_MIN_COMPRESSED - 1,
            quad_block.plain.len() as u64,
            std::slice::from_ref(&quad_block),
        ));
        assert!(!compact_quad_source_eligible(
            COMPACT_QUAD_MIN_COMPRESSED,
            quad_block.plain.len() as u64,
            &[quad_block.clone(), quad_block],
        ));
    }

    #[test]
    fn bounded_generic_routes_preserve_complete_candidates() {
        // Two stored blocks have no deft4j or narrow source route. They make a
        // small generic-only stream whose floor and original-source max routes
        // can be checked independently after their parallel phase rejoins.
        let input = [
            0x00, 0x01, 0x00, 0xfe, 0xff, b'a', // Non-final stored block.
            0x01, 0x01, 0x00, 0xfe, 0xff, b'b', // Final stored block.
        ];
        let parsed = parse_stream(&input, 2).unwrap();
        assert_eq!(parsed.source_block_count, 2);
        assert!(!deft4j_source_route_eligible(
            &parsed.blocks,
            input.len(),
            true
        ));
        assert!(!narrow_source_route_eligible(&parsed.blocks, input.len()));

        let options = Options {
            exhaustive: true,
            timeout: Duration::MAX,
            ..Options::default()
        };
        let identity = StreamIdentity {
            decoded_size: parsed.decoded_size,
            crc32: parsed.crc32,
            adler32: parsed.adler32,
        };
        let source = CandidateInput {
            compressed: &input,
            blocks: &parsed.blocks,
            meaningful_bits: parsed.meaningful_bits,
            decoded_limit: 2,
            identity,
        };
        let deadline = Deadline {
            started: Instant::now(),
            duration: Duration::MAX,
            state: AtomicU8::new(0),
        };
        let progress = Progress::begin(
            &options,
            deadline.started,
            StreamProgress {
                blocks: parsed.source_block_count,
                compressed_bytes: input.len(),
                decoded_bytes: parsed.decoded_size,
                empty_blocks: parsed.source_empty_block_count,
                meaningful_bits: parsed.meaningful_bits,
                parse_elapsed: Duration::ZERO,
            },
        );
        let candidates =
            build_bounded_generic_max_candidates(source, &options, &deadline, progress, None)
                .unwrap();

        assert!(candidates.floor_seeded.is_some());
        assert!(candidates.source_max.is_some());
        assert_eq!(
            candidates
                .source_max
                .as_ref()
                .map(|candidate| candidate.route),
            Some("Columbo source max route")
        );
        assert!(candidates.suppress_later_source_max);
        assert!(!candidates.suppress_later_optional_routes);
        assert!(candidates.deft4j.is_none());
        assert!(candidates.narrow.is_none());
        for candidate in [
            candidates.floor.as_ref(),
            candidates.floor_seeded.as_ref(),
            candidates.source_max.as_ref(),
        ]
        .into_iter()
        .flatten()
        {
            let reparsed = parse_stream(&candidate.data, 2).unwrap();
            validate_replayed_stream(&reparsed, candidate.data.len(), identity).unwrap();
        }
        assert!(!deadline.was_triggered());
    }

    #[test]
    fn parallel_routes_require_small_input_and_model_sizes() {
        assert!(parallel_route_sizes_are_bounded(
            PARALLEL_ROUTE_MAX_COMPRESSED,
            1,
            1,
            1,
        ));
        assert!(!parallel_route_sizes_are_bounded(
            PARALLEL_ROUTE_MAX_COMPRESSED + 1,
            1,
            1,
            1,
        ));
        assert!(!parallel_route_sizes_are_bounded(
            1,
            PARALLEL_ROUTE_MAX_DECODED + 1,
            1,
            1,
        ));

        let oversized_token_count = PARALLEL_ROUTE_MAX_MODEL / std::mem::size_of::<Token>() + 1;
        assert!(!parallel_route_sizes_are_bounded(
            1,
            1,
            oversized_token_count,
            1,
        ));
    }

    #[test]
    fn route_errors_cancel_siblings_without_marking_a_timeout() {
        let deadline = Deadline {
            started: Instant::now(),
            duration: Duration::MAX,
            state: AtomicU8::new(0),
        };
        let failed: Result<()> =
            run_route_with_cancellation(&deadline, || Err(Error::new("synthetic route failure")));

        assert!(failed.is_err());
        assert!(deadline.route_should_stop());
        assert!(!deadline.was_triggered());

        let successful = Deadline {
            started: Instant::now(),
            duration: Duration::MAX,
            state: AtomicU8::new(0),
        };
        let completed: Result<()> = run_route_with_cancellation(&successful, || Ok(()));
        assert!(completed.is_ok());
        assert!(!successful.route_should_stop());
    }

    #[test]
    fn weak_deft4j_threshold_is_strictly_below_two_percent() {
        assert!(deft4j_gain_is_below(10_000, 9_801, 200));
        assert!(!deft4j_gain_is_below(10_000, 9_800, 200));
        assert!(!deft4j_gain_is_below(10_000, 9_000, 200));
    }

    #[test]
    fn bounded_png_max_policy_is_narrow_and_inclusive() {
        assert_eq!(
            bounded_png_max_policy(12, 0, 0, true, true),
            BoundedPngMaxPolicy::default()
        );
        for blocks in [13, 32] {
            assert_eq!(
                bounded_png_max_policy(blocks, 0, 0, true, true),
                BoundedPngMaxPolicy::LegacyGrouping
            );
        }
        assert_eq!(
            bounded_png_max_policy(33, 0, 0, true, true),
            BoundedPngMaxPolicy::default()
        );

        for blocks in [2, 4] {
            assert_eq!(
                bounded_png_max_policy(blocks, 0, 0, true, false),
                BoundedPngMaxPolicy::CoarseExpansion
            );
        }

        // Generic-only streams keep source max beside the floor lineage,
        // including when their block count overlaps the legacy band.
        for blocks in [2, 13] {
            assert_eq!(
                bounded_png_max_policy(blocks, 0, 0, false, false),
                BoundedPngMaxPolicy::GenericParallel
            );
        }

        // A trailing pair that accounts for every empty source block
        // identifies a source-list topology whose deft4j continuation can
        // outperform deterministic legacy grouping. Parsing removes those
        // no-op blocks from the model, but retains both counts for routing.
        assert_eq!(
            bounded_png_max_policy(15, 2, 2, true, true),
            BoundedPngMaxPolicy::Standard
        );

        // A source with interleaved empty blocks can also end in an empty
        // pair. Its useful route remains the legacy grouping.
        assert_eq!(
            bounded_png_max_policy(18, 19, 2, true, true),
            BoundedPngMaxPolicy::LegacyGrouping
        );
    }

    #[test]
    fn replay_validation_is_enforced_in_release_builds() {
        let parsed = parse_stream(&[0x03, 0x00], 1).unwrap();
        let empty = StreamIdentity {
            decoded_size: 0,
            crc32: 0,
            adler32: 1,
        };
        assert!(validate_replayed_stream(&parsed, 2, empty).is_ok());
        assert!(parse_validated_rewrite(&[0x03, 0x00], 1, empty).is_ok());

        // A complete stream followed by another byte is not a valid rewrite
        // of the exact candidate byte range.
        assert_eq!(
            parse_validated_rewrite(&[0x03, 0x00, 0x00], 1, empty)
                .unwrap_err()
                .message(),
            "internal error: rewritten Deflate stream changed decoded data"
        );

        let changed = StreamIdentity {
            decoded_size: 1,
            ..empty
        };
        assert_eq!(
            validate_replayed_stream(&parsed, 2, changed)
                .unwrap_err()
                .message(),
            "internal error: rewritten Deflate stream changed decoded data"
        );
    }
}
