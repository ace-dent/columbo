// SPDX-License-Identifier: MIT
// SPDX-FileCopyrightText: 2026 Andrew C.E. Dent <https://github.com/ace-dent>

//! Split blocks where their symbol statistics change (R15).
//!
//! A Huffman block pays for one table, so a block whose literal/length or
//! distance statistics change part-way codes both regions with a compromise.
//! One pass over a token range keeps the histograms on each side of a moving
//! cut, with their sums of `c log2 c`, so the zeroth-order entropy of every
//! token cut is known exactly. The cut with the lowest total is priced with the
//! bounded planner and kept only if the whole block becomes strictly smaller;
//! each kept half is then cut the same way. Entropy ignores the header each
//! side must send, so when that cut fails, the cut with the lowest entropy plus
//! a header estimate is priced instead; it favors cuts that shrink a side's
//! alphabet.

use std::collections::HashMap;

use super::super::header::HeaderPlanCache;
use super::alphabet::{price_cuts, AlphabetBudget, EdgeCache};
use super::*;

/// Fewest tokens on either side of a cut.
const MIN_SIDE_TOKENS: usize = 16;
/// Smallest estimated entropy saving, in bits, worth an exact price.
const MIN_ESTIMATED_SAVING_BITS: u64 = 32;
/// Fraction bits of the fixed-point logarithms.
const LOG_FRACTION_BITS: u32 = 16;
/// Counts below this read `c log2 c` from a table.
const WEIGHTED_LOG_TABLE_LEN: usize = 4096;
/// The table grows to cover a block's counts and totals, up to this length;
/// larger counts are computed.
const MAX_WEIGHTED_LOG_TABLE_LEN: usize = 1 << 19;
/// Tokens scanned between stop polls.
const SCAN_POLL_TOKENS: usize = 1 << 16;
/// A dynamic header's fixed part, in bits: block type and the HLIT, HDIST and
/// HCLEN fields (17 bits) and about fifteen 3-bit code-length-code lengths.
const HEADER_BASE_BITS: u64 = 64;
/// Header bits per used literal/length or distance symbol: its code-length
/// codeword. Runs of unused symbols cost little under the repeat codes.
const HEADER_BITS_PER_SYMBOL: u64 = 4;

/// Exact range prices per pass.
const RANGE_PRICES: usize = 4096;

/// Work and exact-price budget shared by one terminal pass's entropy cuts.
pub(crate) struct EntropyCutBudget {
    ranges: AlphabetBudget,
    weighted_logs: Vec<u64>,
}

impl EntropyCutBudget {
    /// The budget for one pass over `blocks`: [`terminal_layout_work`] units
    /// and [`RANGE_PRICES`] prices.
    pub(crate) fn for_stream(blocks: &[ParsedBlock]) -> Self {
        Self {
            ranges: AlphabetBudget::with_limits(terminal_layout_work(blocks), RANGE_PRICES),
            weighted_logs: Vec::new(),
        }
    }
}

/// `log2(value)` with [`LOG_FRACTION_BITS`] fraction bits, for `value >= 1`,
/// by repeated squaring of the normalized mantissa. Integer arithmetic keeps
/// cut choices identical on every platform.
fn log2_fixed(value: u64) -> u64 {
    debug_assert!(value >= 1);
    let integer = u64::from(63 - value.leading_zeros());
    // The mantissa in [1, 2) with 31 fraction bits, so its square fits.
    let mut mantissa = if integer >= 31 {
        value >> (integer - 31)
    } else {
        value << (31 - integer)
    };
    let mut fraction = 0;
    for bit in (0..LOG_FRACTION_BITS).rev() {
        mantissa = (mantissa * mantissa) >> 31;
        if mantissa >= 1 << 32 {
            mantissa >>= 1;
            fraction |= 1 << bit;
        }
    }
    (integer << LOG_FRACTION_BITS) | fraction
}

/// `count × log2(count)` in fixed point; zero for counts 0 and 1.
fn weighted_log(table: &[u64], count: u64) -> u64 {
    match table.get(count as usize) {
        Some(&value) => value,
        None => count * log2_fixed(count),
    }
}

/// Fill `table` with `c log2 c` for every count below `len`: at least
/// [`WEIGHTED_LOG_TABLE_LEN`] entries, and as many as the allocator gives up
/// to [`MAX_WEIGHTED_LOG_TABLE_LEN`]. Scans read each side's totals and its
/// frequent symbols' counts at every token, so a block's counts are
/// computed once, not at each cut.
fn weighted_log_table(table: &mut Vec<u64>, len: usize) -> Option<()> {
    let len = len.clamp(WEIGHTED_LOG_TABLE_LEN, MAX_WEIGHTED_LOG_TABLE_LEN);
    if table.len() >= len {
        return Some(());
    }
    if table.try_reserve_exact(len - table.len()).is_err() {
        if !table.is_empty() {
            return Some(());
        }
        table.try_reserve_exact(WEIGHTED_LOG_TABLE_LEN).ok()?;
    }
    if table.is_empty() {
        table.push(0);
    }
    let end = len.min(table.capacity()) as u64;
    let start = table.len() as u64;
    table.extend((start..end).map(|count| count * log2_fixed(count)));
    Some(())
}

/// One side of a cut: literal/length (including one end-of-block) and distance
/// histograms with their totals and sums of `c log2 c`.
#[derive(Clone)]
struct Side {
    literal: [u32; 286],
    distance: [u32; 30],
    literal_total: u64,
    distance_total: u64,
    literal_weighted: u64,
    distance_weighted: u64,
    /// Symbols with a nonzero count, end-of-block included.
    used: u64,
}

impl Side {
    fn end_of_block(table: &[u64]) -> Self {
        let mut literal = [0; 286];
        literal[256] = 1;
        Self {
            literal,
            distance: [0; 30],
            literal_total: 1,
            distance_total: 0,
            literal_weighted: weighted_log(table, 1),
            distance_weighted: 0,
            used: 1,
        }
    }

    fn add(&mut self, table: &[u64], token: &Token) {
        let (literal, distance) = token_symbols(token);
        let count = &mut self.literal[literal];
        self.literal_weighted +=
            weighted_log(table, u64::from(*count) + 1) - weighted_log(table, u64::from(*count));
        self.used += u64::from(*count == 0);
        *count += 1;
        self.literal_total += 1;
        if let Some(distance) = distance {
            let count = &mut self.distance[distance];
            self.distance_weighted +=
                weighted_log(table, u64::from(*count) + 1) - weighted_log(table, u64::from(*count));
            self.used += u64::from(*count == 0);
            *count += 1;
            self.distance_total += 1;
        }
    }

    fn remove(&mut self, table: &[u64], token: &Token) {
        let (literal, distance) = token_symbols(token);
        let count = &mut self.literal[literal];
        self.literal_weighted -=
            weighted_log(table, u64::from(*count)) - weighted_log(table, u64::from(*count) - 1);
        *count -= 1;
        self.used -= u64::from(*count == 0);
        self.literal_total -= 1;
        if let Some(distance) = distance {
            let count = &mut self.distance[distance];
            self.distance_weighted -=
                weighted_log(table, u64::from(*count)) - weighted_log(table, u64::from(*count) - 1);
            *count -= 1;
            self.used -= u64::from(*count == 0);
            self.distance_total -= 1;
        }
    }

    /// Zeroth-order entropy of both alphabets, in fixed-point bits.
    fn entropy(&self, table: &[u64]) -> u64 {
        weighted_log(table, self.literal_total) - self.literal_weighted
            + weighted_log(table, self.distance_total)
            - self.distance_weighted
    }

    /// A first-order estimate of the side's dynamic header, in fixed-point
    /// bits.
    fn header_estimate(&self) -> u64 {
        (HEADER_BASE_BITS + HEADER_BITS_PER_SYMBOL * self.used) << LOG_FRACTION_BITS
    }
}

fn token_symbols(token: &Token) -> (usize, Option<usize>) {
    match *token {
        Token::Literal(value) => (usize::from(value), None),
        Token::Match {
            length_symbol,
            distance_symbol,
            ..
        } => (
            usize::from(length_symbol),
            Some(usize::from(distance_symbol)),
        ),
    }
}

/// The cuts to price for `start..end`, in order: the token cut that minimizes
/// the two sides' total entropy, when it saves at least
/// [`MIN_ESTIMATED_SAVING_BITS`]; then, if different, the cut that minimizes
/// their entropy plus header estimates, when that total is below the whole
/// range's. The outer `None` means the stop or budget ended the search.
fn best_entropy_cuts(
    tokens: &[Token],
    start: usize,
    end: usize,
    table: &[u64],
    budget: &mut AlphabetBudget,
    stop: &mut SearchStop<'_>,
) -> Option<[Option<usize>; 2]> {
    if end - start < 2 * MIN_SIDE_TOKENS {
        return Some([None; 2]);
    }
    budget.spend(2 * (end - start))?;
    let mut right = Side::end_of_block(table);
    for token in &tokens[start..end] {
        right.add(table, token);
    }
    let whole = right.entropy(table);
    let whole_with_header = whole + right.header_estimate();
    let mut left = Side::end_of_block(table);
    let mut best: Option<(u64, usize)> = None;
    let mut best_with_headers: Option<(u64, usize)> = None;
    for (offset, token) in tokens[start..end - MIN_SIDE_TOKENS].iter().enumerate() {
        if offset % SCAN_POLL_TOKENS == 0 && stop.reached() {
            return None;
        }
        left.add(table, token);
        right.remove(table, token);
        if offset + 1 < MIN_SIDE_TOKENS {
            continue;
        }
        let split = left.entropy(table) + right.entropy(table);
        if best.map_or(true, |(cost, _)| split < cost) {
            best = Some((split, start + offset + 1));
        }
        let with_headers = split + left.header_estimate() + right.header_estimate();
        if best_with_headers.map_or(true, |(cost, _)| with_headers < cost) {
            best_with_headers = Some((with_headers, start + offset + 1));
        }
    }
    let entropy_cut = best.and_then(|(split, cut)| {
        (whole.saturating_sub(split) >= MIN_ESTIMATED_SAVING_BITS << LOG_FRACTION_BITS)
            .then_some(cut)
    });
    let header_cut = best_with_headers
        .filter(|&(split, cut)| split < whole_with_header && Some(cut) != entropy_cut)
        .map(|(_, cut)| cut);
    Some([entropy_cut, header_cut])
}

/// R15 for one block: cut it where the zeroth-order entropy of the two sides
/// falls most, or else where entropy plus header estimates falls most, keep
/// the first cut whose exactly planned block sequence is strictly smaller, and
/// cut each kept half again. Returns the plans of the smallest
/// accepted split; completed cuts are kept when the stop or budget ends the
/// search.
pub(crate) fn plan_entropy_cuts(
    block: &ParsedBlock,
    alignment: u8,
    options: &Options,
    budget: &mut EntropyCutBudget,
    stop: &mut SearchStop<'_>,
) -> Option<Vec<PlannedBlock>> {
    if stop.reached()
        || block.source_type == SourceBlockType::Stored
        || block.tokens.len() < 2 * MIN_SIDE_TOKENS
    {
        return None;
    }
    let parent_bits = reusable_original_bits(block, alignment, options.strict)?.len;
    let composite = Composite::new(std::slice::from_ref(block))?;
    let token_count = composite.tokens.len();
    // Pricing uses the bounded table search in both modes, like R6.
    let bounded = Options {
        exhaustive: false,
        ..options.clone()
    };
    let EntropyCutBudget {
        ranges,
        weighted_logs,
    } = budget;
    // A side's totals reach the token count plus its end-of-block code.
    weighted_log_table(weighted_logs, token_count + 2)?;
    let mut cache = EdgeCache {
        edges: HashMap::new(),
        headers: HeaderPlanCache::new(),
        budget: ranges,
    };
    let mut points = Vec::new();
    points.try_reserve(2).ok()?;
    points.extend([0, token_count]);
    let mut pending = Vec::new();
    pending.try_reserve(2).ok()?;
    pending.push((0, token_count));
    let mut best_bits = parent_bits;
    let mut best = None;
    'ranges: while let Some((start, end)) = pending.pop() {
        let Some(cuts) = best_entropy_cuts(
            &composite.tokens,
            start,
            end,
            weighted_logs,
            cache.budget,
            stop,
        ) else {
            break;
        };
        for at in cuts.into_iter().flatten() {
            if points.try_reserve(1).is_err() || pending.try_reserve(2).is_err() {
                break 'ranges;
            }
            let index = points.partition_point(|&point| point < at);
            points.insert(index, at);
            let Some(plans) = price_cuts(
                block, &composite, &points, alignment, &bounded, &mut cache, stop,
            ) else {
                break 'ranges;
            };
            let bits = total_bits(&plans);
            if bits < best_bits {
                best_bits = bits;
                best = Some(plans);
                // Left half first.
                pending.push((at, end));
                pending.push((start, at));
                continue 'ranges;
            }
            points.remove(index);
        }
    }
    best
}

#[cfg(test)]
mod tests;
