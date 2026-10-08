// SPDX-License-Identifier: MIT
// SPDX-FileCopyrightText: 2026 Andrew C.E. Dent <https://github.com/ace-dent>

//! Collapse a block's distance alphabet to a kept set of distance symbols.
//!
//! Every match outside the kept set is spelled as its decoded literals, so the
//! distance tree loses those codes and the length tree those matches. On image
//! data with a strong period, a few short distances can carry the block while
//! many rare far distances lengthen every code: zenflate's PNG mode compares
//! each block with a runs-only parse for this reason. Columbo cannot add the
//! distance-1 matches such a parse would find; it only removes existing ones.
//!
//! The kept sets form three ladders over the used distance symbols: the
//! shortest distances first, the most frequent first, and each symbol alone.
//! One pass records what removing each symbol's matches does to the
//! histograms, so every rung is estimated from frequencies without building
//! its tokens. Only the two best rungs whose estimate does not exceed the
//! parent's own estimate are materialized and priced exactly.

use super::block::plan_owned_block;
use super::header::estimate_boundary_block_bits;
use super::model::{token_extra_bits, ParsedBlock, PlannedBlock, SourceBlockType, Token};
use super::search::{expand_selected_matches, try_transformed_block};
use super::stop::SearchStop;
use crate::Options;

/// Rungs priced exactly per block, in estimate order. A rung is priced only
/// when its estimate does not exceed the same estimate of the parent block.
const MAX_EXACT_RUNGS: usize = 2;
/// Two ladders of at most 29 rungs each, plus one rung per used symbol.
const MAX_RUNGS: usize = 29 + 29 + 30;

/// Histogram changes from spelling every match of one distance symbol as its
/// decoded literals.
struct Removals {
    literals: Vec<[u32; 256]>,
    lengths: [[u32; 29]; 30],
    extra_bits: [u64; 30],
}

impl Removals {
    fn collect(block: &ParsedBlock, stop: &mut SearchStop<'_>) -> Option<Self> {
        let mut literals = Vec::new();
        literals.try_reserve_exact(30).ok()?;
        literals.resize(30, [0_u32; 256]);
        let mut removals = Self {
            literals,
            lengths: [[0; 29]; 30],
            extra_bits: [0; 30],
        };
        let mut position = 0_usize;
        for (index, &token) in block.tokens.iter().enumerate() {
            if index & 4095 == 0 && stop.reached() {
                return None;
            }
            let end = position.checked_add(token.decoded_len())?;
            if let Token::Match {
                length_symbol,
                distance_symbol,
                length_extra_bits,
                distance_extra_bits,
                ..
            } = token
            {
                let symbol = usize::from(distance_symbol);
                let length = removals
                    .lengths
                    .get_mut(symbol)?
                    .get_mut(usize::from(length_symbol.checked_sub(257)?))?;
                *length = length.checked_add(1)?;
                removals.extra_bits[symbol] = removals.extra_bits[symbol]
                    .checked_add(u64::from(length_extra_bits) + u64::from(distance_extra_bits))?;
                let literals = &mut removals.literals[symbol];
                for &byte in block.plain.get(position..end)? {
                    let frequency = &mut literals[usize::from(byte)];
                    *frequency = frequency.checked_add(1)?;
                }
            }
            position = end;
        }
        (position == block.plain.len()).then_some(removals)
    }

    /// Estimate the block after removing every used symbol outside `keep`.
    fn estimate(
        &self,
        block: &ParsedBlock,
        keep: u32,
        extra_bits: u64,
        strict: bool,
    ) -> Option<u64> {
        let mut literal = block.literal_frequencies;
        let mut distance = block.distance_frequencies;
        let mut extra_bits = extra_bits;
        for (symbol, count) in distance.iter_mut().enumerate() {
            if *count == 0 || keep & (1 << symbol) != 0 {
                continue;
            }
            *count = 0;
            for (frequency, &removed) in literal[257..].iter_mut().zip(&self.lengths[symbol]) {
                *frequency = frequency.checked_sub(removed)?;
            }
            for (frequency, &added) in literal[..256].iter_mut().zip(&self.literals[symbol]) {
                *frequency = frequency.checked_add(added)?;
            }
            extra_bits = extra_bits.checked_sub(self.extra_bits[symbol])?;
        }
        estimate_boundary_block_bits(&literal, &distance, extra_bits, strict)
    }
}

/// Kept-symbol masks of the three ladders, deduplicated. Keeping every used
/// symbol is the parent and keeping none is the all-literals endpoint, so
/// neither is a rung.
fn rungs(block: &ParsedBlock) -> Option<([u32; MAX_RUNGS], usize)> {
    let mut used = [0_u8; 30];
    let mut count = 0;
    for (symbol, &frequency) in block.distance_frequencies.iter().enumerate() {
        if frequency != 0 {
            used[count] = symbol as u8;
            count += 1;
        }
    }
    if count < 2 {
        return None;
    }
    let used = &mut used[..count];
    let mut masks = [0_u32; MAX_RUNGS];
    let mut total = 0;
    let mut mask = 0_u32;
    for &symbol in &used[..count - 1] {
        mask |= 1 << symbol;
        masks[total] = mask;
        total += 1;
    }
    for &symbol in used.iter() {
        masks[total] = 1 << symbol;
        total += 1;
    }
    // Stable: equal frequencies keep the shorter distance first.
    used.sort_by_key(|&symbol| std::cmp::Reverse(block.distance_frequencies[usize::from(symbol)]));
    let mut mask = 0_u32;
    for &symbol in &used[..count - 1] {
        mask |= 1 << symbol;
        masks[total] = mask;
        total += 1;
    }
    masks[..total].sort_unstable();
    let mut unique = 0;
    for index in 0..total {
        if unique == 0 || masks[index] != masks[unique - 1] {
            masks[unique] = masks[index];
            unique += 1;
        }
    }
    Some((masks, unique))
}

/// Plan the cheapest collapsed distance alphabet for one parsed block.
///
/// Returns a plan only when it is strictly smaller than the block's current
/// bits. Ties in the estimate keep the smaller mask, so the result is
/// deterministic. Rungs are priced with the ordinary tree families in both
/// modes: Max's mandatory Default sweep must reach exactly the Default
/// endpoint, and Max's later tree searches refine a collapsed block anyway.
pub(crate) fn plan_distance_bands(
    block: &ParsedBlock,
    alignment: u8,
    options: &Options,
    stop: &mut SearchStop<'_>,
) -> Option<PlannedBlock> {
    if block.source_type == SourceBlockType::Stored || stop.reached() {
        return None;
    }
    let original = block.original?;
    let (masks, count) = rungs(block)?;
    let extra_bits = token_extra_bits(&block.tokens);
    let parent_estimate = estimate_boundary_block_bits(
        &block.literal_frequencies,
        &block.distance_frequencies,
        extra_bits,
        options.strict,
    )?;
    let removals = Removals::collect(block, stop)?;

    let mut selected: [Option<(u64, u32)>; MAX_EXACT_RUNGS] = [None; MAX_EXACT_RUNGS];
    for &keep in &masks[..count] {
        let Some(estimate) = removals.estimate(block, keep, extra_bits, options.strict) else {
            continue;
        };
        if estimate > parent_estimate {
            continue;
        }
        let entry = (estimate, keep);
        if let Some(position) = selected
            .iter()
            .position(|slot| slot.map_or(true, |current| entry < current))
        {
            selected[position..].rotate_right(1);
            selected[position] = Some(entry);
        }
    }

    let plan_options = Options {
        exhaustive: false,
        ..options.clone()
    };
    let mut best: Option<PlannedBlock> = None;
    let mut best_bits = original.len;
    for (_, keep) in selected.into_iter().flatten() {
        if stop.reached() {
            break;
        }
        let Some(tokens) =
            expand_selected_matches(&block.tokens, &block.plain, |_, token, _| match token {
                Token::Match {
                    distance_symbol, ..
                } => keep & (1 << distance_symbol) == 0,
                Token::Literal(_) => false,
            })
        else {
            continue;
        };
        let Some(candidate) = try_transformed_block(block, tokens) else {
            continue;
        };
        let plan = plan_owned_block(candidate, alignment, &plan_options, stop);
        if plan.bits < best_bits {
            best_bits = plan.bits;
            best = Some(plan);
        }
    }
    best
}

#[cfg(test)]
pub(crate) mod test_support;
#[cfg(test)]
mod tests;
