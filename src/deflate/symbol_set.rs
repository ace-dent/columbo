// SPDX-License-Identifier: MIT
// SPDX-FileCopyrightText: 2026 Andrew C.E. Dent <https://github.com/ace-dent>

//! Remove small sets of length or distance symbols from a dynamic block to
//! shrink its header (R5).
//!
//! Each change stays within one existing match: a match whose distance symbol
//! is removed becomes literals, and a match whose length symbol is removed is
//! re-spelled as literals and proven submatches at the same distance.

use super::block::plan_block;
use super::header::{
    estimate_boundary_block_bits, plan_bounded_depth_tree_candidate,
    plan_rle_smoothed_tree_candidate,
};
use super::model::{
    count_frequencies, distance_extra_bits, length_extra_bits, same_tokens, token_extra_bits,
    try_clone_slice, ParsedBlock, PlannedBlock, Representation, Token,
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

/// List candidate bans, single symbols first. Each covers 1-4 consecutive
/// used length symbols or distance symbols and every unused symbol between
/// them, so a submatch cannot reintroduce a nonzero length inside the
/// intended zero run.
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

/// Restricted spellings of a block's matches under the parent's code lengths,
/// by token index and the banned length symbols up to the match's own. The
/// submatch solver never uses a longer symbol, so bans that agree on these
/// spell the match alike: the bans holding a symbol first, and the cross bans,
/// repeat their single and group bans' spellings.
struct SubmatchSpellings {
    by_token: Vec<Vec<(u32, Option<Vec<Token>>)>>,
}

impl SubmatchSpellings {
    fn new(block: &ParsedBlock) -> Option<Self> {
        let mut by_token = Vec::new();
        by_token.try_reserve_exact(block.tokens.len()).ok()?;
        by_token.resize_with(block.tokens.len(), Vec::new);
        Some(Self { by_token })
    }

    /// The spelling of the match at `index` with the `relevant` symbols
    /// banned, from `solve` the first time.
    fn spell(
        &mut self,
        index: usize,
        relevant: u32,
        stop: &mut SearchStop<'_>,
        solve: impl FnOnce(&mut SearchStop<'_>) -> Option<Vec<Token>>,
    ) -> Option<Vec<Token>> {
        let known = self.by_token.get_mut(index)?;
        if let Some((_, spelled)) = known.iter().find(|(mask, _)| *mask == relevant) {
            return try_clone_slice(spelled.as_ref()?);
        }
        let spelled = solve(stop);
        // A stopped search proves nothing about the spelling.
        if spelled.is_some() || !stop.reached() {
            let kept = match &spelled {
                Some(tokens) => Some(try_clone_slice(tokens)?),
                None => None,
            };
            known.try_reserve(1).ok()?;
            known.push((relevant, kept));
        }
        spelled
    }
}

/// A block's matches, by token index and decoded start, found once for every
/// ban: a ban changes only matches, so a rewrite need not visit literals.
struct BlockMatches {
    matches: Vec<(usize, usize)>,
    /// Whether the tokens decode to exactly the block's bytes.
    complete: bool,
    extra_bits: u64,
}

impl BlockMatches {
    fn new(block: &ParsedBlock) -> Option<Self> {
        let mut matches = Vec::new();
        // Each match has one distance symbol.
        let match_count = block.distance_frequencies.iter().sum::<u32>();
        matches.try_reserve_exact(match_count as usize).ok()?;
        let mut position = 0_usize;
        for (index, token) in block.tokens.iter().enumerate() {
            if matches!(token, Token::Match { .. }) {
                matches.try_reserve(1).ok()?;
                matches.push((index, position));
            }
            position = position.checked_add(token.decoded_len())?;
        }
        Some(Self {
            matches,
            complete: position == block.plain.len(),
            extra_bits: token_extra_bits(&block.tokens),
        })
    }
}

/// A ban's rewrite of a block: each replaced match's token index with the end
/// of its replacement in `tokens`, and the rewritten block's symbol counts and
/// extra bits.
struct Rewrite {
    replaced: Vec<(usize, usize)>,
    tokens: Vec<Token>,
    literal_frequencies: [u32; 286],
    distance_frequencies: [u32; 30],
    extra_bits: u64,
}

impl Rewrite {
    /// Count `token` in the rewritten block, or uncount it.
    fn count(&mut self, token: Token, added: bool) {
        let step = |count: &mut u32| {
            *count = if added { *count + 1 } else { *count - 1 };
        };
        match token {
            Token::Literal(value) => step(&mut self.literal_frequencies[usize::from(value)]),
            Token::Match {
                length_symbol,
                distance_symbol,
                ..
            } => {
                step(&mut self.literal_frequencies[usize::from(length_symbol)]);
                step(&mut self.distance_frequencies[usize::from(distance_symbol)]);
                let extra = u64::from(length_extra_bits(length_symbol))
                    + u64::from(distance_extra_bits(distance_symbol));
                self.extra_bits = if added {
                    self.extra_bits + extra
                } else {
                    self.extra_bits - extra
                };
            }
        }
    }

    fn estimate(&self) -> Option<u64> {
        estimate_boundary_block_bits(
            &self.literal_frequencies,
            &self.distance_frequencies,
            self.extra_bits,
            true,
        )
    }

    /// The rewritten block's tokens.
    fn spell(&self, block: &ParsedBlock) -> Option<Vec<Token>> {
        let mut result = Vec::new();
        result
            .try_reserve_exact(block.tokens.len() - self.replaced.len() + self.tokens.len())
            .ok()?;
        let (mut next, mut start) = (0, 0);
        for &(index, end) in &self.replaced {
            result.extend_from_slice(&block.tokens[next..index]);
            result.extend_from_slice(&self.tokens[start..end]);
            (next, start) = (index + 1, end);
        }
        result.extend_from_slice(&block.tokens[next..]);
        Some(result)
    }
}

#[allow(clippy::too_many_arguments)]
fn rewrite(
    block: &ParsedBlock,
    matches: &BlockMatches,
    ban: Ban,
    literal_lengths: &[u8],
    distance_lengths: &[u8],
    mut spellings: Option<&mut SubmatchSpellings>,
    budget: &mut SymbolSetBudget,
    stop: &mut SearchStop<'_>,
) -> Option<Rewrite> {
    if stop.reached() || !budget.spend(block.tokens.len()) {
        return None;
    }
    let mut affected = 0;
    let mut bytes = 0;
    let mut edges = 0;
    for &(index, _) in &matches.matches {
        let token = block.tokens[index];
        if !ban.affects(token) {
            continue;
        }
        affected += 1;
        let length = token.decoded_len();
        bytes += length;
        if affected > MAX_AFFECTED_MATCHES || bytes > MAX_REWRITTEN_BYTES {
            return None;
        }
        if let Token::Match {
            distance_symbol, ..
        } = token
        {
            if ban.distances & (1 << distance_symbol) == 0 {
                // The restricted submatch solver visits this many match edges,
                // including banned lengths. Charge the whole search up front.
                edges += (length - 1) * (length - 2) / 2;
            }
        }
    }
    if affected == 0 || !budget.spend(edges + bytes + block.tokens.len()) || !matches.complete {
        return None;
    }
    debug_assert_eq!(
        (block.literal_frequencies, block.distance_frequencies),
        count_frequencies(&block.tokens)
    );
    let mut rewrite = Rewrite {
        replaced: Vec::new(),
        tokens: Vec::new(),
        literal_frequencies: block.literal_frequencies,
        distance_frequencies: block.distance_frequencies,
        extra_bits: matches.extra_bits,
    };
    rewrite.replaced.try_reserve_exact(affected).ok()?;
    rewrite.tokens.try_reserve_exact(bytes).ok()?;
    // Check the stop every 256 tokens, as a pass over every token would.
    let mut checkpoint = 0;
    for &(index, start) in &matches.matches {
        let token = block.tokens[index];
        let Token::Match {
            length_symbol,
            distance_symbol,
            ..
        } = token
        else {
            continue;
        };
        if !ban.affects(token) {
            continue;
        }
        while checkpoint <= index {
            if stop.reached() {
                return None;
            }
            checkpoint += 256;
        }
        let decoded = &block.plain[start..start + token.decoded_len()];
        rewrite.count(token, false);
        let replacement_start = rewrite.tokens.len();
        if ban.distances & (1 << distance_symbol) != 0 {
            rewrite
                .tokens
                .extend(decoded.iter().copied().map(Token::Literal));
        } else {
            let solve = |stop: &mut SearchStop<'_>| {
                solve_proven_submatch_avoiding(
                    token,
                    decoded,
                    literal_lengths,
                    distance_lengths,
                    ban.lengths,
                    stop,
                )
            };
            let spelled = match spellings.as_deref_mut() {
                Some(spellings) => {
                    let relevant = ban.lengths & ((2 << (length_symbol - 257)) - 1);
                    spellings.spell(index, relevant, stop, solve)
                }
                None => solve(stop),
            }?;
            rewrite.tokens.try_reserve(spelled.len()).ok()?;
            rewrite.tokens.extend(spelled);
        }
        for position in replacement_start..rewrite.tokens.len() {
            let replacement = rewrite.tokens[position];
            rewrite.count(replacement, true);
        }
        rewrite.replaced.push((index, rewrite.tokens.len()));
    }
    while checkpoint < block.tokens.len() {
        if stop.reached() {
            return None;
        }
        checkpoint += 256;
    }
    Some(rewrite)
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

/// Keep a proposal among the best `MAX_SELECTED`, spelling its tokens only if
/// it is kept.
fn retain_proposal(
    proposals: &mut Vec<Proposal>,
    ban: Ban,
    score: u64,
    spell: impl FnOnce() -> Option<Vec<Token>>,
) {
    let position = proposals
        .iter()
        .position(|p| (score, ban) < p.key())
        .unwrap_or(proposals.len());
    if position >= MAX_SELECTED {
        return;
    }
    let Some(tokens) = spell() else {
        return;
    };
    if proposals.len() == MAX_SELECTED {
        proposals.pop();
    }
    proposals.insert(position, Proposal { ban, score, tokens });
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

/// Price one token spelling with the standard planner (strict, not exhaustive)
/// plus depth-limited and RLE-smoothed dynamic-code searches; Max's larger
/// cross-product of code builders was not needed in measured cases.
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
    let matches = BlockMatches::new(block)?;
    let parent_score = estimate_boundary_block_bits(
        &block.literal_frequencies,
        &block.distance_frequencies,
        matches.extra_bits,
        true,
    )?;
    let mut proposals = Vec::new();
    proposals.try_reserve_exact(MAX_SELECTED).ok()?;
    let mut seeds = [Vec::new(), Vec::new()];
    for list in &mut seeds {
        list.try_reserve_exact(MAX_CROSS_SEEDS).ok()?;
    }
    let mut spellings = SubmatchSpellings::new(block)?;
    for ban in masks(block)? {
        if stop.reached() {
            break;
        }
        let Some(rewritten) = rewrite(
            block,
            &matches,
            ban,
            &parent.literal_lengths,
            &parent.distance_lengths,
            Some(&mut spellings),
            budget,
            stop,
        ) else {
            continue;
        };
        let Some(score) = rewritten.estimate() else {
            continue;
        };
        let count = ban.targeted_symbols(block);
        if count <= 2 {
            retain_seed(&mut seeds[usize::from(ban.distances != 0)], (score, ban));
        }
        // The estimate only ranks candidates; it is not a bound. Keep
        // candidates up to `ESTIMATE_WINDOW` bits worse than the parent under
        // the same estimate.
        if count >= 2 && score <= parent_score + ESTIMATE_WINDOW {
            retain_proposal(&mut proposals, ban, score, || rewritten.spell(block));
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
            let Some(rewritten) = rewrite(
                block,
                &matches,
                ban,
                &parent.literal_lengths,
                &parent.distance_lengths,
                Some(&mut spellings),
                budget,
                stop,
            ) else {
                continue;
            };
            let Some(score) = rewritten.estimate() else {
                continue;
            };
            if score <= parent_score + ESTIMATE_WINDOW {
                retain_proposal(&mut proposals, ban, score, || rewritten.spell(block));
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
            // Re-spell the original matches under the new code lengths with
            // the same ban; this never extends a submatch or turns a new
            // literal into a match.
            if let Some(tokens) = rewrite(
                block,
                &matches,
                proposal.ban,
                literal_lengths,
                distance_lengths,
                None,
                budget,
                stop,
            )
            .and_then(|rewritten| rewritten.spell(block))
            {
                if !same_tokens(&tokens, &candidate.tokens) {
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
pub(crate) mod test_support;
#[cfg(test)]
mod tests;
