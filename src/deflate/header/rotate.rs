// SPDX-License-Identifier: MIT
// SPDX-FileCopyrightText: 2026 Andrew C.E. Dent <https://github.com/ace-dent>

//! Rotate three code lengths within one data alphabet, then price the complete
//! header. A rotation keeps the alphabet's Kraft sum and set of used symbols,
//! and reaches assignments that a sequence of improving pair swaps may not.

use super::{dynamic_bits, plan_for_advertised_lengths, transitions_removed};
use crate::deflate::model::{DynamicPlan, ParsedBlock, MAX_DYNAMIC_CODE_LENGTH_COUNT};
use crate::deflate::stop::SearchStop;

/// Rotation-generation work shared by all blocks of a stream.
const STREAM_WORK: usize = 1 << 24;
/// Complete header prices per stream, separate from `STREAM_WORK`.
const STREAM_PRICES: usize = 512;
/// Most rotations kept per alphabet.
const MENU_SIZE: usize = 64;
/// Largest payload change, in bits either way, a rotation may make; the
/// complete price decides acceptance.
const MAX_PAYLOAD_TAX: i64 = 32;

/// Shared generation and exact-pricing allowances for the stream.
pub(crate) struct RotationBudget {
    work_left: usize,
    prices_left: usize,
}

impl RotationBudget {
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

/// Sorted by heuristic rank, payload change, positions, then direction.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct Rotation {
    rank: i64,
    tax: i64,
    positions: [usize; 3],
    reverse: bool,
}

impl Rotation {
    fn apply(self, lengths: &mut [u8], offset: usize) {
        let values = self.positions.map(|s| lengths[offset + s]);
        let shift = if self.reverse { 2 } else { 1 };
        for (i, symbol) in self.positions.into_iter().enumerate() {
            lengths[offset + symbol] = values[(i + shift) % 3];
        }
    }

    fn undo(self, lengths: &mut [u8], offset: usize) {
        Self {
            reverse: !self.reverse,
            ..self
        }
        .apply(lengths, offset);
    }
}

/// Enumerate rotations of three distinct nonzero lengths. A repeated length
/// would make it a pair swap; a zero length would change the set of used
/// symbols. Only rotations that remove run transitions are kept; the ranking
/// is a heuristic, not a bound.
fn proposals(
    lengths: &[u8],
    offset: usize,
    frequencies: &[u32],
    budget: &mut RotationBudget,
    stop: &mut SearchStop<'_>,
) -> Option<Vec<Rotation>> {
    let count = frequencies.len().min(lengths.len().checked_sub(offset)?);
    budget.spend(count)?;
    let mut symbols = [0; 286];
    let mut used = 0;
    for symbol in 0..count {
        if lengths[offset + symbol] != 0 {
            symbols[used] = symbol;
            used += 1;
        }
    }
    let symbols = &symbols[..used];
    let mut menu = Vec::new();
    menu.try_reserve_exact(MENU_SIZE + 1).ok()?;
    'scan: for (i, &a) in symbols.iter().enumerate() {
        if stop.reached() || budget.spend(1).is_none() {
            break;
        }
        let la = lengths[offset + a];
        for (j, &b) in symbols.iter().enumerate().skip(i + 1) {
            if stop.reached() || budget.spend(1).is_none() {
                break 'scan;
            }
            let lb = lengths[offset + b];
            if la == lb {
                continue;
            }
            for &c in symbols.iter().skip(j + 1) {
                // One triple visit and two orientations, each with at most
                // six local edges. Charge before entering that work.
                if budget.spend(3).is_none() {
                    break 'scan;
                }
                let lc = lengths[offset + c];
                if lc == la || lc == lb {
                    continue;
                }
                for reverse in [false, true] {
                    let values = if reverse { [lc, la, lb] } else { [lb, lc, la] };
                    let tax = [a, b, c]
                        .into_iter()
                        .zip(values)
                        .map(|(s, value)| {
                            i64::from(frequencies[s])
                                * (i64::from(value) - i64::from(lengths[offset + s]))
                        })
                        .sum::<i64>();
                    if !(-MAX_PAYLOAD_TAX..=MAX_PAYLOAD_TAX).contains(&tax) {
                        continue;
                    }
                    let removed =
                        transitions_removed(lengths, [offset + a, offset + b, offset + c], values);
                    if removed <= 0 {
                        continue;
                    }
                    let proposal = Rotation {
                        rank: tax - 3 * removed,
                        tax,
                        positions: [a, b, c],
                        reverse,
                    };
                    let at = menu.binary_search(&proposal).unwrap_or_else(|at| at);
                    if at < MENU_SIZE {
                        menu.insert(at, proposal);
                        menu.truncate(MENU_SIZE);
                    }
                }
            }
        }
    }
    Some(menu)
}

/// Price each kept rotation as a sibling of the unchanged parent. Rotations
/// keep the tokens, HLIT and HDIST, the set of used symbols and each
/// alphabet's length histogram. A stop returns the best fully priced
/// improvement found so far.
pub(crate) fn plan_code_length_rotations(
    block: &ParsedBlock,
    strict: bool,
    budget: &mut RotationBudget,
    stop: &mut SearchStop<'_>,
) -> Option<DynamicPlan> {
    if budget.prices_left == 0 || stop.reached() {
        return None;
    }
    let parent = block.original_dynamic.as_ref()?;
    if strict && !parent.has_strictly_compatible_huffman_codes() {
        return None;
    }
    let header = dynamic_bits(0, parent)?;
    let data = block.original?.len.checked_sub(header)?;
    let middle = parent.literal_lengths.len();
    let count = middle.checked_add(parent.distance_lengths.len())?;
    let mut lengths = [0; MAX_DYNAMIC_CODE_LENGTH_COUNT];
    let lengths = lengths.get_mut(..count)?;
    lengths[..middle].copy_from_slice(&parent.literal_lengths);
    lengths[middle..].copy_from_slice(&parent.distance_lengths);
    let mut best = None;
    let mut best_bits = block.original?.len;
    for (offset, frequencies) in [
        (0, &block.literal_frequencies[..middle]),
        (middle, block.distance_frequencies.as_slice()),
    ] {
        if budget.prices_left == 0 || stop.reached() {
            break;
        }
        let Some(menu) = proposals(lengths, offset, frequencies, budget, stop) else {
            break;
        };
        for proposal in menu {
            if budget.prices_left == 0 || stop.reached() {
                break;
            }
            let payload = if proposal.tax >= 0 {
                data.checked_add(proposal.tax as u64)
            } else {
                data.checked_sub(proposal.tax.unsigned_abs())
            };
            let Some(payload) = payload else {
                continue;
            };
            proposal.apply(lengths, offset);
            budget.prices_left -= 1;
            let candidate =
                plan_for_advertised_lengths(&lengths[..middle], &lengths[middle..], payload);
            proposal.undo(lengths, offset);
            if let Some(candidate) = candidate {
                if candidate.bits < best_bits
                    && (!strict || candidate.has_strictly_compatible_huffman_codes())
                {
                    best_bits = candidate.bits;
                    best = Some(candidate);
                }
            }
        }
    }
    best
}

#[cfg(test)]
mod tests;
