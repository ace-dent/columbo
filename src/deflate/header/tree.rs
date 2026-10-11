// SPDX-License-Identifier: MIT
// SPDX-FileCopyrightText: 2026 Andrew C.E. Dent <https://github.com/ace-dent>

//! Choose the code-length code and its cheapest RLE spelling together, with
//! the data code lengths, HLIT and HDIST fixed (R7). The cost of a run of a
//! nonzero length depends only on the code lengths of that value and of symbol
//! 16. Enumerating the code lengths of symbols 0, 16, 17 and 18 leaves a small
//! exact Kraft-capacity problem for the nonzero values.

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
        // A valid sequence emits at least two code-length symbols (two nonzero
        // values, or one plus zeros); a single-value sequence would need a
        // filler rule, which this solver does not handle.
        if runs.symbol_count == 0 || (runs.symbol_count == 1 && runs.longest[0] == 0) {
            return None;
        }
        // The optional symbols 16, 17, 18 and 0 come first in the RFC 1951
        // code-length order, so HCLEN depends only on the nonzero values.
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

/// Cost of a run of `count` equal nonzero lengths. After one explicit length,
/// the rest is all explicit, or as many six-repeat codes 16 as fit plus an
/// explicit remainder, or one more code 16 with repeats in groups of three to
/// six. Fewer codes 16 leave more explicit lengths, and more cannot remove the
/// first one.
fn positive_run_cost(count: usize, literal: u8, repeat: u8) -> u32 {
    let count = count as u32;
    let literal = u32::from(literal);
    let direct = count * literal;
    if repeat == 0 || count < 4 {
        return direct;
    }
    let repeat = u32::from(repeat) + 2;
    let full = (count - 1) / 6;
    direct
        .min((count - 6 * full) * literal + full * repeat)
        .min(literal + (full + 1) * repeat)
}

fn positive_costs(
    runs: &Runs,
    repeat: u8,
    budget: &mut HeaderTreeBudget,
) -> Option<[[u32; 8]; 16]> {
    budget.spend(16 * 8 + LENGTHS)?;
    let mut costs = [[INF; 8]; 16];
    for &symbol in &runs.symbols[..runs.symbol_count] {
        // One literal edge and at most four repeat edges per run position.
        budget.spend(7 * runs.longest[symbol] * 5)?;
    }
    // A nonzero run's cost does not depend on its value, so price each run
    // length once. Keep the conservative work charges above.
    let longest = runs.longest[1..].iter().copied().max().unwrap_or(0);
    let mut prices = [0; LENGTHS];
    for length in 1..=7_u8 {
        for (n, price) in prices.iter_mut().enumerate().take(longest + 1).skip(1) {
            *price = positive_run_cost(n, length, repeat);
        }
        for &symbol in &runs.symbols[..runs.symbol_count] {
            costs[symbol][usize::from(length)] = runs.counts[symbol][1..=runs.longest[symbol]]
                .iter()
                .zip(&prices[1..])
                .map(|(&count, &price)| u32::from(count) * price)
                .sum();
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
                // constant time.
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

/// Find the cheapest code-length code below `incumbent_bits`. A completed
/// enumeration is exact: unused code-length leaves cannot help, since removing
/// them and collapsing single-child nodes never lengthens a used code or HCLEN,
/// and at least two used symbols remain. A budget or deadline stop returns the
/// best tree so far.
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
                    // windows per position, each with linear queue work.
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
