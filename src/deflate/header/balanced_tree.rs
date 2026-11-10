// SPDX-License-Identifier: MIT
// SPDX-FileCopyrightText: 2026 Andrew C.E. Dent <https://github.com/ace-dent>

//! Balanced-tree moves on the data alphabets (D3, M4): bounded
//! Kraft-preserving moves that lengthen some codes to shorten one, kept when
//! the header shrinks by more than the payload grows.
//! [`balanced_tree_opportunities`] counts the moves a seed offers, for
//! progress reports.

use super::{
    keep_better, pad_lengths, plan_for_explicit_lengths_with_cost, token_bits_from_frequencies,
};
use crate::deflate::model::{token_extra_bits, DynamicPlan, Token};

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

// Out of line: each call scans and shifts the candidate list.
#[inline(never)]
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
    // At most 13 length bands × 5 shortenings × 10 lengthening pairs = 650
    // moves, within the 700 reserved.
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
    // At most 12 length bands × 5 shortenings × 5 lengthening quads = 300
    // moves.
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

/// Try bounded Kraft-preserving moves on both data alphabets (D3, M4).
///
/// A pair move shortens one code by a bit and lengthens two codes of the same
/// length; a quad move shortens one code and lengthens four codes one bit
/// longer than it. A cross-product of the best moves per alphabet can also
/// exploit the RLE run that crosses the alphabet boundary. The seed plan is
/// not returned; callers keep it.
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

#[cfg(test)]
mod tests;
