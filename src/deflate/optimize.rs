// SPDX-License-Identifier: MIT
// SPDX-FileCopyrightText: 2026 Andrew C.E. Dent <https://github.com/ace-dent>

//! Raw-stream optimization orchestration and complete-candidate selection.
//!
//! Validate the source before scheduling optional routes, retain the required
//! Default comparison floor, and compare emitted candidates by bytes, then
//! meaningful bits. Block-boundary and token-spelling algorithms live in
//! `stream` and `search`. Route labels (D1, M0, R1, …) refer to
//! `docs/routes-and-methods.md`.

use std::thread;
use std::time::Instant;

use crate::progress::{
    reports_enabled, BlockEncoding, BlockProgress, BlockReport, CandidateProgress, Progress,
    RouteProgress, MAX_REPORTED_BLOCKS,
};
use crate::sort::SortShared;
use crate::{Error, Options, Result};

use super::bitstream::BitWriter;
use super::block::{emit_block, plan_block, reusable_original_bits, stored_block_bits};
use super::header::{
    plan_bounded_depth_tree_candidate, plan_columbo_balanced_tree_candidate,
    plan_for_explicit_lengths, plan_literal_span, plan_payload_header_tradeoff,
    plan_rle_smoothed_tree_candidate,
};
use super::model::{
    same_tokens, DynamicPlan, ParsedBlock, ParsedStream, PlannedBlock, Representation,
    SourceBlockType, Token,
};
use super::parse::{parse_stream, parsed_model_bytes};
use super::restore::{
    plan_original_match_restoration, MAX_RESTORATION_BLOCKS, MAX_RESTORATION_BYTES,
};
use super::search::{
    compact_proven_submatch_route_eligible, improve_plan_with_header_aware_proven_composition,
    improve_plan_with_integrated_proven_floor, improve_plan_with_short_family_floor,
    plan_block_with_integrated_proven_search, plan_block_with_seeded_narrow_search,
    plan_length_family_chain, plan_respell_from_literals, plan_terminal_token_replay,
    rewrite_258_symbols, same_distance_opportunities, PROVEN_SUBMATCH_FULL_MATCH_LIMIT,
};
use super::source_recode::plan_source_blocks;
use super::stop::{timeout_grace, Deadline, RouteWindow, SearchStop};
use super::stream::{
    plan_columbo_floor_seeded_bounded_grouping, plan_compact_source_split_floor,
    plan_compact_source_split_floor_until, plan_fragmented_replay,
    plan_integrated_proven_source_route, plan_proven_submatch_route,
    plan_source_individual_no_split_route, plan_source_no_split_route, plan_stream,
    plan_stream_from_established_floor, plan_stream_with_progress, plan_terminal_merge_route,
};

mod schedule;

pub(crate) use schedule::optimize_raw_prefix_with_floor_and_grace;

/// One round groups adjacent source blocks; two more settle the new layout's
/// boundaries and tables.
const DEFAULT_RAW_REPLAY_LIMIT: usize = 3;
/// Extra R13–R15 passes after a winning pass when no later R12 sweep can
/// revisit them.
const TREE_FINISH_REPEATS: usize = 1;
/// Default's size limit for R1b–R5, also used by Max's mandatory Default floor.
const TERMINAL_HEADER_MAX_BYTES: usize = 128 * 1024;
/// Max's size limit for the terminal header methods; also bounds
/// [`TerminalShare::Search`].
const MAX_TERMINAL_HEADER_MAX_BYTES: usize = 1024 * 1024;
const TERMINAL_HEADER_MAX_BLOCKS: usize = 128;
/// Full-header prices per R1b, R2 or R3 invocation, shared across blocks.
const TERMINAL_HEADER_MAX_PRICES: usize = 1024;
/// Sentinel: replay until a round fails to improve, bounded by
/// [`resolved_replay_limit`].
const MAX_RAW_REPLAY_LIMIT: usize = usize::MAX;
const NARROW_SOURCE_LIST_MAX_BLOCKS: usize = 128;
/// From this many nonempty source blocks, the no-split route prunes
/// cumulatively first, so early blocks cannot use up the route window.
const CUMULATIVE_NO_SPLIT_MIN_SOURCE_BLOCKS: usize = 4;
/// A multi-block direct deft4j gain below this (2%) counts as weak.
const WEAK_DEFT4J_GAIN_BASIS_POINTS: u64 = 200;
/// With [`NARROW_SOURCE_LIST_MAX_BLOCKS`], keeps the no-split route's
/// candidates well inside [`PARALLEL_ROUTE_MAX_MODEL`].
const NARROW_SOURCE_MAX_COMPRESSED: usize = 1_024 * 1_024;
/// Smallest decoded stream for Default's source-topology route (D4). Below
/// it, the build costs more than its rare gains, as on APNG frames.
const DEFAULT_SOURCE_TOPOLOGY_MIN_DECODED: u64 = 128 * 1024;
/// Limits for the compact split floor (M10c).
const COMPACT_SPLIT_FLOOR_MAX_COMPRESSED: usize = 16 * 1024;
const COMPACT_SPLIT_FLOOR_MAX_DECODED: u64 = 128 * 1024;
const TERMINAL_SOURCE_SPLIT_MAX_DECODED: u64 = 256 * 1024;
const COMPACT_SPLIT_FLOOR_MAX_BLOCKS: usize = 4;
const COMPACT_SPLIT_FLOOR_MAX_TOKENS: usize = 16 * 1024;
/// A large one-block source max overlaps the completed PNG floor only with at
/// least one repartition run per this many source tokens, since both compete
/// for cache and memory bandwidth.
const DENSE_REPARTITION_TOKENS_PER_RUN: usize = 16;
/// Limits for the one-block compact balanced-tree floor (M4).
const COMPACT_TREE_MAX_COMPRESSED: usize = 8 * 1_024;
const COMPACT_TREE_MAX_DECODED: u64 = 128 * 1_024;
const COMPACT_TREE_MAX_TOKENS: usize = 4_096;
const RLE_SMOOTHED_TREE_FLOOR_MAX_BLOCKS: usize = 8;
const DEFAULT_STRICT_TREE_ROUNDS: usize = 4;
/// M9: up to this many tokens, source max also runs beside M3.
const COMPACT_COMPLEMENTARY_SOURCE_MAX_TOKENS: usize = 2_048;
/// M9: token band in which one source-max worker may replace two long workers
/// on the same graph.
const COMPACT_SINGLE_SOURCE_ROUTE_MAX_TOKENS: usize = 4_000;
const COMPACT_SINGLE_SOURCE_ROUTE_MIN_TOKENS: usize =
    COMPACT_SINGLE_SOURCE_ROUTE_MAX_TOKENS * 5 / 8;
/// Mean decoded bytes per token (7/8 of 256) at or above which a graph of at
/// most 4,000 tokens consists almost entirely of long matches.
const NEAR_MAX_MATCH_MEAN_DECODED_PER_TOKEN: u64 = 7 * 32;
const DEFLATE_MAX_STORED_BLOCK_PLAIN: u64 = 65_535;
/// Limits for running routes in parallel (M5), so peak memory does not grow
/// with every route's own budget.
const PARALLEL_ROUTE_MAX_COMPRESSED: usize = 8 * 1_024 * 1_024;
const PARALLEL_ROUTE_MAX_DECODED: u64 = 64 * 1_024 * 1_024;
const PARALLEL_ROUTE_MAX_MODEL: usize = 64 * 1_024 * 1_024;
/// Limits for the bounded-depth tree rescue after the hard stop; its block
/// limit is [`NARROW_SOURCE_LIST_MAX_BLOCKS`].
const BOUNDED_DEPTH_RESCUE_MAX_COMPRESSED: usize = 1_024 * 1_024;
const BOUNDED_DEPTH_RESCUE_MAX_DECODED: u64 = 1_024 * 1_024;
/// M0: `CompleteThenBounded` floors between these decoded sizes are built
/// beside the source routes, not first, unless the source has one nonempty
/// block or a dense match graph. Smaller floors are cheap, and larger ones
/// would contend with concurrent routes.
const PREBUILD_BOUNDED_FLOOR_MAX_DECODED: u64 = 768 * 1_024;
const CONCURRENT_BOUNDED_FLOOR_MAX_DECODED: u64 = 2 * 1_024 * 1_024;

/// Facts from decoding the source stream, which containers use to check their
/// checksums without inflating again.
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
    pub(crate) output_max_distance: u16,
    pub(crate) timed_out: bool,
}

/// Where Default runs R2–R5. R2–R4 fit code lengths and headers to the codes a
/// tree family has chosen, and R5 keeps a token rewrite only if it beats that
/// fitted parent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum HeaderFit {
    /// In the sweep, after R1b: the floor phase has already run the terminal
    /// tree floors.
    InSweep,
    /// After R18: `ApngDefault` skips the first tree floors, so the finishing
    /// steps (R1c, R13–R17) and R18 still replace its codes, and a fit made
    /// before them is replaced too. R5 moves with the fit, since against an
    /// unfitted parent it keeps rewrites that block the fit.
    AfterTreeFloors,
}

/// How a caller establishes the Default comparison floor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DefaultFloor {
    /// A standalone stream runs the Default route before Max-only work. The
    /// floor finishes even after Max's optional-search deadline, so a timeout
    /// never makes Max worse than Default.
    Complete,
    /// `Complete` for a single scheduled PNG image, before PNG's bounded Max
    /// routes.
    CompleteThenBounded,
    /// A multi-stream container member, which must not consume time needed by
    /// later members.
    Shared,
    /// `Shared`, but keeps the complete Default endpoint before Max-only work.
    SharedExact,
    /// Multi-image APNG Default; leaves replay and feedback lineages to Max.
    ApngDefault,
    /// Multi-image APNG Max; shared scheduling that keeps the APNG Default
    /// endpoint, without the single-image PNG route families.
    ApngMax,
    /// The caller already retains the complete input as its floor.
    Established,
    /// A container-level Default branch built for Max, which Max's clock
    /// cannot interrupt, except that D4 and R18 stay timed.
    MandatoryComplete,
}

impl DefaultFloor {
    fn is_bounded(self) -> bool {
        !matches!(self, Self::Complete | Self::MandatoryComplete)
    }

    fn uses_bounded_png_routes(self) -> bool {
        matches!(self, Self::CompleteThenBounded)
    }

    /// Whether original-source Max may run beside the deft4j refinement (M10b),
    /// even when the direct deft4j parent is smaller: parent order does not
    /// predict endpoint order.
    fn allows_parallel_source_follow_up(self) -> bool {
        matches!(self, Self::CompleteThenBounded | Self::ApngMax)
    }

    fn owns_terminal_stream_time(self) -> bool {
        matches!(self, Self::Complete | Self::CompleteThenBounded)
    }

    /// How much of a Max stream's allowance to reserve for terminal methods
    /// (G0t), so an unfinished primary search cannot starve them.
    fn terminal_share(
        self,
        options: &Options,
        compressed_bytes: usize,
        decoded_bytes: u64,
        source_blocks: usize,
    ) -> TerminalShare {
        if !options.exhaustive || options.timeout.is_zero() {
            return TerminalShare::None;
        }
        let header_class = compressed_bytes <= MAX_TERMINAL_HEADER_MAX_BYTES
            && decoded_bytes <= MAX_TERMINAL_HEADER_MAX_BYTES as u64
            && source_blocks <= TERMINAL_HEADER_MAX_BLOCKS;
        match self {
            _ if !(self.owns_terminal_stream_time() || self == Self::ApngMax) => {
                if self == Self::Established && !header_class {
                    TerminalShare::Finalization
                } else {
                    TerminalShare::None
                }
            }
            _ if header_class => TerminalShare::Search,
            _ => TerminalShare::Finalization,
        }
    }
}

/// How a Max stream divides its allowance between primary and terminal work.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TerminalShare {
    /// Primary routes use the whole allowance.
    None,
    /// Within the terminal header size limit: primary work keeps four fifths
    /// (an APNG child nineteen twentieths) of its slice.
    Search,
    /// Outside that limit only linear finalization (the bounded-depth tree
    /// floor, R1c and R13–R17) uses terminal time, so primary work keeps
    /// nineteen twentieths.
    Finalization,
}

pub(crate) fn optimize_raw(input: &[u8], options: &Options) -> Result<RawOptimization> {
    let mut optimized = optimize_raw_prefix(input, options, options.max_decoded_bytes)?;
    if !options.strip_metadata {
        optimized
            .data
            .try_reserve(input.len() - optimized.consumed)
            .map_err(|_| Error::allocation("Deflate suffix"))?;
        optimized
            .data
            .extend_from_slice(&input[optimized.consumed..]);
    }
    Ok(optimized)
}

/// Parse one complete raw stream without optimizing it, so container inspection
/// can find concatenated GZIP member trailers.
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

/// Parse the first raw stream in `input`, returning its consumed length and
/// whether PNG single-image Max should race an early transformed lineage beside
/// its exact Default lineage. A dense same-distance graph needs the reduced
/// parent, and a small multi-block stream would otherwise build exact Default
/// serially.
pub(crate) fn inspect_early_max_lineage(input: &[u8], decoded_limit: u64) -> Result<(usize, bool)> {
    let parsed = parse_stream(input, decoded_limit)?;
    let dense_match_graph =
        source_run_match_count_exceeds(&parsed.blocks, PROVEN_SUBMATCH_FULL_MATCH_LIMIT);
    let nonempty_blocks = parsed
        .blocks
        .iter()
        .filter(|block| !block.plain.is_empty())
        .count();
    Ok((
        parsed.consumed,
        early_transformed_lineage_is_useful(
            nonempty_blocks,
            parsed.decoded_size,
            dense_match_graph,
        ),
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

/// Rewrite Defluff's length-258 alias (symbol 284) to symbol 285 before
/// strict planning. The source representations are cleared too, so no alias
/// survives through exact source reuse or a header rewrite.
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
            .ok_or_else(|| Error::allocation("strict Deflate token normalization"))?;
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
pub(crate) fn optimize_raw_prefix(
    input: &[u8],
    options: &Options,
    decoded_limit: u64,
) -> Result<RawOptimization> {
    optimize_raw_prefix_with_floor(input, options, decoded_limit, DefaultFloor::Complete)
}

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
    // A lone stored block gives the deft4j graph nothing to improve.
    nonempty >= 2 || (nonempty == 1 && huffman == 1)
}

/// D4: Default prices the source's own block layout, each block searched from
/// the smallest of its ordinary plan, length-family chain and re-spell from
/// literals, for multi-block sources in the no-split route's class (M2) that
/// decode to at least [`DEFAULT_SOURCE_TOPOLOGY_MIN_DECODED`] bytes.
fn default_source_topology_eligible(
    blocks: &[ParsedBlock],
    compressed_len: usize,
    decoded_size: u64,
) -> bool {
    decoded_size >= DEFAULT_SOURCE_TOPOLOGY_MIN_DECODED
        && narrow_source_route_eligible(blocks, compressed_len)
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

/// Choose the bounded PNG route family (M6).
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

/// Whether the completed floor changed a nonempty block boundary or token
/// spelling, giving Max a seed that source-order search may not reach before
/// its deadline.
fn floor_exposes_new_search_states(source: &[ParsedBlock], floor: &[PlannedBlock]) -> bool {
    let mut source = source.iter().filter(|block| !block.plain.is_empty());
    let mut floor = floor.iter().filter(|block| !block.plain.is_empty());
    loop {
        match (source.next(), floor.next()) {
            (Some(source_block), Some(floor_block)) => {
                if source_block.plain.len() != floor_block.plain.len()
                    || !same_tokens(&source_block.tokens, &floor_block.tokens)
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

/// Whether the floor-seeded continuation may run before a pending compact
/// split. If they cannot overlap, it goes first only when it saves at least 2%
/// over the floor, so it cannot starve the split.
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

/// Whether the deft4j refinement runs beside the floor-seeded continuation.
/// Only a stream with the file-level route window and bounded parallel work
/// can hold both expanded models.
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
    /// Exact maximum distance from validation of these emitted bytes.
    output_max_distance: Option<u16>,
    plans: Vec<PlannedBlock>,
    block_report: Option<BlockReport>,
    route: &'static str,
    /// The Max planner reparsed these exact bytes and reached a strict fixed
    /// point before its stop. This describes the bytes, not the route that
    /// emitted them.
    max_planner_is_stable: bool,
    /// Complete candidates this one replaced, by its own replays or as a
    /// lineage step or later route, oldest first. Only Max keeps them: it
    /// closes original-source Max's replay parents after that route, and the
    /// rest before its terminal searches.
    replaced_parents: Vec<Candidate>,
}

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

    /// Compare by bytes, then meaningful bits. A tie keeps the earlier
    /// incumbent, so routing and output stay deterministic.
    fn is_strictly_smaller_than(&self, incumbent: &Self) -> bool {
        is_strictly_better(
            self.data.len(),
            self.bits,
            incumbent.data.len(),
            incumbent.bits,
        )
    }

    fn is_strictly_smaller_than_source(&self, source: CandidateInput<'_>) -> bool {
        is_strictly_better(
            self.data.len(),
            self.bits,
            source.compressed.len(),
            source.meaningful_bits,
        )
    }

    fn replace_if_smaller(&mut self, contender: Self) -> bool {
        if contender.is_strictly_smaller_than(self) {
            *self = contender;
            true
        } else {
            false
        }
    }

    /// Like [`Self::replace_if_smaller`], but with `keep` (Max) the replaced
    /// candidate, bytes only, and the parents of both stay in
    /// `replaced_parents`: a deadline before the replacement would have
    /// handed that candidate over, and a smaller one can close larger.
    fn replace_keeping_parent(&mut self, mut contender: Self, keep: bool) -> bool {
        if !keep {
            return self.replace_if_smaller(contender);
        }
        if !contender.is_strictly_smaller_than(self) {
            self.replaced_parents
                .append(&mut contender.replaced_parents);
            return false;
        }
        let mut parent = std::mem::replace(self, contender);
        self.replaced_parents.append(&mut parent.replaced_parents);
        parent.plans = Vec::new();
        parent.block_report = None;
        self.replaced_parents.push(parent);
        true
    }

    /// Whether another candidate proved this exact encoding to be a Max
    /// planner fixed point.
    fn is_encoding_stabilized_by(&self, contender: &Self) -> bool {
        contender.max_planner_is_stable
            && contender.bits == self.bits
            && contender.data == self.data
    }
}

#[inline(never)]
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

/// Run one original-source Max search, with progress heartbeats only when
/// reports are enabled.
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

/// Whether original-source Max and floor-seeded Max can share the initial
/// phase. Within the compact split limits the overlap's memory and cache cost
/// is small; larger streams run serially so two beams pricing many payload
/// ranges do not starve the floor.
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

/// Whether source max is cheap enough to overlap the bounded floor routes.
/// Only source max combines the choices of a dense repartition graph, and on
/// tiny graphs it cheaply tests original block merges that a rewritten floor
/// cannot reconstruct.
fn bounded_parallel_source_max_work_class(source: CandidateInput<'_>) -> bool {
    compact_complementary_source_max_is_cheap(source)
        || compact_parallel_source_max_work_class(source)
}

/// Whether a source-max beam should overlap the completed single-image PNG
/// floor (M8). A large one-block source overlaps only when its graph is cheap,
/// since otherwise both beams price many of the same payload ranges.
fn complete_png_parallel_source_max_work_class(source: CandidateInput<'_>) -> bool {
    let nonempty_blocks = source
        .blocks
        .iter()
        .filter(|block| !block.plain.is_empty())
        .count();
    if nonempty_blocks != 1 || source.identity.decoded_size <= COMPACT_SPLIT_FLOOR_MAX_DECODED {
        return bounded_parallel_source_max_work_class(source);
    }

    let Some(token_count) = source_token_count(source) else {
        return false;
    };
    if token_count <= COMPACT_COMPLEMENTARY_SOURCE_MAX_TOKENS {
        return true;
    }

    source.compressed.len() <= COMPACT_SPLIT_FLOOR_MAX_COMPRESSED
        && token_count <= COMPACT_SPLIT_FLOOR_MAX_TOKENS
        && (dense_repartition_graph_justifies_parallel_source_max(
            token_count,
            same_distance_opportunities(source.blocks).repartition_runs,
        ) || near_max_match_source_graph_is_cheap(source.identity.decoded_size, token_count))
}

fn dense_repartition_graph_justifies_parallel_source_max(
    token_count: usize,
    repartition_runs: usize,
) -> bool {
    repartition_runs.saturating_mul(DENSE_REPARTITION_TOKENS_PER_RUN) >= token_count
}

fn near_max_match_source_graph_is_cheap(decoded_size: u64, token_count: usize) -> bool {
    token_count <= COMPACT_SINGLE_SOURCE_ROUTE_MAX_TOKENS
        && u64::try_from(token_count)
            .ok()
            .and_then(|tokens| tokens.checked_mul(NEAR_MAX_MATCH_MEAN_DECODED_PER_TOKEN))
            .is_some_and(|minimum_decoded| decoded_size >= minimum_decoded)
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
        // Source max combines choices across blocks only after finishing each
        // block, and a block above 4,000 tokens can use the whole phase.
        if block.tokens.len() > COMPACT_SINGLE_SOURCE_ROUTE_MAX_TOKENS {
            return false;
        }
    }
    nonempty_blocks != 0 && repartition_runs >= nonempty_blocks
}

/// Whether the direct deft4j parent can cheaply finish its dependent split.
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

/// Refine the bounded phase's best deft4j lineage in place (M10b).
///
/// A concurrent original-source Max shares only the deadline. Set
/// `stop_at_soft_deadline` when compact split siblings already cover this
/// lineage's parents, so it does not use the grace.
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
struct BoundedPhaseCandidates {
    floor: Option<Candidate>,
    /// Compared only after the terminal searches.
    slid_default: Option<Candidate>,
    floor_seeded: Option<Candidate>,
    deft4j: Option<Candidate>,
    narrow: Option<Candidate>,
    source_max: Option<Candidate>,
    proven_feedback: Option<Candidate>,
    suppress_later_source_max: bool,
    completed_compact_split_parent: Option<Vec<u8>>,
}

/// A complete compressed stream with the facts needed to verify each
/// reparse/replan round.
#[derive(Clone, Copy)]
struct CandidateInput<'a> {
    compressed: &'a [u8],
    blocks: &'a [ParsedBlock],
    meaningful_bits: u64,
    decoded_limit: u64,
    identity: StreamIdentity,
}

/// Borrow a validated rewrite in the same shape as the original source.
/// Comparisons use `candidate.bits`, the emitter's exact meaningful-bit count;
/// reparsing only proves stream identity.
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

/// Route families requested for the bounded comparison phase.
#[derive(Clone, Copy, Default)]
struct BoundedRoutes {
    /// Finish the exact Default endpoint before optional routes can use time.
    preserve_complete_default: bool,
    /// Continue the ordinary floor with the Max planner.
    seeded_max: bool,
    deft4j: bool,
    narrow: bool,
    source_max: bool,
    proven_feedback: bool,
    parallel: bool,
}

/// Run the independent bounded comparison routes under one deadline, on
/// scoped workers within the parallel limits (M5) and serially otherwise.
fn build_bounded_phase_candidates(
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
            ..BoundedPhaseCandidates::default()
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

/// Start one route on a named scoped worker that cancels siblings on failure.
/// `None` means the worker could not be spawned; callers then run serially.
fn spawn_route<'scope, T: Send + 'scope>(
    scope: &'scope thread::Scope<'scope, '_>,
    name: &str,
    deadline: &'scope Deadline,
    route: impl FnOnce() -> Result<T> + Send + 'scope,
) -> Option<thread::ScopedJoinHandle<'scope, Result<T>>> {
    spawn_boxed_route(scope, name, deadline, Box::new(route))
}

/// The body of [`spawn_route`]. Boxing the route compiles the thread machinery
/// and the cancellation guard once per result type rather than once per route.
fn spawn_boxed_route<'scope, T: Send + 'scope>(
    scope: &'scope thread::Scope<'scope, '_>,
    name: &str,
    deadline: &'scope Deadline,
    route: Box<dyn FnOnce() -> Result<T> + Send + 'scope>,
) -> Option<thread::ScopedJoinHandle<'scope, Result<T>>> {
    thread::Builder::new()
        .name(name.into())
        .spawn_scoped(scope, move || run_route_with_cancellation(deadline, route))
        .ok()
}

/// Join a route worker, resuming its panic on this thread.
fn join_route<T>(worker: thread::ScopedJoinHandle<'_, T>) -> T {
    match worker.join() {
        Ok(result) => result,
        Err(payload) => std::panic::resume_unwind(payload),
    }
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

/// Copy a caller-retained `Established` stream into the local candidate set.
/// Descendants reparse it, so the normal identity checks still apply. Strict
/// mode returns `None` unless the copy is strictly compatible, so the routes
/// build a bounded floor instead.
fn established_floor_candidate(
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

/// Whether Default comparison work may be interrupted. Max uses `Mandatory`
/// only for the Default result it must never lose.
#[derive(Clone, Copy)]
enum DefaultFloorWork<'a> {
    Timed(&'a Deadline),
    Window(&'a RouteWindow<'a>),
    Mandatory,
}

impl<'a> DefaultFloorWork<'a> {
    fn can_start_route(self) -> bool {
        match self {
            Self::Timed(deadline) => deadline.can_start_route(),
            Self::Window(window) => window.can_start_route(),
            Self::Mandatory => true,
        }
    }

    fn stop(self) -> SearchStop<'a> {
        match self {
            Self::Timed(deadline) => deadline.hard_stop(),
            Self::Window(window) => window.stop(),
            Self::Mandatory => SearchStop::never(),
        }
    }

    fn is_mandatory(self) -> bool {
        matches!(self, Self::Mandatory)
    }

    /// Whether linear finalization may still start: the hard stop it polls
    /// has not been reached, even if no new search route may start.
    fn can_finalize(self) -> bool {
        !self.stop().reached()
    }
}

/// Complete the Default floor with D1, D2, D3 and the terminal tree floors.
/// Max's retained Default floor shares this sequence.
fn improve_default_floor_with_feedback(
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

/// Apply the terminal tree floors to one complete candidate, keeping the
/// candidates it replaced for Max's final close.
fn improve_with_terminal_tree_floors(
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

/// R1: restore source-certified matches in a completed candidate. It stays out
/// of the tree floors because its terminal spelling must not redirect the
/// descendants of losing topologies.
fn improve_with_original_match_restoration(
    source: CandidateInput<'_>,
    options: &Options,
    floor_work: DefaultFloorWork<'_>,
    progress: Progress,
    candidate: Candidate,
) -> Result<Candidate> {
    improve_with_original_match_restoration_keeping(
        source, options, floor_work, progress, false, candidate,
    )
}

/// [`improve_with_original_match_restoration`], keeping the candidate a win
/// replaces among its `replaced_parents` when `keep_replaced`.
fn improve_with_original_match_restoration_keeping(
    source: CandidateInput<'_>,
    options: &Options,
    floor_work: DefaultFloorWork<'_>,
    progress: Progress,
    keep_replaced: bool,
    mut candidate: Candidate,
) -> Result<Candidate> {
    if !floor_work.can_start_route()
        || source.identity.decoded_size > MAX_RESTORATION_BYTES as u64
        || source.compressed.len() > MAX_RESTORATION_BYTES
        || candidate.data.len() > MAX_RESTORATION_BYTES
        || source.blocks.len() > MAX_RESTORATION_BLOCKS
        || candidate.data == source.compressed
    {
        return Ok(candidate);
    }
    let step = progress.start("Original-match restoration");
    let restored = refine_with_original_match_restoration(
        source,
        &candidate,
        options,
        &mut floor_work.stop(),
    )?;
    step.finish(restored.as_ref().map(|restored| {
        candidate_progress(
            restored,
            source.meaningful_bits,
            restored.is_strictly_smaller_than_source(source),
        )
    }));
    if let Some(restored) = restored {
        candidate.replace_keeping_parent(restored, keep_replaced);
    }
    Ok(candidate)
}

fn refine_with_original_match_restoration(
    original: CandidateInput<'_>,
    candidate: &Candidate,
    options: &Options,
    stop: &mut SearchStop<'_>,
) -> Result<Option<Candidate>> {
    if stop.reached() {
        return Ok(None);
    }
    let selected =
        parse_validated_rewrite(&candidate.data, original.decoded_limit, original.identity)?;
    // Parsing drops redundant empty blocks; this pass needs the parent's exact
    // block layout.
    if selected.source_block_count != selected.blocks.len() {
        return Ok(None);
    }
    let Some(plans) =
        plan_original_match_restoration(original.blocks, &selected.blocks, options.strict, stop)
    else {
        return Ok(None);
    };
    let source = rewritten_input(
        candidate,
        &selected,
        original.decoded_limit,
        original.identity,
    );
    // Zero replays keeps the exact payload trees and header. The builder
    // records the emitted maximum distance, which a restored match can raise.
    let restored =
        build_candidate_from_plans(source, plans, options, 0, ReplayPlanner::Full, stop)?;
    Ok(restored
        .is_strictly_smaller_than(candidate)
        .then(|| restored.named("Original-match restoration")))
}

/// Terminal methods R1b–R11, R1c and R13–R15, run in that order after R1 by
/// [`improve_with_terminal_searches`], and R16 and R17 once the sweep settles.
#[derive(Clone, Copy, PartialEq, Eq)]
enum TerminalHeaderSearch {
    StrictDistanceCompletion,
    PayloadTradeoff,
    LiteralSpan,
    JointTreeRle,
    SymbolSets,
    DistanceBands,
    SmoothedTrees,
    EntropyCuts,
    Merges,
    TokenReplay,
    AlphabetBoundaries,
    HeaderTree,
    CodeLengthRotations,
    CoupledLengthSwaps,
    HeaderResponse,
    LengthExchange,
    BoundarySlide,
}

struct TerminalSearchBudget {
    header_prices: usize,
    joint: super::joint::JointBudget,
    symbols: super::symbol_set::SymbolSetBudget,
    alphabet: super::stream::AlphabetBudget,
    entropy_cuts: super::stream::EntropyCutBudget,
    header_tree: super::header::HeaderTreeBudget,
    rotations: super::header::RotationBudget,
    coupled_swaps: super::header::CoupledSwapBudget,
    response: super::header::ResponseBudget,
}

impl TerminalHeaderSearch {
    fn name(self) -> &'static str {
        match self {
            Self::StrictDistanceCompletion => "Strict distance completion",
            Self::PayloadTradeoff => "Payload/header tradeoff",
            Self::LiteralSpan => "Literal/length span",
            Self::JointTreeRle => "Joint tree/RLE",
            Self::SymbolSets => "Symbol set removal",
            Self::DistanceBands => "Distance-alphabet ladder",
            Self::SmoothedTrees => "Smoothed-tree finish",
            Self::EntropyCuts => "Entropy cuts",
            Self::Merges => "Terminal merges",
            Self::TokenReplay => "Terminal token replay",
            Self::AlphabetBoundaries => "Alphabet boundary search",
            Self::HeaderTree => "Code-length tree search",
            Self::CodeLengthRotations => "Code-length rotations",
            Self::CoupledLengthSwaps => "Coupled code-length swaps",
            Self::HeaderResponse => "Header-directed match response",
            Self::LengthExchange => "Length-symbol exchange",
            Self::BoundarySlide => "Fixed-tree boundary slide",
        }
    }

    fn max_bytes(self, exhaustive: bool) -> usize {
        // R1c and R13–R17 are linear and parse one candidate, costing no more
        // than a replay round, so they have no size limit.
        if self == Self::BoundarySlide || self.is_tree_finish() {
            return usize::MAX;
        }
        if exhaustive {
            return MAX_TERMINAL_HEADER_MAX_BYTES;
        }
        match self {
            Self::AlphabetBoundaries
            | Self::HeaderTree
            | Self::CodeLengthRotations
            | Self::CoupledLengthSwaps
            | Self::HeaderResponse
            | Self::LengthExchange => MAX_TERMINAL_HEADER_MAX_BYTES,
            _ => TERMINAL_HEADER_MAX_BYTES,
        }
    }

    fn max_blocks(self) -> usize {
        match self {
            _ if self == Self::BoundarySlide || self.is_tree_finish() => usize::MAX,
            _ => TERMINAL_HEADER_MAX_BLOCKS,
        }
    }

    /// Max methods that run only when no earlier method in the sweep changed
    /// the candidate.
    fn waits_for_settled_sweep(self) -> bool {
        matches!(
            self,
            Self::DistanceBands
                | Self::SmoothedTrees
                | Self::EntropyCuts
                | Self::HeaderResponse
                | Self::LengthExchange
                | Self::BoundarySlide
        )
    }

    /// R13–R17: linear finishing steps, each followed by R1c after a win.
    fn is_tree_finish(self) -> bool {
        matches!(
            self,
            Self::DistanceBands
                | Self::SmoothedTrees
                | Self::EntropyCuts
                | Self::Merges
                | Self::TokenReplay
        )
    }

    /// R2–R5: fits to the codes as they stand, or compares against that fit
    /// ([`HeaderFit`]).
    fn fits_codes(self) -> bool {
        matches!(
            self,
            Self::PayloadTradeoff | Self::LiteralSpan | Self::JointTreeRle | Self::SymbolSets
        )
    }

    fn max_only(self) -> bool {
        matches!(
            self,
            Self::AlphabetBoundaries
                | Self::HeaderTree
                | Self::CodeLengthRotations
                | Self::CoupledLengthSwaps
                | Self::HeaderResponse
                | Self::LengthExchange
        )
    }

    fn plan(
        self,
        block: &ParsedBlock,
        alignment: u8,
        options: &Options,
        budget: &mut TerminalSearchBudget,
        stop: &mut SearchStop<'_>,
    ) -> Option<Vec<PlannedBlock>> {
        let plan = match self {
            Self::AlphabetBoundaries => {
                return super::stream::plan_alphabet_boundaries(
                    block,
                    alignment,
                    options,
                    &mut budget.alphabet,
                    stop,
                )
            }
            Self::HeaderResponse => super::header::plan_header_response(
                block,
                options.strict,
                &mut budget.response,
                stop,
            )?,
            Self::LengthExchange => super::header::plan_length_exchange(
                block,
                options.strict,
                &mut budget.response,
                stop,
            )?,
            Self::SymbolSets => super::symbol_set::plan_symbol_sets(
                block,
                alignment,
                options,
                &mut budget.symbols,
                stop,
            )?,
            Self::DistanceBands => {
                super::distance_band::plan_distance_bands(block, alignment, options, stop)?
            }
            Self::TokenReplay => plan_terminal_token_replay(block, alignment, options, stop)?,
            Self::EntropyCuts => {
                return super::stream::plan_entropy_cuts(
                    block,
                    alignment,
                    options,
                    &mut budget.entropy_cuts,
                    stop,
                )
            }
            _ => {
                let dynamic = match self {
                    Self::StrictDistanceCompletion => {
                        super::header::plan_strict_distance_completion(
                            block,
                            options.strict,
                            &mut budget.header_prices,
                            stop,
                        )
                    }
                    Self::PayloadTradeoff => plan_payload_header_tradeoff(
                        block,
                        options.strict,
                        &mut budget.header_prices,
                        stop,
                    ),
                    Self::LiteralSpan => {
                        plan_literal_span(block, options.strict, &mut budget.header_prices, stop)
                    }
                    Self::JointTreeRle => {
                        super::joint::plan_joint_tree_rle(block, &mut budget.joint, stop)
                    }
                    Self::HeaderTree => super::header::plan_header_tree(
                        block,
                        options.strict,
                        &mut budget.header_tree,
                        stop,
                    ),
                    Self::CodeLengthRotations => super::header::plan_code_length_rotations(
                        block,
                        options.strict,
                        &mut budget.rotations,
                        stop,
                    ),
                    Self::CoupledLengthSwaps => super::header::plan_coupled_length_swaps(
                        block,
                        options.strict,
                        &mut budget.coupled_swaps,
                        stop,
                    ),
                    Self::SmoothedTrees => plan_smoothed_tree_finish(block, options.strict, stop),
                    Self::BoundarySlide
                    | Self::SymbolSets
                    | Self::DistanceBands
                    | Self::EntropyCuts
                    | Self::Merges
                    | Self::TokenReplay
                    | Self::AlphabetBoundaries
                    | Self::HeaderResponse
                    | Self::LengthExchange => {
                        unreachable!()
                    }
                }?;
                PlannedBlock {
                    tokens: block.tokens.clone(),
                    plain: block.plain.clone(),
                    bits: dynamic.bits,
                    representation: Representation::Dynamic(dynamic),
                    source_type: block.source_type,
                }
            }
        };
        let mut plans = Vec::new();
        plans.try_reserve_exact(1).ok()?;
        plans.push(plan);
        Some(plans)
    }
}

/// Run the terminal methods (R1, R1b–R11, R1c, R13–R17) on the final
/// candidate. Max repeats the sweep while it improves and time remains (R12);
/// Default runs one sweep. Every accepted candidate strictly decreases (bytes,
/// meaningful bits), so a method's input score identifies an unchanged
/// candidate, and the sweep skips methods already tried on it.
///
/// With `keep_replaced`, each candidate a method replaces that may start only
/// while routes may (R1, R1b–R11) is kept among the result's
/// `replaced_parents`: a deadline before that method would have handed it to
/// the finishing steps.
#[allow(clippy::too_many_arguments)]
fn improve_with_terminal_searches(
    source: CandidateInput<'_>,
    options: &Options,
    default_work: DefaultFloorWork<'_>,
    max_work: DefaultFloorWork<'_>,
    progress: Progress,
    header_fit: HeaderFit,
    keep_replaced: bool,
    mut candidate: Candidate,
) -> Result<Candidate> {
    // Moved here after each method, so a later replacement that keeps
    // nothing cannot drop them.
    let mut kept = Vec::new();
    if keep_replaced {
        kept.append(&mut candidate.replaced_parents);
    }
    let mut visited = [None; 16];
    let mut merged = None;
    let mut first_sweep = true;
    let mut parse_cache = TerminalParseCache::default();
    loop {
        let ordinary_work = if first_sweep { default_work } else { max_work };
        // The score before the first R13–R15 step this sweep runs.
        let mut finish_start = None;
        let before = (candidate.data.len(), candidate.bits);
        if visited[0] != Some(before) {
            visited[0] = Some(before);
            // Restoration parses its own model; do not hold a second one.
            parse_cache.clear();
            candidate = improve_with_original_match_restoration_keeping(
                source,
                options,
                ordinary_work,
                progress,
                keep_replaced,
                candidate,
            )?;
            if keep_replaced {
                kept.append(&mut candidate.replaced_parents);
            }
        }
        for (index, search) in [
            TerminalHeaderSearch::StrictDistanceCompletion,
            TerminalHeaderSearch::PayloadTradeoff,
            TerminalHeaderSearch::LiteralSpan,
            TerminalHeaderSearch::JointTreeRle,
            TerminalHeaderSearch::SymbolSets,
            TerminalHeaderSearch::AlphabetBoundaries,
            TerminalHeaderSearch::HeaderTree,
            TerminalHeaderSearch::CodeLengthRotations,
            TerminalHeaderSearch::CoupledLengthSwaps,
            TerminalHeaderSearch::HeaderResponse,
            TerminalHeaderSearch::LengthExchange,
            TerminalHeaderSearch::BoundarySlide,
            TerminalHeaderSearch::DistanceBands,
            TerminalHeaderSearch::SmoothedTrees,
            TerminalHeaderSearch::EntropyCuts,
        ]
        .into_iter()
        .enumerate()
        {
            let max_only = search.max_only();
            if max_only && !options.exhaustive {
                continue;
            }
            if !options.exhaustive
                && header_fit == HeaderFit::AfterTreeFloors
                && search.fits_codes()
            {
                continue;
            }
            let score = (candidate.data.len(), candidate.bits);
            // Fitting payloads, boundaries or trees too early can redirect a
            // later search. R1c and R13–R15 still run once no later sweep can
            // start, as in Default.
            let later_sweep_possible = options.exhaustive
                && !(matches!(
                    search,
                    TerminalHeaderSearch::BoundarySlide
                        | TerminalHeaderSearch::DistanceBands
                        | TerminalHeaderSearch::SmoothedTrees
                        | TerminalHeaderSearch::EntropyCuts
                ) && !max_work.can_start_route());
            if later_sweep_possible && search.waits_for_settled_sweep() && score != before {
                continue;
            }
            if visited[index + 1] == Some(score) {
                continue;
            }
            visited[index + 1] = Some(score);
            let work = if max_only { max_work } else { ordinary_work };
            if search.is_tree_finish() {
                finish_start.get_or_insert(score);
            }
            candidate = if search.is_tree_finish() {
                improve_with_tree_finish(
                    search,
                    source,
                    options,
                    work,
                    progress,
                    &mut parse_cache,
                    candidate,
                )?
            } else {
                improve_with_terminal_header_search_keeping(
                    search,
                    source,
                    options,
                    work,
                    progress,
                    &mut parse_cache,
                    keep_replaced,
                    candidate,
                )?
            };
            if keep_replaced {
                kept.append(&mut candidate.replaced_parents);
            }
            // A win re-slides the blocks, which can expose another R13, R14 or
            // R15 win; repeat here when no later sweep can revisit them.
            let finish_changed = matches!(search, TerminalHeaderSearch::EntropyCuts)
                && finish_start
                    .is_some_and(|start| start != (candidate.data.len(), candidate.bits));
            if finish_changed && (!options.exhaustive || !max_work.can_start_route()) {
                candidate = improve_with_repeated_tree_finish(
                    TREE_FINISH_REPEATS,
                    source,
                    options,
                    work,
                    progress,
                    &mut parse_cache,
                    candidate,
                )?;
            }
        }
        let settled = !options.exhaustive
            || (candidate.data.len(), candidate.bits) == before
            || !max_work.can_start_route();
        if settled {
            // R16 waits for the settled finish, so it never redirects R13–R15.
            let score = (candidate.data.len(), candidate.bits);
            if merged != Some(score) {
                merged = Some(score);
                candidate = improve_with_terminal_merges(
                    source,
                    options,
                    ordinary_work,
                    progress,
                    &mut parse_cache,
                    candidate,
                )?;
            }
            // A merge win is a strict improvement, so Max sweeps again while
            // it can.
            if !options.exhaustive
                || (candidate.data.len(), candidate.bits) == score
                || !max_work.can_start_route()
            {
                if keep_replaced {
                    candidate.replaced_parents = kept;
                }
                return Ok(candidate);
            }
        }
        first_sweep = false;
    }
}

/// Run one terminal method, keeping the candidate unless the result is
/// strictly smaller.
fn improve_with_terminal_header_search(
    search: TerminalHeaderSearch,
    source: CandidateInput<'_>,
    options: &Options,
    floor_work: DefaultFloorWork<'_>,
    progress: Progress,
    parse_cache: &mut TerminalParseCache,
    candidate: Candidate,
) -> Result<Candidate> {
    improve_with_terminal_header_search_keeping(
        search,
        source,
        options,
        floor_work,
        progress,
        parse_cache,
        false,
        candidate,
    )
}

/// [`improve_with_terminal_header_search`]. With `keep_replaced`, a method
/// that may start only while routes may (not R1c or R13–R17) keeps the
/// candidate its win replaces among its `replaced_parents`.
#[allow(clippy::too_many_arguments)]
fn improve_with_terminal_header_search_keeping(
    search: TerminalHeaderSearch,
    source: CandidateInput<'_>,
    options: &Options,
    floor_work: DefaultFloorWork<'_>,
    progress: Progress,
    parse_cache: &mut TerminalParseCache,
    keep_replaced: bool,
    mut candidate: Candidate,
) -> Result<Candidate> {
    // Relaxed output may omit or halve a degenerate distance tree instead.
    let strict_only = matches!(search, TerminalHeaderSearch::StrictDistanceCompletion);
    // R1c and R13–R17 are linear finalization: they may start until the hard
    // stop, not only before the soft deadline.
    let finalization = search == TerminalHeaderSearch::BoundarySlide || search.is_tree_finish();
    let may_start = if finalization {
        floor_work.can_finalize()
    } else {
        floor_work.can_start_route()
    };
    if (strict_only && !options.strict)
        || !may_start
        || candidate.data.len() > search.max_bytes(options.exhaustive)
        || source.identity.decoded_size > search.max_bytes(options.exhaustive) as u64
    {
        return Ok(candidate);
    }
    let step = progress.start(search.name());
    let refined = refine_with_terminal_header_search_cached(
        search,
        &candidate,
        options,
        source.decoded_limit,
        source.identity,
        &mut floor_work.stop(),
        parse_cache,
    )?;
    step.finish(refined.as_ref().map(|refined| {
        candidate_progress(
            refined,
            source.meaningful_bits,
            refined.is_strictly_smaller_than_source(source),
        )
    }));
    if let Some(refined) = refined {
        candidate.replace_keeping_parent(refined, keep_replaced && !finalization);
    }
    Ok(candidate)
}

/// R14 for one block: the Brotli fixed-point and Zopfli nearby-count smoothed
/// payload trees at maximum depths 15 through 9, priced against the true
/// counts, when strictly smaller than the block's current bits. Running after
/// every tree-changing method, they cannot redirect one.
fn plan_smoothed_tree_finish(
    block: &ParsedBlock,
    strict: bool,
    stop: &mut SearchStop<'_>,
) -> Option<DynamicPlan> {
    if block.source_type == SourceBlockType::Stored || block.tokens.is_empty() || stop.reached() {
        return None;
    }
    let original = block.original?;
    plan_rle_smoothed_tree_candidate(
        &block.tokens,
        &block.literal_frequencies,
        &block.distance_frequencies,
        strict,
    )
    .filter(|plan| {
        plan.bits < original.len && (!strict || plan.has_strictly_compatible_huffman_codes())
    })
}

/// Run one of R13–R17. A win replaces the trees that R1c fitted its cuts to,
/// so the result is slid once more under its new trees.
// Out of line: each call runs a terminal header search or two.
#[inline(never)]
fn improve_with_tree_finish(
    search: TerminalHeaderSearch,
    source: CandidateInput<'_>,
    options: &Options,
    floor_work: DefaultFloorWork<'_>,
    progress: Progress,
    parse_cache: &mut TerminalParseCache,
    candidate: Candidate,
) -> Result<Candidate> {
    let before = (candidate.data.len(), candidate.bits);
    let candidate = improve_with_terminal_header_search(
        search,
        source,
        options,
        floor_work,
        progress,
        parse_cache,
        candidate,
    )?;
    if (candidate.data.len(), candidate.bits) == before {
        return Ok(candidate);
    }
    improve_with_terminal_header_search(
        TerminalHeaderSearch::BoundarySlide,
        source,
        options,
        floor_work,
        progress,
        parse_cache,
        candidate,
    )
}

/// R16 after the settled finish: merge adjacent blocks, slide once more after
/// a win, and let R15 re-cut the merged blocks; then R17 replays the token
/// floor on the blocks as they stand, sliding once more after a win.
fn improve_with_terminal_merges(
    source: CandidateInput<'_>,
    options: &Options,
    floor_work: DefaultFloorWork<'_>,
    progress: Progress,
    parse_cache: &mut TerminalParseCache,
    candidate: Candidate,
) -> Result<Candidate> {
    let before = (candidate.data.len(), candidate.bits);
    let mut candidate = improve_with_tree_finish(
        TerminalHeaderSearch::Merges,
        source,
        options,
        floor_work,
        progress,
        parse_cache,
        candidate,
    )?;
    if (candidate.data.len(), candidate.bits) != before {
        candidate = improve_with_tree_finish(
            TerminalHeaderSearch::EntropyCuts,
            source,
            options,
            floor_work,
            progress,
            parse_cache,
            candidate,
        )?;
    }
    improve_with_tree_finish(
        TerminalHeaderSearch::TokenReplay,
        source,
        options,
        floor_work,
        progress,
        parse_cache,
        candidate,
    )
}

/// Run R13, R14 and R15, each with its re-slide, for at most `passes` passes,
/// stopping after a pass in which none wins.
fn improve_with_repeated_tree_finish(
    passes: usize,
    source: CandidateInput<'_>,
    options: &Options,
    floor_work: DefaultFloorWork<'_>,
    progress: Progress,
    parse_cache: &mut TerminalParseCache,
    mut candidate: Candidate,
) -> Result<Candidate> {
    for _ in 0..passes {
        let before = (candidate.data.len(), candidate.bits);
        for search in [
            TerminalHeaderSearch::DistanceBands,
            TerminalHeaderSearch::SmoothedTrees,
            TerminalHeaderSearch::EntropyCuts,
        ] {
            candidate = improve_with_tree_finish(
                search,
                source,
                options,
                floor_work,
                progress,
                parse_cache,
                candidate,
            )?;
        }
        if (candidate.data.len(), candidate.bits) == before {
            break;
        }
    }
    Ok(candidate)
}

/// The validated parse of the current terminal candidate, so consecutive
/// methods that find nothing do not reparse identical bytes. The bytes alone
/// are the key: decoded limit and stream identity are fixed within one
/// terminal sequence.
#[derive(Default)]
struct TerminalParseCache {
    entry: Option<(Vec<u8>, ParsedStream)>,
}

enum TerminalParse<'a> {
    Cached(&'a ParsedStream),
    Owned(ParsedStream),
}

impl std::ops::Deref for TerminalParse<'_> {
    type Target = ParsedStream;

    fn deref(&self) -> &ParsedStream {
        match self {
            Self::Cached(stream) => stream,
            Self::Owned(stream) => stream,
        }
    }
}

impl TerminalParseCache {
    fn clear(&mut self) {
        self.entry = None;
    }

    /// Keep a validated parse of `data` made elsewhere.
    fn keep(&mut self, data: &[u8], stream: ParsedStream) {
        self.entry = None;
        let mut bytes = Vec::new();
        if bytes.try_reserve_exact(data.len()).is_ok() {
            bytes.extend_from_slice(data);
            self.entry = Some((bytes, stream));
        }
    }

    fn parse(
        &mut self,
        data: &[u8],
        decoded_limit: u64,
        identity: StreamIdentity,
    ) -> Result<TerminalParse<'_>> {
        let hit = matches!(&self.entry, Some((bytes, _)) if bytes.as_slice() == data);
        if hit {
            let (_, stream) = self.entry.as_ref().expect("cache hit has an entry");
            return Ok(TerminalParse::Cached(stream));
        }
        // Release a stale model before building its replacement.
        self.entry = None;
        let stream = parse_validated_rewrite(data, decoded_limit, identity)?;
        let mut bytes = Vec::new();
        if bytes.try_reserve_exact(data.len()).is_err() {
            return Ok(TerminalParse::Owned(stream));
        }
        bytes.extend_from_slice(data);
        let (_, stream) = self.entry.insert((bytes, stream));
        Ok(TerminalParse::Cached(stream))
    }
}

/// Plan one terminal header method block by block, retaining every block it
/// leaves unchanged. Returns `None` when no block changed.
fn plan_terminal_blocks(
    search: TerminalHeaderSearch,
    blocks: &[ParsedBlock],
    options: &Options,
    stop: &mut SearchStop<'_>,
) -> Option<Vec<PlannedBlock>> {
    let mut plans = Vec::new();
    plans.try_reserve_exact(blocks.len()).ok()?;
    let mut budget = TerminalSearchBudget {
        header_prices: TERMINAL_HEADER_MAX_PRICES,
        joint: super::joint::JointBudget::new(),
        symbols: super::symbol_set::SymbolSetBudget::new(),
        alphabet: super::stream::AlphabetBudget::new(),
        entropy_cuts: super::stream::EntropyCutBudget::for_stream(blocks),
        header_tree: super::header::HeaderTreeBudget::new(),
        rotations: super::header::RotationBudget::new(),
        coupled_swaps: super::header::CoupledSwapBudget::new(),
        response: super::header::ResponseBudget::new(),
    };
    let mut bits = 0_u64;
    let mut changed = false;
    for (block_index, block) in blocks.iter().enumerate() {
        let alignment = (bits % 8) as u8;
        if let Some(proposed) = search.plan(block, alignment, options, &mut budget, stop) {
            // Reserve for the remaining blocks too, so later pushes never
            // allocate infallibly.
            let remaining = blocks.len() - block_index - 1;
            plans.try_reserve(proposed.len() + remaining).ok()?;
            bits = proposed
                .iter()
                .try_fold(bits, |sum, plan| sum.checked_add(plan.bits))?;
            plans.extend(proposed);
            changed = true;
            continue;
        }
        let plan = {
            let (representation, block_bits) =
                if let Some(original) = reusable_original_bits(block, alignment, options.strict) {
                    (Representation::Original(original), original.len)
                } else if block.source_type == SourceBlockType::Stored {
                    // Earlier savings can change a stored block's alignment
                    // and so its padding.
                    (
                        Representation::Stored,
                        stored_block_bits(alignment, block.plain.len()),
                    )
                } else {
                    return None;
                };
            PlannedBlock {
                tokens: block.tokens.clone(),
                plain: block.plain.clone(),
                representation,
                bits: block_bits,
                source_type: block.source_type,
            }
        };
        bits = bits.checked_add(plan.bits)?;
        plans.push(plan);
    }

    changed.then_some(plans)
}

fn refine_with_terminal_header_search_cached(
    search: TerminalHeaderSearch,
    candidate: &Candidate,
    options: &Options,
    decoded_limit: u64,
    identity: StreamIdentity,
    stop: &mut SearchStop<'_>,
    parse_cache: &mut TerminalParseCache,
) -> Result<Option<Candidate>> {
    if stop.reached()
        || candidate.data.len() > search.max_bytes(options.exhaustive)
        || identity.decoded_size > search.max_bytes(options.exhaustive) as u64
    {
        return Ok(None);
    }
    let selected = parse_cache.parse(&candidate.data, decoded_limit, identity)?;
    // Parsing drops redundant empty blocks; these methods need the exact
    // layout.
    if selected.source_block_count != selected.blocks.len()
        || selected.blocks.len() > search.max_blocks()
    {
        return Ok(None);
    }
    let plans = match search {
        TerminalHeaderSearch::BoundarySlide => {
            super::slide::plan_boundary_slide(&selected.blocks, options, stop)
        }
        TerminalHeaderSearch::Merges => {
            super::stream::plan_terminal_merges(&selected.blocks, options, stop)
        }
        _ => plan_terminal_blocks(search, &selected.blocks, options, stop),
    };
    let Some(plans) = plans else {
        return Ok(None);
    };
    let source = rewritten_input(candidate, &selected, decoded_limit, identity);
    // Zero replays keeps exactly the proposed tokens and tables.
    let (refined, parsed) = build_candidate_and_parse(
        source,
        plans,
        options,
        0,
        ReplayPlanner::Full,
        stop,
        None,
        false,
    )?;
    if !refined.is_strictly_smaller_than(candidate) {
        return Ok(None);
    }
    // The caller keeps the smaller candidate, which the next method parses.
    if let Some(parsed) = parsed {
        parse_cache.keep(&refined.data, parsed);
    }
    Ok(Some(refined.named(search.name())))
}

/// The exact Default result that a Max stream keeps before its Max routes.
struct CompleteDefaultFloor {
    /// The Default route's candidate before feedback and terminal methods; it
    /// seeds Max's routes.
    max_seed: Candidate,
    /// The complete Default endpoint, which R6–R9 may strengthen within the
    /// Max allowance.
    complete: Candidate,
    /// Default's exact endpoint when it differs from `complete` (after its
    /// final R1c, R13–R18 and, for [`HeaderFit::AfterTreeFloors`], R2–R5, or
    /// with R1b), kept only as a final competitor.
    slid: Option<Candidate>,
}

/// Default's final R1c, R13–R18 and, for [`HeaderFit::AfterTreeFloors`], R2–R5
/// on `complete`, returned only when strictly smaller. It is only a final
/// competitor: boundaries fitted to the current trees can hide an improvement
/// that Max's own tree floors would find.
fn slid_default_endpoint(
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
    if header_fit == HeaderFit::AfterTreeFloors {
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

/// R2–R5 after R18, for [`HeaderFit::AfterTreeFloors`]. R5 rewrites tokens,
/// which in every other order the tree finish and the token replay then see;
/// here, after an R5 win, R14 chooses trees for the new counts and R17 replays
/// the token floor under them, each sliding once more after a win.
fn improve_with_final_header_fit(
    source: CandidateInput<'_>,
    options: &Options,
    work: DefaultFloorWork<'_>,
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
        let before = (candidate.data.len(), candidate.bits);
        candidate = improve_with_terminal_header_search(
            search,
            source,
            options,
            work,
            progress,
            parse_cache,
            candidate,
        )?;
        if search == TerminalHeaderSearch::SymbolSets
            && (candidate.data.len(), candidate.bits) != before
        {
            for finish in [
                TerminalHeaderSearch::SmoothedTrees,
                TerminalHeaderSearch::TokenReplay,
            ] {
                candidate = improve_with_tree_finish(
                    finish,
                    source,
                    options,
                    work,
                    progress,
                    parse_cache,
                    candidate,
                )?;
            }
        }
    }
    Ok(candidate)
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

fn build_complete_default_floor_candidate(
    source: CandidateInput<'_>,
    options: &Options,
    progress: Progress,
    terminal_work: DefaultFloorWork<'_>,
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
    let complete = improve_default_floor_with_feedback(
        source,
        &floor_options,
        DefaultFloorWork::Mandatory,
        progress,
        candidate,
    )?;
    let mut parse_cache = TerminalParseCache::default();
    let (mut complete, slid) = finish_complete_default_lineages(
        source,
        &floor_options,
        progress,
        &mut parse_cache,
        complete,
        HeaderFit::InSweep,
    )?;
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
    })
}

/// Build APNG's complete Default endpoint, with D4 as `ApngDefault` runs it, and
/// its slid form, then run R6–R9 on the endpoint within this frame's Max
/// allowance.
fn build_complete_apng_default_floor_candidate(
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

/// Build the direct deft4j-derived source candidate (M1) without replay.
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

/// Build D4's candidate in the source's own block layout. Each nonempty block
/// is seeded with the smallest of its ordinary plan, its length-family chain
/// and its re-spell from literals, then searched from that seed with the
/// no-split route's narrow whole-block search, which adds the large-source
/// length bands. A reached stop leaves no candidate.
fn build_source_family_candidate(
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
fn build_narrow_source_candidate(
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

fn build_complementary_narrow_source_candidate(
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
        candidate.replace_if_smaller(refined);
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
                            && !same_tokens(&plan.tokens, &block.tokens))
                }))
}

/// Run the deft4j route on a rewritten floor, for when the direct deft4j gain
/// is weak.
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

/// Replan a rewritten candidate with the Default planner and report whether the
/// result changed blocks or tokens.
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

/// Whether a completed no-split candidate gets one no-split continuation
/// (M2c). Sibling sizes do not decide this, since a smaller sibling may have a
/// different topology.
fn changed_narrow_parent_should_continue(
    exposes_new_parent: bool,
    narrow: &Candidate,
    source: CandidateInput<'_>,
) -> bool {
    exposes_new_parent && narrow.is_strictly_smaller_than_source(source)
}

/// A compact completed parent, reparsed for the compact split floor (M10c).
/// The split runs no token search, merge search or replay, so its limits make
/// it safe to finish after the deadline.
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
    prepare_compact_source_split_seed_with_limits(
        candidate,
        decoded_limit,
        identity,
        2,
        COMPACT_SPLIT_FLOOR_MAX_DECODED,
    )
}

fn prepare_compact_source_split_seed_with_limits(
    candidate: &Candidate,
    decoded_limit: u64,
    identity: StreamIdentity,
    minimum_blocks: usize,
    maximum_decoded: u64,
) -> Result<Option<CompactSplitSeed>> {
    if candidate.data.len() > COMPACT_SPLIT_FLOOR_MAX_COMPRESSED {
        return Ok(None);
    }

    let mut data = Vec::new();
    data.try_reserve_exact(candidate.data.len())
        .map_err(|_| Error::allocation("compact route seed"))?;
    data.extend_from_slice(&candidate.data);
    let stream = parse_validated_rewrite(&data, decoded_limit, identity)?;
    if !compact_source_split_floor_eligible_with_limits(
        identity.decoded_size,
        &stream.blocks,
        minimum_blocks,
        maximum_decoded,
    ) {
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
        // The first parent always runs and polls the hard stop, keeping every
        // completed cut. Later parents start only before the soft deadline.
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
    // Smaller parents carry the best known spellings and tables, so try them
    // first. Split pricing is not monotone, so every distinct parent still
    // runs while the deadline allows.
    let mut seeds: Vec<_> = seeds.into_iter().flatten().collect();
    seeds.sort_shared_by_key(|seed| (seed.data.len(), seed.bits));
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

/// Split a compact source-max parent after the ordinary timed routes (M10d).
/// Unlike M10c, it accepts a one-block parent, whose split can find a second
/// Huffman regime, and up to 256 KiB decoded.
fn refine_with_terminal_source_split_floor_until(
    candidate: &Candidate,
    options: &Options,
    decoded_limit: u64,
    identity: StreamIdentity,
    expired: &mut SearchStop<'_>,
) -> Result<Option<Candidate>> {
    let Some(seed) = prepare_compact_source_split_seed_with_limits(
        candidate,
        decoded_limit,
        identity,
        1,
        TERMINAL_SOURCE_SPLIT_MAX_DECODED,
    )?
    else {
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

fn compact_source_split_floor_eligible_with_limits(
    decoded_size: u64,
    blocks: &[ParsedBlock],
    minimum_blocks: usize,
    maximum_decoded: u64,
) -> bool {
    if decoded_size > maximum_decoded
        || !(minimum_blocks..=COMPACT_SPLIT_FLOOR_MAX_BLOCKS).contains(&blocks.len())
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

/// Apply Columbo's bounded pair/quad balanced-tree moves to one dynamic block
/// (D3 and M4), without changing tokens or replaying.
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
    let stream = parse_validated_rewrite(&candidate.data, decoded_limit, identity)?;
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

/// Run compact proven feedback on one balanced-tree rewrite, whose new header
/// changes match and literal prices.
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

/// Apply one linear merge cleanup to a selected seed: price only adjacent
/// merges with bounded table floors, then settle the smaller list with at most
/// two Default replays while time remains.
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

/// Build one complete-stream candidate, follow strict improvements through up
/// to `replay_limit` reparse/replan rounds, and compare the endpoint-proven
/// lineage.
fn build_candidate(
    source: CandidateInput<'_>,
    options: &Options,
    replay_limit: usize,
    expired: &mut SearchStop<'_>,
) -> Result<Candidate> {
    build_candidate_with_proven_policy(source, options, replay_limit, true, expired)
}

/// Run the initial stream planner once, without replay or the endpoint-proven
/// lineage (APNG Default).
fn build_apng_default_candidate(
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

/// Build the compact proven-before-feedback candidate (D1, M3). Default runs
/// both the bounded floor and the full search; Max runs only the floor,
/// because source max covers the full search.
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

fn build_candidate_with_proven_policy(
    source: CandidateInput<'_>,
    options: &Options,
    replay_limit: usize,
    run_proven_lineage: bool,
    expired: &mut SearchStop<'_>,
) -> Result<Candidate> {
    let Some(plans) = plan_stream(
        source.blocks,
        source.compressed.len(),
        0,
        options,
        &mut *expired,
    ) else {
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

    // Run proven-submatch resegmentation as a separate lineage from the
    // endpoint. A local win inside `plan_stream` would change the tokens later
    // replays start from and can hide a better fixed point.
    let endpoint_proven = {
        // Original-bit references are invalid after an accepted replay, so
        // reparse these exact bytes.
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

/// Build a Max candidate when the caller already keeps the complete Default
/// floor, so source max need not rebuild it.
fn build_candidate_from_established_floor(
    source: CandidateInput<'_>,
    options: &Options,
    replay_limit: usize,
    integrated_compact_proven: bool,
    keep_replaced_parents: bool,
    expired: &mut SearchStop<'_>,
) -> Result<Candidate> {
    let Some(plans) = plan_stream_from_established_floor(
        source.blocks,
        source.compressed.len(),
        0,
        options,
        integrated_compact_proven,
        &mut *expired,
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
        None,
        keep_replaced_parents,
    )
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

#[derive(Clone, Copy)]
enum ReplayPlanner {
    Full,
    IntegratedProven,
    Fragmented,
}

/// Resolve [`MAX_RAW_REPLAY_LIMIT`] to eight rounds per emitted byte. Accepted rounds never
/// grow the byte length, and each length has at most eight meaningful-bit scores, so this bounds
/// the strict improvements. The emitted length can exceed the source's after strict repair.
fn resolved_replay_limit(requested: usize, emitted_bytes: usize) -> usize {
    if requested == MAX_RAW_REPLAY_LIMIT {
        emitted_bytes.saturating_mul(8).max(1)
    } else {
        requested
    }
}

/// Emit the given plans, then replay up to `replay_limit` rounds while each
/// round strictly improves. Caller-supplied plans let a route keep a locally
/// dearer seed until its new block boundaries are reparsed.
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
        false,
    )
}

// Out of line: each inlined copy also dropped the unused parse.
#[allow(clippy::too_many_arguments)]
#[inline(never)]
fn build_candidate_from_plans_with_progress(
    source: CandidateInput<'_>,
    plans: Vec<PlannedBlock>,
    options: &Options,
    replay_limit: usize,
    replay_planner: ReplayPlanner,
    expired: &mut SearchStop<'_>,
    progress: Option<&RouteProgress>,
    keep_replaced_parents: bool,
) -> Result<Candidate> {
    build_candidate_and_parse(
        source,
        plans,
        options,
        replay_limit,
        replay_planner,
        expired,
        progress,
        keep_replaced_parents,
    )
    .map(|(candidate, _)| candidate)
}

/// [`build_candidate_from_plans_with_progress`], also returning the validated
/// parse of the candidate's bytes when one was made. `keep_replaced_parents`
/// keeps each complete parent an accepted replay replaces.
#[allow(clippy::too_many_arguments)]
fn build_candidate_and_parse(
    source: CandidateInput<'_>,
    mut plans: Vec<PlannedBlock>,
    options: &Options,
    replay_limit: usize,
    replay_planner: ReplayPlanner,
    expired: &mut SearchStop<'_>,
    progress: Option<&RouteProgress>,
    keep_replaced_parents: bool,
) -> Result<(Candidate, Option<ParsedStream>)> {
    let (mut data, mut bits) = emit_plans(source.compressed, &plans, options.strict)?;
    let replay_limit = resolved_replay_limit(replay_limit, data.len());
    let initial_loses = !is_strictly_better(
        data.len(),
        bits,
        source.compressed.len(),
        source.meaningful_bits,
    );

    // The validated parse of `data`, once one is made.
    let mut validated = None;
    let mut output_max_distance = None;
    let mut max_planner_is_stable = false;
    let mut replaced_parents = Vec::new();
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
        output_max_distance = Some(replayed.max_distance);

        let replay_plans = match replay_planner {
            ReplayPlanner::Full => match progress {
                Some(progress) => plan_stream_with_progress(
                    &replayed.blocks,
                    data.len(),
                    0,
                    options,
                    options.exhaustive,
                    false,
                    &mut *expired,
                    progress,
                ),
                None if options.exhaustive => plan_stream_from_established_floor(
                    &replayed.blocks,
                    data.len(),
                    0,
                    options,
                    false,
                    &mut *expired,
                ),
                None => plan_stream(&replayed.blocks, data.len(), 0, options, &mut *expired),
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
            validated = Some(replayed);
            break;
        };
        let (replay_data, replay_bits) = emit_plans(&data, &replay_plans, options.strict)?;
        if !is_strictly_better(replay_data.len(), replay_bits, data.len(), bits) {
            // A completed Max replay that reproduces the current bytes lets
            // M13 skip repeating it; a deadline-truncated one proves nothing.
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
            validated = Some(replayed);
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
        let parent = std::mem::replace(&mut data, replay_data);
        if keep_replaced_parents {
            replaced_parents.push(Candidate {
                data: parent,
                bits,
                output_max_distance,
                plans: Vec::new(),
                block_report: None,
                route: "Columbo source-max earlier parent",
                max_planner_is_stable: false,
                replaced_parents: Vec::new(),
            });
        }
        bits = replay_bits;
        plans = replay_plans;
        output_max_distance = None;
    }

    // The loop may stop right after an accepted replay. A known-losing relaxed
    // stream is skipped, since the caller keeps the validated source bytes.
    if (!initial_loses || options.strict) && validated.is_none() {
        let replayed = parse_validated_rewrite(&data, source.decoded_limit, source.identity)?;
        output_max_distance = Some(replayed.max_distance);
        validated = Some(replayed);
    }

    let block_report = capture_planned_block_report(&plans, reports_enabled(options));
    let candidate = Candidate {
        data,
        bits,
        output_max_distance,
        plans,
        block_report,
        route: if options.exhaustive {
            "Columbo max route"
        } else {
            "Normal route"
        },
        max_planner_is_stable,
        replaced_parents,
    };
    Ok((candidate, validated))
}

/// Retain a parsed source when the mandatory plan list cannot be allocated.
/// Strict mode must rewrite incompatible Huffman alphabets, so it reports the
/// allocation failure instead.
fn source_candidate(source: CandidateInput<'_>, options: &Options) -> Result<Candidate> {
    if options.strict {
        return Err(Error::allocation("Deflate plan"));
    }
    let mut data = Vec::new();
    data.try_reserve_exact(source.compressed.len())
        .map_err(|_| Error::allocation("Deflate output"))?;
    data.extend_from_slice(source.compressed);
    Ok(Candidate {
        data,
        bits: source.meaningful_bits,
        output_max_distance: None,
        plans: Vec::new(),
        block_report: None,
        route: "Original source",
        max_planner_is_stable: false,
        replaced_parents: Vec::new(),
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

/// The parser has already computed every compared value, so this check is
/// cheap even in release builds.
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
    })
}

fn capture_source_block_report(
    blocks: &[ParsedBlock],
    source_block_count: usize,
    enabled: bool,
) -> Option<BlockReport> {
    // The parser collapses runs of empty blocks; do not present that model as
    // the source.
    if !enabled || blocks.len() != source_block_count {
        return None;
    }
    if blocks.iter().any(|block| block.original.is_none()) {
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

#[cfg(test)]
mod tests;
