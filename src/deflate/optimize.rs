// SPDX-License-Identifier: MIT
// SPDX-FileCopyrightText: 2026 Andrew C.E. Dent <https://github.com/ace-dent>

//! Raw-stream optimization orchestration and complete-candidate selection.
//!
//! Validate the source before scheduling optional routes, retain the required
//! Default comparison floor, and compare emitted candidates by bytes, then
//! meaningful bits. Block-boundary and token-spelling algorithms live in
//! `stream` and `search`. Route labels (D1, M0, R1, …) refer to
//! `docs/routes-and-methods.md`.

use std::cell::RefCell;
use std::thread;
use std::time::Instant;

use crate::progress::{
    reports_enabled, BlockEncoding, BlockProgress, BlockReport, CandidateProgress, RouteProgress,
    MAX_REPORTED_BLOCKS,
};
use crate::{Error, Options, Result};

use super::bitstream::BitWriter;
use super::block::emit_block;
use super::model::{
    ParsedBlock, ParsedStream, PlannedBlock, Representation, SourceBlockType, Token,
};
use super::parse::parse_stream;
use super::search::{
    plan_block_with_integrated_proven_search, rewrite_258_symbols, PROVEN_SUBMATCH_FULL_MATCH_LIMIT,
};
use super::stop::{timeout_grace, Deadline, RouteWindow, SearchStop};
use super::stream::{
    plan_fragmented_replay, plan_integrated_proven_source_route, plan_proven_submatch_route,
    plan_stream, plan_stream_from_established_floor, plan_stream_with_progress,
};

mod bounded;
mod compact_split;
mod default_floor;
mod gates;
mod proven_feedback;
mod schedule;
mod source_routes;
mod terminal;
mod tree_floor;

use bounded::{
    build_bounded_floor_candidate, build_bounded_generic_max_candidates,
    build_bounded_phase_candidates, established_floor_candidate, refine_bounded_deft4j_lineage,
    BoundedFollowUpCandidates, BoundedPhaseCandidates, BoundedRoutes,
};
use compact_split::{
    build_prepared_compact_source_split_floors, compact_split_parent_is_completed,
    prepare_compact_source_split_seed, refine_with_compact_source_split_floor,
    refine_with_compact_source_split_floor_until, refine_with_terminal_source_split_floor_until,
    CompactSplitSeed,
};
use default_floor::{
    build_apng_default_candidate, build_complete_apng_default_floor_candidate,
    build_complete_default_floor, build_complete_default_floor_candidate,
    improve_default_floor_with_feedback, CompleteDefaultFloor,
};
use gates::{
    bounded_png_max_policy, compact_balanced_tree_source_eligible,
    compact_dependent_deft4j_work_class, complete_png_parallel_source_max_work_class,
    continuation_source_max, default_source_topology_eligible, deft4j_source_route_eligible,
    floor_exposes_new_search_states, floor_seeded_priority_with_structural_sibling, gain_is_below,
    has_multiple_nonempty_blocks, independent_deft4j_refinement_can_overlap,
    narrow_source_route_eligible, parallel_route_is_bounded, prebuild_bounded_floor,
    source_run_match_count_exceeds, BoundedPngMaxPolicy, ContinuationSourceMax,
    WEAK_DEFT4J_GAIN_BASIS_POINTS,
};
use proven_feedback::{
    build_compact_proven_feedback_candidate, refine_with_compact_proven_feedback,
};
pub(crate) use schedule::optimize_raw_prefix_with_floor_and_grace;
use source_routes::{
    build_complementary_narrow_source_candidate, build_deft4j_source_candidate,
    build_source_family_candidate, build_source_max_candidate, candidate_exposes_new_parent,
    changed_narrow_parent_should_continue, refine_with_max_planner, refine_with_no_split_route,
};
use terminal::{improve_with_final_header_fit, improve_with_terminal_searches, TerminalParseCache};
use tree_floor::{improve_with_terminal_tree_floors, refine_with_compact_balanced_tree_floor};

/// One round groups adjacent source blocks; two more settle the new layout's
/// boundaries and tables.
const DEFAULT_RAW_REPLAY_LIMIT: usize = 3;
/// Extra R13–R15 passes after a winning pass when no later R12 sweep can
/// revisit them.
const TREE_FINISH_REPEATS: usize = 1;
/// Max's size limit for the terminal header methods; also bounds
/// [`TerminalShare::Search`].
const MAX_TERMINAL_HEADER_MAX_BYTES: usize = 1024 * 1024;
const TERMINAL_HEADER_MAX_BLOCKS: usize = 128;
/// Sentinel: replay until a round fails to improve, bounded by
/// [`resolved_replay_limit`].
const MAX_RAW_REPLAY_LIMIT: usize = usize::MAX;
const NARROW_SOURCE_LIST_MAX_BLOCKS: usize = 128;
/// From this many nonempty source blocks, the no-split route prunes
/// cumulatively first, so early blocks cannot use up the route window.
const CUMULATIVE_NO_SPLIT_MIN_SOURCE_BLOCKS: usize = 4;
/// With [`NARROW_SOURCE_LIST_MAX_BLOCKS`], keeps the no-split route's
/// candidates well inside [`PARALLEL_ROUTE_MAX_MODEL`](gates::PARALLEL_ROUTE_MAX_MODEL).
const NARROW_SOURCE_MAX_COMPRESSED: usize = 1_024 * 1_024;
/// Limits for the compact split floor (M10c).
const COMPACT_SPLIT_FLOOR_MAX_COMPRESSED: usize = 16 * 1024;
const COMPACT_SPLIT_FLOOR_MAX_DECODED: u64 = 128 * 1024;
const COMPACT_SPLIT_FLOOR_MAX_BLOCKS: usize = 4;
const COMPACT_SPLIT_FLOOR_MAX_TOKENS: usize = 16 * 1024;
/// Limits for the one-block compact balanced-tree floor (M4).
const COMPACT_TREE_MAX_COMPRESSED: usize = 8 * 1_024;
const COMPACT_TREE_MAX_DECODED: u64 = 128 * 1_024;
const COMPACT_TREE_MAX_TOKENS: usize = 4_096;
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
    /// Under Max's `SharedExact`, the stream Default returns once its whole
    /// route finishes, for callers that combine streams after optimization.
    pub(crate) default_endpoint: Option<DefaultEndpoint>,
}

/// A complete Default result, in the form Default returns it.
pub(crate) struct DefaultEndpoint {
    pub(crate) data: Vec<u8>,
    pub(crate) deflate_bits: u64,
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
    /// handed that candidate over, and a smaller one can close larger. The
    /// list runs oldest to newest. A losing contender's parents go to the
    /// oldest end: each is larger than a contender that lost here, so it is
    /// the least likely start of a shorter run's finish, and is finished only
    /// if time remains.
    fn replace_keeping_parent(&mut self, mut contender: Self, keep: bool) -> bool {
        if !keep {
            return self.replace_if_smaller(contender);
        }
        if !contender.is_strictly_smaller_than(self) {
            contender
                .replaced_parents
                .append(&mut self.replaced_parents);
            self.replaced_parents = contender.replaced_parents;
            return false;
        }
        let mut parent = std::mem::replace(self, contender);
        self.replaced_parents.append(&mut parent.replaced_parents);
        parent.plans = Vec::new();
        parent.block_report = None;
        self.replaced_parents.push(parent);
        true
    }

    /// Like [`Self::replace_keeping_parent`], but with `keep` (Max) a losing
    /// contender is kept as well, after its own parents: it lost to an
    /// incumbent that other routes reached in their own time, and a shorter
    /// run, which compares it with the incumbent those routes had reached by
    /// then, can let it win.
    fn replace_keeping_both(&mut self, mut contender: Self, keep: bool) -> bool {
        if !keep || contender.is_strictly_smaller_than(self) {
            return self.replace_keeping_parent(contender, keep);
        }
        let mut kept = std::mem::take(&mut contender.replaced_parents);
        contender.plans = Vec::new();
        contender.block_report = None;
        kept.push(contender);
        kept.append(&mut self.replaced_parents);
        self.replaced_parents = kept;
        false
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

/// [`replace_optional_if_smaller`], keeping the replaced candidate with `keep`
/// (Max) as [`Candidate::replace_keeping_parent`] does.
fn replace_optional_keeping_parent(
    incumbent: &mut Option<Candidate>,
    contender: Candidate,
    keep: bool,
) -> bool {
    match incumbent {
        Some(current) => current.replace_keeping_parent(contender, keep),
        None => {
            *incumbent = Some(contender);
            true
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
    let (mut candidate, validated) = build_candidate_and_parse(
        source,
        plans,
        options,
        replay_limit,
        ReplayPlanner::Full,
        expired,
        None,
        false,
    )?;
    if !run_proven_lineage || expired.reached() {
        return Ok(candidate);
    }

    // Run proven-submatch resegmentation as a separate lineage from the
    // endpoint. A local win inside `plan_stream` would change the tokens later
    // replays start from and can hide a better fixed point.
    let endpoint_proven = {
        // Original-bit references are invalid after an accepted replay, so
        // use the parse of these exact bytes that validated them, or make one.
        let replayed = match validated {
            Some(validated) => validated,
            None => {
                parse_validated_rewrite(&candidate.data, source.decoded_limit, source.identity)?
            }
        };
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
    let kept = KEPT_VALIDATED_PARSE
        .try_with(|kept| {
            let kept = kept.try_borrow().ok()?;
            let (bytes, limit, stream) = kept.as_ref()?;
            (*limit == decoded_limit && bytes.as_slice() == data)
                .then(|| stream.try_clone_shared())
                .flatten()
        })
        .ok()
        .flatten();
    if let Some(stream) = kept {
        validate_replayed_stream(&stream, data.len(), identity)?;
        return Ok(stream);
    }
    // Release a stale model before building its replacement.
    release_kept_validated_parse();
    let stream = parse_stream(data, decoded_limit)?;
    validate_replayed_stream(&stream, data.len(), identity)?;
    let _ = KEPT_VALIDATED_PARSE.try_with(|kept| {
        let Ok(mut kept) = kept.try_borrow_mut() else {
            return;
        };
        let mut bytes = Vec::new();
        if bytes.try_reserve_exact(data.len()).is_err() {
            return;
        }
        bytes.extend_from_slice(data);
        *kept = stream
            .try_clone_shared()
            .map(|shared| (bytes, decoded_limit, shared));
    });
    Ok(stream)
}

thread_local! {
    /// The last stream this thread parsed and validated, with its decoded
    /// limit. A method often reparses the bytes the previous step validated;
    /// parsing is a function of the bytes and the limit, so a copy sharing the
    /// kept blocks' buffers is the parse it would make.
    static KEPT_VALIDATED_PARSE: RefCell<Option<(Vec<u8>, u64, ParsedStream)>> =
        const { RefCell::new(None) };
}

/// Drop this thread's kept parse; each stream releases it when it ends.
pub(crate) fn release_kept_validated_parse() {
    let _ = KEPT_VALIDATED_PARSE.try_with(|kept| {
        if let Ok(mut kept) = kept.try_borrow_mut() {
            *kept = None;
        }
    });
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
pub(crate) mod test_support;
#[cfg(test)]
mod tests;
