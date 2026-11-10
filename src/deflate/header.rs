// SPDX-License-Identifier: MIT
// SPDX-FileCopyrightText: 2026 Andrew C.E. Dent <https://github.com/ace-dent>

//! Dynamic-header construction and exact bit accounting.

use std::cell::RefCell;

use super::huffman::{
    code_length_tree_shape_is_valid, huffman_code_lengths_are_valid,
    make_lengths_deflopt_heap_into_with_scratch, payload_tree_shape_is_valid, TreeBuildScratch,
    FIXED_DISTANCE_CODE_LENGTHS, FIXED_LITERAL_CODE_LENGTHS,
};
use super::model::{
    count_frequencies, token_extra_bits, DynamicPlan, RleToken, Token, CODE_LENGTH_ORDER,
    MAX_DYNAMIC_CODE_LENGTH_COUNT, RFC_DISTANCE_CODE_COUNT,
};
use super::stop::SearchStop;

mod balanced_tree;
mod code_length_tree;
mod coupled;
mod deft4j;
mod length_adjustments;
mod payload_trees;
mod plan_cache;
mod response;
mod rle_rewrite;
mod rle_seeds;
mod rotate;
mod shortest_rle;
mod swap_costs;
mod terminal;
mod tree;
pub(crate) use balanced_tree::{
    balanced_tree_opportunities, plan_columbo_balanced_tree_candidate, BalancedTreeOpportunities,
};
use code_length_tree::{
    code_length_tree, CodeLengthBuilder, CodeLengthTree, CodeLengthTrees, CODE_LENGTH_TREES,
};
pub(crate) use coupled::{plan_coupled_length_swaps, CoupledSwapBudget};
pub(crate) use deft4j::{plan_for_deft4j_lengths_with_cost, Deft4jHeaderPolicy};
use length_adjustments::{arrange_equal_frequency_lengths, improve_by_length_swaps};
use payload_trees::{
    columbo_rle_tree_candidate, exact_tree_candidate, tree_candidates, zopfli_rle_tree_candidate,
    ExactTreeBuilder,
};
pub(crate) use payload_trees::{
    plan_bounded_depth_tree_candidate, plan_rle_smoothed_tree_candidate,
};
use plan_cache::{
    header_plan_fingerprint, stored_header_plan_kernel, use_exhaustive_header_plan_budget,
    HeaderPlanStore, HEADER_PLAN_STORE,
};
pub(crate) use plan_cache::{HeaderPlanCache, PackedDynamicPlan};
pub(crate) use response::{plan_header_response, plan_length_exchange, ResponseBudget};
use rle_rewrite::{
    consider_columbo_deflopt_local_rewrite, consider_deft4j_pruned_header,
    rewrite_rle_deflopt_local_into, LocalRewriteSteps,
};
use rle_seeds::{greedy_rle, rle_seed_candidates_from_runs_into};
pub(crate) use rotate::{plan_code_length_rotations, RotationBudget};
pub(super) use shortest_rle::shortest_rle;
use shortest_rle::{shortest_rle_from_runs_into, ShortestRleScratch};
use swap_costs::release_swap_trials;
pub(crate) use terminal::{
    plan_literal_span, plan_payload_header_tradeoff, plan_strict_distance_completion,
};
pub(crate) use tree::{plan_header_tree, HeaderTreeBudget};

const INF: u64 = u64::MAX / 4;
const REDUCED_PAYLOAD_TREE_DEPTHS: [u8; 2] = [10, 9];

/// Drop this thread's stored header plans, code-length trees, run spellings
/// and swap-trial costs. A top-level optimization calls this on return, so
/// library callers keep no plans between inputs; worker threads free theirs
/// when they exit.
pub(crate) fn release_thread_header_plans() {
    let _ = HEADER_PLAN_SEARCH.try_with(|search| {
        if let Ok(mut search) = search.try_borrow_mut() {
            *search = None;
        }
    });
    let _ = HEADER_PLAN_STORE.try_with(|store| {
        if let Ok(mut store) = store.try_borrow_mut() {
            *store = HeaderPlanStore::default();
        }
    });
    let _ = CODE_LENGTH_TREES.try_with(|cache| {
        if let Ok(mut cache) = cache.try_borrow_mut() {
            *cache = None;
        }
    });
    release_swap_trials();
}

pub(crate) fn token_bits(
    tokens: &[Token],
    literal_lengths: &[u8],
    distance_lengths: &[u8],
) -> Option<u64> {
    let mut bits = 0_u64;
    for token in tokens {
        match *token {
            Token::Literal(value) => {
                let literal = *literal_lengths.get(usize::from(value))?;
                if literal == 0 {
                    return None;
                }
                bits = bits.checked_add(u64::from(literal))?;
            }
            Token::Match {
                length_symbol,
                distance_symbol,
                length_extra_bits,
                distance_extra_bits,
                ..
            } => {
                let literal = *literal_lengths.get(usize::from(length_symbol))?;
                let distance = *distance_lengths.get(usize::from(distance_symbol))?;
                if literal == 0 || distance == 0 {
                    return None;
                }
                let token_bits = u64::from(literal)
                    + u64::from(distance)
                    + u64::from(length_extra_bits)
                    + u64::from(distance_extra_bits);
                bits = bits.checked_add(token_bits)?;
            }
        }
    }
    let end = *literal_lengths.get(256)?;
    if end == 0 {
        return None;
    }
    bits.checked_add(u64::from(end))
}

// Out of line: each call scans both alphabets.
#[inline(never)]
pub(crate) fn token_bits_from_frequencies(
    literal_frequencies: &[u32; 286],
    distance_frequencies: &[u32; 30],
    literal_lengths: &[u8],
    distance_lengths: &[u8],
    extra_bits: u64,
) -> Option<u64> {
    let literal_bits = alphabet_payload_bits(literal_frequencies, literal_lengths)?;
    let distance_bits = alphabet_payload_bits(distance_frequencies, distance_lengths)?;
    extra_bits
        .checked_add(literal_bits)?
        .checked_add(distance_bits)
}

// Out of line: one vectorized copy serves every alphabet and caller.
#[inline(never)]
fn alphabet_payload_bits(frequencies: &[u32], lengths: &[u8]) -> Option<u64> {
    // A used symbol needs a code: one past `lengths`, or of length 0, has none.
    let (coded, past) = frequencies.split_at(lengths.len().min(frequencies.len()));
    if past.iter().any(|&frequency| frequency != 0) {
        return None;
    }
    // At most 286 counts below 2^32 times lengths below 2^8 cannot wrap a
    // u64, so the loop needs no overflow checks or branches and vectorizes.
    let mut bits = 0_u64;
    let mut uncoded = false;
    for (&frequency, &length) in coded.iter().zip(lengths) {
        bits = bits.wrapping_add(u64::from(frequency).wrapping_mul(u64::from(length)));
        uncoded |= frequency != 0 && length == 0;
    }
    (!uncoded).then_some(bits)
}

pub(crate) fn score_existing_dynamic(
    tokens: &[Token],
    source: &DynamicPlan,
    strict: bool,
) -> Option<DynamicPlan> {
    score_existing_dynamic_with(source, strict, |literal_lengths, distance_lengths| {
        token_bits(tokens, literal_lengths, distance_lengths)
    })
}

/// [`score_existing_dynamic`] for the tokens these symbol counts and extra
/// bits describe, without visiting the tokens.
fn score_existing_dynamic_from_frequencies(
    literal_frequencies: &[u32; 286],
    distance_frequencies: &[u32; 30],
    extra_bits: u64,
    source: &DynamicPlan,
    strict: bool,
) -> Option<DynamicPlan> {
    score_existing_dynamic_with(source, strict, |literal_lengths, distance_lengths| {
        token_bits_from_frequencies(
            literal_frequencies,
            distance_frequencies,
            literal_lengths,
            distance_lengths,
            extra_bits,
        )
    })
}

fn score_existing_dynamic_with(
    source: &DynamicPlan,
    strict: bool,
    data_bits: impl FnOnce(&[u8], &[u8]) -> Option<u64>,
) -> Option<DynamicPlan> {
    if strict && !source.has_strictly_compatible_huffman_codes() {
        return None;
    }
    if source.hlit < 257
        || source.hlit > 286
        || source.hdist == 0
        || source.hdist > RFC_DISTANCE_CODE_COUNT
        || source.hclen < 4
        || source.hclen > 19
        || source.literal_lengths.len() != source.hlit
        || source.distance_lengths.len() != source.hdist
        || !huffman_code_lengths_are_valid(&source.literal_lengths)
        || !huffman_code_lengths_are_valid(&source.distance_lengths)
        || !huffman_code_lengths_are_valid(&source.code_length_lengths)
        || !code_length_tree_shape_is_valid(&source.code_length_lengths)
        || !payload_tree_shape_is_valid(&source.literal_lengths, false)
        || !payload_tree_shape_is_valid(&source.distance_lengths, true)
    {
        return None;
    }
    let mut plan = source.clone();
    let data_bits = data_bits(&plan.literal_lengths, &plan.distance_lengths)?;
    plan.bits = dynamic_bits(data_bits, &plan)?;
    Some(plan)
}

/// `extra_bits` is the [`token_extra_bits`] total of `tokens`, which the
/// caller has usually summed already.
#[allow(clippy::too_many_arguments)]
pub(crate) fn best_dynamic_plan_cached(
    tokens: &[Token],
    literal_frequencies: &[u32; 286],
    distance_frequencies: &[u32; 30],
    extra_bits: u64,
    original: Option<&DynamicPlan>,
    strict: bool,
    exhaustive: bool,
    stop: &mut SearchStop<'_>,
    header_cache: &mut HeaderPlanCache,
) -> Option<DynamicPlan> {
    debug_assert_eq!(extra_bits, token_extra_bits(tokens));
    // The source header is priced from the counts; an all-literal caller
    // passes counts without tokens, and no source header.
    debug_assert!(
        original.is_none()
            || (*literal_frequencies, *distance_frequencies) == count_frequencies(tokens)
    );
    let source_exact = original.and_then(|plan| {
        score_existing_dynamic_from_frequencies(
            literal_frequencies,
            distance_frequencies,
            extra_bits,
            plan,
            strict,
        )
    });
    let source_is_compatible = source_exact.is_some();
    let mut best = source_exact;
    if exhaustive {
        use_exhaustive_header_plan_budget();
    }

    // Also repack the source code lengths (`score_existing_dynamic` keeps the
    // source RLE); DeflOpt tries both.
    if let Some(source) = original {
        if source_is_compatible {
            let data_bits = token_bits_from_frequencies(
                literal_frequencies,
                distance_frequencies,
                &source.literal_lengths,
                &source.distance_lengths,
                extra_bits,
            );
            if let Some(data_bits) = data_bits {
                if let Some(repacked) = plan_for_explicit_lengths_with_cost_cached(
                    &source.literal_lengths,
                    &source.distance_lengths,
                    data_bits,
                    exhaustive,
                    header_cache,
                ) {
                    keep_better(&mut best, repacked);
                }
            }
        }
    }

    let mut build_literal_frequencies = *literal_frequencies;
    ensure_code_symbols(&mut build_literal_frequencies, strict);
    let mut literal_candidates = tree_candidates(&build_literal_frequencies, 15, exhaustive);
    let mut build_distance_frequencies = *distance_frequencies;
    ensure_distance_symbols(&mut build_distance_frequencies, strict);
    let mut distance_candidates = tree_candidates(&build_distance_frequencies, 15, exhaustive);

    literal_candidates.retain(|lengths| {
        lengths.get(256).copied().unwrap_or(0) != 0 && payload_tree_shape_is_valid(lengths, false)
    });
    distance_candidates.retain(|lengths| {
        payload_tree_shape_is_valid(lengths, true)
            && distance_frequencies
                .iter()
                .enumerate()
                .all(|(symbol, &frequency)| frequency == 0 || lengths[symbol] != 0)
    });
    // Keep insertion order, not payload order: a slightly dearer tree can
    // have a much smaller header.
    literal_candidates.truncate(20);
    distance_candidates.truncate(20);

    'outer: for literal in &literal_candidates {
        for distance in &distance_candidates {
            if stop.reached() {
                break 'outer;
            }
            let data_bits = token_bits_from_frequencies(
                literal_frequencies,
                distance_frequencies,
                literal,
                distance,
                extra_bits,
            );
            if let Some(data_bits) = data_bits {
                if let Some(candidate) = plan_for_explicit_lengths_with_cost_cached(
                    literal,
                    distance,
                    data_bits,
                    exhaustive,
                    header_cache,
                ) {
                    keep_better(&mut best, candidate);
                }
            }
        }
    }

    // A depth-limited tree can have a much cheaper header than a
    // payload-optimal tree with very long rare codes (as in ECT).
    if exhaustive {
        let mut trees = None;
        for max_bits in REDUCED_PAYLOAD_TREE_DEPTHS {
            if stop.reached() {
                break;
            }
            let trees = trees.get_or_insert_with(|| {
                ExactTreeBuilder::new(&build_literal_frequencies, &build_distance_frequencies)
            });
            if let Some(candidate) = exact_tree_candidate(
                literal_frequencies,
                distance_frequencies,
                trees,
                extra_bits,
                max_bits,
                true,
                header_cache,
            ) {
                keep_better(&mut best, candidate);
            }
        }
    }

    if exhaustive && !stop.reached() {
        if let Some(candidate) = columbo_rle_tree_candidate(
            literal_frequencies,
            distance_frequencies,
            extra_bits,
            strict,
            exhaustive,
        ) {
            keep_better(&mut best, candidate);
        }
    }

    if exhaustive && !stop.reached() {
        if let Some(candidate) = zopfli_rle_tree_candidate(
            literal_frequencies,
            distance_frequencies,
            extra_bits,
            strict,
            exhaustive,
        ) {
            keep_better(&mut best, candidate);
        }
    }

    // Equal-frequency symbols can exchange code lengths without changing the
    // payload cost or code validity, but the exchange changes the header.
    if exhaustive && !stop.reached() {
        if let Some(seed) = best.clone() {
            let seed_literal = pad_lengths::<286>(&seed.literal_lengths);
            let seed_distance = pad_lengths::<30>(&seed.distance_lengths);
            for descending in [false, true] {
                // First rearrange the literal/length code lengths alone, with
                // the distance lengths fixed: rearranging both alphabets can
                // hide a literal-side header win.
                let mut literal = seed_literal;
                arrange_equal_frequency_lengths(literal_frequencies, &mut literal, descending);
                if literal != seed_literal && !stop.reached() {
                    if let Some(candidate) =
                        plan_for_explicit_lengths(tokens, &literal, &seed_distance, exhaustive)
                    {
                        keep_better(&mut best, candidate);
                    }
                }

                let mut distance = seed_distance;
                arrange_equal_frequency_lengths(distance_frequencies, &mut distance, descending);
                if (literal != seed_literal || distance != seed_distance) && !stop.reached() {
                    if let Some(candidate) =
                        plan_for_explicit_lengths(tokens, &literal, &distance, exhaustive)
                    {
                        keep_better(&mut best, candidate);
                    }
                }
            }
        }
    }

    // Greedy length swaps help most on small, literal-heavy blocks.
    if exhaustive && tokens.len() <= 700 && !stop.reached() {
        if let Some(seed) = best.clone() {
            improve_by_length_swaps(
                tokens,
                literal_frequencies,
                distance_frequencies,
                &seed,
                exhaustive,
                stop,
                &mut best,
            );
        }
    }
    best
}

fn pad_lengths<const N: usize>(lengths: &[u8]) -> [u8; N] {
    let mut padded = [0_u8; N];
    let count = lengths.len().min(N);
    padded[..count].copy_from_slice(&lengths[..count]);
    padded
}

// Out of line: each call scans an alphabet.
#[inline(never)]
fn ensure_code_symbols(frequencies: &mut [u32], strict: bool) {
    if !strict {
        return;
    }
    let mut used = frequencies
        .iter()
        .enumerate()
        .filter_map(|(symbol, &frequency)| (frequency != 0).then_some(symbol));
    match (used.next(), used.next()) {
        (None, _) => {
            frequencies[0] = 1;
            frequencies[1] = 1;
        }
        (Some(only), None) => frequencies[usize::from(only == 0)] = 1,
        (Some(_), Some(_)) => {}
    }
}

fn ensure_distance_symbols(frequencies: &mut [u32; 30], strict: bool) {
    ensure_code_symbols(frequencies, strict);
}

/// The heap variant whose sift-downs both break ties by frequency alone, so
/// its DeflOpt and order-key builders return the same tree.
const FREQUENCY_ONLY_HEAP_VARIANT: u32 = 3;

// Out of line: the scan of existing candidates outweighs a call.
#[inline(never)]
fn push_unique<T: PartialEq>(candidates: &mut Vec<T>, candidate: T) {
    if !candidates.iter().any(|current| current == &candidate) {
        candidates.push(candidate);
    }
}

/// [`push_unique`] for code-length trees, by their lengths: equal lengths for
/// one histogram are one candidate.
fn push_unique_tree(candidates: &mut Vec<CodeLengthTree>, candidate: CodeLengthTree) {
    if !candidates
        .iter()
        .any(|tree| tree.lengths == candidate.lengths)
    {
        candidates.push(candidate);
    }
}

fn trim_literal(lengths: &[u8]) -> usize {
    trim_to_last_nonzero(lengths, 257)
}

fn trim_distance(lengths: &[u8]) -> usize {
    trim_to_last_nonzero(lengths, 1)
}

// Out of line: every header plan trims both alphabets, from many places.
#[inline(never)]
fn trim_to_last_nonzero(lengths: &[u8], minimum: usize) -> usize {
    lengths
        .iter()
        .enumerate()
        .skip(minimum)
        .rfind(|(_, length)| **length != 0)
        .map_or(minimum, |(index, _)| index + 1)
}

/// Cheap estimate of one block's size, used to rank or prune candidates. It
/// never decides acceptance: selected candidates are replanned with exact
/// pricing.
pub(crate) fn estimate_boundary_block_bits(
    literal_frequencies: &[u32; 286],
    distance_frequencies: &[u32; 30],
    extra_bits: u64,
    strict: bool,
) -> Option<u64> {
    let mut build_literal_frequencies = *literal_frequencies;
    ensure_code_symbols(&mut build_literal_frequencies, strict);
    let mut build_distance_frequencies = *distance_frequencies;
    if build_distance_frequencies
        .iter()
        .all(|&frequency| frequency == 0)
    {
        build_distance_frequencies[0] = 1;
    }
    ensure_distance_symbols(&mut build_distance_frequencies, strict);

    let mut literal_lengths = [0_u8; 286];
    let mut distance_lengths = [0_u8; 30];
    let mut heap_scratch = TreeBuildScratch::default();
    make_lengths_deflopt_heap_into_with_scratch(
        &build_literal_frequencies,
        &mut literal_lengths,
        15,
        0,
        false,
        &mut heap_scratch,
    );
    make_lengths_deflopt_heap_into_with_scratch(
        &build_distance_frequencies,
        &mut distance_lengths,
        15,
        0,
        false,
        &mut heap_scratch,
    );
    let data_bits = token_bits_from_frequencies(
        literal_frequencies,
        distance_frequencies,
        &literal_lengths,
        &distance_lengths,
        extra_bits,
    )?;
    let dynamic_bits = greedy_dynamic_header_bits(
        &literal_lengths,
        &distance_lengths,
        data_bits,
        &mut heap_scratch,
    )?;

    let fixed_bits = token_bits_from_frequencies(
        literal_frequencies,
        distance_frequencies,
        &FIXED_LITERAL_CODE_LENGTHS,
        &FIXED_DISTANCE_CODE_LENGTHS,
        extra_bits,
    )?
    .checked_add(3)?;

    Some(dynamic_bits.min(fixed_bits))
}

fn greedy_dynamic_header_bits(
    literal_lengths: &[u8],
    distance_lengths: &[u8],
    data_bits: u64,
    heap_scratch: &mut TreeBuildScratch,
) -> Option<u64> {
    let hlit = trim_literal(literal_lengths);
    let hdist = trim_distance(distance_lengths);
    let concatenated_len = hlit.checked_add(hdist)?;
    let mut concatenated = [0_u8; MAX_DYNAMIC_CODE_LENGTH_COUNT];
    concatenated
        .get_mut(..hlit)?
        .copy_from_slice(&literal_lengths[..hlit]);
    concatenated
        .get_mut(hlit..concatenated_len)?
        .copy_from_slice(&distance_lengths[..hdist]);

    let rle = greedy_rle(&concatenated[..concatenated_len], false, false, false);
    let code_length_frequencies = rle_frequencies(&rle);
    let code_length_lengths = code_length_tree(
        CodeLengthBuilder::DefloptHeap(0),
        &code_length_frequencies,
        heap_scratch,
    );
    huffman_code_lengths_are_valid(&code_length_lengths).then_some(())?;
    if !code_length_tree_shape_is_valid(&code_length_lengths) {
        return None;
    }

    let hclen = trim_code_lengths(&code_length_lengths);
    let mut bits = 3_u64
        .checked_add(5)?
        .checked_add(5)?
        .checked_add(4)?
        .checked_add(u64::try_from(hclen).ok()?.checked_mul(3)?)?;
    for token in rle {
        let code_bits = *code_length_lengths.get(usize::from(token.symbol))?;
        if code_bits == 0 {
            return None;
        }
        bits = bits
            .checked_add(u64::from(code_bits))?
            .checked_add(rle_extra_bits(token.symbol))?;
    }
    bits.checked_add(data_bits)
}

/// Plan a dynamic block for given literal/length and distance code lengths,
/// without changing the tokens.
// Out of line: inlined, each caller also took a copy of the masked planner.
#[inline(never)]
pub(crate) fn plan_for_explicit_lengths(
    tokens: &[Token],
    literal_lengths: &[u8],
    distance_lengths: &[u8],
    exhaustive: bool,
) -> Option<DynamicPlan> {
    plan_for_explicit_lengths_masked(tokens, literal_lengths, distance_lengths, exhaustive, 0xff)
}

fn plan_for_explicit_lengths_masked(
    tokens: &[Token],
    literal_lengths: &[u8],
    distance_lengths: &[u8],
    exhaustive: bool,
    rle_mask: u8,
) -> Option<DynamicPlan> {
    let hlit = trim_literal(literal_lengths);
    let hdist = trim_distance(distance_lengths);
    let literal_lengths = &literal_lengths[..hlit];
    let distance_lengths = &distance_lengths[..hdist];
    let data_bits = token_bits(tokens, literal_lengths, distance_lengths)?;

    plan_for_trimmed_lengths(
        literal_lengths,
        distance_lengths,
        data_bits,
        exhaustive,
        rle_mask,
    )
}

// Out of line: each call plans a whole header.
#[inline(never)]
pub(crate) fn plan_for_explicit_lengths_with_cost(
    literal_lengths: &[u8],
    distance_lengths: &[u8],
    data_bits: u64,
    exhaustive: bool,
) -> Option<DynamicPlan> {
    let hlit = trim_literal(literal_lengths);
    let hdist = trim_distance(distance_lengths);
    plan_for_trimmed_lengths(
        &literal_lengths[..hlit],
        &distance_lengths[..hdist],
        data_bits,
        exhaustive,
        0xff,
    )
}

fn plan_for_explicit_lengths_with_cost_cached(
    literal_lengths: &[u8],
    distance_lengths: &[u8],
    data_bits: u64,
    exhaustive: bool,
    cache: &mut HeaderPlanCache,
) -> Option<DynamicPlan> {
    let hlit = trim_literal(literal_lengths);
    let hdist = trim_distance(distance_lengths);
    let literal_lengths = &literal_lengths[..hlit];
    let distance_lengths = &distance_lengths[..hdist];
    if !payload_tree_shape_is_valid(literal_lengths, false)
        || !payload_tree_shape_is_valid(distance_lengths, true)
    {
        return None;
    }
    cache.price(
        literal_lengths,
        distance_lengths,
        data_bits,
        exhaustive,
        0xff,
    )
}

// Out of line: each call plans a whole header.
#[inline(never)]
fn plan_for_trimmed_lengths(
    literal_lengths: &[u8],
    distance_lengths: &[u8],
    data_bits: u64,
    exhaustive: bool,
    rle_mask: u8,
) -> Option<DynamicPlan> {
    let fingerprint =
        header_plan_fingerprint(literal_lengths, distance_lengths, exhaustive, rle_mask);
    let mut plan = stored_header_plan_kernel(
        fingerprint,
        literal_lengths,
        distance_lengths,
        exhaustive,
        rle_mask,
    )?;
    plan.bits = plan.bits.checked_add(data_bits)?;
    Some(plan)
}

/// An RLE and its symbol frequencies.
type CountedRle = (Vec<RleToken>, [u32; 19]);

#[derive(Default)]
struct HeaderPlanSearch {
    heap_scratch: TreeBuildScratch,
    best: Option<DynamicPlan>,
    /// This plan's runs of equal code lengths, which every RLE spells.
    runs: Vec<LengthRun>,
    /// Shortest RLEs of this plan's code lengths, by code-length code: RLE
    /// seeds often reach the same feedback tree. Only the first
    /// `shortest_rle_count` are this plan's; the rest keep their buffers for
    /// later plans.
    shortest_rles: Vec<ShortestRleEntry>,
    shortest_rle_count: usize,
    shortest_rle_scratch: ShortestRleScratch,
    /// Buffers for a feedback pass's rewrites, which are priced and mostly
    /// discarded: two for deft4j's pruned header, the first also for the
    /// DeflOpt rewrite that follows it.
    rewrites: [Vec<RleToken>; 2],
    /// Each seed's local rewrite steps and candidate code-length codes, kept
    /// for the next seed's.
    local_rewrite_steps: LocalRewriteSteps,
    code_length_candidates: Vec<CodeLengthTree>,
    /// RLE buffers a finished seed returns, for the next plan's seeds.
    spare_rles: Vec<Vec<RleToken>>,
    /// This plan's RLE seeds; the buffer stays for the next plan's.
    seeds: Vec<CountedRle>,
}

/// A code-length code's shortest RLE in a [`HeaderPlanSearch`].
struct ShortestRleEntry {
    tree: [u8; 19],
    rle: Vec<RleToken>,
    /// The RLE's symbol frequencies, or `None` when there is no RLE.
    frequencies: Option<[u32; 19]>,
    /// The earliest feedback pass a seed of this plan has started from this
    /// RLE, or `usize::MAX`.
    first_pass: usize,
}

impl HeaderPlanSearch {
    /// The shortest RLE under `tree`, its symbol frequencies, its entry and
    /// the entry's [`ShortestRleEntry::first_pass`].
    fn shortest_rle(
        &mut self,
        length_count: usize,
        tree: &[u8; 19],
    ) -> Option<(&[RleToken], &[u32; 19], usize, usize)> {
        let found = self.shortest_rles[..self.shortest_rle_count]
            .iter()
            .position(|entry| entry.tree == *tree);
        let index = match found {
            Some(index) => index,
            None => {
                if self.shortest_rle_count == self.shortest_rles.len() {
                    self.shortest_rles.push(ShortestRleEntry {
                        tree: [0; 19],
                        rle: Vec::new(),
                        frequencies: None,
                        first_pass: usize::MAX,
                    });
                }
                let entry = &mut self.shortest_rles[self.shortest_rle_count];
                entry.tree = *tree;
                entry.first_pass = usize::MAX;
                entry.frequencies = shortest_rle_from_runs_into(
                    &self.runs,
                    length_count,
                    tree,
                    &mut self.shortest_rle_scratch,
                    &mut entry.rle,
                );
                self.shortest_rle_count += 1;
                self.shortest_rle_count - 1
            }
        };
        let entry = &self.shortest_rles[index];
        entry
            .frequencies
            .as_ref()
            .map(|frequencies| (entry.rle.as_slice(), frequencies, index, entry.first_pass))
    }
}

/// Plan a header for the code lengths exactly as given, trailing zeros
/// included, so HLIT and HDIST stay fixed (R4).
pub(crate) fn plan_for_advertised_lengths(
    literal_lengths: &[u8],
    distance_lengths: &[u8],
    data_bits: u64,
) -> Option<DynamicPlan> {
    if !(257..=286).contains(&literal_lengths.len()) || !(1..=32).contains(&distance_lengths.len())
    {
        return None;
    }
    plan_for_trimmed_lengths_uncached(literal_lengths, distance_lengths, data_bits, true, 0xff)
}

fn plan_for_trimmed_lengths_uncached(
    literal_lengths: &[u8],
    distance_lengths: &[u8],
    data_bits: u64,
    exhaustive: bool,
    rle_mask: u8,
) -> Option<DynamicPlan> {
    if !payload_tree_shape_is_valid(literal_lengths, false)
        || !payload_tree_shape_is_valid(distance_lengths, true)
    {
        return None;
    }
    plan_for_valid_trimmed_lengths_uncached(
        literal_lengths,
        distance_lengths,
        data_bits,
        exhaustive,
        rle_mask,
    )
}

/// [`plan_for_trimmed_lengths_uncached`] for payload trees whose shapes the
/// caller has checked.
fn plan_for_valid_trimmed_lengths_uncached(
    literal_lengths: &[u8],
    distance_lengths: &[u8],
    data_bits: u64,
    exhaustive: bool,
    rle_mask: u8,
) -> Option<DynamicPlan> {
    debug_assert!(
        payload_tree_shape_is_valid(literal_lengths, false)
            && payload_tree_shape_is_valid(distance_lengths, true)
    );
    let decoded_length_count = literal_lengths.len().checked_add(distance_lengths.len())?;
    if decoded_length_count > MAX_DYNAMIC_CODE_LENGTH_COUNT {
        return None;
    }
    let mut decoded_length_storage = [0_u8; MAX_DYNAMIC_CODE_LENGTH_COUNT];
    decoded_length_storage[..literal_lengths.len()].copy_from_slice(literal_lengths);
    decoded_length_storage[literal_lengths.len()..decoded_length_count]
        .copy_from_slice(distance_lengths);
    let decoded_lengths = &decoded_length_storage[..decoded_length_count];
    let plan = |search: &mut HeaderPlanSearch| {
        plan_with_search(
            literal_lengths,
            distance_lengths,
            decoded_lengths,
            data_bits,
            exhaustive,
            rle_mask,
            search,
        )
    };
    // This thread's buffers, reused between plans where they lie, not moved
    // in and out; a nested plan, which none makes, would take fresh ones.
    HEADER_PLAN_SEARCH
        .try_with(|cell| match cell.try_borrow_mut() {
            Ok(mut search) => plan(search.get_or_insert_with(HeaderPlanSearch::default)),
            Err(_) => plan(&mut HeaderPlanSearch::default()),
        })
        .unwrap_or_else(|_| plan(&mut HeaderPlanSearch::default()))
}

/// [`plan_for_valid_trimmed_lengths_uncached`] in `search`'s buffers.
fn plan_with_search(
    literal_lengths: &[u8],
    distance_lengths: &[u8],
    decoded_lengths: &[u8],
    data_bits: u64,
    exhaustive: bool,
    rle_mask: u8,
    search: &mut HeaderPlanSearch,
) -> Option<DynamicPlan> {
    let decoded_length_count = decoded_lengths.len();
    // The best plan and the shortest RLEs belong to one plan's code lengths.
    search.best = None;
    search.shortest_rle_count = 0;
    // Every RLE spells the same runs, so find them once.
    length_runs_into(decoded_lengths, &mut search.runs);
    let mut seeds = std::mem::take(&mut search.seeds);
    rle_seed_candidates_from_runs_into(
        &search.runs,
        decoded_length_count,
        rle_mask,
        &mut search.spare_rles,
        &mut seeds,
    );
    for seed in seeds.drain(..) {
        consider_rle(
            data_bits,
            literal_lengths,
            distance_lengths,
            decoded_lengths,
            seed,
            exhaustive,
            search,
        );
    }
    search.seeds = seeds;
    search.best.take()
}

thread_local! {
    /// The uncached header planner's buffers on this thread, between plans.
    static HEADER_PLAN_SEARCH: RefCell<Option<HeaderPlanSearch>> = const { RefCell::new(None) };
}

/// Count the run transitions removed by writing `values` at `positions`, which
/// must be strictly increasing. Each affected edge is visited once, including
/// the edge between the literal/length and distance code lengths.
fn transitions_removed<const N: usize>(
    lengths: &[u8],
    positions: [usize; N],
    values: [u8; N],
) -> i64 {
    debug_assert!(positions.windows(2).all(|pair| pair[0] < pair[1]));
    let mut removed = 0;
    for (i, &position) in positions.iter().enumerate() {
        let before = lengths[position];
        let after = values[i];
        // An adjacent earlier replacement already visited this left edge.
        if position > 0 && (i == 0 || positions[i - 1] != position - 1) {
            let left = lengths[position - 1];
            removed += i64::from(left != before) - i64::from(left != after);
        }
        if position + 1 < lengths.len() {
            let right = lengths[position + 1];
            let changed_right = if i + 1 < N && positions[i + 1] == position + 1 {
                values[i + 1]
            } else {
                right
            };
            removed += i64::from(before != right) - i64::from(after != changed_right);
        }
    }
    removed
}

fn length_swap_delta(frequency_a: u32, frequency_b: u32, length_a: u8, length_b: u8) -> i64 {
    let frequency_a = i64::from(frequency_a);
    let frequency_b = i64::from(frequency_b);
    let length_a = i64::from(length_a);
    let length_b = i64::from(length_b);
    frequency_a * (length_b - length_a) + frequency_b * (length_a - length_b)
}

// Out of line: each call follows a whole header plan, and replacing the
// best plan drops one.
#[inline(never)]
fn keep_better(best: &mut Option<DynamicPlan>, candidate: DynamicPlan) {
    if best
        .as_ref()
        .map_or(true, |current| candidate.bits < current.bits)
    {
        *best = Some(candidate);
    }
}

fn consider_rle(
    data_bits: u64,
    literal_lengths: &[u8],
    distance_lengths: &[u8],
    decoded_lengths: &[u8],
    (initial_rle, initial_frequencies): CountedRle,
    exhaustive: bool,
    search: &mut HeaderPlanSearch,
) {
    let mut rle = initial_rle;
    let mut frequencies = initial_frequencies;
    debug_assert_eq!(frequencies, rle_frequencies(&rle));
    let mut prescored_deflopt_trees = [None; 4];
    let mut local_rewrite_steps = std::mem::take(&mut search.local_rewrite_steps);
    local_rewrite_steps.clear();
    for (variant, tree) in prescored_deflopt_trees.iter_mut().enumerate() {
        *tree = consider_columbo_deflopt_local_rewrite(
            data_bits,
            literal_lengths,
            distance_lengths,
            &rle,
            &frequencies,
            variant as u32,
            &mut local_rewrite_steps,
            search,
        );
    }
    search.local_rewrite_steps = local_rewrite_steps;

    // Up to four feedback passes: price every candidate code-length code,
    // then respell the RLE under the cheapest one and repeat. The pass count
    // follows Defluff, which always emits its fourth pass.
    let passes = 4;
    let mut code_length_candidates = std::mem::take(&mut search.code_length_candidates);
    for pass in 0..passes {
        let variants = 4;
        code_length_candidates.clear();
        let trees = CodeLengthTrees::new(&frequencies);
        let mut tree = |builder| trees.get_tree(builder, &mut search.heap_scratch);
        let first = tree(CodeLengthBuilder::OrderHeap(0));
        push_unique_tree(&mut code_length_candidates, first);
        // deft4j's tree builder is slow on the data alphabets but cheap on
        // the code-length alphabet, so every header search includes it.
        let deft4j = tree(CodeLengthBuilder::Deft4jJavaHeap);
        // A complete, unrepaired Huffman tree, the first candidate, prices
        // this RLE in the fewest bits: no code spells its symbols more
        // cheaply, and every usable code gives each of them a length, so none
        // sends fewer code-length codes. Every later candidate, reordered or
        // not, can at best tie its price and its feedback cost, and a tie keeps
        // the earlier, so this pass need not build them. A repaired first tree
        // does the same when it spells the symbols as cheaply as Defluff's
        // exact package-merge code, the cheapest within 7 bits, which the
        // exhaustive search builds anyway; the heap gives lengths only to
        // used symbols, so it sends no more code-length codes than any other.
        let first_is_cheapest = first.valid
            && (first.huffman
                || exhaustive && {
                    let exact = tree(CodeLengthBuilder::DefluffExact);
                    first.rle_bits() != INF && first.rle_bits() == exact.rle_bits()
                });
        for variant in (0..variants).filter(|_| !first_is_cheapest) {
            if variant == 0 {
                push_unique_tree(&mut code_length_candidates, deft4j);
                push_unique_tree(
                    &mut code_length_candidates,
                    tree(CodeLengthBuilder::DefluffLimited),
                );
            } else {
                // Variant 3 breaks both ties by frequency alone, so its
                // order-key tree is the DeflOpt tree, which in pass 0 the local
                // rewrite built first.
                let order = tree(CodeLengthBuilder::OrderHeap(variant));
                debug_assert!(
                    pass != 0
                        || variant != FREQUENCY_ONLY_HEAP_VARIANT
                        || prescored_deflopt_trees[variant as usize]
                            .map_or(true, |lengths| lengths == order.lengths)
                );
                push_unique_tree(&mut code_length_candidates, order);
            }
            if exhaustive {
                // In pass 0, reuse the trees that the local DeflOpt rewrite
                // already built and priced.
                if pass == 0 {
                    if let Some(lengths) = prescored_deflopt_trees[variant as usize] {
                        let deflopt = tree(CodeLengthBuilder::DefloptHeap(variant));
                        debug_assert_eq!(deflopt.lengths, lengths);
                        push_unique_tree(&mut code_length_candidates, deflopt);
                    }
                } else if variant != FREQUENCY_ONLY_HEAP_VARIANT {
                    // Variant 3's DeflOpt tree is the order-key tree above.
                    push_unique_tree(
                        &mut code_length_candidates,
                        tree(CodeLengthBuilder::DefloptHeap(variant)),
                    );
                }
                push_unique_tree(
                    &mut code_length_candidates,
                    tree(CodeLengthBuilder::Columbo(variant)),
                );

                if variant == 0 {
                    push_unique_tree(
                        &mut code_length_candidates,
                        tree(CodeLengthBuilder::DefluffExact),
                    );
                }
            }
        }
        // Priced before the candidates, so of equal plans its stays.
        consider_deft4j_pruned_header(
            data_bits,
            literal_lengths,
            distance_lengths,
            &rle,
            &frequencies,
            &deft4j,
            &mut search.heap_scratch,
            &mut search.best,
            &mut search.rewrites,
        );

        let mut feedback_tree = None;
        let mut feedback_cost = INF;
        for candidate in code_length_candidates.iter() {
            if !candidate.valid {
                continue;
            }
            let was_prescored = pass == 0
                && prescored_deflopt_trees
                    .iter()
                    .flatten()
                    .any(|tree| tree == &candidate.lengths);
            let valid_header = was_prescored
                || consider_tree_header(
                    data_bits,
                    literal_lengths,
                    distance_lengths,
                    &rle,
                    candidate,
                    &mut search.best,
                )
                .is_some();
            if valid_header {
                let candidate_cost = candidate.rle_bits();
                if candidate_cost < feedback_cost {
                    feedback_cost = candidate_cost;
                    feedback_tree = Some(candidate.lengths);
                }

                // As in DeflOpt, give shorter codes to more frequent RLE
                // symbols; a changed HCLEN can pay off even when the weighted
                // symbol cost ties. Lengths that already follow the
                // frequencies never change.
                if !was_prescored && !candidate.follows {
                    let mut reordered = candidate.lengths;
                    if reorder_code_length_lengths(&mut reordered, &frequencies) {
                        consider_dynamic_header(
                            data_bits,
                            literal_lengths,
                            distance_lengths,
                            &rle,
                            &frequencies,
                            reordered,
                            &mut search.best,
                        );
                    }
                }
            }
        }

        if pass + 1 == passes {
            break;
        }
        let Some(tree) = feedback_tree else {
            break;
        };
        // The shortest RLE is the cheapest spelling of these lengths under
        // `tree`, and every change the DeflOpt rewrite makes is strictly
        // cheaper, so an RLE that already costs the shortest's has nothing to
        // rewrite.
        let rle_is_shortest = exhaustive && {
            // The feedback tree's cost of this RLE.
            let cost = feedback_cost;
            debug_assert_eq!(cost, rle_frequency_cost(&frequencies, &tree));
            cost != INF
                && search
                    .shortest_rle(decoded_lengths.len(), &tree)
                    .is_some_and(|(_, shortest, ..)| rle_frequency_cost(shortest, &tree) == cost)
        };
        // The DeflOpt rewrite goes to a search buffer, which a winning
        // rewrite swaps with this pass's.
        let local = if rle_is_shortest {
            debug_assert!(rewrite_rle_deflopt_local_into(
                &rle,
                &frequencies,
                &tree,
                &mut Vec::new()
            )
            .is_none());
            None
        } else {
            rewrite_rle_deflopt_local_into(&rle, &frequencies, &tree, &mut search.rewrites[0])
        };
        let shortest = if exhaustive {
            search.shortest_rle(decoded_lengths.len(), &tree)
        } else {
            None
        };
        // Both carry their exact symbol counts, which price them as their
        // tokens would. A chosen shortest RLE is copied into this pass's
        // buffer, not cloned from the plan's memo.
        let shortest = shortest.filter(|(_, shortest, ..)| {
            local.as_ref().map_or(true, |local| {
                rle_frequency_cost(shortest, &tree) < rle_frequency_cost(local, &tree)
            })
        });
        if let Some((shortest, shortest_frequencies, entry, first_pass)) = shortest {
            // A pass depends only on its RLE, and pricing a header again
            // cannot change the best plan. A seed that started this pass or an
            // earlier one from this RLE has done all that would follow. Equal
            // RLEs have equal symbol counts, so the counts are compared before
            // the tokens.
            if first_pass <= pass + 1
                || (*shortest_frequencies == frequencies && shortest == rle.as_slice())
            {
                break;
            }
            rle.clear();
            rle.extend_from_slice(shortest);
            frequencies = *shortest_frequencies;
            search.shortest_rles[entry].first_pass = pass + 1;
            continue;
        }
        let Some(rewritten_frequencies) = local else {
            break;
        };
        if search.rewrites[0] == rle {
            break;
        }
        std::mem::swap(&mut rle, &mut search.rewrites[0]);
        frequencies = rewritten_frequencies;
    }
    search.code_length_candidates = code_length_candidates;
    search.spare_rles.push(rle);
}

fn rle_symbol_cost(lengths: &[u8; 19], symbol: u8) -> u64 {
    let length = lengths[usize::from(symbol)];
    if length == 0 {
        INF
    } else {
        u64::from(length)
            .saturating_add(rle_extra_bits(symbol))
            .min(INF)
    }
}

/// Whether no two coded symbols have a longer code for a strictly larger
/// frequency: every length's smallest frequency is at least the largest
/// frequency of any longer code.
fn code_lengths_follow_frequencies(lengths: &[u8; 19], frequencies: &[u32; 19]) -> bool {
    let mut smallest = [u32::MAX; 8];
    let mut largest = [0_u32; 8];
    for (&length, &frequency) in lengths.iter().zip(frequencies) {
        let length = usize::from(length);
        if length == 0 {
            continue;
        }
        if length >= smallest.len() {
            return false;
        }
        smallest[length] = smallest[length].min(frequency);
        largest[length] = largest[length].max(frequency);
    }
    let mut largest_longer = 0;
    for length in (1..smallest.len()).rev() {
        if smallest[length] < largest_longer {
            return false;
        }
        largest_longer = largest_longer.max(largest[length]);
    }
    true
}

fn reorder_code_length_lengths(lengths: &mut [u8; 19], frequencies: &[u32; 19]) -> bool {
    // The loop swaps a pair only when its lengths and frequencies differ in
    // the same direction. When no pair does, nothing ever swaps, so a linear
    // check returns the same result; optimal trees always pass it.
    if code_lengths_follow_frequencies(lengths, frequencies) {
        return false;
    }
    let mut changed = false;
    for left in 0..18 {
        if lengths[left] == 0 {
            continue;
        }
        for right in (left + 1)..19 {
            if lengths[right] == 0 {
                continue;
            }
            let should_swap = if lengths[right] < lengths[left] {
                frequencies[right] < frequencies[left]
            } else if lengths[right] > lengths[left] {
                frequencies[right] > frequencies[left]
            } else {
                false
            };
            if should_swap {
                lengths.swap(left, right);
                changed = true;
            }
        }
    }
    changed
}

/// A run of equal code lengths: the length and how many times it repeats.
type LengthRun = (u8, usize);

/// The maximal runs of equal values in `lengths`, in order.
fn length_runs(lengths: &[u8]) -> Vec<LengthRun> {
    let mut runs = Vec::new();
    length_runs_into(lengths, &mut runs);
    runs
}

/// [`length_runs`] into a reused buffer.
fn length_runs_into(lengths: &[u8], runs: &mut Vec<LengthRun>) {
    runs.clear();
    runs.reserve(lengths.len());
    let Some(&first) = lengths.first() else {
        return;
    };
    // Eight lengths at a time, each compared with the one before: the high bit
    // of each nonzero byte of their difference marks a length that starts a
    // run, without a branch per length.
    const LOW_SEVEN: u64 = 0x7f7f_7f7f_7f7f_7f7f;
    let word = |bytes: &[u8]| {
        let mut word = [0_u8; 8];
        word[..bytes.len()].copy_from_slice(bytes);
        u64::from_le_bytes(word)
    };
    let mut start = 0;
    let mut value = first;
    let mut position = 1;
    while position < lengths.len() {
        let count = (lengths.len() - position).min(8);
        let difference = if count == 8 {
            word(&lengths[position..position + 8]) ^ word(&lengths[position - 1..position + 7])
        } else {
            // The padding of both words is zero, so it marks no start.
            word(&lengths[position..]) ^ word(&lengths[position - 1..lengths.len() - 1])
        };
        let mut starts =
            (((difference & LOW_SEVEN).wrapping_add(LOW_SEVEN)) | difference) & !LOW_SEVEN;
        while starts != 0 {
            let next = position + (starts.trailing_zeros() / 8) as usize;
            runs.push((value, next - start));
            start = next;
            value = lengths[next];
            starts &= starts - 1;
        }
        position += 8;
    }
    runs.push((value, lengths.len() - start));
}

fn rle_frequencies(rle: &[RleToken]) -> [u32; 19] {
    // Four interleaved tables, so a run of one symbol does not wait for each
    // increment's store before the next.
    let mut tables = [[0_u32; 19]; 4];
    let mut chunks = rle.chunks_exact(4);
    for chunk in &mut chunks {
        for (table, token) in tables.iter_mut().zip(chunk) {
            table[usize::from(token.symbol)] += 1;
        }
    }
    for token in chunks.remainder() {
        tables[0][usize::from(token.symbol)] += 1;
    }
    let mut frequencies = [0_u32; 19];
    for table in &tables {
        for (frequency, &count) in frequencies.iter_mut().zip(table) {
            *frequency += count;
        }
    }
    frequencies
}

fn trim_code_lengths(lengths: &[u8; 19]) -> usize {
    (4..19)
        .rev()
        .find(|&index| lengths[CODE_LENGTH_ORDER[index]] != 0)
        .map_or(4, |index| index + 1)
}

/// Bits of the RLE-coded code lengths, repeat extra bits included, from the RLE
/// histogram, so pricing many code-length codes for one spelling does not
/// rescan its tokens.
fn rle_frequency_cost(frequencies: &[u32; 19], lengths: &[u8; 19]) -> u64 {
    // A used symbol without a code makes the cost infinite. Otherwise each
    // term is below 2^41, so wrapping arithmetic never wraps, and keeps
    // release overflow checks out of the loop.
    let mut sum = 0_u64;
    let mut missing = false;
    for symbol in 0..19 {
        let frequency = u64::from(frequencies[symbol]);
        let length = lengths[symbol];
        missing |= frequency != 0 && length == 0;
        let cost = u64::from(length).wrapping_add(RLE_EXTRA_BITS[symbol]);
        sum = sum.wrapping_add(frequency.wrapping_mul(cost));
    }
    if missing {
        INF
    } else {
        sum
    }
}

fn dynamic_bits_from_rle_frequencies(
    data_bits: u64,
    hclen: usize,
    frequencies: &[u32; 19],
    lengths: &[u8; 19],
) -> Option<u64> {
    let rle_bits = rle_frequency_cost(frequencies, lengths);
    if rle_bits == INF {
        return None;
    }
    let bits = 3_u64
        .checked_add(5 + 5 + 4)?
        .checked_add(u64::try_from(hclen).ok()?.checked_mul(3)?)?
        .checked_add(rle_bits)?;
    bits.checked_add(data_bits)
}

/// Price one header and replace `best` on a strict improvement. Returns `None`
/// if the code-length code cannot spell `rle`.
#[allow(clippy::too_many_arguments)]
fn consider_dynamic_header(
    data_bits: u64,
    literal_lengths: &[u8],
    distance_lengths: &[u8],
    rle: &[RleToken],
    frequencies: &[u32; 19],
    code_length_lengths: [u8; 19],
    best: &mut Option<DynamicPlan>,
) -> Option<u64> {
    let hclen = trim_code_lengths(&code_length_lengths);
    let bits =
        dynamic_bits_from_rle_frequencies(data_bits, hclen, frequencies, &code_length_lengths)?;
    keep_dynamic_header(
        bits,
        literal_lengths,
        distance_lengths,
        rle,
        code_length_lengths,
        hclen,
        best,
    );
    Some(bits)
}

/// [`consider_dynamic_header`] under a tree built for the RLE's histogram,
/// which already knows its HCLEN and the RLE's cost.
fn consider_tree_header(
    data_bits: u64,
    literal_lengths: &[u8],
    distance_lengths: &[u8],
    rle: &[RleToken],
    tree: &CodeLengthTree,
    best: &mut Option<DynamicPlan>,
) -> Option<u64> {
    debug_assert_eq!(
        tree.rle_bits(),
        rle_frequency_cost(&rle_frequencies(rle), &tree.lengths)
    );
    let rle_bits = tree.rle_bits();
    if rle_bits == INF {
        return None;
    }
    let bits = (3_u64 + 5 + 5 + 4 + u64::from(tree.hclen) * 3)
        .checked_add(rle_bits)?
        .checked_add(data_bits)?;
    keep_dynamic_header(
        bits,
        literal_lengths,
        distance_lengths,
        rle,
        tree.lengths,
        usize::from(tree.hclen),
        best,
    );
    Some(bits)
}

/// Make the header `bits` long the best plan if it is shorter.
fn keep_dynamic_header(
    bits: u64,
    literal_lengths: &[u8],
    distance_lengths: &[u8],
    rle: &[RleToken],
    code_length_lengths: [u8; 19],
    hclen: usize,
    best: &mut Option<DynamicPlan>,
) {
    match best {
        Some(current) if bits >= current.bits => {}
        // A search improves its best plan several times; reuse its buffers.
        Some(current) => {
            for (buffer, lengths) in [
                (&mut current.literal_lengths, literal_lengths),
                (&mut current.distance_lengths, distance_lengths),
            ] {
                buffer.clear();
                buffer.extend_from_slice(lengths);
            }
            current.rle.clear();
            current.rle.extend_from_slice(rle);
            current.code_length_lengths = code_length_lengths;
            current.hlit = literal_lengths.len();
            current.hdist = distance_lengths.len();
            current.hclen = hclen;
            current.bits = bits;
        }
        None => {
            // Room for the longest RLE, one token per code length, so later
            // improvements need not grow it.
            let mut kept_rle = Vec::with_capacity(literal_lengths.len() + distance_lengths.len());
            kept_rle.extend_from_slice(rle);
            *best = Some(DynamicPlan {
                literal_lengths: literal_lengths.to_vec(),
                distance_lengths: distance_lengths.to_vec(),
                code_length_lengths,
                rle: kept_rle,
                hlit: literal_lengths.len(),
                hdist: distance_lengths.len(),
                hclen,
                bits,
            });
        }
    }
}

// Out of line: each call scans the header's run-length tokens.
#[inline(never)]
fn dynamic_bits(data_bits: u64, plan: &DynamicPlan) -> Option<u64> {
    let mut bits = 3_u64 + 5 + 5 + 4 + u64::try_from(plan.hclen).ok()? * 3;
    for token in &plan.rle {
        let code_bits = *plan.code_length_lengths.get(usize::from(token.symbol))?;
        if code_bits == 0 {
            return None;
        }
        bits = bits.checked_add(u64::from(code_bits) + rle_extra_bits(token.symbol))?;
    }
    bits.checked_add(data_bits)
}

const fn rle_extra_bits(symbol: u8) -> u64 {
    match symbol {
        16 => 2,
        17 => 3,
        18 => 7,
        _ => 0,
    }
}

/// [`rle_extra_bits`] by symbol.
const RLE_EXTRA_BITS: [u64; 19] = {
    let mut bits = [0; 19];
    let mut symbol = 0;
    while symbol < bits.len() {
        bits[symbol] = rle_extra_bits(symbol as u8);
        symbol += 1;
    }
    bits
};

#[cfg(test)]
fn rle_cost(rle: &[RleToken], lengths: &[u8; 19]) -> u64 {
    rle.iter()
        .map(|token| rle_symbol_cost(lengths, token.symbol))
        .fold(0_u64, |sum, value| sum.saturating_add(value).min(INF))
}

#[cfg(test)]
pub(crate) mod test_support;
#[cfg(test)]
mod tests;
