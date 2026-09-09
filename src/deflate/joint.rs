// SPDX-License-Identifier: MIT

//! Joint payload-code and code-length-repeat search under a fixed header tree.
//!
//! A state records the next alphabet position, remaining Kraft capacity, and
//! previous length. An edge emits one complete code-length instruction and
//! charges both its header bits and the payload bits of the lengths it assigns.
//! Literal/length and distance alphabets have separate capacities; an
//! instruction may cross their boundary without resetting the previous length.

use super::header::{plan_for_advertised_lengths, token_bits};
use super::model::{
    token_extra_bits, DynamicPlan, ParsedBlock, RleToken, MAX_DYNAMIC_CODE_LENGTH_COUNT,
};
use super::stop::SearchStop;

const INF: u64 = u64::MAX / 4;
const MAX_DEPTH: usize = 9;

/// One budget shared by every block in a terminal pass. Charge preparation,
/// dense-state scans, and candidate edges, including impossible edges. The
/// fixed-alphabet header repricer runs at most once per successful block.
pub(crate) struct JointBudget {
    work_left: u64,
}

impl JointBudget {
    pub(crate) fn new() -> Self {
        Self { work_left: 1 << 27 }
    }

    fn spend(&mut self, work: u64) -> bool {
        if work > self.work_left {
            self.work_left = 0;
            false
        } else {
            self.work_left -= work;
            true
        }
    }
}

fn filled<T: Clone>(size: usize, value: T) -> Option<Vec<T>> {
    let mut values = Vec::new();
    values.try_reserve_exact(size).ok()?;
    values.resize(size, value);
    Some(values)
}

struct Problem<'a> {
    frequencies: &'a [u32],
    split: usize,
    /// Reserved distance positions are advertised as zero, never payload codes.
    reserved_from: usize,
    costs: &'a [u8; 19],
    max_depth: usize,
}

struct Solution {
    lengths: Vec<u8>,
    rle: Vec<RleToken>,
    bits: u64,
}

fn payload_suffix(
    frequencies: &[u32],
    reserved_from: usize,
    capacity: usize,
    allowed: &[(usize, usize)],
    zero: bool,
    stop: &mut SearchStop<'_>,
) -> Option<Vec<u64>> {
    let stride = capacity + 1;
    let mut costs = filled((frequencies.len() + 1) * stride, INF)?;
    costs[frequencies.len() * stride] = 0;
    for i in (0..frequencies.len()).rev() {
        if stop.reached() {
            return None;
        }
        let (prefix, suffix) = costs.split_at_mut((i + 1) * stride);
        let current = &mut prefix[i * stride..];
        let next = &suffix[..stride];
        if frequencies[i] == 0 && zero {
            current.copy_from_slice(next);
        }
        if i < reserved_from {
            for &(length, units) in allowed {
                let price = u64::from(frequencies[i]) * length as u64;
                // Assign this length to every capacity where it fits. The
                // shifted suffix rows avoid a per-capacity feasibility test.
                for (best, &rest) in current[units..].iter_mut().zip(next) {
                    *best = (*best).min(rest + price);
                }
            }
        }
    }
    Some(costs)
}

/// Exact within the fixed-CL, complete-tree, depth-limited domain when the
/// budget and stop permit completion. A cutoff returns only an already complete
/// strict improvement, never an unfinished path or a claim of optimality.
fn solve(
    problem: &Problem<'_>,
    mut target: u64,
    budget: &mut JointBudget,
    stop: &mut SearchStop<'_>,
) -> Option<Solution> {
    let Problem {
        frequencies,
        split,
        reserved_from,
        costs: cl,
        max_depth,
    } = *problem;
    let n = frequencies.len();
    if stop.reached()
        || n > MAX_DYNAMIC_CODE_LENGTH_COUNT
        || split == 0
        || split >= n
        || reserved_from < split
        || reserved_from > n
        || max_depth > MAX_DEPTH
        || target > INF
    {
        return None;
    }
    // A positive length must first appear as a direct CL symbol. Repeats
    // cannot introduce a length absent from this fixed tree.
    let depth = (1..=max_depth).rev().find(|&length| cl[length] > 0)?;
    let capacity = 1usize << depth;
    let previous_count = depth + 2;
    let none = depth + 1;
    let stride = (capacity + 1) * previous_count;
    let cells = (n + 1) * stride;
    let allowed: Vec<_> = (1..=depth)
        .filter(|&length| cl[length] > 0)
        .map(|length| (length, 1 << (depth - length)))
        .collect();
    // Reserve an upper bound on preparation, allocations, and dense scans
    // before allocating. In particular, impossible blocks also consume work.
    let preparation = 3 * cells
        + (n + 2) * (capacity + 1) * (allowed.len() + 3)
        + n * (depth + 1 + 138 + previous_count * 4);
    if !budget.spend(preparation as u64) {
        return None;
    }
    let zero = cl[0] > 0 || cl[17] > 0 || cl[18] > 0;
    let literal_bound =
        payload_suffix(&frequencies[..split], split, capacity, &allowed, zero, stop)?;
    let distance_bound = payload_suffix(
        &frequencies[split..],
        reserved_from - split,
        capacity,
        &allowed,
        zero,
        stop,
    )?;
    let distance_minimum = distance_bound[capacity];
    let payload_bound = |i: usize, remaining: usize| -> u64 {
        if i < split {
            (literal_bound[i * (capacity + 1) + remaining] + distance_minimum).min(INF)
        } else {
            distance_bound[(i - split) * (capacity + 1) + remaining]
        }
    };
    let mut sums = [0u64; MAX_DYNAMIC_CODE_LENGTH_COUNT + 1];
    let mut used = [0usize; MAX_DYNAMIC_CODE_LENGTH_COUNT + 1];
    let mut zero_run = [0usize; MAX_DYNAMIC_CODE_LENGTH_COUNT + 1];
    for i in 0..n {
        sums[i + 1] = sums[i] + u64::from(frequencies[i]);
        used[i + 1] = used[i] + usize::from(frequencies[i] > 0);
    }
    for i in (0..n).rev() {
        if frequencies[i] == 0 {
            zero_run[i] = zero_run[i + 1] + 1;
        }
    }
    let legal = |i: usize, count: usize, length: usize| -> bool {
        i + count <= n
            && if length == 0 {
                used[i + count] == used[i]
            } else {
                i + count <= reserved_from
            }
    };
    // Independent exact suffix minima ignore each other's assignments. Their
    // sum is an admissible lower bound, even when those assignments disagree.
    let mut header = filled((n + 1) * previous_count, INF)?;
    header[n * previous_count..].fill(0);
    for i in (0..n).rev() {
        if stop.reached() {
            return None;
        }
        let mut common = INF;
        for length in 0..=depth {
            if cl[length] > 0 && legal(i, 1, length) {
                common =
                    common.min(u64::from(cl[length]) + header[(i + 1) * previous_count + length]);
            }
        }
        for (symbol, minimum, maximum, extra) in [(17, 3, 10, 3), (18, 11, 138, 7)] {
            if cl[symbol] > 0 {
                for count in minimum..=zero_run[i].min(maximum) {
                    common = common
                        .min(u64::from(cl[symbol]) + extra + header[(i + count) * previous_count]);
                }
            }
        }
        for previous in 0..previous_count {
            let mut best = common;
            if previous != none && cl[16] > 0 {
                for count in 3..=6 {
                    if legal(i, count, previous) {
                        best = best.min(
                            u64::from(cl[16]) + 2 + header[(i + count) * previous_count + previous],
                        );
                    }
                }
            }
            header[i * previous_count + previous] = best;
        }
    }
    if payload_bound(0, capacity) + header[none] >= target {
        return None;
    }
    // At depth nine the two tables plus suffix bounds occupy less than 24 MiB.
    // Blocks reuse this allowance sequentially without per-block arenas.
    let mut cost = filled(cells, INF)?;
    let mut trace = filled(cells, u32::MAX)?;
    let start = capacity * previous_count + none;
    cost[start] = 0;
    let mut winner = None;
    'search: for i in 0..n {
        for remaining in 0..=capacity {
            if remaining % 32 == 0 && (budget.work_left == 0 || stop.reached()) {
                break 'search;
            }
            for previous in 0..previous_count {
                let from = i * stride + remaining * previous_count + previous;
                let old = cost[from];
                if old == INF
                    || old + payload_bound(i, remaining) + header[i * previous_count + previous]
                        >= target
                {
                    continue;
                }
                let mut edge = |count: usize, length: usize, symbol: usize, kind: u32| {
                    if !budget.spend(1) || !legal(i, count, length) {
                        return;
                    }
                    let j = i + count;
                    let units = if length == 0 {
                        0
                    } else {
                        1 << (depth - length)
                    };
                    let next = if i < split && j >= split {
                        if remaining != (split - i) * units || (j - split) * units > capacity {
                            return;
                        }
                        capacity - (j - split) * units
                    } else {
                        if count * units > remaining {
                            return;
                        }
                        remaining - count * units
                    };
                    let extra = match symbol {
                        16 => 2,
                        17 => 3,
                        18 => 7,
                        _ => 0,
                    };
                    let score =
                        old + u64::from(cl[symbol]) + extra + (sums[j] - sums[i]) * length as u64;
                    if score + payload_bound(j, next) + header[j * previous_count + length]
                        >= target
                    {
                        return;
                    }
                    let to = j * stride + next * previous_count + length;
                    if score < cost[to] {
                        cost[to] = score;
                        // Position always advances, so predecessor traces are
                        // final before they are used. Fewer than 2^21 cells.
                        trace[to] = ((from as u32) << 2) | kind;
                        if j == n {
                            debug_assert_eq!(next, 0);
                            target = score;
                            winner = Some(to);
                        }
                    }
                };
                for (length, &cl_bits) in cl.iter().enumerate().take(depth + 1) {
                    if cl_bits > 0 {
                        edge(1, length, length, 0);
                    }
                }
                if previous != none && cl[16] > 0 {
                    for count in 3..=6 {
                        edge(count, previous, 16, 1);
                    }
                }
                if cl[17] > 0 {
                    for count in 3..=zero_run[i].min(10) {
                        edge(count, 0, 17, 2);
                    }
                }
                if cl[18] > 0 {
                    for count in 11..=zero_run[i].min(138) {
                        edge(count, 0, 18, 3);
                    }
                }
            }
        }
    }
    let mut at = winner?;
    let bits = cost[at];
    let mut rle = Vec::new();
    rle.try_reserve_exact(n).ok()?;
    while at != start {
        let link = trace[at];
        debug_assert_ne!(link, u32::MAX);
        let from = (link >> 2) as usize;
        let count = at / stride - from / stride;
        let length = at % previous_count;
        rle.push(match link & 3 {
            0 => {
                debug_assert_eq!(count, 1);
                RleToken {
                    symbol: length as u8,
                    extra: 0,
                }
            }
            1 => RleToken {
                symbol: 16,
                extra: (count - 3) as u8,
            },
            2 => RleToken {
                symbol: 17,
                extra: (count - 3) as u8,
            },
            _ => RleToken {
                symbol: 18,
                extra: (count - 11) as u8,
            },
        });
        at = from;
    }
    rle.reverse();
    let mut lengths = Vec::new();
    lengths.try_reserve_exact(n).ok()?;
    for token in &rle {
        match token.symbol {
            0..=15 => lengths.push(token.symbol),
            16 => {
                let value = *lengths.last()?;
                lengths.resize(lengths.len() + usize::from(token.extra) + 3, value);
            }
            17 => lengths.resize(lengths.len() + usize::from(token.extra) + 3, 0),
            18 => lengths.resize(lengths.len() + usize::from(token.extra) + 11, 0),
            _ => unreachable!(),
        }
    }
    debug_assert_eq!(lengths.len(), n);
    Some(Solution { lengths, rle, bits })
}

pub(crate) fn plan_joint_tree_rle(
    block: &ParsedBlock,
    budget: &mut JointBudget,
    stop: &mut SearchStop<'_>,
) -> Option<DynamicPlan> {
    let parent = block.original_dynamic.as_ref()?;
    // Always produce complete payload and CL trees, including two usable
    // distance codes. Relaxed sources outside that domain keep their parent.
    if !parent.has_strictly_compatible_huffman_codes() || stop.reached() {
        return None;
    }
    let n = parent.hlit + parent.hdist;
    let mut frequencies = [0u32; MAX_DYNAMIC_CODE_LENGTH_COUNT];
    frequencies[..parent.hlit].copy_from_slice(&block.literal_frequencies[..parent.hlit]);
    for (i, frequency) in frequencies[parent.hlit..n].iter_mut().enumerate() {
        *frequency = block.distance_frequencies.get(i).copied().unwrap_or(0);
    }
    let fixed = 17 + 3 * parent.hclen as u64 + token_extra_bits(&block.tokens);
    let solution = solve(
        &Problem {
            frequencies: &frequencies[..n],
            split: parent.hlit,
            reserved_from: n.min(parent.hlit + 30),
            costs: &parent.code_length_lengths,
            max_depth: MAX_DEPTH,
        },
        block.original?.len.checked_sub(fixed)?,
        budget,
        stop,
    )?;
    let mut lengths = solution.lengths;
    let distance_lengths = super::model::try_clone_slice(&lengths[parent.hlit..])?;
    lengths.truncate(parent.hlit);
    let mut plan = DynamicPlan {
        literal_lengths: lengths,
        distance_lengths,
        code_length_lengths: parent.code_length_lengths,
        rle: solution.rle,
        hlit: parent.hlit,
        hdist: parent.hdist,
        hclen: parent.hclen,
        bits: fixed + solution.bits,
    };
    if !plan.has_strictly_compatible_huffman_codes() {
        return None;
    }
    // Reprice this complete joint winner with the existing header feedback.
    // Preserve the advertised spans, and retain the fixed-CL winner separately.
    if !stop.reached() {
        let payload = token_bits(&block.tokens, &plan.literal_lengths, &plan.distance_lengths)?;
        if let Some(repriced) =
            plan_for_advertised_lengths(&plan.literal_lengths, &plan.distance_lengths, payload)
        {
            if repriced.bits < plan.bits {
                plan = repriced;
            }
        }
    }
    Some(plan)
}

#[cfg(test)]
mod tests;
