// SPDX-License-Identifier: MIT
// SPDX-FileCopyrightText: 2026 Andrew C.E. Dent <https://github.com/ace-dent>

//! Proven-submatch resegmentation (D1, D2).
//!
//! A match of length `L` at distance `D` proves that each of its `L` decoded
//! bytes equals the byte `D` positions earlier, so a shorter match at `D`
//! whose bytes lie inside it copies only proven bytes. A solver finds the
//! cheapest spelling of one source match as literals and such submatches
//! under the current codes, optionally avoiding some length symbols. Ranked
//! target matches are rewritten alone and together, and every candidate is
//! priced as a complete block. It runs inside the integrated block search and
//! floor, and on its own as the stream planner's proven-submatch lineage. The
//! `composition` child combines per-match spellings for Max.

use std::cmp::Reverse;
use std::sync::Arc;

use crate::deflate::minima::SuffixMinima;
use crate::deflate::model::{
    count_frequencies, distance_extra_bits, length_extra_bits, same_tokens, ParsedBlock,
    PlannedBlock, Token, LENGTH_BASE as DEFLATE_LENGTH_BASE,
};
use crate::deflate::stop::SearchStop;
use crate::sort::SortShared;
use crate::Options;

use super::{
    consider_tokens, estimated_length, fixed_lengths, new_token_candidate, plan_lengths,
    plan_tokens, repacked_match, rewrite_258_symbols, MANDATORY_FLOOR_MAX_PLAIN,
    MANDATORY_FLOOR_MAX_TOKENS, SHORT_FAMILY_MAX_TOKENS,
};

mod composition;
pub(crate) use composition::improve_plan_with_header_aware_proven_composition;

const DEFAULT_PROVEN_SUBMATCH_TARGETS: usize = 8;
const DEFAULT_PROVEN_SUBMATCH_INDIVIDUAL_TRIALS: usize = 4;
const COMPACT_PROVEN_SUBMATCH_TOKENS: usize = 4_000;
const DEFAULT_PROVEN_SUBMATCH_TARGETS_PER_SYMBOL: usize = 2;
const MAX_PROVEN_SUBMATCH_TARGETS: usize = 32;
const MAX_PROVEN_SUBMATCH_INDIVIDUAL_TRIALS: usize = 8;
const MAX_PROVEN_SUBMATCH_TARGETS_PER_SYMBOL: usize = 4;
const MAX_PROVEN_SUBMATCH_FULL_TOKENS: usize = 12_000;
const MAX_PROVEN_SUBMATCH_FULL_PLAIN: usize = 80_000;
pub(crate) const PROVEN_SUBMATCH_FULL_MATCH_LIMIT: usize = 512;
const MAX_PROVEN_SUBMATCH_ELIMINATION_MATCHES: usize = 64;
const MAX_PROVEN_SUBMATCH_PASSES: usize = 12;
const PROVEN_SUBMATCH_RARE_FREQUENCY: u32 = 2;
const PROVEN_SUBMATCH_EXPENSIVE_CODE_BITS: u8 = 9;
const PROVEN_SUBMATCH_TRANSITION_BYTES: u16 = 2;
const PROVEN_SUBMATCH_BOUNDARY_RADIUS: usize = 8;
const PROVEN_COMPOSITION_BEAM_WIDTH: usize = 16;

#[derive(Debug, Clone, Copy)]
enum ProvenSubmatchChoice {
    End,
    Literal(u8),
    Match(Token),
}

#[derive(Debug, Clone, Copy)]
struct ProvenSubmatchRank {
    highest: bool,
    rare: bool,
    near_boundary: bool,
    transition: bool,
    expensive: bool,
    code_bits: u8,
    frequency: u32,
    token_index: usize,
}

impl ProvenSubmatchRank {
    fn key(self) -> (bool, bool, bool, bool, bool, Reverse<u8>, u32, usize) {
        (
            !self.highest,
            !self.rare,
            !self.near_boundary,
            !self.transition,
            !self.expensive,
            Reverse(self.code_bits),
            self.frequency,
            self.token_index,
        )
    }
}

#[derive(Debug, Clone, Copy)]
struct ProvenSubmatchTarget {
    token_index: usize,
    plain_offset: usize,
    source: Token,
    rank: ProvenSubmatchRank,
}

struct ProvenSubmatchRewrite {
    token_index: usize,
    replacement: Vec<Token>,
    estimated_saving: i64,
    rank: ProvenSubmatchRank,
}

#[derive(Debug, Clone, Copy)]
enum ProvenSubmatchRestriction {
    None,
    Symbol(u16),
    SourceSymbol,
}

pub(crate) fn proven_submatch_route_eligible(
    tokens: &[Token],
    plain_len: usize,
    exhaustive: bool,
) -> bool {
    let model_is_bounded = if exhaustive {
        tokens.len() <= SHORT_FAMILY_MAX_TOKENS && plain_len <= MANDATORY_FLOOR_MAX_PLAIN
    } else {
        tokens.len() <= MANDATORY_FLOOR_MAX_TOKENS && plain_len <= MANDATORY_FLOOR_MAX_PLAIN
    };
    model_is_bounded
        && tokens
            .iter()
            .any(|token| matches!(token, Token::Match { .. }))
}

pub(crate) fn compact_proven_submatch_route_eligible(tokens: &[Token], plain_len: usize) -> bool {
    tokens.len() <= COMPACT_PROVEN_SUBMATCH_TOKENS
        && plain_len <= MAX_PROVEN_SUBMATCH_FULL_PLAIN
        && tokens
            .iter()
            .any(|token| matches!(token, Token::Match { .. }))
}

/// Re-spell existing matches as literals and proven submatches.
///
/// Default prices a bounded set of target matches. Max widens compact blocks to
/// every match and repeats while the spelling keeps changing.
pub(super) fn consider_proven_submatches(
    block: &ParsedBlock,
    alignment: u8,
    options: &Options,
    stop: &mut SearchStop<'_>,
    best: &mut PlannedBlock,
) {
    if !proven_submatch_route_eligible(&best.tokens, block.plain.len(), options.exhaustive)
        || stop.reached()
    {
        return;
    }

    for pass in 0..MAX_PROVEN_SUBMATCH_PASSES {
        if stop.reached() {
            return;
        }
        let pass_tokens = Arc::clone(&best.tokens);
        let (literal_lengths, distance_lengths) = proven_submatch_seed_lengths(block, best);
        let (literal_frequencies, _) = count_frequencies(&pass_tokens);
        let match_count = pass_tokens
            .iter()
            .filter(|token| matches!(token, Token::Match { .. }))
            .count();
        if match_count == 0 {
            return;
        }
        let full_graph = options.exhaustive
            && pass_tokens.len() <= MAX_PROVEN_SUBMATCH_FULL_TOKENS
            && block.plain.len() <= MAX_PROVEN_SUBMATCH_FULL_PLAIN
            && match_count <= PROVEN_SUBMATCH_FULL_MATCH_LIMIT;

        let Some(targets) = select_proven_submatch_targets(
            &pass_tokens,
            block.plain.len(),
            &block.source_splits,
            &literal_frequencies,
            &literal_lengths,
            full_graph,
            options.exhaustive,
            stop,
        ) else {
            return;
        };
        let Some(mut payload_rewrites) = build_proven_submatch_rewrites(
            &targets,
            &block.plain,
            &literal_lengths,
            &distance_lengths,
            ProvenSubmatchRestriction::None,
            stop,
        ) else {
            return;
        };
        // The source token wins estimated-cost ties. A second rewrite set avoids
        // the source's length symbol, so exact pricing can accept a tie or a
        // small payload loss that drops that symbol from the header.
        let Some(mut symbol_free_rewrites) = build_proven_submatch_rewrites(
            &targets,
            &block.plain,
            &literal_lengths,
            &distance_lengths,
            ProvenSubmatchRestriction::SourceSymbol,
            stop,
        ) else {
            return;
        };

        // Pricing a few single-match rewrites alone stops one useful rewrite
        // from being masked by the others.
        let compact_for_trials =
            compact_proven_submatch_route_eligible(&pass_tokens, block.plain.len());
        let individual_limit = if compact_for_trials {
            if options.exhaustive {
                MAX_PROVEN_SUBMATCH_INDIVIDUAL_TRIALS
            } else {
                DEFAULT_PROVEN_SUBMATCH_INDIVIDUAL_TRIALS
            }
        } else {
            0
        };
        let mut priced_candidates = Vec::new();
        let priced_candidate_limit = individual_limit
            .saturating_mul(2)
            .saturating_add(3)
            .saturating_add(if options.exhaustive {
                PROVEN_COMPOSITION_BEAM_WIDTH
            } else {
                0
            });
        if priced_candidates
            .try_reserve_exact(priced_candidate_limit)
            .is_err()
        {
            return;
        }
        if consider_proven_submatch_rewrite_family(
            block,
            &pass_tokens,
            alignment,
            options,
            individual_limit,
            &mut payload_rewrites,
            &mut priced_candidates,
            stop,
            best,
        )
        .is_none()
        {
            return;
        }
        symbol_free_rewrites.sort_shared_by_key(|rewrite| rewrite.token_index);
        if !same_proven_submatch_rewrites(&payload_rewrites, &symbol_free_rewrites)
            && consider_proven_submatch_rewrite_family(
                block,
                &pass_tokens,
                alignment,
                options,
                individual_limit,
                &mut symbol_free_rewrites,
                &mut priced_candidates,
                stop,
                best,
            )
            .is_none()
        {
            return;
        }

        // Per-symbol target caps stop one common symbol from crowding out the
        // others, so a separate candidate rewrites every match of the highest
        // length symbol.
        if !stop.reached() {
            consider_highest_length_symbol_elimination(
                block,
                &pass_tokens,
                alignment,
                options,
                &literal_lengths,
                &distance_lengths,
                &mut priced_candidates,
                stop,
                best,
            );
        }

        if !options.exhaustive
            || same_tokens(&best.tokens, &pass_tokens)
            || pass + 1 == MAX_PROVEN_SUBMATCH_PASSES
        {
            break;
        }
    }
}

/// Run the proven-submatch search on a completed block plan. The stream
/// optimizer runs this as a separate lineage, so a quick local win cannot
/// replace a spelling that reaches a better fixed point after replay.
pub(crate) fn improve_plan_with_proven_submatches(
    block: &ParsedBlock,
    alignment: u8,
    options: &Options,
    stop: &mut SearchStop<'_>,
    mut best: PlannedBlock,
) -> PlannedBlock {
    consider_proven_submatches(block, alignment, options, stop, &mut best);
    best
}

#[allow(clippy::too_many_arguments)]
fn consider_proven_submatch_rewrite_family(
    block: &ParsedBlock,
    source: &[Token],
    alignment: u8,
    options: &Options,
    individual_limit: usize,
    rewrites: &mut [ProvenSubmatchRewrite],
    priced_candidates: &mut Vec<Arc<Vec<Token>>>,
    stop: &mut SearchStop<'_>,
    best: &mut PlannedBlock,
) -> Option<()> {
    if individual_limit != 0 && !rewrites.is_empty() {
        let mut order = Vec::new();
        order.try_reserve_exact(rewrites.len()).ok()?;
        order.extend(0..rewrites.len());
        order.sort_shared_by_key(|&index| {
            (
                Reverse(rewrites[index].estimated_saving),
                rewrites[index].rank.key(),
            )
        });
        for index in order.into_iter().take(individual_limit) {
            if stop.reached() {
                return None;
            }
            let Some(tokens) = apply_proven_submatch_rewrites(
                source,
                block.plain.len(),
                std::slice::from_ref(&rewrites[index]),
            ) else {
                continue;
            };
            consider_unique_proven_submatch_tokens(
                block,
                tokens,
                alignment,
                options,
                priced_candidates,
                stop,
                best,
            );
        }
    }

    // A single rewrite was already priced above, unless single-match trials
    // are off.
    if should_price_combined_proven_submatch_candidate(rewrites.len(), individual_limit) {
        if stop.reached() {
            return None;
        }
        rewrites.sort_shared_by_key(|rewrite| rewrite.token_index);
        if let Some(tokens) = apply_proven_submatch_rewrites(source, block.plain.len(), rewrites) {
            consider_unique_proven_submatch_tokens(
                block,
                tokens,
                alignment,
                options,
                priced_candidates,
                stop,
                best,
            );
        }
    }
    Some(())
}

fn should_price_combined_proven_submatch_candidate(
    rewrite_count: usize,
    individual_limit: usize,
) -> bool {
    rewrite_count > 1 || (rewrite_count == 1 && individual_limit == 0)
}

fn same_proven_submatch_rewrites(
    left: &[ProvenSubmatchRewrite],
    right: &[ProvenSubmatchRewrite],
) -> bool {
    left.len() == right.len()
        && left.iter().zip(right).all(|(left, right)| {
            left.token_index == right.token_index && left.replacement == right.replacement
        })
}

fn proven_submatch_seed_lengths(block: &ParsedBlock, best: &PlannedBlock) -> (Vec<u8>, Vec<u8>) {
    plan_lengths(best)
        .or_else(|| {
            Some((
                block.original_literal_lengths.as_ref()?.to_vec(),
                block.original_distance_lengths.as_ref()?.to_vec(),
            ))
        })
        .unwrap_or_else(fixed_lengths)
}

#[allow(clippy::too_many_arguments)]
fn select_proven_submatch_targets(
    tokens: &[Token],
    decoded_bytes: usize,
    source_splits: &[usize],
    literal_frequencies: &[u32; 286],
    literal_lengths: &[u8],
    full_graph: bool,
    exhaustive: bool,
    stop: &mut SearchStop<'_>,
) -> Option<Vec<ProvenSubmatchTarget>> {
    let highest_symbol = tokens
        .iter()
        .filter_map(|token| match *token {
            Token::Match { length_symbol, .. } => Some(length_symbol),
            Token::Literal(_) => None,
        })
        .max();
    let target_limit = if full_graph {
        tokens
            .iter()
            .filter(|token| matches!(token, Token::Match { length, .. } if *length >= 4))
            .count()
    } else if exhaustive {
        MAX_PROVEN_SUBMATCH_TARGETS
    } else {
        DEFAULT_PROVEN_SUBMATCH_TARGETS
    };
    let per_symbol_limit = if exhaustive {
        MAX_PROVEN_SUBMATCH_TARGETS_PER_SYMBOL
    } else {
        DEFAULT_PROVEN_SUBMATCH_TARGETS_PER_SYMBOL
    };
    let mut targets = Vec::new();
    targets.try_reserve_exact(target_limit).ok()?;
    let mut plain_offset = 0_usize;

    for (token_index, &token) in tokens.iter().enumerate() {
        if token_index & 1_023 == 0 && stop.reached() {
            return None;
        }
        let end = plain_offset.checked_add(token.decoded_len())?;
        if end > decoded_bytes {
            return None;
        }
        let Token::Match {
            length,
            length_symbol,
            ..
        } = token
        else {
            plain_offset = end;
            continue;
        };
        if length < 4 {
            plain_offset = end;
            continue;
        }

        let frequency = literal_frequencies[usize::from(length_symbol)];
        let code_bits = literal_lengths
            .get(usize::from(length_symbol))
            .copied()
            .unwrap_or(0);
        let transition = length_near_code_transition(length, length_symbol);
        let rank = ProvenSubmatchRank {
            highest: highest_symbol == Some(length_symbol),
            rare: frequency <= PROVEN_SUBMATCH_RARE_FREQUENCY,
            near_boundary: match_near_source_split(plain_offset, end, source_splits),
            transition,
            expensive: code_bits >= PROVEN_SUBMATCH_EXPENSIVE_CODE_BITS || code_bits == 0,
            code_bits,
            frequency,
            token_index,
        };
        let targeted =
            rank.highest || rank.rare || rank.near_boundary || rank.transition || rank.expensive;
        if full_graph || exhaustive || targeted {
            let target = ProvenSubmatchTarget {
                token_index,
                plain_offset,
                source: token,
                rank,
            };
            if full_graph {
                targets.push(target);
            } else {
                insert_ranked_proven_submatch_target(
                    &mut targets,
                    target,
                    target_limit,
                    per_symbol_limit,
                );
            }
        }
        plain_offset = end;
    }
    (plain_offset == decoded_bytes).then_some(targets)
}

fn length_near_code_transition(length: u16, length_symbol: u16) -> bool {
    let Some(index) = length_symbol.checked_sub(257).map(usize::from) else {
        return false;
    };
    let Some(&base) = DEFLATE_LENGTH_BASE.get(index) else {
        return false;
    };
    let near_lower = length.saturating_sub(base) <= PROVEN_SUBMATCH_TRANSITION_BYTES;
    let near_upper = DEFLATE_LENGTH_BASE
        .get(index + 1)
        .is_some_and(|&next_base| {
            next_base.saturating_sub(length) <= PROVEN_SUBMATCH_TRANSITION_BYTES
        });
    near_lower || near_upper
}

fn insert_ranked_proven_submatch_target(
    targets: &mut Vec<ProvenSubmatchTarget>,
    target: ProvenSubmatchTarget,
    limit: usize,
    per_symbol_limit: usize,
) {
    if limit == 0 || per_symbol_limit == 0 {
        return;
    }
    let key = target.rank.key();
    let Token::Match {
        length_symbol: target_symbol,
        ..
    } = target.source
    else {
        return;
    };
    let mut matching_count = 0_usize;
    let mut worst_match = None;
    for (index, existing) in targets.iter().enumerate() {
        if matches!(
            existing.source,
            Token::Match { length_symbol, .. } if length_symbol == target_symbol
        ) {
            matching_count += 1;
            if worst_match.map_or(true, |(_, worst_key)| existing.rank.key() > worst_key) {
                worst_match = Some((index, existing.rank.key()));
            }
        }
    }
    if matching_count >= per_symbol_limit {
        let Some((worst_index, worst_key)) = worst_match else {
            return;
        };
        if key >= worst_key {
            return;
        }
        targets.remove(worst_index);
    }
    let position = targets
        .iter()
        .position(|existing| key < existing.rank.key())
        .unwrap_or(targets.len());
    if targets.len() < limit {
        targets.insert(position, target);
    } else if position < limit {
        targets.pop();
        targets.insert(position, target);
    }
}

fn match_near_source_split(start: usize, end: usize, source_splits: &[usize]) -> bool {
    let lower = start.saturating_sub(PROVEN_SUBMATCH_BOUNDARY_RADIUS);
    let upper = end.saturating_add(PROVEN_SUBMATCH_BOUNDARY_RADIUS);
    let first = source_splits.partition_point(|&split| split < lower);
    source_splits
        .get(first)
        .is_some_and(|&split| split <= upper)
}

#[allow(clippy::too_many_arguments)]
fn build_proven_submatch_rewrites(
    targets: &[ProvenSubmatchTarget],
    plain: &[u8],
    literal_lengths: &[u8],
    distance_lengths: &[u8],
    restriction: ProvenSubmatchRestriction,
    stop: &mut SearchStop<'_>,
) -> Option<Vec<ProvenSubmatchRewrite>> {
    let mut rewrites = Vec::new();
    rewrites.try_reserve_exact(targets.len()).ok()?;
    for target in targets {
        if stop.reached() {
            return None;
        }
        let length = target.source.decoded_len();
        let end = target.plain_offset.checked_add(length)?;
        let decoded = plain.get(target.plain_offset..end)?;
        let forbidden_length_symbol = match restriction {
            ProvenSubmatchRestriction::None => None,
            ProvenSubmatchRestriction::Symbol(symbol) => Some(symbol),
            ProvenSubmatchRestriction::SourceSymbol => match target.source {
                Token::Match { length_symbol, .. } => Some(length_symbol),
                Token::Literal(_) => return None,
            },
        };
        let replacement = solve_proven_submatch(
            target.source,
            decoded,
            literal_lengths,
            distance_lengths,
            forbidden_length_symbol,
            stop,
        );
        if stop.reached() {
            return None;
        }
        let Some(replacement) = replacement else {
            continue;
        };
        let source_bits =
            estimated_match_token_bits(target.source, literal_lengths, distance_lengths)?;
        let replacement_bits =
            estimated_tokens_bits(&replacement, literal_lengths, distance_lengths)?;
        rewrites.push(ProvenSubmatchRewrite {
            token_index: target.token_index,
            replacement,
            estimated_saving: i64::try_from(source_bits)
                .ok()?
                .checked_sub(i64::try_from(replacement_bits).ok()?)?,
            rank: target.rank,
        });
    }
    Some(rewrites)
}

fn solve_proven_submatch(
    source: Token,
    decoded: &[u8],
    literal_lengths: &[u8],
    distance_lengths: &[u8],
    forbidden_length_symbol: Option<u16>,
    stop: &mut SearchStop<'_>,
) -> Option<Vec<Token>> {
    let forbidden = forbidden_length_symbol
        .filter(|symbol| (257..=285).contains(symbol))
        .map_or(0, |symbol| 1 << (symbol - 257));
    solve_proven_submatch_avoiding(
        source,
        decoded,
        literal_lengths,
        distance_lengths,
        forbidden,
        stop,
    )
}

/// Find the cheapest spelling of one source match as literals and proven
/// submatches, never using a length symbol in `forbidden_length_symbols` (bit
/// 0 is symbol 257). Costs are estimated from the given code lengths. Returns
/// `None` if the source token itself is the best spelling.
pub(in crate::deflate) fn solve_proven_submatch_avoiding(
    source: Token,
    decoded: &[u8],
    literal_lengths: &[u8],
    distance_lengths: &[u8],
    forbidden_length_symbols: u32,
    stop: &mut SearchStop<'_>,
) -> Option<Vec<Token>> {
    let Token::Match {
        length,
        length_symbol,
        ..
    } = source
    else {
        return None;
    };
    if usize::from(length) != decoded.len() || decoded.len() > 258 || decoded.len() < 3 {
        return None;
    }
    if stop.reached() {
        return None;
    }

    // Every submatch keeps the source distance, so each length is built and
    // priced once.
    let mut matches = [None; 259];
    for (length, slot) in matches
        .iter_mut()
        .enumerate()
        .take(decoded.len() + 1)
        .skip(3)
    {
        let token = repacked_match(source, length as u16)?;
        let Token::Match { length_symbol, .. } = token else {
            return None;
        };
        if forbidden_length_symbols & (1 << (length_symbol - 257)) == 0 {
            *slot = Some((
                token,
                estimated_match_token_bits(token, literal_lengths, distance_lengths)?,
            ));
        }
    }

    // A canonical length family shares one price, so each needs only its
    // cheapest suffix. A suffix minimum keeps the lowest end on a tie, which
    // is the direct scan's ascending-length preference.
    let symbol = |entry: Option<(Token, u64)>| match entry {
        Some((Token::Match { length_symbol, .. }, _)) => Some(length_symbol),
        _ => None,
    };
    let mut families = [(0_usize, 0_usize, 0_u64); 29];
    let mut family_count = 0;
    for length in 3..=decoded.len() {
        let Some((_, cost)) = matches[length] else {
            continue;
        };
        if family_count > 0 && symbol(matches[length - 1]) == symbol(matches[length]) {
            families[family_count - 1].1 = length;
        } else {
            families[family_count] = (length, length, cost);
            family_count += 1;
        }
    }

    let mut costs = [u64::MAX; 259];
    let mut choices = [ProvenSubmatchChoice::End; 259];
    costs[decoded.len()] = 0;
    let mut minima = SuffixMinima::new(decoded.len());
    minima.insert(decoded.len(), decoded.len(), &costs);

    for start in (0..decoded.len()).rev() {
        if start & 31 == 0 && stop.reached() {
            return None;
        }
        let mut best_cost = u64::MAX;
        let mut best_choice = ProvenSubmatchChoice::End;

        // Retain the exact source token as the first whole-span edge. Strict
        // comparisons below preserve it on an estimated-cost tie.
        if start == 0 && forbidden_length_symbols & (1 << (length_symbol - 257)) == 0 {
            best_cost = estimated_match_token_bits(source, literal_lengths, distance_lengths)?;
            best_choice = ProvenSubmatchChoice::Match(source);
        }

        let literal_cost = estimated_length(literal_lengths, usize::from(decoded[start]))
            .checked_add(costs[start + 1])?;
        if literal_cost < best_cost {
            best_cost = literal_cost;
            best_choice = ProvenSubmatchChoice::Literal(decoded[start]);
        }

        for &(shortest, longest, match_cost) in &families[..family_count] {
            if start + shortest > decoded.len() {
                break;
            }
            let end = minima.minimum(
                start + shortest,
                (start + longest).min(decoded.len()),
                &costs,
            );
            let candidate = match_cost.checked_add(costs[end])?;
            if candidate < best_cost {
                best_cost = candidate;
                best_choice = ProvenSubmatchChoice::Match(matches[end - start]?.0);
            }
        }
        costs[start] = best_cost;
        choices[start] = best_choice;
        minima.insert(start, decoded.len(), &costs);
    }
    if stop.reached() {
        return None;
    }

    let mut token_count = 0_usize;
    let mut at = 0_usize;
    while at < decoded.len() {
        at = match choices[at] {
            ProvenSubmatchChoice::End => return None,
            ProvenSubmatchChoice::Literal(_) => at.checked_add(1)?,
            ProvenSubmatchChoice::Match(token) => at.checked_add(token.decoded_len())?,
        };
        token_count = token_count.checked_add(1)?;
    }
    if at != decoded.len() {
        return None;
    }

    let mut replacement = new_token_candidate(token_count, decoded.len())?;
    at = 0;
    while at < decoded.len() {
        match choices[at] {
            ProvenSubmatchChoice::End => return None,
            ProvenSubmatchChoice::Literal(value) => {
                replacement.push(Token::Literal(value));
                at += 1;
            }
            ProvenSubmatchChoice::Match(token) => {
                replacement.push(token);
                at = at.checked_add(token.decoded_len())?;
            }
        }
    }
    if replacement.as_slice() == std::slice::from_ref(&source) {
        None
    } else {
        Some(replacement)
    }
}

fn estimated_match_token_bits(
    token: Token,
    literal_lengths: &[u8],
    distance_lengths: &[u8],
) -> Option<u64> {
    let Token::Match {
        length_symbol,
        distance_symbol,
        ..
    } = token
    else {
        return None;
    };
    estimated_length(literal_lengths, usize::from(length_symbol))
        .checked_add(u64::from(length_extra_bits(length_symbol)))?
        .checked_add(estimated_length(
            distance_lengths,
            usize::from(distance_symbol),
        ))?
        .checked_add(u64::from(distance_extra_bits(distance_symbol)))
}

fn estimated_tokens_bits(
    tokens: &[Token],
    literal_lengths: &[u8],
    distance_lengths: &[u8],
) -> Option<u64> {
    tokens.iter().try_fold(0_u64, |bits, &token| {
        bits.checked_add(match token {
            Token::Literal(value) => estimated_length(literal_lengths, usize::from(value)),
            Token::Match { .. } => {
                estimated_match_token_bits(token, literal_lengths, distance_lengths)?
            }
        })
    })
}

fn apply_proven_submatch_rewrites(
    source: &[Token],
    decoded_bytes: usize,
    rewrites: &[ProvenSubmatchRewrite],
) -> Option<Vec<Token>> {
    if rewrites.is_empty() {
        return None;
    }
    let mut output_count = source.len();
    let mut previous = None;
    for rewrite in rewrites {
        if rewrite.token_index >= source.len()
            || previous.is_some_and(|index| rewrite.token_index <= index)
        {
            return None;
        }
        let source_length = source[rewrite.token_index].decoded_len();
        let replacement_length = rewrite
            .replacement
            .iter()
            .try_fold(0_usize, |total, token| {
                total.checked_add(token.decoded_len())
            })?;
        if replacement_length != source_length {
            return None;
        }
        output_count = output_count
            .checked_sub(1)?
            .checked_add(rewrite.replacement.len())?;
        previous = Some(rewrite.token_index);
    }

    let mut output = new_token_candidate(output_count, decoded_bytes)?;
    let mut rewrite_index = 0_usize;
    for (token_index, &token) in source.iter().enumerate() {
        if rewrites
            .get(rewrite_index)
            .is_some_and(|rewrite| rewrite.token_index == token_index)
        {
            output.extend_from_slice(&rewrites[rewrite_index].replacement);
            rewrite_index += 1;
        } else {
            output.push(token);
        }
    }
    (rewrite_index == rewrites.len()).then_some(output)
}

#[allow(clippy::too_many_arguments)]
fn consider_highest_length_symbol_elimination(
    block: &ParsedBlock,
    tokens: &[Token],
    alignment: u8,
    options: &Options,
    literal_lengths: &[u8],
    distance_lengths: &[u8],
    priced_candidates: &mut Vec<Arc<Vec<Token>>>,
    stop: &mut SearchStop<'_>,
    best: &mut PlannedBlock,
) {
    let Some(highest_symbol) = tokens
        .iter()
        .filter_map(|token| match *token {
            Token::Match { length_symbol, .. } => Some(length_symbol),
            Token::Literal(_) => None,
        })
        .max()
    else {
        return;
    };
    let limit = if options.exhaustive {
        MAX_PROVEN_SUBMATCH_ELIMINATION_MATCHES
    } else {
        DEFAULT_PROVEN_SUBMATCH_TARGETS
    };
    let occurrence_count = tokens
        .iter()
        .filter(|token| {
            matches!(
                token,
                Token::Match {
                    length_symbol,
                    ..
                } if *length_symbol == highest_symbol
            )
        })
        .count();
    if occurrence_count == 0 || occurrence_count > limit {
        return;
    }

    let mut targets = Vec::new();
    if targets.try_reserve_exact(occurrence_count).is_err() {
        return;
    }
    let mut plain_offset = 0_usize;
    for (token_index, &token) in tokens.iter().enumerate() {
        if token_index & 1_023 == 0 && stop.reached() {
            return;
        }
        let end = match plain_offset.checked_add(token.decoded_len()) {
            Some(end) if end <= block.plain.len() => end,
            _ => return,
        };
        if matches!(
            token,
            Token::Match {
                length_symbol,
                ..
            } if length_symbol == highest_symbol
        ) {
            targets.push(ProvenSubmatchTarget {
                token_index,
                plain_offset,
                source: token,
                rank: ProvenSubmatchRank {
                    highest: true,
                    rare: occurrence_count <= PROVEN_SUBMATCH_RARE_FREQUENCY as usize,
                    near_boundary: match_near_source_split(plain_offset, end, &block.source_splits),
                    transition: false,
                    expensive: true,
                    code_bits: literal_lengths
                        .get(usize::from(highest_symbol))
                        .copied()
                        .unwrap_or(0),
                    frequency: occurrence_count.try_into().unwrap_or(u32::MAX),
                    token_index,
                },
            });
        }
        plain_offset = end;
    }
    if plain_offset != block.plain.len() {
        return;
    }
    let Some(mut rewrites) = build_proven_submatch_rewrites(
        &targets,
        &block.plain,
        literal_lengths,
        distance_lengths,
        ProvenSubmatchRestriction::Symbol(highest_symbol),
        stop,
    ) else {
        return;
    };
    if rewrites.len() != occurrence_count {
        return;
    }
    rewrites.sort_shared_by_key(|rewrite| rewrite.token_index);
    if let Some(candidate) = apply_proven_submatch_rewrites(tokens, block.plain.len(), &rewrites) {
        consider_unique_proven_submatch_tokens(
            block,
            candidate,
            alignment,
            options,
            priced_candidates,
            stop,
            best,
        );
    }
}

#[allow(clippy::too_many_arguments)]
fn consider_unique_proven_submatch_tokens(
    block: &ParsedBlock,
    tokens: Vec<Token>,
    alignment: u8,
    options: &Options,
    priced_candidates: &mut Vec<Arc<Vec<Token>>>,
    stop: &mut SearchStop<'_>,
    best: &mut PlannedBlock,
) {
    if priced_candidates
        .iter()
        .any(|candidate| same_tokens(candidate, &tokens))
    {
        return;
    }
    if let Some(planned_tokens) =
        consider_proven_submatch_tokens(block, tokens, alignment, options, stop, best)
    {
        // If this push cannot allocate, only duplicate detection is lost.
        if priced_candidates.try_reserve(1).is_ok() {
            priced_candidates.push(planned_tokens);
        }
    }
}

fn consider_proven_submatch_tokens(
    block: &ParsedBlock,
    tokens: Vec<Token>,
    alignment: u8,
    options: &Options,
    stop: &mut SearchStop<'_>,
    best: &mut PlannedBlock,
) -> Option<Arc<Vec<Token>>> {
    let candidate = plan_tokens(block, tokens, alignment, options, stop)?;
    let canonical_tokens = Arc::clone(&candidate.tokens);
    if candidate.bits < best.bits {
        *best = candidate;
    }

    // Non-strict mode also prices the length-258 alias (symbol 284).
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
        if let Some(alias) = rewrite_258_symbols(&canonical_tokens, block.plain.len(), true) {
            consider_tokens(block, alias, alignment, options, stop, best);
        }
    }
    Some(canonical_tokens)
}

#[cfg(test)]
mod tests;
