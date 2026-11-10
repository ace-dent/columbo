// SPDX-License-Identifier: MIT
// SPDX-FileCopyrightText: 2026 Andrew C.E. Dent <https://github.com/ace-dent>

//! Candidate data trees priced with complete headers: the block planner's
//! length-limited builder families, Defluff's exact trees at reduced depths,
//! the bounded-depth tree floor, and trees built from RLE-smoothed counts and
//! priced on the true counts (R14). Each alphabet's exact tree is built once
//! for every depth limit asked of it.

use super::plan_cache::HeaderPlanCache;
use super::{
    alphabet_payload_bits, ensure_code_symbols, ensure_distance_symbols, keep_better,
    plan_for_explicit_lengths_with_cost, plan_for_explicit_lengths_with_cost_cached, push_unique,
    token_bits_from_frequencies, FREQUENCY_ONLY_HEAP_VARIANT,
};
use crate::deflate::huffman::{
    huffman_code_lengths_are_valid, make_brotli_rle_pseudofrequencies,
    make_columbo_rle_pseudofrequencies, make_lengths, make_lengths_columbo_defluff_limited,
    make_lengths_deflopt_heap_into_with_scratch, make_lengths_defluff_exact,
    make_lengths_deft4j_java_heap, make_lengths_into, make_lengths_order_heap_into_with_scratch,
    make_lengths_zopfli_package_from, make_zopfli_rle_pseudofrequencies,
    payload_tree_shape_is_valid, DefluffExactTree, TreeBuildScratch,
};
use crate::deflate::model::{token_extra_bits, DynamicPlan, Token};
use crate::deflate::stop::SearchStop;
use crate::sort::SortShared;

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

pub(super) fn tree_candidates(frequencies: &[u32], max_bits: u8, exhaustive: bool) -> Vec<Vec<u8>> {
    let mut candidates = Vec::with_capacity(if exhaustive { 16 } else { 8 });
    let mut heap_scratch = TreeBuildScratch::default();
    // Family order matters: of two equal complete plans, the earlier wins.
    // Without `exhaustive`, only the DeflOpt and order-key heaps run (at most
    // seven trees per alphabet, so 49 pairs). A variant a build shows to build
    // its lengths is skipped: its tree is a copy, which `push_unique` would
    // discard.
    // Variants of each heap known to build a tree already listed.
    let mut listed = [0_u8; 2];
    for variant in 0..4 {
        if listed[0] & 1 << variant != 0 {
            continue;
        }
        let mut lengths = vec![0_u8; frequencies.len()];
        let build = make_lengths_deflopt_heap_into_with_scratch(
            frequencies,
            &mut lengths,
            max_bits,
            variant,
            variant == 0,
            &mut heap_scratch,
        );
        push_unique(&mut candidates, lengths);
        listed[0] |= build.deflopt_variants;
        listed[1] |= build.order_variants;
    }
    // Variant 3 breaks both ties by frequency alone, so its order-key tree is
    // the DeflOpt tree already listed, which `push_unique` would discard.
    for variant in 0..FREQUENCY_ONLY_HEAP_VARIANT {
        if listed[1] & 1 << variant != 0 {
            continue;
        }
        let mut lengths = vec![0_u8; frequencies.len()];
        let build = make_lengths_order_heap_into_with_scratch(
            frequencies,
            &mut lengths,
            max_bits,
            variant,
            variant == 0,
            &mut heap_scratch,
        );
        push_unique(&mut candidates, lengths);
        listed[1] |= build.order_variants;
    }

    if exhaustive {
        // Defluff's limited builder starts from the variant-0 tree and differs
        // only when that tree needs limiting, so an unrepaired one is its tree.
        let mut unlimited = None;
        for variant in 0..4 {
            let mut lengths = vec![0_u8; frequencies.len()];
            let repaired = make_lengths_into(
                frequencies,
                &mut lengths,
                max_bits,
                variant,
                &mut heap_scratch,
            );
            if variant == 0 && !repaired {
                unlimited = Some(lengths.clone());
            }
            push_unique(&mut candidates, lengths);
        }
        push_unique(
            &mut candidates,
            unlimited
                .unwrap_or_else(|| make_lengths_columbo_defluff_limited(frequencies, max_bits, 0)),
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

/// Defluff's exact trees of both alphabets under several depth limits, each
/// alphabet's unconstrained tree built once.
pub(super) struct ExactTreeBuilder<'a> {
    build_literal_frequencies: &'a [u32; 286],
    build_distance_frequencies: &'a [u32; 30],
    literal: DefluffExactTree,
    distance: DefluffExactTree,
    scratch: TreeBuildScratch,
}

impl<'a> ExactTreeBuilder<'a> {
    pub(super) fn new(
        build_literal_frequencies: &'a [u32; 286],
        build_distance_frequencies: &'a [u32; 30],
    ) -> Self {
        let mut scratch = TreeBuildScratch::default();
        Self {
            build_literal_frequencies,
            build_distance_frequencies,
            literal: DefluffExactTree::new(build_literal_frequencies, &mut scratch),
            distance: DefluffExactTree::new(build_distance_frequencies, &mut scratch),
            scratch,
        }
    }

    /// The exact Defluff literal/length lengths limited to `max_bits`.
    fn literal_lengths(&mut self, max_bits: u8) -> Vec<u8> {
        self.literal
            .limited(self.build_literal_frequencies, max_bits, &mut self.scratch)
    }

    /// The exact Defluff distance lengths limited to `max_bits`.
    fn distance_lengths(&mut self, max_bits: u8) -> Vec<u8> {
        self.distance
            .limited(self.build_distance_frequencies, max_bits, &mut self.scratch)
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) fn exact_tree_candidate(
    literal_frequencies: &[u32; 286],
    distance_frequencies: &[u32; 30],
    trees: &mut ExactTreeBuilder<'_>,
    extra_bits: u64,
    max_bits: u8,
    exhaustive: bool,
    header_cache: &mut HeaderPlanCache,
) -> Option<DynamicPlan> {
    let literal_lengths = trees.literal_lengths(max_bits);
    let distance_lengths = trees.distance_lengths(max_bits);

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
        // Payload bits are a lower bound on the block cost, so the smallest
        // first does the most useful work before a deadline and prunes more.
        ordered_pairs.sort_shared_by(Ord::cmp);
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

    // If the ordering buffer cannot be allocated, price the same pairs in
    // index order.
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
    // A dynamic header costs at least one bit, so a pair whose payload alone
    // does not beat the incumbent cannot win.
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

/// Price data trees at every feasible depth limit below 15, without changing
/// tokens.
///
/// By default both alphabets share each limit, and the leaf-first (Defluff) and
/// package-first (Zopfli) trees form a two-by-two product. With
/// `independent_depths`, it prices the full cross-product of every distinct
/// tree per alphabet; the caller must bound that work.
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
    let mut trees = ExactTreeBuilder::new(&build_literal_frequencies, &build_distance_frequencies);

    if independent_depths {
        let mut literal_candidates = Vec::new();
        for max_bits in minimum_complete_tree_depth(&build_literal_frequencies)
            ..=MAX_RESTRICTED_PAYLOAD_TREE_DEPTH
        {
            let defluff = trees.literal_lengths(max_bits);
            let package_first =
                make_lengths_zopfli_package_from(&build_literal_frequencies, &defluff, max_bits);
            push_unique(&mut literal_candidates, defluff);
            push_unique(&mut literal_candidates, package_first);
        }
        let mut distance_candidates = Vec::new();
        for max_bits in minimum_complete_tree_depth(&build_distance_frequencies)
            ..=MAX_RESTRICTED_PAYLOAD_TREE_DEPTH
        {
            let defluff = trees.distance_lengths(max_bits);
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
            let literal_defluff = trees.literal_lengths(max_bits);
            let literal_package_first = make_lengths_zopfli_package_from(
                &build_literal_frequencies,
                &literal_defluff,
                max_bits,
            );
            push_unique(&mut literal_candidates, literal_defluff);
            push_unique(&mut literal_candidates, literal_package_first);
            distance_candidates.clear();
            let distance_defluff = trees.distance_lengths(max_bits);
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

/// Price data trees built from Brotli's and Zopfli's RLE-smoothed counts at
/// each depth limit from 15 down to 9. This stays out of the ordinary block
/// planner because a locally cheaper tree can lead later token feedback to a
/// worse fixed point.
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
        let mut trees =
            ExactTreeBuilder::new(build_literal_frequencies, build_distance_frequencies);
        for max_bits in [15, 14, 13, 12, 11, 10, 9] {
            if let Some(candidate) = exact_tree_candidate(
                literal_frequencies,
                distance_frequencies,
                &mut trees,
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

/// Build a tree from Columbo's RLE-friendly counts, priced with the true
/// frequencies.
pub(super) fn columbo_rle_tree_candidate(
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

/// Build a tree from Zopfli's RLE-friendly counts, priced with the true
/// frequencies.
pub(super) fn zopfli_rle_tree_candidate(
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

#[cfg(test)]
mod tests;
