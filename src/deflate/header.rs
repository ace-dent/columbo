// SPDX-License-Identifier: MIT

//! Dynamic-header construction and exact bit accounting.

use std::collections::{HashMap, VecDeque};

use super::huffman::{
    code_length_tree_shape_is_valid, huffman_code_lengths_are_valid,
    make_brotli_rle_pseudofrequencies, make_columbo_rle_pseudofrequencies, make_lengths,
    make_lengths_columbo_defluff_limited, make_lengths_columbo_defluff_limited_into,
    make_lengths_deflopt_heap_into_with_scratch, make_lengths_defluff_exact,
    make_lengths_defluff_exact_into, make_lengths_deft4j_java_heap,
    make_lengths_deft4j_java_heap_into, make_lengths_into,
    make_lengths_order_heap_into_with_scratch, make_lengths_zopfli_package_from,
    make_zopfli_rle_pseudofrequencies, payload_tree_shape_is_valid, DefloptHeapScratch,
    FIXED_DISTANCE_CODE_LENGTHS, FIXED_LITERAL_CODE_LENGTHS,
};
use super::model::{
    token_extra_bits, try_clone_slice, DynamicPlan, RleToken, Token, CODE_LENGTH_ORDER,
    MAX_DYNAMIC_CODE_LENGTH_COUNT, RFC_DISTANCE_CODE_COUNT,
};
use super::stop::SearchStop;

mod rotate;
mod tree;
pub(crate) use rotate::{plan_code_length_rotations, RotationBudget};
pub(crate) use tree::{plan_header_tree, HeaderTreeBudget};

const INF: u64 = u64::MAX / 4;
const MAX_HEADER_PLAN_CACHE_ENTRIES: usize = 512;
const REDUCED_PAYLOAD_TREE_DEPTHS: [u8; 2] = [10, 9];
const MAX_RESTRICTED_PAYLOAD_TREE_DEPTH: u8 = 14;

fn minimum_complete_tree_depth(frequencies: &[u32]) -> u8 {
    let populated = frequencies
        .iter()
        .filter(|&&frequency| frequency != 0)
        .count();
    if populated <= 1 {
        1
    } else {
        (usize::BITS - (populated - 1).leading_zeros()) as u8
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct HeaderPlanCacheStats {
    pub(crate) lookups: usize,
    pub(crate) hits: usize,
    pub(crate) misses: usize,
    pub(crate) inserts: usize,
    pub(crate) collision_checks: usize,
    pub(crate) saturated: usize,
}

struct CachedHeaderPlan {
    fingerprint: u64,
    next_same_hash: Option<usize>,
    exhaustive: bool,
    rle_mask: u8,
    /// A complete dynamic plan priced with zero payload bits.
    kernel: DynamicPlan,
}

/// Bounded route-local cache for completed dynamic-header kernels.
///
/// The selected header depends only on the exact trimmed literal/distance
/// length sequences and header-search policy. Token order, symbol frequencies,
/// and match extra bits affect only the additive payload cost, which callers
/// continue to calculate independently. Hashes select a collision chain; every
/// hit verifies both complete length sequences and policy before reuse.
pub(crate) struct HeaderPlanCache {
    first_by_hash: HashMap<u64, usize>,
    entries: Vec<CachedHeaderPlan>,
    stats: HeaderPlanCacheStats,
    max_entries: usize,
}

impl Default for HeaderPlanCache {
    fn default() -> Self {
        Self::new()
    }
}

impl HeaderPlanCache {
    pub(crate) fn new() -> Self {
        Self {
            first_by_hash: HashMap::new(),
            entries: Vec::new(),
            stats: HeaderPlanCacheStats::default(),
            max_entries: MAX_HEADER_PLAN_CACHE_ENTRIES,
        }
    }

    fn price(
        &mut self,
        literal_lengths: &[u8],
        distance_lengths: &[u8],
        data_bits: u64,
        exhaustive: bool,
        rle_mask: u8,
    ) -> Option<DynamicPlan> {
        let fingerprint =
            header_plan_fingerprint(literal_lengths, distance_lengths, exhaustive, rle_mask);
        self.stats.lookups = self.stats.lookups.saturating_add(1);
        let mut candidate = self.first_by_hash.get(&fingerprint).copied();
        while let Some(index) = candidate {
            self.stats.collision_checks = self.stats.collision_checks.saturating_add(1);
            let entry = self.entries.get(index)?;
            let next = entry.next_same_hash;
            let matches = entry.fingerprint == fingerprint
                && entry.exhaustive == exhaustive
                && entry.rle_mask == rle_mask
                && entry.kernel.literal_lengths == literal_lengths
                && entry.kernel.distance_lengths == distance_lengths;
            if matches {
                let mut plan = entry.kernel.try_clone()?;
                plan.bits = plan.bits.checked_add(data_bits)?;
                self.stats.hits = self.stats.hits.saturating_add(1);
                return Some(plan);
            }
            candidate = next;
        }
        self.stats.misses = self.stats.misses.saturating_add(1);

        let mut kernel = plan_for_trimmed_lengths_uncached(
            literal_lengths,
            distance_lengths,
            0,
            exhaustive,
            rle_mask,
        )?;
        let Some(mut plan) = kernel.try_clone() else {
            kernel.bits = kernel.bits.checked_add(data_bits)?;
            return Some(kernel);
        };
        plan.bits = plan.bits.checked_add(data_bits)?;
        self.insert(fingerprint, exhaustive, rle_mask, kernel);
        Some(plan)
    }

    fn insert(&mut self, fingerprint: u64, exhaustive: bool, rle_mask: u8, kernel: DynamicPlan) {
        if self.entries.len() >= self.max_entries {
            self.stats.saturated = self.stats.saturated.saturating_add(1);
            return;
        }
        if self.entries.try_reserve(1).is_err() || self.first_by_hash.try_reserve(1).is_err() {
            self.stats.saturated = self.stats.saturated.saturating_add(1);
            return;
        }
        let next_same_hash = self.first_by_hash.get(&fingerprint).copied();
        let index = self.entries.len();
        self.entries.push(CachedHeaderPlan {
            fingerprint,
            next_same_hash,
            exhaustive,
            rle_mask,
            kernel,
        });
        self.first_by_hash.insert(fingerprint, index);
        self.stats.inserts = self.stats.inserts.saturating_add(1);
    }
}

fn header_plan_fingerprint(
    literal_lengths: &[u8],
    distance_lengths: &[u8],
    exhaustive: bool,
    rle_mask: u8,
) -> u64 {
    let mut fingerprint = 0xcbf2_9ce4_8422_2325_u64;
    mix_header_plan_fingerprint(&mut fingerprint, literal_lengths.len() as u64);
    for &length in literal_lengths {
        mix_header_plan_fingerprint(&mut fingerprint, u64::from(length));
    }
    mix_header_plan_fingerprint(&mut fingerprint, distance_lengths.len() as u64);
    for &length in distance_lengths {
        mix_header_plan_fingerprint(&mut fingerprint, u64::from(length));
    }
    mix_header_plan_fingerprint(
        &mut fingerprint,
        u64::from(exhaustive) | (u64::from(rle_mask) << 1),
    );
    fingerprint
}

#[inline]
fn mix_header_plan_fingerprint(fingerprint: &mut u64, value: u64) {
    *fingerprint ^= value;
    *fingerprint = fingerprint.wrapping_mul(0x0000_0100_0000_01b3);
    *fingerprint ^= *fingerprint >> 32;
}

/// Which deft4j header spelling governs a source-state decision.
///
/// `Complete` is the full 56-way header option grid used by
/// `addOptimisedRecoded`. The deliberately narrower `DefaultRecode` spelling
/// is used only while deciding whether deft4j's repeated individual-prune step
/// reached a smaller fixed point.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Deft4jHeaderPolicy {
    Complete,
    DefaultRecode,
}

/// Switches used by deft4j's code-length run packer.
///
/// Keeping these as named fields makes the deft4j option grid readable
/// without changing its insertion order or adding any dynamic dispatch.
#[derive(Debug, Clone, Copy)]
struct Deft4jPackOptions {
    special_repeat: bool,
    use_eight: bool,
    use_seven: bool,
    no_repeat: bool,
    no_zero_repeat: bool,
    no_long_zero_repeat: bool,
    no_repeat_zeros: bool,
}

/// Header-level choices layered on top of a code-length packing strategy.
#[derive(Debug, Clone, Copy)]
struct Deft4jHeaderOptions {
    pack: Deft4jPackOptions,
    prune: bool,
    optimize_header: bool,
}

const DEFT4J_DEFAULT_RECODE_OPTIONS: Deft4jHeaderOptions = Deft4jHeaderOptions {
    pack: Deft4jPackOptions {
        special_repeat: true,
        use_eight: true,
        use_seven: true,
        no_repeat: false,
        no_zero_repeat: false,
        no_long_zero_repeat: false,
        no_repeat_zeros: false,
    },
    prune: false,
    optimize_header: false,
};

pub(crate) fn token_bits(
    tokens: &[Token],
    literal_lengths: &[u8],
    distance_lengths: &[u8],
) -> Option<u64> {
    // Code lengths and match extras are both token-local. Accumulate them in
    // one pass because fixed-block pricing calls this on every ordinary plan.
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

fn alphabet_payload_bits<const N: usize>(frequencies: &[u32; N], lengths: &[u8]) -> Option<u64> {
    let mut bits = 0_u64;
    for (symbol, &frequency) in frequencies.iter().enumerate() {
        if frequency == 0 {
            continue;
        }
        let length = *lengths.get(symbol)?;
        if length == 0 {
            return None;
        }
        bits = bits.checked_add(u64::from(frequency) * u64::from(length))?;
    }
    Some(bits)
}

pub(crate) fn score_existing_dynamic(
    tokens: &[Token],
    source: &DynamicPlan,
    strict: bool,
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
    let data_bits = token_bits(tokens, &plan.literal_lengths, &plan.distance_lengths)?;
    plan.bits = dynamic_bits(data_bits, &plan)?;
    Some(plan)
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn best_dynamic_plan_cached(
    tokens: &[Token],
    literal_frequencies: &[u32; 286],
    distance_frequencies: &[u32; 30],
    original: Option<&DynamicPlan>,
    strict: bool,
    exhaustive: bool,
    stop: &mut SearchStop<'_>,
    header_cache: &mut HeaderPlanCache,
) -> Option<DynamicPlan> {
    let extra_bits = token_extra_bits(tokens);
    let source_exact = original.and_then(|plan| score_existing_dynamic(tokens, plan, strict));
    let source_is_compatible = source_exact.is_some();
    let mut best = source_exact;

    // Preserve the source trees as a data-code candidate, but repack their
    // header. This is distinct from `score_existing_dynamic`, which retains
    // the source RLE byte-for-byte; DeflOpt explicitly tries both forms.
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
    // The original Columbo C planner preserves family/variant insertion order
    // and keeps up to twenty unique trees per alphabet. Payload sorting is
    // incorrect here: a slightly dearer tree can encode a much smaller header.
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

    // A payload-optimal tree may use very deep codes for rare symbols even
    // when a shallower length-limited tree makes the transmitted code-length
    // sequence substantially cheaper. ECT exercises the general reduced-depth
    // idea while recompressing; Columbo independently applies it to the
    // existing token frequencies and accepts only an exactly priced complete
    // header. Keep this as two paired Max candidates instead of widening the
    // literal/distance family cross-product.
    if exhaustive {
        for max_bits in REDUCED_PAYLOAD_TREE_DEPTHS {
            if stop.reached() {
                break;
            }
            if let Some(candidate) = exact_tree_candidate(
                literal_frequencies,
                distance_frequencies,
                &build_literal_frequencies,
                &build_distance_frequencies,
                extra_bits,
                max_bits,
                true,
                header_cache,
            ) {
                keep_better(&mut best, candidate);
            }
        }
    }

    // Keep the RLE-smoothed pair in --max: the ordinary tree-family grid
    // already covers the fast path, and `keep_better` requires a strict win
    // from the completely priced alternate plan.
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

    // Differential control for Zopfli's published count-smoothing heuristic.
    // Keep it as one paired Max candidate so it cannot multiply the existing
    // literal/distance tree-family grid.
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

    // Symbols with the same frequency may exchange code lengths without
    // changing the payload cost or invalidating either Huffman tree. Their
    // positions do affect the run-length encoded dynamic header, however.
    // Columbo's --max route tries these stable assignments before its more
    // expensive finished-tree searches. DeflOpt 2.07 does not permute a
    // finished tree, so this remains explicitly a Columbo extension.
    if exhaustive && !stop.reached() {
        if let Some(seed) = best.clone() {
            let seed_literal = pad_lengths::<286>(&seed.literal_lengths);
            let seed_distance = pad_lengths::<30>(&seed.distance_lengths);
            for descending in [false, true] {
                // The finished-tree route rearranges literal symbols alone.
                // Keep the distance assignment fixed as an independent
                // candidate; arranging both alphabets can hide a literal-side
                // header win even though neither change affects payload cost.
                let mut literal = seed_literal;
                arrange_equal_frequency_lengths(literal_frequencies, &mut literal, descending);
                if literal != seed_literal && !stop.reached() {
                    if let Some(candidate) =
                        plan_for_explicit_lengths(tokens, &literal, &seed_distance, exhaustive)
                    {
                        keep_better(&mut best, candidate);
                    }
                }

                // The lightweight max route also arranges both alphabets.
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

    // Columbo's greedy swap route explores the same finished-tree degree of
    // freedom more locally. It is particularly useful for small, literal-heavy
    // blocks, where rearranging the tree can save a whole byte without changing
    // a single decoded token. This route is not part of DeflOpt 2.07.
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

const BALANCED_TREE_CANDIDATE_CAP: usize = 5;
const BALANCED_TREE_PAIRED_CAP: usize = 4;
const BALANCED_TREE_PAYLOAD_MARGIN_BITS: i64 = 18;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct BalancedTreeOpportunities {
    pub(crate) dynamic_blocks: usize,
    pub(crate) literal_pair_moves: usize,
    pub(crate) literal_quad_moves: usize,
    pub(crate) distance_pair_moves: usize,
    pub(crate) distance_quad_moves: usize,
    pub(crate) paired_prices: usize,
}

impl BalancedTreeOpportunities {
    pub(crate) fn add_assign(&mut self, other: Self) {
        self.dynamic_blocks = self.dynamic_blocks.saturating_add(other.dynamic_blocks);
        self.literal_pair_moves = self
            .literal_pair_moves
            .saturating_add(other.literal_pair_moves);
        self.literal_quad_moves = self
            .literal_quad_moves
            .saturating_add(other.literal_quad_moves);
        self.distance_pair_moves = self
            .distance_pair_moves
            .saturating_add(other.distance_pair_moves);
        self.distance_quad_moves = self
            .distance_quad_moves
            .saturating_add(other.distance_quad_moves);
        self.paired_prices = self.paired_prices.saturating_add(other.paired_prices);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum BalancedTreeFamily {
    Pair,
    Quad,
}

#[derive(Clone)]
struct BalancedTreeMove<const N: usize> {
    lengths: [u8; N],
    payload_delta: i64,
    family: BalancedTreeFamily,
}

fn insert_frequency_candidate(
    candidates: &mut Vec<(usize, u32)>,
    entry: (usize, u32),
    descending: bool,
) {
    let insertion = candidates
        .iter()
        .position(|&(_, frequency)| {
            if descending {
                frequency < entry.1
            } else {
                frequency > entry.1
            }
        })
        .unwrap_or(candidates.len());
    if insertion < BALANCED_TREE_CANDIDATE_CAP {
        if candidates.len() == BALANCED_TREE_CANDIDATE_CAP {
            candidates.pop();
        }
        candidates.insert(insertion, entry);
    }
}

fn collect_pair_moves<const N: usize>(
    seed_lengths: &[u8; N],
    frequencies: &[u32; N],
) -> Option<Vec<BalancedTreeMove<N>>> {
    // Fourteen length bands, five shortening choices, and ten pairs from the
    // five lengthening choices make 700 the exact pre-filter upper bound.
    let mut moves = Vec::new();
    if moves.try_reserve_exact(14 * 5 * 10).is_err() {
        return None;
    }
    let mut shorten = Vec::<(usize, u32)>::new();
    let mut lengthen = Vec::<(usize, u32)>::new();
    if shorten
        .try_reserve_exact(BALANCED_TREE_CANDIDATE_CAP)
        .is_err()
        || lengthen
            .try_reserve_exact(BALANCED_TREE_CANDIDATE_CAP)
            .is_err()
    {
        return None;
    }

    for length in 2_u8..=14 {
        shorten.clear();
        lengthen.clear();
        for (symbol, &candidate) in seed_lengths.iter().enumerate() {
            if candidate != length {
                continue;
            }
            let entry = (symbol, frequencies[symbol]);
            insert_frequency_candidate(&mut shorten, entry, true);
            insert_frequency_candidate(&mut lengthen, entry, false);
        }
        if shorten.is_empty() || lengthen.len() < 2 {
            continue;
        }

        for &(short_symbol, short_frequency) in &shorten {
            for first in 0..lengthen.len() - 1 {
                for second in first + 1..lengthen.len() {
                    let long_symbols = [lengthen[first], lengthen[second]];
                    if long_symbols
                        .iter()
                        .any(|&(symbol, _)| symbol == short_symbol)
                    {
                        continue;
                    }
                    let payload_delta = long_symbols
                        .iter()
                        .fold(-i64::from(short_frequency), |delta, (_, frequency)| {
                            delta + i64::from(*frequency)
                        });
                    if payload_delta > BALANCED_TREE_PAYLOAD_MARGIN_BITS {
                        continue;
                    }

                    let mut lengths = *seed_lengths;
                    lengths[short_symbol] -= 1;
                    for &(symbol, _) in &long_symbols {
                        lengths[symbol] += 1;
                    }
                    moves.push(BalancedTreeMove {
                        lengths,
                        payload_delta,
                        family: BalancedTreeFamily::Pair,
                    });
                }
            }
        }
    }
    Some(moves)
}

fn collect_quad_moves<const N: usize>(
    seed_lengths: &[u8; N],
    frequencies: &[u32; N],
) -> Option<Vec<BalancedTreeMove<N>>> {
    // The literal/length form is Columbo C's original
    // `consider_quad_lengthen_moves_fast`; applying the same equal-Kraft
    // transformation to distance lengths is the Rust extension.
    // Twelve length bands, five shortening choices, and five four-of-five
    // choices make 300 the exact pre-filter upper bound.
    let mut moves = Vec::new();
    if moves.try_reserve_exact(12 * 5 * 5).is_err() {
        return None;
    }
    let mut shorten = Vec::<(usize, u32)>::new();
    let mut lengthen = Vec::<(usize, u32)>::new();
    if shorten
        .try_reserve_exact(BALANCED_TREE_CANDIDATE_CAP)
        .is_err()
        || lengthen
            .try_reserve_exact(BALANCED_TREE_CANDIDATE_CAP)
            .is_err()
    {
        return None;
    }

    for length in 2_u8..=13 {
        shorten.clear();
        lengthen.clear();
        for (symbol, &candidate) in seed_lengths.iter().enumerate() {
            if candidate == length {
                insert_frequency_candidate(&mut shorten, (symbol, frequencies[symbol]), true);
            } else if candidate == length + 1 {
                insert_frequency_candidate(&mut lengthen, (symbol, frequencies[symbol]), false);
            }
        }
        if shorten.is_empty() || lengthen.len() < 4 {
            continue;
        }

        for &(short_symbol, short_frequency) in &shorten {
            for first in 0..lengthen.len() - 3 {
                for second in first + 1..lengthen.len() - 2 {
                    for third in second + 1..lengthen.len() - 1 {
                        for fourth in third + 1..lengthen.len() {
                            let long_symbols = [
                                lengthen[first],
                                lengthen[second],
                                lengthen[third],
                                lengthen[fourth],
                            ];
                            let payload_delta = long_symbols
                                .iter()
                                .fold(-i64::from(short_frequency), |delta, (_, frequency)| {
                                    delta + i64::from(*frequency)
                                });
                            if payload_delta > BALANCED_TREE_PAYLOAD_MARGIN_BITS {
                                continue;
                            }

                            let mut lengths = *seed_lengths;
                            lengths[short_symbol] -= 1;
                            for &(symbol, _) in &long_symbols {
                                lengths[symbol] += 1;
                            }
                            moves.push(BalancedTreeMove {
                                lengths,
                                payload_delta,
                                family: BalancedTreeFamily::Quad,
                            });
                        }
                    }
                }
            }
        }
    }
    Some(moves)
}

fn retain_best_balanced_moves<const N: usize>(
    pair: &[BalancedTreeMove<N>],
    quad: &[BalancedTreeMove<N>],
) -> Option<Vec<BalancedTreeMove<N>>> {
    let mut best = Vec::new();
    if best.try_reserve_exact(BALANCED_TREE_PAIRED_CAP).is_err() {
        return None;
    }
    for candidate in pair.iter().chain(quad) {
        let insertion = best
            .iter()
            .position(|current: &BalancedTreeMove<N>| {
                (current.payload_delta, current.family, &current.lengths)
                    > (
                        candidate.payload_delta,
                        candidate.family,
                        &candidate.lengths,
                    )
            })
            .unwrap_or(best.len());
        if insertion < BALANCED_TREE_PAIRED_CAP {
            if best.len() == BALANCED_TREE_PAIRED_CAP {
                best.pop();
            }
            best.insert(insertion, candidate.clone());
        }
    }
    Some(best)
}

pub(crate) fn balanced_tree_opportunities(
    literal_frequencies: &[u32; 286],
    distance_frequencies: &[u32; 30],
    seed: &DynamicPlan,
) -> Option<BalancedTreeOpportunities> {
    if seed.literal_lengths.len() > 286 || seed.distance_lengths.len() > 30 {
        return None;
    }
    let seed_literal = pad_lengths::<286>(&seed.literal_lengths);
    let seed_distance = pad_lengths::<30>(&seed.distance_lengths);
    let literal_pair = collect_pair_moves(&seed_literal, literal_frequencies)?.len();
    let literal_quad = collect_quad_moves(&seed_literal, literal_frequencies)?.len();
    let distance_pair = collect_pair_moves(&seed_distance, distance_frequencies)?.len();
    let distance_quad = collect_quad_moves(&seed_distance, distance_frequencies)?.len();
    let paired_prices = literal_pair
        .saturating_add(literal_quad)
        .min(BALANCED_TREE_PAIRED_CAP)
        .saturating_mul(
            distance_pair
                .saturating_add(distance_quad)
                .min(BALANCED_TREE_PAIRED_CAP),
        );
    Some(BalancedTreeOpportunities {
        dynamic_blocks: 1,
        literal_pair_moves: literal_pair,
        literal_quad_moves: literal_quad,
        distance_pair_moves: distance_pair,
        distance_quad_moves: distance_quad,
        paired_prices,
    })
}

/// Try Columbo's bounded equal-Kraft pair/quad moves on both data alphabets.
///
/// Standalone literal/length and distance moves receive exact full-header
/// pricing. A sixteen-candidate cross-product of the lowest-payload-delta moves
/// can additionally exploit the RLE run that crosses the alphabet boundary.
/// The original complete candidate remains outside this helper as an
/// independent fallback.
pub(crate) fn plan_columbo_balanced_tree_candidate(
    tokens: &[Token],
    literal_frequencies: &[u32; 286],
    distance_frequencies: &[u32; 30],
    seed: &DynamicPlan,
    exhaustive_header: bool,
    price_literal_pair: bool,
) -> Option<DynamicPlan> {
    if seed.literal_lengths.len() > 286 || seed.distance_lengths.len() > 30 {
        return None;
    }
    let seed_literal = pad_lengths::<286>(&seed.literal_lengths);
    let seed_distance = pad_lengths::<30>(&seed.distance_lengths);
    let extra_bits = token_extra_bits(tokens);
    let mut best: Option<DynamicPlan> = None;
    let literal_pair = collect_pair_moves(&seed_literal, literal_frequencies)?;
    let literal_quad = collect_quad_moves(&seed_literal, literal_frequencies)?;
    let distance_pair = collect_pair_moves(&seed_distance, distance_frequencies)?;
    let distance_quad = collect_quad_moves(&seed_distance, distance_frequencies)?;

    for candidate in literal_pair
        .iter()
        .filter(|_| price_literal_pair)
        .chain(&literal_quad)
    {
        let Some(data_bits) = token_bits_from_frequencies(
            literal_frequencies,
            distance_frequencies,
            &candidate.lengths,
            &seed_distance,
            extra_bits,
        ) else {
            continue;
        };
        if let Some(candidate) = plan_for_explicit_lengths_with_cost(
            &candidate.lengths,
            &seed_distance,
            data_bits,
            exhaustive_header,
        ) {
            keep_better(&mut best, candidate);
        }
    }

    for candidate in distance_pair.iter().chain(&distance_quad) {
        let Some(data_bits) = token_bits_from_frequencies(
            literal_frequencies,
            distance_frequencies,
            &seed_literal,
            &candidate.lengths,
            extra_bits,
        ) else {
            continue;
        };
        if let Some(candidate) = plan_for_explicit_lengths_with_cost(
            &seed_literal,
            &candidate.lengths,
            data_bits,
            exhaustive_header,
        ) {
            keep_better(&mut best, candidate);
        }
    }

    let literal_best = retain_best_balanced_moves(&literal_pair, &literal_quad)?;
    let distance_best = retain_best_balanced_moves(&distance_pair, &distance_quad)?;
    for literal in &literal_best {
        for distance in &distance_best {
            let combined_delta = literal.payload_delta.saturating_add(distance.payload_delta);
            if combined_delta > BALANCED_TREE_PAYLOAD_MARGIN_BITS
                || (!exhaustive_header && combined_delta > 0)
            {
                continue;
            }
            let Some(data_bits) = token_bits_from_frequencies(
                literal_frequencies,
                distance_frequencies,
                &literal.lengths,
                &distance.lengths,
                extra_bits,
            ) else {
                continue;
            };
            if let Some(candidate) = plan_for_explicit_lengths_with_cost(
                &literal.lengths,
                &distance.lengths,
                data_bits,
                exhaustive_header,
            ) {
                keep_better(&mut best, candidate);
            }
        }
    }

    best
}

/// Reassign one tree's lengths within equal-frequency symbol groups.
///
/// Sorting only the lengths (and retaining the symbol positions) preserves
/// both the Kraft sum and the exact frequency-weighted payload cost. The
/// stable symbol order makes this a cheap pair of useful header candidates
/// instead of a factorial permutation search.
fn arrange_equal_frequency_lengths<const N: usize>(
    frequencies: &[u32; N],
    lengths: &mut [u8; N],
    descending: bool,
) {
    let mut visited = [false; N];
    for first in 0..N {
        if visited[first] || frequencies[first] == 0 || lengths[first] == 0 {
            continue;
        }

        let frequency = frequencies[first];
        let mut positions = Vec::new();
        let mut assigned_lengths = Vec::new();
        for symbol in first..N {
            if !visited[symbol] && frequencies[symbol] == frequency && lengths[symbol] != 0 {
                visited[symbol] = true;
                positions.push(symbol);
                assigned_lengths.push(lengths[symbol]);
            }
        }
        if positions.len() < 2 {
            continue;
        }

        assigned_lengths.sort_unstable();
        if descending {
            assigned_lengths.reverse();
        }
        for (symbol, length) in positions.into_iter().zip(assigned_lengths) {
            lengths[symbol] = length;
        }
    }
}

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

fn tree_candidates(frequencies: &[u32], max_bits: u8, exhaustive: bool) -> Vec<Vec<u8>> {
    let mut candidates = Vec::with_capacity(if exhaustive { 16 } else { 8 });
    let mut heap_scratch = DefloptHeapScratch::default();
    // Family order is observable because equal complete plans retain the
    // earlier candidate. The original Columbo C selector combines the mapped
    // DeflOpt heap with Columbo's legacy order-key heap. Broader Columbo and
    // exact Defluff families belong to max or terminal feedback routes.
    // Keeping that separation caps the ordinary cross product at 64 pairs.
    for variant in 0..4 {
        let mut lengths = vec![0_u8; frequencies.len()];
        make_lengths_deflopt_heap_into_with_scratch(
            frequencies,
            &mut lengths,
            max_bits,
            variant,
            &mut heap_scratch,
        );
        push_unique(&mut candidates, lengths);
    }
    for variant in 0..4 {
        let mut lengths = vec![0_u8; frequencies.len()];
        make_lengths_order_heap_into_with_scratch(
            frequencies,
            &mut lengths,
            max_bits,
            variant,
            &mut heap_scratch,
        );
        push_unique(&mut candidates, lengths);
    }

    if exhaustive {
        for variant in 0..4 {
            push_unique(
                &mut candidates,
                make_lengths(frequencies, max_bits, variant),
            );
        }
        push_unique(
            &mut candidates,
            make_lengths_columbo_defluff_limited(frequencies, max_bits, 0),
        );
        let defluff_exact = make_lengths_defluff_exact(frequencies, max_bits, 0);
        push_unique(&mut candidates, defluff_exact.clone());
        push_unique(
            &mut candidates,
            make_lengths_deft4j_java_heap(frequencies, max_bits),
        );
        push_unique(
            &mut candidates,
            make_lengths_zopfli_package_from(frequencies, &defluff_exact, max_bits),
        );
    }
    candidates.retain(|lengths| {
        lengths.len() == frequencies.len()
            && frequencies
                .iter()
                .zip(lengths)
                .all(|(&frequency, &length)| frequency == 0 || length != 0)
            && huffman_code_lengths_are_valid(lengths)
    });
    candidates
}

#[allow(clippy::too_many_arguments)]
fn exact_tree_candidate(
    literal_frequencies: &[u32; 286],
    distance_frequencies: &[u32; 30],
    build_literal_frequencies: &[u32; 286],
    build_distance_frequencies: &[u32; 30],
    extra_bits: u64,
    max_bits: u8,
    exhaustive: bool,
    header_cache: &mut HeaderPlanCache,
) -> Option<DynamicPlan> {
    let literal_lengths = make_lengths_defluff_exact(build_literal_frequencies, max_bits, 0);
    let distance_lengths = make_lengths_defluff_exact(build_distance_frequencies, max_bits, 0);

    explicit_exact_tree_candidate(
        literal_frequencies,
        distance_frequencies,
        &literal_lengths,
        &distance_lengths,
        extra_bits,
        exhaustive,
        header_cache,
    )
}

#[allow(clippy::too_many_arguments)]
fn explicit_exact_tree_candidate(
    literal_frequencies: &[u32; 286],
    distance_frequencies: &[u32; 30],
    literal_lengths: &[u8],
    distance_lengths: &[u8],
    extra_bits: u64,
    exhaustive: bool,
    header_cache: &mut HeaderPlanCache,
) -> Option<DynamicPlan> {
    let data_bits = token_bits_from_frequencies(
        literal_frequencies,
        distance_frequencies,
        literal_lengths,
        distance_lengths,
        extra_bits,
    )?;
    price_explicit_exact_tree_candidate(
        distance_frequencies,
        literal_lengths,
        distance_lengths,
        data_bits,
        exhaustive,
        header_cache,
    )
}

fn price_explicit_exact_tree_candidate(
    distance_frequencies: &[u32; 30],
    literal_lengths: &[u8],
    distance_lengths: &[u8],
    data_bits: u64,
    exhaustive: bool,
    header_cache: &mut HeaderPlanCache,
) -> Option<DynamicPlan> {
    if literal_lengths.get(256).copied().unwrap_or(0) == 0
        || !payload_tree_shape_is_valid(literal_lengths, false)
        || !payload_tree_shape_is_valid(distance_lengths, true)
        || distance_frequencies
            .iter()
            .enumerate()
            .any(|(symbol, &frequency)| frequency != 0 && distance_lengths[symbol] == 0)
    {
        return None;
    }
    plan_for_explicit_lengths_with_cost_cached(
        literal_lengths,
        distance_lengths,
        data_bits,
        exhaustive,
        header_cache,
    )
}

#[allow(clippy::too_many_arguments)]
fn price_exact_tree_cross_product(
    literal_frequencies: &[u32; 286],
    distance_frequencies: &[u32; 30],
    literal_candidates: &[Vec<u8>],
    distance_candidates: &[Vec<u8>],
    extra_bits: u64,
    exhaustive_header: bool,
    header_cache: &mut HeaderPlanCache,
    best: &mut Option<DynamicPlan>,
    stop: &mut SearchStop<'_>,
) -> bool {
    let literal_payload_bits: Vec<Option<u64>> = literal_candidates
        .iter()
        .map(|lengths| alphabet_payload_bits(literal_frequencies, lengths))
        .collect();
    let distance_payload_bits: Vec<Option<u64>> = distance_candidates
        .iter()
        .map(|lengths| alphabet_payload_bits(distance_frequencies, lengths))
        .collect();
    let pair_capacity = literal_candidates
        .len()
        .checked_mul(distance_candidates.len())
        .unwrap_or(0);
    let mut ordered_pairs = Vec::new();
    if ordered_pairs.try_reserve_exact(pair_capacity).is_ok() {
        for (literal_index, &literal_bits) in literal_payload_bits.iter().enumerate() {
            let Some(literal_bits) = literal_bits else {
                continue;
            };
            for (distance_index, &distance_bits) in distance_payload_bits.iter().enumerate() {
                let Some(data_bits) = distance_bits.and_then(|distance_bits| {
                    extra_bits
                        .checked_add(literal_bits)?
                        .checked_add(distance_bits)
                }) else {
                    continue;
                };
                ordered_pairs.push((data_bits, literal_index, distance_index));
            }
        }
        // Payload cost is an admissible lower bound on complete dynamic-block
        // cost. Visiting the smallest bound first maximizes useful work before
        // a deadline and lets the exact incumbent reject hopeless headers;
        // it changes neither the finite candidate set nor acceptance.
        ordered_pairs.sort_unstable();
        for (data_bits, literal_index, distance_index) in ordered_pairs {
            if !price_exact_tree_pair(
                distance_frequencies,
                &literal_candidates[literal_index],
                &distance_candidates[distance_index],
                data_bits,
                exhaustive_header,
                header_cache,
                best,
                stop,
            ) {
                return false;
            }
        }
        return true;
    }

    // Allocation failure is not a reason to lose an otherwise valid frontier;
    // retain the original deterministic traversal without the ordering aid.
    for (literal_index, &literal_bits) in literal_payload_bits.iter().enumerate() {
        let Some(literal_bits) = literal_bits else {
            continue;
        };
        for (distance_index, &distance_bits) in distance_payload_bits.iter().enumerate() {
            let Some(data_bits) = distance_bits.and_then(|distance_bits| {
                extra_bits
                    .checked_add(literal_bits)?
                    .checked_add(distance_bits)
            }) else {
                continue;
            };
            if !price_exact_tree_pair(
                distance_frequencies,
                &literal_candidates[literal_index],
                &distance_candidates[distance_index],
                data_bits,
                exhaustive_header,
                header_cache,
                best,
                stop,
            ) {
                return false;
            }
        }
    }
    true
}

#[allow(clippy::too_many_arguments)]
fn price_exact_tree_pair(
    distance_frequencies: &[u32; 30],
    literal_lengths: &[u8],
    distance_lengths: &[u8],
    data_bits: u64,
    exhaustive_header: bool,
    header_cache: &mut HeaderPlanCache,
    best: &mut Option<DynamicPlan>,
    stop: &mut SearchStop<'_>,
) -> bool {
    if stop.reached() {
        return false;
    }
    // A dynamic header has positive cost. If its payload alone cannot
    // strictly beat the completed incumbent, constructing every RLE and
    // code-length-tree spelling cannot change the result.
    if best
        .as_ref()
        .is_some_and(|current| data_bits >= current.bits)
    {
        return true;
    }
    if let Some(candidate) = price_explicit_exact_tree_candidate(
        distance_frequencies,
        literal_lengths,
        distance_lengths,
        data_bits,
        exhaustive_header,
        header_cache,
    ) {
        keep_better(best, candidate);
    }
    true
}

/// Price every feasible restricted payload-tree depth without changing tokens.
///
/// The minimum comes from the prefix-code capacity bound `2^depth >= leaves`;
/// the frontier then covers every ceiling below Deflate's unrestricted
/// fifteen-bit maximum. This avoids using corpus-trained token-count bands to
/// choose one ceiling. At each ordinary frontier depth, the leaf-first and
/// package-first equal-payload shapes form a deduplicated two-by-two alphabet
/// product. A caller with a separate structural work bound may instead request
/// the complete cross-product of every unique depth-and-tie tree for each
/// alphabet; this is a finite tree-shape dimension, not a corpus gate. Both
/// forms remain additive at stream level: callers retain their completed
/// parent and accept a sibling only after exact emission.
pub(crate) fn plan_bounded_depth_tree_candidate(
    tokens: &[Token],
    literal_frequencies: &[u32; 286],
    distance_frequencies: &[u32; 30],
    strict: bool,
    exhaustive_header: bool,
    independent_depths: bool,
    stop: &mut SearchStop<'_>,
) -> Option<DynamicPlan> {
    let mut build_literal_frequencies = *literal_frequencies;
    ensure_code_symbols(&mut build_literal_frequencies, strict);
    let mut build_distance_frequencies = *distance_frequencies;
    ensure_distance_symbols(&mut build_distance_frequencies, strict);
    let extra_bits = token_extra_bits(tokens);
    let mut header_cache = HeaderPlanCache::new();
    let mut best = None;

    if independent_depths {
        let mut literal_candidates = Vec::new();
        for max_bits in minimum_complete_tree_depth(&build_literal_frequencies)
            ..=MAX_RESTRICTED_PAYLOAD_TREE_DEPTH
        {
            let defluff = make_lengths_defluff_exact(&build_literal_frequencies, max_bits, 0);
            let package_first =
                make_lengths_zopfli_package_from(&build_literal_frequencies, &defluff, max_bits);
            push_unique(&mut literal_candidates, defluff);
            push_unique(&mut literal_candidates, package_first);
        }
        let mut distance_candidates = Vec::new();
        for max_bits in minimum_complete_tree_depth(&build_distance_frequencies)
            ..=MAX_RESTRICTED_PAYLOAD_TREE_DEPTH
        {
            let defluff = make_lengths_defluff_exact(&build_distance_frequencies, max_bits, 0);
            let package_first =
                make_lengths_zopfli_package_from(&build_distance_frequencies, &defluff, max_bits);
            push_unique(&mut distance_candidates, defluff);
            push_unique(&mut distance_candidates, package_first);
        }
        price_exact_tree_cross_product(
            literal_frequencies,
            distance_frequencies,
            &literal_candidates,
            &distance_candidates,
            extra_bits,
            exhaustive_header,
            &mut header_cache,
            &mut best,
            stop,
        );
    } else {
        let minimum_depth = minimum_complete_tree_depth(&build_literal_frequencies)
            .max(minimum_complete_tree_depth(&build_distance_frequencies));
        let mut literal_candidates = Vec::with_capacity(2);
        let mut distance_candidates = Vec::with_capacity(2);
        for max_bits in minimum_depth..=MAX_RESTRICTED_PAYLOAD_TREE_DEPTH {
            literal_candidates.clear();
            let literal_defluff =
                make_lengths_defluff_exact(&build_literal_frequencies, max_bits, 0);
            let literal_package_first = make_lengths_zopfli_package_from(
                &build_literal_frequencies,
                &literal_defluff,
                max_bits,
            );
            push_unique(&mut literal_candidates, literal_defluff);
            push_unique(&mut literal_candidates, literal_package_first);
            distance_candidates.clear();
            let distance_defluff =
                make_lengths_defluff_exact(&build_distance_frequencies, max_bits, 0);
            let distance_package_first = make_lengths_zopfli_package_from(
                &build_distance_frequencies,
                &distance_defluff,
                max_bits,
            );
            push_unique(&mut distance_candidates, distance_defluff);
            push_unique(&mut distance_candidates, distance_package_first);
            if !price_exact_tree_cross_product(
                literal_frequencies,
                distance_frequencies,
                &literal_candidates,
                &distance_candidates,
                extra_bits,
                exhaustive_header,
                &mut header_cache,
                &mut best,
                stop,
            ) {
                break;
            }
        }
    }
    best
}

/// Price a compact RLE-smoothed payload-tree frontier.
///
/// This is intentionally separate from the ordinary block planner. A locally
/// cheaper tree can redirect later token feedback into a worse fixed point;
/// callers use this as an additive completed-stream floor, retaining the
/// original lineage unless the fully emitted stream is strictly smaller. The
/// paired literal/distance candidates use every maximum depth from 15 through
/// 9 without multiplying either alphabet through the ordinary family
/// cross-product. The two seed families are Brotli's fixed-point smoother and
/// Zopfli's classic nearby-count smoother.
pub(crate) fn plan_rle_smoothed_tree_candidate(
    tokens: &[Token],
    literal_frequencies: &[u32; 286],
    distance_frequencies: &[u32; 30],
    strict: bool,
) -> Option<DynamicPlan> {
    let mut families = [
        (*literal_frequencies, *distance_frequencies),
        (*literal_frequencies, *distance_frequencies),
    ];
    make_brotli_rle_pseudofrequencies(&mut families[0].0);
    make_brotli_rle_pseudofrequencies(&mut families[0].1);
    make_zopfli_rle_pseudofrequencies(&mut families[1].0);
    make_zopfli_rle_pseudofrequencies(&mut families[1].1);
    let extra_bits = token_extra_bits(tokens);
    let mut cache = HeaderPlanCache::new();
    let mut best = None;
    for (build_literal_frequencies, build_distance_frequencies) in &mut families {
        if *build_literal_frequencies == *literal_frequencies
            && *build_distance_frequencies == *distance_frequencies
        {
            continue;
        }
        ensure_code_symbols(build_literal_frequencies, strict);
        ensure_distance_symbols(build_distance_frequencies, strict);
        for max_bits in [15, 14, 13, 12, 11, 10, 9] {
            if let Some(candidate) = exact_tree_candidate(
                literal_frequencies,
                distance_frequencies,
                build_literal_frequencies,
                build_distance_frequencies,
                extra_bits,
                max_bits,
                true,
                &mut cache,
            ) {
                keep_better(&mut best, candidate);
            }
        }
    }
    best
}

/// Build Columbo's adjacency-quantized RLE-friendly tree candidate.
///
/// Zopfli's pseudo-frequency route, as used by Turtledeflate when writing
/// dynamic blocks and, above compression level seven, estimating them,
/// motivates this candidate shape. Columbo's adjacency quantizer, tree
/// construction, complete header search, and strict scoring against the
/// original frequencies are original.
fn columbo_rle_tree_candidate(
    literal_frequencies: &[u32; 286],
    distance_frequencies: &[u32; 30],
    extra_bits: u64,
    strict: bool,
    exhaustive: bool,
) -> Option<DynamicPlan> {
    let mut pseudo_literal_frequencies = *literal_frequencies;
    let mut pseudo_distance_frequencies = *distance_frequencies;
    make_columbo_rle_pseudofrequencies(&mut pseudo_literal_frequencies);
    make_columbo_rle_pseudofrequencies(&mut pseudo_distance_frequencies);

    let quantization_changed_tree_weights = pseudo_literal_frequencies != *literal_frequencies
        || pseudo_distance_frequencies != *distance_frequencies;
    if !quantization_changed_tree_weights {
        return None;
    }

    ensure_code_symbols(&mut pseudo_literal_frequencies, strict);
    ensure_distance_symbols(&mut pseudo_distance_frequencies, strict);
    let literal = make_lengths(&pseudo_literal_frequencies, 15, 0);
    let distance = make_lengths(&pseudo_distance_frequencies, 15, 0);
    if literal_frequencies
        .iter()
        .zip(&literal)
        .any(|(&frequency, &length)| frequency != 0 && length == 0)
        || distance_frequencies
            .iter()
            .zip(&distance)
            .any(|(&frequency, &length)| frequency != 0 && length == 0)
    {
        return None;
    }
    huffman_code_lengths_are_valid(&literal).then_some(())?;
    huffman_code_lengths_are_valid(&distance).then_some(())?;
    let data_bits = token_bits_from_frequencies(
        literal_frequencies,
        distance_frequencies,
        &literal,
        &distance,
        extra_bits,
    )?;
    plan_for_explicit_lengths_with_cost(&literal, &distance, data_bits, exhaustive)
}

/// Build one paired data-tree candidate from Zopfli's RLE-friendly counts.
///
/// Only the pseudo-frequency transform is imported. Columbo supplies its own
/// tree builder and complete dynamic-header search, then exact-prices payload
/// bits using the unmodified source frequencies.
fn zopfli_rle_tree_candidate(
    literal_frequencies: &[u32; 286],
    distance_frequencies: &[u32; 30],
    extra_bits: u64,
    strict: bool,
    exhaustive: bool,
) -> Option<DynamicPlan> {
    let mut pseudo_literal_frequencies = *literal_frequencies;
    let mut pseudo_distance_frequencies = *distance_frequencies;
    make_zopfli_rle_pseudofrequencies(&mut pseudo_literal_frequencies);
    make_zopfli_rle_pseudofrequencies(&mut pseudo_distance_frequencies);

    let smoothing_changed_tree_weights = pseudo_literal_frequencies != *literal_frequencies
        || pseudo_distance_frequencies != *distance_frequencies;
    if !smoothing_changed_tree_weights {
        return None;
    }

    ensure_code_symbols(&mut pseudo_literal_frequencies, strict);
    ensure_distance_symbols(&mut pseudo_distance_frequencies, strict);
    let literal = make_lengths(&pseudo_literal_frequencies, 15, 0);
    let distance = make_lengths(&pseudo_distance_frequencies, 15, 0);
    if literal_frequencies
        .iter()
        .zip(&literal)
        .any(|(&frequency, &length)| frequency != 0 && length == 0)
        || distance_frequencies
            .iter()
            .zip(&distance)
            .any(|(&frequency, &length)| frequency != 0 && length == 0)
    {
        return None;
    }
    huffman_code_lengths_are_valid(&literal).then_some(())?;
    huffman_code_lengths_are_valid(&distance).then_some(())?;
    let data_bits = token_bits_from_frequencies(
        literal_frequencies,
        distance_frequencies,
        &literal,
        &distance,
        extra_bits,
    )?;
    plan_for_explicit_lengths_with_cost(&literal, &distance, data_bits, exhaustive)
}

fn push_unique<T: PartialEq>(candidates: &mut Vec<T>, candidate: T) {
    if !candidates.iter().any(|current| current == &candidate) {
        candidates.push(candidate);
    }
}

fn trim_literal(lengths: &[u8]) -> usize {
    trim_to_last_nonzero(lengths, 257)
}

fn trim_distance(lengths: &[u8]) -> usize {
    trim_to_last_nonzero(lengths, 1)
}

fn trim_to_last_nonzero(lengths: &[u8], minimum: usize) -> usize {
    lengths
        .iter()
        .enumerate()
        .skip(minimum)
        .rfind(|(_, length)| **length != 0)
        .map_or(minimum, |(index, _)| index + 1)
}

/// Estimate one block for coarse structural boundary discovery.
///
/// This intentionally prices only DeflOpt's variant-zero data trees with one
/// ordinary greedy dynamic-header spelling, plus the fixed tree. It is cheap
/// enough for many histogram probes and is never an acceptance test: every
/// selected boundary is subsequently replanned by Columbo's complete exact
/// representation selector.
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
    let mut heap_scratch = DefloptHeapScratch::default();
    make_lengths_deflopt_heap_into_with_scratch(
        &build_literal_frequencies,
        &mut literal_lengths,
        15,
        0,
        &mut heap_scratch,
    );
    make_lengths_deflopt_heap_into_with_scratch(
        &build_distance_frequencies,
        &mut distance_lengths,
        15,
        0,
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
    heap_scratch: &mut DefloptHeapScratch,
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
    let mut code_length_lengths = [0_u8; 19];
    make_lengths_deflopt_heap_into_with_scratch(
        &code_length_frequencies,
        &mut code_length_lengths,
        7,
        0,
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

/// Score an explicitly assigned literal/length and distance tree.
///
/// Keeping this separate from tree construction lets structural searches
/// rearrange a valid length histogram solely to make its dynamic header less
/// expensive. The represented token stream is never changed here.
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

/// Score a dynamic header with deft4j beta-17's ordered header grid.
///
/// Columbo's ordinary planner intentionally considers a wider family of
/// headers. The deft4j-derived route cannot use that wider score to guide its
/// state graph without changing which intermediate states the source ordering
/// retains. This helper therefore keeps the data trees fixed and reproduces
/// deft4j's option grid and insertion order after Columbo trims HLIT and HDIST.
/// beta-17 can instead retain the source header's advertised spans for its
/// source-dynamic base.
pub(crate) fn plan_for_deft4j_lengths_with_cost(
    literal_frequencies: &[u32; 286],
    distance_frequencies: &[u32; 30],
    extra_bits: u64,
    literal_lengths: &[u8; 286],
    distance_lengths: &[u8; 30],
    strict: bool,
    policy: Deft4jHeaderPolicy,
) -> Option<DynamicPlan> {
    let mut literal_lengths = *literal_lengths;
    apply_min_code_lengths(&mut literal_lengths, literal_frequencies, strict);
    let mut distance_lengths = *distance_lengths;
    apply_min_code_lengths(&mut distance_lengths, distance_frequencies, strict);

    let data_bits = token_bits_from_frequencies(
        literal_frequencies,
        distance_frequencies,
        &literal_lengths,
        &distance_lengths,
        extra_bits,
    )?;
    let hlit = trim_literal(&literal_lengths);
    let hdist = trim_distance(&distance_lengths);
    let literal = try_clone_slice(&literal_lengths[..hlit])?;
    let distance = try_clone_slice(&distance_lengths[..hdist])?;
    if !huffman_code_lengths_are_valid(&literal)
        || !huffman_code_lengths_are_valid(&distance)
        || !payload_tree_shape_is_valid(&literal, false)
        || !payload_tree_shape_is_valid(&distance, true)
    {
        return None;
    }

    let mut combined = Vec::new();
    combined
        .try_reserve_exact(literal.len().checked_add(distance.len())?)
        .ok()?;
    combined.extend_from_slice(&literal);
    combined.extend_from_slice(&distance);

    if policy == Deft4jHeaderPolicy::DefaultRecode {
        return build_deft4j_header(
            data_bits,
            &literal,
            &distance,
            &combined,
            DEFT4J_DEFAULT_RECODE_OPTIONS,
        );
    }

    let mut best = None;
    // Keep deft4j's loop order. Equal-sized headers deliberately retain the
    // first spelling because it becomes the block object used by later
    // transformations and merges.
    for no_repeat_zeros in [false, true] {
        for prune in [false, true] {
            for no_repeat in [false, true] {
                if no_repeat_zeros && no_repeat {
                    continue;
                }
                for no_zero_repeat in [false, true] {
                    if no_repeat_zeros && !no_zero_repeat {
                        continue;
                    }
                    for no_long_zero_repeat in [false, true] {
                        for special_repeat in [true, false] {
                            if special_repeat {
                                if no_repeat {
                                    continue;
                                }
                                for use_eight in [true, false] {
                                    for use_seven in [true, false] {
                                        if !use_eight && !use_seven {
                                            continue;
                                        }
                                        if let Some(candidate) = build_deft4j_header(
                                            data_bits,
                                            &literal,
                                            &distance,
                                            &combined,
                                            Deft4jHeaderOptions {
                                                pack: Deft4jPackOptions {
                                                    special_repeat: true,
                                                    use_eight,
                                                    use_seven,
                                                    no_repeat,
                                                    no_zero_repeat,
                                                    no_long_zero_repeat,
                                                    no_repeat_zeros,
                                                },
                                                prune,
                                                optimize_header: true,
                                            },
                                        ) {
                                            keep_better(&mut best, candidate);
                                        }
                                    }
                                }
                            } else if let Some(candidate) = build_deft4j_header(
                                data_bits,
                                &literal,
                                &distance,
                                &combined,
                                Deft4jHeaderOptions {
                                    pack: Deft4jPackOptions {
                                        special_repeat: false,
                                        use_eight: false,
                                        use_seven: false,
                                        no_repeat,
                                        no_zero_repeat,
                                        no_long_zero_repeat,
                                        no_repeat_zeros,
                                    },
                                    prune,
                                    optimize_header: true,
                                },
                            ) {
                                keep_better(&mut best, candidate);
                            }
                        }
                    }
                }
            }
        }
    }
    best
}

fn apply_min_code_lengths(lengths: &mut [u8], frequencies: &[u32], enabled: bool) {
    if !enabled || lengths.iter().filter(|&&length| length != 0).count() >= 2 {
        return;
    }
    let used = frequencies.iter().position(|&frequency| frequency != 0);
    match used {
        Some(symbol) => {
            lengths[symbol] = 1;
            lengths[usize::from(symbol == 0)] = 1;
        }
        None => {
            lengths[0] = 1;
            lengths[1] = 1;
        }
    }
}

fn build_deft4j_header(
    data_bits: u64,
    literal_lengths: &[u8],
    distance_lengths: &[u8],
    combined: &[u8],
    options: Deft4jHeaderOptions,
) -> Option<DynamicPlan> {
    let mut rle = deft4j_pack_code_lengths(combined, options.pack)?;
    let mut code_length_lengths = deft4j_code_length_tree(&rle)?;

    if options.prune {
        if let Some(pruned) = rewrite_rle_deft4j_literals(&rle, &code_length_lengths, true) {
            rle = pruned;
            code_length_lengths = deft4j_code_length_tree(&rle)?;
        }
    }

    let mut plan = deft4j_dynamic_plan(
        data_bits,
        literal_lengths,
        distance_lengths,
        rle,
        code_length_lengths,
    )?;
    if options.optimize_header {
        if let Some(optimized) =
            rewrite_rle_deft4j_literals(&plan.rle, &plan.code_length_lengths, false)
        {
            if let Some(candidate) = deft4j_dynamic_plan(
                data_bits,
                literal_lengths,
                distance_lengths,
                optimized,
                plan.code_length_lengths,
            ) {
                if candidate.bits < plan.bits {
                    plan = candidate;
                }
            }
        }
    }
    Some(plan)
}

fn deft4j_code_length_tree(rle: &[RleToken]) -> Option<[u8; 19]> {
    let frequencies = rle_frequencies(rle);
    let mut lengths = [0_u8; 19];
    make_lengths_deft4j_java_heap_into(&frequencies, &mut lengths, 7);
    if !code_length_tree_shape_is_valid(&lengths) {
        return None;
    }
    rle.iter()
        .all(|token| lengths[usize::from(token.symbol)] != 0)
        .then_some(lengths)
}

fn deft4j_dynamic_plan(
    data_bits: u64,
    literal_lengths: &[u8],
    distance_lengths: &[u8],
    rle: Vec<RleToken>,
    code_length_lengths: [u8; 19],
) -> Option<DynamicPlan> {
    let hclen = trim_code_lengths(&code_length_lengths);
    let mut plan = DynamicPlan {
        literal_lengths: try_clone_slice(literal_lengths)?,
        distance_lengths: try_clone_slice(distance_lengths)?,
        code_length_lengths,
        rle,
        hlit: literal_lengths.len(),
        hdist: distance_lengths.len(),
        hclen,
        bits: 0,
    };
    plan.bits = dynamic_bits(data_bits, &plan)?;
    Some(plan)
}

fn deft4j_pack_code_lengths(lengths: &[u8], options: Deft4jPackOptions) -> Option<Vec<RleToken>> {
    let mut output = Vec::new();
    output
        .try_reserve_exact(MAX_DYNAMIC_CODE_LENGTH_COUNT)
        .ok()?;
    let mut index = 0;
    while index < lengths.len() {
        let value = lengths[index];
        let mut run = 1;
        while index + run < lengths.len() && lengths[index + run] == value {
            run += 1;
        }
        index += run;

        if value == 0 {
            if !options.no_long_zero_repeat {
                let mut count = 138;
                while count >= 11 {
                    if run >= count {
                        output.push(RleToken {
                            symbol: 18,
                            extra: (count - 11) as u8,
                        });
                        run -= count;
                    } else {
                        count -= 1;
                    }
                }
            }
            if !options.no_zero_repeat {
                let mut count = 10;
                while count >= 3 {
                    if run >= count {
                        output.push(RleToken {
                            symbol: 17,
                            extra: (count - 3) as u8,
                        });
                        run -= count;
                    } else {
                        count -= 1;
                    }
                }
            }
        }

        if !options.no_repeat && run != 0 && (!options.no_repeat_zeros || value != 0) {
            output.push(RleToken {
                symbol: value,
                extra: 0,
            });
            run -= 1;
            let mut count = 6;
            while count >= 3 {
                if options.special_repeat && options.use_eight && run == 8 {
                    output.push(RleToken {
                        symbol: 16,
                        extra: 1,
                    });
                    output.push(RleToken {
                        symbol: 16,
                        extra: 1,
                    });
                    run -= 8;
                    break;
                }
                if options.special_repeat && options.use_seven && run == 7 {
                    output.push(RleToken {
                        symbol: 16,
                        extra: 1,
                    });
                    output.push(RleToken {
                        symbol: 16,
                        extra: 0,
                    });
                    run -= 7;
                    break;
                }
                if run >= count {
                    output.push(RleToken {
                        symbol: 16,
                        extra: (count - 3) as u8,
                    });
                    run -= count;
                } else {
                    count -= 1;
                }
            }
        }

        output.extend(
            std::iter::repeat(RleToken {
                symbol: value,
                extra: 0,
            })
            .take(run),
        );
    }
    (output.len() <= MAX_DYNAMIC_CODE_LENGTH_COUNT).then_some(output)
}

fn plan_for_trimmed_lengths(
    literal_lengths: &[u8],
    distance_lengths: &[u8],
    data_bits: u64,
    exhaustive: bool,
    rle_mask: u8,
) -> Option<DynamicPlan> {
    plan_for_trimmed_lengths_uncached(
        literal_lengths,
        distance_lengths,
        data_bits,
        exhaustive,
        rle_mask,
    )
}

#[derive(Default)]
struct HeaderPlanSearch {
    heap_scratch: DefloptHeapScratch,
    best: Option<DynamicPlan>,
}

/// Fully reprice a legal advertised alphabet span, including trailing zeros.
/// Joint tree/header search holds these counts fixed while choosing lengths.
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
    let decoded_length_count = literal_lengths.len().checked_add(distance_lengths.len())?;
    if decoded_length_count > MAX_DYNAMIC_CODE_LENGTH_COUNT {
        return None;
    }
    let mut decoded_length_storage = [0_u8; MAX_DYNAMIC_CODE_LENGTH_COUNT];
    decoded_length_storage[..literal_lengths.len()].copy_from_slice(literal_lengths);
    decoded_length_storage[literal_lengths.len()..decoded_length_count]
        .copy_from_slice(distance_lengths);
    let decoded_lengths = &decoded_length_storage[..decoded_length_count];
    let mut search = HeaderPlanSearch::default();
    for rle in rle_seed_candidates(decoded_lengths, rle_mask) {
        consider_rle(
            data_bits,
            literal_lengths,
            distance_lengths,
            decoded_lengths,
            rle,
            exhaustive,
            &mut search,
        );
    }
    search.best
}

#[derive(Default)]
struct RleSeedOpportunities {
    balanced_repeat: bool,
    zero_repeat: [bool; 4],
    zero_17: bool,
    zero_18: bool,
}

fn rle_seed_opportunities(lengths: &[u8]) -> RleSeedOpportunities {
    let mut opportunities = RleSeedOpportunities::default();
    let mut index = 0;
    while index < lengths.len() {
        let value = lengths[index];
        let mut run = 1;
        while index + run < lengths.len() && lengths[index + run] == value {
            run += 1;
        }
        index += run;

        if value != 0 {
            let repeated = run.saturating_sub(1);
            opportunities.balanced_repeat |= repeated >= 7 && matches!(repeated % 6, 1 | 2);
            continue;
        }

        opportunities.zero_17 |= run >= 3;
        opportunities.zero_18 |= run >= 11;
        for no_17 in [false, true] {
            for no_18 in [false, true] {
                let mut remaining = run;
                while !no_18 && remaining >= 11 {
                    remaining -= remaining.min(138);
                }
                while !no_17 && remaining >= 3 {
                    remaining -= remaining.min(10);
                }
                let variant = usize::from(no_17) | (usize::from(no_18) << 1);
                opportunities.zero_repeat[variant] |= remaining >= 4;
            }
        }
    }
    opportunities
}

/// Produce each distinct RLE seed once, retaining the historical first-seen
/// order across mask and additive spelling families.
fn rle_seed_candidates(decoded_lengths: &[u8], rle_mask: u8) -> Vec<Vec<RleToken>> {
    let opportunities = rle_seed_opportunities(decoded_lengths);
    // Bit zero also admits the additive balanced/zero families, so preserve
    // it even when disabling repeat-16 would not change the greedy spelling.
    let relevant_mask =
        1 | (u8::from(opportunities.zero_17) << 1) | (u8::from(opportunities.zero_18) << 2);
    let mut seen_masks = [false; 8];
    let mut candidates = Vec::new();
    for mask in 0..8 {
        if rle_mask & (1 << mask) == 0 {
            continue;
        }
        let mask = mask & relevant_mask;
        if seen_masks[usize::from(mask)] {
            continue;
        }
        seen_masks[usize::from(mask)] = true;
        let no_16 = mask & 1 != 0;
        let no_17 = mask & 2 != 0;
        let no_18 = mask & 4 != 0;
        let rle = greedy_rle(decoded_lengths, no_16, no_17, no_18);
        push_unique(&mut candidates, rle);

        // A greedy six-length repeat can leave one or two explicit lengths.
        // Columbo's additive packer generalizes deft4j's 4+3 and 4+4 OHH
        // alternatives so those tails can use repeats instead of literals.
        if !no_16 && opportunities.balanced_repeat {
            let balanced = balanced_repeat_rle(decoded_lengths, no_17, no_18);
            push_unique(&mut candidates, balanced);
        }

        // Inspired by deft4j's ability to continue a zero with symbol 16,
        // Columbo generalizes the residual split. This source-shaped header
        // is distinct from both the exact deft4j packer and ordinary masks.
        if !no_16 {
            let zero_variant = usize::from(no_17) | (usize::from(no_18) << 1);
            if opportunities.zero_repeat[zero_variant] {
                let zero_repeat = columbo_zero_repeat_rle(decoded_lengths, no_17, no_18);
                push_unique(&mut candidates, zero_repeat);
            }
        }
    }
    candidates
}

/// A terminal sibling may spend a small payload tax to make the length list
/// easier to describe. Do not relax the ordinary greedy swap guard: changing
/// those intermediate winners would redirect existing search descendants.
const MAX_SWAP_PAYLOAD_TAX: i64 = 18;
const MAX_TAXED_SWAPS_PER_ALPHABET: usize = 32;

/// Exact change in adjacent transitions, including the LL/DD seam. Only the
/// four edges touching the swapped positions can change; adjacent swaps share
/// an edge, which must be counted once.
fn swap_transitions_removed(lengths: &[u8], a: usize, b: usize) -> i64 {
    let edges = [a, a + 1, b, b + 1];
    let swapped = |i| {
        if i == a {
            lengths[b]
        } else if i == b {
            lengths[a]
        } else {
            lengths[i]
        }
    };
    let mut removed = 0;
    for (index, &end) in edges.iter().enumerate() {
        if end == 0 || end >= lengths.len() || edges[..index].contains(&end) {
            continue;
        }
        removed += i64::from(lengths[end - 1] != lengths[end])
            - i64::from(swapped(end - 1) != swapped(end));
    }
    removed
}

/// Keep a small deterministic menu. Transition reduction is a search heuristic,
/// not a lower bound; only the complete header price can establish a saving.
fn payload_taxed_swap_proposals(
    lengths: &[u8],
    offset: usize,
    frequencies: &[u32],
    stop: &mut SearchStop<'_>,
) -> Option<Vec<(i64, i64, usize, usize)>> {
    let count = frequencies.len().min(lengths.len().checked_sub(offset)?);
    let mut proposals = Vec::new();
    proposals
        .try_reserve_exact(MAX_TAXED_SWAPS_PER_ALPHABET + 1)
        .ok()?;
    for a in 0..count {
        if stop.reached() {
            return None;
        }
        let length_a = lengths[offset + a];
        if length_a == 0 {
            continue;
        }
        for b in a + 1..count {
            let length_b = lengths[offset + b];
            if length_b == 0 || length_a == length_b {
                continue;
            }
            let tax = length_swap_delta(frequencies[a], frequencies[b], length_a, length_b);
            if !(1..=MAX_SWAP_PAYLOAD_TAX).contains(&tax) {
                continue;
            }
            let removed = swap_transitions_removed(lengths, offset + a, offset + b);
            if removed <= 0 {
                continue;
            }
            let proposal = (tax - 3 * removed, tax, a, b);
            let at = proposals.binary_search(&proposal).unwrap_or_else(|at| at);
            if at < MAX_TAXED_SWAPS_PER_ALPHABET {
                proposals.insert(at, proposal);
                proposals.truncate(MAX_TAXED_SWAPS_PER_ALPHABET);
            }
        }
    }
    Some(proposals)
}

/// Reprice the unchanged tree and bounded, positive-payload permutations of
/// each alphabet. Every proposal is a sibling of the same parent. Nonzero
/// length swaps preserve support, maximum depth and the complete Kraft sum;
/// the distance frequency slice excludes reserved symbols 30 and 31.
pub(crate) fn plan_payload_header_tradeoff(
    block: &super::model::ParsedBlock,
    strict: bool,
    prices_left: &mut usize,
    stop: &mut SearchStop<'_>,
) -> Option<DynamicPlan> {
    if *prices_left == 0 || stop.reached() {
        return None;
    }
    let original = block.original_dynamic.as_ref()?;
    if strict && !original.has_strictly_compatible_huffman_codes() {
        return None;
    }
    let literal = &original.literal_lengths[..trim_literal(&original.literal_lengths)];
    let distance = &original.distance_lengths[..trim_distance(&original.distance_lengths)];
    let data_bits = token_bits(&block.tokens, literal, distance)?;
    let original_bits = dynamic_bits(data_bits, original)?;
    let mut best = None;
    let mut best_bits = original_bits;
    *prices_left -= 1;
    if let Some(plan) = plan_for_trimmed_lengths_uncached(literal, distance, data_bits, true, 0xff)
    {
        if plan.bits < best_bits && (!strict || plan.has_strictly_compatible_huffman_codes()) {
            best_bits = plan.bits;
            best = Some(plan);
        }
    }
    let mut lengths = [0_u8; MAX_DYNAMIC_CODE_LENGTH_COUNT];
    let count = literal.len().checked_add(distance.len())?;
    let lengths = lengths.get_mut(..count)?;
    lengths[..literal.len()].copy_from_slice(literal);
    lengths[literal.len()..].copy_from_slice(distance);
    for (offset, frequencies) in [
        (0, &block.literal_frequencies[..literal.len()]),
        (literal.len(), block.distance_frequencies.as_slice()),
    ] {
        if *prices_left == 0 || stop.reached() {
            break;
        }
        let Some(proposals) = payload_taxed_swap_proposals(lengths, offset, frequencies, stop)
        else {
            break;
        };
        for (_, tax, a, b) in proposals {
            if *prices_left == 0 || stop.reached() {
                break;
            }
            lengths.swap(offset + a, offset + b);
            *prices_left -= 1;
            let plan = plan_for_trimmed_lengths_uncached(
                &lengths[..literal.len()],
                &lengths[literal.len()..],
                data_bits.checked_add(tax as u64)?,
                true,
                0xff,
            );
            lengths.swap(offset + a, offset + b);
            if let Some(plan) = plan {
                if plan.bits < best_bits
                    && (!strict || plan.has_strictly_compatible_huffman_codes())
                {
                    best_bits = plan.bits;
                    best = Some(plan);
                }
            }
        }
    }
    best
}

/// Advertise unused literal/length symbols to change the zero run at the
/// LL/DD seam. Zero lengths consume no code space and change no codeword.
/// Search every legal span, preserving the parent's distance span and all
/// nonzero lengths. Full header feedback matters: the existing CL tree alone
/// can miss a gain, even when only one zero is inserted.
pub(crate) fn plan_literal_span(
    block: &super::model::ParsedBlock,
    strict: bool,
    prices_left: &mut usize,
    stop: &mut SearchStop<'_>,
) -> Option<DynamicPlan> {
    if *prices_left == 0 || stop.reached() {
        return None;
    }
    let original = block.original_dynamic.as_ref()?;
    if strict && !original.has_strictly_compatible_huffman_codes() {
        return None;
    }
    let minimum = trim_literal(&original.literal_lengths);
    let mut literal = [0_u8; 286];
    if minimum >= literal.len() {
        return None;
    }
    literal[..minimum].copy_from_slice(&original.literal_lengths[..minimum]);
    let distance = &original.distance_lengths;
    let data_bits = token_bits(&block.tokens, &literal[..minimum], distance)?;
    let mut best_bits = dynamic_bits(data_bits, original)?;
    let mut best = None;
    for span in minimum..=literal.len() {
        if *prices_left == 0 || stop.reached() {
            break;
        }
        if span == original.hlit {
            continue;
        }
        *prices_left -= 1;
        // This pricer uses the supplied spans verbatim. Calling the ordinary
        // explicit-length wrapper here would trim away the zero padding.
        if let Some(plan) =
            plan_for_trimmed_lengths_uncached(&literal[..span], distance, data_bits, true, 0xff)
        {
            if plan.bits < best_bits && (!strict || plan.has_strictly_compatible_huffman_codes()) {
                best_bits = plan.bits;
                best = Some(plan);
            }
        }
    }
    best
}

fn length_swap_delta(frequency_a: u32, frequency_b: u32, length_a: u8, length_b: u8) -> i64 {
    let frequency_a = i64::from(frequency_a);
    let frequency_b = i64::from(frequency_b);
    let length_a = i64::from(length_a);
    let length_b = i64::from(length_b);
    frequency_a * (length_b - length_a) + frequency_b * (length_a - length_b)
}

#[allow(clippy::too_many_arguments)]
fn improve_by_length_swaps(
    tokens: &[Token],
    literal_frequencies: &[u32; 286],
    distance_frequencies: &[u32; 30],
    seed: &DynamicPlan,
    exhaustive: bool,
    stop: &mut SearchStop<'_>,
    best: &mut Option<DynamicPlan>,
) {
    let mut literal_lengths = vec![0_u8; 286];
    let mut distance_lengths = vec![0_u8; 30];
    if seed.literal_lengths.len() > literal_lengths.len()
        || seed.distance_lengths.len() > distance_lengths.len()
    {
        return;
    }
    literal_lengths[..seed.literal_lengths.len()].copy_from_slice(&seed.literal_lengths);
    distance_lengths[..seed.distance_lengths.len()].copy_from_slice(&seed.distance_lengths);

    // Columbo's greedy swap search uses only the ordinary RLE spelling while
    // deciding which length exchange to commit. It then scores the complete
    // header family. DeflOpt 2.07 has no finished-tree swap search.
    let Some(mut current) = plan_for_explicit_lengths_masked(
        tokens,
        &literal_lengths,
        &distance_lengths,
        exhaustive,
        0x01,
    ) else {
        return;
    };
    keep_better(best, current.clone());

    improve_one_tree_by_swaps(
        tokens,
        literal_frequencies,
        &mut literal_lengths,
        &mut distance_lengths,
        true,
        exhaustive,
        stop,
        &mut current,
        best,
    );
    improve_one_tree_by_swaps(
        tokens,
        distance_frequencies,
        &mut literal_lengths,
        &mut distance_lengths,
        false,
        exhaustive,
        stop,
        &mut current,
        best,
    );
}

#[allow(clippy::too_many_arguments)]
fn improve_one_tree_by_swaps(
    tokens: &[Token],
    frequencies: &[u32],
    literal_lengths: &mut [u8],
    distance_lengths: &mut [u8],
    literal_tree: bool,
    exhaustive: bool,
    stop: &mut SearchStop<'_>,
    current: &mut DynamicPlan,
    best: &mut Option<DynamicPlan>,
) {
    const MAX_PASSES: usize = 12;

    for _ in 0..MAX_PASSES {
        if stop.reached() {
            break;
        }

        let mut selected_pair = None;
        let mut selected_plan = current.clone();
        for a in 0..frequencies.len() {
            if stop.reached() {
                return;
            }
            let length_a = if literal_tree {
                literal_lengths[a]
            } else {
                distance_lengths[a]
            };
            if length_a == 0 {
                continue;
            }

            for b in (a + 1)..frequencies.len() {
                let length_b = if literal_tree {
                    literal_lengths[b]
                } else {
                    distance_lengths[b]
                };
                if length_b == 0 || length_a == length_b {
                    continue;
                }
                if length_swap_delta(frequencies[a], frequencies[b], length_a, length_b) > 0 {
                    continue;
                }

                if literal_tree {
                    literal_lengths.swap(a, b);
                } else {
                    distance_lengths.swap(a, b);
                }
                let candidate = plan_for_explicit_lengths_masked(
                    tokens,
                    literal_lengths,
                    distance_lengths,
                    exhaustive,
                    0x01,
                );
                if literal_tree {
                    literal_lengths.swap(a, b);
                } else {
                    distance_lengths.swap(a, b);
                }

                if let Some(candidate) = candidate {
                    if candidate.bits < selected_plan.bits {
                        selected_pair = Some((a, b));
                        selected_plan = candidate;
                    }
                }
            }
        }

        let Some((a, b)) = selected_pair else {
            break;
        };
        if literal_tree {
            literal_lengths.swap(a, b);
        } else {
            distance_lengths.swap(a, b);
        }
        *current = selected_plan;
        keep_better(best, current.clone());

        if let Some(full_plan) =
            plan_for_explicit_lengths(tokens, literal_lengths, distance_lengths, exhaustive)
        {
            keep_better(best, full_plan);
        }
    }
}

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
    initial_rle: Vec<RleToken>,
    exhaustive: bool,
    search: &mut HeaderPlanSearch,
) {
    // Each seed is consumed by exactly one feedback lineage. Taking ownership
    // reuses its allocation through all four passes instead of cloning every
    // candidate immediately after `rle_seed_candidates` built it.
    let mut rle = initial_rle;

    // From each Columbo RLE-mask seed, price a DeflOpt-derived local candidate:
    // build DeflOpt's height-tied code-length tree, replace repeat tokens that
    // are locally dearer than explicit lengths, then rebuild. Every rewrite
    // strictly reduces the finite repeat rank, so this remains bounded. It is
    // not DeflOpt's complete state-feedback route.
    let mut frequencies = rle_frequencies(&rle);
    let mut prescored_deflopt_trees = [None; 4];
    for (variant, tree) in prescored_deflopt_trees.iter_mut().enumerate() {
        *tree = consider_columbo_deflopt_local_rewrite(
            data_bits,
            literal_lengths,
            distance_lengths,
            &rle,
            &frequencies,
            variant as u32,
            search,
        );
    }

    // This four-pass, best-intermediate loop is Columbo's composite header
    // route. Its numeric bound is inspired by Defluff, but Defluff always emits
    // its fourth pass and neither stops early nor retains earlier winners.
    let passes = 4;
    // Reuse this tiny candidate arena across feedback passes. Every candidate
    // is an inline array, so clearing it releases no state needed by a plan.
    let candidate_capacity = if exhaustive { 18 } else { 6 };
    let mut code_length_candidates = Vec::with_capacity(candidate_capacity);
    for pass in 0..passes {
        let variants = 4;
        // The code-length alphabet is always exactly nineteen symbols. Store
        // its candidates inline so each builder does not allocate a Vec only
        // for the result to be copied into DynamicPlan's fixed-size array.
        code_length_candidates.clear();
        for variant in 0..variants {
            let mut order_lengths = [0_u8; 19];
            make_lengths_order_heap_into_with_scratch(
                &frequencies,
                &mut order_lengths,
                7,
                variant,
                &mut search.heap_scratch,
            );
            push_unique(&mut code_length_candidates, order_lengths);
            // The deft4j tree's PriorityQueue-compatible heap is expensive
            // across the full data alphabets, but the code-length alphabet has
            // only nineteen symbols. Keeping it in Columbo's ordinary header
            // repack closes RLE gaps without broadening token search.
            if variant == 0 {
                let mut deft4j_lengths = [0_u8; 19];
                make_lengths_deft4j_java_heap_into(&frequencies, &mut deft4j_lengths, 7);
                consider_deft4j_pruned_header(
                    data_bits,
                    literal_lengths,
                    distance_lengths,
                    &rle,
                    &deft4j_lengths,
                    &mut search.best,
                );
                push_unique(&mut code_length_candidates, deft4j_lengths);
                // This is Columbo's generic code-length tree with Defluff's
                // limiter, not Defluff's complete tree builder. Pricing the
                // hybrid once is inexpensive when the data trees are fixed.
                let mut defluff_limited_lengths = [0_u8; 19];
                make_lengths_columbo_defluff_limited_into(
                    &frequencies,
                    &mut defluff_limited_lengths,
                    7,
                    0,
                );
                push_unique(&mut code_length_candidates, defluff_limited_lengths);
            }
            if exhaustive {
                // The local DeflOpt feedback route already built and scored
                // this exact first-state tree. Reuse it in the composite
                // feedback ranking instead of rebuilding and rescoring it.
                if pass == 0 {
                    if let Some(lengths) = prescored_deflopt_trees[variant as usize] {
                        push_unique(&mut code_length_candidates, lengths);
                    }
                } else {
                    let mut deflopt_lengths = [0_u8; 19];
                    make_lengths_deflopt_heap_into_with_scratch(
                        &frequencies,
                        &mut deflopt_lengths,
                        7,
                        variant,
                        &mut search.heap_scratch,
                    );
                    push_unique(&mut code_length_candidates, deflopt_lengths);
                }
                let mut columbo_lengths = [0_u8; 19];
                make_lengths_into(&frequencies, &mut columbo_lengths, 7, variant);
                push_unique(&mut code_length_candidates, columbo_lengths);

                let mut defluff_lengths = [0_u8; 19];
                make_lengths_defluff_exact_into(&frequencies, &mut defluff_lengths, 7, variant);
                push_unique(&mut code_length_candidates, defluff_lengths);
            }
        }

        let mut feedback_tree = None;
        let mut feedback_cost = INF;
        for code_length_lengths in code_length_candidates.iter().copied() {
            if !code_length_tree_shape_is_valid(&code_length_lengths) {
                continue;
            }
            let was_prescored = pass == 0
                && prescored_deflopt_trees
                    .iter()
                    .flatten()
                    .any(|tree| tree == &code_length_lengths);
            let valid_header = was_prescored
                || consider_dynamic_header(
                    data_bits,
                    literal_lengths,
                    distance_lengths,
                    &rle,
                    &frequencies,
                    code_length_lengths,
                    &mut search.best,
                )
                .is_some();
            if valid_header {
                let candidate_cost = rle_frequency_cost(&frequencies, &code_length_lengths);
                if candidate_cost < feedback_cost {
                    feedback_cost = candidate_cost;
                    feedback_tree = Some(code_length_lengths);
                }

                // Reassigning the same code-length histogram preserves a
                // valid canonical tree. DeflOpt pairs shorter codes with the
                // more frequent RLE symbols; the changed HCLEN tail can make
                // this worthwhile even when weighted symbol cost ties.
                if !was_prescored {
                    let mut reordered = code_length_lengths;
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
        let mut rewritten = rewrite_rle_deflopt_local(&rle, &tree);
        if exhaustive {
            if let Some(shortest) = shortest_rle(decoded_lengths, &tree) {
                if rewritten.as_ref().map_or(true, |local| {
                    rle_cost(&shortest, &tree) < rle_cost(local, &tree)
                }) {
                    rewritten = Some(shortest);
                }
            }
        }
        let Some(rewritten) = rewritten else {
            break;
        };
        if rewritten == rle {
            break;
        }
        rle = rewritten;
        frequencies = rle_frequencies(&rle);
    }
}

/// Add deft4j's code-length tree and equal-cost RLE prune to a Columbo header.
///
/// The deft4j route first expands repeat tokens whose explicit spelling is no
/// dearer under the current code-length tree, then rebuilds that tiny tree.
/// Although the first rewrite may tie, the changed frequencies can shorten the
/// rebuilt header. Only the dynamic header changes; data symbols and LZ77
/// tokens remain untouched.
fn consider_deft4j_pruned_header(
    data_bits: u64,
    literal_lengths: &[u8],
    distance_lengths: &[u8],
    rle: &[RleToken],
    deft4j_lengths: &[u8],
    best: &mut Option<DynamicPlan>,
) {
    if deft4j_lengths.len() != 19 || !code_length_tree_shape_is_valid(deft4j_lengths) {
        return;
    }
    let mut code_length_lengths = [0_u8; 19];
    code_length_lengths.copy_from_slice(deft4j_lengths);
    if rle
        .iter()
        .any(|token| code_length_lengths[usize::from(token.symbol)] == 0)
    {
        return;
    }

    let Some(pruned) = rewrite_rle_deft4j_literals(rle, &code_length_lengths, true) else {
        return;
    };
    let frequencies = rle_frequencies(&pruned);
    let mut code_length_lengths = [0_u8; 19];
    make_lengths_deft4j_java_heap_into(&frequencies, &mut code_length_lengths, 7);
    if !code_length_tree_shape_is_valid(&code_length_lengths) {
        return;
    }
    if pruned
        .iter()
        .any(|token| code_length_lengths[usize::from(token.symbol)] == 0)
    {
        return;
    }

    consider_dynamic_header(
        data_bits,
        literal_lengths,
        distance_lengths,
        &pruned,
        &frequencies,
        code_length_lengths,
        best,
    );

    // deft4j's final optimiseHeader step expands only strictly dearer repeats
    // under the rebuilt tree and deliberately keeps that tree unchanged.
    if let Some(optimized) = rewrite_rle_deft4j_literals(&pruned, &code_length_lengths, false) {
        let optimized_frequencies = rle_frequencies(&optimized);
        consider_dynamic_header(
            data_bits,
            literal_lengths,
            distance_lengths,
            &optimized,
            &optimized_frequencies,
            code_length_lengths,
            best,
        );
    }
}

/// Price Columbo's bounded DeflOpt-derived local rewrite/rebuild candidate.
///
/// DeflOpt supplies the strict local repeat rewrite and height-tied tree
/// rebuild. Columbo starts from each of its own RLE-mask seeds and also prices
/// a frequency-reassigned tree as an additive candidate, rather than feeding
/// every state through DeflOpt's complete bounded feedback route. The data-code
/// lengths and LZ77 parse remain fixed.
fn consider_columbo_deflopt_local_rewrite(
    data_bits: u64,
    literal_lengths: &[u8],
    distance_lengths: &[u8],
    initial_rle: &[RleToken],
    initial_frequencies: &[u32; 19],
    variant: u32,
    search: &mut HeaderPlanSearch,
) -> Option<[u8; 19]> {
    // Score the seed by reference. Allocate only if the local rewrite finds a
    // genuinely different next state; four variants otherwise cloned this
    // same RLE before discovering that no rewrite was possible.
    let mut rewritten_rle: Option<Vec<RleToken>> = None;
    let mut frequencies = *initial_frequencies;
    let mut rank = repeat_rank(initial_rle);
    let mut initial_tree = None;

    loop {
        let rle = rewritten_rle.as_deref().unwrap_or(initial_rle);
        let mut code_length_lengths = [0_u8; 19];
        make_lengths_deflopt_heap_into_with_scratch(
            &frequencies,
            &mut code_length_lengths,
            7,
            variant,
            &mut search.heap_scratch,
        );
        if !code_length_tree_shape_is_valid(&code_length_lengths) {
            return initial_tree;
        }

        let Some(_) = consider_dynamic_header(
            data_bits,
            literal_lengths,
            distance_lengths,
            rle,
            &frequencies,
            code_length_lengths,
            &mut search.best,
        ) else {
            return initial_tree;
        };
        initial_tree.get_or_insert(code_length_lengths);

        // The pair-swap pass preserves the code-length tree histogram while
        // assigning its shorter codes to more frequent RLE symbols. Retain it
        // as an additive candidate; the local rewrite/rebuild loop follows the
        // unmodified DeflOpt tree, which keeps tie behaviour deterministic.
        let mut reordered = code_length_lengths;
        if reorder_code_length_lengths(&mut reordered, &frequencies) {
            consider_dynamic_header(
                data_bits,
                literal_lengths,
                distance_lengths,
                rle,
                &frequencies,
                reordered,
                &mut search.best,
            );
        }

        if rank == 0 {
            break;
        }
        let Some(rewritten) = rewrite_rle_deflopt_local(rle, &code_length_lengths) else {
            break;
        };
        let next_rank = repeat_rank(&rewritten);
        if next_rank >= rank {
            break;
        }

        // Price the rewrite once under the existing tree before rebuilding.
        // This is a distinct legal header and occasionally beats both adjacent
        // fixed-point states by one or two bits.
        let rewritten_frequencies = rle_frequencies(&rewritten);
        consider_dynamic_header(
            data_bits,
            literal_lengths,
            distance_lengths,
            &rewritten,
            &rewritten_frequencies,
            code_length_lengths,
            &mut search.best,
        );

        rewritten_rle = Some(rewritten);
        frequencies = rewritten_frequencies;
        rank = next_rank;
    }
    initial_tree
}

/// Replace repeat tokens only when their current-tree local cost decreases.
fn rewrite_rle_deflopt_local(
    input: &[RleToken],
    code_length_lengths: &[u8; 19],
) -> Option<Vec<RleToken>> {
    let mut output = Vec::new();
    output.try_reserve(input.len()).ok()?;
    let mut previous = None;
    let mut changed = false;

    for &token in input {
        match token.symbol {
            value @ 0..=15 => {
                output.push(token);
                previous = Some(value);
            }
            18 => {
                let count = usize::from(token.extra) + 11;
                if explicit_rle_cost(code_length_lengths, 0, count)
                    < rle_symbol_cost(code_length_lengths, 18)
                {
                    output.extend(
                        std::iter::repeat(RleToken {
                            symbol: 0,
                            extra: 0,
                        })
                        .take(count),
                    );
                    changed = true;
                } else {
                    output.push(token);
                }
                previous = Some(0);
            }
            17 => {
                let count = usize::from(token.extra) + 3;
                if previous == Some(0)
                    && token.extra <= 3
                    && code_length_lengths[16] != 0
                    && code_length_lengths[17] != 0
                    && code_length_lengths[16] <= code_length_lengths[17]
                {
                    output.push(RleToken {
                        symbol: 16,
                        extra: token.extra,
                    });
                    changed = true;
                } else if explicit_rle_cost(code_length_lengths, 0, count)
                    < rle_symbol_cost(code_length_lengths, 17)
                {
                    output.extend(
                        std::iter::repeat(RleToken {
                            symbol: 0,
                            extra: 0,
                        })
                        .take(count),
                    );
                    changed = true;
                } else {
                    output.push(token);
                }
                previous = Some(0);
            }
            16 => {
                let previous = previous?;
                let count = usize::from(token.extra) + 3;
                if explicit_rle_cost(code_length_lengths, previous, count)
                    < rle_symbol_cost(code_length_lengths, 16)
                {
                    output.extend(
                        std::iter::repeat(RleToken {
                            symbol: previous,
                            extra: 0,
                        })
                        .take(count),
                    );
                    changed = true;
                } else {
                    output.push(token);
                }
            }
            _ => return None,
        }
    }

    changed.then_some(output)
}

/// Expand repeat tokens that are dearer (or tied) under a fixed tree.
///
/// This mirrors deft4j's header-only prune. `include_equal` is used before a
/// deft4j tree rebuild because a tied local rewrite can change the next tree;
/// the final optimize-header pass accepts strict local savings only.
fn rewrite_rle_deft4j_literals(
    input: &[RleToken],
    code_length_lengths: &[u8; 19],
    include_equal: bool,
) -> Option<Vec<RleToken>> {
    let mut output = Vec::new();
    output.try_reserve(MAX_DYNAMIC_CODE_LENGTH_COUNT).ok()?;
    let mut previous = None;
    let mut changed = false;

    for &token in input {
        let (value, count) = match token.symbol {
            value @ 0..=15 => {
                output.push(token);
                previous = Some(value);
                continue;
            }
            16 => (previous?, usize::from(token.extra) + 3),
            17 => (0, usize::from(token.extra) + 3),
            18 => (0, usize::from(token.extra) + 11),
            _ => return None,
        };
        let explicit = explicit_rle_cost(code_length_lengths, value, count);
        let repeat = rle_symbol_cost(code_length_lengths, token.symbol);
        if explicit < repeat || (include_equal && explicit == repeat) {
            if output.len().checked_add(count)? > MAX_DYNAMIC_CODE_LENGTH_COUNT {
                return None;
            }
            output.extend(
                std::iter::repeat(RleToken {
                    symbol: value,
                    extra: 0,
                })
                .take(count),
            );
            changed = true;
        } else {
            output.push(token);
        }
        previous = Some(value);
    }
    changed.then_some(output)
}

fn explicit_rle_cost(lengths: &[u8; 19], symbol: u8, count: usize) -> u64 {
    let length = lengths[usize::from(symbol)];
    if length == 0 {
        INF
    } else {
        u64::from(length).saturating_mul(count as u64).min(INF)
    }
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

fn repeat_rank(rle: &[RleToken]) -> usize {
    rle.iter()
        .map(|token| match token.symbol {
            17 => 2,
            16 | 18 => 1,
            _ => 0,
        })
        .sum()
}

fn reorder_code_length_lengths(lengths: &mut [u8; 19], frequencies: &[u32; 19]) -> bool {
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

fn greedy_rle(lengths: &[u8], no_16: bool, no_17: bool, no_18: bool) -> Vec<RleToken> {
    let mut output = Vec::new();
    let mut index = 0;
    while index < lengths.len() {
        let value = lengths[index];
        let mut run = 1;
        while index + run < lengths.len() && lengths[index + run] == value {
            run += 1;
        }
        index += run;

        if value == 0 {
            while !no_18 && run >= 11 {
                let count = run.min(138);
                output.push(RleToken {
                    symbol: 18,
                    extra: (count - 11) as u8,
                });
                run -= count;
            }
            while !no_17 && run >= 3 {
                let count = run.min(10);
                output.push(RleToken {
                    symbol: 17,
                    extra: (count - 3) as u8,
                });
                run -= count;
            }
        } else if !no_16 && run >= 4 {
            output.push(RleToken {
                symbol: value,
                extra: 0,
            });
            run -= 1;
            while run >= 3 {
                let count = run.min(6);
                output.push(RleToken {
                    symbol: 16,
                    extra: (count - 3) as u8,
                });
                run -= count;
            }
        }
        output.extend(
            std::iter::repeat(RleToken {
                symbol: value,
                extra: 0,
            })
            .take(run),
        );
    }
    output
}

/// Columbo's generalized form of deft4j's balanced repeat-16 alternatives.
///
/// deft4j directly tries 4+3 and 4+4 for seven- and eight-value tails.
/// Columbo applies the same idea whenever greedy six-value chunks would leave
/// one or two explicit values.
fn balanced_repeat_rle(lengths: &[u8], no_17: bool, no_18: bool) -> Vec<RleToken> {
    let mut output = Vec::new();
    let mut index = 0;
    while index < lengths.len() {
        let value = lengths[index];
        let mut run = 1;
        while index + run < lengths.len() && lengths[index + run] == value {
            run += 1;
        }
        index += run;

        if value == 0 {
            while !no_18 && run >= 11 {
                let count = run.min(138);
                output.push(RleToken {
                    symbol: 18,
                    extra: (count - 11) as u8,
                });
                run -= count;
            }
            while !no_17 && run >= 3 {
                let count = run.min(10);
                output.push(RleToken {
                    symbol: 17,
                    extra: (count - 3) as u8,
                });
                run -= count;
            }
        } else if run >= 4 {
            output.push(RleToken {
                symbol: value,
                extra: 0,
            });
            run -= 1;
            while run >= 3 {
                let count = if matches!(run % 6, 1 | 2) && run >= 7 {
                    4
                } else {
                    run.min(6)
                };
                output.push(RleToken {
                    symbol: 16,
                    extra: (count - 3) as u8,
                });
                run -= count;
            }
        }
        output.extend(
            std::iter::repeat(RleToken {
                symbol: value,
                extra: 0,
            })
            .take(run),
        );
    }
    output
}

/// Build Columbo's deft4j-inspired residual-zero repeat candidate.
fn columbo_zero_repeat_rle(lengths: &[u8], no_17: bool, no_18: bool) -> Vec<RleToken> {
    let mut output = Vec::new();
    let mut index = 0;
    while index < lengths.len() {
        let value = lengths[index];
        let mut run = 1;
        while index + run < lengths.len() && lengths[index + run] == value {
            run += 1;
        }
        index += run;

        if value == 0 {
            while !no_18 && run >= 11 {
                let count = run.min(138);
                output.push(RleToken {
                    symbol: 18,
                    extra: (count - 11) as u8,
                });
                run -= count;
            }
            while !no_17 && run >= 3 {
                let count = run.min(10);
                output.push(RleToken {
                    symbol: 17,
                    extra: (count - 3) as u8,
                });
                run -= count;
            }
        }

        if run >= 4 {
            output.push(RleToken {
                symbol: value,
                extra: 0,
            });
            run -= 1;
            while run >= 3 {
                let count = if matches!(run % 6, 1 | 2) && run >= 7 {
                    4
                } else {
                    run.min(6)
                };
                output.push(RleToken {
                    symbol: 16,
                    extra: (count - 3) as u8,
                });
                run -= count;
            }
        }
        output.extend(
            std::iter::repeat(RleToken {
                symbol: value,
                extra: 0,
            })
            .take(run),
        );
    }
    output
}

fn rle_frequencies(rle: &[RleToken]) -> [u32; 19] {
    let mut frequencies = [0_u32; 19];
    for token in rle {
        frequencies[usize::from(token.symbol)] += 1;
    }
    frequencies
}

fn trim_code_lengths(lengths: &[u8; 19]) -> usize {
    (4..19)
        .rev()
        .find(|&index| lengths[CODE_LENGTH_ORDER[index]] != 0)
        .map_or(4, |index| index + 1)
}

/// Score a candidate header from the nineteen-symbol RLE histogram.
///
/// Header search prices many code-length trees for the same RLE spelling.
/// Walking at most nineteen frequencies is equivalent to rescanning as many
/// as 316 RLE tokens for every tree, while keeping the repeat extra bits exact.
fn rle_frequency_cost(frequencies: &[u32; 19], lengths: &[u8; 19]) -> u64 {
    frequencies
        .iter()
        .enumerate()
        .filter(|&(_, &frequency)| frequency != 0)
        .map(|(symbol, &frequency)| {
            rle_symbol_cost(lengths, symbol as u8).saturating_mul(u64::from(frequency))
        })
        .fold(0_u64, |sum, value| sum.saturating_add(value).min(INF))
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

/// Materialize owned plan vectors only when an exactly scored header wins.
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
    if best.as_ref().map_or(true, |current| bits < current.bits) {
        *best = Some(DynamicPlan {
            literal_lengths: literal_lengths.to_vec(),
            distance_lengths: distance_lengths.to_vec(),
            code_length_lengths,
            rle: rle.to_vec(),
            hlit: literal_lengths.len(),
            hdist: distance_lengths.len(),
            hclen,
            bits,
        });
    }
    Some(bits)
}

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

fn rle_extra_bits(symbol: u8) -> u64 {
    match symbol {
        16 => 2,
        17 => 3,
        18 => 7,
        _ => 0,
    }
}

fn rle_cost(rle: &[RleToken], lengths: &[u8; 19]) -> u64 {
    rle.iter()
        .map(|token| rle_symbol_cost(lengths, token.symbol))
        .fold(0_u64, |sum, value| sum.saturating_add(value).min(INF))
}

/// Find the cheapest valid RLE stream under a fixed code-length tree.
pub(super) fn shortest_rle(lengths: &[u8], costs: &[u8; 19]) -> Option<Vec<RleToken>> {
    #[derive(Clone, Copy)]
    struct Step {
        next: usize,
        token: RleToken,
    }

    let mut best = vec![INF; lengths.len() + 1];
    let mut step = vec![None; lengths.len()];
    let mut run = 0;
    let mut zero_repeats = VecDeque::<usize>::new();
    best[lengths.len()] = 0;
    for index in (0..lengths.len()).rev() {
        if index + 1 < lengths.len() && lengths[index] == lengths[index + 1] {
            run += 1;
        } else {
            run = 1;
            zero_repeats.clear();
        }

        // Symbol 18 charges the same bits for every repeat from 11 to 138.
        // Keep the cheapest suffix in that sliding window instead of scanning
        // up to 128 suffixes again at each zero. Each end enters and leaves
        // the deque at most once. Newer ends are smaller, so removing equal
        // costs retains the historical preference for the shortest repeat.
        let zero_repeat = if lengths[index] == 0 && costs[18] != 0 && run >= 11 {
            let first_end = index + 11;
            let last_end = index + run.min(138);
            while zero_repeats.front().is_some_and(|&end| end > last_end) {
                zero_repeats.pop_front();
            }
            while zero_repeats
                .back()
                .is_some_and(|&end| best[end] >= best[first_end])
            {
                zero_repeats.pop_back();
            }
            zero_repeats.push_back(first_end);
            zero_repeats.front().map(|&end| end - index)
        } else {
            None
        };

        let mut consider = |count: usize, symbol: u8, extra: u8| {
            let code = costs[usize::from(symbol)];
            if code == 0 || index + count > lengths.len() {
                return;
            }
            let cost = u64::from(code)
                .saturating_add(rle_extra_bits(symbol))
                .saturating_add(best[index + count]);
            // Strict replacement retains Columbo's source-like transition
            // order on equal cost, matching the original Columbo C
            // implementation.
            if cost < best[index] {
                best[index] = cost;
                step[index] = Some(Step {
                    next: index + count,
                    token: RleToken { symbol, extra },
                });
            }
        };

        consider(1, lengths[index], 0);
        if index > 0 && lengths[index] == lengths[index - 1] {
            for count in 3..=run.min(6) {
                consider(count, 16, (count - 3) as u8);
            }
        }
        if lengths[index] == 0 {
            for count in 3..=run.min(10) {
                consider(count, 17, (count - 3) as u8);
            }
            if let Some(count) = zero_repeat {
                consider(count, 18, (count - 11) as u8);
            }
        }
    }
    if best[0] == INF {
        return None;
    }
    let mut output = Vec::new();
    let mut index = 0;
    while index < lengths.len() {
        let selected = step[index]?;
        output.push(selected.token);
        index = selected.next;
    }
    Some(output)
}

#[cfg(test)]
pub(crate) mod test_support;
#[cfg(test)]
mod tests;
