// SPDX-License-Identifier: MIT
// SPDX-FileCopyrightText: 2026 Andrew C.E. Dent <https://github.com/ace-dent>

//! Raw-stream optimization orchestration and complete-candidate selection.
//!
//! Validate the source before scheduling optional routes, retain the required
//! Default comparison floor, and compare emitted candidates by physical bytes
//! followed by meaningful bits. Search policies live here; block-boundary and
//! token-spelling algorithms live in `stream` and `search` respectively.

use std::thread;
use std::time::Instant;

use crate::progress::{
    reports_enabled, BlockEncoding, BlockProgress, BlockReport, CandidateProgress, Progress,
    RouteProgress, MAX_REPORTED_BLOCKS,
};
use crate::{Error, Options, Result};

use super::bitstream::BitWriter;
use super::block::{emit_block, plan_block, reusable_original_bits, stored_block_bits};
use super::header::{
    plan_bounded_depth_tree_candidate, plan_columbo_balanced_tree_candidate,
    plan_for_explicit_lengths, plan_literal_span, plan_payload_header_tradeoff,
    plan_rle_smoothed_tree_candidate,
};
use super::model::{
    ParsedBlock, ParsedStream, PlannedBlock, Representation, SourceBlockType, Token,
};
use super::parse::{parse_stream, parsed_model_bytes};
use super::restore::{
    plan_original_match_restoration, MAX_RESTORATION_BLOCKS, MAX_RESTORATION_BYTES,
};
use super::search::{
    compact_proven_submatch_route_eligible, improve_plan_with_header_aware_proven_composition,
    improve_plan_with_integrated_proven_floor, improve_plan_with_short_family_floor,
    plan_block_with_integrated_proven_search, rewrite_258_symbols, same_distance_opportunities,
    PROVEN_SUBMATCH_FULL_MATCH_LIMIT,
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

/// Long source-block chains can need one pass to establish profitable adjacent
/// groups, then two inexpensive passes over that much simpler block layout to
/// settle their boundaries and tables. Every round below must strictly improve
/// the complete stream, so the extra slot cannot oscillate or grow the output.
const DEFAULT_RAW_REPLAY_LIMIT: usize = 3;
/// Bound terminal header work even inside the mandatory Default comparison
/// floor. The swap/span passes admit at most 1,024 full header prices each;
/// joint tree/RLE search has its own shared operation and scratch bounds.
const TERMINAL_HEADER_MAX_BYTES: usize = 128 * 1024;
/// Max's terminal header methods share this larger work class. Their search
/// budgets stay bounded independently of the surrounding decoded bytes.
/// Stream owners also reserve terminal time throughout the admitted class.
const MAX_TERMINAL_HEADER_MAX_BYTES: usize = 1024 * 1024;
const TERMINAL_HEADER_MAX_BLOCKS: usize = 128;
const TERMINAL_HEADER_MAX_PRICES: usize = 1024;
/// Max uses the sentinel below to resolve a proof-derived replay ceiling after
/// its initial candidate is emitted. For an L-byte stream there are only 8L
/// possible (byte length, meaningful-bit residue) scores no worse than it, and
/// every accepted replay strictly improves that pair. This reaches a metric
/// fixed point given sufficient time without an arbitrary eight-round cutoff.
const MAX_RAW_REPLAY_LIMIT: usize = usize::MAX;
const NARROW_SOURCE_LIST_MAX_BLOCKS: usize = 128;
/// Three source blocks expose at most two adjacent-merge boundaries, so the
/// individual-pruning walk can still cover the complete short list. Longer
/// chains prioritize cumulative pruning first: local work on an early block can
/// otherwise consume the route window before later alignment/merge states are
/// visited. The complementary policy remains eligible later in Max.
const CUMULATIVE_NO_SPLIT_MIN_SOURCE_BLOCKS: usize = 4;
const WEAK_DEFT4J_GAIN_BASIS_POINTS: u64 = 200;
/// The no-split sibling is linear in the source block list, but each bounded
/// per-block search retains route-local candidates. Pair a 1 MiB compressed
/// ceiling with the 128-block ceiling below so this worker stays well inside
/// the existing 64 MiB parallel-model class. No lower size bound is needed: on
/// compact multi-block streams it cheaply reaches later blocks that a broad
/// source-order graph may not visit before the deadline.
const NARROW_SOURCE_MAX_COMPRESSED: usize = 1_024 * 1_024;
/// A completed compact deft4j-derived seed may expose useful eighth-position
/// child splits after the timed route has settled its tokens and source joins.
/// Keep this deterministic Columbo floor tightly bounded: it finishes at most
/// seven structural prices per block and never starts another token search.
const COMPACT_SPLIT_FLOOR_MAX_COMPRESSED: usize = 16 * 1024;
const COMPACT_SPLIT_FLOOR_MAX_DECODED: u64 = 128 * 1024;
const TERMINAL_SOURCE_SPLIT_MAX_DECODED: u64 = 256 * 1024;
const COMPACT_SPLIT_FLOOR_MAX_BLOCKS: usize = 4;
const COMPACT_SPLIT_FLOOR_MAX_TOKENS: usize = 16 * 1024;
/// A large one-block source beam competes with the completed PNG floor for the
/// same cache and memory bandwidth. Start both only when independent
/// same-distance repartitions are common enough to justify that competition:
/// one opportunity per sixteen source tokens keeps prospective structural work
/// proportional to the token graph that must be searched.
const DENSE_REPARTITION_TOKENS_PER_RUN: usize = 16;
/// The original Columbo C quad-lengthening move is a bounded one-block header
/// floor. Its upper model limits avoid turning it into another general search;
/// no corpus-derived lower size or token threshold is needed.
const COMPACT_TREE_MAX_COMPRESSED: usize = 8 * 1_024;
const COMPACT_TREE_MAX_DECODED: u64 = 128 * 1_024;
const COMPACT_TREE_MAX_TOKENS: usize = 4_096;
const RLE_SMOOTHED_TREE_FLOOR_MAX_BLOCKS: usize = 8;
const DEFAULT_STRICT_TREE_ROUNDS: usize = 4;
/// A complementary source-root beam remains cheap on very small token graphs,
/// even when proven feedback has already improved the ordinary floor. Retain
/// both basins through a 2,048-token graph; above it, the extra beam competes
/// materially with the richer floor-derived route.
const COMPACT_COMPLEMENTARY_SOURCE_MAX_TOKENS: usize = 2_048;
/// The compact proven-feedback route is itself limited to 4,000 tokens and
/// exact-prices only a capped set of candidate siblings. In the upper
/// three-eighths of that work class, run one broad source-token owner instead
/// of two long workers. Source max retains the wider state graph, and its
/// deterministic finalization still applies proven feedback to a winning
/// compact header rewrite. The restart still requires either a proved
/// same-distance repartition or enough decoded data to span RFC 1951's maximum
/// stored-block payload; without either topology, the floor lineage retains the
/// only structurally motivated basin. Smaller graphs retain the cheaper proven
/// lineage unless multiple independent repartitions justify source max.
const COMPACT_SINGLE_SOURCE_ROUTE_MAX_TOKENS: usize = 4_000;
const COMPACT_SINGLE_SOURCE_ROUTE_MIN_TOKENS: usize =
    COMPACT_SINGLE_SOURCE_ROUTE_MAX_TOKENS * 5 / 8;
/// A source graph whose tokens each cover almost a full Deflate match on
/// average has little literal/alphabet work for every decoded byte. Within the
/// existing 4,000-token single-route bound, 224 bytes (7/8 of 256) provides a
/// rounded, format-independent definition of that cheap long-match topology.
const NEAR_MAX_MATCH_MEAN_DECODED_PER_TOKEN: u64 = 7 * 32;
const DEFLATE_MAX_STORED_BLOCK_PLAIN: u64 = 65_535;
/// Parallel routes shorten a container's wall-clock search without making its
/// peak memory proportional to every individually valid route budget. Larger
/// streams retain the same candidates, but evaluate them serially.
const PARALLEL_ROUTE_MAX_COMPRESSED: usize = 8 * 1_024 * 1_024;
const PARALLEL_ROUTE_MAX_DECODED: u64 = 64 * 1_024 * 1_024;
const PARALLEL_ROUTE_MAX_MODEL: usize = 64 * 1_024 * 1_024;
/// A post-deadline tree rescue reparses and may re-emit its completed parent.
/// Cap both byte traversals at one MiB and the block walk at the existing
/// narrow-list bound; only one block receives the fixed-alphabet tree search.
const BOUNDED_DEPTH_RESCUE_MAX_COMPRESSED: usize = 1_024 * 1_024;
const BOUNDED_DEPTH_RESCUE_MAX_DECODED: u64 = 1_024 * 1_024;
/// A small ordinary floor is cheap enough to establish before launching the
/// heavier source graphs. Above this class, overlap preserves max-search wall
/// time unless the floor's decoded work is itself large enough to cause working
/// set contention. The match-work check at the call site also prebuilds floors
/// whose source graph cannot reliably finish inside the initial four-fifths.
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
    /// Largest backward distance actually emitted in `data`.
    pub(crate) output_max_distance: u16,
    pub(crate) timed_out: bool,
}

/// Decide how a caller establishes the ordinary-mode comparison floor.
///
/// A standalone stream uses [`DefaultFloor::Complete`] to try the ordinary
/// route before max-only work. A single scheduled PNG image uses
/// [`DefaultFloor::CompleteThenBounded`] for the same ordering before PNG's
/// bounded max routes. Both finish the comparison floor even when Max's
/// optional-search deadline expires: timeout pressure may curtail Max work, but
/// it must never make Max worse than Default. Multi-stream containers use
/// [`DefaultFloor::Shared`] so one member cannot consume time needed by later
/// members. [`DefaultFloor::SharedExact`] keeps the same multi-stream schedule
/// but retains the complete ordinary feedback endpoint before Max-only work.
/// Multi-image APNG Default uses [`DefaultFloor::ApngDefault`] to keep the full
/// initial planner but leave repeated replay and feedback lineages to Max.
/// Multi-image APNG Max uses [`DefaultFloor::ApngMax`] to retain shared
/// container scheduling while running the full Max route set. Every Max floor
/// admits potentially useful direct deft4j source work; the floor enum no
/// longer acts as a permanent topology gate. [`DefaultFloor::Established`]
/// means the caller already retains the complete input stream as its comparison
/// floor, so descendants can begin without rebuilding an ordinary candidate.
/// [`DefaultFloor::MandatoryComplete`] is the corresponding policy for a
/// container-level Default branch built on behalf of Max: the branch uses
/// ordinary routes, but its raw members cannot be interrupted by Max's clock.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DefaultFloor {
    Complete,
    CompleteThenBounded,
    Shared,
    SharedExact,
    ApngDefault,
    ApngMax,
    Established,
    MandatoryComplete,
}

impl DefaultFloor {
    fn is_bounded(self) -> bool {
        !matches!(self, Self::Complete | Self::MandatoryComplete)
    }

    fn uses_bounded_png_routes(self) -> bool {
        matches!(self, Self::CompleteThenBounded)
    }

    /// Whether a bounded PNG owner can run original-source Max beside the
    /// dependent deft4j refinement inside the same assigned wall window.
    ///
    /// A standalone PNG owns the file window directly. Each APNG child owns a
    /// proportional frame window assigned by the outer scheduler. Both start
    /// the independent source root even when the direct deft4j parent is
    /// immediately smaller: score order between complete parents does not
    /// prove order between their eventual search endpoints. Giving both roots
    /// a positive concurrent share makes each reachable as the allowance
    /// grows. Other shared container members retain their serial route order.
    fn allows_parallel_source_follow_up(self) -> bool {
        matches!(self, Self::CompleteThenBounded | Self::ApngMax)
    }

    fn owns_terminal_stream_time(self) -> bool {
        matches!(self, Self::Complete | Self::CompleteThenBounded)
    }

    /// Give admitted terminal methods a positive share instead of allowing an
    /// unfinished primary search to make their endpoint unreachable. An APNG
    /// Max child owns a proportional image-job slice, so it can reserve the
    /// tail of that slice without consuming a sibling frame's allowance.
    /// An established continuation also runs in its own window; outside the
    /// header work class it reserves the same linear finalization share.
    /// Other shared streams keep their schedule.
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
    /// The terminal header methods' work class. Primary work keeps four
    /// fifths, or an APNG child nineteen twentieths of its slice.
    Search,
    /// Outside that class only linear finalization, the bounded-depth tree
    /// floor and R1c, can use terminal time, so primary work keeps nineteen
    /// twentieths.
    Finalization,
}

pub(crate) fn optimize_raw(input: &[u8], options: &Options) -> Result<RawOptimization> {
    let mut optimized = optimize_raw_prefix(input, options, options.max_decoded_bytes)?;
    if !options.strip_metadata {
        optimized
            .data
            .try_reserve(input.len() - optimized.consumed)
            .map_err(|_| Error::internal("could not allocate Deflate suffix"))?;
        optimized
            .data
            .extend_from_slice(&input[optimized.consumed..]);
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

/// Parse the first raw stream in `input` once, returning its consumed length
/// and whether a PNG Max scheduler benefits from an early transformed lineage
/// beside its exact Default lineage.
///
/// A dense same-distance graph needs the reduced parent because the direct
/// bounded graph cannot enumerate every combination. A small multi-block
/// stream needs it for a different reason: exact Default is deliberately
/// established serially for this work class, so it can otherwise consume a
/// short Max allowance before any independent source route starts. Larger
/// multi-block floors already overlap those routes internally and must not
/// receive a redundant outer worker. The probe performs only the normal
/// bounded parse; bytes after the stream belong to the caller's wrapper, and
/// optimization reparses and independently validates every selectable output.
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
        let tokens =
            rewrite_258_symbols(&block.tokens, block.plain.len(), false).ok_or_else(|| {
                Error::internal("could not allocate strict Deflate token normalization")
            })?;
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
    /// Exact maximum distance from validation of these emitted bytes.
    output_max_distance: Option<u16>,
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
/// max has a small predictable memory and cache-bandwidth cost. One-block
/// inputs use the ordinary aggregate bound. A short multi-block list is also
/// safe when its total decoded work stays within that bound per non-empty
/// source block, the complete token graph stays compact, and multiple proved
/// repartitions justify starting source max before the dependent deft4j
/// refinement. Larger streams retain the sequential schedule so two
/// range-materializing beams do not starve the stronger completed floor.
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

/// Whether a source-root beam should overlap an already completed PNG floor.
///
/// A single large decoded block makes both beams materialize and price many of
/// the same payload ranges. Let the stronger completed-floor lineage own that
/// cache/memory bandwidth unless the source graph either has dense independent
/// repartitions or consists almost entirely of long matches. The latter keeps
/// range pricing cheap because few tokens and literal symbols represent each
/// decoded region. Very small token graphs are also cheap enough to overlap,
/// while shared multi-stream policies retain
/// [`bounded_parallel_source_max_work_class`] so one member cannot remove
/// another member's independent source route.
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
    /// The slid Default endpoint, compared only after terminal searches.
    slid_default: Option<Candidate>,
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

/// Route families requested for the bounded comparison phase.
///
/// The phase still rechecks each family's work class and route window before
/// starting it, so a requested family may not run.
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
    /// Run admitted families on scoped workers instead of serially.
    parallel: bool,
}

/// Run the independent bounded comparison routes under one wall clock.
///
/// Small inputs can safely share their immutable parsed blocks across worker
/// threads. Larger inputs use the same fixed route order serially, preventing
/// otherwise bounded per-route arenas from adding up to an excessive peak.
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
                    // The no-split walk combines block-local pruning with
                    // adjacent merges. That ordering is complementary to
                    // source max: an early block choice changes alignment
                    // and merge prices for every later source block.
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

        // A route error (or unwind) asks its siblings to stop at their next
        // ordinary deadline check. Join every successfully spawned worker
        // before choosing the fixed deft4j/narrow/floor error order below.
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

/// Retain the deft4j/narrow/floor deadline order without overlapping arenas.
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
    // A serial work class cannot overlap its exact comparison floor with the
    // independent source routes. Establish the mandatory floor first so those
    // optional routes can never consume the result Max promises to retain.
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
///
/// `None` means the worker could not be spawned; each caller keeps its own
/// serial fallback for that case.
fn spawn_route<'scope, T: Send + 'scope>(
    scope: &'scope thread::Scope<'scope, '_>,
    name: &str,
    deadline: &'scope Deadline,
    route: impl FnOnce() -> Result<T> + Send + 'scope,
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

/// Copy a complete caller-retained stream into the local candidate set.
///
/// `Established` is used only for a stream just emitted and validated by an
/// independent Columbo lineage. Re-running Default here would rediscover a
/// floor the caller already owns; later descendants reparse these bytes before
/// using them, preserving the normal identity checks.
fn established_floor_candidate(source: CandidateInput<'_>) -> Result<Candidate> {
    let mut data = Vec::new();
    data.try_reserve_exact(source.compressed.len())
        .map_err(|_| Error::internal("could not allocate Deflate output"))?;
    data.extend_from_slice(source.compressed);
    Ok(Candidate {
        data,
        bits: source.meaningful_bits,
        output_max_distance: None,
        plans: Vec::new(),
        block_report: None,
        route: "Normal floor",
        max_planner_is_stable: false,
    })
}

/// Controls whether ordinary comparison work may be interrupted.
///
/// A normal invocation remains governed by its configured deadline. Max uses
/// `Mandatory` only for the Default dependency it promises to dominate; every
/// Max-exclusive descendant continues to use the ordinary timed policy.
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

/// Add the bounded siblings that form the complete ordinary-mode floor.
///
/// These routes are deliberately shared by a normal invocation and the floor
/// established at the start of a single-stream PNG max invocation. Keeping the
/// sequence in one helper prevents max from approximating Default with only
/// its first route and losing a completed byte or bit saving.
fn improve_default_floor_with_feedback(
    source: CandidateInput<'_>,
    options: &Options,
    floor_work: DefaultFloorWork<'_>,
    progress: Progress,
    mut candidate: Candidate,
) -> Result<Candidate> {
    debug_assert!(!options.exhaustive);

    // Proven-before-feedback has a distinct compact fixed point from the
    // ordinary endpoint ordering. Retain both as complete candidates whenever
    // the source contains a proved match and the whole sibling is within its
    // explicit token/plain work bounds.
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

/// Apply terminal tree-only improvements to one already complete candidate.
///
/// This sequence is shared deliberately by an ordinary invocation and Max's
/// retained Default comparison floor. Keeping every Default terminal method
/// here prevents a newly added cleanup pass from silently making Max worse
/// when its independent searches consume the remaining deadline.
fn improve_with_terminal_tree_floors(
    source: CandidateInput<'_>,
    options: &Options,
    floor_work: DefaultFloorWork<'_>,
    progress: Progress,
    mut candidate: Candidate,
) -> Result<Candidate> {
    // The fixed-point and nearby-count smoothers each use a seven-pair exact
    // tree frontier, but keep their terminal reparse inside the same compact
    // memory/work class. The completed output, rather than the source, decides
    // whether its final block topology is applicable.
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

    // The compact pass above shares this frontier when its structural work
    // bounds apply. Every other completed stream receives the general linear
    // sibling. At the hard boundary, the explicitly bounded rescue finishes
    // one fixed work unit while retaining the complete incumbent.
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

/// Keep restoration separate from tree closure: earlier losing topologies may
/// receive tree closure before search is over, but this new token spelling is
/// a terminal candidate and must not redirect their existing descendants.
fn improve_with_original_match_restoration(
    source: CandidateInput<'_>,
    options: &Options,
    floor_work: DefaultFloorWork<'_>,
    progress: Progress,
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
        candidate.replace_if_smaller(restored);
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
    // Parsing discards redundant empty blocks. This pass keeps the parent's
    // exact header/block layout, so leave normalization to existing routes.
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
    // Zero replays preserves the exact payload trees and header spelling. The
    // existing builder validates emitted bytes and records their actual max
    // distance for wrappers; a restored match can exceed the parent's distance.
    let restored =
        build_candidate_from_plans(source, plans, options, 0, ReplayPlanner::Full, stop)?;
    Ok(restored
        .is_strictly_smaller_than(candidate)
        .then(|| restored.named("Original-match restoration")))
}

#[derive(Clone, Copy)]
enum TerminalHeaderSearch {
    StrictDistanceCompletion,
    PayloadTradeoff,
    LiteralSpan,
    JointTreeRle,
    SymbolSets,
    DistanceBands,
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
        // The boundary slide and distance ladder are linear in the parsed
        // tokens and blocks. Their one candidate parse costs no more than a
        // replay round, which every candidate may already pay at any size, so
        // they have no work class.
        if matches!(self, Self::BoundarySlide | Self::DistanceBands) {
            return usize::MAX;
        }
        // The smaller class also bounds mandatory Default work. Optional Max
        // header searches can reuse the larger parsed-stream envelope without
        // increasing their per-invocation work, price or block-local limits.
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
            Self::BoundarySlide | Self::DistanceBands => usize::MAX,
            _ => TERMINAL_HEADER_MAX_BLOCKS,
        }
    }

    /// Max methods that run only after a sweep in which no earlier method
    /// changed the candidate.
    fn waits_for_settled_sweep(self) -> bool {
        matches!(
            self,
            Self::DistanceBands | Self::HeaderResponse | Self::LengthExchange | Self::BoundarySlide
        )
    }

    /// Methods that run only in Max.
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
                    Self::BoundarySlide
                    | Self::SymbolSets
                    | Self::DistanceBands
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

/// Close the final Max endpoint under the existing terminal methods.
///
/// A later method can change the tokens, payload lengths or alphabet spans
/// priced by an earlier method. Finishing one ordered sweep therefore does
/// not establish a local fixed point. Revisit changed inputs while optional
/// time remains, retaining the original single sweep in Default mode.
/// Every accepted candidate strictly decreases (bytes, meaningful bits), so
/// an input score uniquely identifies its generation within this descent.
/// Remembering each method's input skips suffix work already completed on
/// the unchanged candidate without retaining additional encoded streams.
fn improve_with_terminal_searches(
    source: CandidateInput<'_>,
    options: &Options,
    default_work: DefaultFloorWork<'_>,
    max_work: DefaultFloorWork<'_>,
    progress: Progress,
    mut candidate: Candidate,
) -> Result<Candidate> {
    let mut visited = [None; 14];
    let mut first_sweep = true;
    let mut parse_cache = TerminalParseCache::default();
    loop {
        let ordinary_work = if first_sweep { default_work } else { max_work };
        let before = (candidate.data.len(), candidate.bits);
        if visited[0] != Some(before) {
            visited[0] = Some(before);
            // Restoration parses its own model; do not hold a second one.
            parse_cache.clear();
            candidate = improve_with_original_match_restoration(
                source,
                options,
                ordinary_work,
                progress,
                candidate,
            )?;
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
        ]
        .into_iter()
        .enumerate()
        {
            let max_only = search.max_only();
            if max_only && !options.exhaustive {
                continue;
            }
            let score = (candidate.data.len(), candidate.bits);
            // Settle the established methods before fitting a new payload to
            // a proposed tree, boundaries to the current trees, or a
            // collapsed distance alphabet to the slid blocks. Earlier
            // adoption can redirect a later search and lose an improvement
            // reachable from the unchanged endpoint. Default's single sweep
            // has no later search, so its boundary slide and distance ladder
            // run regardless, as do Max's once no further sweep can start.
            let later_sweep_possible = options.exhaustive
                && !(matches!(
                    search,
                    TerminalHeaderSearch::BoundarySlide | TerminalHeaderSearch::DistanceBands
                ) && !max_work.can_start_route());
            if later_sweep_possible && search.waits_for_settled_sweep() && score != before {
                continue;
            }
            if visited[index + 1] == Some(score) {
                continue;
            }
            visited[index + 1] = Some(score);
            let work = if max_only { max_work } else { ordinary_work };
            candidate = if matches!(search, TerminalHeaderSearch::DistanceBands) {
                improve_with_distance_ladder(
                    source,
                    options,
                    work,
                    progress,
                    &mut parse_cache,
                    candidate,
                )?
            } else {
                improve_with_terminal_header_search(
                    search,
                    source,
                    options,
                    work,
                    progress,
                    &mut parse_cache,
                    candidate,
                )?
            };
        }
        if !options.exhaustive
            || (candidate.data.len(), candidate.bits) == before
            || !max_work.can_start_route()
        {
            return Ok(candidate);
        }
        first_sweep = false;
    }
}

/// Header-driven terminal searches retain every complete parent independently.
/// Alphabet search can split blocks; the other searches keep their boundaries.
/// None redirects the established search lineages.
fn improve_with_terminal_header_search(
    search: TerminalHeaderSearch,
    source: CandidateInput<'_>,
    options: &Options,
    floor_work: DefaultFloorWork<'_>,
    progress: Progress,
    parse_cache: &mut TerminalParseCache,
    mut candidate: Candidate,
) -> Result<Candidate> {
    // Relaxed output may omit or halve a degenerate distance tree instead.
    let strict_only = matches!(search, TerminalHeaderSearch::StrictDistanceCompletion);
    // The boundary slide and distance ladder are linear finalization, like
    // the bounded-depth tree floor: they may start until the hard stop they
    // poll, not only before the soft deadline that admits new search routes.
    let may_start = if matches!(
        search,
        TerminalHeaderSearch::BoundarySlide | TerminalHeaderSearch::DistanceBands
    ) {
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
        candidate.replace_if_smaller(refined);
    }
    Ok(candidate)
}

/// R13 follows R1c: a collapsed distance alphabet replaces trees the slide
/// fitted its cuts to, so a winning ladder is slid once more under its new
/// trees. Each step keeps its parent unless strictly smaller.
fn improve_with_distance_ladder(
    source: CandidateInput<'_>,
    options: &Options,
    floor_work: DefaultFloorWork<'_>,
    progress: Progress,
    parse_cache: &mut TerminalParseCache,
    candidate: Candidate,
) -> Result<Candidate> {
    let before = (candidate.data.len(), candidate.bits);
    let candidate = improve_with_terminal_header_search(
        TerminalHeaderSearch::DistanceBands,
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

/// The validated parse of an unchanged terminal candidate.
///
/// Terminal methods replace a candidate only with a strictly smaller one, so
/// consecutive methods that find nothing would otherwise reparse and revalidate
/// identical bytes. Each entry keeps the exact bytes it validated and is reused
/// only for an identical candidate within one terminal sequence, whose decoded
/// limit and stream identity are fixed.
#[derive(Default)]
struct TerminalParseCache {
    entry: Option<(Vec<u8>, ParsedStream)>,
}

/// A terminal candidate's parse, borrowed from the cache when possible.
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
            // Also reserve one original plan for every remaining source
            // block, so falling back after a split never grows infallibly.
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
                    // Earlier savings can shift the next stored block.
                    // Regenerate and price its padding at the actual
                    // new alignment.
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
    // The parser discards redundant empty blocks. Preserve the parent's block
    // source mapping here and leave empty-block normalization to established routes.
    if selected.source_block_count != selected.blocks.len()
        || selected.blocks.len() > search.max_blocks()
    {
        return Ok(None);
    }
    let plans = match search {
        // The slide moves tokens between neighbouring blocks, so it plans the
        // whole stream rather than one block at a time.
        TerminalHeaderSearch::BoundarySlide => {
            super::slide::plan_boundary_slide(&selected.blocks, options, stop)
        }
        _ => plan_terminal_blocks(search, &selected.blocks, options, stop),
    };
    let Some(plans) = plans else {
        return Ok(None);
    };
    let source = rewritten_input(candidate, &selected, decoded_limit, identity);
    // Zero replays preserves exactly the proposed tokens and tables. The common
    // builder validates emission and records the actual wrapper window needs.
    let refined = build_candidate_from_plans(source, plans, options, 0, ReplayPlanner::Full, stop)?;
    Ok(refined
        .is_strictly_smaller_than(candidate)
        .then(|| refined.named(search.name())))
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
    /// The complete Default endpoint, optionally strengthened by the bounded
    /// Max-only terminal siblings within the existing Max allowance.
    complete: Candidate,
    /// The Default result after R1c and R13, when either wins.
    slid: Option<Candidate>,
}

/// Default ends its single sweep with R1c's boundary slide and R13's
/// distance ladder. Max keeps that result only as a final competitor:
/// boundaries fitted to the current trees can remove an improvement Max's own
/// tree floors would find from the unslid endpoint, so the slide must not
/// choose Max's terminal parent.
fn slid_default_endpoint(
    source: CandidateInput<'_>,
    floor_options: &Options,
    progress: Progress,
    parse_cache: &mut TerminalParseCache,
    complete: &Candidate,
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
    let slid = improve_with_distance_ladder(
        source,
        floor_options,
        DefaultFloorWork::Mandatory,
        progress,
        parse_cache,
        slid,
    )?;
    Ok(slid.is_strictly_smaller_than(complete).then_some(slid))
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
    let complete = improve_with_original_match_restoration(
        source,
        &floor_options,
        DefaultFloorWork::Mandatory,
        progress,
        complete,
    )?;
    let mut parse_cache = TerminalParseCache::default();
    let complete = improve_with_terminal_header_search(
        TerminalHeaderSearch::StrictDistanceCompletion,
        source,
        &floor_options,
        DefaultFloorWork::Mandatory,
        progress,
        &mut parse_cache,
        complete,
    )?;
    let complete = improve_with_terminal_header_search(
        TerminalHeaderSearch::PayloadTradeoff,
        source,
        &floor_options,
        DefaultFloorWork::Mandatory,
        progress,
        &mut parse_cache,
        complete,
    )?;
    let complete = improve_with_terminal_header_search(
        TerminalHeaderSearch::LiteralSpan,
        source,
        &floor_options,
        DefaultFloorWork::Mandatory,
        progress,
        &mut parse_cache,
        complete,
    )?;
    let complete = improve_with_terminal_header_search(
        TerminalHeaderSearch::JointTreeRle,
        source,
        &floor_options,
        DefaultFloorWork::Mandatory,
        progress,
        &mut parse_cache,
        complete,
    )?;
    let complete = improve_with_terminal_header_search(
        TerminalHeaderSearch::SymbolSets,
        source,
        &floor_options,
        DefaultFloorWork::Mandatory,
        progress,
        &mut parse_cache,
        complete,
    )?;
    let slid = slid_default_endpoint(
        source,
        &floor_options,
        progress,
        &mut parse_cache,
        &complete,
    )?;
    // Max alone may strengthen the completed ordinary comparison endpoint.
    // The historical seed stays independent, and this extra search consumes
    // the caller's existing Max allowance rather than mandatory Default work.
    let mut complete = complete;
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

/// Keep APNG's Default endpoint, then admit the Max-only terminal siblings
/// within this frame's existing allowance.
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
    let mut complete = improve_with_original_match_restoration(
        source,
        &floor_options,
        DefaultFloorWork::Mandatory,
        progress,
        initial,
    )?;
    let mut parse_cache = TerminalParseCache::default();
    for search in [
        TerminalHeaderSearch::StrictDistanceCompletion,
        TerminalHeaderSearch::PayloadTradeoff,
        TerminalHeaderSearch::LiteralSpan,
        TerminalHeaderSearch::JointTreeRle,
        TerminalHeaderSearch::SymbolSets,
    ] {
        complete = improve_with_terminal_header_search(
            search,
            source,
            &floor_options,
            DefaultFloorWork::Mandatory,
            progress,
            &mut parse_cache,
            complete,
        )?;
    }
    let slid = slid_default_endpoint(
        source,
        &floor_options,
        progress,
        &mut parse_cache,
        &complete,
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

/// Build the bounded lineage while retaining the exact ordinary endpoint.
///
/// Medium multi-block PNG streams reach this helper from the concurrent phase:
/// their independent Max workers continue in parallel, while this route runs
/// the same complete sequence as Default and keeps that result separate from
/// the historical Max seed. This closes the comparison-floor invariant
/// without serializing work that was deliberately admitted for concurrency.
/// The third result is the slid Default endpoint, a final competitor only.
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
/// Encoded score alone cannot dominate this dependency: even a smaller sibling
/// may have a different token/tree topology, while a tied second no-split pass
/// can expose a better terminal tree fixed point. The completed source and all
/// sibling candidates remain retained regardless of whether this continuation
/// improves.
fn changed_narrow_parent_should_continue(
    exposes_new_parent: bool,
    narrow: &Candidate,
    source: CandidateInput<'_>,
) -> bool {
    exposes_new_parent && narrow.is_strictly_smaller_than_source(source)
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
        .map_err(|_| Error::internal("could not allocate compact route seed"))?;
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

/// Finish a compact source-max parent after ordinary timed routes complete.
///
/// Unlike the earlier deft4j-derived floor, this terminal dependency admits a
/// single parent block: splitting that block is how a second payload regime is
/// discovered. The separate 256-KiB decoded cap remains a small fixed memory
/// bound while covering compact token graphs whose long matches expand beyond
/// the early route's 128-KiB scheduling class.
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
/// fully emitted stream is strictly smaller. If the hard stop closes during a
/// block, its best completed tree is retained and later blocks reuse their
/// originals, preserving a complete stream prefix rather than losing the
/// whole terminal pass.
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

/// Finish the exact mixed-depth tree floor after the hard search boundary.
///
/// Payload/header search has a fixed Deflate-alphabet frontier per dynamic
/// block once frequencies are known. Price every block so reaching the hard
/// boundary a few milliseconds earlier or later cannot change which terminal
/// floor quiet, Verbose, or Visual mode receives. One-MiB compressed/decoded
/// limits and the existing 128-block narrow-list bound cap total work; no file
/// family or measured corpus band participates in admission.
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
/// default mode: either immediate plan can reach the smaller final stream. Max
/// mode runs only the floor here because its later source-max route covers the
/// remaining full search; repeating the sibling first would consume time
/// reserved for that broader route. The 4,000-token/80-KiB eligibility band is
/// enforced by `compact_proven_submatch_route_eligible` before this helper
/// is called.
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
    let mut output_max_distance = None;
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
        output_max_distance = Some(replayed.max_distance);

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
        output_max_distance = None;
    }

    // The last accepted replay is not necessarily followed by another loop
    // iteration: the replay cap or deadline may stop immediately after it.
    // Validate that final selectable stream as well. A known-losing generated
    // stream is skipped because the caller will retain the already-validated
    // source bytes instead.
    if (!initial_loses || options.strict) && !data_is_validated {
        let replayed = parse_validated_rewrite(&data, source.decoded_limit, source.identity)?;
        output_max_distance = Some(replayed.max_distance);
    }

    let block_report = capture_planned_block_report(&plans, reports_enabled(options));
    Ok(Candidate {
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
    })
}

/// Retain a parsed source when the mandatory plan list cannot be allocated.
/// Strict mode must rewrite incompatible Huffman alphabets, so it reports an
/// allocation failure rather than silently ignoring the requested transform.
fn source_candidate(source: CandidateInput<'_>, options: &Options) -> Result<Candidate> {
    if options.strict {
        return Err(Error::internal("could not allocate Deflate plan"));
    }
    let mut data = Vec::new();
    data.try_reserve_exact(source.compressed.len())
        .map_err(|_| Error::internal("could not allocate Deflate output"))?;
    data.extend_from_slice(source.compressed);
    Ok(Candidate {
        data,
        bits: source.meaningful_bits,
        output_max_distance: None,
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
mod tests;
