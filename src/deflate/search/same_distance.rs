// SPDX-License-Identifier: MIT
// SPDX-FileCopyrightText: 2026 Andrew C.E. Dent <https://github.com/ace-dent>

//! Same-distance match-run normalization.
//!
//! A run of adjacent matches at one distance copies every byte from that
//! distance, overlapping copies included, so its internal boundaries can move
//! anywhere that leaves each match 3–258 bytes long. A run that decodes `S`
//! bytes needs only `ceil(S / 258)` matches; a dynamic program over the bytes
//! short of an all-258 partition picks the cheapest lengths under the current
//! codes. Default bounds the program's depth and otherwise uses a legal
//! minimum-token partition. The run counts for verbose reports are measured
//! here too.

use std::sync::Arc;

use crate::deflate::huffman::FIXED_LITERAL_CODE_LENGTHS;
use crate::deflate::minima::SuffixMinima;
use crate::deflate::model::{
    same_tokens, ParsedBlock, PlannedBlock, Representation, Token,
    LENGTH_BASE as DEFLATE_LENGTH_BASE, LENGTH_EXTRA_BITS,
};
use crate::deflate::stop::SearchStop;
use crate::sort::SortShared;
use crate::Options;

use super::{
    consider_tokens, estimated_length, new_token_candidate, plan_tokens, repacked_match,
    rewrite_258_symbols,
};

/// Most shortened matches (DP layers) for which Default solves a same-distance
/// repartition exactly; above it Default uses `append_minimum_token_fallback`.
const DEFAULT_SAME_DISTANCE_MAX_DP_ACTIVE: usize = 16;

/// Diagnostics for adjacent matches at one distance, measured on the parsed
/// input so verbose reports do not depend on search order or timeout. Runs
/// cross source-block boundaries: an empty block does not end a run, but
/// stored bytes and literals do.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct SameDistanceOpportunities {
    pub(crate) runs: usize,
    pub(crate) matches: usize,
    pub(crate) decoded_bytes: usize,
    pub(crate) coalescible_runs: usize,
    pub(crate) repartition_runs: usize,
    pub(crate) tokens_removable: usize,
}

#[derive(Debug, Clone, Copy)]
struct SameDistanceRun {
    start: usize,
    end: usize,
    decoded_bytes: usize,
    first: Token,
}

fn same_distance_run_at(tokens: &[Token], start: usize) -> Option<SameDistanceRun> {
    let first = *tokens.get(start)?;
    let Token::Match {
        length, distance, ..
    } = first
    else {
        return None;
    };
    let mut decoded_bytes = usize::from(length);
    let mut end = start + 1;
    while let Some(Token::Match {
        length,
        distance: next_distance,
        ..
    }) = tokens.get(end)
    {
        if *next_distance != distance {
            break;
        }
        decoded_bytes = decoded_bytes.checked_add(usize::from(*length))?;
        end += 1;
    }
    (end >= start + 2).then_some(SameDistanceRun {
        start,
        end,
        decoded_bytes,
        first,
    })
}

pub(crate) fn same_distance_opportunities(blocks: &[ParsedBlock]) -> SameDistanceOpportunities {
    let mut opportunities = SameDistanceOpportunities::default();
    let mut run_distance = None;
    let mut run_matches = 0_usize;
    let mut run_bytes = 0_usize;
    for block in blocks {
        for token in block.tokens.iter() {
            match *token {
                Token::Match {
                    length, distance, ..
                } if run_distance == Some(distance) => {
                    run_matches = run_matches.saturating_add(1);
                    run_bytes = run_bytes.saturating_add(usize::from(length));
                }
                Token::Match {
                    length, distance, ..
                } => {
                    record_same_distance_opportunity(&mut opportunities, run_matches, run_bytes);
                    run_distance = Some(distance);
                    run_matches = 1;
                    run_bytes = usize::from(length);
                }
                Token::Literal(_) => {
                    record_same_distance_opportunity(&mut opportunities, run_matches, run_bytes);
                    run_distance = None;
                    run_matches = 0;
                    run_bytes = 0;
                }
            }
        }
    }
    record_same_distance_opportunity(&mut opportunities, run_matches, run_bytes);
    opportunities
}

fn record_same_distance_opportunity(
    opportunities: &mut SameDistanceOpportunities,
    matches: usize,
    decoded_bytes: usize,
) {
    if matches < 2 {
        return;
    }
    let minimum_matches = decoded_bytes.saturating_add(257) / 258;
    opportunities.runs = opportunities.runs.saturating_add(1);
    opportunities.matches = opportunities.matches.saturating_add(matches);
    opportunities.decoded_bytes = opportunities.decoded_bytes.saturating_add(decoded_bytes);
    opportunities.tokens_removable = opportunities
        .tokens_removable
        .saturating_add(matches.saturating_sub(minimum_matches));
    if decoded_bytes <= 258 {
        opportunities.coalescible_runs = opportunities.coalescible_runs.saturating_add(1);
    } else if matches > minimum_matches
        || 258_usize
            .saturating_mul(minimum_matches)
            .saturating_sub(decoded_bytes)
            != 0
    {
        opportunities.repartition_runs = opportunities.repartition_runs.saturating_add(1);
    }
}

/// Add one same-distance run candidate to an existing plan, always with Default
/// planning, even from a Max floor.
pub(crate) fn improve_plan_with_same_distance_floor(
    block: &ParsedBlock,
    alignment: u8,
    options: &Options,
    mut best: PlannedBlock,
) -> PlannedBlock {
    let mut floor_options = options.clone();
    floor_options.exhaustive = false;
    consider_same_distance_runs(
        block,
        alignment,
        &floor_options,
        &mut SearchStop::never(),
        &mut best,
    );
    best
}

pub(super) fn consider_same_distance_runs(
    block: &ParsedBlock,
    alignment: u8,
    options: &Options,
    stop: &mut SearchStop<'_>,
    best: &mut PlannedBlock,
) {
    if block.tokens.len() < 2 || stop.reached() {
        return;
    }
    let literal_lengths: &[u8] = match &best.representation {
        Representation::Dynamic(dynamic) => &dynamic.literal_lengths,
        Representation::Fixed => &FIXED_LITERAL_CODE_LENGTHS,
        Representation::Original(_) | Representation::Stored => block
            .original_literal_lengths
            .as_ref()
            .map_or(&FIXED_LITERAL_CODE_LENGTHS, |lengths| lengths),
    };
    let Some(repacked) = repack_same_distance_runs(
        &block.tokens,
        block.plain.len(),
        literal_lengths,
        options.exhaustive,
        stop,
    ) else {
        return;
    };

    let Some(candidate) = plan_tokens(block, repacked, alignment, options, stop) else {
        return;
    };
    let canonical_tokens = Arc::clone(&candidate.tokens);
    if candidate.bits < best.bits {
        *best = candidate;
    }
    if !options.strict
        && !stop.reached()
        && canonical_tokens.iter().any(|token| {
            matches!(
                token,
                Token::Match {
                    length: 258,
                    length_symbol: 285,
                    ..
                }
            )
        })
    {
        if let Some(aliased) = rewrite_258_symbols(&canonical_tokens, block.plain.len(), true) {
            consider_tokens(block, aliased, alignment, options, stop, best);
        }
    }
}

/// Repartition every maximal run of adjacent matches with the same distance.
///
/// A run that decodes `S` bytes needs only `ceil(S / 258)` matches. Every byte
/// of the run copies from the same distance (overlapping copies included), so
/// moving internal boundaries is lossless.
fn repack_same_distance_runs(
    tokens: &[Token],
    decoded_bytes: usize,
    literal_lengths: &[u8],
    exhaustive: bool,
    stop: &mut SearchStop<'_>,
) -> Option<Vec<Token>> {
    let mut output_count = tokens.len();
    let mut decoded_total = 0_usize;
    let mut max_active = 0_usize;
    let mut max_deficit = 0_usize;
    let mut has_run = false;
    let mut index = 0_usize;
    while index < tokens.len() {
        if let Some(run) = same_distance_run_at(tokens, index) {
            has_run = true;
            decoded_total = decoded_total.checked_add(run.decoded_bytes)?;
            let source_matches = run.end - run.start;
            let output_matches = run.decoded_bytes.checked_add(257)?.checked_div(258)?;
            output_count = output_count.checked_sub(source_matches.checked_sub(output_matches)?)?;
            let deficit = 258_usize
                .checked_mul(output_matches)?
                .checked_sub(run.decoded_bytes)?;
            debug_assert!(deficit <= 257);
            let active = output_matches.min(deficit);
            if run.decoded_bytes > 258
                && deficit != 0
                && (exhaustive || active <= DEFAULT_SAME_DISTANCE_MAX_DP_ACTIVE)
            {
                max_active = max_active.max(active);
                max_deficit = max_deficit.max(deficit);
            }
            index = run.end;
        } else {
            decoded_total = decoded_total.checked_add(tokens[index].decoded_len())?;
            index += 1;
        }
    }
    if !has_run || decoded_total != decoded_bytes {
        return None;
    }

    let partitioner = if max_active == 0 {
        None
    } else {
        Some(SameDistancePartitioner::new(
            literal_lengths,
            max_active,
            max_deficit,
            stop,
        )?)
    };
    let mut output = new_token_candidate(output_count, decoded_bytes)?;
    let mut changed = false;
    index = 0;
    while index < tokens.len() {
        let Some(run) = same_distance_run_at(tokens, index) else {
            output.push(tokens[index]);
            index += 1;
            continue;
        };
        let output_matches = run.decoded_bytes.checked_add(257)?.checked_div(258)?;
        let deficit = 258_usize
            .checked_mul(output_matches)?
            .checked_sub(run.decoded_bytes)?;
        let output_start = output.len();
        if run.decoded_bytes <= 258 {
            output.push(repacked_match(
                run.first,
                run.decoded_bytes.try_into().ok()?,
            )?);
        } else if deficit == 0 {
            for _ in 0..output_matches {
                output.push(repacked_match(run.first, 258)?);
            }
        } else {
            let active = output_matches.min(deficit);
            if exhaustive || active <= DEFAULT_SAME_DISTANCE_MAX_DP_ACTIVE {
                let lengths = partitioner.as_ref()?.active_lengths(active, deficit)?;
                for _ in 0..output_matches.checked_sub(active)? {
                    output.push(repacked_match(run.first, 258)?);
                }
                for length in lengths {
                    output.push(repacked_match(run.first, length)?);
                }
            } else {
                append_minimum_token_fallback(&mut output, run.first, output_matches, deficit)?;
            }
        }
        changed |= !same_tokens(&output[output_start..], &tokens[run.start..run.end]);
        index = run.end;
    }
    debug_assert_eq!(output.len(), output_count);
    changed.then_some(output)
}

/// Append a legal minimum-token partition without running the cost DP.
///
/// The total deficit is at most 257 and each match can absorb up to 255
/// (length 3), so at most two matches are shortened.
fn append_minimum_token_fallback(
    output: &mut Vec<Token>,
    source: Token,
    output_matches: usize,
    deficit: usize,
) -> Option<()> {
    debug_assert!((1..=257).contains(&deficit));
    let shortened = deficit.checked_add(254)?.checked_div(255)?;
    for _ in 0..output_matches.checked_sub(shortened)? {
        output.push(repacked_match(source, 258)?);
    }
    let mut remaining = deficit;
    for slots_left in (1..=shortened).rev() {
        let reserved = 255_usize.checked_mul(slots_left.checked_sub(1)?)?;
        let token_deficit = remaining.saturating_sub(reserved);
        output.push(repacked_match(
            source,
            258_u16.checked_sub(token_deficit.try_into().ok()?)?,
        )?);
        remaining = remaining.checked_sub(token_deficit)?;
    }
    (remaining == 0).then_some(())
}

struct SameDistancePartitioner {
    choices: Vec<u16>,
    width: usize,
    max_active: usize,
}

impl SameDistancePartitioner {
    /// Build a DP over the deficit (bytes short of an all-258 partition). The
    /// total deficit is at most 257 bytes and at most `deficit` matches are
    /// shorter than 258, so long runs add no DP depth.
    fn new(
        literal_lengths: &[u8],
        max_active: usize,
        max_deficit: usize,
        stop: &mut SearchStop<'_>,
    ) -> Option<Self> {
        let width = max_deficit.checked_add(1)?;
        let choice_count = max_active.checked_add(1)?.checked_mul(width)?;
        let mut choices = Vec::new();
        choices.try_reserve_exact(choice_count).ok()?;
        choices.resize(choice_count, u16::MAX);

        // Visit length symbols in ascending deficit order, keeping the first
        // choice on equal costs. Length 258 (symbol 285) is its own family.
        let families: [_; DEFLATE_LENGTH_BASE.len()] = std::array::from_fn(|offset| {
            let index = DEFLATE_LENGTH_BASE.len() - 1 - offset;
            let first = DEFLATE_LENGTH_BASE
                .get(index + 1)
                .map_or(0, |&next| 259 - usize::from(next));
            let last = 258 - usize::from(DEFLATE_LENGTH_BASE[index]);
            let cost = estimated_length(literal_lengths, 257 + index)
                + u64::from(LENGTH_EXTRA_BITS[index]);
            (first, last, cost)
        });

        // Reverse the deficit axis so a suffix minimum's lowest position
        // chooses the smallest token deficit.
        let mut previous = [u64::MAX; 259];
        let mut current = [u64::MAX; 259];
        let mut minima = SuffixMinima::new(max_deficit);
        previous[max_deficit] = 0;
        for slot in 1..=max_active {
            if stop.reached() {
                return None;
            }
            for used_deficit in 0..=max_deficit {
                if used_deficit & 31 == 0 && stop.reached() {
                    return None;
                }
                let start = max_deficit - used_deficit;
                minima.insert(start, max_deficit, &previous);
                let mut best_cost = u64::MAX;
                let mut best_deficit = u16::MAX;
                for &(first, last, cost) in &families {
                    if first > used_deficit {
                        break;
                    }
                    let position =
                        minima.minimum(start + first, start + last.min(used_deficit), &previous);
                    let prefix = previous[position];
                    if prefix == u64::MAX {
                        continue;
                    }
                    let candidate = prefix.checked_add(cost)?;
                    if candidate < best_cost {
                        best_cost = candidate;
                        best_deficit = (position - start) as u16;
                    }
                }
                current[start] = best_cost;
                choices[slot * width + used_deficit] = best_deficit;
            }
            previous = current;
        }

        Some(Self {
            choices,
            width,
            max_active,
        })
    }

    fn active_lengths(&self, active: usize, deficit: usize) -> Option<Vec<u16>> {
        if active > self.max_active || deficit >= self.width {
            return None;
        }
        let mut lengths = Vec::new();
        lengths.try_reserve_exact(active).ok()?;
        let mut remaining = deficit;
        for slot in (1..=active).rev() {
            let token_deficit = *self.choices.get(slot * self.width + remaining)?;
            if token_deficit == u16::MAX {
                return None;
            }
            remaining = remaining.checked_sub(usize::from(token_deficit))?;
            lengths.push(258_u16.checked_sub(token_deficit)?);
        }
        if remaining != 0 {
            return None;
        }
        lengths.sort_shared_by(|left, right| right.cmp(left));
        Some(lengths)
    }
}

#[cfg(test)]
mod tests;
