// SPDX-License-Identifier: MIT

//! Jointly choose the code-length tree and its shortest RLE spelling while
//! holding the data trees and advertised spans fixed. Nonzero runs depend
//! only on their literal code price and repeat-16 price. Enumerating the four
//! optional zero/repeat prices leaves a small exact Kraft-capacity problem.

use super::{dynamic_bits, shortest_rle, trim_code_lengths};
use crate::deflate::model::{try_clone_slice, DynamicPlan, ParsedBlock, CODE_LENGTH_ORDER};
use crate::deflate::stop::SearchStop;

const LENGTHS: usize = 319;
const CAPACITY: usize = 128;
const INF: u32 = u32::MAX / 4;
const STREAM_WORK: usize = 1 << 24;

pub(crate) struct HeaderTreeBudget {
    work_left: usize,
}

impl HeaderTreeBudget {
    pub(crate) fn new() -> Self {
        Self {
            work_left: STREAM_WORK,
        }
    }

    fn spend(&mut self, work: usize) -> Option<()> {
        self.work_left = self.work_left.checked_sub(work)?;
        Some(())
    }
}

struct Runs {
    counts: [[u16; LENGTHS]; 16],
    longest: [usize; 16],
    symbols: [usize; 15],
    symbol_count: usize,
    hclen: usize,
}

impl Runs {
    fn new(sequence: &[u8]) -> Option<Self> {
        if sequence.len() >= LENGTHS || sequence.iter().any(|&length| length > 15) {
            return None;
        }
        let mut runs = Self {
            counts: [[0; LENGTHS]; 16],
            longest: [0; 16],
            symbols: [0; 15],
            symbol_count: 0,
            hclen: 0,
        };
        let mut at = 0;
        while at < sequence.len() {
            let value = usize::from(sequence[at]);
            let start = at;
            while at < sequence.len() && usize::from(sequence[at]) == value {
                at += 1;
            }
            let count = at - start;
            runs.counts[value][count] += 1;
            runs.longest[value] = runs.longest[value].max(count);
        }
        for symbol in 1..16 {
            if runs.longest[symbol] != 0 {
                runs.symbols[runs.symbol_count] = symbol;
                runs.symbol_count += 1;
            }
        }
        // A valid Deflate data-length list always needs two emitted CL
        // symbols: either two positive lengths or a positive length and a
        // zero encoding. A singleton arbitrary sequence needs a filler rule
        // and is outside this solver's domain.
        if runs.symbol_count == 0 || (runs.symbol_count == 1 && runs.longest[0] == 0) {
            return None;
        }
        // All optional symbols (16, 17, 18, 0) precede every mandatory
        // positive literal in the transmission order, so HCLEN is constant.
        runs.hclen = CODE_LENGTH_ORDER
            .iter()
            .rposition(|&s| (1..16).contains(&s) && runs.longest[s] != 0)?
            + 1;
        Some(runs)
    }
}

fn weight(length: u8) -> usize {
    if length == 0 {
        0
    } else {
        1 << (7 - length)
    }
}

fn positive_costs(
    runs: &Runs,
    repeat: u8,
    budget: &mut HeaderTreeBudget,
) -> Option<[[u32; 8]; 16]> {
    budget.spend(16 * 8 + LENGTHS)?;
    let mut costs = [[INF; 8]; 16];
    let mut dp = [0; LENGTHS];
    for &symbol in &runs.symbols[..runs.symbol_count] {
        // One literal edge and at most four repeat edges per run position.
        budget.spend(7 * runs.longest[symbol] * 5)?;
        for length in 1..=7_u8 {
            let mut sum = 0;
            for n in 1..=runs.longest[symbol] {
                dp[n] = dp[n - 1] + u32::from(length);
                if repeat != 0 {
                    // The first positive value must be explicit. A repeat
                    // can only follow at least one already-emitted value.
                    for count in 3..=6.min(n.saturating_sub(1)) {
                        dp[n] = dp[n].min(dp[n - count] + u32::from(repeat) + 2);
                    }
                }
                sum += u32::from(runs.counts[symbol][n]) * dp[n];
            }
            costs[symbol][usize::from(length)] = sum;
        }
    }
    Some(costs)
}

struct KraftPlan {
    costs: [u32; CAPACITY + 1],
    choices: [[u8; CAPACITY + 1]; 16],
}

fn kraft_plan(
    runs: &Runs,
    repeat: u8,
    costs: &[[u32; 8]; 16],
    budget: &mut HeaderTreeBudget,
    stop: &mut SearchStop<'_>,
) -> Option<KraftPlan> {
    budget.spend(18 * (CAPACITY + 1))?;
    let mut result = KraftPlan {
        costs: [INF; CAPACITY + 1],
        choices: [[0; CAPACITY + 1]; 16],
    };
    result.costs[0] = 0;
    let limit = CAPACITY - weight(repeat);
    for (i, &symbol) in runs.symbols[..runs.symbol_count].iter().enumerate() {
        if stop.reached() {
            return None;
        }
        budget.spend(CAPACITY + 1)?;
        let mut next = [INF; CAPACITY + 1];
        for capacity in 0..=limit {
            if result.costs[capacity] == INF {
                continue;
            }
            budget.spend(7)?;
            for length in 1..=7_u8 {
                let end = capacity + weight(length);
                if end > limit {
                    continue;
                }
                let cost = result.costs[capacity] + costs[symbol][usize::from(length)];
                if cost < next[end] {
                    next[end] = cost;
                    result.choices[i + 1][end] = length;
                }
            }
        }
        result.costs = next;
    }
    Some(result)
}

struct ZeroScratch {
    costs: [u32; LENGTHS],
    queues: [[usize; LENGTHS]; 3],
}

impl ZeroScratch {
    fn new() -> Self {
        Self {
            costs: [0; LENGTHS],
            queues: [[0; LENGTHS]; 3],
        }
    }

    fn price(&mut self, runs: &Runs, tree: &[u8; 19]) -> Option<u32> {
        let mut heads = [0; 3];
        let mut tails = [0; 3];
        self.costs[0] = 0;
        let mut total = 0;
        for n in 1..=runs.longest[0] {
            self.costs[n] = if tree[0] == 0 {
                INF
            } else {
                self.costs[n - 1] + u32::from(tree[0])
            };
            for (q, symbol, min, max, extra) in
                [(0, 16, 3, 6, 2), (1, 17, 3, 10, 3), (2, 18, 11, 138, 7)]
            {
                if tree[symbol] == 0 || n < min {
                    continue;
                }
                let at = n - min;
                if symbol == 16 && at == 0 {
                    continue;
                }
                // Sliding minima price each repeat range in amortized
                // constant time. Every suffix enters and leaves once.
                while tails[q] > heads[q]
                    && self.costs[self.queues[q][tails[q] - 1]] >= self.costs[at]
                {
                    tails[q] -= 1;
                }
                self.queues[q][tails[q]] = at;
                tails[q] += 1;
                while heads[q] < tails[q] && self.queues[q][heads[q]] + max < n {
                    heads[q] += 1;
                }
                if heads[q] < tails[q] {
                    self.costs[n] = self.costs[n].min(
                        self.costs[self.queues[q][heads[q]]] + u32::from(tree[symbol]) + extra,
                    );
                }
            }
            if runs.counts[0][n] != 0 {
                if self.costs[n] >= INF {
                    return None;
                }
                total += u32::from(runs.counts[0][n]) * self.costs[n];
            }
        }
        Some(total)
    }
}

/// Completed enumeration is exact for fixed data-length lists and spans.
/// Unused CL leaves cannot help: removing them and suppressing unary nodes
/// never lengthens an emitted code or HCLEN, and at least two emitted symbols
/// remain. Enumerate all usable repeat/zero support choices, including absence.
/// Budget and deadline stops return the best completed tree, without claiming
/// the unfinished search proved optimality.
fn search(
    runs: &Runs,
    incumbent_bits: u64,
    budget: &mut HeaderTreeBudget,
    stop: &mut SearchStop<'_>,
) -> Option<([u8; 19], u64)> {
    budget.spend(4 * LENGTHS)?;
    let mut zero = ZeroScratch::new();
    let mut best = None;
    let mut best_bits = incumbent_bits;
    let can_repeat = runs.longest.iter().any(|&n| n >= 4);
    for repeat in 0..=if can_repeat { 7 } else { 0 } {
        if stop.reached() {
            return best;
        }
        let Some(costs) = positive_costs(runs, repeat, budget) else {
            return best;
        };
        let Some(kraft) = kraft_plan(runs, repeat, &costs, budget, stop) else {
            return best;
        };
        for literal_zero in 0..=if runs.longest[0] != 0 { 7 } else { 0 } {
            for short_zero in 0..=if runs.longest[0] >= 3 { 7 } else { 0 } {
                for long_zero in 0..=if runs.longest[0] >= 11 { 7 } else { 0 } {
                    if stop.reached() || budget.spend(1).is_none() {
                        return best;
                    }
                    let used = weight(repeat)
                        + weight(literal_zero)
                        + weight(short_zero)
                        + weight(long_zero);
                    if used > CAPACITY {
                        continue;
                    }
                    let remainder = CAPACITY - used;
                    let positive = kraft.costs[remainder];
                    let fixed = 17 + 3 * runs.hclen as u64;
                    if positive == INF || fixed + u64::from(positive) >= best_bits {
                        continue;
                    }
                    // One literal transition and three monotone repeat
                    // windows per position. A window's total queue work is
                    // linear, including all entries removed in its loops.
                    if budget.spend(4 * runs.longest[0]).is_none() {
                        return best;
                    }
                    let mut tree = [0; 19];
                    tree[0] = literal_zero;
                    tree[16] = repeat;
                    tree[17] = short_zero;
                    tree[18] = long_zero;
                    let Some(zero_bits) = zero.price(runs, &tree) else {
                        continue;
                    };
                    let bits = fixed + u64::from(positive + zero_bits);
                    if bits >= best_bits {
                        continue;
                    }
                    let mut remaining = remainder;
                    for i in (1..=runs.symbol_count).rev() {
                        let length = kraft.choices[i][remaining];
                        tree[runs.symbols[i - 1]] = length;
                        remaining -= weight(length);
                    }
                    debug_assert_eq!(remaining, 0);
                    best = Some((tree, bits));
                    best_bits = bits;
                }
            }
        }
    }
    best
}

pub(crate) fn plan_header_tree(
    block: &ParsedBlock,
    strict: bool,
    budget: &mut HeaderTreeBudget,
    stop: &mut SearchStop<'_>,
) -> Option<DynamicPlan> {
    if stop.reached() {
        return None;
    }
    let parent = block.original_dynamic.as_ref()?;
    if strict && !parent.has_strictly_compatible_huffman_codes() {
        return None;
    }
    let header = dynamic_bits(0, parent)?;
    let data = block.original?.len.checked_sub(header)?;
    budget.spend(16 * LENGTHS + LENGTHS)?;
    let count = parent
        .literal_lengths
        .len()
        .checked_add(parent.distance_lengths.len())?;
    if count >= LENGTHS {
        return None;
    }
    let mut sequence = [0; LENGTHS];
    let middle = parent.literal_lengths.len();
    sequence[..middle].copy_from_slice(&parent.literal_lengths);
    sequence[middle..count].copy_from_slice(&parent.distance_lengths);
    let sequence = &sequence[..count];
    let runs = Runs::new(sequence)?;
    let (tree, bits) = search(&runs, header, budget, stop)?;
    // Reconstruct one shortest spelling only after selecting a completed
    // tree. The data codewords, token sequence and advertised spans stay fixed.
    let rle = shortest_rle(sequence, &tree)?;
    Some(DynamicPlan {
        literal_lengths: try_clone_slice(&parent.literal_lengths)?,
        distance_lengths: try_clone_slice(&parent.distance_lengths)?,
        code_length_lengths: tree,
        rle,
        hlit: parent.hlit,
        hdist: parent.hdist,
        hclen: trim_code_lengths(&tree),
        bits: data.checked_add(bits)?,
    })
}

#[cfg(test)]
mod tests;
