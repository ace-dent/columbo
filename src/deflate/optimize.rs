// SPDX-License-Identifier: MIT

use std::thread;
use std::time::Instant;

use crate::progress::{
    reports_enabled, BlockEncoding, BlockProgress, BlockReport, CandidateProgress, Progress,
    RouteProgress, SameDistanceProgress, StreamProgress, MAX_REPORTED_BLOCKS,
};
use crate::{Error, Options, Result};

use super::bitstream::BitWriter;
use super::block::{emit_block, plan_block};
use super::deft4j::plan_source_blocks;
use super::header::{
    plan_columbo_pair_lengthen_candidate, plan_columbo_quad_lengthen_candidate,
    plan_for_explicit_lengths,
};
use super::model::{
    ParsedBlock, ParsedStream, PlannedBlock, Representation, SourceBlockType, Token,
};
use super::parse::{parse_stream, parsed_model_bytes};
use super::search::{
    compact_proven_submatch_route_eligible, improve_plan_with_integrated_proven_floor,
    improve_plan_with_short_family_floor, plan_block_with_integrated_proven_search,
    rewrite_258_symbols, same_distance_opportunities, PROVEN_SUBMATCH_FULL_MATCH_LIMIT,
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
const MAX_RAW_REPLAY_LIMIT: usize = 8;
const DEFT4J_MULTI_BLOCK_MIN_HUFFMAN_BLOCKS: usize = 2;
const DEFT4J_SOURCE_LIST_MAX_BLOCKS: usize = 128;
const WEAK_DEFT4J_GAIN_BASIS_POINTS: u64 = 200;
// The no-split sibling is bounded above because its per-block searches retain
// route-local state. No lower size bound is needed: on compact multi-block
// streams it cheaply reaches later blocks that a broad source-order graph may
// not visit before the deadline.
const NARROW_SOURCE_MAX_COMPRESSED: usize = 512 * 1_024;
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
/// members. [`DefaultFloor::Established`] means the caller already retains the
/// complete input stream as its comparison floor, so descendants can begin
/// without rebuilding an ordinary candidate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DefaultFloor {
    Complete,
    CompleteThenBounded,
    Shared,
    Established,
}

impl DefaultFloor {
    fn is_bounded(self) -> bool {
        !matches!(self, Self::Complete)
    }

    fn uses_bounded_png_routes(self) -> bool {
        matches!(self, Self::CompleteThenBounded)
    }

    fn allows_single_block_deft4j(self) -> bool {
        !matches!(self, Self::Shared | Self::Established)
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
    if input.len() as u64 > options.max_input_bytes {
        return Err(Error::new("input exceeds configured safety limit"));
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
    }
    progress.routes();
    let deadline = Deadline::new(started, options.timeout);

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
            DefaultFloor::Shared => true,
            DefaultFloor::Established => false,
            DefaultFloor::Complete => false,
        };
    let guaranteed_floor_step =
        prebuild_floor_first.then(|| progress.start("Normal comparison floor"));
    let mut complete_default_candidate = None;
    let mut guaranteed_floor_candidate = if default_floor == DefaultFloor::Established {
        Some(established_floor_candidate(source)?)
    } else if prebuild_floor_first {
        Some(if default_floor == DefaultFloor::CompleteThenBounded {
            let floors =
                build_complete_default_floor_candidate(source, options, &deadline, progress)?;
            complete_default_candidate = Some(floors.complete);
            floors.max_seed
        } else {
            build_bounded_floor_candidate(source, options, &mut deadline.hard_stop())?
        })
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
    let compact_proven_feedback_eligible = options.exhaustive
        && default_floor.uses_bounded_png_routes()
        && source.blocks.len() == 1
        && compact_proven_submatch_route_eligible(
            &source.blocks[0].tokens,
            source.blocks[0].plain.len(),
        );
    let deft4j_eligible = options.exhaustive
        && deft4j_source_route_eligible(&blocks, default_floor.allows_single_block_deft4j());

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
                    && compact_parallel_source_max_work_class(source);
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
    let run_bounded_refinement = default_floor.is_bounded() && deadline.can_start_route();
    let refinement_step = (run_bounded_refinement
        && (seed_weak_deft4j || deft4j_candidate.is_some()))
    .then(|| progress.start("deft4j-derived refinement"));
    let run_compact_split_floor = default_floor.uses_bounded_png_routes() && seed_weak_deft4j;
    let compact_split_step =
        run_compact_split_floor.then(|| progress.start("Columbo compact split floor"));
    // Split pricing is not monotone in the parent stream's encoded size: two
    // independently rewritten block topologies can have opposite local
    // ordering after new cuts are inserted. Preserve the ordinary,
    // floor-seeded, and direct deft4j parents while collapsing exact encoded
    // duplicates.
    let compact_split_normal_parent = run_compact_split_floor
        .then_some(bounded_floor_candidate.as_ref())
        .flatten();
    let compact_split_seeded_parent = run_compact_split_floor
        .then_some(floor_seeded_candidate.as_ref())
        .flatten()
        .filter(|seeded| {
            compact_split_normal_parent.map_or(true, |normal| normal.data != seeded.data)
        });
    let compact_split_deft4j_parent = if run_compact_split_floor {
        deft4j_candidate.as_ref().filter(|deft4j| {
            [compact_split_normal_parent, compact_split_seeded_parent]
                .into_iter()
                .flatten()
                .all(|seed| seed.data != deft4j.data)
        })
    } else {
        None
    };
    let compact_split_normal_seed = compact_split_normal_parent
        .map(|candidate| prepare_compact_source_split_seed(candidate, decoded_limit, identity))
        .transpose()?
        .flatten();
    let compact_split_seeded_seed = compact_split_seeded_parent
        .map(|candidate| prepare_compact_source_split_seed(candidate, decoded_limit, identity))
        .transpose()?
        .flatten();
    let compact_split_deft4j_seed = compact_split_deft4j_parent
        .map(|candidate| prepare_compact_source_split_seed(candidate, decoded_limit, identity))
        .transpose()?
        .flatten();
    let mut compact_split_attempted = false;
    let mut compact_split_candidate = None;
    if run_bounded_refinement {
        let run_concurrent_source_max = options.exhaustive
            && default_floor.uses_bounded_png_routes()
            && parallel_routes
            && source_max_candidate.is_none()
            && !suppress_later_source_max
            && deadline.can_start_route();
        let run_concurrent_compact_split = compact_split_normal_seed.is_some()
            || compact_split_seeded_seed.is_some()
            || compact_split_deft4j_seed.is_some();
        let (concurrent_source_max, attempted_compact_split, concurrent_compact_split) =
            if run_concurrent_source_max || run_concurrent_compact_split {
                thread::scope(
                    |scope| -> Result<(Option<Candidate>, bool, Option<Candidate>)> {
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
                        let refinement = refine_bounded_deft4j_lineage(
                            source,
                            options,
                            decoded_limit,
                            identity,
                            default_floor,
                            &deadline,
                            bounded_floor_candidate.as_ref(),
                            narrow_candidate.as_ref(),
                            seed_weak_deft4j,
                            run_concurrent_compact_split,
                            &mut deft4j_candidate,
                        );
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
                                .any(|seed| seed.data == deft4j.data);
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
                        Ok((source_max, attempted_compact_split, compact_split))
                    },
                )?
            } else {
                refine_bounded_deft4j_lineage(
                    source,
                    options,
                    decoded_limit,
                    identity,
                    default_floor,
                    &deadline,
                    bounded_floor_candidate.as_ref(),
                    narrow_candidate.as_ref(),
                    seed_weak_deft4j,
                    false,
                    &mut deft4j_candidate,
                )?;
                (None, false, None)
            };
        compact_split_attempted = attempted_compact_split;
        compact_split_candidate = concurrent_compact_split;
        if let Some(source_max) = concurrent_source_max {
            source_max_candidate = Some(source_max);
            suppress_later_source_max = true;
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
    // This deterministic structural cleanup normally runs in the deft4j
    // lineage beside source max. If that concurrent phase could not run,
    // finish it serially so scheduling cannot discard a bounded saving.
    if run_compact_split_floor && !compact_split_attempted {
        compact_split_candidate = match deft4j_candidate.as_ref() {
            Some(deft4j) => {
                refine_with_compact_source_split_floor(deft4j, options, decoded_limit, identity)?
            }
            None => None,
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
    if let Some(complete_default) = complete_default_candidate {
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
        let mut floor_options = options.clone();
        floor_options.exhaustive = false;
        match default_floor {
            DefaultFloor::Complete => build_candidate(
                source,
                &floor_options,
                DEFAULT_RAW_REPLAY_LIMIT,
                &mut deadline.hard_stop(),
            )?,
            DefaultFloor::CompleteThenBounded
            | DefaultFloor::Shared
            | DefaultFloor::Established => {
                build_bounded_floor_candidate(source, options, &mut deadline.hard_stop())?
            }
        }
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
    if !options.exhaustive {
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

fn deft4j_source_route_eligible(blocks: &[ParsedBlock], allow_single_block: bool) -> bool {
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
        1 => allow_single_block && huffman == 1,
        2..=DEFT4J_SOURCE_LIST_MAX_BLOCKS => huffman >= DEFT4J_MULTI_BLOCK_MIN_HUFFMAN_BLOCKS,
        _ => false,
    }
}

fn narrow_source_route_eligible(blocks: &[ParsedBlock], compressed_len: usize) -> bool {
    compressed_len <= NARROW_SOURCE_MAX_COMPRESSED
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
            details.finalizing_after_soft_deadline(timeout_grace(deadline.duration));
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
    default_floor: DefaultFloor,
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
                default_floor.allows_single_block_deft4j(),
                &mut deadline.bounded_stop(stop_at_soft_deadline),
            )? {
                if deadline.can_start_route() {
                    let refined = refine_with_default_planner(
                        &seeded,
                        options,
                        decoded_limit,
                        identity,
                        &mut deadline.bounded_stop(stop_at_soft_deadline),
                    )?;
                    seeded.replace_if_smaller(refined);
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
        let refined = refine_with_default_planner(
            deft4j,
            options,
            decoded_limit,
            identity,
            &mut deadline.bounded_stop(stop_at_soft_deadline),
        )?;
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
            if let Some(split) = refine_with_compact_source_split_floor(
                deft4j,
                options,
                source.decoded_limit,
                source.identity,
            )? {
                deft4j.replace_if_smaller(split);
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
            ..BoundedPhaseCandidates::default()
        });
    }

    if !parallel_routes {
        return build_bounded_phase_candidates_sequential(
            source,
            options,
            run_seeded_max,
            run_deft4j,
            run_narrow,
            &route_window,
            completed_floor,
        );
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
                        build_narrow_source_candidate(source, options, &mut route_window.stop())
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
                build_narrow_source_candidate(source, options, &mut route_window.stop())
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
        build_narrow_source_candidate(source, options, &mut route_window.stop())?
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
/// This mirrors the useful no-split route family while sharing the caller's
/// parsed payloads and deadline. It is intentionally not replayed through
/// `plan_stream`; doing so would immediately repeat the grouping and split
/// work this sibling exists to bypass.
fn build_narrow_source_candidate(
    source: CandidateInput<'_>,
    options: &Options,
    expired: &mut SearchStop<'_>,
) -> Result<Option<Candidate>> {
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
fn build_deft4j_seed_candidate(
    candidate: &Candidate,
    options: &Options,
    decoded_limit: u64,
    identity: StreamIdentity,
    allow_single_block: bool,
    expired: &mut SearchStop<'_>,
) -> Result<Option<Candidate>> {
    let stream = parse_validated_rewrite(&candidate.data, decoded_limit, identity)?;
    if !deft4j_source_route_eligible(&stream.blocks, allow_single_block) {
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
    let mut candidate = None;
    let mut attempted = false;
    for seed in seeds.into_iter().flatten() {
        // Always finish one eligible structural parent. Later independent
        // parents start only inside the soft schedule; a larger max budget
        // naturally evaluates all of them.
        if attempted && !deadline.can_start_route() {
            break;
        }
        attempted = true;
        if let Some(contender) =
            build_prepared_compact_source_split_floor(seed, options, decoded_limit, identity)?
        {
            replace_optional_if_smaller(&mut candidate, contender);
        }
    }
    Ok(candidate)
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
    // Default's bounded feedback floors gain broadly from the cheap pair move.
    // In Max, matched streams already have several independent token and
    // boundary lineages; repeating this tree search there can delay larger
    // wins. Keep Max's pair move for the empty-distance case that motivated it.
    let pair = (!options.exhaustive
        || block
            .distance_frequencies
            .iter()
            .all(|&frequency| frequency == 0))
    .then(|| {
        plan_columbo_pair_lengthen_candidate(
            &block.tokens,
            &block.literal_frequencies,
            &block.distance_frequencies,
            seed,
            options.exhaustive,
        )
    })
    .flatten();
    let dynamic = [
        pair,
        plan_columbo_quad_lengthen_candidate(
            &block.tokens,
            &block.literal_frequencies,
            &block.distance_frequencies,
            seed,
            options.exhaustive,
        ),
    ]
    .into_iter()
    .flatten()
    .min_by_key(|candidate| candidate.bits);
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
    let floor_plan =
        improve_plan_with_integrated_proven_floor(block, 0, &route_options, true, floor_base);
    let floor_plan = improve_plan_with_short_family_floor(block, &route_options, floor_plan);
    // Stabilize the cheap floor before starting its heavier full-search
    // sibling. Besides securing an early complete candidate for shared
    // deadlines, this avoids making a later local search consume time needed
    // by the floor's distinct replay fixed point.
    let mut candidate =
        build_compact_proven_seed_candidate(source, floor_plan, &route_options, options, expired)?;
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
    .named("Columbo proven-feedback floor");
    let integrated = build_candidate_from_plans(
        source,
        vec![plan],
        route_options,
        DEFAULT_RAW_REPLAY_LIMIT,
        ReplayPlanner::IntegratedProven,
        expired,
    )?
    .named("Columbo proven-feedback floor");
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
    debug_assert_eq!(bits, plans.iter().map(|plan| plan.bits).sum::<u64>());
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

        // A zero-budget max run keeps a complete source fallback instead of
        // making the comparison floor an unbounded timeout exception.
        let zero_budget_max = Options {
            exhaustive: true,
            strict: false,
            timeout: Duration::ZERO,
            ..Options::default()
        };
        let stopped = optimize_raw(&input, &zero_budget_max).unwrap();
        assert!(stopped.timed_out);
        assert_eq!(stopped.info.deflate_bits, source.meaningful_bits);
        assert_eq!(stopped.data, input);

        // With sufficient time, max mode closes the same feedback graph in one
        // invocation and is itself at a fixed point.
        let max_options = Options {
            timeout: Duration::from_secs(1),
            ..zero_budget_max
        };
        let maximum = optimize_raw(&input, &max_options).unwrap();
        assert!(maximum.info.deflate_bits <= first.info.deflate_bits);
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
        assert!(!deft4j_source_route_eligible(&parsed.blocks, true));
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
        assert!(deft4j_gain_is_below(10_000, 9_801, 200));
        assert!(!deft4j_gain_is_below(10_000, 9_800, 200));
        assert!(!deft4j_gain_is_below(10_000, 9_000, 200));
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
