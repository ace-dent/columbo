// SPDX-License-Identifier: MIT
// SPDX-FileCopyrightText: 2026 Andrew C.E. Dent <https://github.com/ace-dent>

//! The terminal sweep on a complete candidate: original-match restoration
//! (R1), the terminal methods R1b–R11, the boundary slide (R1c) and the
//! finishing steps R13–R17. Max repeats the sweep while it improves (R12).
//! Also the final header fit (R2–R5 after R18), the relaxed distance code
//! (R19) and the parse cache that the methods share.

use crate::progress::Progress;
use crate::{Options, Result};

use super::{
    build_candidate_and_parse, build_candidate_from_plans, candidate_progress,
    parse_validated_rewrite, rewritten_input, Candidate, CandidateInput, DefaultFloorWork,
    HeaderFit, ReplayPlanner, StreamIdentity, MAX_TERMINAL_HEADER_MAX_BYTES,
    TERMINAL_HEADER_MAX_BLOCKS, TREE_FINISH_REPEATS,
};
use crate::deflate::block::{reusable_original_bits, stored_block_bits};
use crate::deflate::header::{
    plan_literal_span, plan_payload_header_tradeoff, plan_rle_smoothed_tree_candidate,
};
use crate::deflate::model::{
    DynamicPlan, ParsedBlock, ParsedStream, PlannedBlock, Representation, SourceBlockType, Token,
};
use crate::deflate::restore::{
    plan_original_match_restoration, MAX_RESTORATION_BLOCKS, MAX_RESTORATION_BYTES,
};
use crate::deflate::search::plan_terminal_token_replay;
use crate::deflate::stop::SearchStop;

/// Default's size limit for R1b–R5, also used by Max's mandatory Default floor.
const TERMINAL_HEADER_MAX_BYTES: usize = 128 * 1024;
/// Full-header prices per R1b, R2 or R3 invocation, shared across blocks.
const TERMINAL_HEADER_MAX_PRICES: usize = 1024;

/// R1: restore source-certified matches in a completed candidate. It stays out
/// of the tree floors because its terminal spelling must not redirect the
/// descendants of losing topologies.
pub(super) fn improve_with_original_match_restoration(
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
    let selected = parse_validated_rewrite(
        &candidate.data,
        original.decoded_limit,
        original.identity,
        &candidate.plans,
    )?;
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
/// R19 runs outside the sweep, on the candidate every route has finished.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum TerminalHeaderSearch {
    StrictDistanceCompletion,
    RelaxedDistanceCode,
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
    joint: crate::deflate::joint::JointBudget,
    symbols: crate::deflate::symbol_set::SymbolSetBudget,
    alphabet: crate::deflate::stream::AlphabetBudget,
    entropy_cuts: crate::deflate::stream::EntropyCutBudget,
    header_tree: crate::deflate::header::HeaderTreeBudget,
    rotations: crate::deflate::header::RotationBudget,
    coupled_swaps: crate::deflate::header::CoupledSwapBudget,
    response: crate::deflate::header::ResponseBudget,
}

impl TerminalHeaderSearch {
    fn name(self) -> &'static str {
        match self {
            Self::StrictDistanceCompletion => "Strict distance completion",
            Self::RelaxedDistanceCode => "Relaxed distance code",
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
        // R1c, R13–R17 and R19 are linear and parse one candidate, costing no
        // more than a replay round, so they have no size limit.
        if self.is_linear() {
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
            _ if self.is_linear() => usize::MAX,
            _ => TERMINAL_HEADER_MAX_BLOCKS,
        }
    }

    /// R1c, R13–R17 and R19: one pass over the blocks, with bounded work in
    /// each.
    fn is_linear(self) -> bool {
        self == Self::BoundarySlide || self == Self::RelaxedDistanceCode || self.is_tree_finish()
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
                return crate::deflate::stream::plan_alphabet_boundaries(
                    block,
                    alignment,
                    options,
                    &mut budget.alphabet,
                    stop,
                )
            }
            Self::HeaderResponse => crate::deflate::header::plan_header_response(
                block,
                options.strict,
                &mut budget.response,
                stop,
            )?,
            Self::LengthExchange => crate::deflate::header::plan_length_exchange(
                block,
                options.strict,
                &mut budget.response,
                stop,
            )?,
            Self::SymbolSets => crate::deflate::symbol_set::plan_symbol_sets(
                block,
                alignment,
                options,
                &mut budget.symbols,
                stop,
            )?,
            Self::DistanceBands => {
                crate::deflate::distance_band::plan_distance_bands(block, alignment, options, stop)?
            }
            Self::TokenReplay => plan_terminal_token_replay(block, alignment, options, stop)?,
            Self::EntropyCuts => {
                return crate::deflate::stream::plan_entropy_cuts(
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
                        crate::deflate::header::plan_strict_distance_completion(
                            block,
                            options.strict,
                            &mut budget.header_prices,
                            stop,
                        )
                    }
                    Self::RelaxedDistanceCode => {
                        crate::deflate::header::plan_relaxed_distance_code(
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
                        crate::deflate::joint::plan_joint_tree_rle(block, &mut budget.joint, stop)
                    }
                    Self::HeaderTree => crate::deflate::header::plan_header_tree(
                        block,
                        options.strict,
                        &mut budget.header_tree,
                        stop,
                    ),
                    Self::CodeLengthRotations => {
                        crate::deflate::header::plan_code_length_rotations(
                            block,
                            options.strict,
                            &mut budget.rotations,
                            stop,
                        )
                    }
                    Self::CoupledLengthSwaps => crate::deflate::header::plan_coupled_length_swaps(
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
pub(super) fn improve_with_terminal_searches(
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
pub(super) fn improve_with_terminal_header_search(
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
pub(super) fn improve_with_terminal_merges(
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
pub(super) fn improve_with_repeated_tree_finish(
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
pub(super) struct TerminalParseCache {
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
        plans: &[PlannedBlock],
    ) -> Result<TerminalParse<'_>> {
        let hit = matches!(&self.entry, Some((bytes, _)) if bytes.as_slice() == data);
        if hit {
            let (_, stream) = self.entry.as_ref().expect("cache hit has an entry");
            return Ok(TerminalParse::Cached(stream));
        }
        // Release a stale model before building its replacement.
        self.entry = None;
        let stream = parse_validated_rewrite(data, decoded_limit, identity, plans)?;
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
        joint: crate::deflate::joint::JointBudget::new(),
        symbols: crate::deflate::symbol_set::SymbolSetBudget::new(),
        alphabet: crate::deflate::stream::AlphabetBudget::new(),
        entropy_cuts: crate::deflate::stream::EntropyCutBudget::for_stream(blocks),
        header_tree: crate::deflate::header::HeaderTreeBudget::new(),
        rotations: crate::deflate::header::RotationBudget::new(),
        coupled_swaps: crate::deflate::header::CoupledSwapBudget::new(),
        response: crate::deflate::header::ResponseBudget::new(),
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

pub(super) fn refine_with_terminal_header_search_cached(
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
    let selected = parse_cache.parse(&candidate.data, decoded_limit, identity, &candidate.plans)?;
    // Parsing drops redundant empty blocks; these methods need the exact
    // layout.
    if selected.source_block_count != selected.blocks.len()
        || selected.blocks.len() > search.max_blocks()
    {
        return Ok(None);
    }
    let plans = match search {
        TerminalHeaderSearch::BoundarySlide => {
            crate::deflate::slide::plan_boundary_slide_in_stream(
                &selected.blocks,
                options,
                crate::deflate::slide::SlideStream {
                    decoded_size: identity.decoded_size,
                    crc32: identity.crc32,
                    adler32: identity.adler32,
                },
                stop,
            )
        }
        TerminalHeaderSearch::Merges => {
            crate::deflate::stream::plan_terminal_merges(&selected.blocks, options, stop)
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

/// R2–R5 after R18, for [`HeaderFit::AfterTreeFloors`]. R5 rewrites tokens,
/// which in every other order the tree finish and the token replay then see;
/// here, after an R5 win, R14 chooses trees for the new counts and R17 replays
/// the token floor under them, each sliding once more after a win.
pub(super) fn improve_with_final_header_fit(
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

/// R19: in relaxed mode, give each block that uses at most one distance
/// symbol the cheapest distance code its header allows. Returns the rewrite
/// only when it is strictly smaller.
///
/// It runs on a candidate that every route has finished, so it redirects no
/// search, and it is linear finalization that no deadline skips: Default and
/// Max then apply it alike, and Max can compare its result with Default's.
pub(super) fn refine_with_relaxed_distance_codes(
    candidate: &Candidate,
    options: &Options,
    decoded_limit: u64,
    identity: StreamIdentity,
) -> Result<Option<Candidate>> {
    if options.strict || !may_use_degenerate_distance_code(&candidate.plans) {
        return Ok(None);
    }
    refine_with_terminal_header_search_cached(
        TerminalHeaderSearch::RelaxedDistanceCode,
        candidate,
        options,
        decoded_limit,
        identity,
        &mut SearchStop::never(),
        &mut TerminalParseCache::default(),
    )
}

/// Whether some dynamic block may use fewer than two distance symbols, so
/// that R19 need not parse a candidate it cannot change. A candidate without
/// its plans has to be parsed to tell.
fn may_use_degenerate_distance_code(plans: &[PlannedBlock]) -> bool {
    plans.is_empty()
        || plans.iter().any(|plan| {
            let dynamic = match plan.representation {
                Representation::Dynamic(_) => true,
                Representation::Original(_) => plan.source_type == SourceBlockType::Dynamic,
                Representation::Stored | Representation::Fixed => false,
            };
            if !dynamic {
                return false;
            }
            let mut first = None;
            !plan.tokens.iter().any(|token| match *token {
                Token::Literal(_) => false,
                Token::Match {
                    distance_symbol, ..
                } => *first.get_or_insert(distance_symbol) != distance_symbol,
            })
        })
}

#[cfg(test)]
mod tests;
