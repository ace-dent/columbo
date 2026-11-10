// SPDX-License-Identifier: MIT
// SPDX-FileCopyrightText: 2026 Andrew C.E. Dent <https://github.com/ace-dent>

//! Propose split cuts from range histograms (the adaptive split probe).
//!
//! The probe follows the shape of Turtledeflate's coarse-to-fine block split.
//! It prices each side of a candidate cut from the composite's cumulative
//! histogram checkpoints, as the cheaper of its stored size and a Huffman
//! estimate, samples evenly spaced cuts and narrows around the best local
//! minimum. The best cut, and a well-separated runner-up, go to exact pricing
//! by the source split, the comparison-floor reseat, the forced-split escape
//! and R6.

use crate::deflate::block::stored_block_bits;
use crate::deflate::header::estimate_boundary_block_bits;
use crate::deflate::stop::SearchStop;
use crate::sort::SortShared;

use super::{add_cut, Composite, Cut, FrequencyCheckpoint, MAX_MERGED_TOKENS};

/// Limits of the adaptive split search ([`add_adaptive_split_cut`]).
pub(super) const ADAPTIVE_SPLIT_MIN_TOKENS: usize = 513;
const ADAPTIVE_SPLIT_INTERVALS: usize = 7;
const ADAPTIVE_SPLIT_CENTER_RADIUS: usize = 1;
const ADAPTIVE_SPLIT_FINAL_WIDTH: usize = 16;
const ADAPTIVE_SPLIT_MAX_PROBES: usize = 128;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct AdaptiveSplit {
    pub(super) token: usize,
    bits: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct AdaptiveSplitSearch {
    pub(super) best: AdaptiveSplit,
    pub(super) secondary: Option<AdaptiveSplit>,
}

/// Add one Max-only cut from an independent, bounded implementation of
/// Turtledeflate's coarse-to-fine `turtledeflate_best_block_split` (see
/// `docs/research/turtledeflate-methods.md`).
///
/// It samples evenly spaced split costs, smooths neighboring samples, narrows
/// around the best local minimum and scans the final interval, within probe and
/// deadline caps.
pub(super) fn add_adaptive_split_cut(
    cuts: &mut Vec<Cut>,
    composite: &Composite,
    start: Cut,
    end: Cut,
    min_distance_codes: bool,
    stop: &mut SearchStop<'_>,
) -> Option<()> {
    let Some((unsplit_bits, search)) =
        adaptive_histogram_split_search(composite, start, end, min_distance_codes, stop)
    else {
        return Some(());
    };
    if search.best.bits < unsplit_bits {
        add_cut(cuts, composite, search.best.token)?;
    }
    Some(())
}

/// Return a well-separated runner-up local minimum from the adaptive probes,
/// for [`plan_forced_split_boundary_escape`](super::plan_forced_split_boundary_escape).
pub(super) fn secondary_adaptive_split_cut(
    composite: &Composite,
    start: Cut,
    end: Cut,
    min_distance_codes: bool,
    stop: &mut SearchStop<'_>,
) -> Option<Cut> {
    let (_, search) =
        adaptive_histogram_split_search(composite, start, end, min_distance_codes, stop)?;
    let candidate = search.secondary?;
    let plain = *composite.token_plain_offsets.get(candidate.token)?;
    let cut = Cut {
        token: candidate.token,
        plain,
    };
    (composite.cut_is_token_boundary(cut) && start.plain < plain && plain < end.plain)
        .then_some(cut)
}

pub(super) fn adaptive_histogram_split_search(
    composite: &Composite,
    start: Cut,
    end: Cut,
    min_distance_codes: bool,
    stop: &mut SearchStop<'_>,
) -> Option<(u64, AdaptiveSplitSearch)> {
    if !composite.cut_is_token_boundary(start) || !composite.cut_is_token_boundary(end) {
        return None;
    }
    let token_count = end.token.checked_sub(start.token)?;
    let plain_count = end.plain.checked_sub(start.plain)?;
    if !(ADAPTIVE_SPLIT_MIN_TOKENS..=MAX_MERGED_TOKENS).contains(&token_count)
        || plain_count < 128
        || composite.frequency_checkpoints.is_none()
    {
        return None;
    }

    let whole = composite.range_frequencies(start.token, end.token)?;
    let unsplit_bits = estimate_histogram_range_bits(&whole, plain_count, min_distance_codes);
    let mut score_split = |token: usize| {
        let split_plain = *composite.token_plain_offsets.get(token)?;
        if split_plain <= start.plain || split_plain >= end.plain {
            return None;
        }

        let left = composite.range_frequencies(start.token, token)?;
        let mut right = whole.checked_sub(&left)?;
        // Both range histograms count one end-of-block symbol, so their
        // difference has none; the right child needs one.
        right.literal[256] = 1;
        let left_bits = estimate_histogram_range_bits(
            &left,
            split_plain.checked_sub(start.plain)?,
            min_distance_codes,
        );
        let right_bits = estimate_histogram_range_bits(
            &right,
            end.plain.checked_sub(split_plain)?,
            min_distance_codes,
        );
        left_bits.checked_add(right_bits)
    };

    let search = coarse_to_fine_split_search(start.token, end.token, &mut score_split, stop)?;
    Some((unsplit_bits, search))
}

pub(super) fn estimate_histogram_range_bits(
    frequencies: &FrequencyCheckpoint,
    plain_len: usize,
    min_distance_codes: bool,
) -> u64 {
    let stored = stored_block_bits(0, plain_len);
    estimate_boundary_block_bits(
        &frequencies.literal,
        &frequencies.distance,
        frequencies.extra_bits,
        min_distance_codes,
    )
    .map_or(stored, |huffman| huffman.min(stored))
}

fn coarse_to_fine_split_search<S>(
    start: usize,
    end: usize,
    score: &mut S,
    stop: &mut SearchStop<'_>,
) -> Option<AdaptiveSplitSearch>
where
    S: FnMut(usize) -> Option<u64>,
{
    if end <= start.checked_add(2)? {
        return None;
    }
    let original_midpoint = start + (end - start) / 2;
    let mut range_start = start + 1;
    let mut range_end = end - 1;
    let mut probes = 0_usize;
    let mut cache = Vec::<AdaptiveSplit>::new();
    cache.try_reserve_exact(ADAPTIVE_SPLIT_MAX_PROBES).ok()?;
    // Always probe the midpoint, so a flat score picks balanced children.
    cached_adaptive_split_score(original_midpoint, &mut probes, &mut cache, score, stop)?;

    while range_end - range_start > ADAPTIVE_SPLIT_FINAL_WIDTH {
        if stop.reached() {
            return None;
        }
        let span = range_end - range_start;
        let mut samples = Vec::new();
        samples
            .try_reserve_exact(ADAPTIVE_SPLIT_INTERVALS + 1)
            .ok()?;
        for interval in 0..=ADAPTIVE_SPLIT_INTERVALS {
            let token = range_start + span.checked_mul(interval)? / ADAPTIVE_SPLIT_INTERVALS;
            if samples
                .last()
                .is_some_and(|sample: &AdaptiveSplit| sample.token == token)
            {
                continue;
            }
            let bits = cached_adaptive_split_score(token, &mut probes, &mut cache, score, stop)?;
            samples.push(AdaptiveSplit { token, bits });
        }
        if samples.len() < 2 {
            return None;
        }

        let interval_midpoint = range_start + span / 2;
        let mut selected = 0_usize;
        let mut selected_key = (u128::MAX, usize::MAX, usize::MAX);
        for (index, sample) in samples.iter().enumerate() {
            let left = samples[index.saturating_sub(1)].bits;
            let right = samples[(index + 1).min(samples.len() - 1)].bits;
            let filtered = u128::from(left)
                .checked_add(u128::from(sample.bits))?
                .checked_add(u128::from(right))?;
            let key = (
                filtered,
                sample.token.abs_diff(interval_midpoint),
                sample.token,
            );
            if key < selected_key {
                selected = index;
                selected_key = key;
            }
        }

        let left_index = selected.saturating_sub(ADAPTIVE_SPLIT_CENTER_RADIUS);
        let right_index = (selected + ADAPTIVE_SPLIT_CENTER_RADIUS).min(samples.len() - 1);
        let next_start = samples[left_index].token;
        let next_end = samples[right_index].token;
        if next_start == range_start && next_end == range_end {
            break;
        }
        range_start = next_start;
        range_end = next_end;
    }

    if stop.reached() {
        return None;
    }
    for token in range_start..=range_end {
        cached_adaptive_split_score(token, &mut probes, &mut cache, score, stop)?;
    }

    let candidate_key = |candidate: &AdaptiveSplit| {
        (
            candidate.bits,
            candidate.token.abs_diff(original_midpoint),
            candidate.token,
        )
    };
    let best = *cache
        .iter()
        .min_by_key(|candidate| candidate_key(candidate))?;
    // The cache holds one candidate per token.
    cache.sort_shared_by_key(|candidate| candidate.token);
    let separation = ((end - start) / ADAPTIVE_SPLIT_INTERVALS).max(ADAPTIVE_SPLIT_FINAL_WIDTH);
    let secondary = cache
        .iter()
        .enumerate()
        .filter(|(_, candidate)| candidate.token.abs_diff(best.token) >= separation)
        .filter(|(index, candidate)| {
            let left_is_no_better = index
                .checked_sub(1)
                .and_then(|left| cache.get(left))
                .map_or(true, |left| left.bits >= candidate.bits);
            let right_is_no_better = cache
                .get(index + 1)
                .map_or(true, |right| right.bits >= candidate.bits);
            left_is_no_better && right_is_no_better
        })
        .min_by_key(|(_, candidate)| candidate_key(candidate))
        .map(|(_, candidate)| *candidate);
    Some(AdaptiveSplitSearch { best, secondary })
}

fn cached_adaptive_split_score<S>(
    token: usize,
    probes: &mut usize,
    cache: &mut Vec<AdaptiveSplit>,
    score: &mut S,
    stop: &mut SearchStop<'_>,
) -> Option<u64>
where
    S: FnMut(usize) -> Option<u64>,
{
    if let Some(candidate) = cache.iter().find(|candidate| candidate.token == token) {
        return Some(candidate.bits);
    }
    if *probes >= ADAPTIVE_SPLIT_MAX_PROBES || stop.reached() {
        return None;
    }
    let bits = score(token)?;
    *probes += 1;
    cache.push(AdaptiveSplit { token, bits });
    Some(bits)
}

#[cfg(test)]
mod tests;
