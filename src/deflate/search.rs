// SPDX-License-Identifier: MIT
// SPDX-FileCopyrightText: 2026 Andrew C.E. Dent <https://github.com/ace-dent>

//! Token-spelling searches over the input's existing LZ77 matches.
//!
//! These routes never search the history window or choose a new distance. They
//! spell an existing match as literals, split it into literals and proven
//! submatches, or move boundaries inside a run of adjacent matches at one
//! distance, then rebuild the Huffman codes. Equal-cost spellings are useful
//! intermediate states: their changed frequencies can make the next tree
//! smaller. Items named after DeflOpt, Defluff or deft4j follow those programs.

use std::cmp::Reverse;
use std::collections::HashSet;
use std::sync::Arc;

use crate::sort::SortShared;
use crate::Options;

use super::block::{plan_block, plan_owned_block, reusable_original_bits};
use super::header::{
    best_dynamic_plan_cached, estimate_boundary_block_bits, plan_for_explicit_lengths,
    plan_for_explicit_lengths_with_cost, token_bits_from_frequencies, HeaderPlanCache,
};
use super::huffman::{
    make_lengths_columbo_defluff_limited, make_lengths_deflopt_heap_into_with_scratch,
    make_lengths_defluff_exact, make_lengths_deft4j_java_heap, DefloptHeapScratch,
    FIXED_DISTANCE_CODE_LENGTHS, FIXED_LITERAL_CODE_LENGTHS,
};
use super::minima::SuffixMinima;
use super::model::{
    canonical_length_encoding, count_frequencies, same_tokens, token_extra_bits, try_clone_slice,
    DynamicPlan, ParsedBlock, PlannedBlock, Representation, Token,
    LENGTH_BASE as DEFLATE_LENGTH_BASE, LENGTH_EXTRA_BITS,
};
use super::parse::{parsed_model_bytes, MAX_PARSED_MODEL_BYTES};
use super::stop::SearchStop;

/// Byte limit for one temporary token candidate: expanding one match can
/// produce up to 258 literal tokens.
const MAX_TOKEN_CANDIDATE_BYTES: usize = MAX_PARSED_MODEL_BYTES;
const MANDATORY_FLOOR_MAX_TOKENS: usize = 50_000;
const MANDATORY_FLOOR_MAX_PLAIN: usize = 10_000_000;
const SHORT_FAMILY_MAX_TOKENS: usize = 250_000;
const SHORT_FAMILY_MAX_PLAIN: usize = 64_000_000;
const COMPACT_SHORT_BAND_MAX_TOKENS: usize = 12_000;
const COMPACT_SHORT_BAND_MAX_PLAIN: usize = 80_000;
const COMPACT_SHORT_BAND_ENDS: [u16; 6] = [257, 258, 259, 260, 262, 264];
const TABLE_REPLAY_MAX_TOKENS: usize = 100_000;
const TABLE_REPLAY_PASSES: usize = 4;
const ALL_LITERAL_DENSE_MAX_PLAIN: usize = 80_000;
const ALL_LITERAL_SPARSE_MAX_PLAIN: usize = 1_000_000;
const ALL_LITERAL_SPARSE_MAX_MATCHES: u64 = 256;
/// Most shortened matches (DP layers) for which Default solves a same-distance
/// repartition exactly; above it Default uses `append_minimum_token_fallback`.
const DEFAULT_SAME_DISTANCE_MAX_DP_ACTIVE: usize = 16;
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
const PROVEN_COMPOSITION_MAX_SOURCE_MATCHES: usize = 128;
const PROVEN_COMPOSITION_MAX_TOKENS: usize = 8_000;
const PROVEN_COMPOSITION_MAX_TARGETS: usize = 8;
const PROVEN_COMPOSITION_MAX_SPELLINGS: usize = 4;
const PROVEN_COMPOSITION_BEAM_WIDTH: usize = 16;
const PROVEN_COMPOSITION_PAYLOAD_WINDOW: i64 = 24;
const PROVEN_COMPOSITION_EXACT_LIMIT: usize = 32;
const PROVEN_CLOSED_LOOP_MAX_PLAIN: usize = 1_024;
/// The second round runs only after a strict win and shares the exact-price
/// limit, so a block without a gain is priced at most fifteen times.
const PROVEN_CLOSED_LOOP_ROUNDS: usize = 2;
const PROVEN_CLOSED_LOOP_EXACT_LIMIT: usize = 24;
/// Ranked single-match trials in the compact floor; the small limit stops a
/// many-frame container from spending its shared budget on nearly identical
/// candidates.
const COLUMBO_SINGLE_MATCH_TRIALS: usize = 8;
/// Whole length-family expansions in one length-family chain (D4).
const LENGTH_FAMILY_STEPS: usize = 3;
/// Most expand-and-rebuild passes in one prune chain.
const PRUNE_CHAIN_PASSES: usize = 32;

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

fn consider_same_distance_runs(
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

fn repacked_match(source: Token, length: u16) -> Option<Token> {
    let Token::Match {
        distance,
        distance_symbol,
        distance_extra,
        distance_extra_bits,
        ..
    } = source
    else {
        return None;
    };
    let (length_symbol, length_extra, length_extra_bits) = canonical_length_encoding(length)?;
    Some(Token::Match {
        length,
        distance,
        length_symbol,
        distance_symbol,
        length_extra,
        distance_extra,
        length_extra_bits,
        distance_extra_bits,
    })
}

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
fn consider_proven_submatches(
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
                PROVEN_COMPOSITION_EXACT_LIMIT
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

/// Max only: combine per-match proven spellings with a beam search and price
/// the best combinations with complete headers.
///
/// Beam states must have distinct frequencies.
pub(crate) fn improve_plan_with_header_aware_proven_composition(
    block: &ParsedBlock,
    alignment: u8,
    options: &Options,
    stop: &mut SearchStop<'_>,
    mut best: PlannedBlock,
) -> PlannedBlock {
    if !options.exhaustive
        || best.tokens.len() > PROVEN_COMPOSITION_MAX_TOKENS
        || block.plain.len() > MAX_PROVEN_SUBMATCH_FULL_PLAIN
        || stop.reached()
    {
        return best;
    }
    let source = Arc::clone(&best.tokens);
    let source_matches = source
        .iter()
        .filter(|token| matches!(token, Token::Match { .. }))
        .count();
    if !(2..=PROVEN_COMPOSITION_MAX_SOURCE_MATCHES).contains(&source_matches) {
        return best;
    }
    let Some(menus) = build_current_proven_composition_menus(block, &source, &best, stop) else {
        return best;
    };
    let mut priced_candidates = Vec::new();
    if priced_candidates
        .try_reserve_exact(PROVEN_COMPOSITION_EXACT_LIMIT)
        .is_err()
    {
        return best;
    }
    consider_header_aware_proven_composition(
        block,
        &source,
        alignment,
        options,
        &menus,
        &mut priced_candidates,
        stop,
        &mut best,
    );
    if block.plain.len() <= PROVEN_CLOSED_LOOP_MAX_PLAIN && block.source_splits.is_empty() {
        consider_closed_loop_proven_composition(
            block,
            alignment,
            options,
            &mut priced_candidates,
            stop,
            &mut best,
        );
    }
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

struct ProvenCompositionMenu {
    token_index: usize,
    source: Token,
    alternatives: Vec<ProvenSubmatchRewrite>,
}

#[derive(Clone)]
struct ProvenCompositionState {
    literal_frequencies: [u32; 286],
    distance_frequencies: [u32; 30],
    extra_bits: u64,
    estimated_delta: i64,
    choices: [u8; PROVEN_COMPOSITION_MAX_TARGETS],
    rewrite_count: u8,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ProvenCompositionMove {
    menu_index: usize,
    choice: u8,
}

type ProvenCompositionStateKey = (i64, i64, usize, usize, [u8; PROVEN_COMPOSITION_MAX_TARGETS]);

#[allow(clippy::too_many_arguments)]
fn consider_header_aware_proven_composition(
    block: &ParsedBlock,
    source: &[Token],
    alignment: u8,
    options: &Options,
    menus: &[ProvenCompositionMenu],
    priced_candidates: &mut Vec<Arc<Vec<Token>>>,
    stop: &mut SearchStop<'_>,
    best: &mut PlannedBlock,
) {
    let source_matches = source
        .iter()
        .filter(|token| matches!(token, Token::Match { .. }))
        .count();
    if source.len() > PROVEN_COMPOSITION_MAX_TOKENS
        || !(2..=PROVEN_COMPOSITION_MAX_SOURCE_MATCHES).contains(&source_matches)
    {
        return;
    }

    if menus.len() < 2 {
        return;
    }

    let (literal_frequencies, distance_frequencies) = count_frequencies(source);
    let source_literal_frequencies = literal_frequencies;
    let source_distance_frequencies = distance_frequencies;
    let mut states = Vec::new();
    if states.try_reserve_exact(1).is_err() {
        return;
    }
    states.push(ProvenCompositionState {
        literal_frequencies,
        distance_frequencies,
        extra_bits: token_extra_bits(source),
        estimated_delta: 0,
        choices: [0; PROVEN_COMPOSITION_MAX_TARGETS],
        rewrite_count: 0,
    });
    for (depth, menu) in menus.iter().enumerate() {
        if stop.reached() {
            return;
        }
        let branch_count = menu.alternatives.len().saturating_add(1);
        let Some(capacity) = states.len().checked_mul(branch_count) else {
            return;
        };
        let mut next = Vec::new();
        if next.try_reserve_exact(capacity).is_err() {
            return;
        }
        for state in &states {
            next.push(state.clone());
            for (alternative_index, alternative) in menu.alternatives.iter().enumerate() {
                let mut candidate = state.clone();
                let Some(choice) = alternative_index
                    .checked_add(1)
                    .and_then(|choice| u8::try_from(choice).ok())
                else {
                    return;
                };
                candidate.choices[depth] = choice;
                candidate.rewrite_count = candidate.rewrite_count.saturating_add(1);
                let Some(delta) = candidate
                    .estimated_delta
                    .checked_sub(alternative.estimated_saving)
                else {
                    continue;
                };
                candidate.estimated_delta = delta;
                if !apply_proven_composition_frequency_delta(
                    &mut candidate,
                    menu.source,
                    &alternative.replacement,
                ) {
                    continue;
                }
                if next
                    .iter()
                    .any(|existing| same_proven_composition_frequency_state(existing, &candidate))
                {
                    continue;
                }
                next.push(candidate);
            }
        }
        let Some(best_delta) = next.iter().map(|state| state.estimated_delta).min() else {
            return;
        };
        let ceiling = best_delta.saturating_add(PROVEN_COMPOSITION_PAYLOAD_WINDOW);
        next.retain(|state| state.estimated_delta <= ceiling);
        next.sort_shared_by_key(|state| {
            proven_composition_state_key(
                state,
                &source_literal_frequencies,
                &source_distance_frequencies,
            )
        });
        next.truncate(PROVEN_COMPOSITION_BEAM_WIDTH);
        states = next;
    }

    // `states` is the root or a beam step sorted by `proven_composition_state_key`.
    for state in states
        .iter()
        .filter(|state| state.rewrite_count >= 2)
        .take(PROVEN_COMPOSITION_EXACT_LIMIT)
    {
        if stop.reached() {
            break;
        }
        let Some(tokens) = apply_proven_composition_state(source, block.plain.len(), menus, state)
        else {
            continue;
        };
        if priced_candidates
            .iter()
            .any(|candidate| same_tokens(candidate, &tokens))
        {
            continue;
        }
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

fn build_current_proven_composition_menus(
    block: &ParsedBlock,
    source: &[Token],
    best: &PlannedBlock,
    stop: &mut SearchStop<'_>,
) -> Option<Vec<ProvenCompositionMenu>> {
    let (literal_lengths, distance_lengths) = proven_submatch_seed_lengths(block, best);
    let (literal_frequencies, _) = count_frequencies(source);
    let targets = select_proven_submatch_targets(
        source,
        block.plain.len(),
        &block.source_splits,
        &literal_frequencies,
        &literal_lengths,
        true,
        true,
        stop,
    )?;
    let payload_rewrites = build_proven_submatch_rewrites(
        &targets,
        &block.plain,
        &literal_lengths,
        &distance_lengths,
        ProvenSubmatchRestriction::None,
        stop,
    )?;
    let symbol_free_rewrites = build_proven_submatch_rewrites(
        &targets,
        &block.plain,
        &literal_lengths,
        &distance_lengths,
        ProvenSubmatchRestriction::SourceSymbol,
        stop,
    )?;
    build_proven_composition_menus(
        block,
        &targets,
        &payload_rewrites,
        &symbol_free_rewrites,
        &literal_lengths,
        &distance_lengths,
    )
}

/// Closed-loop search over the proven-spelling menus, adapted from Guetzli.
///
/// Rank single-match changes under the current codes, apply them one at a time
/// and exactly price each prefix, then undo them in ranked order from the
/// all-applied endpoint. After a strict win, rebuild the menus from the winner
/// and repeat.
#[allow(clippy::too_many_arguments)]
fn consider_closed_loop_proven_composition(
    block: &ParsedBlock,
    alignment: u8,
    options: &Options,
    priced_candidates: &mut Vec<Arc<Vec<Token>>>,
    stop: &mut SearchStop<'_>,
    best: &mut PlannedBlock,
) {
    // Tiny blocks only, so exact pricing does not delay later Max work under a
    // shared container deadline.
    if block.plain.len() > PROVEN_CLOSED_LOOP_MAX_PLAIN || !block.source_splits.is_empty() {
        return;
    }
    let mut exact_prices = 0_usize;
    for _ in 0..PROVEN_CLOSED_LOOP_ROUNDS {
        if stop.reached() || exact_prices == PROVEN_CLOSED_LOOP_EXACT_LIMIT {
            break;
        }
        let source = Arc::clone(&best.tokens);
        let source_matches = source
            .iter()
            .filter(|token| matches!(token, Token::Match { .. }))
            .count();
        if source.len() > PROVEN_COMPOSITION_MAX_TOKENS
            || !(3..=PROVEN_COMPOSITION_MAX_SOURCE_MATCHES).contains(&source_matches)
        {
            break;
        }
        let Some(menus) = build_current_proven_composition_menus(block, &source, best, stop) else {
            break;
        };
        if menus.len() < 3 {
            break;
        }
        let Some((mut aggressive_state, moves)) =
            ranked_forward_proven_composition_moves(&source, &menus)
        else {
            break;
        };
        if moves.len() < 3 {
            break;
        }

        let incumbent_bits = best.bits;
        let mut round_best: Option<PlannedBlock> = None;
        let mut aggressive_lengths = None;
        for (index, &movement) in moves.iter().enumerate() {
            if stop.reached() || exact_prices == PROVEN_CLOSED_LOOP_EXACT_LIMIT {
                break;
            }
            if !apply_proven_composition_move(&mut aggressive_state, &menus, movement, true) {
                return;
            }
            let is_aggressive_endpoint = index + 1 == moves.len();
            let Some(candidate) = price_proven_composition_state(
                block,
                &source,
                alignment,
                options,
                &menus,
                &aggressive_state,
                priced_candidates,
                is_aggressive_endpoint,
                stop,
            ) else {
                continue;
            };
            exact_prices += 1;
            if is_aggressive_endpoint {
                aggressive_lengths = plan_lengths(&candidate);
            }
            if candidate.bits < incumbent_bits
                && round_best
                    .as_ref()
                    .map_or(true, |selected| candidate.bits < selected.bits)
            {
                round_best = Some(candidate);
            }
        }

        if let Some((literal_lengths, distance_lengths)) = aggressive_lengths {
            if exact_prices < PROVEN_CLOSED_LOOP_EXACT_LIMIT && !stop.reached() {
                let Some(reverse_moves) = ranked_reverse_proven_composition_moves(
                    &menus,
                    &moves,
                    &aggressive_state,
                    &literal_lengths,
                    &distance_lengths,
                ) else {
                    return;
                };
                let mut repaired_state = aggressive_state;
                for (index, movement) in reverse_moves.into_iter().enumerate() {
                    // Undoing every move reproduces the completed source.
                    if index + 1 == moves.len()
                        || stop.reached()
                        || exact_prices == PROVEN_CLOSED_LOOP_EXACT_LIMIT
                    {
                        break;
                    }
                    if !apply_proven_composition_move(&mut repaired_state, &menus, movement, false)
                    {
                        return;
                    }
                    let Some(candidate) = price_proven_composition_state(
                        block,
                        &source,
                        alignment,
                        options,
                        &menus,
                        &repaired_state,
                        priced_candidates,
                        false,
                        stop,
                    ) else {
                        continue;
                    };
                    exact_prices += 1;
                    if candidate.bits < incumbent_bits
                        && round_best
                            .as_ref()
                            .map_or(true, |selected| candidate.bits < selected.bits)
                    {
                        round_best = Some(candidate);
                    }
                }
            }
        }

        let Some(winner) = round_best else {
            break;
        };
        *best = winner;
    }
}

fn proven_composition_root(source: &[Token]) -> ProvenCompositionState {
    let (literal_frequencies, distance_frequencies) = count_frequencies(source);
    ProvenCompositionState {
        literal_frequencies,
        distance_frequencies,
        extra_bits: token_extra_bits(source),
        estimated_delta: 0,
        choices: [0; PROVEN_COMPOSITION_MAX_TARGETS],
        rewrite_count: 0,
    }
}

fn ranked_forward_proven_composition_moves(
    source: &[Token],
    menus: &[ProvenCompositionMenu],
) -> Option<(ProvenCompositionState, Vec<ProvenCompositionMove>)> {
    let root = proven_composition_root(source);
    let mut ranked = Vec::new();
    ranked.try_reserve_exact(menus.len()).ok()?;
    for (menu_index, menu) in menus.iter().enumerate() {
        let mut selected: Option<(ProvenCompositionStateKey, ProvenCompositionMove)> = None;
        for alternative_index in 0..menu.alternatives.len() {
            let choice = u8::try_from(alternative_index.checked_add(1)?).ok()?;
            let movement = ProvenCompositionMove { menu_index, choice };
            let mut candidate = root.clone();
            if !apply_proven_composition_move(&mut candidate, menus, movement, true) {
                return None;
            }
            let key = proven_composition_state_key(
                &candidate,
                &root.literal_frequencies,
                &root.distance_frequencies,
            );
            if selected
                .as_ref()
                .map_or(true, |(best_key, _)| key < *best_key)
            {
                selected = Some((key, movement));
            }
        }
        if let Some(selected) = selected {
            ranked.push(selected);
        }
    }
    ranked.sort_shared_by_key(|&(key, movement)| (key, movement.menu_index, movement.choice));
    Some((
        root,
        ranked.into_iter().map(|(_, movement)| movement).collect(),
    ))
}

fn ranked_reverse_proven_composition_moves(
    menus: &[ProvenCompositionMenu],
    moves: &[ProvenCompositionMove],
    aggressive: &ProvenCompositionState,
    literal_lengths: &[u8],
    distance_lengths: &[u8],
) -> Option<Vec<ProvenCompositionMove>> {
    let mut ranked = Vec::new();
    ranked.try_reserve_exact(moves.len()).ok()?;
    for &movement in moves {
        let menu = menus.get(movement.menu_index)?;
        let alternative = menu
            .alternatives
            .get(usize::from(movement.choice).checked_sub(1)?)?;
        let source_bits =
            estimated_match_token_bits(menu.source, literal_lengths, distance_lengths)?;
        let replacement_bits =
            estimated_tokens_bits(&alternative.replacement, literal_lengths, distance_lengths)?;
        let mut candidate = aggressive.clone();
        if !apply_proven_composition_move(&mut candidate, menus, movement, false) {
            return None;
        }
        candidate.estimated_delta = i64::try_from(source_bits)
            .ok()?
            .checked_sub(i64::try_from(replacement_bits).ok()?)?;
        let key = proven_composition_state_key(
            &candidate,
            &aggressive.literal_frequencies,
            &aggressive.distance_frequencies,
        );
        ranked.push((key, movement));
    }
    ranked.sort_shared_by_key(|&(key, movement)| (key, movement.menu_index, movement.choice));
    Some(ranked.into_iter().map(|(_, movement)| movement).collect())
}

fn apply_proven_composition_move(
    state: &mut ProvenCompositionState,
    menus: &[ProvenCompositionMenu],
    movement: ProvenCompositionMove,
    forward: bool,
) -> bool {
    let Some(menu) = menus.get(movement.menu_index) else {
        return false;
    };
    let Some(choice_index) = usize::from(movement.choice).checked_sub(1) else {
        return false;
    };
    let Some(alternative) = menu.alternatives.get(choice_index) else {
        return false;
    };
    let Some(&current_choice) = state.choices.get(movement.menu_index) else {
        return false;
    };
    if (forward && current_choice != 0) || (!forward && current_choice != movement.choice) {
        return false;
    }

    let mut updated = state.clone();
    if forward {
        updated.choices[movement.menu_index] = movement.choice;
        updated.rewrite_count = updated.rewrite_count.saturating_add(1);
        let Some(delta) = updated
            .estimated_delta
            .checked_sub(alternative.estimated_saving)
        else {
            return false;
        };
        updated.estimated_delta = delta;
        if !apply_proven_composition_spelling_delta(
            &mut updated,
            std::slice::from_ref(&menu.source),
            &alternative.replacement,
        ) {
            return false;
        }
    } else {
        updated.choices[movement.menu_index] = 0;
        let Some(rewrite_count) = updated.rewrite_count.checked_sub(1) else {
            return false;
        };
        updated.rewrite_count = rewrite_count;
        let Some(delta) = updated
            .estimated_delta
            .checked_add(alternative.estimated_saving)
        else {
            return false;
        };
        updated.estimated_delta = delta;
        if !apply_proven_composition_spelling_delta(
            &mut updated,
            &alternative.replacement,
            std::slice::from_ref(&menu.source),
        ) {
            return false;
        }
    }
    *state = updated;
    true
}

#[allow(clippy::too_many_arguments)]
fn price_proven_composition_state(
    block: &ParsedBlock,
    source: &[Token],
    alignment: u8,
    options: &Options,
    menus: &[ProvenCompositionMenu],
    state: &ProvenCompositionState,
    priced_candidates: &mut Vec<Arc<Vec<Token>>>,
    force_price: bool,
    stop: &mut SearchStop<'_>,
) -> Option<PlannedBlock> {
    let tokens = apply_proven_composition_state(source, block.plain.len(), menus, state)?;
    let duplicate = priced_candidates
        .iter()
        .any(|candidate| same_tokens(candidate, &tokens));
    if duplicate && !force_price {
        return None;
    }
    let candidate = plan_tokens(block, tokens, alignment, options, stop)?;
    if !duplicate && priced_candidates.try_reserve(1).is_ok() {
        priced_candidates.push(Arc::clone(&candidate.tokens));
    }
    Some(candidate)
}

fn build_proven_composition_menus(
    block: &ParsedBlock,
    targets: &[ProvenSubmatchTarget],
    payload_rewrites: &[ProvenSubmatchRewrite],
    symbol_free_rewrites: &[ProvenSubmatchRewrite],
    literal_lengths: &[u8],
    distance_lengths: &[u8],
) -> Option<Vec<ProvenCompositionMenu>> {
    let mut ranked: Vec<&ProvenSubmatchTarget> = Vec::new();
    ranked.try_reserve_exact(targets.len()).ok()?;
    ranked.extend(targets.iter());
    ranked.sort_shared_by_key(|target| target.rank.key());

    let mut menus = Vec::new();
    menus
        .try_reserve_exact(PROVEN_COMPOSITION_MAX_TARGETS)
        .ok()?;
    for target in ranked {
        if menus.len() == PROVEN_COMPOSITION_MAX_TARGETS {
            break;
        }
        let mut alternatives = Vec::new();
        alternatives
            .try_reserve_exact(PROVEN_COMPOSITION_MAX_SPELLINGS - 1)
            .ok()?;
        for family in [payload_rewrites, symbol_free_rewrites] {
            let Some(rewrite) = family
                .iter()
                .find(|rewrite| rewrite.token_index == target.token_index)
            else {
                continue;
            };
            let rewrite = try_clone_proven_submatch_rewrite(rewrite)?;
            insert_distinct_proven_composition_alternative(
                &mut alternatives,
                target.source,
                rewrite,
            );
        }

        let end = target
            .plain_offset
            .checked_add(target.source.decoded_len())?;
        let decoded = block.plain.get(target.plain_offset..end)?;
        let mut literals = new_token_candidate(decoded.len(), decoded.len())?;
        literals.extend(decoded.iter().copied().map(Token::Literal));
        let source_bits =
            estimated_match_token_bits(target.source, literal_lengths, distance_lengths)?;
        let literal_bits = estimated_tokens_bits(&literals, literal_lengths, distance_lengths)?;
        let literal_rewrite = ProvenSubmatchRewrite {
            token_index: target.token_index,
            replacement: literals,
            estimated_saving: i64::try_from(source_bits)
                .ok()?
                .checked_sub(i64::try_from(literal_bits).ok()?)?,
            rank: target.rank,
        };
        insert_distinct_proven_composition_alternative(
            &mut alternatives,
            target.source,
            literal_rewrite,
        );
        alternatives.truncate(PROVEN_COMPOSITION_MAX_SPELLINGS - 1);
        if alternatives.is_empty() {
            continue;
        }
        menus.push(ProvenCompositionMenu {
            token_index: target.token_index,
            source: target.source,
            alternatives,
        });
    }
    menus.sort_shared_by_key(|menu| menu.token_index);
    Some(menus)
}

fn try_clone_proven_submatch_rewrite(
    rewrite: &ProvenSubmatchRewrite,
) -> Option<ProvenSubmatchRewrite> {
    Some(ProvenSubmatchRewrite {
        token_index: rewrite.token_index,
        replacement: try_clone_slice(&rewrite.replacement)?,
        estimated_saving: rewrite.estimated_saving,
        rank: rewrite.rank,
    })
}

fn insert_distinct_proven_composition_alternative(
    alternatives: &mut Vec<ProvenSubmatchRewrite>,
    source: Token,
    candidate: ProvenSubmatchRewrite,
) {
    if same_proven_composition_spelling_cost(std::slice::from_ref(&source), &candidate.replacement)
        || alternatives.iter().any(|existing| {
            same_proven_composition_spelling_cost(&existing.replacement, &candidate.replacement)
        })
    {
        return;
    }
    alternatives.push(candidate);
}

fn same_proven_composition_spelling_cost(left: &[Token], right: &[Token]) -> bool {
    count_frequencies(left) == count_frequencies(right)
        && token_extra_bits(left) == token_extra_bits(right)
}

fn apply_proven_composition_frequency_delta(
    state: &mut ProvenCompositionState,
    source: Token,
    replacement: &[Token],
) -> bool {
    apply_proven_composition_spelling_delta(state, std::slice::from_ref(&source), replacement)
}

fn apply_proven_composition_spelling_delta(
    state: &mut ProvenCompositionState,
    removed: &[Token],
    added: &[Token],
) -> bool {
    for &token in removed {
        if !adjust_proven_composition_token_frequency(
            &mut state.literal_frequencies,
            &mut state.distance_frequencies,
            token,
            false,
        ) {
            return false;
        }
    }
    for &token in added {
        if !adjust_proven_composition_token_frequency(
            &mut state.literal_frequencies,
            &mut state.distance_frequencies,
            token,
            true,
        ) {
            return false;
        }
    }
    let removed_extra = token_extra_bits(removed);
    let added_extra = token_extra_bits(added);
    let Some(extra_bits) = state
        .extra_bits
        .checked_sub(removed_extra)
        .and_then(|bits| bits.checked_add(added_extra))
    else {
        return false;
    };
    state.extra_bits = extra_bits;
    true
}

fn adjust_proven_composition_token_frequency(
    literal_frequencies: &mut [u32; 286],
    distance_frequencies: &mut [u32; 30],
    token: Token,
    add: bool,
) -> bool {
    let (literal_symbol, distance_symbol) = match token {
        Token::Literal(value) => (usize::from(value), None),
        Token::Match {
            length_symbol,
            distance_symbol,
            ..
        } => (
            usize::from(length_symbol),
            Some(usize::from(distance_symbol)),
        ),
    };
    let Some(literal) = literal_frequencies.get_mut(literal_symbol) else {
        return false;
    };
    let Some(updated) = (if add {
        literal.checked_add(1)
    } else {
        literal.checked_sub(1)
    }) else {
        return false;
    };
    *literal = updated;
    if let Some(distance_symbol) = distance_symbol {
        let Some(distance) = distance_frequencies.get_mut(distance_symbol) else {
            return false;
        };
        let Some(updated) = (if add {
            distance.checked_add(1)
        } else {
            distance.checked_sub(1)
        }) else {
            return false;
        };
        *distance = updated;
    }
    true
}

fn same_proven_composition_frequency_state(
    left: &ProvenCompositionState,
    right: &ProvenCompositionState,
) -> bool {
    left.literal_frequencies == right.literal_frequencies
        && left.distance_frequencies == right.distance_frequencies
        && left.extra_bits == right.extra_bits
}

fn proven_composition_state_key(
    state: &ProvenCompositionState,
    source_literal_frequencies: &[u32; 286],
    source_distance_frequencies: &[u32; 30],
) -> (i64, i64, usize, usize, [u8; PROVEN_COMPOSITION_MAX_TARGETS]) {
    let literal_span = state
        .literal_frequencies
        .iter()
        .rposition(|&frequency| frequency != 0)
        .map_or(257, |symbol| symbol + 1)
        .max(257);
    let distance_span = state
        .distance_frequencies
        .iter()
        .rposition(|&frequency| frequency != 0)
        .map_or(1, |symbol| symbol + 1)
        .max(1);
    let source_literal_span = source_literal_frequencies
        .iter()
        .rposition(|&frequency| frequency != 0)
        .map_or(257, |symbol| symbol + 1)
        .max(257);
    let source_distance_span = source_distance_frequencies
        .iter()
        .rposition(|&frequency| frequency != 0)
        .map_or(1, |symbol| symbol + 1)
        .max(1);
    let span_reduction = source_literal_span
        .saturating_sub(literal_span)
        .saturating_add(source_distance_span.saturating_sub(distance_span));
    let rare_removed = source_literal_frequencies
        .iter()
        .zip(&state.literal_frequencies)
        .chain(
            source_distance_frequencies
                .iter()
                .zip(&state.distance_frequencies),
        )
        .filter(|(source, current)| **source != 0 && **source <= 2 && **current == 0)
        .count();
    let active_symbols = state
        .literal_frequencies
        .iter()
        .chain(&state.distance_frequencies)
        .filter(|&&frequency| frequency != 0)
        .count();
    let header_credit = span_reduction
        .saturating_mul(2)
        .saturating_add(rare_removed.saturating_mul(2));
    let adjusted = state
        .estimated_delta
        .saturating_sub(i64::try_from(header_credit).unwrap_or(i64::MAX));
    (
        adjusted,
        state.estimated_delta,
        literal_span.saturating_add(distance_span),
        active_symbols,
        state.choices,
    )
}

fn apply_proven_composition_state(
    source: &[Token],
    decoded_bytes: usize,
    menus: &[ProvenCompositionMenu],
    state: &ProvenCompositionState,
) -> Option<Vec<Token>> {
    let mut output_count = source.len();
    for (menu_index, menu) in menus.iter().enumerate() {
        let choice = usize::from(*state.choices.get(menu_index)?);
        if choice == 0 {
            continue;
        }
        let alternative = menu.alternatives.get(choice - 1)?;
        output_count = output_count
            .checked_sub(1)?
            .checked_add(alternative.replacement.len())?;
    }
    let mut output = new_token_candidate(output_count, decoded_bytes)?;
    let mut menu_index = 0_usize;
    for (token_index, &token) in source.iter().enumerate() {
        if menus
            .get(menu_index)
            .is_some_and(|menu| menu.token_index == token_index)
        {
            let menu = &menus[menu_index];
            if token != menu.source {
                return None;
            }
            let choice = usize::from(state.choices[menu_index]);
            if choice == 0 {
                output.push(token);
            } else {
                output.extend_from_slice(&menu.alternatives.get(choice - 1)?.replacement);
            }
            menu_index += 1;
        } else {
            output.push(token);
        }
    }
    (menu_index == menus.len()).then_some(output)
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
pub(super) fn solve_proven_submatch_avoiding(
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
        length_extra_bits,
        distance_extra_bits,
        ..
    } = token
    else {
        return None;
    };
    estimated_length(literal_lengths, usize::from(length_symbol))
        .checked_add(u64::from(length_extra_bits))?
        .checked_add(estimated_length(
            distance_lengths,
            usize::from(distance_symbol),
        ))?
        .checked_add(u64::from(distance_extra_bits))
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

/// Blocks with at least this many tokens get R17's extended floor
/// unconditionally; smaller ones only when a match costs more than its
/// literals under their current codes.
const TOKEN_REPLAY_FULL_MIN_TOKENS: usize = 4096;

/// R17 for one block: replay the extended floor from the block's final codes.
/// R13–R16 re-cut, merge and re-tree blocks after the main planner chose their
/// spellings, so the floor's match expansions and feedback trees can find a
/// cheaper spelling for the block as it now stands. Large blocks always get
/// the floor, whose grouped expansions refit their trees; a smaller block gets
/// it only when one of its matches costs more than its literals. Returns the
/// new plan when it is strictly smaller than the block's current bits.
pub(crate) fn plan_terminal_token_replay(
    block: &ParsedBlock,
    alignment: u8,
    options: &Options,
    stop: &mut SearchStop<'_>,
) -> Option<PlannedBlock> {
    if stop.reached()
        || block.tokens.len() > MANDATORY_FLOOR_MAX_TOKENS
        || block.plain.len() > MANDATORY_FLOOR_MAX_PLAIN
    {
        return None;
    }
    let codes = block.original_dynamic.as_ref()?;
    if block.tokens.len() < TOKEN_REPLAY_FULL_MIN_TOKENS
        && !has_dearer_match(
            &block.tokens,
            &block.plain,
            &codes.literal_lengths,
            &codes.distance_lengths,
        )
    {
        return None;
    }
    let current = reusable_original_bits(block, alignment, options.strict)?.len;
    let plan = plan_block_with_floor(block, alignment, options, true);
    (!matches!(plan.representation, Representation::Stored) && plan.bits < current).then_some(plan)
}

/// Plan a block with the cheap floor: same-distance runs and match-to-literal
/// expansions, plus the feedback-tree and replay floor if `extended`. It runs
/// even after a container's shared search deadline, so every ZIP member or
/// APNG frame gets one useful pass.
pub(crate) fn plan_block_with_floor(
    block: &ParsedBlock,
    alignment: u8,
    options: &Options,
    extended: bool,
) -> PlannedBlock {
    let mut floor_options = options.clone();
    floor_options.exhaustive = false;
    let base = plan_block(block, alignment, &floor_options, &mut SearchStop::never());
    improve_plan_with_floor(block, alignment, &floor_options, extended, base)
}

/// Apply the cheap floor to an already-priced base plan. Comparisons are
/// strict: the earlier candidate wins a tie.
pub(crate) fn improve_plan_with_floor(
    block: &ParsedBlock,
    alignment: u8,
    options: &Options,
    extended: bool,
    best: PlannedBlock,
) -> PlannedBlock {
    improve_plan_with_floor_policy(block, alignment, options, extended, false, best)
}

/// Run the cheap floor with proven-submatch resegmentation inside it, for when
/// a preceding same-distance repartition makes the two dependent. Callers keep
/// it as a separate lineage.
pub(crate) fn improve_plan_with_integrated_proven_floor(
    block: &ParsedBlock,
    alignment: u8,
    options: &Options,
    extended: bool,
    best: PlannedBlock,
) -> PlannedBlock {
    improve_plan_with_floor_policy(block, alignment, options, extended, true, best)
}

fn improve_plan_with_floor_policy(
    block: &ParsedBlock,
    alignment: u8,
    options: &Options,
    extended: bool,
    include_proven: bool,
    mut best: PlannedBlock,
) -> PlannedBlock {
    let mut floor_options = options.clone();
    floor_options.exhaustive = false;

    if block.tokens.len() > MANDATORY_FLOOR_MAX_TOKENS
        || block.plain.len() > MANDATORY_FLOOR_MAX_PLAIN
    {
        return best;
    }
    let match_count = block
        .tokens
        .iter()
        .filter(|token| matches!(token, Token::Match { .. }))
        .count();
    if match_count == 0 {
        return best;
    }

    let mut never_expired = SearchStop::never();
    consider_same_distance_runs(
        block,
        alignment,
        &floor_options,
        &mut never_expired,
        &mut best,
    );
    if include_proven {
        consider_proven_submatches(
            block,
            alignment,
            &floor_options,
            &mut never_expired,
            &mut best,
        );
    }
    if let Some(normalized) = rewrite_258_symbols(&block.tokens, block.plain.len(), false) {
        if !same_tokens(&normalized, &block.tokens) {
            consider_tokens(
                block,
                normalized,
                alignment,
                &floor_options,
                &mut never_expired,
                &mut best,
            );
        }
    }
    if !options.strict {
        if let Some(aliased) = rewrite_258_symbols(&block.tokens, block.plain.len(), true) {
            if !same_tokens(&aliased, &block.tokens) {
                consider_tokens(
                    block,
                    aliased,
                    alignment,
                    &floor_options,
                    &mut never_expired,
                    &mut best,
                );
            }
        }
    }

    let mut seeds = Vec::new();
    if let (Some(literal), Some(distance)) = (
        block.original_literal_lengths.as_ref(),
        block.original_distance_lengths.as_ref(),
    ) {
        seeds.push((literal.to_vec(), distance.to_vec()));
    }
    if let Some(lengths) = plan_lengths(&best) {
        seeds.push(lengths);
    }
    seeds.push(fixed_lengths());
    deduplicate_seeds(&mut seeds);
    for (literal, distance) in seeds {
        if let Some(tokens) =
            expand_matches(&block.tokens, &block.plain, &literal, &distance, false)
        {
            consider_tokens(
                block,
                tokens,
                alignment,
                &floor_options,
                &mut never_expired,
                &mut best,
            );
        }
    }

    if all_literal_endpoint_is_bounded(block.plain.len(), match_count as u64) {
        consider_all_literals(block, &floor_options, &mut never_expired, &mut best);
    }

    // The rest is only for compact block lists; on every large image block it
    // would exhaust the caller's budget before grouping.
    if !extended {
        return best;
    }
    consider_compact_short_bands(
        block,
        alignment,
        &floor_options,
        &mut never_expired,
        &mut best,
    );
    consider_deft4j_trees(
        block,
        alignment,
        &floor_options,
        &mut never_expired,
        &mut best,
        COLUMBO_SINGLE_MATCH_TRIALS,
    );
    let feedback_seeds = feedback_tree_seeds(block, options.strict);
    consider_feedback_seed_trees(block, &floor_options, &feedback_seeds, &mut best);
    consider_columbo_defluff_derived_rescan(
        block,
        alignment,
        &floor_options,
        &feedback_seeds,
        &mut never_expired,
        &mut best,
    );

    // Replay once from the winner, under the same limits, to carry a strict
    // intermediate expansion into a finished adjacent merge and repack its
    // header.
    if !same_tokens(&best.tokens, &block.tokens) {
        if let Some(replay_tokens) = try_clone_token_candidate(&best.tokens, block.plain.len()) {
            if let Some(replay_block) = try_transformed_block(block, replay_tokens) {
                let mut replay = plan_block(
                    &replay_block,
                    alignment,
                    &floor_options,
                    &mut SearchStop::never(),
                );
                let feedback_seeds = feedback_tree_seeds(&replay_block, options.strict);
                consider_feedback_seed_trees(
                    &replay_block,
                    &floor_options,
                    &feedback_seeds,
                    &mut replay,
                );
                consider_deft4j_trees(
                    &replay_block,
                    alignment,
                    &floor_options,
                    &mut never_expired,
                    &mut replay,
                    0,
                );
                consider_columbo_defluff_derived_rescan(
                    &replay_block,
                    alignment,
                    &floor_options,
                    &feedback_seeds,
                    &mut never_expired,
                    &mut replay,
                );
                if replay.bits < best.bits {
                    best = replay;
                }
            }
        }
    }
    best
}

/// Apply deft4j's tree builder and its two whole-block recodes to an
/// already-priced block, without the ranked single-match trials. The work is
/// deadline-independent.
pub(crate) fn improve_plan_with_deft4j_tree_floor(
    block: &ParsedBlock,
    alignment: u8,
    options: &Options,
    mut best: PlannedBlock,
) -> PlannedBlock {
    let mut floor_options = options.clone();
    floor_options.exhaustive = false;
    if block.tokens.len() > MANDATORY_FLOOR_MAX_TOKENS
        || block.plain.len() > MANDATORY_FLOOR_MAX_PLAIN
    {
        return best;
    }

    consider_deft4j_trees(
        block,
        alignment,
        &floor_options,
        &mut SearchStop::never(),
        &mut best,
        0,
    );
    best
}

/// For each end in `COMPACT_SHORT_BAND_ENDS`, expand every match whose length
/// symbol is in 257..=end; inspired by deft4j's least-family pruning.
fn consider_compact_short_bands(
    block: &ParsedBlock,
    alignment: u8,
    options: &Options,
    stop: &mut SearchStop<'_>,
    best: &mut PlannedBlock,
) {
    if block.tokens.len() > COMPACT_SHORT_BAND_MAX_TOKENS
        || block.plain.len() > COMPACT_SHORT_BAND_MAX_PLAIN
    {
        return;
    }

    for last_symbol in COMPACT_SHORT_BAND_ENDS {
        if stop.reached() {
            return;
        }
        let Some(tokens) = expand_selected_matches(&block.tokens, &block.plain, |_, token, _| {
            matches!(
                token,
                Token::Match { length_symbol, .. }
                    if (257..=last_symbol).contains(&length_symbol)
            )
        }) else {
            continue;
        };
        consider_tokens(block, tokens, alignment, options, stop, best);
    }
}

/// Apply a bounded feedback and replay pass to a finished Huffman plan. A
/// cheaper block layout can reach the deadline before its last blocks get
/// optional feedback, so the selected blocks get this pass.
pub(crate) fn tighten_terminal_plan(plan: &mut PlannedBlock, options: &Options) {
    if plan.tokens.len() > MANDATORY_FLOOR_MAX_TOKENS
        || plan.plain.len() > MANDATORY_FLOOR_MAX_PLAIN
        || (plan.tokens.is_empty() && !plan.plain.is_empty())
    {
        return;
    }
    let (literal_frequencies, distance_frequencies) = count_frequencies(&plan.tokens);
    let block = ParsedBlock {
        tokens: plan.tokens.clone(),
        plain: plan.plain.clone(),
        literal_frequencies,
        distance_frequencies,
        original_literal_lengths: None,
        original_distance_lengths: None,
        original_dynamic: None,
        original: None,
        source_splits: Vec::new(),
        source_type: plan.source_type,
    };
    let mut floor_options = options.clone();
    floor_options.exhaustive = false;
    let seeds = feedback_tree_seeds(&block, options.strict);
    consider_feedback_seed_trees(&block, &floor_options, &seeds, plan);

    // DeflOpt's strict table replay: expand strictly dearer matches, then
    // rebuild once.
    let Some((literal_lengths, distance_lengths)) = plan_lengths(plan) else {
        return;
    };
    let Some(tokens) = expand_matches(
        &block.tokens,
        &block.plain,
        &literal_lengths,
        &distance_lengths,
        false,
    ) else {
        return;
    };
    let Some(transformed) = try_transformed_block(&block, tokens) else {
        return;
    };
    // Huffman costs do not depend on the bit alignment. Ignore a stored result:
    // its padding was priced at the dummy alignment, and the stream planner
    // already kept the valid stored form.
    let mut candidate = plan_block(&transformed, 0, &floor_options, &mut SearchStop::never());
    let seeds = feedback_tree_seeds(&transformed, options.strict);
    consider_feedback_seed_trees(&transformed, &floor_options, &seeds, &mut candidate);
    if !matches!(candidate.representation, Representation::Stored) && candidate.bits < plan.bits {
        *plan = candidate;
    }
}

/// Replay the extended cheap floor once from an already-selected plan, with its
/// current codes as the source codes.
///
/// Rebuilt codes can make more matches strictly dearer than their literals, a
/// state container streams may not reach before their shared deadline.
pub(crate) fn replay_extended_floor(
    plan: &PlannedBlock,
    alignment: u8,
    options: &Options,
) -> Option<PlannedBlock> {
    if plan.tokens.len() > MANDATORY_FLOOR_MAX_TOKENS
        || plan.plain.len() > MANDATORY_FLOOR_MAX_PLAIN
        || matches!(
            plan.representation,
            Representation::Stored | Representation::Original(_)
        )
    {
        return None;
    }

    let block = parsed_block_from_plan(plan)?;
    let replay = plan_block_with_floor(&block, alignment, options, true);
    (!matches!(replay.representation, Representation::Stored) && replay.bits < plan.bits)
        .then_some(replay)
}

/// Follow the selected Huffman codes through a short fixed-point ladder of
/// expand-and-rebuild passes; a pass that does not win can still lead to a
/// smaller next pass. `TABLE_REPLAY_PASSES` was chosen from measurements.
pub(crate) fn replay_table_ladder(
    plan: &PlannedBlock,
    alignment: u8,
    options: &Options,
) -> Option<PlannedBlock> {
    if plan.tokens.len() > TABLE_REPLAY_MAX_TOKENS || plan.plain.len() > MANDATORY_FLOOR_MAX_PLAIN {
        return None;
    }

    let block = parsed_block_from_plan(plan)?;
    let (mut literal_lengths, mut distance_lengths) = plan_lengths(plan)?;
    let mut seed_tokens = plan.tokens.clone();
    let mut best = None;
    let mut best_bits = plan.bits;
    let mut floor_options = options.clone();
    floor_options.exhaustive = false;

    for _ in 0..TABLE_REPLAY_PASSES {
        let Some(tokens) = expand_matches_with_token_limit(
            &seed_tokens,
            &block.plain,
            &literal_lengths,
            &distance_lengths,
            true,
            TABLE_REPLAY_MAX_TOKENS,
        ) else {
            break;
        };
        let Some(candidate) = plan_tokens(
            &block,
            tokens,
            alignment,
            &floor_options,
            &mut SearchStop::never(),
        ) else {
            break;
        };
        let Some((next_literal, next_distance)) = plan_lengths(&candidate) else {
            break;
        };
        let next_tokens = candidate.tokens.clone();

        if candidate.bits < best_bits {
            best_bits = candidate.bits;
            best = Some(candidate);
        }
        seed_tokens = next_tokens;
        literal_lengths = next_literal;
        distance_lengths = next_distance;
    }
    best
}

/// Rebuild the parsed metadata for a selected plan, with its current codes as
/// the source codes.
fn parsed_block_from_plan(plan: &PlannedBlock) -> Option<ParsedBlock> {
    let (literal_lengths, distance_lengths) = plan_lengths(plan)?;
    let mut original_literal_lengths = [0_u8; 286];
    let mut original_distance_lengths = [0_u8; 30];
    let literal_count = literal_lengths.len().min(original_literal_lengths.len());
    let distance_count = distance_lengths.len().min(original_distance_lengths.len());
    original_literal_lengths[..literal_count].copy_from_slice(&literal_lengths[..literal_count]);
    original_distance_lengths[..distance_count]
        .copy_from_slice(&distance_lengths[..distance_count]);
    let original_dynamic = match &plan.representation {
        Representation::Dynamic(dynamic) => Some(dynamic.try_clone()?),
        Representation::Fixed => None,
        Representation::Stored | Representation::Original(_) => return None,
    };
    let (literal_frequencies, distance_frequencies) = count_frequencies(&plan.tokens);
    Some(ParsedBlock {
        tokens: plan.tokens.clone(),
        plain: plan.plain.clone(),
        literal_frequencies,
        distance_frequencies,
        original_literal_lengths: Some(original_literal_lengths),
        original_distance_lengths: Some(original_distance_lengths),
        original_dynamic,
        original: None,
        source_splits: Vec::new(),
        source_type: plan.source_type,
    })
}

/// Try the five cumulative short-length bands 260..=260 through 260..=264
/// (lengths 6 to 10).
pub(crate) fn improve_plan_with_short_family_floor(
    block: &ParsedBlock,
    options: &Options,
    mut best: PlannedBlock,
) -> PlannedBlock {
    let mut floor_options = options.clone();
    floor_options.exhaustive = false;
    if block.tokens.len() > SHORT_FAMILY_MAX_TOKENS || block.plain.len() > SHORT_FAMILY_MAX_PLAIN {
        return best;
    }

    for last_symbol in 260..=264 {
        let Some(tokens) =
            expand_selected_matches(&block.tokens, &block.plain, |_, token, _| match token {
                Token::Match { length_symbol, .. } => (260..=last_symbol).contains(&length_symbol),
                Token::Literal(_) => false,
            })
        else {
            continue;
        };
        consider_short_family_tokens(block, tokens, &floor_options, &mut best);
    }
    best
}

/// Frequency changes from expanding the matches of each length symbol
/// 260..=264. The summary is additive, so stream grouping can score many ranges
/// without rescanning their tokens.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ShortFamilyStats {
    literal_additions: [[u32; 256]; 5],
    distance_removals: [[u32; 30]; 5],
    match_removals: [u32; 5],
    extra_bits_removed: [u64; 5],
}

impl ShortFamilyStats {
    pub(crate) fn from_block(block: &ParsedBlock) -> Option<Self> {
        let mut stats = Self {
            literal_additions: [[0; 256]; 5],
            distance_removals: [[0; 30]; 5],
            match_removals: [0; 5],
            extra_bits_removed: [0; 5],
        };
        let mut plain_offset = 0_usize;
        for &token in block.tokens.iter() {
            let end = plain_offset.checked_add(token.decoded_len())?;
            let decoded = block.plain.get(plain_offset..end)?;
            if let Token::Match {
                length_symbol,
                distance_symbol,
                length_extra_bits,
                distance_extra_bits,
                ..
            } = token
            {
                if (260..=264).contains(&length_symbol) {
                    let family = usize::from(length_symbol - 260);
                    stats.match_removals[family] = stats.match_removals[family].checked_add(1)?;
                    let frequency =
                        stats.distance_removals[family].get_mut(usize::from(distance_symbol))?;
                    *frequency = frequency.checked_add(1)?;
                    stats.extra_bits_removed[family] = stats.extra_bits_removed[family]
                        .checked_add(
                            u64::from(length_extra_bits) + u64::from(distance_extra_bits),
                        )?;
                    for &byte in decoded {
                        let frequency = &mut stats.literal_additions[family][usize::from(byte)];
                        *frequency = frequency.checked_add(1)?;
                    }
                }
            }
            plain_offset = end;
        }
        (plain_offset == block.plain.len()).then_some(stats)
    }

    pub(crate) fn add_assign(&mut self, other: &Self) -> Option<()> {
        for family in 0..5 {
            self.match_removals[family] =
                self.match_removals[family].checked_add(other.match_removals[family])?;
            self.extra_bits_removed[family] =
                self.extra_bits_removed[family].checked_add(other.extra_bits_removed[family])?;
            for symbol in 0..256 {
                self.literal_additions[family][symbol] = self.literal_additions[family][symbol]
                    .checked_add(other.literal_additions[family][symbol])?;
            }
            for symbol in 0..30 {
                self.distance_removals[family][symbol] = self.distance_removals[family][symbol]
                    .checked_add(other.distance_removals[family][symbol])?;
            }
        }
        Some(())
    }
}

pub(crate) fn score_short_family_frequencies(
    literal_frequencies: &[u32; 286],
    distance_frequencies: &[u32; 30],
    extra_bits: u64,
    stats: &ShortFamilyStats,
    min_distance_codes: bool,
) -> Option<u64> {
    let mut literal = *literal_frequencies;
    let mut distance = *distance_frequencies;
    let mut remaining_extra_bits = extra_bits;
    let mut best = None;
    let mut literal_lengths = [0_u8; 286];
    let mut distance_lengths = [0_u8; 30];
    let mut heap_scratch = DefloptHeapScratch::default();

    // Stage 0 is the unchanged token stream; each later stage also expands the
    // next short-length symbol, matching the five token candidates built after
    // segmentation.
    for stage in 0..=5 {
        if stage != 0 {
            let family = stage - 1;
            if stats.match_removals[family] == 0 {
                continue;
            }
            let symbol = 260 + family;
            literal[symbol] = literal[symbol].checked_sub(stats.match_removals[family])?;
            for (frequency, &addition) in literal[..256]
                .iter_mut()
                .zip(&stats.literal_additions[family])
            {
                *frequency = frequency.checked_add(addition)?;
            }
            for (frequency, &removal) in distance.iter_mut().zip(&stats.distance_removals[family]) {
                *frequency = frequency.checked_sub(removal)?;
            }
            remaining_extra_bits =
                remaining_extra_bits.checked_sub(stats.extra_bits_removed[family])?;
        }

        let mut build_distance = distance;
        ensure_floor_distance_symbols(&mut build_distance, min_distance_codes);
        for variant in 0..4 {
            make_lengths_deflopt_heap_into_with_scratch(
                &literal,
                &mut literal_lengths,
                15,
                variant,
                &mut heap_scratch,
            );
            make_lengths_deflopt_heap_into_with_scratch(
                &build_distance,
                &mut distance_lengths,
                15,
                variant,
                &mut heap_scratch,
            );
            let Some(data_bits) = token_bits_from_frequencies(
                &literal,
                &distance,
                &literal_lengths,
                &distance_lengths,
                remaining_extra_bits,
            ) else {
                continue;
            };
            let Some(plan) = plan_for_explicit_lengths_with_cost(
                &literal_lengths,
                &distance_lengths,
                data_bits,
                false,
            ) else {
                continue;
            };
            if best.map_or(true, |bits| plan.bits < bits) {
                best = Some(plan.bits);
            }
        }
    }
    best
}

fn consider_short_family_tokens(
    source: &ParsedBlock,
    tokens: Vec<Token>,
    options: &Options,
    best: &mut PlannedBlock,
) {
    let (literal_frequencies, mut distance_frequencies) = count_frequencies(&tokens);
    ensure_floor_distance_symbols(&mut distance_frequencies, options.strict);
    let mut best_dynamic = None;
    let mut literal_lengths = [0_u8; 286];
    let mut distance_lengths = [0_u8; 30];
    let mut heap_scratch = DefloptHeapScratch::default();
    for variant in 0..4 {
        make_lengths_deflopt_heap_into_with_scratch(
            &literal_frequencies,
            &mut literal_lengths,
            15,
            variant,
            &mut heap_scratch,
        );
        make_lengths_deflopt_heap_into_with_scratch(
            &distance_frequencies,
            &mut distance_lengths,
            15,
            variant,
            &mut heap_scratch,
        );
        let Some(dynamic) = plan_for_explicit_lengths(
            &tokens,
            &literal_lengths,
            &distance_lengths,
            options.exhaustive,
        ) else {
            continue;
        };
        if best_dynamic
            .as_ref()
            .map_or(true, |current: &DynamicPlan| dynamic.bits < current.bits)
        {
            best_dynamic = Some(dynamic);
        }
    }

    let Some(dynamic) = best_dynamic else {
        return;
    };
    if dynamic.bits < best.bits {
        let bits = dynamic.bits;
        best.tokens = tokens.into();
        best.plain = source.plain.clone();
        best.representation = Representation::Dynamic(dynamic);
        best.bits = bits;
        best.source_type = source.source_type;
    }
}

fn ensure_floor_distance_symbols(frequencies: &mut [u32; 30], min_distance_codes: bool) {
    if !min_distance_codes {
        return;
    }
    let used = frequencies
        .iter()
        .filter(|&&frequency| frequency != 0)
        .count();
    if used == 0 {
        frequencies[0] = 1;
        frequencies[1] = 1;
    } else if used == 1 {
        let only_zero = frequencies[0] != 0;
        frequencies[usize::from(only_zero)] = 1;
    }
}

fn consider_deft4j_trees(
    block: &ParsedBlock,
    alignment: u8,
    options: &Options,
    stop: &mut SearchStop<'_>,
    best: &mut PlannedBlock,
    individual_trial_limit: usize,
) {
    let (literal, distance) = deft4j_heap_lengths(
        &block.literal_frequencies,
        &block.distance_frequencies,
        options.strict,
    );
    if let Some(dynamic) =
        plan_for_explicit_lengths(&block.tokens, &literal, &distance, options.exhaustive)
    {
        if dynamic.bits < best.bits {
            let bits = dynamic.bits;
            best.representation = Representation::Dynamic(dynamic);
            best.bits = bits;
            best.tokens = block.tokens.clone();
            best.plain = block.plain.clone();
            best.source_type = block.source_type;
        }
    }

    // deft4j's recode prunes matches under these trees, then rebuilds. Try both
    // the strict and the tie-including forms: a tie changes the next
    // frequencies.
    let strict = expand_matches(&block.tokens, &block.plain, &literal, &distance, false);
    let non_larger = expand_matches(&block.tokens, &block.plain, &literal, &distance, true);
    if let Some(tokens) = strict {
        let duplicate = non_larger
            .as_ref()
            .is_some_and(|other| same_tokens(other, &tokens));
        consider_deft4j_rebuild(block, tokens, alignment, options, stop, best);
        if !duplicate {
            if let Some(tokens) = non_larger {
                consider_deft4j_rebuild(block, tokens, alignment, options, stop, best);
            }
        }
    } else if let Some(tokens) = non_larger {
        consider_deft4j_rebuild(block, tokens, alignment, options, stop, best);
    }
    if individual_trial_limit != 0 {
        individual_prune_from_lengths(
            block,
            alignment,
            options,
            stop,
            best,
            &literal,
            &distance,
            individual_trial_limit,
        );
    }
}

/// deft4j's heap code lengths for these counts, with the distance counts
/// padded as the floor pads them.
// Out of line: each call builds two codes.
#[inline(never)]
fn deft4j_heap_lengths(
    literal_frequencies: &[u32; 286],
    distance_frequencies: &[u32; 30],
    strict: bool,
) -> (Vec<u8>, Vec<u8>) {
    let mut distance_frequencies = *distance_frequencies;
    ensure_floor_distance_symbols(&mut distance_frequencies, strict);
    // Emulate OpenJDK `PriorityQueue` heap order: deft4j's comparator has no
    // tie key, so equal weights follow the reference heap.
    let literal = make_lengths_deft4j_java_heap(literal_frequencies, 15);
    // Unlike deft4j, also use it for zero or one used distance symbol.
    let distance = make_lengths_deft4j_java_heap(&distance_frequencies, 15);
    (literal, distance)
}

/// Rebuild a deft4j-pruned token state with Columbo's ordinary planner and
/// with deft4j's deterministic heap, keeping whichever complete block wins.
fn consider_deft4j_rebuild(
    source: &ParsedBlock,
    tokens: Vec<Token>,
    alignment: u8,
    options: &Options,
    stop: &mut SearchStop<'_>,
    best: &mut PlannedBlock,
) {
    if let Some(planned) = deft4j_rebuild(source, tokens, alignment, options, stop) {
        if planned.bits < best.bits {
            *best = planned;
        }
    }
}

/// The smaller of a token state planned by Columbo's ordinary planner and by
/// deft4j's deterministic heap.
fn deft4j_rebuild(
    source: &ParsedBlock,
    tokens: Vec<Token>,
    alignment: u8,
    options: &Options,
    stop: &mut SearchStop<'_>,
) -> Option<PlannedBlock> {
    let candidate = try_transformed_block(source, tokens)?;
    let (literal, distance) = deft4j_heap_lengths(
        &candidate.literal_frequencies,
        &candidate.distance_frequencies,
        options.strict,
    );
    let deft4j =
        plan_for_explicit_lengths(&candidate.tokens, &literal, &distance, options.exhaustive);

    let mut planned = plan_owned_block(candidate, alignment, options, stop);
    if let Some(deft4j) = deft4j {
        if deft4j.bits < planned.bits {
            planned.bits = deft4j.bits;
            planned.representation = Representation::Dynamic(deft4j);
        }
    }
    Some(planned)
}

/// D4's length-family chain for one block, after deft4j's least-expensive
/// family transform. Expanding each match whose literals cost no more under
/// the current codes stops once every remaining match is cheaper alone, but
/// expanding every match of one length symbol drops that symbol from the
/// alphabet, and codes rebuilt from the new counts can make further matches
/// dearer than their literals. The chain first prunes from the block's own
/// codes, then up to [`LENGTH_FAMILY_STEPS`] times expands the length symbol
/// whose matches cost least as literals in total. Each step is followed by a
/// prune chain: rebuild deft4j's heap codes from the counts, then expand every
/// match no cheaper than its literals, at most [`PRUNE_CHAIN_PASSES`] times.
/// Every state is planned exactly; the smallest plan is returned if it is
/// strictly below `bound`.
pub(crate) fn plan_length_family_chain(
    block: &ParsedBlock,
    alignment: u8,
    options: &Options,
    bound: u64,
    stop: &mut SearchStop<'_>,
) -> Option<PlannedBlock> {
    let (mut literal, mut distance) = match &block.original_dynamic {
        Some(codes) => (
            codes.literal_lengths.clone(),
            codes.distance_lengths.clone(),
        ),
        None => deft4j_heap_lengths(
            &block.literal_frequencies,
            &block.distance_frequencies,
            options.strict,
        ),
    };
    let mut tokens = try_clone_token_candidate(&block.tokens, block.plain.len())?;
    let mut best: Option<PlannedBlock> = None;
    for step in 0..=LENGTH_FAMILY_STEPS {
        if stop.reached() {
            break;
        }
        let expanded = if step == 0 {
            expand_matches(&tokens, &block.plain, &literal, &distance, true)
        } else {
            let Some(symbol) =
                least_expensive_length_symbol(&tokens, &block.plain, &literal, &distance)
            else {
                break;
            };
            expand_selected_matches_with_limit(
                &tokens,
                &block.plain,
                MAX_TOKEN_CANDIDATE_BYTES,
                |_, token, _| matches!(token, Token::Match { length_symbol, .. } if length_symbol == symbol),
            )
        };
        let Some(expanded) = expanded else {
            if step == 0 {
                continue;
            }
            break;
        };
        tokens = expanded;
        for _ in 0..PRUNE_CHAIN_PASSES {
            let (literal_frequencies, distance_frequencies) = count_frequencies(&tokens);
            (literal, distance) =
                deft4j_heap_lengths(&literal_frequencies, &distance_frequencies, options.strict);
            let state = try_clone_token_candidate(&tokens, block.plain.len())?;
            if let Some(plan) = deft4j_rebuild(block, state, alignment, options, stop) {
                if plan.bits < best.as_ref().map_or(bound, |best| best.bits) {
                    best = Some(plan);
                }
            }
            if stop.reached() {
                return best;
            }
            let Some(expanded) = expand_matches(&tokens, &block.plain, &literal, &distance, true)
            else {
                break;
            };
            tokens = expanded;
        }
    }
    best
}

/// D4's re-spell from literals for one block. Starting from deft4j's heap
/// codes for the block's bytes as literals, where no match has a code, each
/// pass keeps exactly the source matches that cost less than their literals
/// under the current codes, then rebuilds the codes from the new counts, until
/// the spelling repeats or after [`PRUNE_CHAIN_PASSES`] passes. Unlike the
/// prune chains, a match dropped in one pass can return in the next. Starting
/// from literals keeps only matches that clearly pay before their codes shape
/// the tree, which can reach a smaller spelling than pruning down from the
/// source's matches. Every state is planned exactly; the smallest plan is
/// returned if it is strictly below `bound`.
pub(crate) fn plan_respell_from_literals(
    block: &ParsedBlock,
    alignment: u8,
    options: &Options,
    bound: u64,
    stop: &mut SearchStop<'_>,
) -> Option<PlannedBlock> {
    if block
        .distance_frequencies
        .iter()
        .all(|&frequency| frequency == 0)
    {
        return None;
    }
    let mut byte_frequencies = [0_u32; 286];
    for &byte in block.plain.iter() {
        byte_frequencies[usize::from(byte)] += 1;
    }
    byte_frequencies[256] += 1;
    let (mut literal, _) = deft4j_heap_lengths(&byte_frequencies, &[0; 30], options.strict);
    let mut distance = vec![0_u8; 30];
    let mut previous: Option<Vec<Token>> = None;
    let mut best: Option<PlannedBlock> = None;
    for _ in 0..PRUNE_CHAIN_PASSES {
        if stop.reached() {
            break;
        }
        let tokens = match expand_matches(&block.tokens, &block.plain, &literal, &distance, true) {
            Some(tokens) => tokens,
            None => try_clone_token_candidate(&block.tokens, block.plain.len())?,
        };
        if previous.as_ref() == Some(&tokens) {
            break;
        }
        let (literal_frequencies, distance_frequencies) = count_frequencies(&tokens);
        let state = try_clone_token_candidate(&tokens, block.plain.len())?;
        if let Some(plan) = deft4j_rebuild(block, state, alignment, options, stop) {
            if plan.bits < best.as_ref().map_or(bound, |best| best.bits) {
                best = Some(plan);
            }
        }
        (literal, distance) =
            deft4j_heap_lengths(&literal_frequencies, &distance_frequencies, options.strict);
        previous = Some(tokens);
    }
    best
}

/// The length symbol whose matches together cost least, or save most, when
/// spelled as literals under these codes, priced as the table replay prices
/// them; ties keep the lowest symbol.
fn least_expensive_length_symbol(
    tokens: &[Token],
    plain: &[u8],
    literal_lengths: &[u8],
    distance_lengths: &[u8],
) -> Option<u16> {
    let mut deltas = [0_i64; 29];
    let mut counts = [0_u32; 29];
    let mut offset = 0_usize;
    for &token in tokens {
        let end = offset.checked_add(token.decoded_len())?;
        if let Token::Match {
            length_symbol,
            distance_symbol,
            length_extra_bits,
            distance_extra_bits,
            ..
        } = token
        {
            let family = usize::from(length_symbol.checked_sub(257)?);
            let literal_bits: u64 = plain
                .get(offset..end)?
                .iter()
                .map(|&byte| estimated_length(literal_lengths, usize::from(byte)))
                .sum();
            let match_bits = estimated_length(literal_lengths, usize::from(length_symbol))
                + u64::from(length_extra_bits)
                + estimated_length(distance_lengths, usize::from(distance_symbol))
                + u64::from(distance_extra_bits);
            *deltas.get_mut(family)? += literal_bits as i64 - match_bits as i64;
            counts[family] += 1;
        }
        offset = end;
    }
    (0..29)
        .filter(|&family| counts[family] != 0)
        .min_by_key(|&family| deltas[family])
        .map(|family| family as u16 + 257)
}

pub(crate) fn plan_block_with_search(
    block: &ParsedBlock,
    alignment: u8,
    options: &Options,
    stop: &mut SearchStop<'_>,
) -> PlannedBlock {
    plan_block_with_search_policy(
        block,
        alignment,
        options,
        SearchPolicy::FULL,
        SearchBase::Price,
        stop,
    )
}

/// Run the full token search from an already priced ordinary plan.
pub(crate) fn plan_block_with_complete_base_search(
    block: &ParsedBlock,
    alignment: u8,
    options: &Options,
    base: PlannedBlock,
    stop: &mut SearchStop<'_>,
) -> PlannedBlock {
    plan_block_with_search_policy(
        block,
        alignment,
        options,
        SearchPolicy::FULL,
        SearchBase::Complete(base),
        stop,
    )
}

/// Search one compact block with proven resegmentation before the table
/// feedback ladder. This order can reach a different fixed point, so Max runs
/// it as a separate candidate.
pub(crate) fn plan_block_with_integrated_proven_search(
    block: &ParsedBlock,
    alignment: u8,
    options: &Options,
    stop: &mut SearchStop<'_>,
) -> PlannedBlock {
    plan_block_with_search_policy(
        block,
        alignment,
        options,
        SearchPolicy {
            integrated_proven: true,
            ..SearchPolicy::FULL
        },
        SearchBase::Price,
        stop,
    )
}

/// Continue a completed integrated floor through the same proven-first search,
/// as a separate comparison route.
pub(crate) fn plan_block_with_complete_integrated_proven_search(
    block: &ParsedBlock,
    alignment: u8,
    options: &Options,
    base: PlannedBlock,
    stop: &mut SearchStop<'_>,
) -> PlannedBlock {
    plan_block_with_search_policy(
        block,
        alignment,
        options,
        SearchPolicy {
            integrated_proven: true,
            ..SearchPolicy::FULL
        },
        SearchBase::Complete(base),
        stop,
    )
}

/// Search one source block without match groups, the ordered state queue or
/// replay, so a long chain of source blocks does not spend its time on the
/// first promising block.
pub(crate) fn plan_block_with_narrow_search(
    block: &ParsedBlock,
    alignment: u8,
    options: &Options,
    allow_individual_prune: bool,
    stop: &mut SearchStop<'_>,
) -> PlannedBlock {
    plan_block_with_search_policy(
        block,
        alignment,
        options,
        SearchPolicy {
            match_groups: false,
            individual_prune: allow_individual_prune,
            ordered_queue: false,
            replay: false,
            large_source_bands: true,
            integrated_proven: false,
        },
        SearchBase::Price,
        stop,
    )
}

/// Run the narrow search with `seed` as the incumbent if it beats the ordinary
/// plan; token transformations still start from the source tokens.
pub(crate) fn plan_block_with_seeded_narrow_search(
    block: &ParsedBlock,
    alignment: u8,
    options: &Options,
    allow_individual_prune: bool,
    seed: PlannedBlock,
    stop: &mut SearchStop<'_>,
) -> PlannedBlock {
    plan_block_with_search_policy(
        block,
        alignment,
        options,
        SearchPolicy {
            match_groups: false,
            individual_prune: allow_individual_prune,
            ordered_queue: false,
            replay: false,
            large_source_bands: true,
            integrated_proven: false,
        },
        SearchBase::Additional(seed),
        stop,
    )
}

enum SearchBase {
    Price,
    /// An independent candidate, kept if it beats the ordinary plan.
    Additional(PlannedBlock),
    /// An already-priced ordinary plan of the same block.
    Complete(PlannedBlock),
}

#[derive(Debug, Clone, Copy)]
struct SearchPolicy {
    match_groups: bool,
    individual_prune: bool,
    ordered_queue: bool,
    replay: bool,
    large_source_bands: bool,
    integrated_proven: bool,
}

impl SearchPolicy {
    const FULL: Self = Self {
        match_groups: true,
        individual_prune: true,
        ordered_queue: true,
        replay: true,
        large_source_bands: false,
        integrated_proven: false,
    };
}

fn plan_block_with_search_policy(
    block: &ParsedBlock,
    alignment: u8,
    options: &Options,
    policy: SearchPolicy,
    base: SearchBase,
    stop: &mut SearchStop<'_>,
) -> PlannedBlock {
    let mut best = match base {
        SearchBase::Price => plan_block(block, alignment, options, &mut *stop),
        SearchBase::Additional(seed) => {
            let ordinary = plan_block(block, alignment, options, &mut *stop);
            if seed.bits < ordinary.bits {
                seed
            } else {
                ordinary
            }
        }
        SearchBase::Complete(base) => base,
    };
    // Price the two feedback seeds before optional work.
    let feedback_seeds = feedback_tree_seeds(block, options.strict);
    consider_feedback_seed_trees(block, options, &feedback_seeds, &mut best);
    if stop.reached() {
        return best;
    }
    consider_same_distance_runs(block, alignment, options, stop, &mut best);
    if stop.reached() {
        return best;
    }
    if policy.integrated_proven {
        consider_proven_submatches(block, alignment, options, stop, &mut best);
        if stop.reached() {
            return best;
        }
    }
    // Like Defluff, normalize the non-standard length-258 alias (symbol 284)
    // on input. Non-strict mode also tries the reverse whenever symbol 285 is
    // used, since symbol 284 plus five extra bits may be cheaper.
    let has_258_alias = block.literal_frequencies[284] != 0
        && block.tokens.iter().any(|token| {
            matches!(
                token,
                Token::Match {
                    length: 258,
                    length_symbol: 284,
                    ..
                }
            )
        });
    if has_258_alias {
        if let Some(normalized) = rewrite_258_symbols(&block.tokens, block.plain.len(), false) {
            if !same_tokens(&normalized, &block.tokens) {
                consider_tokens(block, normalized, alignment, options, stop, &mut best);
            }
        }
    }
    if !options.strict && block.literal_frequencies[285] != 0 {
        if let Some(aliased) = rewrite_258_symbols(&block.tokens, block.plain.len(), true) {
            if !same_tokens(&aliased, &block.tokens) {
                consider_tokens(block, aliased, alignment, options, stop, &mut best);
            }
        }
    }

    let match_count = block
        .distance_frequencies
        .iter()
        .fold(0_u64, |count, &frequency| {
            count.saturating_add(u64::from(frequency))
        });
    if match_count == 0 {
        return best;
    }

    // Also try the all-literals endpoint, as libdeflate does.
    if all_literal_endpoint_is_bounded(block.plain.len(), match_count) && !stop.reached() {
        consider_all_literals(block, options, stop, &mut best);
    }

    let mut seeds = Vec::new();
    if let (Some(literal), Some(distance)) = (
        block.original_literal_lengths.as_ref(),
        block.original_distance_lengths.as_ref(),
    ) {
        seeds.push((literal.to_vec(), distance.to_vec()));
    }
    if let Representation::Dynamic(dynamic) = &best.representation {
        seeds.push((
            dynamic.literal_lengths.clone(),
            dynamic.distance_lengths.clone(),
        ));
    }
    seeds.push(fixed_lengths());
    deduplicate_seeds(&mut seeds);

    if !stop.reached() {
        consider_columbo_defluff_derived_rescan(
            block,
            alignment,
            options,
            &feedback_seeds,
            stop,
            &mut best,
        );
    }

    // Each pass expands every match whose literals cost no more under the
    // current codes, then rebuilds the codes. Passes continue from the previous
    // codes, not the seed, so several strict wins can follow in a row.
    let pass_limit = 12;
    let fixed_seed = fixed_lengths();
    for (mut literal_lengths, mut distance_lengths) in seeds {
        if stop.reached() {
            break;
        }

        // DeflOpt's first expansion pass uses strict cost. Price it separately:
        // expanding equal-cost matches too changes the frequencies and can miss
        // a smaller rebuilt table.
        let is_fixed_seed = literal_lengths == fixed_seed.0 && distance_lengths == fixed_seed.1;
        if !is_fixed_seed {
            if let Some(strictly_expanded) = expand_matches(
                &block.tokens,
                &block.plain,
                &literal_lengths,
                &distance_lengths,
                false,
            ) {
                consider_tokens(
                    block,
                    strictly_expanded,
                    alignment,
                    options,
                    stop,
                    &mut best,
                );
            }
        }

        let Some(mut seed_tokens) = try_clone_token_candidate(&block.tokens, block.plain.len())
        else {
            continue;
        };
        for _ in 0..pass_limit {
            let Some(expanded) = expand_matches(
                &seed_tokens,
                &block.plain,
                &literal_lengths,
                &distance_lengths,
                true,
            ) else {
                break;
            };
            let Some(candidate) = plan_tokens(block, expanded, alignment, options, stop) else {
                break;
            };
            let next_lengths = plan_lengths(&candidate);
            if candidate.bits < best.bits {
                // The winner and the next pass both need these tokens.
                let Some(next_seed_tokens) =
                    try_clone_token_candidate(&candidate.tokens, block.plain.len())
                else {
                    best = candidate;
                    break;
                };
                best = candidate;
                seed_tokens = next_seed_tokens;
            } else {
                seed_tokens = match Arc::try_unwrap(candidate.tokens) {
                    Ok(tokens) => tokens,
                    Err(tokens) => {
                        let Some(tokens) = try_clone_token_candidate(&tokens, block.plain.len())
                        else {
                            break;
                        };
                        tokens
                    }
                };
            }

            let Some((next_literal, next_distance)) = next_lengths else {
                break;
            };
            literal_lengths = next_literal;
            distance_lengths = next_distance;
            if stop.reached() {
                break;
            }
        }
    }

    // Defluff's fixed-code comparison: expand a source match only when its
    // literals are strictly cheaper under the fixed codes.
    let (fixed_literal, fixed_distance) = fixed_seed;
    if let Some(tokens) = expand_matches(
        &block.tokens,
        &block.plain,
        &fixed_literal,
        &fixed_distance,
        false,
    ) {
        consider_tokens(block, tokens, alignment, options, stop, &mut best);
    }

    if policy.large_source_bands && !stop.reached() {
        consider_large_source_bands(block, alignment, options, stop, &mut best);
    }

    if policy.match_groups
        && options.exhaustive
        && block.tokens.len() <= 250_000
        && block.plain.len() <= 10_000_000
        && !stop.reached()
    {
        match_group_search(block, alignment, options, stop, &mut best);
    }

    // Literal-heavy blocks often have only one or two marginal matches, so
    // small blocks also try each match alone.
    let try_individual_prune = policy.individual_prune
        && if options.exhaustive {
            block.tokens.len() <= 4_000
        } else {
            block.tokens.len() <= 20_000 && block.plain.len() <= 10_000_000 && match_count <= 32
        };
    let try_ordered_queue = policy.ordered_queue
        && options.exhaustive
        && block.tokens.len() <= 12_000
        && block.plain.len() <= 80_000;
    // Individual pruning is greedy. Keep the state before it so the ordered
    // queue can also explore the variant in which those matches stay intact.
    let pre_individual = (try_ordered_queue && try_individual_prune && match_count != 0)
        .then(|| try_clone_planned_block(&best))
        .flatten();
    if try_individual_prune && match_count != 0 && !stop.reached() {
        individual_prune_search(block, alignment, options, stop, &mut best);
    }

    if try_ordered_queue && !stop.reached() {
        let alternate =
            pre_individual.filter(|alternate| !same_tokens(&alternate.tokens, &best.tokens));
        let mut seen = HashSet::new();
        // Queue edges only expand matches, so they cannot restore a match that
        // individual pruning removed. Search the intact root first, since it
        // reaches a superset of states.
        if let Some(mut intact) = alternate {
            ordered_state_queue(block, alignment, options, stop, &mut intact, &mut seen);
            if intact.bits < best.bits {
                best = intact;
            }
        }
        if !stop.reached() {
            ordered_state_queue(block, alignment, options, stop, &mut best, &mut seen);
        }
    }

    if policy.replay
        && options.exhaustive
        && !same_tokens(&best.tokens, &block.tokens)
        && !stop.reached()
    {
        // Replay winning token states through the Default search, which avoids
        // recursive route multiplication.
        let mut replay_options = options.clone();
        replay_options.exhaustive = false;
        let Some(mut replay_tokens) = try_clone_token_candidate(&best.tokens, block.plain.len())
        else {
            return best;
        };
        for _ in 0..4 {
            let Some(replay_block) = try_transformed_block(block, replay_tokens) else {
                break;
            };
            let replay = plan_block_with_search(&replay_block, alignment, &replay_options, stop);
            if replay.bits < best.bits {
                let next_tokens = try_clone_token_candidate(&replay.tokens, block.plain.len());
                best = replay;
                let Some(next_tokens) = next_tokens else {
                    break;
                };
                replay_tokens = next_tokens;
            } else {
                break;
            }
            if stop.reached() {
                break;
            }
        }
    }
    best
}

struct QueueState {
    tokens: Vec<Token>,
    bits: u64,
    table: QueueTable,
}

enum QueueTable {
    /// The root's codes are replanned, since the incumbent may use special or
    /// source codes.
    Unknown,
    /// The ordinary table built by `plan_tokens`.
    Lengths { literal: Vec<u8>, distance: Vec<u8> },
    /// A stored child still takes its beam slot at the next depth.
    NoCodes,
}

fn ordered_state_queue(
    block: &ParsedBlock,
    alignment: u8,
    options: &Options,
    stop: &mut SearchStop<'_>,
    best: &mut PlannedBlock,
    seen: &mut HashSet<Vec<Token>>,
) {
    const BEAM: usize = 8;
    const DEPTH: usize = 5;
    const CHILDREN: usize = 6;
    const MARGIN: u64 = 256;

    let Some(initial_tokens) = try_clone_token_candidate(&best.tokens, block.plain.len()) else {
        return;
    };
    let mut current = vec![QueueState {
        tokens: initial_tokens,
        bits: best.bits,
        table: QueueTable::Unknown,
    }];
    // `HashSet` compares full token sequences, so a hash collision cannot
    // suppress a distinct candidate.
    seen.reserve(BEAM * DEPTH * CHILDREN);
    for state in &current {
        let Some(tokens) = try_clone_token_candidate(&state.tokens, block.plain.len()) else {
            return;
        };
        seen.insert(tokens);
    }

    for _ in 0..DEPTH {
        let mut next = Vec::new();
        for state in &current {
            if stop.reached() {
                return;
            }
            let repriced_lengths;
            let (literal_lengths, distance_lengths) = match &state.table {
                QueueTable::Unknown => {
                    let Some(state_tokens) =
                        try_clone_token_candidate(&state.tokens, block.plain.len())
                    else {
                        return;
                    };
                    let Some(state_block) = try_transformed_block(block, state_tokens) else {
                        return;
                    };
                    let state_plan = plan_block(&state_block, alignment, options, &mut *stop);
                    let Some(lengths) = plan_lengths(&state_plan) else {
                        continue;
                    };
                    repriced_lengths = lengths;
                    (repriced_lengths.0.as_slice(), repriced_lengths.1.as_slice())
                }
                QueueTable::Lengths { literal, distance } => {
                    (literal.as_slice(), distance.as_slice())
                }
                QueueTable::NoCodes => continue,
            };
            let groups = collect_match_groups(
                &state.tokens,
                &block.plain,
                literal_lengths,
                distance_lengths,
            );
            for group in groups.iter().take(CHILDREN) {
                if stop.reached() {
                    return;
                }
                let Some(tokens) =
                    expand_groups(&state.tokens, &block.plain, std::slice::from_ref(group))
                else {
                    continue;
                };
                let Some(seen_tokens) = try_clone_token_candidate(&tokens, block.plain.len())
                else {
                    return;
                };
                if !seen.insert(seen_tokens) {
                    continue;
                }
                let Some(planned_tokens) = try_clone_token_candidate(&tokens, block.plain.len())
                else {
                    return;
                };
                let Some(plan) = plan_tokens(block, planned_tokens, alignment, options, stop)
                else {
                    return;
                };
                let plan_bits = plan.bits;
                let improves_best = plan_bits < best.bits;
                let enters_beam =
                    plan_bits <= state.bits.saturating_add(MARGIN) || plan_bits <= best.bits;
                let table = enters_beam.then(|| {
                    plan_lengths(&plan).map_or(QueueTable::NoCodes, |(literal, distance)| {
                        QueueTable::Lengths { literal, distance }
                    })
                });
                if improves_best {
                    // The queue keeps its own tokens and codes below, so the
                    // plan can become the incumbent without a clone.
                    *best = plan;
                }
                if let Some(table) = table {
                    next.push(QueueState {
                        tokens,
                        bits: plan_bits,
                        table,
                    });
                }
            }
        }
        next.sort_shared_by_key(|state| state.bits);
        next.truncate(BEAM);
        if next.is_empty() {
            break;
        }
        current = next;
    }
}

type LengthBuilder = fn(&[u32], u8, u32) -> Vec<u8>;

struct FeedbackTreeSeed {
    builder: LengthBuilder,
    literal: Vec<u8>,
    distance: Vec<u8>,
}

/// Build the two feedback seed trees: Defluff's exact builder, and Columbo's
/// ordinary tree with only Defluff's package-merge limiter.
fn feedback_tree_seeds(block: &ParsedBlock, min_distance_codes: bool) -> [FeedbackTreeSeed; 2] {
    let mut distance_frequencies = block.distance_frequencies;
    ensure_floor_distance_symbols(&mut distance_frequencies, min_distance_codes);
    [
        make_lengths_columbo_defluff_limited as LengthBuilder,
        make_lengths_defluff_exact as LengthBuilder,
    ]
    .map(|builder| FeedbackTreeSeed {
        builder,
        literal: builder(&block.literal_frequencies, 15, 0),
        distance: builder(&distance_frequencies, 15, 0),
    })
}

fn consider_feedback_seed_trees(
    block: &ParsedBlock,
    options: &Options,
    seeds: &[FeedbackTreeSeed; 2],
    best: &mut PlannedBlock,
) {
    for seed in seeds {
        let Some(dynamic) = plan_for_explicit_lengths(
            &block.tokens,
            &seed.literal,
            &seed.distance,
            options.exhaustive,
        ) else {
            continue;
        };
        if dynamic.bits < best.bits {
            let bits = dynamic.bits;
            best.representation = Representation::Dynamic(dynamic);
            best.bits = bits;
            best.tokens = block.tokens.clone();
            best.plain = block.plain.clone();
            best.source_type = block.source_type;
        }
    }
}

/// Defluff-style two-generation rescan for each feedback seed: expand matches
/// that the seed tree cannot code or codes more cheaply as literals, rebuild
/// the tree with the same builder, then rescan the original tokens under it.
/// Columbo's planner, not Defluff's tables, prices each token set.
fn consider_columbo_defluff_derived_rescan(
    block: &ParsedBlock,
    alignment: u8,
    options: &Options,
    seeds: &[FeedbackTreeSeed; 2],
    stop: &mut SearchStop<'_>,
    best: &mut PlannedBlock,
) {
    for seed in seeds {
        if stop.reached() {
            return;
        }
        let Some(fresh_tokens) =
            expand_defluff_matches(&block.tokens, &block.plain, &seed.literal, &seed.distance)
        else {
            continue;
        };
        let (fresh_literal_frequencies, fresh_distance_frequencies) =
            count_frequencies(&fresh_tokens);
        consider_tokens(block, fresh_tokens, alignment, options, stop, best);

        let adjusted_literal = (seed.builder)(&fresh_literal_frequencies, 15, 0);
        let adjusted_distance = (seed.builder)(&fresh_distance_frequencies, 15, 0);
        // A rescan that marks no match gives the already priced source.
        if let Some(adjusted_tokens) = expand_defluff_matches(
            &block.tokens,
            &block.plain,
            &adjusted_literal,
            &adjusted_distance,
        ) {
            consider_tokens(block, adjusted_tokens, alignment, options, stop, best);
        }
    }
}

fn expand_defluff_matches(
    tokens: &[Token],
    plain: &[u8],
    literal_lengths: &[u8],
    distance_lengths: &[u8],
) -> Option<Vec<Token>> {
    expand_selected_matches(tokens, plain, |_, token, decoded| {
        let Token::Match {
            length,
            length_symbol,
            distance_symbol,
            length_extra_bits,
            distance_extra_bits,
            ..
        } = token
        else {
            return false;
        };
        debug_assert_eq!(decoded.len(), usize::from(length));

        let match_literal_length = literal_lengths
            .get(usize::from(length_symbol))
            .copied()
            .unwrap_or(0);
        let match_distance_length = distance_lengths
            .get(usize::from(distance_symbol))
            .copied()
            .unwrap_or(0);
        if match_literal_length == 0 || match_distance_length == 0 {
            return true;
        }

        let literals_available = decoded
            .iter()
            .all(|&byte| literal_lengths.get(usize::from(byte)).copied().unwrap_or(0) != 0);
        if !literals_available {
            return false;
        }
        let literal_bits: u64 = decoded
            .iter()
            .map(|&byte| u64::from(literal_lengths[usize::from(byte)]))
            .sum();
        let match_bits = u64::from(match_literal_length)
            + u64::from(length_extra_bits)
            + u64::from(match_distance_length)
            + u64::from(distance_extra_bits);
        literal_bits < match_bits
    })
}

/// Price length-symbol bands for the narrow search on large source blocks:
/// five cumulative bands 257..=end and one sparse set, `LONG_FAMILIES`.
fn consider_large_source_bands(
    block: &ParsedBlock,
    alignment: u8,
    options: &Options,
    stop: &mut SearchStop<'_>,
    best: &mut PlannedBlock,
) {
    if !large_source_bands_eligible(block.plain.len(), block.tokens.len()) {
        return;
    }

    const CUMULATIVE_ENDS: [u16; 5] = [262, 265, 267, 269, 270];
    for end in CUMULATIVE_ENDS {
        if stop.reached() {
            return;
        }
        let Some(tokens) = expand_selected_matches(
            &block.tokens,
            &block.plain,
            |_, token, _| matches!(token, Token::Match { length_symbol, .. } if (257..=end).contains(&length_symbol)),
        ) else {
            continue;
        };
        consider_tokens(block, tokens, alignment, options, stop, best);
    }

    if stop.reached() {
        return;
    }
    const LONG_FAMILIES: [u16; 12] = [260, 261, 262, 263, 264, 265, 266, 267, 268, 269, 270, 280];
    if let Some(tokens) = expand_selected_matches(
        &block.tokens,
        &block.plain,
        |_, token, _| matches!(token, Token::Match { length_symbol, .. } if LONG_FAMILIES.contains(&length_symbol)),
    ) {
        consider_tokens(block, tokens, alignment, options, stop, best);
    }
}

fn large_source_bands_eligible(plain_bytes: usize, token_count: usize) -> bool {
    plain_bytes >= 128_000 && token_count <= 80_000
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum GroupKind {
    Length,
    Distance,
}

#[derive(Debug, Clone, Copy)]
struct MatchGroup {
    kind: GroupKind,
    symbol: u16,
    count: usize,
    delta: i64,
}

fn match_group_search(
    block: &ParsedBlock,
    alignment: u8,
    options: &Options,
    stop: &mut SearchStop<'_>,
    best: &mut PlannedBlock,
) {
    let mut seeds = Vec::new();
    if let Some(lengths) = plan_lengths(best) {
        seeds.push(lengths);
    }
    if let (Some(literal), Some(distance)) = (
        block.original_literal_lengths.as_ref(),
        block.original_distance_lengths.as_ref(),
    ) {
        seeds.push((literal.to_vec(), distance.to_vec()));
    }
    deduplicate_seeds(&mut seeds);

    for (literal_lengths, distance_lengths) in seeds {
        let groups = collect_match_groups(
            &block.tokens,
            &block.plain,
            &literal_lengths,
            &distance_lengths,
        );
        let cap = if options.exhaustive { 20 } else { 8 };
        for group in groups.iter().take(cap) {
            if stop.reached() {
                return;
            }
            if let Some(tokens) =
                expand_groups(&block.tokens, &block.plain, std::slice::from_ref(group))
            {
                consider_tokens(block, tokens, alignment, options, stop, best);
            }
        }

        // Also expand the first few prefixes of the ranked groups together.
        let combined_cap = if options.exhaustive { 5 } else { 3 };
        for count in 2..=combined_cap.min(groups.len()) {
            if stop.reached() {
                return;
            }
            if let Some(tokens) = expand_groups(&block.tokens, &block.plain, &groups[..count]) {
                consider_tokens(block, tokens, alignment, options, stop, best);
            }
        }
    }
}

fn collect_match_groups(
    tokens: &[Token],
    plain: &[u8],
    literal_lengths: &[u8],
    distance_lengths: &[u8],
) -> Vec<MatchGroup> {
    let mut length_delta = [0_i64; 29];
    let mut length_count = [0_usize; 29];
    let mut length_valid = [true; 29];
    let mut distance_delta = [0_i64; 30];
    let mut distance_count = [0_usize; 30];
    let mut distance_valid = [true; 30];
    let mut plain_offset = 0;

    for &token in tokens {
        if let Token::Match {
            length,
            length_symbol,
            distance_symbol,
            length_extra_bits,
            distance_extra_bits,
            ..
        } = token
        {
            let decoded = &plain[plain_offset..plain_offset + usize::from(length)];
            let length_index = usize::from(length_symbol - 257);
            let distance_index = usize::from(distance_symbol);
            let literal_codes_available = decoded
                .iter()
                .all(|&byte| literal_lengths.get(usize::from(byte)).copied().unwrap_or(0) != 0);
            let match_codes_available = literal_lengths
                .get(usize::from(length_symbol))
                .copied()
                .unwrap_or(0)
                != 0
                && distance_lengths.get(distance_index).copied().unwrap_or(0) != 0;
            length_count[length_index] += 1;
            distance_count[distance_index] += 1;
            if !literal_codes_available || !match_codes_available {
                length_valid[length_index] = false;
                distance_valid[distance_index] = false;
            } else {
                let literal_bits: i64 = decoded
                    .iter()
                    .map(|&byte| i64::from(literal_lengths[usize::from(byte)]))
                    .sum();
                let match_bits = i64::from(literal_lengths[usize::from(length_symbol)])
                    + i64::from(length_extra_bits)
                    + i64::from(distance_lengths[distance_index])
                    + i64::from(distance_extra_bits);
                let delta = literal_bits - match_bits;
                length_delta[length_index] += delta;
                distance_delta[distance_index] += delta;
            }
        }
        plain_offset += token.decoded_len();
    }

    let mut groups = Vec::new();
    for index in 0..29 {
        if length_count[index] != 0 && length_valid[index] {
            groups.push(MatchGroup {
                kind: GroupKind::Length,
                symbol: (index + 257) as u16,
                count: length_count[index],
                delta: length_delta[index],
            });
        }
    }
    for index in 0..30 {
        if distance_count[index] != 0 && distance_valid[index] {
            groups.push(MatchGroup {
                kind: GroupKind::Distance,
                symbol: index as u16,
                count: distance_count[index],
                delta: distance_delta[index],
            });
        }
    }
    groups.sort_shared_by_key(|group| (group.delta, group.count, group.symbol));
    groups
}

fn expand_groups(tokens: &[Token], plain: &[u8], groups: &[MatchGroup]) -> Option<Vec<Token>> {
    expand_selected_matches(tokens, plain, |_, token, _| match token {
        Token::Literal(_) => false,
        Token::Match {
            length_symbol,
            distance_symbol,
            ..
        } => groups.iter().any(|group| match group.kind {
            GroupKind::Length => group.symbol == length_symbol,
            GroupKind::Distance => group.symbol == u16::from(distance_symbol),
        }),
    })
}

// Out of line: each call plans a whole block.
#[inline(never)]
fn consider_tokens(
    source: &ParsedBlock,
    tokens: Vec<Token>,
    alignment: u8,
    options: &Options,
    stop: &mut SearchStop<'_>,
    best: &mut PlannedBlock,
) {
    if let Some(candidate) = plan_tokens(source, tokens, alignment, options, stop) {
        if candidate.bits < best.bits {
            *best = candidate;
        }
    }
}

fn plan_tokens(
    source: &ParsedBlock,
    tokens: Vec<Token>,
    alignment: u8,
    options: &Options,
    stop: &mut SearchStop<'_>,
) -> Option<PlannedBlock> {
    let candidate = try_transformed_block(source, tokens)?;
    Some(plan_owned_block(candidate, alignment, options, stop))
}

/// Build the block needed to price one optional token transformation. The
/// original bit count is cleared because it describes the source tokens.
pub(super) fn try_transformed_block(
    source: &ParsedBlock,
    tokens: Vec<Token>,
) -> Option<ParsedBlock> {
    if parsed_model_bytes(source.plain.len(), tokens.len(), 1)? > MAX_TOKEN_CANDIDATE_BYTES {
        return None;
    }
    let plain = Arc::clone(&source.plain);
    let source_splits = try_clone_slice(&source.source_splits)?;
    let original_dynamic = match source.original_dynamic.as_ref() {
        Some(dynamic) => Some(dynamic.try_clone()?),
        None => None,
    };
    let (literal_frequencies, distance_frequencies) = count_frequencies(&tokens);
    Some(ParsedBlock {
        tokens: tokens.into(),
        plain,
        literal_frequencies,
        distance_frequencies,
        original_literal_lengths: source.original_literal_lengths,
        original_distance_lengths: source.original_distance_lengths,
        original_dynamic,
        original: None,
        source_splits,
        source_type: source.source_type,
    })
}

fn plan_lengths(plan: &PlannedBlock) -> Option<(Vec<u8>, Vec<u8>)> {
    match &plan.representation {
        Representation::Dynamic(dynamic) => Some((
            dynamic.literal_lengths.clone(),
            dynamic.distance_lengths.clone(),
        )),
        Representation::Fixed => Some(fixed_lengths()),
        Representation::Original(_) | Representation::Stored => None,
    }
}

pub(crate) fn try_clone_planned_block(plan: &PlannedBlock) -> Option<PlannedBlock> {
    Some(PlannedBlock {
        tokens: Arc::clone(&plan.tokens),
        plain: Arc::clone(&plan.plain),
        representation: plan.representation.try_clone()?,
        bits: plan.bits,
        source_type: plan.source_type,
    })
}

fn fixed_lengths() -> (Vec<u8>, Vec<u8>) {
    (
        FIXED_LITERAL_CODE_LENGTHS.to_vec(),
        FIXED_DISTANCE_CODE_LENGTHS.to_vec(),
    )
}

fn deduplicate_seeds(seeds: &mut Vec<(Vec<u8>, Vec<u8>)>) {
    let mut index = 0;
    while index < seeds.len() {
        if seeds[..index].iter().any(|seed| seed == &seeds[index]) {
            seeds.remove(index);
        } else {
            index += 1;
        }
    }
}

/// `None` is not an error: callers keep their complete plan.
fn new_token_candidate_with_limit(
    token_count: usize,
    decoded_bytes: usize,
    byte_limit: usize,
) -> Option<Vec<Token>> {
    if parsed_model_bytes(decoded_bytes, token_count, 1)? > byte_limit {
        return None;
    }
    let mut tokens = Vec::new();
    tokens.try_reserve_exact(token_count).ok()?;
    Some(tokens)
}

fn new_token_candidate(token_count: usize, decoded_bytes: usize) -> Option<Vec<Token>> {
    new_token_candidate_with_limit(token_count, decoded_bytes, MAX_TOKEN_CANDIDATE_BYTES)
}

// Out of line: each call copies a whole token list.
#[inline(never)]
fn try_clone_token_candidate(source: &[Token], decoded_bytes: usize) -> Option<Vec<Token>> {
    let mut tokens = new_token_candidate(source.len(), decoded_bytes)?;
    tokens.extend_from_slice(source);
    Some(tokens)
}

fn literal_token_candidate(plain: &[u8]) -> Option<Vec<Token>> {
    let mut tokens = new_token_candidate(plain.len(), plain.len())?;
    tokens.extend(plain.iter().copied().map(Token::Literal));
    Some(tokens)
}

fn all_literal_endpoint_is_bounded(plain_len: usize, match_count: u64) -> bool {
    plain_len <= ALL_LITERAL_DENSE_MAX_PLAIN
        || (plain_len <= ALL_LITERAL_SPARSE_MAX_PLAIN
            && match_count <= ALL_LITERAL_SPARSE_MAX_MATCHES)
}

/// Exactly price the all-literals endpoint from its byte frequencies, building
/// its tokens only for a strict win.
fn consider_all_literals(
    block: &ParsedBlock,
    options: &Options,
    stop: &mut SearchStop<'_>,
    best: &mut PlannedBlock,
) {
    if block.plain.is_empty() || best.tokens.len() == block.plain.len() || stop.reached() {
        return;
    }

    let mut literal_frequencies = [0_u32; 286];
    for &byte in block.plain.iter() {
        literal_frequencies[usize::from(byte)] += 1;
    }
    literal_frequencies[256] = 1;
    let distance_frequencies = [0_u32; 30];

    // A one-tree, greedy-header estimate rejects endpoints far from the
    // incumbent; the 256-bit margin leaves rare small wins to the full planner.
    let Some(estimated_bits) = estimate_boundary_block_bits(
        &literal_frequencies,
        &distance_frequencies,
        0,
        options.strict,
    ) else {
        return;
    };
    if estimated_bits >= best.bits.saturating_add(256) {
        return;
    }

    let fixed_bits = token_bits_from_frequencies(
        &literal_frequencies,
        &distance_frequencies,
        &FIXED_LITERAL_CODE_LENGTHS,
        &FIXED_DISTANCE_CODE_LENGTHS,
        0,
    )
    .and_then(|bits| bits.checked_add(3))
    .unwrap_or(u64::MAX);
    let dynamic = best_dynamic_plan_cached(
        &[],
        &literal_frequencies,
        &distance_frequencies,
        None,
        options.strict,
        false,
        stop,
        &mut HeaderPlanCache::new(),
    );
    let (representation, bits) = match dynamic {
        Some(dynamic) if dynamic.bits < fixed_bits => {
            let bits = dynamic.bits;
            (Representation::Dynamic(dynamic), bits)
        }
        _ => (Representation::Fixed, fixed_bits),
    };
    if bits >= best.bits {
        return;
    }

    let Some(tokens) = literal_token_candidate(&block.plain) else {
        return;
    };
    *best = PlannedBlock {
        tokens: tokens.into(),
        plain: Arc::clone(&block.plain),
        representation,
        bits,
        source_type: block.source_type,
    };
}

/// Expand exactly the matches selected by `should_expand` into literals;
/// `None` if nothing is selected or on failure.
pub(super) fn expand_selected_matches<F>(
    source: &[Token],
    plain: &[u8],
    should_expand: F,
) -> Option<Vec<Token>>
where
    F: Fn(usize, Token, &[u8]) -> bool,
{
    expand_selected_matches_with_limit(source, plain, MAX_TOKEN_CANDIDATE_BYTES, should_expand)
}

fn expand_selected_matches_with_limit<F>(
    source: &[Token],
    plain: &[u8],
    byte_limit: usize,
    should_expand: F,
) -> Option<Vec<Token>>
where
    F: Fn(usize, Token, &[u8]) -> bool,
{
    expand_selected_matches_with_limits(source, plain, byte_limit, usize::MAX, should_expand)
}

fn expand_selected_matches_with_limits<F>(
    source: &[Token],
    plain: &[u8],
    byte_limit: usize,
    token_limit: usize,
    should_expand: F,
) -> Option<Vec<Token>>
where
    F: Fn(usize, Token, &[u8]) -> bool,
{
    let mut output_count = 0_usize;
    let mut plain_offset = 0_usize;
    let mut changed = false;
    for (index, &token) in source.iter().enumerate() {
        let end = plain_offset.checked_add(token.decoded_len())?;
        let decoded = plain.get(plain_offset..end)?;
        let expand = matches!(token, Token::Match { .. }) && should_expand(index, token, decoded);
        let added = if expand { decoded.len() } else { 1 };
        output_count = output_count.checked_add(added)?;
        if output_count > token_limit {
            return None;
        }
        changed |= expand;
        plain_offset = end;
    }
    if !changed || plain_offset != plain.len() {
        return None;
    }

    let mut output = new_token_candidate_with_limit(output_count, plain.len(), byte_limit)?;
    plain_offset = 0;
    for (index, &token) in source.iter().enumerate() {
        let end = plain_offset.checked_add(token.decoded_len())?;
        let decoded = plain.get(plain_offset..end)?;
        let expand = matches!(token, Token::Match { .. }) && should_expand(index, token, decoded);
        if expand {
            output.extend(decoded.iter().copied().map(Token::Literal));
        } else {
            output.push(token);
        }
        plain_offset = end;
    }
    debug_assert_eq!(output.len(), output_count);
    Some(output)
}

fn expand_matches(
    tokens: &[Token],
    plain: &[u8],
    literal_lengths: &[u8],
    distance_lengths: &[u8],
    include_equal: bool,
) -> Option<Vec<Token>> {
    expand_matches_with_token_limit(
        tokens,
        plain,
        literal_lengths,
        distance_lengths,
        include_equal,
        usize::MAX,
    )
}

fn expand_matches_with_token_limit(
    tokens: &[Token],
    plain: &[u8],
    literal_lengths: &[u8],
    distance_lengths: &[u8],
    include_equal: bool,
    token_limit: usize,
) -> Option<Vec<Token>> {
    expand_selected_matches_with_limits(
        tokens,
        plain,
        MAX_TOKEN_CANDIDATE_BYTES,
        token_limit,
        |_, token, decoded| {
            let Token::Match {
                length_symbol,
                distance_symbol,
                length_extra_bits,
                distance_extra_bits,
                ..
            } = token
            else {
                return false;
            };
            let literal_bits: u64 = decoded
                .iter()
                .map(|&byte| estimated_length(literal_lengths, usize::from(byte)))
                .sum();
            let match_bits = estimated_length(literal_lengths, usize::from(length_symbol))
                + u64::from(length_extra_bits)
                + estimated_length(distance_lengths, usize::from(distance_symbol))
                + u64::from(distance_extra_bits);
            literal_bits < match_bits || (include_equal && literal_bits == match_bits)
        },
    )
}

/// Whether any match costs strictly more than its literals under the given
/// code lengths, priced as the table replay prices them.
fn has_dearer_match(
    tokens: &[Token],
    plain: &[u8],
    literal_lengths: &[u8],
    distance_lengths: &[u8],
) -> bool {
    let mut offset = 0_usize;
    for token in tokens {
        match *token {
            Token::Literal(_) => offset += 1,
            Token::Match {
                length,
                length_symbol,
                distance_symbol,
                length_extra_bits,
                distance_extra_bits,
                ..
            } => {
                let end = offset + usize::from(length);
                let Some(decoded) = plain.get(offset..end) else {
                    return false;
                };
                let literal_bits: u64 = decoded
                    .iter()
                    .map(|&byte| estimated_length(literal_lengths, usize::from(byte)))
                    .sum();
                let match_bits = estimated_length(literal_lengths, usize::from(length_symbol))
                    + u64::from(length_extra_bits)
                    + estimated_length(distance_lengths, usize::from(distance_symbol))
                    + u64::from(distance_extra_bits);
                if literal_bits < match_bits {
                    return true;
                }
                offset = end;
            }
        }
    }
    false
}

fn estimated_length(lengths: &[u8], symbol: usize) -> u64 {
    lengths
        .get(symbol)
        .copied()
        .filter(|&length| length != 0)
        .map_or(15, u64::from)
}

pub(crate) fn rewrite_258_symbols(
    tokens: &[Token],
    decoded_bytes: usize,
    use_alias: bool,
) -> Option<Vec<Token>> {
    let mut rewritten = new_token_candidate(tokens.len(), decoded_bytes)?;
    for token in tokens.iter().copied() {
        rewritten.push(match token {
            Token::Match {
                length: 258,
                distance,
                length_symbol,
                distance_symbol,
                length_extra: _,
                distance_extra,
                length_extra_bits: _,
                distance_extra_bits,
            } if (use_alias && length_symbol == 285) || (!use_alias && length_symbol == 284) => {
                Token::Match {
                    length: 258,
                    distance,
                    length_symbol: if use_alias { 284 } else { 285 },
                    distance_symbol,
                    length_extra: if use_alias { 31 } else { 0 },
                    distance_extra,
                    length_extra_bits: if use_alias { 5 } else { 0 },
                    distance_extra_bits,
                }
            }
            _ => token,
        });
    }
    Some(rewritten)
}

fn individual_prune_search(
    block: &ParsedBlock,
    alignment: u8,
    options: &Options,
    stop: &mut SearchStop<'_>,
    best: &mut PlannedBlock,
) {
    // If the original encoding is still the incumbent, `best` has no codes, so
    // use the source's decoded codes.
    let seed_lengths = individual_prune_seed_lengths(block, best);
    let Some((literal_lengths, distance_lengths)) = seed_lengths else {
        return;
    };
    individual_prune_from_lengths(
        block,
        alignment,
        options,
        stop,
        best,
        &literal_lengths,
        &distance_lengths,
        32,
    );
}

#[allow(clippy::too_many_arguments)]
fn individual_prune_from_lengths(
    block: &ParsedBlock,
    alignment: u8,
    options: &Options,
    stop: &mut SearchStop<'_>,
    best: &mut PlannedBlock,
    literal_lengths: &[u8],
    distance_lengths: &[u8],
    trial_limit: usize,
) {
    if trial_limit == 0 {
        return;
    }
    let mut trials = Vec::new();
    if trials.try_reserve_exact(trial_limit).is_err() {
        return;
    }
    let mut plain_offset = 0;
    for (index, &token) in block.tokens.iter().enumerate() {
        if let Token::Match {
            length,
            length_symbol,
            distance_symbol,
            length_extra_bits,
            distance_extra_bits,
            ..
        } = token
        {
            let decoded = &block.plain[plain_offset..plain_offset + usize::from(length)];
            let literal_bits: i64 = decoded
                .iter()
                .map(|&byte| estimated_length(literal_lengths, usize::from(byte)) as i64)
                .sum();
            let match_bits = estimated_length(literal_lengths, usize::from(length_symbol))
                + u64::from(length_extra_bits)
                + estimated_length(distance_lengths, usize::from(distance_symbol))
                + u64::from(distance_extra_bits);
            let trial = (literal_bits - match_bits as i64, index);
            let position = trials.partition_point(|candidate| *candidate <= trial);
            if trials.len() < trial_limit {
                trials.insert(position, trial);
            } else if position < trial_limit {
                // Drop the worst trial first to stay within the reservation.
                trials.pop();
                trials.insert(position, trial);
            }
        }
        plain_offset += token.decoded_len();
    }

    for (_, token_index) in trials {
        if stop.reached() {
            break;
        }
        if let Some(tokens) = expand_selected_matches(&block.tokens, &block.plain, |index, _, _| {
            index == token_index
        }) {
            consider_tokens(block, tokens, alignment, options, stop, best);
        }
    }
}

fn individual_prune_seed_lengths(
    block: &ParsedBlock,
    best: &PlannedBlock,
) -> Option<(Vec<u8>, Vec<u8>)> {
    plan_lengths(best).or_else(|| {
        Some((
            block.original_literal_lengths.as_ref()?.to_vec(),
            block.original_distance_lengths.as_ref()?.to_vec(),
        ))
    })
}

#[cfg(test)]
mod tests;
