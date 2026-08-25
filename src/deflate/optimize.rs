// SPDX-License-Identifier: MIT

use std::thread;
use std::time::{Duration, Instant};

use crate::progress::{
    reports_enabled, BalancedTreeProgress, BlockEncoding, BlockProgress, BlockReport,
    CandidateProgress, Progress, RouteProgress, SameDistanceProgress, StreamProgress,
    MAX_REPORTED_BLOCKS,
};
use crate::{Error, Options, Result};

use super::bitstream::BitWriter;
use super::block::{emit_block, plan_block, reusable_original_bits};
use super::deft4j::plan_source_blocks;
use super::header::{
    balanced_tree_opportunities, plan_bounded_depth_tree_candidate,
    plan_columbo_balanced_tree_candidate, plan_for_explicit_lengths,
    plan_rle_smoothed_tree_candidate, BalancedTreeOpportunities,
};
use super::model::{
    ParsedBlock, ParsedStream, PlannedBlock, Representation, SourceBlockType, Token,
};
use super::parse::{parse_stream, parsed_model_bytes};
use super::search::{
    compact_proven_submatch_route_eligible, improve_plan_with_header_aware_proven_composition,
    improve_plan_with_integrated_proven_floor, improve_plan_with_short_family_floor,
    plan_block_with_integrated_proven_search, rewrite_258_symbols, same_distance_opportunities,
    PROVEN_SUBMATCH_FULL_MATCH_LIMIT,
};
use super::stop::{timeout_grace, Deadline, RouteWindow, SearchStop};
use super::stream::{
    fragmented_collect_seed, plan_columbo_floor_seeded_bounded_grouping,
    plan_compact_source_split_floor, plan_compact_source_split_floor_until, plan_fragmented_replay,
    plan_integrated_proven_source_route, plan_proven_submatch_route, plan_source_no_split_route,
    plan_stream, plan_stream_from_established_floor, plan_stream_with_progress,
    plan_terminal_merge_route,
};

// Long source-block chains can need one pass to establish profitable adjacent
// groups, then two inexpensive passes over that much simpler block layout to
// settle their boundaries and tables. Every round below must strictly improve
// the complete stream, so the extra slot cannot oscillate or grow the output.
const DEFAULT_RAW_REPLAY_LIMIT: usize = 3;
// Max uses the sentinel below to resolve a proof-derived replay ceiling after
// its initial candidate is emitted. For an L-byte stream there are only 8L
// possible (byte length, meaningful-bit residue) scores no worse than it, and
// every accepted replay strictly improves that pair. This reaches a metric
// fixed point given sufficient time without an arbitrary eight-round cutoff.
const MAX_RAW_REPLAY_LIMIT: usize = usize::MAX;
const NARROW_SOURCE_LIST_MAX_BLOCKS: usize = 128;
const WEAK_DEFT4J_GAIN_BASIS_POINTS: u64 = 200;
// The no-split sibling is linear in the source block list, but each bounded
// per-block search retains route-local candidates. Pair a 1 MiB compressed
// ceiling with the 128-block ceiling below so this worker stays well inside
// the existing 64 MiB parallel-model class. No lower size bound is needed: on
// compact multi-block streams it cheaply reaches later blocks that a broad
// source-order graph may not visit before the deadline.
const NARROW_SOURCE_MAX_COMPRESSED: usize = 1_024 * 1_024;
// A completed compact deft4j-derived seed may expose useful eighth-position
// child splits after the timed route has settled its tokens and source joins.
// Keep this deterministic Columbo floor tightly bounded: it finishes at most
// seven structural prices per block and never starts another token search.
const COMPACT_SPLIT_FLOOR_MAX_COMPRESSED: usize = 16 * 1024;
const COMPACT_SPLIT_FLOOR_MAX_DECODED: u64 = 128 * 1024;
const COMPACT_SPLIT_FLOOR_MAX_BLOCKS: usize = 4;
const COMPACT_SPLIT_FLOOR_MAX_TOKENS: usize = 16 * 1024;
// The original Columbo C quad-lengthening move is a bounded one-block header
// floor. Its upper model limits avoid turning it into another general search;
// no corpus-derived lower size or token threshold is needed.
const COMPACT_TREE_MAX_COMPRESSED: usize = 8 * 1_024;
const COMPACT_TREE_MAX_DECODED: u64 = 128 * 1_024;
const COMPACT_TREE_MAX_TOKENS: usize = 4_096;
const RLE_SMOOTHED_TREE_FLOOR_MAX_BLOCKS: usize = 8;
const DEFAULT_STRICT_TREE_ROUNDS: usize = 4;
// A complementary source-root beam remains cheap on very small token graphs,
// even when proven feedback has already improved the ordinary floor. Retain
// both basins through a 2,048-token graph; above it, the extra beam competes
// materially with the richer floor-derived route.
const COMPACT_COMPLEMENTARY_SOURCE_MAX_TOKENS: usize = 2_048;
// The compact proven-feedback route is itself limited to 4,000 tokens and
// exact-prices only a capped set of candidate siblings. In the upper
// three-eighths of that work class, run one broad source-token owner instead
// of two long workers. Source max retains the wider state graph, and its
// deterministic finalization still applies proven feedback to a winning
// compact header rewrite. The restart still requires either a proved
// same-distance
// repartition or enough decoded data to span RFC 1951's maximum stored-block
// payload; without either topology, the floor lineage retains the only
// structurally motivated basin. Smaller graphs retain the cheaper proven
// lineage unless multiple independent repartitions justify source max.
const COMPACT_SINGLE_SOURCE_ROUTE_MAX_TOKENS: usize = 4_000;
const COMPACT_SINGLE_SOURCE_ROUTE_MIN_TOKENS: usize =
    COMPACT_SINGLE_SOURCE_ROUTE_MAX_TOKENS * 5 / 8;
const DEFLATE_MAX_STORED_BLOCK_PLAIN: u64 = 65_535;
// Parallel routes shorten a container's wall-clock search without making its
// peak memory proportional to every individually valid route budget. Larger
// streams retain the same candidates, but evaluate them serially.
const PARALLEL_ROUTE_MAX_COMPRESSED: usize = 8 * 1_024 * 1_024;
const PARALLEL_ROUTE_MAX_DECODED: u64 = 64 * 1_024 * 1_024;
const PARALLEL_ROUTE_MAX_MODEL: usize = 64 * 1_024 * 1_024;
// A small ordinary floor is cheap enough to establish before launching the
// heavier source graphs. Above this class, overlap preserves max-search wall
// time unless the floor's decoded work is itself large enough to cause working
// set contention. The match-work check at the call site also prebuilds floors
// whose source graph cannot reliably finish inside the initial four-fifths.
const PREBUILD_BOUNDED_FLOOR_MAX_DECODED: u64 = 768 * 1_024;
const CONCURRENT_BOUNDED_FLOOR_MAX_DECODED: u64 = 2 * 1_024 * 1_024;

/// Facts collected while decoding the source stream. Container handlers use
/// these values to validate their checksums without inflating a second time.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct RawInfo {
    pub(crate) crc32: u32,
    pub(crate) adler32: u32,
    pub(crate) size: u64,
    pub(crate) max_distance: u16,
    pub(crate) source_deflate_bits: u64,
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
/// A standalone stream uses [`DefaultFloor::Complete`] to try the ordinary
/// route before max-only work. A single scheduled PNG image uses
/// [`DefaultFloor::CompleteThenBounded`] for the same ordering before PNG's
/// bounded max routes. Both still observe the hard deadline: with sufficient
/// time they retain the ordinary result, while a short run may fall back to
/// any earlier complete candidate. Multi-stream containers use
/// [`DefaultFloor::Shared`] so one member cannot consume time needed by later
/// members. [`DefaultFloor::SharedExact`] keeps the same multi-stream schedule
/// but retains the complete ordinary feedback endpoint before Max-only work.
/// Multi-image APNG Default uses [`DefaultFloor::ApngDefault`] to keep the full
/// initial planner but leave repeated replay and feedback lineages to Max.
/// Multi-image APNG Max uses [`DefaultFloor::ApngMax`] to retain shared
/// container scheduling while running the full Max route set. Every Max floor
/// admits potentially useful direct deft4j source work; the floor enum no
/// longer acts as a permanent topology gate. [`DefaultFloor::Established`]
/// means the caller
/// already retains the complete input stream as its comparison floor, so
/// descendants can begin without rebuilding an ordinary candidate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DefaultFloor {
    Complete,
    CompleteThenBounded,
    Shared,
    SharedExact,
    ApngDefault,
    ApngMax,
    Established,
}

impl DefaultFloor {
    fn is_bounded(self) -> bool {
        !matches!(self, Self::Complete)
    }

    fn uses_bounded_png_routes(self) -> bool {
        matches!(self, Self::CompleteThenBounded)
    }
}

pub(crate) fn optimize_raw(input: &[u8], options: &Options) -> Result<RawOptimization> {
    let optimized = optimize_raw_prefix(input, options, options.max_decoded_bytes)?;
    if optimized.consumed != input.len() {
        return Err(Error::new("trailing data after Deflate stream"));
    }
    Ok(optimized)
}

/// Parse one complete raw stream without running any optimization routes.
///
/// Detailed container inspection uses this to locate concatenated GZIP member
/// trailers before the live header is printed. The normal optimization pass
/// still performs its own validation and retains the decoded model it plans.
pub(crate) fn inspect_raw_prefix(input: &[u8], decoded_limit: u64) -> Result<(usize, RawInfo)> {
    let parsed = parse_stream(input, decoded_limit)?;
    Ok((
        parsed.consumed,
        RawInfo {
            crc32: parsed.crc32,
            adler32: parsed.adler32,
            size: parsed.decoded_size,
            max_distance: parsed.max_distance,
            source_deflate_bits: parsed.meaningful_bits,
            deflate_bits: parsed.meaningful_bits,
            source_block_count: parsed.source_block_count,
            source_empty_block_count: parsed.source_empty_block_count,
        },
    ))
}

/// Report whether a PNG Max scheduler benefits from an early transformed
/// lineage beside its exact Default lineage.
///
/// A dense same-distance graph needs the reduced parent because the direct
/// bounded graph cannot enumerate every combination. A small multi-block
/// stream needs it for a different reason: exact Default is deliberately
/// established serially for this work class, so it can otherwise consume a
/// short Max allowance before any independent source route starts. Larger
/// multi-block floors already overlap those routes internally and must not
/// receive a redundant outer worker. The probe performs only the normal
/// bounded parse; optimization reparses and independently validates every
/// selectable output.
pub(crate) fn raw_source_benefits_from_early_max_lineage(
    input: &[u8],
    decoded_limit: u64,
) -> Result<bool> {
    let parsed = parse_stream(input, decoded_limit)?;
    if parsed.consumed != input.len() {
        return Err(Error::new("trailing data after Deflate stream"));
    }
    let dense_match_graph =
        source_run_match_count_exceeds(&parsed.blocks, PROVEN_SUBMATCH_FULL_MATCH_LIMIT);
    let nonempty_blocks = parsed
        .blocks
        .iter()
        .filter(|block| !block.plain.is_empty())
        .count();
    Ok(early_transformed_lineage_is_useful(
        nonempty_blocks,
        parsed.decoded_size,
        dense_match_graph,
    ))
}

fn early_transformed_lineage_is_useful(
    nonempty_blocks: usize,
    decoded_size: u64,
    dense_match_graph: bool,
) -> bool {
    dense_match_graph
        || (nonempty_blocks >= 2 && decoded_size <= PREBUILD_BOUNDED_FLOOR_MAX_DECODED)
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
    optimize_raw_prefix_with_floor_and_grace(
        input,
        options,
        decoded_limit,
        default_floor,
        timeout_grace(options.timeout),
    )
}

pub(crate) fn optimize_raw_prefix_with_floor_and_grace(
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
    let parsed = parse_stream(input, decoded_limit.min(options.max_decoded_bytes))?;
    // Avoid even an extra clock read in ordinary speed-first runs.
    let reporting = reports_enabled(options);
    let parse_elapsed = if reporting {
        started.elapsed()
    } else {
        std::time::Duration::ZERO
    };
    let mut blocks = parsed.blocks;
    let source_report = capture_source_block_report(&blocks, parsed.source_block_count, reporting);
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
        source_report,
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
        let mut tree_opportunities = BalancedTreeOpportunities::default();
        for block in &blocks {
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
    progress.routes();
    let deadline = Deadline::with_grace(started, options.timeout, grace);

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
    let source_nonempty_blocks = blocks
        .iter()
        .filter(|block| !block.plain.is_empty())
        .count();
    // A single scheduled PNG promises that max retains the complete ordinary
    // result. Prebuild compact, very large, one-block, or match-dense floors;
    // their exact Default route either is a cheap dependency or cannot
    // reliably finish inside the concurrent phase's reserved four-fifths.
    // Medium multi-block floors instead remain in the existing parallel phase,
    // where their completed ordinary candidate is still retained while max
    // preserves enough wall time for independent source routes.
    let prebuild_floor_first = options.exhaustive
        && match default_floor {
            DefaultFloor::CompleteThenBounded => {
                prebuild_bounded_floor(source_nonempty_blocks, parsed.decoded_size)
                    || source_run_match_count_exceeds(&blocks, PROVEN_SUBMATCH_FULL_MATCH_LIMIT)
            }
            DefaultFloor::Shared
            | DefaultFloor::SharedExact
            | DefaultFloor::ApngDefault
            | DefaultFloor::ApngMax => true,
            DefaultFloor::Established => false,
            DefaultFloor::Complete => false,
        };
    let guaranteed_floor_step =
        prebuild_floor_first.then(|| progress.start("Normal comparison floor"));
    let mut complete_default_candidate = None;
    let mut guaranteed_floor_candidate = if default_floor == DefaultFloor::Established {
        Some(established_floor_candidate(source)?)
    } else if prebuild_floor_first {
        Some(
            if matches!(
                default_floor,
                DefaultFloor::CompleteThenBounded | DefaultFloor::SharedExact
            ) {
                let floors =
                    build_complete_default_floor_candidate(source, options, &deadline, progress)?;
                complete_default_candidate = Some(floors.complete);
                floors.max_seed
            } else {
                build_bounded_floor_candidate(source, options, &mut deadline.hard_stop())?
            },
        )
    } else {
        None
    };
    if let Some(step) = guaranteed_floor_step {
        let reported_floor = complete_default_candidate
            .as_ref()
            .or(guaranteed_floor_candidate.as_ref());
        step.finish(reported_floor.map(|candidate| {
            candidate_progress(
                candidate,
                source.meaningful_bits,
                candidate.is_strictly_smaller_than_source(source),
            )
        }));
    }
    let compact_tree_eligible = options.exhaustive
        && default_floor.uses_bounded_png_routes()
        && compact_balanced_tree_source_eligible(original.len(), parsed.decoded_size, &blocks);
    // The fixed-point and nearby-count smoothers each use a seven-pair exact
    // tree frontier, but keep their terminal reparse inside the same compact
    // memory/work class. Unlike the PNG-specific balanced-tree routes this can
    // also help standalone streams, and the completed output rather than the
    // source decides whether its final block topology is applicable.
    let smoothed_tree_eligible = !options.timeout.is_zero()
        && original.len() <= COMPACT_TREE_MAX_COMPRESSED
        && parsed.decoded_size <= COMPACT_TREE_MAX_DECODED;
    // The general depth floor is a linear terminal pass over any completed
    // stream. Admission depends on available route time, not on a
    // corpus-trained size band; exact whole-stream pricing decides acceptance.
    let bounded_depth_tree_eligible = !options.timeout.is_zero();
    let compact_proven_feedback_eligible = options.exhaustive
        && default_floor.uses_bounded_png_routes()
        && source.blocks.len() == 1
        && compact_proven_submatch_route_eligible(
            &source.blocks[0].tokens,
            source.blocks[0].plain.len(),
        );
    // The direct source graph is a Max quality route, not a PNG-only
    // specialization. Container scheduling may decide when it runs, but no
    // accepted Huffman/stored topology is permanently excluded: otherwise a
    // longer timeout could never recover a compatible deft4j endpoint.
    let deft4j_eligible = options.exhaustive && deft4j_source_route_eligible(&blocks);

    // Bounded PNG routes share the parsed stream and one deadline. Streams
    // without a specialized source sibling run source max beside the floor
    // lineage; multi-block floors may also receive one deterministic Columbo
    // grouping pass. Standalone streams keep the same route order without
    // overlapping their larger working sets.
    let run_narrow_source = options.exhaustive
        && default_floor.uses_bounded_png_routes()
        && narrow_source_route_eligible(&blocks, original.len());
    let parallel_routes =
        options.exhaustive && default_floor.is_bounded() && parallel_route_is_bounded(source);
    let png_policy = if default_floor.uses_bounded_png_routes() && parallel_routes {
        let floor_exposes_new_states = guaranteed_floor_candidate
            .as_ref()
            .is_some_and(|floor| floor_exposes_new_search_states(&blocks, &floor.plans));
        bounded_png_max_policy(
            source_nonempty_blocks,
            floor_exposes_new_states,
            deft4j_eligible,
            run_narrow_source,
        )
    } else {
        BoundedPngMaxPolicy::default()
    };
    let bounded_step = (options.exhaustive && default_floor.is_bounded())
        .then(|| progress.start("Bounded comparison routes"));
    let bounded_candidates = if options.exhaustive && default_floor.is_bounded() {
        // A one-block topology probe has already finished this exact floor.
        // Reuse it; multi-block streams build the same floor concurrently with
        // their independent source routes inside the bounded phase.
        let completed_floor = guaranteed_floor_candidate.take();
        match png_policy {
            BoundedPngMaxPolicy::GenericParallel => build_bounded_generic_max_candidates(
                source,
                options,
                &deadline,
                progress,
                completed_floor,
            )?,
            BoundedPngMaxPolicy::Standard | BoundedPngMaxPolicy::FloorExpansion => {
                let run_deft4j = deft4j_eligible && deadline.can_start_route();
                let run_source_max = parallel_routes
                    && png_policy == BoundedPngMaxPolicy::FloorExpansion
                    && bounded_parallel_source_max_work_class(source);
                let run_proven_feedback = run_source_max && compact_proven_feedback_eligible;
                build_bounded_phase_candidates(
                    source,
                    options,
                    png_policy == BoundedPngMaxPolicy::FloorExpansion,
                    run_deft4j,
                    run_narrow_source,
                    run_source_max,
                    run_proven_feedback,
                    parallel_routes,
                    &deadline,
                    progress,
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
    let mut proven_feedback_candidate = bounded_candidates.proven_feedback;
    let mut suppress_later_source_max = bounded_candidates.suppress_later_source_max;
    let suppress_later_optional_routes = bounded_candidates.suppress_later_optional_routes;
    let completed_compact_split_parent = bounded_candidates.completed_compact_split_parent;
    if let Some(step) = bounded_step {
        step.finish_phase();
        for (name, candidate) in [
            ("Normal floor", bounded_floor_candidate.as_ref()),
            ("Columbo floor-seeded", floor_seeded_candidate.as_ref()),
            ("deft4j-derived source", deft4j_candidate.as_ref()),
            ("No-split source", narrow_candidate.as_ref()),
            ("Columbo source max", source_max_candidate.as_ref()),
            (
                "Columbo proven-feedback",
                proven_feedback_candidate.as_ref(),
            ),
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
    // A compact one-block stream has one additional fixed point when proven
    // resegmentation feeds later table feedback before the normal endpoint
    // ordering. Price that bounded sibling before the general source-max graph
    // can consume the shared deadline. The completed normal floor remains an
    // independent fallback.
    let run_proven_feedback = compact_proven_feedback_eligible
        && proven_feedback_candidate.is_none()
        && deadline.can_start_route();
    let proven_feedback_step =
        run_proven_feedback.then(|| progress.start("Columbo proven-feedback floor"));
    if run_proven_feedback {
        proven_feedback_candidate =
            build_compact_proven_feedback_candidate(source, options, &mut deadline.hard_stop())?;
    }
    if let Some(step) = proven_feedback_step {
        step.finish(proven_feedback_candidate.as_ref().map(|candidate| {
            candidate_progress(
                candidate,
                source.meaningful_bits,
                candidate.is_strictly_smaller_than_source(source),
            )
        }));
    }
    // The completed compact routes provide a direct scheduling signal for
    // source max. If proven-before-feedback supplies only a bit-level win,
    // continue that state order in the one expensive beam to seek the next
    // byte boundary. Once that bounded lineage already wins a byte, retain it
    // and spend the beam on complementary ordinary states instead. A tie also
    // selects ordinary states. Larger blocks use the integrated order
    // independently inside the stream planner.
    let integrated_compact_source_max = proven_feedback_candidate
        .as_ref()
        .zip(bounded_floor_candidate.as_ref())
        .is_some_and(|(proven, normal)| {
            proven.data.len() == normal.data.len() && proven.bits < normal.bits
        });
    if let Some(proven_feedback) = proven_feedback_candidate {
        replace_optional_if_smaller(&mut bounded_floor_candidate, proven_feedback);
    }
    let seed_weak_deft4j = default_floor.uses_bounded_png_routes()
        && deft4j_candidate.as_ref().is_some_and(|deft4j| {
            has_multiple_nonempty_blocks(&blocks)
                && gain_is_below(
                    parsed.meaningful_bits,
                    deft4j.bits,
                    WEAK_DEFT4J_GAIN_BASIS_POINTS,
                )
        });
    let run_compact_split_floor = default_floor.uses_bounded_png_routes() && seed_weak_deft4j;
    // Prepare the exact structural siblings before choosing which route gets
    // the remaining time. A weak direct gain admits compact-split inspection,
    // but it does not prove that any parent satisfies that route's bounded
    // topology and work model. Retaining these prepared seeds also avoids
    // reparsing the same candidates after the scheduling decision.
    let (
        mut compact_split_normal_seed,
        mut compact_split_seeded_seed,
        mut compact_split_deft4j_seed,
    ) = if run_compact_split_floor {
        // Split pricing is not monotone in the parent stream's encoded size:
        // independently rewritten block topologies can have opposite local
        // ordering after new cuts are inserted. Preserve each distinct parent.
        let normal_parent = bounded_floor_candidate.as_ref().filter(|candidate| {
            !compact_split_parent_is_completed(candidate, completed_compact_split_parent.as_deref())
        });
        let seeded_parent = floor_seeded_candidate.as_ref().filter(|seeded| {
            !compact_split_parent_is_completed(seeded, completed_compact_split_parent.as_deref())
                && normal_parent.map_or(true, |normal| normal.data != seeded.data)
        });
        let deft4j_parent = deft4j_candidate.as_ref().filter(|deft4j| {
            !compact_split_parent_is_completed(deft4j, completed_compact_split_parent.as_deref())
                && [normal_parent, seeded_parent]
                    .into_iter()
                    .flatten()
                    .all(|seed| seed.data != deft4j.data)
        });
        (
            normal_parent
                .map(|candidate| {
                    prepare_compact_source_split_seed(candidate, decoded_limit, identity)
                })
                .transpose()?
                .flatten(),
            seeded_parent
                .map(|candidate| {
                    prepare_compact_source_split_seed(candidate, decoded_limit, identity)
                })
                .transpose()?
                .flatten(),
            deft4j_parent
                .map(|candidate| {
                    prepare_compact_source_split_seed(candidate, decoded_limit, identity)
                })
                .transpose()?
                .flatten(),
        )
    } else {
        (None, None, None)
    };
    let compact_split_pending = [
        compact_split_normal_seed.as_ref(),
        compact_split_seeded_seed.as_ref(),
        compact_split_deft4j_seed.as_ref(),
    ]
    .into_iter()
    .any(|seed| seed.is_some());
    let mut compact_split_attempted = false;
    let mut compact_split_candidate = None;
    if let Some(floor) = &mut bounded_floor_candidate {
        // The later deft4j refinement needs only the encoded floor for its
        // strict comparison. Release transformed floor plans before it
        // reparses another complete candidate.
        floor.plans.clear();
    }
    // A completed floor-seeded max route may end at a different header/token
    // fixed point from source max. Finish the same bounded tree cleanup on
    // that exact parent before releasing it. This is deterministic finalization
    // of an already completed candidate, so it remains valid after the soft
    // deadline and cannot discard the parent when no balanced-tree improvement
    // exists.
    if compact_tree_eligible {
        if let Some(seeded) = &mut floor_seeded_candidate {
            // A one-block source may become a multi-block floor after the max
            // descendant. Those new boundaries admit the same deterministic
            // split floor used for compact source lists, and the original
            // one-block routes cannot reconstruct that parent state.
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
    if let Some(seeded) = &mut floor_seeded_candidate {
        seeded.plans.clear();
    }
    // Continue the strongest unfinished dependent lineage before starting
    // refinements from weaker parents. This is a score-ordered search rule,
    // not a size or corpus gate: the retained incumbent protects every other
    // complete result, and sufficient time still reaches the later siblings.
    // It also avoids waiting for an independent source worker and then
    // spending the final allowance refining a candidate already behind the
    // floor-seeded endpoint.
    //
    // An admitted compact split is an independent bounded sibling, but merely
    // being eligible does not predict that it will improve its parent. When
    // the parsed model permits route parallelism, overlap it with this
    // continuation so neither speculative route can starve the other. A
    // serial work class retains the material-gain priority rule below.
    let overlap_compact_split_with_continuation = compact_split_pending && parallel_routes;
    let continue_best_floor_seeded = floor_seeded_candidate.as_ref().is_some_and(|seeded| {
        !seeded.max_planner_is_stable
            && bounded_floor_candidate.as_ref().is_some_and(|floor| {
                seeded.is_strictly_smaller_than(floor)
                    && floor_seeded_priority_with_structural_sibling(
                        floor.bits,
                        seeded.bits,
                        compact_split_pending,
                        overlap_compact_split_with_continuation,
                    )
            })
            && [
                deft4j_candidate.as_ref(),
                narrow_candidate.as_ref(),
                source_max_candidate.as_ref(),
            ]
            .into_iter()
            .flatten()
            .all(|other| !other.is_strictly_smaller_than(seeded))
    }) && deadline.can_start_route();
    // An encoded-size lead does not dominate the endpoint of a different
    // planner topology. In the bounded parallel work class, keep the
    // independent deft4j-derived refinement live beside the best floor-seeded
    // continuation. Otherwise a long fixed-point continuation can consume
    // every larger deadline without ever admitting the complementary route.
    // This uses the same at-most-three-worker envelope as the historical
    // source/deft4j/compact phase and does not add wall-clock budget.
    let overlap_deft4j_refinement_with_continuation = independent_deft4j_refinement_can_overlap(
        continue_best_floor_seeded,
        default_floor.uses_bounded_png_routes(),
        parallel_routes,
        seed_weak_deft4j || deft4j_candidate.is_some(),
    );
    let floor_seeded_step =
        continue_best_floor_seeded.then(|| progress.start("Columbo floor-seeded continuation"));
    let continuation_split_step = (continue_best_floor_seeded
        && overlap_compact_split_with_continuation)
        .then(|| progress.start("Columbo compact split floor"));
    let continuation_deft4j_step = overlap_deft4j_refinement_with_continuation
        .then(|| progress.start("deft4j-derived refinement"));
    let mut floor_seeded_changed = false;
    let mut deft4j_refinement_completed = false;
    if continue_best_floor_seeded {
        let seeded = floor_seeded_candidate
            .as_mut()
            .expect("continuation requires a floor-seeded candidate");
        if overlap_deft4j_refinement_with_continuation {
            let (refined, split, refinement_completed) = thread::scope(
                |scope| -> Result<(Option<Candidate>, Option<Candidate>, bool)> {
                    let refinement_worker = thread::Builder::new()
                        .name("columbo-deft4j-continuation".into())
                        .spawn_scoped(scope, || {
                            run_route_with_cancellation(&deadline, || {
                                refine_bounded_deft4j_lineage(
                                    source,
                                    options,
                                    decoded_limit,
                                    identity,
                                    &deadline,
                                    bounded_floor_candidate.as_ref(),
                                    narrow_candidate.as_ref(),
                                    seed_weak_deft4j,
                                    false,
                                    &mut deft4j_candidate,
                                )
                            })
                        });
                    let Ok(refinement_worker) = refinement_worker else {
                        // Retain both complete parents and let the ordinary
                        // serial phase below run the independent refinement.
                        return Ok((None, None, false));
                    };
                    let split_worker = overlap_compact_split_with_continuation
                        .then(|| {
                            thread::Builder::new()
                                .name("columbo-compact-split-continuation".into())
                                .spawn_scoped(scope, || {
                                    run_route_with_cancellation(&deadline, || {
                                        build_prepared_compact_source_split_floors(
                                            [
                                                compact_split_normal_seed.as_ref(),
                                                compact_split_seeded_seed.as_ref(),
                                                compact_split_deft4j_seed.as_ref(),
                                            ],
                                            options,
                                            decoded_limit,
                                            identity,
                                            &deadline,
                                        )
                                    })
                                })
                                .ok()
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
                    match refinement_worker.join() {
                        Ok(result) => result,
                        Err(payload) => std::panic::resume_unwind(payload),
                    }?;
                    let split = match split_worker {
                        Some(worker) => match worker.join() {
                            Ok(result) => result,
                            Err(payload) => std::panic::resume_unwind(payload),
                        }?,
                        None if overlap_compact_split_with_continuation => {
                            // A failed split-worker spawn retains the bounded
                            // synchronous fallback used by the prior schedule.
                            build_prepared_compact_source_split_floors(
                                [
                                    compact_split_normal_seed.as_ref(),
                                    compact_split_seeded_seed.as_ref(),
                                    compact_split_deft4j_seed.as_ref(),
                                ],
                                options,
                                decoded_limit,
                                identity,
                                &deadline,
                            )?
                        }
                        None => None,
                    };
                    Ok((Some(refined?), split, true))
                },
            )?;
            deft4j_refinement_completed = refinement_completed;
            if refinement_completed && overlap_compact_split_with_continuation {
                compact_split_attempted = true;
                compact_split_normal_seed = None;
                compact_split_seeded_seed = None;
                compact_split_deft4j_seed = None;
            }
            if let Some(split) = split {
                replace_optional_if_smaller(&mut compact_split_candidate, split);
            }
            if let Some(refined) = refined {
                floor_seeded_changed = seeded.replace_if_smaller(refined);
            }
        } else if overlap_compact_split_with_continuation {
            let (refined, split) =
                thread::scope(|scope| -> Result<(Option<Candidate>, Option<Candidate>)> {
                    let split_worker = thread::Builder::new()
                        .name("columbo-compact-split-continuation".into())
                        .spawn_scoped(scope, || {
                            run_route_with_cancellation(&deadline, || {
                                build_prepared_compact_source_split_floors(
                                    [
                                        compact_split_normal_seed.as_ref(),
                                        compact_split_seeded_seed.as_ref(),
                                        compact_split_deft4j_seed.as_ref(),
                                    ],
                                    options,
                                    decoded_limit,
                                    identity,
                                    &deadline,
                                )
                            })
                        });
                    let Ok(split_worker) = split_worker else {
                        // Thread exhaustion must not discard the independent
                        // structural sibling. Finish it on this thread and
                        // retain the complete seeded parent as the fallback.
                        return build_prepared_compact_source_split_floors(
                            [
                                compact_split_normal_seed.as_ref(),
                                compact_split_seeded_seed.as_ref(),
                                compact_split_deft4j_seed.as_ref(),
                            ],
                            options,
                            decoded_limit,
                            identity,
                            &deadline,
                        )
                        .map(|split| (None, split));
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
                    let split = match split_worker.join() {
                        Ok(result) => result,
                        Err(payload) => std::panic::resume_unwind(payload),
                    }?;
                    Ok((Some(refined?), split))
                })?;
            compact_split_attempted = true;
            compact_split_normal_seed = None;
            compact_split_seeded_seed = None;
            compact_split_deft4j_seed = None;
            if let Some(split) = split {
                replace_optional_if_smaller(&mut compact_split_candidate, split);
            }
            if let Some(refined) = refined {
                floor_seeded_changed = seeded.replace_if_smaller(refined);
            }
        } else {
            let refined = refine_with_max_planner(
                seeded,
                options,
                decoded_limit,
                identity,
                &mut deadline.hard_stop(),
            )?;
            floor_seeded_changed = seeded.replace_if_smaller(refined);
        }
    }
    if let Some(step) = floor_seeded_step {
        step.finish(floor_seeded_candidate.as_ref().map(|seeded| {
            candidate_progress(
                seeded,
                source.meaningful_bits,
                seeded.is_strictly_smaller_than_source(source),
            )
        }));
    }
    if let Some(step) = continuation_split_step {
        step.finish(compact_split_candidate.as_ref().map(|candidate| {
            candidate_progress(
                candidate,
                source.meaningful_bits,
                candidate.is_strictly_smaller_than_source(source),
            )
        }));
    }
    if let Some(step) = continuation_deft4j_step {
        step.finish(deft4j_candidate.as_ref().map(|candidate| {
            candidate_progress(
                candidate,
                source.meaningful_bits,
                candidate.is_strictly_smaller_than_source(source),
            )
        }));
    }
    if floor_seeded_changed && run_compact_split_floor {
        // Continuation can emit a new topology. Refresh only that changed
        // parent; the normal and direct deft4j seeds above remain exact and
        // must not be reparsed. Exact identity with either seed proves that
        // its structural work is already represented.
        compact_split_seeded_seed = floor_seeded_candidate
            .as_ref()
            .filter(|seeded| {
                !compact_split_parent_is_completed(
                    seeded,
                    completed_compact_split_parent.as_deref(),
                ) && [
                    compact_split_normal_seed.as_ref(),
                    compact_split_deft4j_seed.as_ref(),
                ]
                .into_iter()
                .flatten()
                .all(|seed| seed.data != seeded.data)
            })
            .map(|seeded| prepare_compact_source_split_seed(seeded, decoded_limit, identity))
            .transpose()?
            .flatten();
    }
    let run_bounded_refinement = default_floor.is_bounded() && deadline.can_start_route();
    let compact_split_work_possible = run_bounded_refinement
        && ([
            compact_split_normal_seed.as_ref(),
            compact_split_seeded_seed.as_ref(),
            compact_split_deft4j_seed.as_ref(),
        ]
        .into_iter()
        .any(|seed| seed.is_some())
            // Refinement can expose a distinct eligible parent even when none
            // of the early encodings qualify. Use the same bounded source work
            // model that admits that dependency; larger ineligible streams
            // should not display an idle compact-split route.
            || (run_compact_split_floor && compact_dependent_deft4j_work_class(source)))
        // If no early split ran, the hard-deadline fallback below may still
        // finish one parent after the soft route window closes.
        || (run_compact_split_floor && !compact_split_attempted && !deadline.expired());
    let compact_split_step =
        compact_split_work_possible.then(|| progress.start("Columbo compact split floor"));
    // The direct no-split walk is not idempotent across an emitted token or
    // block rewrite: its new parent can expose another cumulative-pruning or
    // adjacent-merge choice. Continue that dependency once when it is a
    // non-dominated complete result. This is score ordered rather than tied to
    // a corpus shape, and source max remains available later if time remains.
    // Prefer the dependent continuation to a simultaneous source-max worker so
    // the bounded phase retains its existing three-worker memory envelope.
    let continue_best_narrow = run_bounded_refinement
        && narrow_candidate.as_ref().is_some_and(|narrow| {
            changed_narrow_parent_should_continue(
                candidate_exposes_new_parent(narrow, source),
                narrow,
                source,
                &[
                    bounded_floor_candidate.as_ref(),
                    floor_seeded_candidate.as_ref(),
                    deft4j_candidate.as_ref(),
                    source_max_candidate.as_ref(),
                    complete_default_candidate.as_ref(),
                ],
            )
        })
        && deadline.can_start_route();
    let narrow_continuation_step =
        continue_best_narrow.then(|| progress.start("Columbo no-split continuation"));
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
    let refinement_step = (run_bounded_refinement
        && !deft4j_refinement_completed
        && (seed_weak_deft4j || deft4j_candidate.is_some()))
    .then(|| progress.start("deft4j-derived refinement"));
    if run_bounded_refinement {
        let run_concurrent_source_max = options.exhaustive
            && default_floor.uses_bounded_png_routes()
            && parallel_routes
            && source_max_candidate.is_none()
            && !suppress_later_source_max
            && !continue_best_narrow
            && deadline.can_start_route();
        let run_concurrent_narrow = continue_best_narrow;
        let run_concurrent_compact_split = compact_split_normal_seed.is_some()
            || compact_split_seeded_seed.is_some()
            || compact_split_deft4j_seed.is_some();
        let BoundedFollowUpCandidates {
            source_max: concurrent_source_max,
            attempted_compact_split,
            compact_split: concurrent_compact_split,
            narrow: concurrent_narrow,
        } = if run_concurrent_source_max || run_concurrent_narrow || run_concurrent_compact_split {
            thread::scope(|scope| -> Result<BoundedFollowUpCandidates> {
                let source_worker = run_concurrent_source_max
                    .then(|| {
                        thread::Builder::new()
                            .name("columbo-source-max-follow-up".into())
                            .spawn_scoped(scope, || {
                                run_route_with_cancellation(&deadline, || {
                                    build_source_max_candidate(
                                        source,
                                        options,
                                        progress,
                                        &deadline,
                                        integrated_compact_source_max,
                                        &mut deadline.hard_stop(),
                                    )
                                })
                            })
                            .ok()
                    })
                    .flatten();
                let compact_worker = run_concurrent_compact_split
                    .then(|| {
                        thread::Builder::new()
                            .name("columbo-compact-split-lineages".into())
                            .spawn_scoped(scope, || {
                                run_route_with_cancellation(&deadline, || {
                                    build_prepared_compact_source_split_floors(
                                        [
                                            compact_split_normal_seed.as_ref(),
                                            compact_split_seeded_seed.as_ref(),
                                            compact_split_deft4j_seed.as_ref(),
                                        ],
                                        options,
                                        decoded_limit,
                                        identity,
                                        &deadline,
                                    )
                                })
                            })
                            .ok()
                    })
                    .flatten();
                let narrow_worker = run_concurrent_narrow
                    .then(|| {
                        thread::Builder::new()
                            .name("columbo-no-split-continuation".into())
                            .spawn_scoped(scope, || {
                                run_route_with_cancellation(&deadline, || {
                                    let narrow = narrow_candidate
                                        .as_ref()
                                        .expect("scheduled no-split continuation");
                                    let mut route_stop = deadline.hard_stop();
                                    let mut refinement_stop = deadline.hard_stop();
                                    refine_with_no_split_route(
                                        narrow,
                                        options,
                                        decoded_limit,
                                        identity,
                                        &mut route_stop,
                                        &mut refinement_stop,
                                    )
                                })
                            })
                            .ok()
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
                        &deadline,
                        bounded_floor_candidate.as_ref(),
                        narrow_candidate.as_ref(),
                        seed_weak_deft4j,
                        run_concurrent_compact_split,
                        &mut deft4j_candidate,
                    )
                };
                if refinement.is_err() {
                    deadline.cancel_routes();
                }
                // A genuinely different refined topology is another
                // split parent. Price it while source max is still
                // running when soft time remains, or unconditionally
                // when it strictly improves every early parent.
                // Exact encoded identity proves duplicate work.
                let dependent_split = if refinement.is_ok() && run_compact_split_floor {
                    deft4j_candidate.as_ref().and_then(|deft4j| {
                        let duplicates_early_seed = [
                            compact_split_normal_seed.as_ref(),
                            compact_split_seeded_seed.as_ref(),
                            compact_split_deft4j_seed.as_ref(),
                        ]
                        .into_iter()
                        .flatten()
                        .any(|seed| seed.data == deft4j.data)
                            || compact_split_parent_is_completed(
                                deft4j,
                                completed_compact_split_parent.as_deref(),
                            );
                        let improves_early_seeds = [
                            compact_split_normal_seed.as_ref(),
                            compact_split_seeded_seed.as_ref(),
                            compact_split_deft4j_seed.as_ref(),
                        ]
                        .into_iter()
                        .flatten()
                        .all(|seed| {
                            is_strictly_better(
                                deft4j.data.len(),
                                deft4j.bits,
                                seed.data.len(),
                                seed.bits,
                            )
                        });
                        (!duplicates_early_seed
                            && (improves_early_seeds || deadline.can_start_route()))
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
                    Some(worker) => match worker.join() {
                        Ok(result) => Some(result?),
                        Err(payload) => std::panic::resume_unwind(payload),
                    },
                    None => None,
                };
                let narrow_continuation = match narrow_worker {
                    Some(worker) => match worker.join() {
                        Ok(result) => result,
                        Err(payload) => std::panic::resume_unwind(payload),
                    },
                    None if run_concurrent_narrow => {
                        let narrow = narrow_candidate
                            .as_ref()
                            .expect("scheduled no-split continuation");
                        let mut route_stop = deadline.hard_stop();
                        let mut refinement_stop = deadline.hard_stop();
                        refine_with_no_split_route(
                            narrow,
                            options,
                            decoded_limit,
                            identity,
                            &mut route_stop,
                            &mut refinement_stop,
                        )
                    }
                    None => Ok(None),
                }?;
                let attempted_compact_split =
                    run_concurrent_compact_split || dependent_split.is_some();
                // A failed worker spawn falls back to the same bounded
                // pass on this thread while source max is still joined.
                let early_split = match compact_worker {
                    Some(worker) => match worker.join() {
                        Ok(result) => result,
                        Err(payload) => std::panic::resume_unwind(payload),
                    },
                    None => build_prepared_compact_source_split_floors(
                        [
                            compact_split_normal_seed.as_ref(),
                            compact_split_seeded_seed.as_ref(),
                            compact_split_deft4j_seed.as_ref(),
                        ],
                        options,
                        decoded_limit,
                        identity,
                        &deadline,
                    ),
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
            })?
        } else {
            if !deft4j_refinement_completed {
                refine_bounded_deft4j_lineage(
                    source,
                    options,
                    decoded_limit,
                    identity,
                    &deadline,
                    bounded_floor_candidate.as_ref(),
                    narrow_candidate.as_ref(),
                    seed_weak_deft4j,
                    false,
                    &mut deft4j_candidate,
                )?;
            }
            BoundedFollowUpCandidates::default()
        };
        compact_split_attempted |= attempted_compact_split;
        if let Some(concurrent_compact_split) = concurrent_compact_split {
            replace_optional_if_smaller(&mut compact_split_candidate, concurrent_compact_split);
        }
        if let Some(source_max) = concurrent_source_max {
            source_max_candidate = Some(source_max);
            suppress_later_source_max = true;
        }
        if let Some(continued) = concurrent_narrow {
            replace_optional_if_smaller(&mut narrow_candidate, continued);
        }
    }
    if let Some(step) = narrow_continuation_step {
        step.finish(narrow_candidate.as_ref().map(|candidate| {
            candidate_progress(
                candidate,
                source.meaningful_bits,
                candidate.is_strictly_smaller_than_source(source),
            )
        }));
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
    // This structural cleanup normally runs in the deft4j lineage beside
    // source max. If that concurrent phase could not run, finish it serially
    // only while the file's critical deadline remains. The deadline-aware
    // variant retains the completed parent if an active split trial runs out
    // of grace instead of beginning unbounded work after the hard stop.
    if run_compact_split_floor && !compact_split_attempted && !deadline.expired() {
        compact_split_candidate = match deft4j_candidate.as_ref() {
            Some(deft4j)
                if !compact_split_parent_is_completed(
                    deft4j,
                    completed_compact_split_parent.as_deref(),
                ) =>
            {
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
        step.finish(compact_split_candidate.as_ref().map(|candidate| {
            candidate_progress(
                candidate,
                source.meaningful_bits,
                candidate.is_strictly_smaller_than_source(source),
            )
        }));
    }
    if let Some(split) = compact_split_candidate {
        replace_optional_if_smaller(&mut deft4j_candidate, split);
    }
    if let Some(deft4j) = &mut deft4j_candidate {
        // Keep only the encoded incumbent for comparison and later routes;
        // refinement can otherwise retain another expanded token graph.
        deft4j.plans.clear();
    }
    if let Some(narrow) = narrow_candidate {
        replace_optional_if_smaller(&mut deft4j_candidate, narrow);
    }

    // Bounded max routes deliberately retain their historical ordinary seed:
    // a smaller complete Default floor can occupy a different search basin.
    // Compare that independent floor only after those descendants finish, so
    // max gets both the established Default result and its original routes
    // without recomputing the shared base candidate.
    if let Some(complete_default) = complete_default_candidate.take() {
        replace_optional_if_smaller(&mut bounded_floor_candidate, complete_default);
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
        // Max mode tries the genuine normal-mode route first. Its best complete
        // candidate remains the comparison floor, but the hard deadline may
        // stop an unusually slow route before all of its replay work finishes.
        match default_floor {
            DefaultFloor::Complete => {
                let floors =
                    build_complete_default_floor_candidate(source, options, &deadline, progress)?;
                complete_default_candidate = Some(floors.complete);
                floors.max_seed
            }
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
        // An APNG image stream is one part of a larger file-level Default run.
        // Keep its full initial planner, but leave repeated replay and the
        // independent endpoint-proven lineage to Max. Applying those additive
        // routes to every frame made Default scale with route count rather
        // than useful savings.
        build_apng_default_candidate(source, options, &mut deadline.hard_stop())?
    } else {
        build_candidate(
            source,
            options,
            DEFAULT_RAW_REPLAY_LIMIT,
            &mut deadline.hard_stop(),
        )?
    };
    if let Some(step) = initial_step {
        step.finish(Some(candidate_progress(
            &candidate,
            source.meaningful_bits,
            candidate.is_strictly_smaller_than_source(source),
        )));
    }
    // The APNG-specific fast shared floor is complete and validated, but
    // repeating optional compact feedback siblings for every frame can turn
    // individually bounded routes into an unbounded file-wide Default cost.
    // Standalone, metadata, and other shared callers retain their existing
    // fixed points; Max covers the broader APNG feedback families.
    if !options.exhaustive && default_floor != DefaultFloor::ApngDefault {
        candidate =
            improve_default_floor_with_feedback(source, options, &deadline, progress, candidate)?;
    }
    // Any separately completed topology floor is consumed by the bounded phase
    // and returned as `bounded_floor_candidate`.
    debug_assert!(guaranteed_floor_candidate.is_none());
    if deft4j_eligible && default_floor == DefaultFloor::Complete && deadline.can_start_route() {
        let deft4j_step = progress.start("deft4j-derived source route");
        deft4j_candidate =
            build_deft4j_source_candidate(source, options, &mut deadline.hard_stop())?;
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
    if let Some(deft4j) = deft4j_candidate {
        candidate.replace_if_smaller(deft4j);
    }

    // A 4,096-token collection can start slightly larger but converge to a
    // better fragmented-stream layout after strict replays. It is additive to
    // the normal-mode comparison floor above, and starts only while the soft
    // deadline still permits a new independent route.
    let run_fragmented =
        options.exhaustive && !suppress_later_optional_routes && deadline.can_start_route();
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
                    &mut deadline.hard_stop(),
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
        let source_max = if let Some(max_candidate) = source_max_candidate {
            Some(max_candidate)
        } else {
            let run_source_max = !suppress_later_source_max && deadline.can_start_route();
            if run_source_max {
                Some(build_source_max_candidate(
                    source,
                    options,
                    progress,
                    &deadline,
                    integrated_compact_source_max,
                    &mut deadline.hard_stop(),
                )?)
            } else {
                None
            }
        };
        if let Some(mut max_candidate) = source_max {
            // A locally smaller proven-feedback endpoint can hide the bounded
            // balanced-tree header win reachable from source max. Finish that
            // cheap
            // lineage-specific cleanup before comparing complete streams.
            if compact_tree_eligible {
                let tree_step = progress.start("Columbo source-max balanced-tree floor");
                let mut tree = refine_with_compact_balanced_tree_floor(
                    &max_candidate,
                    options,
                    decoded_limit,
                    identity,
                )?;
                if let Some(candidate) = tree.as_mut() {
                    if let Some(feedback) = refine_with_compact_proven_feedback(
                        candidate,
                        options,
                        decoded_limit,
                        identity,
                    )? {
                        candidate.replace_if_smaller(feedback);
                    }
                }
                tree_step.finish(tree.as_ref().map(|tree| {
                    candidate_progress(
                        tree,
                        source.meaningful_bits,
                        tree.is_strictly_smaller_than_source(source),
                    )
                }));
                if let Some(tree) = tree {
                    max_candidate.replace_if_smaller(tree);
                }
            }
            source_max_stabilized_incumbent = candidate.is_encoding_stabilized_by(&max_candidate);
            candidate.replace_if_smaller(max_candidate);
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
            && deadline.can_start_route();
        let seeded_step = run_seeded.then(|| progress.start("Columbo rewritten-seed refinement"));
        if run_seeded {
            // Rewritten match choices and boundaries can expose later max
            // transformations, so retain one additive seeded pass after both
            // source-shaped routes. Its incumbent remains available if this
            // final route times out or fails to improve it.
            let seeded_candidate = refine_with_max_planner(
                &candidate,
                options,
                decoded_limit,
                identity,
                &mut deadline.hard_stop(),
            )?;
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

    if compact_tree_eligible && deadline.can_start_route() {
        let tree_step = progress.start("Columbo compact balanced-tree floor");
        let tree =
            refine_with_compact_balanced_tree_floor(&candidate, options, decoded_limit, identity)?;
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

    // Standalone Complete work reaches this point with the historical Max seed
    // still driving every heuristic lineage. Compare the independently retained
    // complete Default endpoint only after those routes finish, so adding the
    // quality floor cannot redirect Max into a different rewritten-seed basin.
    if let Some(complete_default) = complete_default_candidate {
        candidate.replace_if_smaller(complete_default);
    }

    let mut bounded_depth_covered = false;
    if smoothed_tree_eligible {
        let tree_step = progress.start("Compact payload-tree floor");
        let (covered, tree) =
            refine_with_compact_payload_tree_floor(&candidate, options, decoded_limit, identity)?;
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
                decoded_limit,
                identity,
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

    // The compact pass above shares this frontier when its structural work
    // bounds apply. Every other completed stream receives the general linear
    // sibling, so an existing compact smoother gate cannot create a coverage
    // hole. It starts only while route time remains, observes the hard stop
    // between blocks, and can never discard the incumbent.
    if !bounded_depth_covered && bounded_depth_tree_eligible && deadline.can_start_route() {
        let tree_step = progress.start("Bounded-depth tree floor");
        let tree = refine_with_bounded_depth_tree_floor(
            &candidate,
            options,
            decoded_limit,
            identity,
            &mut deadline.hard_stop(),
        )?;
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
            source_deflate_bits: parsed.meaningful_bits,
            deflate_bits,
            source_block_count: parsed.source_block_count,
            source_empty_block_count: parsed.source_empty_block_count,
        },
        timed_out,
    })
}

fn deft4j_source_route_eligible(blocks: &[ParsedBlock]) -> bool {
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
    // A lone stored block has no table, token, boundary, or adjacent-merge
    // operation for the deft4j graph to improve. Every other live topology is
    // admitted; route-local byte accounting handles its work size.
    nonempty >= 2 || (nonempty == 1 && huffman == 1)
}

fn narrow_source_route_eligible(blocks: &[ParsedBlock], compressed_len: usize) -> bool {
    compressed_len <= NARROW_SOURCE_MAX_COMPRESSED
        && (2..=NARROW_SOURCE_LIST_MAX_BLOCKS).contains(
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

fn compact_balanced_tree_source_eligible(
    compressed_len: usize,
    decoded_size: u64,
    blocks: &[ParsedBlock],
) -> bool {
    if compressed_len > COMPACT_TREE_MAX_COMPRESSED || decoded_size > COMPACT_TREE_MAX_DECODED {
        return false;
    }
    let mut nonempty = blocks.iter().filter(|block| !block.plain.is_empty());
    let Some(block) = nonempty.next() else {
        return false;
    };
    nonempty.next().is_none()
        && block.source_type == SourceBlockType::Dynamic
        && block.tokens.len() <= COMPACT_TREE_MAX_TOKENS
}

fn compact_strict_literal_tree_eligible(compressed_len: usize, blocks: &[ParsedBlock]) -> bool {
    if compressed_len > COMPACT_TREE_MAX_COMPRESSED {
        return false;
    }
    let mut nonempty = blocks.iter().filter(|block| !block.plain.is_empty());
    let Some(block) = nonempty.next() else {
        return false;
    };
    nonempty.next().is_none()
        && block.source_type == SourceBlockType::Dynamic
        && block.tokens.len() <= COMPACT_TREE_MAX_TOKENS
        && block
            .distance_frequencies
            .iter()
            .all(|&frequency| frequency == 0)
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
enum BoundedPngMaxPolicy {
    #[default]
    Standard,
    FloorExpansion,
    GenericParallel,
}

/// Choose the bounded PNG route family from available independent work.
///
/// Generic streams have no specialized source sibling, so source max remains
/// available beside the floor lineage. A multi-block source retains its broad
/// floor continuation. A one-block source receives that continuation only when
/// the completed ordinary floor exposed a new block boundary or token spelling.
/// Those are search states which the original-source siblings cannot explore,
/// so this is an algorithmic capability gate rather than a size band.
fn bounded_png_max_policy(
    nonempty_blocks: usize,
    floor_exposes_new_states: bool,
    deft4j_eligible: bool,
    narrow_eligible: bool,
) -> BoundedPngMaxPolicy {
    if !deft4j_eligible && !narrow_eligible {
        BoundedPngMaxPolicy::GenericParallel
    } else if nonempty_blocks >= 2 || floor_exposes_new_states {
        BoundedPngMaxPolicy::FloorExpansion
    } else {
        BoundedPngMaxPolicy::Standard
    }
}

/// Whether reparsing the completed floor gives max search a genuinely new seed.
///
/// Compare only meaningful blocks: encoders commonly append empty flush
/// blocks, but those cannot create token-search states. Different non-empty
/// boundaries or different token spellings can. The latter matters even for a
/// one-block input because a normal-floor repartition may reach a fixed point
/// that source-order max cannot reconstruct before its deadline.
fn floor_exposes_new_search_states(source: &[ParsedBlock], floor: &[PlannedBlock]) -> bool {
    let mut source = source.iter().filter(|block| !block.plain.is_empty());
    let mut floor = floor.iter().filter(|block| !block.plain.is_empty());
    loop {
        match (source.next(), floor.next()) {
            (Some(source_block), Some(floor_block)) => {
                if source_block.plain.len() != floor_block.plain.len()
                    || source_block.tokens.as_slice() != floor_block.tokens.as_slice()
                {
                    return true;
                }
            }
            (None, None) => return false,
            _ => return true,
        }
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

fn source_run_match_count_exceeds(blocks: &[ParsedBlock], limit: usize) -> bool {
    same_distance_opportunities(blocks).matches > limit
}

fn prebuild_bounded_floor(nonempty_blocks: usize, decoded_size: u64) -> bool {
    nonempty_blocks <= 1
        || decoded_size <= PREBUILD_BOUNDED_FLOOR_MAX_DECODED
        || decoded_size > CONCURRENT_BOUNDED_FLOOR_MAX_DECODED
}

fn gain_is_below(source_bits: u64, candidate_bits: u64, basis_points: u64) -> bool {
    let saved = source_bits.saturating_sub(candidate_bits);
    saved.saturating_mul(10_000) < source_bits.saturating_mul(basis_points)
}

/// Preserve a serial compact-split sibling unless the floor-seeded endpoint
/// crossed the same material-gain threshold.
///
/// Compact split is inspected because the direct deft4j-derived route saved
/// less than two percent. In a serial work class, giving another sub-threshold
/// descendant all remaining time would starve the independent structural
/// topology for the same reason that admitted it. A bounded parallel class
/// can evaluate both, while a seeded endpoint that already saves at least the
/// threshold remains the stronger signal on its own.
fn floor_seeded_priority_with_structural_sibling(
    floor_bits: u64,
    seeded_bits: u64,
    compact_split_pending: bool,
    compact_split_can_overlap: bool,
) -> bool {
    !compact_split_pending
        || compact_split_can_overlap
        || !gain_is_below(floor_bits, seeded_bits, WEAK_DEFT4J_GAIN_BASIS_POINTS)
}

/// Whether a non-dominated deft4j-derived descendant should share the active
/// floor-continuation window.
///
/// Only the standalone file-level owner in the bounded parallel work class
/// admits both expanded candidate models. Container children retain their
/// shared scheduler. Within the admitted class, both routes are necessary
/// because parent ordering does not prove the order of later endpoints.
fn independent_deft4j_refinement_can_overlap(
    continuing_floor_seed: bool,
    owns_file_level_route_window: bool,
    parallel_work_is_bounded: bool,
    refinement_is_pending: bool,
) -> bool {
    continuing_floor_seed
        && owns_file_level_route_window
        && parallel_work_is_bounded
        && refinement_is_pending
}

#[derive(Clone, Copy)]
struct StreamIdentity {
    decoded_size: u64,
    crc32: u32,
    adler32: u32,
}

#[derive(Clone)]
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

/// Results from the bounded follow-up workers after all started routes join.
///
/// Naming these fields keeps the scheduling decision readable and prevents a
/// positional tuple from silently swapping two optional candidates.
#[derive(Default)]
struct BoundedFollowUpCandidates {
    source_max: Option<Candidate>,
    attempted_compact_split: bool,
    compact_split: Option<Candidate>,
    narrow: Option<Candidate>,
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
        report: candidate.block_report.clone(),
        reference_bits,
        profitable,
    }
}

/// Run one original-source max search, with nested telemetry only in a human
/// reporting mode and the historical hot path otherwise.
fn build_source_max_candidate(
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

/// Add progress heartbeats to the shared concrete stop policy.
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

fn compact_source_has_bounded_match_preserving_feedback(source: CandidateInput<'_>) -> bool {
    let [block] = source.blocks else {
        return false;
    };
    compact_proven_submatch_route_eligible(&block.tokens, block.plain.len())
}

fn compact_source_has_bounded_integrated_proven_feedback(source: CandidateInput<'_>) -> bool {
    if !(2..=COMPACT_SPLIT_FLOOR_MAX_BLOCKS).contains(&source.blocks.len())
        || source.compressed.len() > COMPACT_SPLIT_FLOOR_MAX_COMPRESSED
        || source.identity.decoded_size > COMPACT_SPLIT_FLOOR_MAX_DECODED
    {
        return false;
    }
    let Some(token_count) = source.blocks.iter().try_fold(0_usize, |total, block| {
        total.checked_add(block.tokens.len())
    }) else {
        return false;
    };
    token_count <= COMPACT_SPLIT_FLOOR_MAX_TOKENS
        && source
            .blocks
            .iter()
            .any(|block| compact_proven_submatch_route_eligible(&block.tokens, block.plain.len()))
}

/// Whether two independent max beams can safely share the initial wall clock.
///
/// This uses the compact structural route's existing compressed/decoded work
/// bounds. In that class, overlapping original-source max with floor-seeded
/// max has a small predictable memory cost. One-block inputs use the ordinary
/// aggregate bound. A short multi-block list is also safe when its total
/// decoded work stays within that bound per non-empty source block, the
/// complete token graph stays compact, and multiple proved repartitions
/// justify starting source max before the dependent deft4j refinement. Larger
/// streams retain the sequential schedule.
fn compact_parallel_source_max_work_class(source: CandidateInput<'_>) -> bool {
    if source.compressed.len() > COMPACT_SPLIT_FLOOR_MAX_COMPRESSED {
        return false;
    }
    let nonempty_blocks = source
        .blocks
        .iter()
        .filter(|block| !block.plain.is_empty())
        .count();
    if nonempty_blocks == 1 {
        return source.identity.decoded_size <= COMPACT_SPLIT_FLOOR_MAX_DECODED
            || (source_token_count(source)
                .is_some_and(|tokens| tokens <= COMPACT_SPLIT_FLOOR_MAX_TOKENS)
                && same_distance_opportunities(source.blocks).repartition_runs >= 2);
    }
    let decoded_limit = u64::try_from(nonempty_blocks)
        .ok()
        .and_then(|blocks| COMPACT_SPLIT_FLOOR_MAX_DECODED.checked_mul(blocks));
    (2..=COMPACT_SPLIT_FLOOR_MAX_BLOCKS).contains(&nonempty_blocks)
        && decoded_limit.is_some_and(|limit| source.identity.decoded_size <= limit)
        && source_token_count(source).is_some_and(|tokens| tokens <= COMPACT_SPLIT_FLOOR_MAX_TOKENS)
        && same_distance_opportunities(source.blocks).repartition_runs >= 2
}

/// Whether source max is cheap enough to overlap the bounded floor family.
///
/// Dense repartition graphs justify the independent source root because only
/// it can combine their choices. Tiny token graphs are complementary for a
/// simpler reason: they can cheaply test original block merges that a
/// rewritten floor descendant can no longer reconstruct. The outer parsed
/// model bound still limits aggregate memory and decoded work.
fn bounded_parallel_source_max_work_class(source: CandidateInput<'_>) -> bool {
    compact_complementary_source_max_is_cheap(source)
        || compact_parallel_source_max_work_class(source)
}

fn compact_complementary_source_max_is_cheap(source: CandidateInput<'_>) -> bool {
    source_token_count(source)
        .is_some_and(|tokens| tokens <= COMPACT_COMPLEMENTARY_SOURCE_MAX_TOKENS)
}

fn compact_single_source_route_work_class(source: CandidateInput<'_>) -> bool {
    source_token_count(source).is_some_and(|tokens| {
        (COMPACT_SINGLE_SOURCE_ROUTE_MIN_TOKENS..=COMPACT_SINGLE_SOURCE_ROUTE_MAX_TOKENS)
            .contains(&tokens)
    })
}

fn source_token_count(source: CandidateInput<'_>) -> Option<usize> {
    source.blocks.iter().try_fold(0_usize, |total, block| {
        total.checked_add(block.tokens.len())
    })
}

fn repartition_graph_covers_source_blocks(blocks: &[ParsedBlock], repartition_runs: usize) -> bool {
    let mut nonempty_blocks = 0_usize;
    for block in blocks.iter().filter(|block| !block.plain.is_empty()) {
        nonempty_blocks += 1;
        // Source max must finish each block before it can combine choices
        // across the list. A block outside the bounded 4,000-token work class
        // can consume the entire phase by itself, so dense cross-block
        // opportunities are not yet reachable and cannot justify deferring
        // the compact dependent lineage.
        if block.tokens.len() > COMPACT_SINGLE_SOURCE_ROUTE_MAX_TOKENS {
            return false;
        }
    }
    nonempty_blocks != 0 && repartition_runs >= nonempty_blocks
}

/// Whether the direct deft4j parent can cheaply finish its dependent split.
///
/// The split route already enforces these compressed, decoded, and token
/// bounds on every prepared parent. Applying the same work model before the
/// long independent beams keeps this dependency reachable without introducing
/// a file-name or elapsed-time gate.
fn compact_dependent_deft4j_work_class(source: CandidateInput<'_>) -> bool {
    let Some(token_count) = source.blocks.iter().try_fold(0_usize, |total, block| {
        total.checked_add(block.tokens.len())
    }) else {
        return false;
    };
    source.compressed.len() <= COMPACT_SPLIT_FLOOR_MAX_COMPRESSED
        && source.identity.decoded_size <= COMPACT_SPLIT_FLOOR_MAX_DECODED
        && token_count <= COMPACT_SPLIT_FLOOR_MAX_TOKENS
        && has_multiple_nonempty_blocks(source.blocks)
}

/// Refine the bounded route family's best deft4j lineage in place.
///
/// Keeping this stage in one helper lets the caller evaluate the independent
/// original-source max route concurrently. Both routes preserve their own
/// complete incumbents and share only the global cooperative deadline. When
/// complete compact siblings already cover its parent states, this optional
/// lineage yields at the soft boundary rather than consuming primary-route
/// grace.
#[allow(clippy::too_many_arguments)]
fn refine_bounded_deft4j_lineage(
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
                    seeded.replace_if_smaller(refined);
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
                        seeded.replace_if_smaller(continued);
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
                        seeded.replace_if_smaller(terminal);
                    }
                }
                replace_optional_if_smaller(deft4j_candidate, seeded);
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
        // A productive no-split route can expose another independent fixed
        // point when Default turns the deft4j parent into a smaller, genuinely
        // new topology. Continue only when that new parent also beats the
        // completed original-source no-split result; otherwise the earlier
        // route already dominates this scheduling signal. This is a single
        // changed-parent continuation, not a replay of the original source.
        let continue_no_split = changed_parent_no_split_should_continue(
            exposes_new_parent,
            &refined,
            deft4j,
            narrow_candidate,
        ) && deadline.can_start_route();
        if continue_no_split {
            // The superior changed parent is not represented by the compact
            // sibling that caused the surrounding refinement to yield at the
            // soft boundary. Once started inside that boundary, let this one
            // dependent route finalize under the ordinary file-wide grace.
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
                refined.replace_if_smaller(continued);
            }
        }
        // Default can turn the source-ordered deft4j result into a genuinely
        // new boundary/token topology. That state is not covered by source
        // max or by Max over the unrefined deft4j parent. Continue it once
        // through the full planner while this already-parallel phase has
        // time; header-only rewrites deliberately stop at Default.
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
            refined.replace_if_smaller(continued);
        }
        replace_optional_if_smaller(deft4j_candidate, refined);
    }
    Ok(())
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
    proven_feedback: Option<Candidate>,
    suppress_later_source_max: bool,
    suppress_later_optional_routes: bool,
    completed_compact_split_parent: Option<Vec<u8>>,
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
    run_seeded_max: bool,
    run_deft4j: bool,
    run_narrow: bool,
    run_source_max: bool,
    run_proven_feedback: bool,
    parallel_routes: bool,
    deadline: &Deadline,
    progress: Progress,
    completed_floor: Option<Candidate>,
) -> Result<BoundedPhaseCandidates> {
    let run_source_max = run_source_max && parallel_routes;
    let run_proven_feedback = run_proven_feedback && parallel_routes;
    // A one-block floor descendant is the only route which can continue the
    // floor's newly discovered token spelling. Let that primary route use the
    // complete file window; reserving a fixed tail for source-order siblings
    // can stop it just before a byte boundary which those siblings cannot
    // reach. Multi-block work retains the follow-up reservation because its
    // independent compact routes cover additional structural parents.
    let unique_one_block_floor_descendant = run_seeded_max
        && source
            .blocks
            .iter()
            .filter(|block| !block.plain.is_empty())
            .count()
            == 1;
    // The direct deft4j graph and its ordinary Columbo replay are a compact,
    // dependent lineage. On one-block inputs, finish that cheap dependency
    // before launching the long independent beams: returning only the direct
    // parent and waiting for every long worker can otherwise make its unique
    // replay fixed point unreachable. A compact multi-block lineage also
    // finishes its deterministic split descendant here; unlike source max,
    // that split depends on the refined deft4j parent and cannot usefully run
    // first. Larger graphs retain the parallel worker because their cost is
    // not similarly bounded.
    let repartition_runs = same_distance_opportunities(source.blocks).repartition_runs;
    // Multiple independent same-distance repartitions create combinations
    // that only the original-source max graph can explore. Do not spend its
    // initial wall clock serially completing the dependent deft4j split
    // lineage when that opportunity graph covers every non-empty source
    // block. Sparser graphs retain the dependent lineage first: their source
    // beam cannot combine independent choices across the full block list. The
    // direct deft4j parent still runs beside source max, and its split
    // descendant remains available in the later refinement stage.
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
            deft4j.replace_if_smaller(refined);
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
                // Exact parent identity suppresses the later copy of this
                // completed deterministic route. If no structural cut won,
                // its emitted endpoint has the same token/block state and is
                // already the route's fixed point; otherwise only the input
                // parent is complete and the split descendant remains useful.
                completed_compact_split_parent = if preserves_source_blocks {
                    Some(deft4j.data.clone())
                } else {
                    Some(seed.data)
                };
            }
        }
    }
    // Establish the bounded phase window after finishing the cheap dependency,
    // so its reserved fifth is measured from the actual independent work that
    // remains rather than being silently consumed by prerequisite work.
    let route_window = if (run_deft4j || run_narrow || run_source_max || run_proven_feedback)
        && !unique_one_block_floor_descendant
    {
        RouteWindow::reserving_follow_up(deadline)
    } else {
        RouteWindow::full(deadline)
    };
    let run_deft4j = run_deft4j && !prebuild_deft4j && route_window.can_start_route();
    let run_narrow = run_narrow && route_window.can_start_route();
    // Tiny graphs retain both long roots because their complementary search
    // is cheap and can settle in a different basin. In the capped route's
    // upper work band, give the broader source-max graph sole ownership of
    // source-token search rather than making two workers contend for the same
    // deadline. Multiple independent same-distance repartition runs also keep
    // source max: their combinations exist only in that original-source
    // topology. Above these bounds, omit the source-root beam only when proven
    // feedback actually covers source-token work.
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
        let (floor, floor_seeded) = build_bounded_floor_descendants(
            source,
            options,
            run_seeded_max,
            &route_window,
            completed_floor,
        )?;
        return Ok(BoundedPhaseCandidates {
            floor: Some(floor),
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
            run_seeded_max,
            run_deft4j,
            run_narrow,
            &route_window,
            completed_floor,
        )?;
        if let Some(prebuilt) = prebuilt_deft4j {
            replace_optional_if_smaller(&mut candidates.deft4j, prebuilt);
        }
        candidates.completed_compact_split_parent = completed_compact_split_parent;
        return Ok(candidates);
    }

    thread::scope(|scope| {
        let deft4j_worker = run_deft4j.then(|| {
            thread::Builder::new()
                .name("columbo-deft4j-derived".into())
                .spawn_scoped(scope, || {
                    run_route_with_cancellation(deadline, || {
                        build_deft4j_source_candidate(source, options, &mut route_window.stop())
                    })
                })
                .ok()
        });
        let narrow_worker = run_narrow.then(|| {
            thread::Builder::new()
                .name("columbo-no-split".into())
                .spawn_scoped(scope, || {
                    run_route_with_cancellation(deadline, || {
                        // The streamlined no-split walk owns cumulative
                        // pruning and adjacent merges, so it can forward a
                        // complete incumbent at the shared phase boundary
                        // without repeating source-max's individual pruning.
                        build_narrow_source_candidate(
                            source,
                            options,
                            &mut route_window.stop(),
                            &mut route_window.stop(),
                        )
                    })
                })
                .ok()
        });
        let source_max_worker = run_source_max.then(|| {
            thread::Builder::new()
                .name("columbo-source-max-initial".into())
                .spawn_scoped(scope, || {
                    run_route_with_cancellation(deadline, || {
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
                .ok()
        });
        let proven_feedback_worker = run_proven_feedback.then(|| {
            thread::Builder::new()
                .name("columbo-proven-feedback-initial".into())
                .spawn_scoped(scope, || {
                    run_route_with_cancellation(deadline, || {
                        build_compact_proven_feedback_candidate(
                            source,
                            options,
                            &mut route_window.stop(),
                        )
                    })
                })
                .ok()
        });

        // A route error (or unwind) asks its siblings to stop at their next
        // ordinary deadline check. Join every successfully spawned worker
        // before choosing the fixed deft4j/narrow/floor error order below.
        let floor = run_route_with_cancellation(deadline, || {
            build_bounded_floor_descendants(
                source,
                options,
                run_seeded_max,
                &route_window,
                completed_floor,
            )
        });
        let deft4j = match deft4j_worker.flatten() {
            Some(worker) => worker.join(),
            None if run_deft4j => Ok(run_route_with_cancellation(deadline, || {
                build_deft4j_source_candidate(source, options, &mut route_window.stop())
            })),
            None => Ok(Ok(None)),
        };
        let narrow = match narrow_worker.flatten() {
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
        let source_max = match source_max_worker.flatten() {
            Some(worker) => match worker.join() {
                Ok(result) => result.map(Some),
                Err(payload) => std::panic::resume_unwind(payload),
            },
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
        let proven_feedback = match proven_feedback_worker.flatten() {
            Some(worker) => match worker.join() {
                Ok(result) => result,
                Err(payload) => std::panic::resume_unwind(payload),
            },
            None if run_proven_feedback => run_route_with_cancellation(deadline, || {
                build_compact_proven_feedback_candidate(source, options, &mut route_window.stop())
            }),
            None => Ok(None),
        };

        // A panic still denotes an internal invariant failure. All joins are
        // complete now, so resuming it cannot strand a sibling worker.
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
        let (floor, floor_seeded) = floor?;
        if let Some(worker_deft4j) = worker_deft4j {
            replace_optional_if_smaller(&mut prebuilt_deft4j, worker_deft4j);
        }
        Ok(BoundedPhaseCandidates {
            floor: Some(floor),
            floor_seeded,
            deft4j: prebuilt_deft4j,
            narrow,
            source_max,
            proven_feedback,
            suppress_later_source_max: run_source_max,
            completed_compact_split_parent,
            ..BoundedPhaseCandidates::default()
        })
    })
}

/// Retain the deft4j/narrow/floor deadline order without overlapping arenas.
fn build_bounded_phase_candidates_sequential(
    source: CandidateInput<'_>,
    options: &Options,
    run_seeded_max: bool,
    run_deft4j: bool,
    run_narrow: bool,
    route_window: &RouteWindow<'_>,
    completed_floor: Option<Candidate>,
) -> Result<BoundedPhaseCandidates> {
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
    let (floor, floor_seeded) = build_bounded_floor_descendants(
        source,
        options,
        run_seeded_max,
        route_window,
        completed_floor,
    )?;
    Ok(BoundedPhaseCandidates {
        floor: Some(floor),
        floor_seeded,
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

fn build_bounded_floor_candidate(
    source: CandidateInput<'_>,
    options: &Options,
    expired: &mut SearchStop<'_>,
) -> Result<Candidate> {
    let mut floor_options = options.clone();
    floor_options.exhaustive = false;
    build_candidate(source, &floor_options, DEFAULT_RAW_REPLAY_LIMIT, expired)
        .map(|candidate| candidate.named("Normal floor"))
}

/// Copy a complete caller-retained stream into the local candidate set.
///
/// `Established` is used only for a stream just emitted and validated by an
/// independent Columbo lineage. Re-running Default here would rediscover a
/// floor the caller already owns; later descendants reparse these bytes before
/// using them, preserving the normal identity checks.
fn established_floor_candidate(source: CandidateInput<'_>) -> Result<Candidate> {
    let mut data = Vec::new();
    data.try_reserve_exact(source.compressed.len())
        .map_err(|_| Error::new("could not allocate Deflate output"))?;
    data.extend_from_slice(source.compressed);
    Ok(Candidate {
        data,
        bits: source.meaningful_bits,
        plans: Vec::new(),
        block_report: None,
        route: "Normal floor",
        max_planner_is_stable: false,
    })
}

/// Add the bounded siblings that form the complete ordinary-mode floor.
///
/// These routes are deliberately shared by a normal invocation and the floor
/// established at the start of a single-stream PNG max invocation. Keeping the
/// sequence in one helper prevents max from approximating Default with only
/// its first route and losing a completed byte or bit saving.
fn improve_default_floor_with_feedback(
    source: CandidateInput<'_>,
    options: &Options,
    deadline: &Deadline,
    progress: Progress,
    mut candidate: Candidate,
) -> Result<Candidate> {
    debug_assert!(!options.exhaustive);

    // Proven-before-feedback has a distinct compact fixed point from the
    // ordinary endpoint ordering. Retain both as complete candidates whenever
    // the source contains a proved match and the whole sibling is within its
    // explicit token/plain work bounds.
    if compact_source_has_bounded_match_preserving_feedback(source) && deadline.can_start_route() {
        let step = progress.start("Columbo match-preserving feedback");
        let contender =
            build_compact_proven_feedback_candidate(source, options, &mut deadline.hard_stop())?;
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
    if compact_source_has_bounded_integrated_proven_feedback(source) && deadline.can_start_route() {
        let step = progress.start("Columbo integrated proven feedback");
        let contender = build_compact_integrated_proven_feedback_candidate(
            source,
            options,
            &candidate,
            &mut deadline.hard_stop(),
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
    Ok(candidate)
}

/// Establish the exact ordinary result before starting single-PNG max routes.
///
/// The benchmark grants max the measured Default time plus additional search
/// time. Reusing the finished floor both honors that contract and lets later
/// max descendants start from its already selected blocks instead of repeating
/// the ordinary route.
struct CompleteDefaultFloor {
    /// The ordinary base used by the established bounded max lineage.
    max_seed: Candidate,
    /// The complete result produced by the same routes as Default mode.
    complete: Candidate,
}

fn build_complete_default_floor_candidate(
    source: CandidateInput<'_>,
    options: &Options,
    deadline: &Deadline,
    progress: Progress,
) -> Result<CompleteDefaultFloor> {
    let mut floor_options = options.clone();
    floor_options.exhaustive = false;
    let candidate = build_candidate(
        source,
        &floor_options,
        DEFAULT_RAW_REPLAY_LIMIT,
        &mut deadline.hard_stop(),
    )?;
    let max_seed = candidate.clone().named("Normal floor");
    let complete =
        improve_default_floor_with_feedback(source, &floor_options, deadline, progress, candidate)?;
    Ok(CompleteDefaultFloor { max_seed, complete })
}

/// Reuse a completed ordinary-mode floor when PNG scheduling already made it.
///
/// Its retained plans have the same shape needed by each bounded continuation,
/// so rebuilding the candidate would only consume deadline and memory.
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

/// Build the ordinary bounded floor, then continue through the historical
/// Columbo max route seeded from that rewritten floor.
///
/// Both complete candidates remain independent, so timeout or non-improvement
/// cannot discard the normal-mode floor. The caller applies the parallel model
/// cap before selecting this route.
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

/// Continue the selected floor into max search when requested.
///
/// A rewrite can expose new token/table feedback even when it retains or
/// reduces the source block count, so topology alone is not a sound dominance
/// test for this descendant.
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

    // Inspect the plans already retained by the floor. Re-parsing merely to
    // rediscover this topology would spend time and allocate another model.
    continue_bounded_floor_lineage(source, floor, options, route_window, true)
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

    // Parse the finished floor once for both independent descendants. The
    // previous implementation reparsed the same bytes separately for bounded
    // grouping and max refinement, duplicating validation and model storage.
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
            &mut route_window.stop(),
        )?
        .named("Columbo max refinement");
        descendant = Some(seeded);
    } else if has_multiple_nonempty_blocks(&stream.blocks) && route_window.can_start_route() {
        // The full max planner starts from this same bounded grouping floor.
        // Run the standalone structural form only when a broader seeded max
        // descendant is not scheduled; doing both repeats range pricing and
        // can starve the more capable route.
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
    let route_window = RouteWindow::full(deadline);
    thread::scope(|scope| {
        let run_source_max = deadline.can_start_route();
        let source_worker = run_source_max
            .then(|| {
                thread::Builder::new()
                    .name("columbo-source-max".into())
                    .spawn_scoped(scope, || {
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
                    })
                    .ok()
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
fn build_deft4j_source_candidate(
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

/// Build one direct source-order candidate without the broader split graph.
///
/// A winning merge or token rewrite exposes a parent that the original source
/// planner could not inspect. Reparse that genuinely new state once through
/// the ordinary planner; header-only rewrites are already fully priced by the
/// first pass and do not justify repeating the work.
fn build_narrow_source_candidate(
    source: CandidateInput<'_>,
    options: &Options,
    expired: &mut SearchStop<'_>,
    refinement_stop: &mut SearchStop<'_>,
) -> Result<Option<Candidate>> {
    // The dedicated source-max route owns one-match-at-a-time pruning, while
    // the deft4j-derived seed already supplies table-driven pruning states.
    // Repeating the individual family in this sibling delays later source
    // blocks; keep no-split focused on its distinct cumulative pruning order
    // and adjacent merges.
    let Some(plans) = plan_source_no_split_route(source.blocks, 0, options, &mut *expired) else {
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
        candidate.replace_if_smaller(refined);
    }
    Ok(Some(candidate.named("No-split source")))
}

/// Whether emitting a route changed block boundaries or token spellings.
///
/// A different Huffman representation alone cannot improve when immediately
/// fed to the smaller Default tree search: Max has already priced that same
/// token state with a superset of header candidates. Boundary or token changes
/// are different—they expose a new planner input and justify one replay.
fn candidate_exposes_new_parent(candidate: &Candidate, source: CandidateInput<'_>) -> bool {
    candidate.data.as_slice() != source.compressed
        && (candidate.plans.len() != source.blocks.len()
            || candidate
                .plans
                .iter()
                .zip(source.blocks)
                .any(|(plan, block)| {
                    plan.plain.len() != block.plain.len()
                        || (!std::sync::Arc::ptr_eq(&plan.tokens, &block.tokens)
                            && plan.tokens.as_ref() != block.tokens.as_ref())
                }))
}

/// Apply the source-ordered deft4j route to a genuinely rewritten floor state.
///
/// Weak gains from the original deft4j route can indicate that Columbo's
/// ordinary bounded floor is the more useful parent in its route graph.
/// Reparse that encoded incumbent once, validate its identity, and keep the
/// seeded deft4j walk additive to the original deft4j candidate rather than
/// replaying duplicate work on the original blocks.
fn build_deft4j_seed_candidate(
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

/// Apply the ordinary planner to boundaries and token spellings produced by
/// the source-ordered deft4j route while the same container deadline has room.
///
/// This is not duplicate source work: the reparsed deft4j candidate contains
/// merged blocks and expanded token states that the original normal floor
/// cannot see. The original floor remains an independent comparison below.
fn refine_with_default_planner(
    candidate: &Candidate,
    options: &Options,
    decoded_limit: u64,
    identity: StreamIdentity,
    expired: &mut SearchStop<'_>,
) -> Result<Candidate> {
    refine_with_default_planner_and_change(candidate, options, decoded_limit, identity, expired)
        .map(|(candidate, _)| candidate)
}

/// Apply Default and report whether its result exposes a new planner state.
///
/// The signal is computed against the already parsed input while both models
/// are live. Callers can then admit one dependent Max continuation without
/// reparsing the parent merely to distinguish topology changes from cheaper
/// headers over otherwise identical blocks and tokens.
fn refine_with_default_planner_and_change(
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

/// Apply the narrow source-order route to a complete rewritten candidate.
fn refine_with_no_split_route(
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

fn changed_parent_no_split_should_continue(
    exposes_new_parent: bool,
    refined: &Candidate,
    parent: &Candidate,
    completed_no_split: Option<&Candidate>,
) -> bool {
    exposes_new_parent
        && refined.is_strictly_smaller_than(parent)
        && completed_no_split.is_some_and(|no_split| refined.is_strictly_smaller_than(no_split))
}

/// Decide whether a completed no-split result owns the next dependent step.
///
/// Equal-scoring siblings do not dominate it: their distinct encodings can
/// still lead to differently ordered endpoints. A strictly better completed
/// sibling does dominate the scheduling signal, while the original candidate
/// remains retained regardless of whether this optional continuation runs.
fn changed_narrow_parent_should_continue(
    exposes_new_parent: bool,
    narrow: &Candidate,
    source: CandidateInput<'_>,
    competitors: &[Option<&Candidate>],
) -> bool {
    exposes_new_parent
        && narrow.is_strictly_smaller_than_source(source)
        && competitors
            .iter()
            .flatten()
            .all(|other| !other.is_strictly_smaller_than(narrow))
}

/// Finish a small deft4j-derived seed with Columbo's structural split floor.
///
/// The timed source route can settle its token spellings just before the
/// deadline, leaving no opportunity to price the seven inexpensive
/// eighth-position cuts already used elsewhere by Columbo. This route performs
/// no token search, merge search, or replay. Its explicit size, topology, and
/// token limits make it safe to finish deterministically after the shared
/// deadline, while the caller keeps the unsplit candidate as a fallback.
struct CompactSplitSeed {
    data: Vec<u8>,
    bits: u64,
    stream: ParsedStream,
}

fn prepare_compact_source_split_seed(
    candidate: &Candidate,
    decoded_limit: u64,
    identity: StreamIdentity,
) -> Result<Option<CompactSplitSeed>> {
    if candidate.data.len() > COMPACT_SPLIT_FLOOR_MAX_COMPRESSED {
        return Ok(None);
    }

    let mut data = Vec::new();
    data.try_reserve_exact(candidate.data.len())
        .map_err(|_| Error::new("could not allocate compact route seed"))?;
    data.extend_from_slice(&candidate.data);
    let stream = parse_validated_rewrite(&data, decoded_limit, identity)?;
    if !compact_source_split_floor_eligible(identity.decoded_size, &stream.blocks) {
        return Ok(None);
    }
    Ok(Some(CompactSplitSeed {
        data,
        bits: candidate.bits,
        stream,
    }))
}

fn build_prepared_compact_source_split_floor(
    seed: &CompactSplitSeed,
    options: &Options,
    decoded_limit: u64,
    identity: StreamIdentity,
) -> Result<Option<Candidate>> {
    let Some(plans) = plan_compact_source_split_floor(&seed.stream.blocks, 0, options) else {
        return Ok(None);
    };
    build_prepared_compact_source_split_floor_from_plans(
        seed,
        options,
        decoded_limit,
        identity,
        plans,
    )
}

fn build_prepared_compact_source_split_floor_until(
    seed: &CompactSplitSeed,
    options: &Options,
    decoded_limit: u64,
    identity: StreamIdentity,
    expired: &mut SearchStop<'_>,
) -> Result<Option<Candidate>> {
    let Some(plans) =
        plan_compact_source_split_floor_until(&seed.stream.blocks, 0, options, expired)
    else {
        return Ok(None);
    };
    build_prepared_compact_source_split_floor_from_plans(
        seed,
        options,
        decoded_limit,
        identity,
        plans,
    )
}

fn build_prepared_compact_source_split_floor_from_plans(
    seed: &CompactSplitSeed,
    options: &Options,
    decoded_limit: u64,
    identity: StreamIdentity,
    plans: Vec<PlannedBlock>,
) -> Result<Option<Candidate>> {
    let source = CandidateInput {
        compressed: &seed.data,
        blocks: &seed.stream.blocks,
        meaningful_bits: seed.bits,
        decoded_limit,
        identity,
    };
    let mut never_expires = SearchStop::never();
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

fn build_prepared_compact_source_split_floors(
    seeds: [Option<&CompactSplitSeed>; 3],
    options: &Options,
    decoded_limit: u64,
    identity: StreamIdentity,
    deadline: &Deadline,
) -> Result<Option<Candidate>> {
    let seeds = ordered_compact_split_seeds(seeds);
    let mut candidate = None;
    let mut attempted = false;
    for seed in seeds {
        // A selected parent is already a complete incumbent. Price its split
        // descendants cooperatively so the hard file boundary forwards every
        // completed useful cut instead of waiting for an untimed full sweep.
        // Later independent parents still begin only inside the soft schedule;
        // a larger Max budget naturally evaluates all of them.
        if attempted && !deadline.can_start_route() {
            break;
        }
        attempted = true;
        if let Some(contender) = build_prepared_compact_source_split_floor_until(
            seed,
            options,
            decoded_limit,
            identity,
            &mut deadline.hard_stop(),
        )? {
            replace_optional_if_smaller(&mut candidate, contender);
        }
    }
    Ok(candidate)
}

fn ordered_compact_split_seeds(seeds: [Option<&CompactSplitSeed>; 3]) -> Vec<&CompactSplitSeed> {
    // A smaller completed parent is the strongest general prior for a useful
    // split descendant: it already carries the best known token spellings and
    // table choices, while the split route adds only new structural cuts.
    // Split pricing is not monotone, so retain every distinct parent when the
    // deadline permits; ordering merely ensures that a bounded run evaluates
    // its most promising complete lineage first.
    let mut seeds: Vec<_> = seeds.into_iter().flatten().collect();
    seeds.sort_by_key(|seed| (seed.data.len(), seed.bits));
    seeds
}

fn compact_split_parent_is_completed(candidate: &Candidate, completed: Option<&[u8]>) -> bool {
    completed.is_some_and(|data| data == candidate.data)
}

fn compact_split_preserves_source_blocks(blocks: &[ParsedBlock], plans: &[PlannedBlock]) -> bool {
    blocks.len() == plans.len()
        && blocks.iter().zip(plans).all(|(block, plan)| {
            std::sync::Arc::ptr_eq(&block.tokens, &plan.tokens)
                && std::sync::Arc::ptr_eq(&block.plain, &plan.plain)
                && match &plan.representation {
                    Representation::Original(original) => original.block_type == block.source_type,
                    Representation::Stored => block.source_type == SourceBlockType::Stored,
                    Representation::Fixed => block.source_type == SourceBlockType::Fixed,
                    Representation::Dynamic(_) => block.source_type == SourceBlockType::Dynamic,
                }
        })
}

fn refine_with_compact_source_split_floor(
    candidate: &Candidate,
    options: &Options,
    decoded_limit: u64,
    identity: StreamIdentity,
) -> Result<Option<Candidate>> {
    let Some(seed) = prepare_compact_source_split_seed(candidate, decoded_limit, identity)? else {
        return Ok(None);
    };
    build_prepared_compact_source_split_floor(&seed, options, decoded_limit, identity)
}

fn refine_with_compact_source_split_floor_until(
    candidate: &Candidate,
    options: &Options,
    decoded_limit: u64,
    identity: StreamIdentity,
    expired: &mut SearchStop<'_>,
) -> Result<Option<Candidate>> {
    let Some(seed) = prepare_compact_source_split_seed(candidate, decoded_limit, identity)? else {
        return Ok(None);
    };
    build_prepared_compact_source_split_floor_until(
        &seed,
        options,
        decoded_limit,
        identity,
        expired,
    )
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

/// Apply Columbo's bounded pair/quad balanced-tree moves to one dynamic block.
///
/// The source-level gate is an upper work bound. This post-route helper neither
/// changes tokens nor replays the result; the encoded incumbent remains an
/// independent fallback if every legal tree move loses.
fn refine_with_compact_balanced_tree_floor(
    candidate: &Candidate,
    options: &Options,
    decoded_limit: u64,
    identity: StreamIdentity,
) -> Result<Option<Candidate>> {
    let stream = parse_validated_rewrite(&candidate.data, decoded_limit, identity)?;
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
    // Default's bounded feedback floors gain broadly from the cheap literal
    // pair move. Matched Max streams omit that standalone family, but still
    // price distance moves and the bounded paired cross-alphabet search.
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
    // Default ranks the bounded move family with its ordinary header grid,
    // then gives only the winning tree one exhaustive header finalization.
    // Max already prices every prospective tree exhaustively. This retains
    // the useful header tail without repeating that work for losing moves.
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

/// Apply the complete feasible restricted-depth frontier to a completed
/// Huffman stream. This is a terminal sibling, so a locally attractive tree
/// can never redirect or replace the established search lineage unless the
/// fully emitted stream is strictly smaller.
fn refine_with_bounded_depth_tree_floor(
    candidate: &Candidate,
    options: &Options,
    decoded_limit: u64,
    identity: StreamIdentity,
    expired: &mut SearchStop<'_>,
) -> Result<Option<Candidate>> {
    let stream = parse_validated_rewrite(&candidate.data, decoded_limit, identity)?;
    if stream.blocks.is_empty() {
        return Ok(None);
    }

    let mut plans = Vec::new();
    if plans.try_reserve_exact(stream.blocks.len()).is_err() {
        return Ok(None);
    }
    let mut alignment = 0_u8;
    let mut improved = false;
    for block in &stream.blocks {
        if expired.reached() {
            return Ok(None);
        }
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

/// Apply raw restricted-depth and RLE-smoothed payload trees to a completed
/// compact Huffman stream.
///
/// Keeping this out of the central block planner is deliberate: tree prices
/// influence later token feedback, so replacing an intermediate winner can
/// lose a better downstream fixed point. This terminal sibling preserves the
/// completed parent and is accepted only after exact whole-stream emission.
fn refine_with_compact_payload_tree_floor(
    candidate: &Candidate,
    options: &Options,
    decoded_limit: u64,
    identity: StreamIdentity,
) -> Result<(bool, Option<Candidate>)> {
    let stream = parse_validated_rewrite(&candidate.data, decoded_limit, identity)?;
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

/// Stabilize one balanced-tree rewrite through proven-feedback's fixed point.
///
/// A new Huffman header changes match-to-literal prices; the resulting token
/// feedback can in turn expose a second header win. The compact route enforces
/// its own 4,000-token/80-KiB work bounds and applies balanced-tree cleanup to
/// its own final seed, so one composition closes this dependency without
/// rerunning max.
fn refine_with_compact_proven_feedback(
    candidate: &Candidate,
    options: &Options,
    decoded_limit: u64,
    identity: StreamIdentity,
) -> Result<Option<Candidate>> {
    let stream = parse_validated_rewrite(&candidate.data, decoded_limit, identity)?;
    let source = rewritten_input(candidate, &stream, decoded_limit, identity);
    let mut never_expires = SearchStop::never();
    build_compact_proven_feedback_candidate(source, options, &mut never_expires)
}

/// Close the bounded tree-shape dependency exposed by a winning payload-tree
/// floor.
///
/// The fixed-point smoother can expose a profitable pair/quad Kraft move that
/// the parent tree did not contain. Price that already-bounded tree-only floor
/// once, then reapply smoothing without rerunning token feedback or the
/// ordinary/Max planner. Further rounds produce diminishing corpus gains for
/// a measurable Default slowdown, so they remain a rejected expansion.
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

/// Apply the full Columbo max planner to a complete rewritten candidate.
fn refine_with_max_planner(
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

/// Apply one linear, deterministic merge cleanup to a selected max seed.
///
/// The timed route may end immediately after producing a substantially better
/// block list. Reparse that completed stream and price only adjacent merges
/// with bounded table floors. If time remains, at most two ordinary replays
/// may stabilize the smaller list. Optional probes observe the caller's
/// deadline; complete candidate emission and validation still finish.
fn refine_with_terminal_merge(
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

/// Build one complete-stream candidate and follow strict improvements through
/// a small number of reparse/replan rounds.
fn build_candidate(
    source: CandidateInput<'_>,
    options: &Options,
    replay_limit: usize,
    expired: &mut SearchStop<'_>,
) -> Result<Candidate> {
    build_candidate_with_proven_policy(source, options, replay_limit, true, expired)
}

/// Run the full initial APNG stream planner once, without entering its
/// additive reparse/replay or independent endpoint-proven lineages.
fn build_apng_default_candidate(
    source: CandidateInput<'_>,
    options: &Options,
    expired: &mut SearchStop<'_>,
) -> Result<Candidate> {
    let Some(plans) = plan_stream(source.blocks, 0, options, expired) else {
        return source_candidate(source, options);
    };
    build_candidate_from_plans(source, plans, options, 0, ReplayPlanner::Full, expired)
}

/// Build the compact proven-before-feedback comparison candidate.
///
/// The bounded floor and full search are separate replay fixed points in
/// default mode: either immediate plan can reach the smaller final stream.
/// Max mode runs only the floor here because its later source-max route covers
/// the remaining full search; repeating the sibling first would consume time
/// reserved for that broader route. The 4,000-token/80-KiB eligibility band is
/// enforced by
/// `compact_proven_submatch_route_eligible` before this helper is called.
fn build_compact_proven_feedback_candidate(
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
    // Max may mix several locally valid match spellings before replay. Keep
    // this attached to M3's already-complete compact floor: the floor remains
    // a fallback, while the beam inherits its useful table as the cost seed.
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
    // Stabilize the cheap floor before starting its heavier full-search
    // sibling. Besides securing an early complete candidate for shared
    // deadlines, this avoids making a later local search consume time needed
    // by the floor's distinct replay fixed point.
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
    // Start the default-only sibling from its own ordinary price. This retains
    // the full search's established tie order while the floor above remains an
    // independent complete candidate.
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

/// Stabilize both replay endpoints reachable from one compact seed.
///
/// Ordinary replay and repeated proven-before-feedback replay can each win.
/// Keeping this small helper shared by both seed lineages makes that necessary
/// comparison explicit without rebuilding the seed plan itself.
fn build_compact_proven_seed_candidate(
    source: CandidateInput<'_>,
    plan: PlannedBlock,
    route_options: &Options,
    options: &Options,
    route: &'static str,
    expired: &mut SearchStop<'_>,
) -> Result<Candidate> {
    // The first proven-before-feedback pass exposes two distinct replay fixed
    // points. Ordinary replays can stabilize a better table for some streams,
    // while preserving the integrated ordering through every replay wins on
    // others. Both are compact, bounded siblings of the same initial plan.
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
    // Balanced-tree cleanup follows each seed lineage independently. Comparing
    // the pre-cleanup candidates first can hide a locally dearer seed whose
    // table exposes the smaller completed tree endpoint.
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

/// Build a compact multi-block proven-before-feedback comparison candidate.
///
/// This recreates the integrated floor used before proven resegmentation was
/// separated into an endpoint lineage, but retains it as an additive complete
/// stream. The caller enforces the four-block/16-KiB-token work bound.
fn build_compact_integrated_proven_feedback_candidate(
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
    // Ordinary replay is the cheaper historical endpoint and already recovers
    // many compact multi-block wins. Only follow the second integrated replay
    // fixed point when that completed candidate did not improve the incumbent;
    // this keeps the route adaptive without using corpus- or time-based gates.
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

fn build_candidate_with_proven_policy(
    source: CandidateInput<'_>,
    options: &Options,
    replay_limit: usize,
    run_proven_lineage: bool,
    expired: &mut SearchStop<'_>,
) -> Result<Candidate> {
    let Some(plans) = plan_stream(source.blocks, 0, options, &mut *expired) else {
        return source_candidate(source, options);
    };
    let mut candidate = build_candidate_from_plans(
        source,
        plans,
        options,
        replay_limit,
        ReplayPlanner::Full,
        expired,
    )?;
    if !run_proven_lineage || expired.reached() {
        return Ok(candidate);
    }

    // Stabilize proven-submatch resegmentation as an independent lineage.
    // Selecting an immediate local win inside `plan_stream` can change the
    // parent token state of later replay and hide a better established fixed
    // point. Compare the completed route and its endpoint branch as complete
    // emitted streams.
    let endpoint_proven = {
        // Reparse the completed ordinary candidate so any retained original
        // representations refer to these exact bytes. Re-emitting its plan
        // list against the earlier source would be invalid after an accepted
        // replay, because original-bit references are generation-local.
        let replayed =
            parse_validated_rewrite(&candidate.data, source.decoded_limit, source.identity)?;
        let proven_source =
            rewritten_input(&candidate, &replayed, source.decoded_limit, source.identity);
        plan_proven_submatch_route(&replayed.blocks, 0, options, &mut *expired)
            .map(|proven_plans| {
                build_candidate_from_plans(
                    proven_source,
                    proven_plans,
                    options,
                    replay_limit,
                    ReplayPlanner::Full,
                    expired,
                )
            })
            .transpose()?
    };
    if let Some(proven) = endpoint_proven {
        candidate.replace_if_smaller(proven);
    }
    Ok(candidate)
}

/// Build an additive max candidate after its caller retained the complete
/// ordinary-mode floor.
///
/// This avoids rebuilding the same token-preserving floor inside source max;
/// the planner still carries a complete structural fallback of its own.
fn build_candidate_from_established_floor(
    source: CandidateInput<'_>,
    options: &Options,
    replay_limit: usize,
    integrated_compact_proven: bool,
    expired: &mut SearchStop<'_>,
) -> Result<Candidate> {
    let Some(plans) = plan_stream_from_established_floor(
        source.blocks,
        0,
        options,
        integrated_compact_proven,
        &mut *expired,
    ) else {
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
    )
}

#[derive(Clone, Copy)]
enum ReplayPlanner {
    Full,
    IntegratedProven,
    Fragmented,
}

fn resolved_replay_limit(requested: usize, emitted_bytes: usize) -> usize {
    if requested == MAX_RAW_REPLAY_LIMIT {
        emitted_bytes.saturating_mul(8).max(1)
    } else {
        requested
    }
}

/// Emit an explicitly selected structural seed and stabilize strict replays.
///
/// Most callers obtain their first plans from [`plan_stream`]. Alternate
/// collection strategies intentionally need to preserve a locally dearer seed,
/// so accepting the initial plan list here prevents the ordinary planner from
/// replacing it before its new block boundaries have been reparsed.
fn build_candidate_from_plans(
    source: CandidateInput<'_>,
    plans: Vec<PlannedBlock>,
    options: &Options,
    replay_limit: usize,
    replay_planner: ReplayPlanner,
    expired: &mut SearchStop<'_>,
) -> Result<Candidate> {
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
fn build_candidate_from_plans_with_progress(
    source: CandidateInput<'_>,
    mut plans: Vec<PlannedBlock>,
    options: &Options,
    replay_limit: usize,
    replay_planner: ReplayPlanner,
    expired: &mut SearchStop<'_>,
    progress: Option<&RouteProgress>,
) -> Result<Candidate> {
    let (mut data, mut bits) = emit_plans(source.compressed, &plans, options.strict)?;
    // `data.len() == ceil(bits / 8)`. At a fixed byte length there are at most
    // eight meaningful-bit scores, and accepted rounds never increase byte
    // length. Eight times the emitted length therefore bounds every possible
    // strict score improvement, even when strict repair initially grows an
    // incompatible source stream.
    let replay_limit = resolved_replay_limit(replay_limit, data.len());
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
        if expired.reached() {
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
                Some(progress) => plan_stream_with_progress(
                    &replayed.blocks,
                    0,
                    options,
                    options.exhaustive,
                    false,
                    &mut *expired,
                    progress,
                ),
                None if options.exhaustive => plan_stream_from_established_floor(
                    &replayed.blocks,
                    0,
                    options,
                    false,
                    &mut *expired,
                ),
                None => plan_stream(&replayed.blocks, 0, options, &mut *expired),
            },
            ReplayPlanner::IntegratedProven => {
                if let [block] = replayed.blocks.as_slice() {
                    Some(vec![plan_block_with_integrated_proven_search(
                        block,
                        0,
                        options,
                        &mut *expired,
                    )])
                } else {
                    plan_integrated_proven_source_route(&replayed.blocks, 0, options, &mut *expired)
                }
            }
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
                && !expired.reached();
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

    let block_report = capture_planned_block_report(&plans, reports_enabled(options));
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
        return Err(Error::internal(
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
        return Err(Error::internal(
            "internal strict Deflate plan has an incomplete Huffman code",
        ));
    }

    let planned_bits = plans
        .iter()
        .try_fold(0_u64, |total, plan| total.checked_add(plan.bits));
    let planned_bits = planned_bits.ok_or_else(|| Error::new("Deflate output is too large"))?;
    let mut writer = BitWriter::with_capacity_bits(planned_bits)?;
    for (index, plan) in plans.iter().enumerate() {
        emit_block(&mut writer, input, plan, index + 1 == plans.len())?;
    }
    let bits = writer.bit_position();
    debug_assert_eq!(bits, planned_bits);
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
        total_bits: plans.iter().map(|plan| plan.bits).sum(),
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
    let total_bits = blocks.iter().try_fold(0_u64, |total, block| {
        Some(total.saturating_add(block.original?.len))
    })?;
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
        total_bits,
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

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::time::Duration;

    use super::super::stop::{
        initial_bounded_phase_share, TIMEOUT_GRACE_BASE, TIMEOUT_GRACE_DIVISOR,
    };
    use super::*;
    use crate::deflate::bitstream::BitWriter;
    use crate::deflate::huffman::{fixed_trees, huffman_tree_shape_is_complete};
    use crate::deflate::model::Token;

    const FEEDBACK_RAW: &[u8] = &[
        0x25, 0xc0, 0x01, 0x01, 0xc0, 0x30, 0x0c, 0xc3, 0x30, 0x6c, 0xb5, 0x9b, 0xf0, 0x87, 0xf4,
        0x7d, 0xd3, 0xcc, 0xcc, 0xcc, 0xcc, 0x01, 0x00, 0x00, 0xc0, 0x71, 0x5d, 0xaa, 0xaa, 0xaa,
        0xfe, 0x76, 0x77, 0x93, 0x24, 0x49, 0x9e, 0xa7, 0x6d, 0xdb, 0xf6, 0x03,
    ];
    const RLE_SMOOTHING_PNG: &[u8] =
        include_bytes!("../../tests/fixtures/png/PngSuite/tbbn2c16.png");

    fn png_raw_deflate(input: &[u8]) -> Vec<u8> {
        assert_eq!(&input[..8], b"\x89PNG\r\n\x1a\n");
        let mut offset = 8;
        let mut zlib = Vec::new();
        while offset + 12 <= input.len() {
            let length = u32::from_be_bytes(input[offset..offset + 4].try_into().unwrap()) as usize;
            let kind = &input[offset + 4..offset + 8];
            let data_start = offset + 8;
            let data_end = data_start + length;
            assert!(data_end + 4 <= input.len());
            if kind == b"IDAT" {
                zlib.extend_from_slice(&input[data_start..data_end]);
            }
            offset = data_end + 4;
            if kind == b"IEND" {
                break;
            }
        }
        assert!(zlib.len() >= 6);
        zlib[2..zlib.len() - 4].to_vec()
    }

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
    fn smoothed_tree_floor_rebuilds_multiple_huffman_blocks_exactly() {
        let raw = png_raw_deflate(RLE_SMOOTHING_PNG);
        let parsed = parse_stream(&raw, 1 << 20).unwrap();
        let [block] = parsed.blocks.as_slice() else {
            panic!("fixture must contain one source block");
        };
        assert_eq!(block.source_type, SourceBlockType::Dynamic);
        let original = block.original.unwrap();
        let plan = PlannedBlock {
            tokens: block.tokens.clone(),
            plain: block.plain.clone(),
            bits: original.len,
            representation: Representation::Original(original),
            source_type: block.source_type,
        };
        let (data, bits) = emit_plans(&raw, &[plan.clone(), plan.clone()], true).unwrap();
        let combined = parse_stream(&data, 1 << 20).unwrap();
        assert_eq!(combined.blocks.len(), 2);
        let identity = StreamIdentity {
            decoded_size: combined.decoded_size,
            crc32: combined.crc32,
            adler32: combined.adler32,
        };
        let candidate = Candidate {
            data,
            bits,
            plans: Vec::new(),
            block_report: None,
            route: "test",
            max_planner_is_stable: false,
        };

        let refined = refine_with_compact_payload_tree_floor(
            &candidate,
            &Options::default(),
            1 << 20,
            identity,
        )
        .unwrap()
        .1
        .unwrap();

        assert!(refined.bits < candidate.bits);
        let reparsed = parse_validated_rewrite(&refined.data, 1 << 20, identity).unwrap();
        assert_eq!(reparsed.blocks.len(), 2);
        assert!(reparsed
            .blocks
            .iter()
            .all(|block| block.source_type == SourceBlockType::Dynamic));

        let (data, bits) = emit_plans(&raw, &vec![plan; 9], true).unwrap();
        let combined = parse_stream(&data, 1 << 20).unwrap();
        let identity = StreamIdentity {
            decoded_size: combined.decoded_size,
            crc32: combined.crc32,
            adler32: combined.adler32,
        };
        let candidate = Candidate {
            data,
            bits,
            plans: Vec::new(),
            block_report: None,
            route: "test",
            max_planner_is_stable: false,
        };
        let (covered, compact) = refine_with_compact_payload_tree_floor(
            &candidate,
            &Options::default(),
            1 << 20,
            identity,
        )
        .unwrap();
        assert!(!covered);
        assert!(compact.is_none());
        refine_with_bounded_depth_tree_floor(
            &candidate,
            &Options::default(),
            1 << 20,
            identity,
            &mut SearchStop::never(),
        )
        .unwrap();
    }

    #[test]
    fn changed_parent_no_split_requires_a_new_best_topology() {
        let parent = comparison_candidate(12, 90, 1);
        let no_split = comparison_candidate(10, 80, 2);
        let refined = comparison_candidate(9, 79, 3);

        assert!(changed_parent_no_split_should_continue(
            true,
            &refined,
            &parent,
            Some(&no_split),
        ));
        assert!(!changed_parent_no_split_should_continue(
            false,
            &refined,
            &parent,
            Some(&no_split),
        ));
        assert!(!changed_parent_no_split_should_continue(
            true,
            &no_split,
            &parent,
            Some(&refined),
        ));
        assert!(!changed_parent_no_split_should_continue(
            true, &refined, &parent, None,
        ));
    }

    #[test]
    fn changed_narrow_parent_continues_only_while_nondominated() {
        let compressed = [0_u8; 12];
        let source = CandidateInput {
            compressed: &compressed,
            blocks: &[],
            meaningful_bits: 90,
            decoded_limit: 0,
            identity: StreamIdentity {
                decoded_size: 0,
                crc32: 0,
                adler32: 0,
            },
        };
        let narrow = comparison_candidate(10, 80, 1);
        let weaker = comparison_candidate(11, 81, 2);
        let tied = comparison_candidate(10, 80, 3);
        let better = comparison_candidate(9, 79, 4);

        assert!(changed_narrow_parent_should_continue(
            true,
            &narrow,
            source,
            &[Some(&weaker), Some(&tied)],
        ));
        assert!(!changed_narrow_parent_should_continue(
            false,
            &narrow,
            source,
            &[Some(&weaker)],
        ));
        assert!(!changed_narrow_parent_should_continue(
            true,
            &narrow,
            source,
            &[Some(&better)],
        ));

        let unchanged = comparison_candidate(12, 90, 5);
        assert!(!changed_narrow_parent_should_continue(
            true,
            &unchanged,
            source,
            &[],
        ));
    }

    #[test]
    fn no_split_replay_requires_a_new_parent_state() {
        let compressed = [0_u8];
        let source = CandidateInput {
            compressed: &compressed,
            blocks: &[],
            meaningful_bits: 8,
            decoded_limit: 0,
            identity: StreamIdentity {
                decoded_size: 0,
                crc32: 0,
                adler32: 0,
            },
        };
        let mut candidate = comparison_candidate(1, 7, 1);

        // Different output bytes alone can be only a better Huffman header;
        // Max already priced that token state, so Default must not repeat it.
        assert!(!candidate_exposes_new_parent(&candidate, source));

        candidate.plans.push(PlannedBlock {
            tokens: Arc::new(vec![Token::Literal(0)]),
            plain: Arc::new(vec![0]),
            representation: Representation::Fixed,
            bits: 10,
            source_type: SourceBlockType::Fixed,
        });
        assert!(candidate_exposes_new_parent(&candidate, source));

        candidate.data.copy_from_slice(&compressed);
        assert!(!candidate_exposes_new_parent(&candidate, source));
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
        let input = FEEDBACK_RAW;
        let source = parse_stream(input, 86).unwrap();
        assert_eq!(source.meaningful_bits, 331);
        assert_eq!(source.decoded_size, 86);

        let options = Options {
            strict: false,
            ..Options::default()
        };
        let first = optimize_raw(input, &options).unwrap();
        // Later additive structural routes may beat the recovered 325-bit
        // fixed point, but must never lose it.
        assert!(first.info.deflate_bits <= 325);
        let second = optimize_raw(&first.data, &options).unwrap();
        assert_eq!(second.info.deflate_bits, first.info.deflate_bits);
        assert_eq!(second.data, first.data);

        // A zero-budget max run keeps a complete source fallback instead of
        // making the comparison floor an unbounded timeout exception.
        let zero_budget_max = Options {
            exhaustive: true,
            strict: false,
            timeout: Duration::ZERO,
            ..Options::default()
        };
        let stopped = optimize_raw(input, &zero_budget_max).unwrap();
        assert!(stopped.timed_out);
        assert_eq!(stopped.info.deflate_bits, source.meaningful_bits);
        assert_eq!(stopped.data, input);

        // With sufficient time, max mode closes the same feedback graph in one
        // invocation and is itself at a fixed point.
        let max_options = Options {
            timeout: Duration::from_secs(1),
            ..zero_budget_max
        };
        let maximum = optimize_raw(input, &max_options).unwrap();
        assert!(maximum.info.deflate_bits <= first.info.deflate_bits);
        let repeated_max = optimize_raw(&maximum.data, &max_options).unwrap();
        assert_eq!(repeated_max.data, maximum.data);
    }

    #[test]
    fn sufficient_time_max_floors_retain_their_exact_default_endpoint() {
        let default_options = Options {
            strict: false,
            timeout: Duration::from_secs(1),
            ..Options::default()
        };
        let default = optimize_raw_prefix_with_floor(
            FEEDBACK_RAW,
            &default_options,
            86,
            DefaultFloor::Shared,
        )
        .unwrap();
        let apng_default = optimize_raw_prefix_with_floor(
            FEEDBACK_RAW,
            &default_options,
            86,
            DefaultFloor::ApngDefault,
        )
        .unwrap();
        let max_options = Options {
            exhaustive: true,
            ..default_options
        };

        for (floor, expected) in [
            (DefaultFloor::Complete, &default),
            (DefaultFloor::SharedExact, &default),
            (DefaultFloor::ApngMax, &apng_default),
        ] {
            let maximum =
                optimize_raw_prefix_with_floor(FEEDBACK_RAW, &max_options, 86, floor).unwrap();
            assert!(maximum.data.len() <= expected.data.len(), "{floor:?}");
            assert!(
                maximum.info.deflate_bits <= expected.info.deflate_bits,
                "{floor:?}"
            );
        }
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
    fn every_potentially_improvable_source_topology_admits_the_deft4j_route() {
        // A one-literal fixed stream represents the single-block members used
        // by GZIP, ZIP, and compressed PNG metadata. Container floor policy
        // must not make its direct deft4j endpoint unreachable.
        let parsed = parse_stream(&[0x4b, 0x04, 0x00], 1).unwrap();
        assert_eq!(parsed.decoded_size, 1);
        assert!(deft4j_source_route_eligible(&parsed.blocks));

        // Route memory accounting, rather than an arbitrary source-list cap,
        // controls very fragmented streams.
        let many = (0..129)
            .map(|_| parsed.blocks[0].try_clone_shared().unwrap())
            .collect::<Vec<_>>();
        assert!(deft4j_source_route_eligible(&many));
    }

    #[test]
    fn no_split_route_uses_bounded_source_size_and_topology() {
        let parsed = parse_stream(&[0x4b, 0x04, 0x00], 1).unwrap();
        let blocks = vec![
            parsed.blocks[0].try_clone_shared().unwrap(),
            parsed.blocks[0].try_clone_shared().unwrap(),
        ];

        assert!(narrow_source_route_eligible(
            &blocks,
            NARROW_SOURCE_MAX_COMPRESSED
        ));
        assert!(!narrow_source_route_eligible(
            &blocks,
            NARROW_SOURCE_MAX_COMPRESSED + 1
        ));

        let too_many = (0..=NARROW_SOURCE_LIST_MAX_BLOCKS)
            .map(|_| parsed.blocks[0].try_clone_shared().unwrap())
            .collect::<Vec<_>>();
        assert!(!narrow_source_route_eligible(&too_many, 1));

        let mut stored = blocks;
        stored[0].source_type = SourceBlockType::Stored;
        assert!(!narrow_source_route_eligible(&stored, 1));
    }

    #[test]
    fn max_replay_ceiling_covers_every_strict_stream_score() {
        // Meaningful bits can occupy only the eight residues belonging to each
        // emitted byte length. Max therefore gets a complete metric bound,
        // while Default retains its small consistent replay budget.
        assert_eq!(resolved_replay_limit(MAX_RAW_REPLAY_LIMIT, 10), 80);
        assert_eq!(resolved_replay_limit(DEFAULT_RAW_REPLAY_LIMIT, 10), 3);
        assert_eq!(resolved_replay_limit(MAX_RAW_REPLAY_LIMIT, 0), 1);
        assert_eq!(
            resolved_replay_limit(MAX_RAW_REPLAY_LIMIT, usize::MAX),
            usize::MAX
        );
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
        let completed = build_candidate(
            source,
            &options,
            DEFAULT_RAW_REPLAY_LIMIT,
            &mut SearchStop::never(),
        )
        .unwrap();
        let expected_data = completed.data.clone();
        let expected_bits = completed.bits;

        let mut must_not_build =
            || panic!("reusing a completed floor must not start another build");
        let reused = completed_or_bounded_floor(
            source,
            &options,
            Some(completed),
            &mut SearchStop::callback(&mut must_not_build),
        )
        .unwrap();

        assert_eq!(reused.data, expected_data);
        assert_eq!(reused.bits, expected_bits);
        assert_eq!(reused.route, "Normal floor");
    }

    #[test]
    fn established_floor_is_a_complete_strict_max_candidate() {
        let source = [0x03, 0x00];
        let established = optimize_raw(&source, &Options::default()).unwrap();
        let options = Options {
            exhaustive: true,
            timeout: Duration::ZERO,
            ..Options::default()
        };

        let maximum = optimize_raw_prefix_with_floor(
            &established.data,
            &options,
            options.max_decoded_bytes,
            DefaultFloor::Established,
        )
        .unwrap();

        assert!(maximum.timed_out);
        assert_eq!(maximum.data, established.data);
        assert_eq!(maximum.info.deflate_bits, established.info.deflate_bits);
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
        let plans: Vec<_> = parsed
            .blocks
            .iter()
            .map(|block| {
                let original = block.original.expect("parsed block has source bits");
                PlannedBlock {
                    tokens: Arc::clone(&block.tokens),
                    plain: Arc::clone(&block.plain),
                    representation: Representation::Original(original),
                    bits: original.len,
                    source_type: block.source_type,
                }
            })
            .collect();
        assert!(compact_split_preserves_source_blocks(
            &parsed.blocks,
            &plans
        ));
        let mut changed_plans = plans.clone();
        changed_plans[0].plain = vec![b'a'; 127].into();
        assert!(!compact_split_preserves_source_blocks(
            &parsed.blocks,
            &changed_plans
        ));
        let mut changed_type = plans;
        changed_type[0].representation = Representation::Stored;
        assert!(!compact_split_preserves_source_blocks(
            &parsed.blocks,
            &changed_type
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

        let mut tree_block = parsed.blocks[0].clone();
        tree_block.source_type = SourceBlockType::Dynamic;
        tree_block.tokens = vec![Token::Literal(b'a')].into();
        assert!(compact_balanced_tree_source_eligible(
            1,
            tree_block.plain.len() as u64,
            std::slice::from_ref(&tree_block),
        ));
        assert!(!compact_balanced_tree_source_eligible(
            COMPACT_TREE_MAX_COMPRESSED + 1,
            tree_block.plain.len() as u64,
            std::slice::from_ref(&tree_block),
        ));
        assert!(!compact_balanced_tree_source_eligible(
            1,
            tree_block.plain.len() as u64,
            &[tree_block.clone(), tree_block.clone()],
        ));

        tree_block.distance_frequencies = [0; 30];
        assert!(compact_strict_literal_tree_eligible(
            1,
            std::slice::from_ref(&tree_block),
        ));
        tree_block.distance_frequencies[0] = 1;
        assert!(!compact_strict_literal_tree_eligible(
            1,
            std::slice::from_ref(&tree_block),
        ));
    }

    #[test]
    fn compact_split_parents_are_ordered_by_complete_stream_score() {
        fn seed(bytes: usize, bits: u64) -> CompactSplitSeed {
            CompactSplitSeed {
                data: vec![0; bytes],
                bits,
                stream: parse_stream(&[0x03, 0x00], 1).unwrap(),
            }
        }

        let largest = seed(12, 80);
        let bit_tie = seed(10, 79);
        let smallest = seed(10, 78);
        let ordered =
            ordered_compact_split_seeds([Some(&largest), Some(&bit_tie), Some(&smallest)]);

        assert_eq!(
            ordered
                .iter()
                .map(|seed| (seed.data.len(), seed.bits))
                .collect::<Vec<_>>(),
            [(10, 78), (10, 79), (12, 80)]
        );

        let tied_first = seed(10, 78);
        let tied_second = seed(10, 78);
        let ordered =
            ordered_compact_split_seeds([Some(&tied_first), Some(&tied_second), Some(&largest)]);
        assert!(std::ptr::eq(ordered[0], &tied_first));
        assert!(std::ptr::eq(ordered[1], &tied_second));
    }

    #[test]
    fn completed_compact_split_parent_requires_exact_encoded_identity() {
        let candidate = comparison_candidate(4, 29, 7);

        assert!(compact_split_parent_is_completed(
            &candidate,
            Some(&[7, 7, 7, 7])
        ));
        assert!(!compact_split_parent_is_completed(
            &candidate,
            Some(&[7, 7, 7, 6])
        ));
        assert!(!compact_split_parent_is_completed(&candidate, None));
    }

    #[test]
    fn bounded_generic_routes_preserve_complete_candidates() {
        // Empty stored blocks have no useful deft4j or narrow source route.
        // They make a small generic-only stream whose floor and original-source
        // max routes can be checked independently after their parallel phase
        // rejoins.
        let input = [
            0x00, 0x00, 0x00, 0xff, 0xff, // Non-final empty stored block.
            0x01, 0x00, 0x00, 0xff, 0xff, // Final empty stored block.
        ];
        let parsed = parse_stream(&input, 1).unwrap();
        assert_eq!(parsed.source_block_count, 2);
        assert!(!deft4j_source_route_eligible(&parsed.blocks));
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
            decoded_limit: 1,
            identity,
        };
        let deadline = Deadline::new(Instant::now(), Duration::MAX);
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
            None,
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
    fn bounded_floor_prebuild_uses_topology_and_route_work() {
        assert!(prebuild_bounded_floor(1, 1));
        assert!(prebuild_bounded_floor(
            2,
            PREBUILD_BOUNDED_FLOOR_MAX_DECODED
        ));
        assert!(!prebuild_bounded_floor(
            2,
            PREBUILD_BOUNDED_FLOOR_MAX_DECODED + 1
        ));
        assert!(!prebuild_bounded_floor(
            2,
            CONCURRENT_BOUNDED_FLOOR_MAX_DECODED
        ));
        assert!(prebuild_bounded_floor(
            2,
            CONCURRENT_BOUNDED_FLOOR_MAX_DECODED + 1
        ));

        let source_match = Token::Match {
            length: 3,
            distance: 1,
            length_symbol: 257,
            distance_symbol: 0,
            length_extra: 0,
            distance_extra: 0,
            length_extra_bits: 0,
            distance_extra_bits: 0,
        };
        let block = ParsedBlock {
            tokens: Arc::new(vec![source_match; PROVEN_SUBMATCH_FULL_MATCH_LIMIT + 1]),
            plain: Arc::new(Vec::new()),
            literal_frequencies: [0; 286],
            distance_frequencies: [0; 30],
            original_literal_lengths: None,
            original_distance_lengths: None,
            original_dynamic: None,
            original: None,
            source_splits: Vec::new(),
            source_type: SourceBlockType::Dynamic,
        };
        assert!(source_run_match_count_exceeds(
            std::slice::from_ref(&block),
            PROVEN_SUBMATCH_FULL_MATCH_LIMIT
        ));
    }

    #[test]
    fn early_max_lineage_probe_rejects_trailing_data() {
        assert!(!raw_source_benefits_from_early_max_lineage(&[0x03, 0x00], 0).unwrap());
        let error = raw_source_benefits_from_early_max_lineage(&[0x03, 0x00, 0xff], 0).unwrap_err();
        assert_eq!(error.message(), "trailing data after Deflate stream");
    }

    #[test]
    fn early_max_lineage_covers_only_distinct_or_serial_work() {
        assert!(early_transformed_lineage_is_useful(1, 1, true));
        assert!(early_transformed_lineage_is_useful(
            2,
            PREBUILD_BOUNDED_FLOOR_MAX_DECODED,
            false,
        ));
        assert!(!early_transformed_lineage_is_useful(
            2,
            PREBUILD_BOUNDED_FLOOR_MAX_DECODED + 1,
            false,
        ));
        assert!(!early_transformed_lineage_is_useful(1, 1, false));
    }

    #[test]
    fn complete_png_floor_reuses_the_full_default_route_sequence() {
        let input = [
            0x65, 0xc1, 0x31, 0x01, 0x00, 0x00, 0x00, 0xc2, 0xa0, 0x6c, 0xf4, 0x2f, 0xe5, 0x3f,
            0x41, 0x29, 0xa5, 0x94, 0x72, 0x06,
        ];
        let parsed = parse_stream(&input, 120).unwrap();
        let source = CandidateInput {
            compressed: &input,
            blocks: &parsed.blocks,
            meaningful_bits: parsed.meaningful_bits,
            decoded_limit: 120,
            identity: StreamIdentity {
                decoded_size: parsed.decoded_size,
                crc32: parsed.crc32,
                adler32: parsed.adler32,
            },
        };
        let options = Options {
            exhaustive: true,
            timeout: Duration::MAX,
            ..Options::default()
        };
        let deadline = Deadline::new(Instant::now(), Duration::MAX);
        let base =
            build_bounded_floor_candidate(source, &options, &mut SearchStop::never()).unwrap();
        let complete = build_complete_default_floor_candidate(
            source,
            &options,
            &deadline,
            Progress::begin(
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
                None,
            ),
        )
        .unwrap()
        .complete;
        let ordinary = optimize_raw(&input, &Options::default()).unwrap();

        assert!(
            complete.data.len() < base.data.len()
                || (complete.data.len() == base.data.len() && complete.bits <= base.bits)
        );
        assert_eq!(complete.data, ordinary.data);
        assert_eq!(complete.bits, ordinary.info.deflate_bits);
    }

    #[test]
    fn route_errors_cancel_siblings_without_marking_a_timeout() {
        let deadline = Deadline::new(Instant::now(), Duration::MAX);
        let failed: Result<()> =
            run_route_with_cancellation(&deadline, || Err(Error::new("synthetic route failure")));

        assert!(failed.is_err());
        assert!(deadline.route_should_stop());
        assert!(!deadline.was_triggered());

        let successful = Deadline::new(Instant::now(), Duration::MAX);
        let completed: Result<()> = run_route_with_cancellation(&successful, || Ok(()));
        assert!(completed.is_ok());
        assert!(!successful.route_should_stop());
    }

    #[test]
    fn timeout_grace_is_ten_percent_plus_one_second_and_disabled_at_zero() {
        assert_eq!(timeout_grace(Duration::ZERO), Duration::ZERO);
        assert_eq!(
            timeout_grace(Duration::from_secs(10)),
            TIMEOUT_GRACE_BASE + Duration::from_secs(10) / TIMEOUT_GRACE_DIVISOR
        );
        assert_eq!(
            timeout_grace(Duration::from_secs(4_000)),
            Duration::from_secs(401)
        );
    }

    #[test]
    fn initial_bounded_routes_leave_one_fifth_for_follow_up_work() {
        assert_eq!(
            initial_bounded_phase_share(Duration::from_secs(100)),
            Duration::from_secs(80)
        );
        assert_eq!(
            initial_bounded_phase_share(Duration::from_secs(10)),
            Duration::from_secs(8)
        );
    }

    #[test]
    fn soft_deadline_stops_new_routes_before_active_work() {
        let duration = Duration::from_secs(10);
        let grace = timeout_grace(duration);
        let inside_grace = Deadline::new(
            Instant::now()
                .checked_sub(duration + grace / 2)
                .expect("test duration fits in Instant"),
            duration,
        );
        assert!(!inside_grace.can_start_route());
        assert!(!inside_grace.expired());
        assert!(inside_grace.was_triggered());

        let past_hard_deadline = Deadline::new(
            Instant::now()
                .checked_sub(duration + grace + Duration::from_millis(1))
                .expect("test duration fits in Instant"),
            duration,
        );
        assert!(past_hard_deadline.expired());
        assert!(past_hard_deadline.was_triggered());
    }

    #[test]
    fn weak_deft4j_threshold_is_strictly_below_two_percent() {
        assert!(gain_is_below(10_000, 9_801, 200));
        assert!(!gain_is_below(10_000, 9_800, 200));
        assert!(!gain_is_below(10_000, 9_000, 200));
    }

    #[test]
    fn best_first_floor_seed_respects_the_structural_split_signal() {
        assert!(!floor_seeded_priority_with_structural_sibling(
            10_000, 9_801, true, false,
        ));
        assert!(floor_seeded_priority_with_structural_sibling(
            10_000, 9_800, true, false,
        ));
        assert!(floor_seeded_priority_with_structural_sibling(
            10_000, 9_999, false, false,
        ));
        assert!(floor_seeded_priority_with_structural_sibling(
            10_000, 9_999, true, true,
        ));
    }

    #[test]
    fn independent_refinement_overlaps_only_inside_the_bounded_parallel_class() {
        assert!(independent_deft4j_refinement_can_overlap(
            true, true, true, true
        ));
        assert!(!independent_deft4j_refinement_can_overlap(
            false, true, true, true
        ));
        assert!(!independent_deft4j_refinement_can_overlap(
            true, false, true, true
        ));
        assert!(!independent_deft4j_refinement_can_overlap(
            true, true, false, true
        ));
        assert!(!independent_deft4j_refinement_can_overlap(
            true, true, true, false
        ));
    }

    #[test]
    fn bounded_png_max_policy_follows_available_route_families() {
        assert_eq!(
            bounded_png_max_policy(1, false, true, true),
            BoundedPngMaxPolicy::Standard
        );
        assert_eq!(
            bounded_png_max_policy(1, true, true, true),
            BoundedPngMaxPolicy::FloorExpansion
        );
        for blocks in [2, 4, 13, 32, 33] {
            assert_eq!(
                bounded_png_max_policy(blocks, false, true, true),
                BoundedPngMaxPolicy::FloorExpansion
            );
        }

        // Generic-only streams keep source max beside the floor lineage,
        // regardless of source block count.
        for blocks in [2, 13] {
            assert_eq!(
                bounded_png_max_policy(blocks, false, false, false),
                BoundedPngMaxPolicy::GenericParallel
            );
        }
    }

    #[test]
    fn floor_state_probe_detects_token_and_boundary_changes() {
        fn parsed(tokens: Vec<Token>, plain: Vec<u8>) -> ParsedBlock {
            ParsedBlock {
                tokens: Arc::new(tokens),
                plain: Arc::new(plain),
                literal_frequencies: [0; 286],
                distance_frequencies: [0; 30],
                original_literal_lengths: None,
                original_distance_lengths: None,
                original_dynamic: None,
                original: None,
                source_splits: Vec::new(),
                source_type: SourceBlockType::Dynamic,
            }
        }
        fn planned(tokens: Vec<Token>, plain: Vec<u8>) -> PlannedBlock {
            PlannedBlock {
                tokens: Arc::new(tokens),
                plain: Arc::new(plain),
                representation: Representation::Fixed,
                bits: 0,
                source_type: SourceBlockType::Dynamic,
            }
        }

        let source = [parsed(
            vec![Token::Literal(b'a'), Token::Literal(b'b')],
            b"ab".to_vec(),
        )];
        let same = [planned(
            vec![Token::Literal(b'a'), Token::Literal(b'b')],
            b"ab".to_vec(),
        )];
        assert!(!floor_exposes_new_search_states(&source, &same));

        let changed_tokens = [planned(
            vec![Token::Literal(b'a'), Token::Literal(b'c')],
            b"ab".to_vec(),
        )];
        assert!(floor_exposes_new_search_states(&source, &changed_tokens));

        let changed_boundaries = [
            planned(vec![Token::Literal(b'a')], b"a".to_vec()),
            planned(vec![Token::Literal(b'b')], b"b".to_vec()),
        ];
        assert!(floor_exposes_new_search_states(
            &source,
            &changed_boundaries
        ));
    }

    #[test]
    fn compact_max_scheduling_follows_dependency_topology() {
        let block = ParsedBlock {
            tokens: Arc::new(vec![Token::Literal(0)]),
            plain: Arc::new(vec![0]),
            literal_frequencies: [0; 286],
            distance_frequencies: [0; 30],
            original_literal_lengths: None,
            original_distance_lengths: None,
            original_dynamic: None,
            original: None,
            source_splits: Vec::new(),
            source_type: SourceBlockType::Dynamic,
        };
        let compressed = [0_u8];
        let identity = StreamIdentity {
            decoded_size: 2,
            crc32: 0,
            adler32: 0,
        };
        let one_block = [block.clone()];
        let one_block_source = CandidateInput {
            compressed: &compressed,
            blocks: &one_block,
            meaningful_bits: 1,
            decoded_limit: 2,
            identity,
        };
        assert!(compact_parallel_source_max_work_class(one_block_source));
        assert!(compact_complementary_source_max_is_cheap(one_block_source));
        assert!(bounded_parallel_source_max_work_class(one_block_source));
        assert!(!compact_dependent_deft4j_work_class(one_block_source));

        let two_blocks = [block.clone(), block];
        let two_block_source = CandidateInput {
            compressed: &compressed,
            blocks: &two_blocks,
            meaningful_bits: 1,
            decoded_limit: 2,
            identity,
        };
        assert!(!compact_parallel_source_max_work_class(two_block_source));
        assert!(compact_complementary_source_max_is_cheap(two_block_source));
        assert!(bounded_parallel_source_max_work_class(two_block_source));
        assert!(compact_dependent_deft4j_work_class(two_block_source));
        assert!(!repartition_graph_covers_source_blocks(&two_blocks, 1));
        assert!(repartition_graph_covers_source_blocks(&two_blocks, 2));

        let large_token_block = ParsedBlock {
            tokens: Arc::new(vec![
                Token::Literal(0);
                COMPACT_COMPLEMENTARY_SOURCE_MAX_TOKENS + 1
            ]),
            ..two_blocks[0].clone()
        };
        let large_token_blocks = [large_token_block];
        let large_token_source = CandidateInput {
            compressed: &compressed,
            blocks: &large_token_blocks,
            meaningful_bits: 1,
            decoded_limit: 2,
            identity,
        };
        assert!(!compact_complementary_source_max_is_cheap(
            large_token_source
        ));
        assert!(!compact_single_source_route_work_class(large_token_source));
        assert!(repartition_graph_covers_source_blocks(
            &large_token_blocks,
            1
        ));

        let single_route_token_block = ParsedBlock {
            tokens: Arc::new(vec![
                Token::Literal(0);
                COMPACT_SINGLE_SOURCE_ROUTE_MIN_TOKENS
            ]),
            ..large_token_blocks[0].clone()
        };
        let single_route_token_blocks = [single_route_token_block];
        let single_route_token_source = CandidateInput {
            compressed: &compressed,
            blocks: &single_route_token_blocks,
            meaningful_bits: 1,
            decoded_limit: 2,
            identity,
        };
        assert!(compact_single_source_route_work_class(
            single_route_token_source
        ));

        let oversized_token_block = ParsedBlock {
            tokens: Arc::new(vec![
                Token::Literal(0);
                COMPACT_SINGLE_SOURCE_ROUTE_MAX_TOKENS + 1
            ]),
            ..large_token_blocks[0].clone()
        };
        let oversized_token_blocks = [oversized_token_block];
        let oversized_token_source = CandidateInput {
            compressed: &compressed,
            blocks: &oversized_token_blocks,
            meaningful_bits: 1,
            decoded_limit: 2,
            identity,
        };
        assert!(!compact_single_source_route_work_class(
            oversized_token_source
        ));
        assert!(!repartition_graph_covers_source_blocks(
            &oversized_token_blocks,
            1
        ));
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
