// SPDX-License-Identifier: MIT
// SPDX-FileCopyrightText: 2026 Andrew C.E. Dent <https://github.com/ace-dent>

//! Swap two code lengths in the literal/length alphabet and two in the distance
//! alphabet together. Both alphabets share one code-length code and one RLE
//! sequence, so the combined saving need not be the sum of the separate
//! savings.

use super::{dynamic_bits, plan_for_advertised_lengths};
use crate::deflate::model::{DynamicPlan, ParsedBlock, MAX_DYNAMIC_CODE_LENGTH_COUNT};
use crate::deflate::stop::SearchStop;

const STREAM_WORK: usize = 1 << 20;
const STREAM_PRICES: usize = 512;
const ALPHABET_MENU: usize = 32;
const PAIR_MENU: usize = 256;
const MAX_PAYLOAD_TAX: i64 = 32;

pub(crate) struct CoupledSwapBudget {
    work_left: usize,
    prices_left: usize,
}

impl CoupledSwapBudget {
    pub(crate) fn new() -> Self {
        Self {
            work_left: STREAM_WORK,
            prices_left: STREAM_PRICES,
        }
    }

    fn spend(&mut self, work: usize) -> Option<()> {
        self.work_left = self.work_left.checked_sub(work)?;
        Some(())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct Swap {
    rank: i64,
    tax: i64,
    positions: [usize; 2],
}

/// Run transitions removed by swapping the values at `positions[2k]` and
/// `positions[2k + 1]`. Positions must be strictly increasing; edges across
/// the boundary between the two alphabets count too.
fn transitions_removed<const N: usize>(lengths: &[u8], positions: [usize; N]) -> i64 {
    let values = std::array::from_fn(|i| lengths[positions[i ^ 1]]);
    super::transitions_removed(lengths, positions, values)
}

fn insert<T: Ord>(menu: &mut Vec<T>, value: T, limit: usize) {
    let at = menu.binary_search(&value).unwrap_or_else(|at| at);
    if at < limit {
        menu.insert(at, value);
        menu.truncate(limit);
    }
}

fn alphabet_menu(
    lengths: &[u8],
    offset: usize,
    frequencies: &[u32],
    budget: &mut CoupledSwapBudget,
    stop: &mut SearchStop<'_>,
) -> Option<Vec<Swap>> {
    let count = frequencies.len().min(lengths.len().checked_sub(offset)?);
    budget.spend(count)?;
    let mut menu = Vec::new();
    menu.try_reserve_exact(ALPHABET_MENU + 1).ok()?;
    'scan: for a in 0..count {
        if stop.reached() || budget.spend(1).is_none() {
            break;
        }
        let la = lengths[offset + a];
        if la == 0 {
            continue;
        }
        for b in a + 1..count {
            if budget.spend(1).is_none() {
                break 'scan;
            }
            let lb = lengths[offset + b];
            if lb == 0 || la == lb {
                continue;
            }
            let tax = (i64::from(frequencies[a]) - i64::from(frequencies[b]))
                * (i64::from(lb) - i64::from(la));
            if !(-MAX_PAYLOAD_TAX..=MAX_PAYLOAD_TAX).contains(&tax) {
                continue;
            }
            let positions = [offset + a, offset + b];
            let removed = transitions_removed(lengths, positions);
            // Keep swaps that leave the transition count unchanged: they can
            // still help the shared code-length code. This filter and the
            // ranking are heuristics, not bounds.
            if removed < 0 {
                continue;
            }
            insert(
                &mut menu,
                Swap {
                    rank: tax - 3 * removed,
                    tax,
                    positions,
                },
                ALPHABET_MENU,
            );
        }
    }
    Some(menu)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct Pair {
    rank: i64,
    tax: i64,
    literal: usize,
    distance: usize,
}

fn pair_menu(
    lengths: &[u8],
    literal: &[Swap],
    distance: &[Swap],
    budget: &mut CoupledSwapBudget,
    stop: &mut SearchStop<'_>,
) -> Option<Vec<Pair>> {
    let mut menu = Vec::new();
    menu.try_reserve_exact(PAIR_MENU + 1).ok()?;
    'scan: for (i, a) in literal.iter().enumerate() {
        if stop.reached() {
            break;
        }
        for (j, b) in distance.iter().enumerate() {
            if budget.spend(1).is_none() {
                break 'scan;
            }
            let tax = a.tax + b.tax;
            if !(-MAX_PAYLOAD_TAX..=MAX_PAYLOAD_TAX).contains(&tax) {
                continue;
            }
            // Count all four positions together: when the two swaps touch
            // neighboring positions at the alphabet boundary, adding their
            // separate counts would be wrong.
            let removed = transitions_removed(
                lengths,
                [
                    a.positions[0],
                    a.positions[1],
                    b.positions[0],
                    b.positions[1],
                ],
            );
            insert(
                &mut menu,
                Pair {
                    rank: tax - 3 * removed,
                    tax,
                    literal: i,
                    distance: j,
                },
                PAIR_MENU,
            );
        }
    }
    Some(menu)
}

/// Try one literal/length swap and one distance swap together. Every pair
/// starts from the same parent and keeps the tokens, the set of used symbols,
/// each alphabet's length histogram, and HLIT and HDIST. Neither swap needs to
/// improve the parent on its own. A stop returns the best fully priced plan
/// found so far.
pub(crate) fn plan_coupled_length_swaps(
    block: &ParsedBlock,
    strict: bool,
    budget: &mut CoupledSwapBudget,
    stop: &mut SearchStop<'_>,
) -> Option<DynamicPlan> {
    if budget.prices_left == 0 || stop.reached() {
        return None;
    }
    let parent = block.original_dynamic.as_ref()?;
    if strict && !parent.has_strictly_compatible_huffman_codes() {
        return None;
    }
    let data = block.original?.len.checked_sub(dynamic_bits(0, parent)?)?;
    let middle = parent.literal_lengths.len();
    let count = middle.checked_add(parent.distance_lengths.len())?;
    let mut lengths = [0; MAX_DYNAMIC_CODE_LENGTH_COUNT];
    let lengths = lengths.get_mut(..count)?;
    lengths[..middle].copy_from_slice(&parent.literal_lengths);
    lengths[middle..].copy_from_slice(&parent.distance_lengths);
    // Search the small distance side first: empty, singleton and uniform
    // distance alphabets cannot supply a swap.
    let distance = alphabet_menu(lengths, middle, &block.distance_frequencies, budget, stop)?;
    if distance.is_empty() || stop.reached() {
        return None;
    }
    let literal = alphabet_menu(
        lengths,
        0,
        &block.literal_frequencies[..middle],
        budget,
        stop,
    )?;
    if literal.is_empty() || stop.reached() {
        return None;
    }
    let menu = pair_menu(lengths, &literal, &distance, budget, stop)?;
    let mut best = None;
    let mut best_bits = block.original?.len;
    for pair in menu {
        if budget.prices_left == 0 || stop.reached() {
            break;
        }
        let payload = if pair.tax >= 0 {
            data.checked_add(pair.tax as u64)
        } else {
            data.checked_sub(pair.tax.unsigned_abs())
        };
        let Some(payload) = payload else {
            continue;
        };
        let [a, b] = literal[pair.literal].positions;
        let [c, d] = distance[pair.distance].positions;
        lengths.swap(a, b);
        lengths.swap(c, d);
        budget.prices_left -= 1;
        let candidate =
            plan_for_advertised_lengths(&lengths[..middle], &lengths[middle..], payload);
        lengths.swap(a, b);
        lengths.swap(c, d);
        if let Some(candidate) = candidate {
            if candidate.bits < best_bits
                && (!strict || candidate.has_strictly_compatible_huffman_codes())
            {
                best_bits = candidate.bits;
                best = Some(candidate);
            }
        }
    }
    best
}

#[cfg(test)]
mod tests;
