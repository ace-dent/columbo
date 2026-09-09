// SPDX-License-Identifier: MIT

//! Remove small sets of payload symbols for their combined header effect.
//! Every replacement is confined to an existing match and retains its distance.

use super::block::plan_block;
use super::header::{
    estimate_boundary_block_bits, plan_bounded_depth_tree_candidate,
    plan_rle_smoothed_tree_candidate,
};
use super::model::{
    count_frequencies, token_extra_bits, ParsedBlock, PlannedBlock, Representation, Token,
};
use super::search::{solve_proven_submatch_avoiding, try_transformed_block};
use super::stop::SearchStop;
use crate::Options;

const MAX_BLOCK_TOKENS: usize = 8192;
const MAX_AFFECTED_MATCHES: usize = 64;
const MAX_REWRITTEN_BYTES: usize = 8192;
const MAX_SELECTED: usize = 8;
const MAX_CROSS_SEEDS: usize = 8;
const ESTIMATE_WINDOW: u64 = 16;

pub(crate) struct SymbolSetBudget {
    work_left: usize,
    prices_left: usize,
}

impl SymbolSetBudget {
    pub(crate) fn new() -> Self {
        Self {
            work_left: 1 << 25,
            prices_left: 128,
        }
    }

    fn spend(&mut self, work: usize) -> bool {
        if work > self.work_left {
            return false;
        }
        self.work_left -= work;
        true
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct Ban {
    lengths: u32,
    distances: u32,
}

impl Ban {
    fn affects(self, token: Token) -> bool {
        match token {
            Token::Literal(_) => false,
            Token::Match {
                length_symbol,
                distance_symbol,
                ..
            } => {
                self.lengths & (1 << (length_symbol - 257)) != 0
                    || self.distances & (1 << distance_symbol) != 0
            }
        }
    }

    fn targeted_symbols(self, block: &ParsedBlock) -> usize {
        (0..29)
            .filter(|&i| self.lengths & (1 << i) != 0 && block.literal_frequencies[257 + i] > 0)
            .count()
            + (0..30)
                .filter(|&i| self.distances & (1 << i) != 0 && block.distance_frequencies[i] > 0)
                .count()
    }
}

/// Forbid every position between two support endpoints, including unused
/// interior positions. Otherwise a submatch could reintroduce a hole inside
/// the intended zero run. Only usable distance symbols enter this menu.
fn masks(block: &ParsedBlock) -> Option<Vec<Ban>> {
    let mut singles = Vec::new();
    let mut groups = Vec::new();
    singles.try_reserve_exact(59).ok()?;
    groups.try_reserve_exact(165).ok()?;
    for distance in [false, true] {
        let mut used = [0; 30];
        let mut count = 0;
        for i in 0..if distance { 30 } else { 29 } {
            let frequency = if distance {
                block.distance_frequencies[i]
            } else {
                block.literal_frequencies[257 + i]
            };
            if frequency != 0 {
                used[count] = i;
                count += 1;
            }
        }
        for size in 1..=4 {
            for window in used[..count].windows(size) {
                let mask = ((1_u32 << (window[size - 1] + 1)) - 1) ^ ((1_u32 << window[0]) - 1);
                let ban = if distance {
                    Ban {
                        lengths: 0,
                        distances: mask,
                    }
                } else {
                    Ban {
                        lengths: mask,
                        distances: 0,
                    }
                };
                if size == 1 {
                    singles.push(ban);
                } else {
                    groups.push(ban);
                }
            }
        }
    }
    singles.try_reserve_exact(groups.len()).ok()?;
    singles.extend(groups);
    Some(singles)
}

fn rewrite(
    block: &ParsedBlock,
    ban: Ban,
    literal_lengths: &[u8],
    distance_lengths: &[u8],
    budget: &mut SymbolSetBudget,
    stop: &mut SearchStop<'_>,
) -> Option<Vec<Token>> {
    if stop.reached() || !budget.spend(block.tokens.len()) {
        return None;
    }
    let mut affected = 0;
    let mut bytes = 0;
    let mut edges = 0;
    for &token in block.tokens.iter() {
        if !ban.affects(token) {
            continue;
        }
        affected += 1;
        let length = token.decoded_len();
        bytes += length;
        // These totals only grow. Once either limit is exceeded, later
        // tokens cannot make this candidate eligible for rewriting.
        if affected > MAX_AFFECTED_MATCHES || bytes > MAX_REWRITTEN_BYTES {
            return None;
        }
        if let Token::Match {
            distance_symbol, ..
        } = token
        {
            if ban.distances & (1 << distance_symbol) == 0 {
                // The restricted submatch solver visits this many match edges,
                // including forbidden lengths. Reserve the whole search first.
                edges += (length - 1) * (length - 2) / 2;
            }
        }
    }
    if affected == 0 || !budget.spend(edges + bytes + block.tokens.len()) {
        return None;
    }
    let mut result = Vec::new();
    result.try_reserve_exact(block.tokens.len() + bytes).ok()?;
    let mut position = 0;
    for (i, &token) in block.tokens.iter().enumerate() {
        if i & 255 == 0 && stop.reached() {
            return None;
        }
        let end = position + token.decoded_len();
        let decoded = block.plain.get(position..end)?;
        match token {
            Token::Match {
                distance_symbol, ..
            } if ban.distances & (1 << distance_symbol) != 0 => {
                result.extend(decoded.iter().copied().map(Token::Literal))
            }
            Token::Match { length_symbol, .. }
                if ban.lengths & (1 << (length_symbol - 257)) != 0 =>
            {
                result.extend(solve_proven_submatch_avoiding(
                    token,
                    decoded,
                    literal_lengths,
                    distance_lengths,
                    ban.lengths,
                    stop,
                )?);
            }
            _ => result.push(token),
        }
        position = end;
    }
    (position == block.plain.len()).then_some(result)
}

struct Proposal {
    ban: Ban,
    score: u64,
    tokens: Vec<Token>,
}

impl Proposal {
    fn key(&self) -> (u64, Ban) {
        (self.score, self.ban)
    }
}

fn retain_proposal(proposals: &mut Vec<Proposal>, proposal: Proposal) {
    let position = proposals
        .iter()
        .position(|p| proposal.key() < p.key())
        .unwrap_or(proposals.len());
    if position >= MAX_SELECTED {
        return;
    }
    if proposals.len() == MAX_SELECTED {
        proposals.pop();
    }
    proposals.insert(position, proposal);
}

fn retain_seed(seeds: &mut Vec<(u64, Ban)>, seed: (u64, Ban)) {
    let position = seeds.iter().position(|&p| seed < p).unwrap_or(seeds.len());
    if position >= MAX_CROSS_SEEDS {
        return;
    }
    if seeds.len() == MAX_CROSS_SEEDS {
        seeds.pop();
    }
    seeds.insert(position, seed);
}

fn estimate(tokens: &[Token]) -> Option<u64> {
    let (literal, distance) = count_frequencies(tokens);
    estimate_boundary_block_bits(&literal, &distance, token_extra_bits(tokens), true)
}

/// Price one token spelling with the ordinary family grid and two small
/// existing table families. Full Max's much larger tree cross-product is not
/// needed for the measured set-removal witnesses. Each complete intermediate
/// remains available if a later family reaches the stop boundary.
fn price_spelling(
    parent: &ParsedBlock,
    tokens: Vec<Token>,
    alignment: u8,
    options: &Options,
    budget: &mut SymbolSetBudget,
    stop: &mut SearchStop<'_>,
) -> Option<PlannedBlock> {
    if budget.prices_left == 0 || stop.reached() {
        return None;
    }
    budget.prices_left -= 1;
    let block = try_transformed_block(parent, tokens)?;
    let search_options = Options {
        exhaustive: false,
        strict: true,
        ..options.clone()
    };
    let mut best = plan_block(&block, alignment, &search_options, stop);
    if !stop.reached() {
        if let Some(plan) = plan_bounded_depth_tree_candidate(
            &block.tokens,
            &block.literal_frequencies,
            &block.distance_frequencies,
            true,
            true,
            false,
            stop,
        ) {
            if plan.bits < best.bits {
                best.bits = plan.bits;
                best.representation = Representation::Dynamic(plan);
            }
        }
    }
    if !stop.reached() {
        if let Some(plan) = plan_rle_smoothed_tree_candidate(
            &block.tokens,
            &block.literal_frequencies,
            &block.distance_frequencies,
            true,
        ) {
            if plan.bits < best.bits {
                best.bits = plan.bits;
                best.representation = Representation::Dynamic(plan);
            }
        }
    }
    Some(best)
}

pub(crate) fn plan_symbol_sets(
    block: &ParsedBlock,
    alignment: u8,
    options: &Options,
    budget: &mut SymbolSetBudget,
    stop: &mut SearchStop<'_>,
) -> Option<PlannedBlock> {
    let parent = block.original_dynamic.as_ref()?;
    if !parent.has_strictly_compatible_huffman_codes()
        || block.tokens.len() > MAX_BLOCK_TOKENS
        || block.plain.len() > 128 * 1024
        || budget.prices_left == 0
        || stop.reached()
    {
        return None;
    }
    let parent_score = estimate_boundary_block_bits(
        &block.literal_frequencies,
        &block.distance_frequencies,
        token_extra_bits(&block.tokens),
        true,
    )?;
    let mut proposals = Vec::new();
    proposals.try_reserve_exact(MAX_SELECTED).ok()?;
    let mut seeds = [Vec::new(), Vec::new()];
    for list in &mut seeds {
        list.try_reserve_exact(MAX_CROSS_SEEDS).ok()?;
    }
    for ban in masks(block)? {
        if stop.reached() {
            break;
        }
        let Some(tokens) = rewrite(
            block,
            ban,
            &parent.literal_lengths,
            &parent.distance_lengths,
            budget,
            stop,
        ) else {
            continue;
        };
        let Some(score) = estimate(&tokens) else {
            continue;
        };
        let count = ban.targeted_symbols(block);
        if count <= 2 {
            retain_seed(&mut seeds[usize::from(ban.distances != 0)], (score, ban));
        }
        // This is candidate ranking, not an impossibility bound. Normalize to
        // the same cheap model of the parent and keep a small losing window.
        if count >= 2 && score <= parent_score + ESTIMATE_WINDOW {
            retain_proposal(&mut proposals, Proposal { ban, score, tokens });
        }
    }
    for &(_, literal) in &seeds[0] {
        for &(_, distance) in &seeds[1] {
            if stop.reached() {
                break;
            }
            let ban = Ban {
                lengths: literal.lengths,
                distances: distance.distances,
            };
            let Some(tokens) = rewrite(
                block,
                ban,
                &parent.literal_lengths,
                &parent.distance_lengths,
                budget,
                stop,
            ) else {
                continue;
            };
            let Some(score) = estimate(&tokens) else {
                continue;
            };
            if score <= parent_score + ESTIMATE_WINDOW {
                retain_proposal(&mut proposals, Proposal { ban, score, tokens });
            }
        }
    }
    let mut best = None;
    let mut best_bits = block.original?.len;
    for proposal in proposals {
        let Some(mut candidate) =
            price_spelling(block, proposal.tokens, alignment, options, budget, stop)
        else {
            break;
        };
        let lengths = match &candidate.representation {
            Representation::Dynamic(plan) => Some((
                plan.literal_lengths.as_slice(),
                plan.distance_lengths.as_slice(),
            )),
            Representation::Fixed => Some((
                super::huffman::FIXED_LITERAL_CODE_LENGTHS.as_slice(),
                super::huffman::FIXED_DISTANCE_CODE_LENGTHS.as_slice(),
            )),
            _ => None,
        };
        if let Some((literal_lengths, distance_lengths)) = lengths {
            // Revisit the original certificates under the new table costs,
            // keeping the entire forbidden set. Do not extend a submatch or
            // turn a newly emitted literal into a new match certificate.
            if let Some(tokens) = rewrite(
                block,
                proposal.ban,
                literal_lengths,
                distance_lengths,
                budget,
                stop,
            ) {
                if tokens.as_slice() != candidate.tokens.as_slice() {
                    if let Some(feedback) =
                        price_spelling(block, tokens, alignment, options, budget, stop)
                    {
                        if feedback.bits < candidate.bits {
                            candidate = feedback;
                        }
                    }
                }
            }
        }
        if candidate.bits < best_bits {
            best_bits = candidate.bits;
            best = Some(candidate);
        }
    }
    best
}

#[cfg(test)]
pub(crate) fn symbol_set_test_block() -> ParsedBlock {
    // Completed R4 g10n3p04 parent: grouped removal saves four further bits.
    let raw = [
        0xbd, 0xce, 0xb1, 0x09, 0x80, 0x00, 0x0c, 0x44, 0xd1, 0x54, 0xb1, 0x76, 0x05, 0x27, 0x10,
        0xb2, 0x82, 0x2b, 0xd8, 0xa7, 0x4a, 0x6d, 0x97, 0x15, 0x5c, 0xc1, 0x6d, 0x3d, 0x51, 0xc2,
        0x61, 0xa9, 0x90, 0x5f, 0x1e, 0x2f, 0x10, 0xd9, 0xd1, 0x8a, 0x02, 0x4d, 0x48, 0x78, 0x90,
        0xab, 0x1e, 0x31, 0xa2, 0x03, 0x95, 0xe0, 0xa1, 0x51, 0x2c, 0xc8, 0x50, 0x09, 0x1e, 0xfe,
        0x89, 0xd9, 0xc2, 0x95, 0x85, 0x58, 0xa8, 0xbf, 0x84, 0xca, 0x2d, 0xae, 0x32, 0xb7, 0x0c,
        0x79, 0x44, 0x0d, 0x38, 0x21, 0x21, 0x83, 0xba, 0x06, 0x89, 0xcc, 0xfa, 0x83, 0x87, 0x8f,
        0xe2, 0x04,
    ];
    super::parse::parse_stream(&raw, 2048)
        .unwrap()
        .blocks
        .remove(0)
}

#[cfg(test)]
pub(crate) fn assert_proven_rewrite(source: &ParsedBlock, tokens: &[Token]) {
    if source.source_type == super::model::SourceBlockType::Stored {
        assert_eq!(tokens, source.tokens.as_slice());
        return;
    }
    if source.tokens.is_empty() {
        assert!(tokens.is_empty());
        assert!(source.plain.is_empty());
        return;
    }
    let mut source_index = 0;
    let mut source_end = source.tokens[0].decoded_len();
    let mut position = 0;
    for &token in tokens {
        while position >= source_end {
            source_index += 1;
            source_end += source.tokens[source_index].decoded_len();
        }
        let end = position + token.decoded_len();
        match token {
            Token::Literal(value) => assert_eq!(value, source.plain[position]),
            Token::Match {
                distance,
                length,
                length_symbol,
                length_extra,
                length_extra_bits,
                ..
            } => {
                assert!(end <= source_end);
                assert!(
                    matches!(source.tokens[source_index], Token::Match { distance: original, .. } if original == distance)
                );
                assert_eq!(
                    super::model::canonical_length_encoding(length),
                    Some((length_symbol, length_extra, length_extra_bits))
                );
            }
        }
        position = end;
    }
    assert_eq!(position, source.plain.len());
}

#[cfg(test)]
mod tests;
