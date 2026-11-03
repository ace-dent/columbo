// SPDX-License-Identifier: MIT
// SPDX-FileCopyrightText: 2026 Andrew C.E. Dent <https://github.com/ace-dent>

//! Joint search over payload code lengths and their run-length encoding, with
//! the code-length code held fixed.
//!
//! A dynamic-programming state is (next symbol position, remaining Kraft
//! capacity, previous length). Each edge emits one code-length symbol (a
//! length 0-15 or a 16/17/18 run) and costs its header bits plus the payload
//! bits of the lengths it assigns. The literal/length and distance codes have
//! separate capacities; a run may cross between them without resetting the
//! previous length.

use super::header::{plan_for_advertised_lengths, token_bits};
use super::model::{
    token_extra_bits, DynamicPlan, ParsedBlock, RleToken, MAX_DYNAMIC_CODE_LENGTH_COUNT,
};
use super::stop::SearchStop;

const INF: u64 = u64::MAX / 4;
const MAX_DEPTH: usize = 9;

/// Work budget shared by every block in a terminal pass. It charges
/// preparation, dense-state scans and every candidate edge, including illegal
/// ones; header repricing (once per successful block) is not charged.
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

// The search's arithmetic cannot wrap: table indices stay below its cell
// count, under 2^21, and a score (a cost, header bits and a payload) plus two
// bounds stays below 2^64, since costs and bounds are at most `INF` (2^62) and
// payloads below 2^46. Wrapping operations keep the release build's overflow
// checks out of the search loop.

/// Entry `column` of row `row` in a table `width` entries wide.
#[inline(always)]
fn entry(row: usize, width: usize, column: usize) -> usize {
    row.wrapping_mul(width).wrapping_add(column)
}

/// A cost or score plus two bounds, header bits or payloads.
#[inline(always)]
fn sum3(cost: u64, first: u64, second: u64) -> u64 {
    cost.wrapping_add(first).wrapping_add(second)
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
    /// First reserved position (distance symbol 30, or the end). Reserved
    /// positions always get length 0.
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
                // Offsetting the rows by `units` tries this length at every
                // capacity where it fits. Entries stay at most `INF` and a
                // price is below 2^36, so the sum cannot wrap, and the loop
                // vectorizes without overflow checks.
                for (best, &rest) in current[units..].iter_mut().zip(next) {
                    *best = (*best).min(rest.wrapping_add(price));
                }
            }
        }
    }
    Some(costs)
}

/// Find the cheapest complete, depth-limited code lengths and run-length
/// encoding below `target` bits, with the code-length code fixed. The result
/// is optimal in that domain unless the budget or stop cuts the search off.
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
    // Each nonzero length needs its own code-length symbol (16 only repeats
    // one), so no length can exceed the deepest coded one.
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
    // Charge an upper bound for preparation, allocation and dense scans
    // before allocating, so blocks with no solution also use budget.
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
    // One table of payload bounds by position and remaining capacity: a
    // literal/length position adds the cheapest complete distance code.
    let bound_width = capacity + 1;
    let distance_minimum = distance_bound[capacity];
    let mut payload_bound = filled((n + 1) * bound_width, INF)?;
    for (bound, &literal) in payload_bound[..split * bound_width]
        .iter_mut()
        .zip(&literal_bound)
    {
        *bound = (literal + distance_minimum).min(INF);
    }
    payload_bound[split * bound_width..].copy_from_slice(&distance_bound);
    drop((literal_bound, distance_bound));
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
    // The payload and header suffix minima are computed independently and may
    // assume different lengths; their sum is still a safe lower bound.
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
    if payload_bound[capacity] + header[none] >= target {
        return None;
    }
    // At depth 9 the two tables plus suffix bounds take less than 24 MiB,
    // held for one block at a time.
    let mut cost = filled(cells, INF)?;
    let mut trace = filled(cells, u32::MAX)?;
    let start = capacity * previous_count + none;
    cost[start] = 0;
    // Capacities with a reached state, one bit each in 32-bit words per
    // position, so the search skips capacities no edge has reached.
    let live_words = capacity / 32 + 1;
    let mut live = filled((n + 1) * live_words, 0_u32)?;
    live[capacity / 32] = 1 << (capacity % 32);
    let mut winner = None;
    // Single code lengths the code-length code can spell, in symbol order,
    // with their header bits and capacity units.
    let singles: Vec<_> = (0..=depth)
        .filter(|&length| cl[length] > 0)
        .map(|length| {
            let units = if length == 0 {
                0
            } else {
                1 << (depth - length)
            };
            (length, u64::from(cl[length]), units)
        })
        .collect();
    let repeat_edges = if cl[16] > 0 { 4 } else { 0 };
    let mut work_left = budget.work_left;
    // Relax the edge into position `j` with `next` capacity and previous
    // length `length` at `score`, unless its bound reaches the target.
    macro_rules! relax {
        ($j:expr, $next:expr, $length:expr, $score:expr, $link:expr) => {{
            let (j, next, length, score): (usize, usize, usize, u64) = ($j, $next, $length, $score);
            if sum3(
                score,
                payload_bound[entry(j, bound_width, next)],
                header[entry(j, previous_count, length)],
            ) < target
            {
                let to = entry(j, stride, entry(next, previous_count, length));
                if score < cost[to] {
                    cost[to] = score;
                    trace[to] = $link;
                    live[entry(j, live_words, next / 32)] |= 1 << (next % 32);
                    if j == n {
                        debug_assert_eq!(next, 0);
                        target = score;
                        winner = Some(to);
                    }
                }
            }
        }};
    }
    // Relax the single code lengths from `old` at position `i` and `remaining`
    // capacity: the first `count` the code-length code can spell.
    macro_rules! relax_singles {
        ($i:expr, $remaining:expr, $old:expr, $link:expr, $count:expr) => {{
            let (i, remaining, old, link) = ($i, $remaining, $old, $link);
            let j = i + 1;
            let frequency = u64::from(frequencies[i]);
            for &(length, bits, units) in &singles[..$count] {
                let legal = if length == 0 {
                    frequency == 0
                } else {
                    j <= reserved_from
                };
                if !legal {
                    continue;
                }
                let next = if j == split {
                    if remaining != units {
                        continue;
                    }
                    capacity
                } else {
                    if units > remaining {
                        continue;
                    }
                    remaining - units
                };
                let payload = frequency.wrapping_mul(length as u64);
                relax!(j, next, length, sum3(old, bits, payload), link);
            }
        }};
    }
    // Relax the zero runs (17, then 18) from `old` at position `i` and
    // `remaining` capacity. They stay inside `zero_run[i]`, so they are legal,
    // add no payload and use no capacity; crossing into the distance code
    // needs a complete literal/length code.
    macro_rules! relax_zero_runs {
        ($i:expr, $remaining:expr, $old:expr, $link:expr, $short:expr, $long:expr) => {{
            let (i, remaining, old, link) = ($i, $remaining, $old, $link);
            for (first, count, bits, kind) in [
                (3, $short, u64::from(cl[17]) + 3, 2),
                (11, $long, u64::from(cl[18]) + 7, 3),
            ] {
                let score = old.wrapping_add(bits);
                for j in i + first..i + first + count {
                    let next = if i < split && j >= split {
                        if remaining != 0 {
                            continue;
                        }
                        capacity
                    } else {
                        remaining
                    };
                    relax!(j, next, 0, score, link | kind);
                }
            }
        }};
    }
    'search: for i in 0..n {
        let short_zero_edges = if cl[17] > 0 {
            zero_run[i].min(10).saturating_sub(2)
        } else {
            0
        };
        let long_zero_edges = if cl[18] > 0 {
            zero_run[i].min(138).saturating_sub(10)
        } else {
            0
        };
        // Only an edge that reaches the end lowers the target, so if none can
        // from here, the target holds while position `i` is searched.
        let fixed_target = i + zero_run[i].clamp(6, 138) < n;
        let group_edges = (previous_count
            * (singles.len() + repeat_edges + short_zero_edges + long_zero_edges))
            as u64;
        // The budget and stop are checked at every 32nd capacity, reached or
        // not, before its states.
        for word in 0..live_words {
            if work_left == 0 || stop.reached() {
                break 'search;
            }
            let mut reached = live[i * live_words + word];
            while reached != 0 {
                let remaining = word * 32 + reached.trailing_zeros() as usize;
                reached &= reached - 1;
                let state_bound = payload_bound[i * bound_width + remaining];
                // A single length or zero run from any previous length reaches the
                // same states at the same added cost, and no other edge from this
                // position and capacity reaches them except the 16 repeats of
                // length 0, which come first. With a fixed target and every edge
                // affordable, the first cheapest state's edges therefore set what
                // all of them would, so the others' are charged but not relaxed.
                let merge = fixed_target && group_edges <= work_left;
                let mut cheapest = (INF, 0);
                for previous in 0..previous_count {
                    let from = entry(i, stride, entry(remaining, previous_count, previous));
                    let old = cost[from];
                    if old == INF
                        || sum3(old, state_bound, header[entry(i, previous_count, previous)])
                            >= target
                    {
                        continue;
                    }
                    // Every edge is charged, legal or not, in the order below; a
                    // budget that runs out mid-state cuts the remaining edges.
                    let repeats = if previous == none { 0 } else { repeat_edges };
                    let edges = singles.len() + repeats + short_zero_edges + long_zero_edges;
                    let mut allowance = (edges as u64).min(work_left);
                    work_left -= allowance;
                    let mut take = |count: usize| {
                        let taken = (count as u64).min(allowance);
                        allowance -= taken;
                        taken as usize
                    };
                    // Position always advances, so a predecessor's trace is final
                    // before it is read. There are fewer than 2^21 cells, so
                    // `from << 2` fits in a u32.
                    let link = (from as u32) << 2;

                    let single_count = take(singles.len());
                    if !merge {
                        relax_singles!(i, remaining, old, link, single_count);
                    }

                    let repeats = take(repeats);
                    if repeats > 0 {
                        let units = if previous == 0 {
                            0
                        } else {
                            1 << (depth - previous)
                        };
                        let bits = u64::from(cl[16]) + 2;
                        for count in 3..3 + repeats {
                            let j = i + count;
                            if !legal(i, count, previous) {
                                continue;
                            }
                            let next = if i < split && j >= split {
                                if remaining != (split - i) * units
                                    || (j - split) * units > capacity
                                {
                                    continue;
                                }
                                capacity - (j - split) * units
                            } else {
                                if count * units > remaining {
                                    continue;
                                }
                                remaining - count * units
                            };
                            let payload =
                                sums[j].wrapping_sub(sums[i]).wrapping_mul(previous as u64);
                            let score = sum3(old, bits, payload);
                            relax!(j, next, previous, score, link | 1);
                        }
                    }

                    if merge {
                        if old < cheapest.0 {
                            cheapest = (old, link);
                        }
                    } else {
                        let short = take(short_zero_edges);
                        let long = take(long_zero_edges);
                        relax_zero_runs!(i, remaining, old, link, short, long);
                    }
                }
                if merge && cheapest.0 != INF {
                    let (old, link) = cheapest;
                    relax_singles!(i, remaining, old, link, singles.len());
                    relax_zero_runs!(i, remaining, old, link, short_zero_edges, long_zero_edges);
                }
            }
        }
    }
    budget.work_left = work_left;
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
    // The search produces only strictly compatible codes. Sources outside
    // that domain (possible in relaxed mode) keep their original header.
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
    // Reprice the winning lengths with the general header optimizer, which
    // keeps HLIT and HDIST but may change the code-length code.
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
