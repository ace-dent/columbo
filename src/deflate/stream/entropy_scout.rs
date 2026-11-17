// SPDX-License-Identifier: MIT
// SPDX-FileCopyrightText: 2026 Andrew C.E. Dent <https://github.com/ace-dent>

//! Suggest boundary-graph cuts from an entropy-state path.
//!
//! In Max, an independent Deflate adaptation of Brotli's block splitter fits a
//! few paired literal/length and distance codes to the tokens, refines them,
//! then labels each token by a shortest path that charges every code switch.
//! The highest-scoring label changes become cut anchors for the global boundary
//! graph and R6's dense graph. The scout emits no block; exact pricing
//! decides.

use crate::deflate::huffman::make_lengths_deflopt_heap_into;
use crate::deflate::model::{distance_extra_bits, length_extra_bits, SourceBlockType, Token};
use crate::deflate::stop::SearchStop;
use crate::sort::SortShared;

use super::adaptive_split::ADAPTIVE_SPLIT_MIN_TOKENS;
use super::{Composite, Cut, MAX_MERGED_TOKENS, RANGE_HISTOGRAM_INTERVAL};

/// Limits of the entropy-state boundary scout ([`entropy_state_boundary_cuts`]).
const ENTROPY_SCOUT_MIN_TOKENS: usize = ADAPTIVE_SPLIT_MIN_TOKENS;
const ENTROPY_SCOUT_MAX_TOKENS: usize = MAX_MERGED_TOKENS;
const ENTROPY_SCOUT_TOKENS_PER_PROTOTYPE: usize = 512;
const ENTROPY_SCOUT_MAX_PROTOTYPES: usize = 8;
const ENTROPY_SCOUT_SEED_TOKENS: usize = 256;
const ENTROPY_SCOUT_REFINEMENT_ROUNDS: usize = 2;
const ENTROPY_SCOUT_SWITCH_BITS: u64 = 128;
const ENTROPY_SCOUT_SCORE_RADIUS: usize = 64;
const ENTROPY_SCOUT_MIN_EDGE_TOKENS: usize = 16;
const ENTROPY_SCOUT_MAX_CUTS: usize = 16;

#[derive(Clone)]
struct EntropyScoutHistogram {
    literal: [u32; 286],
    distance: [u32; 30],
    token_count: usize,
}

impl EntropyScoutHistogram {
    fn zero() -> Self {
        Self {
            literal: [0; 286],
            distance: [0; 30],
            token_count: 0,
        }
    }

    fn add_token(&mut self, token: Token) {
        match token {
            Token::Literal(value) => self.literal[usize::from(value)] += 1,
            Token::Match {
                length_symbol,
                distance_symbol,
                ..
            } => {
                self.literal[usize::from(length_symbol)] += 1;
                self.distance[usize::from(distance_symbol)] += 1;
            }
        }
        self.token_count += 1;
    }
}

struct EntropyScoutCode {
    literal_lengths: [u8; 286],
    distance_lengths: [u8; 30],
    unseen_literal_bits: u8,
    unseen_distance_bits: u8,
}

impl EntropyScoutCode {
    fn from_histogram(histogram: &EntropyScoutHistogram, strict: bool) -> Self {
        let mut literal_frequencies = histogram.literal;
        literal_frequencies[256] += 1;
        complete_scout_alphabet(&mut literal_frequencies, strict);
        let mut distance_frequencies = histogram.distance;
        if distance_frequencies.iter().all(|&frequency| frequency == 0) {
            distance_frequencies[0] = 1;
        }
        complete_scout_alphabet(&mut distance_frequencies, strict);

        let mut literal_lengths = [0; 286];
        let mut distance_lengths = [0; 30];
        make_lengths_deflopt_heap_into(&literal_frequencies, &mut literal_lengths, 15, 0);
        make_lengths_deflopt_heap_into(&distance_frequencies, &mut distance_lengths, 15, 0);

        Self {
            literal_lengths,
            distance_lengths,
            unseen_literal_bits: unseen_symbol_bits(&literal_frequencies),
            unseen_distance_bits: unseen_symbol_bits(&distance_frequencies),
        }
    }

    fn token_bits(&self, token: Token) -> u64 {
        match token {
            Token::Literal(value) => u64::from(code_or_unseen(
                &self.literal_lengths,
                usize::from(value),
                self.unseen_literal_bits,
            )),
            Token::Match {
                length_symbol,
                distance_symbol,
                ..
            } => {
                u64::from(code_or_unseen(
                    &self.literal_lengths,
                    usize::from(length_symbol),
                    self.unseen_literal_bits,
                )) + u64::from(code_or_unseen(
                    &self.distance_lengths,
                    usize::from(distance_symbol),
                    self.unseen_distance_bits,
                )) + u64::from(length_extra_bits(length_symbol))
                    + u64::from(distance_extra_bits(distance_symbol))
            }
        }
    }
}

fn complete_scout_alphabet<const N: usize>(frequencies: &mut [u32; N], strict: bool) {
    if !strict
        || frequencies
            .iter()
            .filter(|&&frequency| frequency != 0)
            .count()
            >= 2
    {
        return;
    }
    if let Some(frequency) = frequencies.iter_mut().find(|frequency| **frequency == 0) {
        *frequency = 1;
    }
}

fn unseen_symbol_bits<const N: usize>(frequencies: &[u32; N]) -> u8 {
    let total = frequencies
        .iter()
        .fold(0_u64, |sum, &frequency| sum + u64::from(frequency));
    let rounded_log = if total <= 1 {
        0
    } else {
        u64::BITS - (total - 1).leading_zeros()
    };
    (rounded_log + 2).min(u32::from(u8::MAX)) as u8
}

fn code_or_unseen(lengths: &[u8], symbol: usize, unseen: u8) -> u8 {
    lengths
        .get(symbol)
        .copied()
        .filter(|&length| length != 0)
        .unwrap_or(unseen)
}

/// Suggest token-boundary cuts for the boundary DP from an entropy-state path:
/// an independent Deflate adaptation of Brotli's block splitter that refines a
/// few paired literal/length and distance codes, then labels tokens by a
/// shortest path charging each code switch.
pub(super) fn entropy_state_boundary_cuts(
    composite: &Composite<'_>,
    strict: bool,
    stop: &mut SearchStop<'_>,
) -> Option<Vec<Cut>> {
    let token_count = composite.tokens.len();
    if !(ENTROPY_SCOUT_MIN_TOKENS..=ENTROPY_SCOUT_MAX_TOKENS).contains(&token_count)
        || composite.plain.len() < 128
        || composite
            .sources
            .iter()
            .any(|source| source.source_type == SourceBlockType::Stored)
        || stop.reached()
    {
        return None;
    }

    let prototype_count =
        (token_count / ENTROPY_SCOUT_TOKENS_PER_PROTOTYPE).clamp(2, ENTROPY_SCOUT_MAX_PROTOTYPES);
    let mut histograms = seed_entropy_scout_histograms(&composite.tokens, prototype_count)?;
    for _ in 0..ENTROPY_SCOUT_REFINEMENT_ROUNDS {
        let codes: Vec<_> = histograms
            .iter()
            .map(|histogram| EntropyScoutCode::from_histogram(histogram, strict))
            .collect();
        let assigned =
            assign_entropy_states(&composite.tokens, &codes, ENTROPY_SCOUT_SWITCH_BITS, stop)?;
        let refined = rebuild_entropy_scout_histograms(&composite.tokens, &assigned, &histograms)?;
        histograms = refined;
    }

    let codes: Vec<_> = histograms
        .iter()
        .map(|histogram| EntropyScoutCode::from_histogram(histogram, strict))
        .collect();
    let final_states =
        assign_entropy_states(&composite.tokens, &codes, ENTROPY_SCOUT_SWITCH_BITS, stop)?;
    ranked_entropy_state_cuts(composite, &codes, &final_states)
}

fn seed_entropy_scout_histograms(
    tokens: &[Token],
    prototype_count: usize,
) -> Option<Vec<EntropyScoutHistogram>> {
    if !(2..=ENTROPY_SCOUT_MAX_PROTOTYPES).contains(&prototype_count) {
        return None;
    }
    let seed_tokens = ENTROPY_SCOUT_SEED_TOKENS.min(tokens.len() / prototype_count);
    if seed_tokens == 0 {
        return None;
    }
    let mut histograms = Vec::new();
    histograms.try_reserve_exact(prototype_count).ok()?;
    for prototype in 0..prototype_count {
        let center = tokens
            .len()
            .checked_mul(prototype.checked_mul(2)?.checked_add(1)?)?
            / prototype_count.checked_mul(2)?;
        let start = center
            .saturating_sub(seed_tokens / 2)
            .min(tokens.len().saturating_sub(seed_tokens));
        let mut histogram = EntropyScoutHistogram::zero();
        for &token in &tokens[start..start + seed_tokens] {
            histogram.add_token(token);
        }
        histograms.push(histogram);
    }
    Some(histograms)
}

fn rebuild_entropy_scout_histograms(
    tokens: &[Token],
    states: &[u8],
    previous: &[EntropyScoutHistogram],
) -> Option<Vec<EntropyScoutHistogram>> {
    if tokens.len() != states.len() || previous.is_empty() {
        return None;
    }
    let mut histograms = Vec::new();
    histograms.try_reserve_exact(previous.len()).ok()?;
    histograms.resize_with(previous.len(), EntropyScoutHistogram::zero);
    for (&token, &state) in tokens.iter().zip(states) {
        histograms.get_mut(usize::from(state))?.add_token(token);
    }
    for (histogram, old) in histograms.iter_mut().zip(previous) {
        if histogram.token_count == 0 {
            *histogram = old.clone();
        }
    }
    Some(histograms)
}

fn assign_entropy_states(
    tokens: &[Token],
    codes: &[EntropyScoutCode],
    switch_bits: u64,
    stop: &mut SearchStop<'_>,
) -> Option<Vec<u8>> {
    let state_count = codes.len();
    if tokens.is_empty() || !(2..=ENTROPY_SCOUT_MAX_PROTOTYPES).contains(&state_count) {
        return None;
    }
    let mut back = Vec::<[u8; ENTROPY_SCOUT_MAX_PROTOTYPES]>::new();
    back.try_reserve_exact(tokens.len()).ok()?;
    let mut previous = [u64::MAX; ENTROPY_SCOUT_MAX_PROTOTYPES];
    for state in 0..state_count {
        previous[state] = codes[state].token_bits(tokens[0]);
    }
    back.push(std::array::from_fn(|state| state as u8));

    for (position, &token) in tokens.iter().enumerate().skip(1) {
        if position % RANGE_HISTOGRAM_INTERVAL == 0 && stop.reached() {
            return None;
        }
        let mut ranked = [(u64::MAX, usize::MAX); 2];
        for (state, &cost) in previous[..state_count].iter().enumerate() {
            let candidate = (cost, state);
            if candidate < ranked[0] {
                ranked[1] = ranked[0];
                ranked[0] = candidate;
            } else if candidate < ranked[1] {
                ranked[1] = candidate;
            }
        }

        let mut current = [u64::MAX; ENTROPY_SCOUT_MAX_PROTOTYPES];
        let mut predecessors = [0_u8; ENTROPY_SCOUT_MAX_PROTOTYPES];
        for state in 0..state_count {
            let switch_from = if ranked[0].1 == state {
                ranked[1]
            } else {
                ranked[0]
            };
            let switch_cost = switch_from.0.checked_add(switch_bits)?;
            let (base, predecessor) = if switch_cost < previous[state] {
                (switch_cost, switch_from.1)
            } else {
                (previous[state], state)
            };
            current[state] = base.checked_add(codes[state].token_bits(token))?;
            predecessors[state] = predecessor.try_into().ok()?;
        }
        let floor = *current[..state_count].iter().min()?;
        for cost in &mut current[..state_count] {
            *cost -= floor;
        }
        previous = current;
        back.push(predecessors);
    }
    if stop.reached() {
        return None;
    }

    let mut final_state = (0..state_count).min_by_key(|&state| (previous[state], state))?;
    let mut states = Vec::new();
    states.try_reserve_exact(tokens.len()).ok()?;
    states.resize(tokens.len(), 0_u8);
    *states.last_mut()? = final_state.try_into().ok()?;
    for position in (1..tokens.len()).rev() {
        final_state = usize::from(back[position][final_state]);
        states[position - 1] = final_state.try_into().ok()?;
    }
    Some(states)
}

fn ranked_entropy_state_cuts(
    composite: &Composite<'_>,
    codes: &[EntropyScoutCode],
    states: &[u8],
) -> Option<Vec<Cut>> {
    if states.len() != composite.tokens.len() {
        return None;
    }
    let mut transitions = Vec::<(u64, usize)>::new();
    transitions
        .try_reserve_exact(states.len().saturating_sub(1))
        .ok()?;
    for boundary in 1..states.len() {
        let left_state = usize::from(states[boundary - 1]);
        let right_state = usize::from(states[boundary]);
        if left_state == right_state
            || boundary < ENTROPY_SCOUT_MIN_EDGE_TOKENS
            || states.len() - boundary < ENTROPY_SCOUT_MIN_EDGE_TOKENS
        {
            continue;
        }
        let left_code = codes.get(left_state)?;
        let right_code = codes.get(right_state)?;
        let left_start = boundary.saturating_sub(ENTROPY_SCOUT_SCORE_RADIUS);
        let right_end = boundary
            .checked_add(ENTROPY_SCOUT_SCORE_RADIUS)?
            .min(states.len());
        let mut score = 0_u64;
        for &token in &composite.tokens[left_start..boundary] {
            score = score.checked_add(
                right_code
                    .token_bits(token)
                    .saturating_sub(left_code.token_bits(token)),
            )?;
        }
        for &token in &composite.tokens[boundary..right_end] {
            score = score.checked_add(
                left_code
                    .token_bits(token)
                    .saturating_sub(right_code.token_bits(token)),
            )?;
        }
        if score != 0 {
            transitions.push((score, boundary));
        }
    }
    // Each boundary scores once, so it breaks every tie.
    transitions
        .sort_shared_by(|left, right| right.0.cmp(&left.0).then_with(|| left.1.cmp(&right.1)));
    transitions.truncate(ENTROPY_SCOUT_MAX_CUTS);
    transitions.sort_shared_by_key(|&(_, boundary)| boundary);

    let mut cuts = Vec::new();
    cuts.try_reserve_exact(transitions.len()).ok()?;
    for (_, token) in transitions {
        let plain = *composite.token_plain_offsets.get(token)?;
        cuts.push(Cut { token, plain });
    }
    Some(cuts)
}

#[cfg(test)]
mod tests;
