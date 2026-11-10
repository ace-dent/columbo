// SPDX-License-Identifier: MIT
// SPDX-FileCopyrightText: 2026 Andrew C.E. Dent <https://github.com/ace-dent>

//! The shortest RLE spelling of code lengths under a fixed code-length code,
//! used by the exhaustive header search and the code-length tree search (R7).
//! No repeat crosses a run of equal lengths, so each run is solved alone, and
//! each thread keeps the spellings of runs by the costs that can spell them.

use super::{length_runs, rle_extra_bits, rle_frequencies, CountedRle, LengthRun, INF};
use crate::deflate::model::RleToken;

/// Find the cheapest RLE spelling of `lengths` under a fixed code-length code.
pub(in crate::deflate) fn shortest_rle(lengths: &[u8], costs: &[u8; 19]) -> Option<Vec<RleToken>> {
    shortest_rle_with_frequencies(lengths, costs, &mut ShortestRleScratch::default())
        .map(|(rle, _)| rle)
}

/// Marks a run length no spelling reaches; no code-length symbol is this large.
const UNSET_RLE_STEP: RleToken = RleToken {
    symbol: u8::MAX,
    extra: 0,
};

/// Slots for run spellings: one for each zero-run key of four 3-bit costs,
/// then one for each other key of two.
const RUN_SPELLING_SLOTS: usize = (1 << 12) + (1 << 6);
/// A slot with no run spellings yet.
const NO_RUN_SPELLINGS: u16 = u16::MAX;

/// Run-spelling entries a thread keeps, 4 bytes each, before dropping them all.
const MAX_RUN_SPELLING_ENTRIES: usize = 1 << 17;

/// The cheapest spellings of runs of one code length, by run length, under the
/// costs that can spell it: its own code and code 16, and for zeros codes 17
/// and 18. No code crosses a run of equal lengths, since code 16 repeats the
/// previous length and codes 17 and 18 write zeros, so a run's spelling depends
/// only on its length and these costs. A token below 16 stands for the run's
/// own length.
#[derive(Default)]
struct RunSpellings {
    /// Whether the length is zero, then the costs these spellings are for.
    key: u64,
    /// `first[run]` starts a run of `run` lengths, or is [`UNSET_RLE_STEP`]
    /// when no spelling exists.
    first: Vec<RleToken>,
    /// `inside[left]` spells on from a later position of a run with `left`
    /// lengths still to spell, where code 16 can repeat the length before.
    inside: Vec<RleToken>,
}

/// Run spellings for [`shortest_rle_with_frequencies`], kept between searches
/// on one thread, and the buffers that build them.
#[derive(Default)]
pub(super) struct ShortestRleScratch {
    /// The index in `spellings` for each cost slot, or [`NO_RUN_SPELLINGS`].
    slots: Vec<u16>,
    spellings: Vec<RunSpellings>,
    /// Run lengths held by all `spellings`.
    entries: usize,
    /// Each count of lengths left's cheapest cost, while spellings are built.
    costs: Vec<u64>,
    /// Code 18's deque while spellings are built.
    zero_repeats: Vec<usize>,
}

impl ShortestRleScratch {
    /// The index of the spellings of runs of `value` up to `run` long under
    /// `costs`, building or extending them if needed, and whether an index
    /// found earlier may now hold other spellings. `None` if buffers cannot
    /// grow.
    fn run_spellings(
        &mut self,
        value: u8,
        costs: &[u8; 19],
        run: usize,
        longest: usize,
    ) -> Option<(u16, bool)> {
        let explicit = costs[usize::from(value)];
        let zero = value == 0;
        let (short_zero, long_zero) = if zero { (costs[17], costs[18]) } else { (0, 0) };
        let key = u64::from(zero) << 32
            | u64::from(u32::from_le_bytes([
                explicit, costs[16], short_zero, long_zero,
            ]));
        let slot = if zero {
            usize::from(explicit & 7)
                | usize::from(costs[16] & 7) << 3
                | usize::from(short_zero & 7) << 6
                | usize::from(long_zero & 7) << 9
        } else {
            (1 << 12) | usize::from(explicit & 7) | usize::from(costs[16] & 7) << 3
        };
        if self.slots.is_empty() {
            self.slots.try_reserve_exact(RUN_SPELLING_SLOTS).ok()?;
            self.slots.resize(RUN_SPELLING_SLOTS, NO_RUN_SPELLINGS);
        }
        let mut index = self.slots[slot];
        if let Some(spellings) = self.spellings.get(usize::from(index)) {
            if spellings.key == key && spellings.first.len() > run {
                return Some((index, false));
            }
        }
        // Build at least twice the old length, so a run that keeps growing
        // rebuilds its spellings only a few times. Costs above 7 bits can
        // share a slot, whose spellings are then rebuilt for the new key.
        let (old, mut replaced) = match self.spellings.get(usize::from(index)) {
            Some(spellings) if spellings.key == key => (spellings.first.len(), false),
            Some(_) => (0, true),
            None => (0, false),
        };
        let length = run.max(old.saturating_mul(2).min(longest)) + 1;
        if self.entries + length > MAX_RUN_SPELLING_ENTRIES {
            replaced = true;
            self.spellings.clear();
            self.slots.fill(NO_RUN_SPELLINGS);
            self.entries = 0;
            index = NO_RUN_SPELLINGS;
        }
        if index == NO_RUN_SPELLINGS {
            index = u16::try_from(self.spellings.len()).ok()?;
            self.spellings.try_reserve(1).ok()?;
            self.spellings.push(RunSpellings::default());
            self.slots[slot] = index;
        }
        let spellings = &mut self.spellings[usize::from(index)];
        self.entries = self.entries - spellings.first.len() + length;
        spellings.key = key;
        build_run_spellings(
            spellings,
            [explicit, costs[16], short_zero, long_zero],
            zero,
            length,
            &mut self.costs,
            &mut self.zero_repeats,
        )?;
        Some((index, replaced))
    }
}

/// Fill `spellings` for runs shorter than `length` under the costs of a run's
/// own code and codes 16, 17 and 18, the last two only for `zero`. Lengths left
/// are solved from a run's end, each from shorter ones. A tie keeps the option
/// offered first: an explicit length, then codes 16, 17 and 18, each with its
/// shortest repeat. Costs stay below `INF` plus a few bits, so sums cannot
/// overflow, and a sum through an unreachable `INF` never wins.
fn build_run_spellings(
    spellings: &mut RunSpellings,
    [explicit, repeat, short_zero, long_zero]: [u8; 4],
    zero: bool,
    length: usize,
    costs: &mut Vec<u64>,
    zero_repeats: &mut Vec<usize>,
) -> Option<()> {
    let RunSpellings { first, inside, .. } = spellings;
    for buffer in [&mut *first, &mut *inside] {
        buffer.clear();
        buffer.try_reserve_exact(length).ok()?;
    }
    costs.clear();
    costs.try_reserve_exact(length).ok()?;
    zero_repeats.clear();
    zero_repeats.try_reserve_exact(length).ok()?;
    let price =
        |cost: u8, symbol: u8| (cost != 0).then(|| u64::from(cost) + rle_extra_bits(symbol));
    let explicit = price(explicit, 0);
    let repeat = price(repeat, 16);
    let short_zero = if zero { price(short_zero, 17) } else { None };
    let long_zero = if zero { price(long_zero, 18) } else { None };

    // `costs[left]` is the cheapest spelling of a run's last `left` lengths
    // past its first; a run's end costs nothing more.
    first.push(UNSET_RLE_STEP);
    inside.push(UNSET_RLE_STEP);
    costs.push(0);
    // Code 18 costs the same for every repeat from 11 to 138, so a monotonic
    // deque in `zero_repeats[front..]` keeps the cheapest of what it can leave
    // to spell. Newer entries are shorter repeats, so dropping equal costs
    // keeps the preference for the shortest.
    let mut front = 0;
    for left in 1..length {
        let mut best_inside = (INF, UNSET_RLE_STEP);
        let mut best_first = (INF, UNSET_RLE_STEP);
        let mut offer = |cost: u64, symbol: u8, extra: u8, at_first: bool| {
            let token = RleToken { symbol, extra };
            if cost < best_inside.0 {
                best_inside = (cost, token);
            }
            if at_first && cost < best_first.0 {
                best_first = (cost, token);
            }
        };
        let long_count = if long_zero.is_some() && left >= 11 {
            let newest = left - 11;
            while front < zero_repeats.len() && left - zero_repeats[front] > 138 {
                front += 1;
            }
            while zero_repeats.len() > front
                && zero_repeats
                    .last()
                    .is_some_and(|&last| costs[last] >= costs[newest])
            {
                zero_repeats.pop();
            }
            zero_repeats.push(newest);
            Some(left - zero_repeats[front])
        } else {
            None
        };
        if let Some(explicit) = explicit {
            offer(explicit + costs[left - 1], 0, 0, true);
        }
        if left >= 3 {
            // Repeats in order of count, so the earliest minimum is the
            // shortest repeat.
            let window = |most: usize| {
                let mut count = 3;
                for candidate in 4..=left.min(most) {
                    if costs[left - candidate] < costs[left - count] {
                        count = candidate;
                    }
                }
                count
            };
            if let Some(repeat) = repeat {
                let count = window(6);
                offer(repeat + costs[left - count], 16, (count - 3) as u8, false);
            }
            if let Some(short_zero) = short_zero {
                let count = window(10);
                offer(
                    short_zero + costs[left - count],
                    17,
                    (count - 3) as u8,
                    true,
                );
            }
            if let (Some(long_zero), Some(count)) = (long_zero, long_count) {
                offer(
                    long_zero + costs[left - count],
                    18,
                    (count - 11) as u8,
                    true,
                );
            }
        }
        costs.push(best_inside.0);
        inside.push(best_inside.1);
        first.push(best_first.1);
    }
    Some(())
}

/// [`shortest_rle`] with the RLE's symbol frequencies. Each run of equal
/// lengths takes its cheapest spelling, kept in `scratch` for the next search
/// with the same costs. `None` also when the buffers cannot grow.
fn shortest_rle_with_frequencies(
    lengths: &[u8],
    costs: &[u8; 19],
    scratch: &mut ShortestRleScratch,
) -> Option<CountedRle> {
    shortest_rle_from_runs(&length_runs(lengths), lengths.len(), costs, scratch)
}

/// [`shortest_rle_with_frequencies`] for code lengths given as their `runs`,
/// `length_count` lengths in all.
fn shortest_rle_from_runs(
    runs: &[LengthRun],
    length_count: usize,
    costs: &[u8; 19],
    scratch: &mut ShortestRleScratch,
) -> Option<CountedRle> {
    let mut output = Vec::new();
    let frequencies = shortest_rle_from_runs_into(runs, length_count, costs, scratch, &mut output)?;
    Some((output, frequencies))
}

/// [`shortest_rle_from_runs`] into `output`, which this clears; returns the
/// RLE's symbol frequencies, or `None`, leaving `output` unspecified.
pub(super) fn shortest_rle_from_runs_into(
    runs: &[LengthRun],
    length_count: usize,
    costs: &[u8; 19],
    scratch: &mut ShortestRleScratch,
    output: &mut Vec<RleToken>,
) -> Option<[u32; 19]> {
    // Each token spells at least one length.
    output.clear();
    output.try_reserve_exact(length_count).ok()?;
    // Counted as spelled: each run's explicit lengths, and the repeat codes.
    let mut frequencies = [0_u32; 19];
    let mut repeats = [0_u32; 3];
    // This search's spellings by length, found once each.
    let mut by_value = [NO_RUN_SPELLINGS; 16];
    for &(value, mut run) in runs {
        let known = by_value
            .get(usize::from(value))
            .and_then(|&known| scratch.spellings.get(usize::from(known)))
            .filter(|spellings| spellings.first.len() > run);
        let spellings = match known {
            Some(spellings) => spellings,
            None => {
                let (found, replaced) = scratch.run_spellings(value, costs, run, length_count)?;
                if replaced {
                    by_value = [NO_RUN_SPELLINGS; 16];
                }
                if let Some(known) = by_value.get_mut(usize::from(value)) {
                    *known = found;
                }
                &scratch.spellings[usize::from(found)]
            }
        };
        let mut token = spellings.first[run];
        if run == 1 && token.symbol < 16 {
            output.push(RleToken {
                symbol: value,
                extra: 0,
            });
            frequencies[usize::from(value)] += 1;
            continue;
        }
        let mut explicit = 0;
        loop {
            let count = match token.symbol {
                0..=15 => 1,
                16 | 17 => usize::from(token.extra) + 3,
                18 => usize::from(token.extra) + 11,
                _ => return None,
            };
            if token.symbol < 16 {
                output.push(RleToken {
                    symbol: value,
                    extra: 0,
                });
                explicit += 1;
            } else {
                output.push(token);
                repeats[usize::from(token.symbol - 16)] += 1;
            }
            run -= count;
            if run == 0 {
                break;
            }
            token = spellings.inside[run];
        }
        frequencies[usize::from(value)] += explicit;
    }
    frequencies[16..].copy_from_slice(&repeats);
    debug_assert_eq!(frequencies, rle_frequencies(output));
    Some(frequencies)
}

#[cfg(test)]
mod tests;
