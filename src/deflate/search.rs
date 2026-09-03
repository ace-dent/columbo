// SPDX-License-Identifier: MIT

//! Proven-LZ77 structural searches.
//!
//! These routes never search the history window or choose a new distance. They
//! selectively spell an existing match as its already-decoded literals,
//! resegment one match into literals and same-distance submatches, or move
//! boundaries inside an adjacent run already proven at one distance, then
//! rebuild the Huffman representation. Equal-cost token transformations are
//! useful intermediate states because their changed frequencies can make the
//! next tree smaller.
//!
//! Names identify recovered primitives precisely. DeflOpt and Defluff labels
//! apply only to behavior mapped to those programs; bounded floors, cumulative
//! length-family bands, match groups, queues, and repeated replay are Columbo
//! compositions, even when they use a recovered primitive internally.

use std::cmp::Reverse;
use std::collections::HashSet;
use std::sync::Arc;

use crate::Options;

use super::block::{plan_block, plan_owned_block};
use super::header::{
    best_dynamic_plan_cached, estimate_boundary_block_bits, plan_for_explicit_lengths,
    plan_for_explicit_lengths_with_cost, token_bits_from_frequencies, HeaderPlanCache,
};
#[cfg(test)]
use super::huffman::make_lengths_deflopt_heap;
use super::huffman::{
    make_lengths_columbo_defluff_limited, make_lengths_deflopt_heap_into_with_scratch,
    make_lengths_defluff_exact, make_lengths_deft4j_java_heap, DefloptHeapScratch,
    FIXED_DISTANCE_CODE_LENGTHS, FIXED_LITERAL_CODE_LENGTHS,
};
use super::model::{
    canonical_length_encoding, count_frequencies, token_extra_bits, try_clone_slice, DynamicPlan,
    ParsedBlock, PlannedBlock, Representation, Token, LENGTH_BASE as DEFLATE_LENGTH_BASE,
};
use super::parse::{parsed_model_bytes, MAX_PARSED_MODEL_BYTES};
use super::stop::SearchStop;

/// A transformed token vector is temporary, but it can be much larger than
/// its source: one compact match may become 258 literal `Token` values. Give a
/// single optional candidate the same byte ceiling as the persistent parser
/// model, and allocate it fallibly. The parsed source remains separately
/// accounted by the same policy in `parse`.
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
// The exhaustive deficit DP can require 257 layers. Default mode keeps the
// exact cost search for ordinary runs, but uses a legal minimum-token fallback
// once a run would make this cheap candidate family disproportionate.
const DEFAULT_SAME_DISTANCE_MAX_DP_ACTIVE: usize = 16;
// Default mode exact-prices one combined candidate from a short ranked list.
// Compact blocks may also try a few single-match siblings, while max mode
// widens to every match in the explicitly bounded full-graph band.
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
// The adaptive sibling exact-prices at most fifteen forward/reverse prefixes
// in its first round. A second round is admitted only after a strict win and
// shares this overall cap, so no-gain compact blocks pay for one bounded pass.
const PROVEN_CLOSED_LOOP_MAX_PLAIN: usize = 1_024;
const PROVEN_CLOSED_LOOP_ROUNDS: usize = 2;
const PROVEN_CLOSED_LOOP_EXACT_LIMIT: usize = 24;
// These source-tree probes are part of the deadline-independent compact floor.
// Keep only a small, ranked set so a many-frame container cannot spend its
// shared budget rebuilding every nearly identical local candidate.
// Ranked single-match trials are a Columbo extension. deft4j expands every
// eligible match together in its strict and no-larger recode operations.
const COLUMBO_SINGLE_MATCH_TRIALS: usize = 8;

/// Source-only diagnostics for adjacent matches that reuse one distance.
///
/// These values describe the parsed input, not the number of candidates later
/// visited by merged and replayed routes. Keeping that distinction prevents
/// verbose corpus measurements from depending on search order or timeout.
/// Adjacent source blocks are treated as one token stream, so an empty block
/// is not an artificial barrier; stored bytes and other literals naturally
/// terminate a match run.
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

/// Add one deterministic same-distance run candidate to an existing plan.
///
/// This wrapper deliberately uses the ordinary header policy even when called
/// from a max-mode structural floor. The full search invokes the shared inner
/// helper with its own policy and deadline.
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

    // Price the canonical candidate first. Its planned token storage is
    // reference-counted, so relaxed mode can derive an alias sibling lazily
    // without retaining or eagerly allocating a second large token vector.
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

/// Repartition every maximal adjacent run that uses one semantic distance.
///
/// If a run decodes `S` bytes, `ceil(S / 258)` is both legal and no larger
/// than its source match count. The first source match proves the distance is
/// valid at the run start; every later byte follows the same backward relation,
/// including overlapping copies. Moving only those internal boundaries is
/// therefore lossless and never searches the history window.
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
            // A run of at most 258 bytes becomes one canonical match directly;
            // an already-minimal all-258 run is unchanged. Neither case needs
            // to allocate or traverse the deficit DP.
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
        changed |= output[output_start..] != tokens[run.start..run.end];
        index = run.end;
    }
    debug_assert_eq!(output.len(), output_count);
    changed.then_some(output)
}

/// Append a legal minimum-token partition without running the cost DP.
///
/// The total deficit is at most 257, so at most two shortened matches are
/// needed when each match may contribute up to 255 deficit bytes (length 3).
/// Exact stream pricing still decides whether this bounded default candidate
/// is useful.
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
    /// Build a DP over the deficit from an all-length-258 partition.
    ///
    /// With the minimum number of output matches the total deficit is at most
    /// 257 bytes, regardless of run length. At most `deficit` matches can have
    /// a non-zero deficit, so long runs add no DP depth.
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

        let mut cost_by_deficit = [0_u32; 256];
        for (deficit, cost) in cost_by_deficit.iter_mut().enumerate() {
            let length = 258_u16.checked_sub(deficit as u16)?;
            let (symbol, _, extra_bits) = canonical_length_encoding(length)?;
            *cost = estimated_length(literal_lengths, usize::from(symbol))
                .checked_add(u64::from(extra_bits))?
                .try_into()
                .ok()?;
        }

        let mut previous = [u32::MAX; 258];
        let mut current = [u32::MAX; 258];
        previous[0] = 0;
        for slot in 1..=max_active {
            if stop.reached() {
                return None;
            }
            current[..width].fill(u32::MAX);
            for used_deficit in 0..=max_deficit {
                if used_deficit & 31 == 0 && stop.reached() {
                    return None;
                }
                let mut best_cost = u32::MAX;
                let mut best_deficit = u16::MAX;
                for token_deficit in 0..=used_deficit.min(255) {
                    let prefix = previous[used_deficit - token_deficit];
                    if prefix == u32::MAX {
                        continue;
                    }
                    let candidate = prefix.checked_add(cost_by_deficit[token_deficit])?;
                    if candidate < best_cost {
                        best_cost = candidate;
                        best_deficit = token_deficit as u16;
                    }
                }
                current[used_deficit] = best_cost;
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
        lengths.sort_unstable_by(|left, right| right.cmp(left));
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

/// Add subranges already proved by each existing match.
///
/// This is not match finding: every generated match retains the source
/// distance and stays inside the source match's decoded interval. Default mode
/// exact-prices one bounded set of rare, header-expensive, transition-adjacent
/// or boundary-near matches. Max mode widens compact blocks to every match and
/// repeats only strict winners until their token spelling stabilizes.
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

/// Whether the complete proven-feedback sibling has tightly bounded work.
pub(crate) fn compact_proven_submatch_route_eligible(tokens: &[Token], plain_len: usize) -> bool {
    tokens.len() <= COMPACT_PROVEN_SUBMATCH_TOKENS
        && plain_len <= MAX_PROVEN_SUBMATCH_FULL_PLAIN
        && tokens
            .iter()
            .any(|token| matches!(token, Token::Match { .. }))
}

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
        // The source edge intentionally wins estimated payload ties. Build a
        // second, source-symbol-free path so exact header pricing can still
        // accept a tie or small payload penalty that removes an expensive
        // code-length entry.
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

        // A few single-match siblings keep one locally useful resegmentation
        // from being hidden by other graph choices. The combined candidate
        // below remains the main default route.
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
        symbol_free_rewrites.sort_by_key(|rewrite| rewrite.token_index);
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

        // Per-symbol target caps deliberately keep one common symbol from
        // starving other opportunities. This separate candidate still removes
        // the highest symbol from every occurrence when that complete
        // header-oriented rewrite fits its explicit bound.
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
            || best.tokens.as_slice() == pass_tokens.as_slice()
            || pass + 1 == MAX_PROVEN_SUBMATCH_PASSES
        {
            break;
        }
    }
}

/// Continue one complete block through Columbo's proven-submatch endpoint.
///
/// The stream optimizer runs this as an independent candidate lineage. Keeping
/// it separate prevents an immediate local resegmentation win from replacing a
/// different token spelling that reaches a better fixed point after replay.
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

/// Compose a bounded menu of already-proven match spellings under Max's
/// complete header pricing.
///
/// This is an additive compact route: it starts from a completed incumbent,
/// never searches history, and keeps only frequency-distinct beam states.
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
        order.sort_by_key(|&index| {
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

    // With one rewrite, the combined candidate is exactly the individual
    // candidate already priced above. Avoid rebuilding the same Huffman table
    // twice in the deadline-independent default floor.
    if should_price_combined_proven_submatch_candidate(rewrites.len(), individual_limit) {
        if stop.reached() {
            return None;
        }
        rewrites.sort_by_key(|rewrite| rewrite.token_index);
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
        next.sort_by_key(|state| {
            proven_composition_state_key(
                state,
                &source_literal_frequencies,
                &source_distance_frequencies,
            )
        });
        next.truncate(PROVEN_COMPOSITION_BEAM_WIDTH);
        states = next;
    }

    states.sort_by_key(|state| {
        proven_composition_state_key(
            state,
            &source_literal_frequencies,
            &source_distance_frequencies,
        )
    });
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
            .any(|candidate| candidate.as_slice() == tokens.as_slice())
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

/// Re-rank the compact proven-spelling menu after every strict exact win.
///
/// The search shape is independently adapted from Guetzli's closed loop: rank
/// local changes under the current global model, exact-price change batches,
/// then sweep backwards from the aggressive endpoint. Columbo never relaxes
/// decoded identity, and this additive sibling can replace its completed
/// incumbent only on a strict complete-block bit win.
#[allow(clippy::too_many_arguments)]
fn consider_closed_loop_proven_composition(
    block: &ParsedBlock,
    alignment: u8,
    options: &Options,
    priced_candidates: &mut Vec<Arc<Vec<Token>>>,
    stop: &mut SearchStop<'_>,
    best: &mut PlannedBlock,
) {
    // This route buys its observed marginal win on tiny Huffman-sensitive
    // blocks. Keeping it in that regime prevents its exact-pricing probes from
    // displacing later Max work under a shared container deadline.
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
    ranked.sort_by_key(|&(key, movement)| (key, movement.menu_index, movement.choice));
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
    ranked.sort_by_key(|&(key, movement)| (key, movement.menu_index, movement.choice));
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
        .any(|candidate| candidate.as_slice() == tokens.as_slice());
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
    ranked.sort_by_key(|target| target.rank.key());

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
    menus.sort_by_key(|menu| menu.token_index);
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

    let mut costs = [u64::MAX; 259];
    let mut choices = [ProvenSubmatchChoice::End; 259];
    costs[decoded.len()] = 0;
    let mut visited_edges = 0_usize;

    for start in (0..decoded.len()).rev() {
        let mut best_cost = u64::MAX;
        let mut best_choice = ProvenSubmatchChoice::End;

        // Retain the exact source token as the first whole-span edge. Strict
        // comparisons below preserve it on an estimated-cost tie.
        if start == 0 && forbidden_length_symbol != Some(length_symbol) {
            best_cost = estimated_match_token_bits(source, literal_lengths, distance_lengths)?;
            best_choice = ProvenSubmatchChoice::Match(source);
        }

        let literal_cost = estimated_length(literal_lengths, usize::from(decoded[start]))
            .checked_add(costs[start + 1])?;
        if literal_cost < best_cost {
            best_cost = literal_cost;
            best_choice = ProvenSubmatchChoice::Literal(decoded[start]);
        }

        for match_length in 3..=decoded.len() - start {
            visited_edges = visited_edges.checked_add(1)?;
            if visited_edges & 31 == 0 && stop.reached() {
                return None;
            }
            let match_length: u16 = match_length.try_into().ok()?;
            let token = repacked_match(source, match_length)?;
            let Token::Match { length_symbol, .. } = token else {
                return None;
            };
            if forbidden_length_symbol == Some(length_symbol) {
                continue;
            }
            let end = start.checked_add(usize::from(match_length))?;
            let candidate = estimated_match_token_bits(token, literal_lengths, distance_lengths)?
                .checked_add(costs[end])?;
            if candidate < best_cost {
                best_cost = candidate;
                best_choice = ProvenSubmatchChoice::Match(token);
            }
        }
        costs[start] = best_cost;
        choices[start] = best_choice;
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
    rewrites.sort_by_key(|rewrite| rewrite.token_index);
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
        .any(|candidate| candidate.as_slice() == tokens.as_slice())
    {
        return;
    }
    if let Some(planned_tokens) =
        consider_proven_submatch_tokens(block, tokens, alignment, options, stop, best)
    {
        // Capacity is preflighted once per pass. If an unusual allocator
        // failure still prevents retaining the exact identity witness, exact
        // pricing remains correct; only duplicate suppression is lost.
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

    // Generated length 258 is canonical. Relaxed mode retains the explicit
    // non-standard sibling only when complete exact pricing proves it smaller.
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

/// Complete the cheap token-preserving floor even when a container's shared
/// search deadline has already elapsed.
///
/// This is intentionally much smaller than [`plan_block_with_search`]: it
/// prices same-distance runs and a bounded set of match-to-literal rewrites
/// against the source, best, and fixed trees. Compact block lists may request
/// Columbo's `extended` feedback-tree/replay floor. Proven-submatch
/// resegmentation is evaluated separately at stream scope so its replay fixed
/// point cannot displace this floor. There are no beams, match-group
/// combinations, or newly discovered LZ77 matches. The bound lets ZIP/APNG
/// give every member one useful pass before optional search time is
/// concentrated on harder streams.
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

/// Add the bounded token-preserving floor to an already-priced base block.
///
/// Callers that compose several deterministic candidate families can price
/// the ordinary stored/fixed/dynamic representations once, then pass that
/// complete plan through each family. Every comparison remains strict, so
/// the earlier candidate still wins a tie exactly as it did when the families
/// each built their own identical base plan.
pub(crate) fn improve_plan_with_floor(
    block: &ParsedBlock,
    alignment: u8,
    options: &Options,
    extended: bool,
    best: PlannedBlock,
) -> PlannedBlock {
    improve_plan_with_floor_policy(block, alignment, options, extended, false, best)
}

/// Compose proven-submatch resegmentation inside the bounded source floor.
///
/// The ordinary stream planner uses this only when a preceding same-distance
/// repartition makes the transformations dependent. A separate compact route
/// may also use it to preserve the historical integrated-before-feedback fixed
/// point without letting a local token choice displace the ordinary lineage.
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
        if normalized.as_slice() != block.tokens.as_slice() {
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
            if aliased.as_slice() != block.tokens.as_slice() {
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

    // Columbo reserves its exact-Defluff-tree/hybrid-tree feedback seeds and
    // terminal replay for compact block lists. Applying this composite floor
    // to every photographic block would overrun the caller's budget before
    // grouping.
    if !extended {
        return best;
    }
    // Columbo's bounded cumulative symbol bands are inspired by deft4j's
    // repeated least-family pruning, but are not named deft4j states. Each band
    // replaces source matches whose length symbol falls in one selected prefix.
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

    // One replay is enough to carry a strict intermediate expansion into a
    // finished adjacent merge and to repack its terminal header. The replay
    // remains bounded by the same token/model limits as the first pass.
    if best.tokens != block.tokens {
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

/// Add Columbo's deft4j-tree hybrid to one already-priced block.
///
/// The ordinary source-boundary floor uses this on at most 28 merged ranges;
/// fragmented replay admits at most 66. It applies the recovered deft4j tree
/// builder to both alphabets and keeps its two whole-block recodes, but omits
/// Columbo's ranked single-match trials. For zero or one used distance symbol,
/// deft4j's payload caller bypasses that builder, so this floor remains a
/// Columbo/deft4j hybrid. The work is deadline-independent and only removes
/// source-supplied matches.
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

/// Price Columbo's six cumulative length-symbol-band candidates.
///
/// The original Columbo C implementation introduced this bounded family after
/// studying deft4j's least-family pruning. It is a Columbo extension rather
/// than a reconstruction of deft4j's ordered state graph.
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

/// Apply Columbo's bounded feedback/replay floor to a finished Huffman plan.
///
/// Stream search compares several complete block layouts. A cheaper layout can
/// reach the deadline before its final blocks receive optional feedback, so a
/// per-source floor alone is not composable: the best header for each selected
/// block may live in a different candidate. This bounded terminal pass prices
/// one exact-Defluff-tree, one Columbo/Defluff hybrid tree, and one strict
/// DeflOpt-style token spelling. Defluff itself has no terminal replay.
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

    // This Columbo terminal pass applies DeflOpt's strict table-replay
    // primitive: write every strictly cheaper existing match as literals, then
    // rebuild once. It cannot discover a new LZ77 match.
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
    // Huffman representations do not depend on the incoming bit alignment.
    // Ignore a stored result here because its padding was priced at the dummy
    // alignment; the stream planner has already retained the valid stored form.
    let mut candidate = plan_block(&transformed, 0, &floor_options, &mut SearchStop::never());
    let seeds = feedback_tree_seeds(&transformed, options.strict);
    consider_feedback_seed_trees(&transformed, &floor_options, &seeds, &mut candidate);
    if !matches!(candidate.representation, Representation::Stored) && candidate.bits < plan.bits {
        *plan = candidate;
    }
}

/// Replay the complete bounded floor once from an already-selected plan.
///
/// Rebuilding a table can make a second set of existing matches strictly
/// dearer than their decoded literals. Container streams may exhaust their
/// shared deadline before the ordinary search reaches that second state, so
/// the stream planner uses this helper once for its complete merged range.
/// The current Huffman tree is retained as a seed, matching a parse of the
/// emitted plan; no replacement or newly discovered LZ77 match is introduced.
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

/// Follow the selected Huffman table through a short fixed-point ladder.
///
/// Each pass spells only the existing matches that are no cheaper than their
/// decoded literals under the current table, then rebuilds that table. A
/// non-winning pass remains a useful intermediate because its frequencies can
/// make the following pass smaller. Four passes cover useful states observed in
/// the original Columbo C implementation; the ladder itself is a Columbo route.
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

/// Reconstruct the parsed metadata for a selected plan without copying its
/// immutable payload. Keeping its current tree as the source tree lets replay
/// price that exact table against each transformed token stream.
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

/// Price Columbo's bounded cumulative length-symbol-family states.
///
/// Some merged photographic blocks get a smaller table only after all
/// existing matches in symbol 260, then symbols 260..=261, and so on through
/// symbol 264 are written as literals. The family is inspired by repeated
/// deft4j least-family pruning, but the fixed cumulative bands are Columbo's.
#[cfg(test)]
pub(crate) fn plan_block_with_short_family_floor(
    block: &ParsedBlock,
    alignment: u8,
    options: &Options,
) -> PlannedBlock {
    let mut floor_options = options.clone();
    floor_options.exhaustive = false;
    let base = plan_block(block, alignment, &floor_options, &mut SearchStop::never());
    let base = improve_plan_with_same_distance_floor(block, alignment, &floor_options, base);
    improve_plan_with_short_family_floor(block, &floor_options, base)
}

/// Add the five cumulative short-length candidates to an existing base plan.
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

/// Additive frequency effects of expanding exact length symbols 260..=264.
/// Keeping this summary per source block lets stream grouping score many
/// ranges without repeatedly copying or rescanning their token vectors.
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

    // Stage zero prices the unchanged grouped token stream. Each later stage
    // cumulatively expands one more exact short-length symbol, matching the
    // five concrete token candidates materialized after segmentation.
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
    let mut distance_frequencies = block.distance_frequencies;
    ensure_floor_distance_symbols(&mut distance_frequencies, options.strict);
    // Emulate the reference OpenJDK `PriorityQueue` heap operations. deft4j's
    // comparator has no secondary tie key, and `PriorityQueue` does not specify
    // equal-weight ordering; this implementation fixes the recovered reference
    // heap behavior and prices it once.
    let literal = make_lengths_deft4j_java_heap(&block.literal_frequencies, 15);
    // Columbo's compact floor applies deft4j's raw `HuffmanTree` mechanics to
    // both alphabets. deft4j's payload caller bypasses that tree for zero or
    // one used distance symbol, so this remains a Columbo/deft4j hybrid.
    let distance = make_lengths_deft4j_java_heap(&distance_frequencies, 15);
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

    // deft4j's recode pipeline prunes matches under its freshly rebuilt data
    // trees before rebuilding once more. Retain both strict and tied local
    // forms: a tie can change the next Huffman frequencies even though the
    // supplied LZ77 parse and decoded bytes remain unchanged.
    let strict = expand_matches(&block.tokens, &block.plain, &literal, &distance, false);
    let non_larger = expand_matches(&block.tokens, &block.plain, &literal, &distance, true);
    if let Some(tokens) = strict {
        let duplicate = non_larger.as_ref().is_some_and(|other| other == &tokens);
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

/// Rebuild a deft4j-pruned token state with Columbo's ordinary candidates and
/// deft4j's deterministic heap, retaining whichever complete block wins.
///
/// deft4j deliberately builds twice: its first tree decides which
/// existing matches to spell literally, and the changed frequencies feed a
/// second deft4j tree. Moving the transformed block into Columbo's planner
/// after pricing that second tree avoids another large token-vector clone.
fn consider_deft4j_rebuild(
    source: &ParsedBlock,
    tokens: Vec<Token>,
    alignment: u8,
    options: &Options,
    stop: &mut SearchStop<'_>,
    best: &mut PlannedBlock,
) {
    let Some(candidate) = try_transformed_block(source, tokens) else {
        return;
    };
    let mut distance_frequencies = candidate.distance_frequencies;
    ensure_floor_distance_symbols(&mut distance_frequencies, options.strict);
    let literal = make_lengths_deft4j_java_heap(&candidate.literal_frequencies, 15);
    let distance = make_lengths_deft4j_java_heap(&distance_frequencies, 15);
    let deft4j =
        plan_for_explicit_lengths(&candidate.tokens, &literal, &distance, options.exhaustive);

    let mut planned = plan_owned_block(candidate, alignment, options, stop);
    if let Some(deft4j) = deft4j {
        if deft4j.bits < planned.bits {
            planned.bits = deft4j.bits;
            planned.representation = Representation::Dynamic(deft4j);
        }
    }
    if planned.bits < best.bits {
        *best = planned;
    }
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

/// Continue the full token search from an already completed ordinary plan.
///
/// Stream split discovery prices the unsplit block before it ranks any
/// boundaries. Passing that exact plan back here avoids rebuilding the same
/// exhaustive Huffman representation when the token search begins.
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

/// Search one compact block with proven resegmentation before table feedback.
///
/// This preserves a distinct fixed point from the ordinary endpoint ordering.
/// Max mode runs it as an independent comparison candidate, so the locally
/// smaller seed cannot displace the completed normal route.
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

/// Continue a completed integrated floor through the same proven-first search.
///
/// This preserves the historical ordering as an additive comparison route:
/// the bounded floor establishes feedback-tree seeds, then the full search may
/// extend them without replacing the ordinary stream lineage.
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

/// Search one complete source block without stream-split or iterative siblings.
///
/// The direct source route uses the same table-feedback ladder as the ordinary
/// block planner, but deliberately leaves match-group beams, the ordered state
/// queue, and post-search replay to their independent candidates. This keeps a
/// long source-block chain moving forward instead of spending its entire wall
/// budget on the first locally interesting block.
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

/// Continue the narrow whole-block route from an independently completed
/// exact candidate while retaining the original source tokens as another
/// transformation parent.
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
    /// Price the ordinary representation before trying token transforms.
    Price,
    /// Price the ordinary representation and retain this independent sibling.
    Additional(PlannedBlock),
    /// This is the already-priced ordinary representation for the same block.
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
    // Columbo's two feedback seeds are a bounded comparison floor: one is
    // Defluff's exact builder, while the other combines Columbo's generic tree
    // with Defluff's limiter. Price both before optional byte-seeking work.
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
    // Defluff normalizes the noncanonical 258 alias on input. Columbo also
    // offers a broader opt-in reverse candidate whenever symbol 284 plus five
    // extra bits may beat symbol 285; unlike Defluff, it does not require
    // existing ordinary symbol-284 traffic.
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
            if normalized.as_slice() != block.tokens.as_slice() {
                consider_tokens(block, normalized, alignment, options, stop, &mut best);
            }
        }
    }
    if !options.strict && block.literal_frequencies[285] != 0 {
        if let Some(aliased) = rewrite_258_symbols(&block.tokens, block.plain.len(), true) {
            if aliased.as_slice() != block.tokens.as_slice() {
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
        // Every remaining family only spells existing matches as literals.
        // Feedback-tree pricing above is the only useful work on an already
        // literal-only token stream.
        return best;
    }

    // libdeflate's explicit all-literals alternative motivates retaining this
    // endpoint. Columbo supplies the bounded frequency preflight and complete
    // existing-stream pricing; it never searches for a replacement match.
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

    // Feed every locally no-larger match expansion into the table rebuilt for
    // that exact token state. Compact streams can need several strict wins in
    // succession; restarting each pass from the seed table stalls that ladder
    // after its first recode. The token count only grows, and these small caps
    // keep both ordinary and exhaustive searches predictable.
    let pass_limit = 12;
    let fixed_seed = fixed_lengths();
    for (mut literal_lengths, mut distance_lengths) in seeds {
        if stop.reached() {
            break;
        }

        // DeflOpt's first match-expansion pass uses strict wire cost. Keep
        // that state as a sibling of the non-larger ladder below: expanding
        // its zero-delta matches at the same time can change frequencies and
        // skip a smaller rebuilt table. This is one bounded candidate per
        // seed and still only spells existing matches as decoded literals.
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
                // The winning plan and the next ladder state both own this
                // token sequence. Make only that necessary duplicate, and do
                // so fallibly; the completed winner remains valid if another
                // pass cannot be allocated.
                let Some(next_seed_tokens) =
                    try_clone_token_candidate(&candidate.tokens, block.plain.len())
                else {
                    best = candidate;
                    break;
                };
                best = candidate;
                seed_tokens = next_seed_tokens;
            } else {
                // A non-winning candidate is consumed directly by the next
                // pass instead of cloning its expanded token vector.
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

            // Keep the transformation as an intermediate even when rebuilding
            // its table did not immediately beat the global best.
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

    // A strict fixed-table expansion applies Defluff's fixed-block comparison:
    // replace a source match only when its literals are strictly cheaper.
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

    // Literal-heavy encoder blocks often contain only one or two marginal
    // matches. Testing those matches independently is linear in their count
    // and is part of the ordinary structural floor, not just byte-hunting
    // max mode. Keep the default route bounded by both parsed size and the
    // number of existing matches; it never invents a replacement match.
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
    // Individual pruning is a greedy local route. Keep the state immediately
    // before it so the bounded ordered queue can also explore the sibling in
    // which those matches remain intact. This mirrors an alternate in the
    // original Columbo C scheduler without widening the queue or creating a
    // new LZ77 match. A failed clone simply retains the established path.
    let pre_individual = (try_ordered_queue && try_individual_prune && match_count != 0)
        .then(|| try_clone_planned_block(&best))
        .flatten();
    if try_individual_prune && match_count != 0 && !stop.reached() {
        individual_prune_search(block, alignment, options, stop, &mut best);
    }

    if try_ordered_queue && !stop.reached() {
        let alternate = pre_individual.filter(|alternate| alternate.tokens != best.tokens);
        let mut seen = HashSet::new();
        // Queue edges only expand matches to literals; they cannot reconstruct
        // a match removed by the greedy individual-prune floor. Search the
        // intact root first because its reachable graph is therefore a strict
        // superset. If time remains, continue from the smaller pruned root.
        // Both beams share exact visited state, so converged descendants are
        // priced once rather than once per lineage.
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

    if policy.replay && options.exhaustive && best.tokens != block.tokens && !stop.reached() {
        // Columbo's original C --max scheduler replays completed winning token
        // states through the default ladder. Use a non-exhaustive child round
        // to avoid recursive route multiplication while retaining that
        // fixed-point behavior.
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
    /// The incumbent can carry a specialized or exact source table, while the
    /// queue historically begins from its ordinary representation.
    Unknown,
    /// A child already owns the ordinary table built by `plan_tokens`.
    Lengths { literal: Vec<u8>, distance: Vec<u8> },
    /// A stored child has no Huffman descendants, but still occupies its
    /// historical beam slot at the next depth.
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
    // Every edge expands at least one match of length three or more. Seed the
    // shared exact history with this root so a later alternate queue cannot
    // reprice it or any child already visited here. A linear history made every
    // new state rescan up to hundreds of earlier multi-thousand-token vectors,
    // wasting most of a max route on duplicate detection. `HashSet` still
    // confirms full token equality after hashing, so collisions cannot
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
                    // The queue keeps its own tokens and table below, so the
                    // complete plan can become the incumbent without cloning.
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
        next.sort_by_key(|state| state.bits);
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

/// Build Columbo's two bounded strict-feedback seeds.
///
/// The exact seed reproduces Defluff's complete two-queue/package-list
/// builder. The hybrid seed retains Columbo's generic ordinary tree and uses
/// only Defluff's package-list limiter when that tree exceeds the depth limit.
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

/// Apply Columbo's Defluff-derived two-generation rescan topology.
///
/// One seed uses Defluff's exact tree builder and the other is a
/// Columbo/Defluff hybrid. Defluff supplies the strict fresh/adjusted rescan
/// shape, but Columbo sends each token set through its broader planner instead
/// of scoring Defluff's supplied tables and exact four-pass header directly.
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
        // Defluff rescans the original tokens under its adjusted tree. A zero
        // mark second pass is meaningful only to the exact explicit-tree
        // scorer; the broad planner already considered the unchanged source.
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

/// Price the bounded cumulative length families used by the direct no-split
/// route on large, moderately tokenized source blocks.
///
/// These candidates are structural match removals, not fixture heuristics:
/// they cover progressively longer short-match alphabets plus one sparse
/// longer-match set. Each candidate is rebuilt and must strictly beat the
/// complete incumbent before it can affect output.
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

        // Ordered-queue searches retain combinations of the locally cheapest
        // length/distance families. Test the first few prefixes directly.
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
    groups.sort_by_key(|group| (group.delta, group.count, group.symbol));
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

/// Build the owned block needed to price one optional token transformation.
/// Large vectors and source metadata are copied fallibly; exact original bits
/// are cleared because they describe the pre-transformation token stream.
fn try_transformed_block(source: &ParsedBlock, tokens: Vec<Token>) -> Option<ParsedBlock> {
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

/// Allocate one optional token candidate within an explicit byte ceiling.
/// Returning `None` is deliberately non-fatal: every caller already has a
/// complete source or best plan to keep when a transformation is too large or
/// the allocator cannot satisfy it.
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

/// Exact-price the all-literals endpoint before allocating its token vector.
///
/// The ordinary dynamic planner depends only on symbol frequencies and match
/// extra bits. An all-literals stream has no extras, so its fixed and dynamic
/// representations can be decided from the decoded bytes alone. The much
/// larger token vector is retained only if that complete block is a strict win.
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

    // One tree and one greedy header are enough to reject endpoints that are
    // nowhere near the incumbent. Leave a small header-sized margin so the
    // complete planner can still recover rare sub-byte and one-byte wins.
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

/// Expand exactly the matches selected by `should_expand`.
///
/// The first pass computes the exact resulting token count with checked
/// arithmetic. Only then do we reserve the complete vector fallibly and make
/// a second, allocation-free pass to populate it. A malformed internal
/// token/plain pairing is treated like any other unusable optional candidate.
fn expand_selected_matches<F>(
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
    // Exact source bits can remain the block winner even though deleting one
    // costly match and rebuilding the table would improve it. In that case
    // there is no tree on `best`, so use the decoded source tree as the cost
    // seed rather than silently skipping the ordinary prune route.
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
                // Capacity is already reserved. Remove the current worst
                // candidate before inserting the newly ranked one.
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
mod tests {
    use super::*;
    use crate::deflate::model::{
        token_extra_bits, OriginalBits, SourceBlockType, LENGTH_BASE, LENGTH_EXTRA_BITS,
    };

    fn test_match(
        length: u16,
        distance: u16,
        distance_symbol: u8,
        distance_extra: u16,
        distance_extra_bits: u8,
    ) -> Token {
        let (length_symbol, length_extra, length_extra_bits) =
            canonical_length_encoding(length).expect("test length is legal");
        Token::Match {
            length,
            distance,
            length_symbol,
            distance_symbol,
            length_extra,
            distance_extra,
            length_extra_bits,
            distance_extra_bits,
        }
    }

    fn test_repack(
        tokens: &[Token],
        decoded_bytes: usize,
        literal_lengths: &[u8],
        exhaustive: bool,
    ) -> Option<Vec<Token>> {
        repack_same_distance_runs(
            tokens,
            decoded_bytes,
            literal_lengths,
            exhaustive,
            &mut SearchStop::never(),
        )
    }

    fn test_partitioner(
        literal_lengths: &[u8],
        max_active: usize,
        max_deficit: usize,
    ) -> Option<SameDistancePartitioner> {
        SameDistancePartitioner::new(
            literal_lengths,
            max_active,
            max_deficit,
            &mut SearchStop::never(),
        )
    }

    fn decode_test_tokens(tokens: &[Token]) -> Option<Vec<u8>> {
        let decoded_len = tokens.iter().try_fold(0_usize, |total, token| {
            total.checked_add(token.decoded_len())
        })?;
        let mut decoded = Vec::new();
        decoded.try_reserve_exact(decoded_len).ok()?;
        for &token in tokens {
            match token {
                Token::Literal(value) => decoded.push(value),
                Token::Match {
                    length, distance, ..
                } => {
                    let distance = usize::from(distance);
                    if distance == 0 || distance > decoded.len() {
                        return None;
                    }
                    for _ in 0..length {
                        let source = decoded.len().checked_sub(distance)?;
                        let value = *decoded.get(source)?;
                        decoded.push(value);
                    }
                }
            }
        }
        Some(decoded)
    }

    fn assert_proven_submatch_rewrite(source: Token, replacement: &[Token], expected: &[Token]) {
        assert_eq!(replacement, expected);
        let seed: Vec<_> = b"abcdef".iter().copied().map(Token::Literal).collect();
        let mut source_tokens = seed.clone();
        source_tokens.push(source);
        let mut rewritten_tokens = seed;
        rewritten_tokens.extend_from_slice(replacement);
        assert_eq!(
            decode_test_tokens(&source_tokens),
            decode_test_tokens(&rewritten_tokens)
        );

        let Token::Match {
            distance,
            distance_symbol,
            distance_extra,
            distance_extra_bits,
            ..
        } = source
        else {
            panic!("the test source must be a match");
        };
        for token in replacement {
            if let Token::Match {
                length,
                distance: rewritten_distance,
                length_symbol,
                distance_symbol: rewritten_distance_symbol,
                length_extra,
                distance_extra: rewritten_distance_extra,
                length_extra_bits,
                distance_extra_bits: rewritten_distance_extra_bits,
            } = *token
            {
                assert_eq!(
                    (length_symbol, length_extra, length_extra_bits),
                    canonical_length_encoding(length).unwrap()
                );
                assert_eq!(
                    (
                        rewritten_distance,
                        rewritten_distance_symbol,
                        rewritten_distance_extra,
                        rewritten_distance_extra_bits,
                    ),
                    (
                        distance,
                        distance_symbol,
                        distance_extra,
                        distance_extra_bits,
                    )
                );
            }
        }
    }

    #[test]
    fn the_258_alias_rewrite_is_bidirectional_but_explicit() {
        let canonical = Token::Match {
            length: 258,
            distance: 1,
            length_symbol: 285,
            distance_symbol: 0,
            length_extra: 0,
            distance_extra: 0,
            length_extra_bits: 0,
            distance_extra_bits: 0,
        };

        let aliased = rewrite_258_symbols(&[canonical], 258, true)
            .expect("the bounded one-token rewrite fits");
        assert!(matches!(
            aliased.as_slice(),
            [Token::Match {
                length_symbol: 284,
                length_extra: 31,
                length_extra_bits: 5,
                ..
            }]
        ));

        let normalized = rewrite_258_symbols(&aliased, 258, false)
            .expect("the bounded one-token normalization fits");
        assert_eq!(normalized, [canonical]);
    }

    #[test]
    fn generated_match_lengths_are_canonical() {
        for length in 3..=258 {
            let (symbol, extra, extra_bits) =
                canonical_length_encoding(length).expect("every Deflate match length is legal");
            let index = usize::from(symbol - 257);
            assert_eq!(LENGTH_BASE[index] + extra, length);
            assert_eq!(LENGTH_EXTRA_BITS[index], extra_bits);
            assert!(extra < (1_u16 << extra_bits).max(1));
            if length == 258 {
                assert_eq!((symbol, extra, extra_bits), (285, 0, 0));
            }
        }
        assert!(canonical_length_encoding(2).is_none());
        assert!(canonical_length_encoding(259).is_none());
    }

    #[test]
    fn proven_submatch_graph_can_keep_a_literal_prefix_and_suffix_match() {
        let source = test_match(17, 6, 4, 1, 1);
        let decoded = b"abcdefabcdefabcde";
        let mut literal_lengths = [0_u8; 286];
        literal_lengths[usize::from(b'a')] = 1;
        for &byte in b"bcdef" {
            literal_lengths[usize::from(byte)] = 10;
        }
        literal_lengths[267] = 1;
        literal_lengths[268] = 15;
        let mut distance_lengths = [0_u8; 30];
        distance_lengths[4] = 1;

        let replacement = solve_proven_submatch(
            source,
            decoded,
            &literal_lengths,
            &distance_lengths,
            None,
            &mut SearchStop::never(),
        )
        .unwrap();
        assert_proven_submatch_rewrite(
            source,
            &replacement,
            &[Token::Literal(b'a'), test_match(16, 6, 4, 1, 1)],
        );
    }

    #[test]
    fn proven_submatch_graph_can_keep_a_prefix_match_and_literal_suffix() {
        let source = test_match(17, 6, 4, 1, 1);
        let decoded = b"abcdefabcdefabcde";
        let mut literal_lengths = [0_u8; 286];
        for &byte in b"abcdf" {
            literal_lengths[usize::from(byte)] = 10;
        }
        literal_lengths[usize::from(b'e')] = 1;
        literal_lengths[267] = 1;
        literal_lengths[268] = 15;
        let mut distance_lengths = [0_u8; 30];
        distance_lengths[4] = 1;

        let replacement = solve_proven_submatch(
            source,
            decoded,
            &literal_lengths,
            &distance_lengths,
            None,
            &mut SearchStop::never(),
        )
        .unwrap();
        assert_proven_submatch_rewrite(
            source,
            &replacement,
            &[test_match(16, 6, 4, 1, 1), Token::Literal(b'e')],
        );
    }

    #[test]
    fn proven_submatch_graph_can_emit_multiple_matches_at_the_proven_distance() {
        let source = test_match(17, 6, 4, 1, 1);
        let decoded = b"abcdefabcdefabcde";
        let mut literal_lengths = [0_u8; 286];
        literal_lengths.fill(15);
        literal_lengths[262] = 1;
        literal_lengths[263] = 1;
        literal_lengths[268] = 15;
        let mut distance_lengths = [0_u8; 30];
        distance_lengths[4] = 1;

        let replacement = solve_proven_submatch(
            source,
            decoded,
            &literal_lengths,
            &distance_lengths,
            None,
            &mut SearchStop::never(),
        )
        .unwrap();
        assert_proven_submatch_rewrite(
            source,
            &replacement,
            &[test_match(8, 6, 4, 1, 1), test_match(9, 6, 4, 1, 1)],
        );
    }

    #[test]
    fn proven_submatch_source_symbol_free_path_exposes_payload_ties() {
        let source = test_match(17, 6, 4, 1, 1);
        let decoded = b"abcdefabcdefabcde";
        let mut literal_lengths = [0_u8; 286];
        literal_lengths.fill(15);
        literal_lengths[usize::from(b'a')] = 1;
        literal_lengths[267] = 4;
        literal_lengths[268] = 5;
        let mut distance_lengths = [0_u8; 30];
        distance_lengths[4] = 1;

        // Both spellings cost seven estimated payload bits. The exact source
        // remains the deterministic unrestricted winner.
        assert!(solve_proven_submatch(
            source,
            decoded,
            &literal_lengths,
            &distance_lengths,
            None,
            &mut SearchStop::never(),
        )
        .is_none());

        let replacement = solve_proven_submatch(
            source,
            decoded,
            &literal_lengths,
            &distance_lengths,
            Some(268),
            &mut SearchStop::never(),
        )
        .unwrap();
        assert_proven_submatch_rewrite(
            source,
            &replacement,
            &[Token::Literal(b'a'), test_match(16, 6, 4, 1, 1)],
        );
    }

    #[test]
    fn proven_submatch_graph_handles_overlap_and_deadline_expiry() {
        let source = test_match(258, 1, 0, 0, 0);
        let decoded = [b'A'; 258];
        let mut literal_lengths = [0_u8; 286];
        literal_lengths[usize::from(b'A')] = 1;
        literal_lengths[285] = 15;
        let mut distance_lengths = [0_u8; 30];
        distance_lengths[0] = 1;
        let replacement = solve_proven_submatch(
            source,
            &decoded,
            &literal_lengths,
            &distance_lengths,
            Some(285),
            &mut SearchStop::never(),
        )
        .unwrap();
        let seed = [Token::Literal(b'A')];
        let mut source_tokens = seed.to_vec();
        source_tokens.push(source);
        let mut rewritten_tokens = seed.to_vec();
        rewritten_tokens.extend_from_slice(&replacement);
        assert_eq!(
            decode_test_tokens(&source_tokens),
            decode_test_tokens(&rewritten_tokens)
        );

        let mut checks = 0_usize;
        let mut expires = || {
            checks += 1;
            checks > 2
        };
        let result = solve_proven_submatch(
            source,
            &decoded,
            &literal_lengths,
            &distance_lengths,
            None,
            &mut SearchStop::callback(&mut expires),
        );
        assert!(result.is_none());
        assert!(checks > 2);
    }

    #[test]
    fn compact_proven_feedback_uses_token_and_plain_work_bounds() {
        let matching = [test_match(4, 1, 0, 0, 0)];
        assert!(compact_proven_submatch_route_eligible(&matching, 4));
        assert!(!compact_proven_submatch_route_eligible(
            &[Token::Literal(0)],
            1
        ));
        assert!(!compact_proven_submatch_route_eligible(
            &vec![matching[0]; COMPACT_PROVEN_SUBMATCH_TOKENS + 1],
            4
        ));
        assert!(!compact_proven_submatch_route_eligible(
            &matching,
            MAX_PROVEN_SUBMATCH_FULL_PLAIN + 1
        ));
    }

    #[test]
    fn proven_submatch_default_selection_is_ranked_and_bounded() {
        let common = test_match(39, 1, 0, 0, 0);
        let highest = test_match(258, 1, 0, 0, 0);
        let mut tokens = vec![common; 12];
        tokens.push(highest);
        let decoded_bytes: usize = tokens.iter().map(|token| token.decoded_len()).sum();
        let (literal_frequencies, _) = count_frequencies(&tokens);

        let default = select_proven_submatch_targets(
            &tokens,
            decoded_bytes,
            &[],
            &literal_frequencies,
            &FIXED_LITERAL_CODE_LENGTHS,
            false,
            false,
            &mut SearchStop::never(),
        )
        .unwrap();
        assert!(default.len() <= DEFAULT_PROVEN_SUBMATCH_TARGETS);
        assert_eq!(default[0].token_index, 12);
        assert!(default.iter().all(|target| target.token_index == 12));

        let exhaustive = select_proven_submatch_targets(
            &tokens,
            decoded_bytes,
            &[],
            &literal_frequencies,
            &FIXED_LITERAL_CODE_LENGTHS,
            true,
            true,
            &mut SearchStop::never(),
        )
        .unwrap();
        assert_eq!(exhaustive.len(), tokens.len());
        assert_eq!(
            exhaustive
                .iter()
                .map(|target| target.token_index)
                .collect::<Vec<_>>(),
            (0..tokens.len()).collect::<Vec<_>>()
        );
    }

    #[test]
    fn proven_submatch_transition_targeting_checks_both_code_edges() {
        let symbol = match test_match(39, 1, 0, 0, 0) {
            Token::Match { length_symbol, .. } => length_symbol,
            Token::Literal(_) => unreachable!(),
        };
        assert!(length_near_code_transition(37, symbol));
        assert!(!length_near_code_transition(38, symbol));
        assert!(!length_near_code_transition(40, symbol));
        assert!(length_near_code_transition(41, symbol));
    }

    #[test]
    fn proven_submatch_selection_reserves_space_across_length_symbols() {
        let common_highest = test_match(258, 1, 0, 0, 0);
        let rare_lower = test_match(17, 1, 0, 0, 0);
        let mut tokens = vec![common_highest; 12];
        tokens.push(rare_lower);
        let decoded_bytes: usize = tokens.iter().map(|token| token.decoded_len()).sum();
        let (literal_frequencies, _) = count_frequencies(&tokens);

        let targets = select_proven_submatch_targets(
            &tokens,
            decoded_bytes,
            &[],
            &literal_frequencies,
            &FIXED_LITERAL_CODE_LENGTHS,
            false,
            false,
            &mut SearchStop::never(),
        )
        .unwrap();
        assert!(targets.len() <= DEFAULT_PROVEN_SUBMATCH_TARGETS);
        assert!(targets.iter().any(|target| target.token_index == 12));
        assert!(
            targets
                .iter()
                .filter(|target| {
                    matches!(
                        target.source,
                        Token::Match {
                            length_symbol: 285,
                            ..
                        }
                    )
                })
                .count()
                <= DEFAULT_PROVEN_SUBMATCH_TARGETS_PER_SYMBOL
        );
    }

    #[test]
    fn proven_submatch_single_rewrite_skips_a_duplicate_combined_price() {
        assert!(!should_price_combined_proven_submatch_candidate(0, 0));
        assert!(!should_price_combined_proven_submatch_candidate(1, 1));
        assert!(!should_price_combined_proven_submatch_candidate(1, 4));
        assert!(should_price_combined_proven_submatch_candidate(1, 0));
        assert!(should_price_combined_proven_submatch_candidate(2, 1));
    }

    #[test]
    fn proven_submatch_materialization_rejects_invalid_rewrite_sets() {
        let source = test_match(17, 6, 4, 1, 1);
        let tokens = [source, source];
        let rank = ProvenSubmatchRank {
            highest: false,
            rare: false,
            near_boundary: false,
            transition: false,
            expensive: false,
            code_bits: 0,
            frequency: 0,
            token_index: 0,
        };
        let first = ProvenSubmatchRewrite {
            token_index: 0,
            replacement: vec![source],
            estimated_saving: 0,
            rank,
        };
        let duplicate = ProvenSubmatchRewrite {
            token_index: 0,
            replacement: vec![source],
            estimated_saving: 0,
            rank,
        };
        assert!(apply_proven_submatch_rewrites(&tokens, 34, &[first, duplicate]).is_none());

        let wrong_length = ProvenSubmatchRewrite {
            token_index: 0,
            replacement: vec![test_match(16, 6, 4, 1, 1)],
            estimated_saving: 0,
            rank,
        };
        assert!(apply_proven_submatch_rewrites(&tokens, 34, &[wrong_length]).is_none());
    }

    #[test]
    fn proven_composition_frequency_state_matches_materialized_tokens() {
        let source_match = test_match(6, 6, 4, 1, 1);
        let mut source: Vec<_> = b"abcdef".iter().copied().map(Token::Literal).collect();
        source.extend([source_match, source_match]);
        let decoded = b"abcdefabcdefabcdef";
        let rank = ProvenSubmatchRank {
            highest: true,
            rare: true,
            near_boundary: false,
            transition: true,
            expensive: false,
            code_bits: 4,
            frequency: 2,
            token_index: 6,
        };
        let literal_replacement: Vec<_> = b"abcdef".iter().copied().map(Token::Literal).collect();
        let menus = vec![
            ProvenCompositionMenu {
                token_index: 6,
                source: source_match,
                alternatives: vec![ProvenSubmatchRewrite {
                    token_index: 6,
                    replacement: literal_replacement.clone(),
                    estimated_saving: -3,
                    rank,
                }],
            },
            ProvenCompositionMenu {
                token_index: 7,
                source: source_match,
                alternatives: vec![ProvenSubmatchRewrite {
                    token_index: 7,
                    replacement: literal_replacement,
                    estimated_saving: -3,
                    rank: ProvenSubmatchRank {
                        token_index: 7,
                        ..rank
                    },
                }],
            },
        ];
        let (literal_frequencies, distance_frequencies) = count_frequencies(&source);
        let mut state = ProvenCompositionState {
            literal_frequencies,
            distance_frequencies,
            extra_bits: token_extra_bits(&source),
            estimated_delta: 6,
            choices: [1, 1, 0, 0, 0, 0, 0, 0],
            rewrite_count: 2,
        };
        for menu in &menus {
            assert!(apply_proven_composition_frequency_delta(
                &mut state,
                menu.source,
                &menu.alternatives[0].replacement,
            ));
        }

        let materialized = apply_proven_composition_state(&source, decoded.len(), &menus, &state)
            .expect("the two nonoverlapping menu choices materialize");
        assert_eq!(
            decode_test_tokens(&materialized).as_deref(),
            Some(decoded.as_slice())
        );
        assert_eq!(
            count_frequencies(&materialized).0,
            state.literal_frequencies
        );
        assert_eq!(
            count_frequencies(&materialized).1,
            state.distance_frequencies
        );
        assert_eq!(token_extra_bits(&materialized), state.extra_bits);
    }

    #[test]
    fn proven_composition_forward_and_reverse_moves_restore_the_exact_state() {
        let source_match = test_match(6, 6, 4, 1, 1);
        let mut source: Vec<_> = b"abcdef".iter().copied().map(Token::Literal).collect();
        source.extend([source_match, source_match, source_match]);
        let decoded = b"abcdefabcdefabcdefabcdef";
        let literal_replacement: Vec<_> = b"abcdef".iter().copied().map(Token::Literal).collect();
        let menus: Vec<_> = (0..3)
            .map(|index| {
                let token_index = 6 + index;
                ProvenCompositionMenu {
                    token_index,
                    source: source_match,
                    alternatives: vec![ProvenSubmatchRewrite {
                        token_index,
                        replacement: literal_replacement.clone(),
                        estimated_saving: index as i64 - 1,
                        rank: ProvenSubmatchRank {
                            highest: true,
                            rare: true,
                            near_boundary: false,
                            transition: true,
                            expensive: false,
                            code_bits: 4,
                            frequency: 3,
                            token_index,
                        },
                    }],
                }
            })
            .collect();

        let (root, moves) = ranked_forward_proven_composition_moves(&source, &menus)
            .expect("the three independent moves are rankable");
        assert_eq!(moves.len(), 3);
        let mut aggressive = root.clone();
        for &movement in &moves {
            assert!(apply_proven_composition_move(
                &mut aggressive,
                &menus,
                movement,
                true,
            ));
        }
        let materialized =
            apply_proven_composition_state(&source, decoded.len(), &menus, &aggressive)
                .expect("the aggressive endpoint materializes");
        assert_eq!(
            decode_test_tokens(&materialized).as_deref(),
            Some(decoded.as_slice())
        );
        assert_eq!(
            count_frequencies(&materialized).0,
            aggressive.literal_frequencies
        );
        assert_eq!(
            count_frequencies(&materialized).1,
            aggressive.distance_frequencies
        );
        assert_eq!(token_extra_bits(&materialized), aggressive.extra_bits);

        let (literal_lengths, distance_lengths) = fixed_lengths();
        let reverse = ranked_reverse_proven_composition_moves(
            &menus,
            &moves,
            &aggressive,
            &literal_lengths,
            &distance_lengths,
        )
        .expect("the aggressive endpoint has a reverse ranking");
        assert_eq!(reverse.len(), moves.len());
        let mut repaired = aggressive;
        for movement in reverse {
            assert!(apply_proven_composition_move(
                &mut repaired,
                &menus,
                movement,
                false,
            ));
        }
        assert_eq!(repaired.literal_frequencies, root.literal_frequencies);
        assert_eq!(repaired.distance_frequencies, root.distance_frequencies);
        assert_eq!(repaired.extra_bits, root.extra_bits);
        assert_eq!(repaired.estimated_delta, root.estimated_delta);
        assert_eq!(repaired.choices, root.choices);
        assert_eq!(repaired.rewrite_count, root.rewrite_count);
    }

    #[test]
    fn closed_loop_proven_composition_retains_a_complete_strict_incumbent() {
        let source_match = test_match(6, 6, 4, 1, 1);
        let mut source: Vec<_> = b"abcdef".iter().copied().map(Token::Literal).collect();
        source.extend([source_match, source_match, source_match]);
        let plain = decode_test_tokens(&source).expect("the source token stream is valid");
        let block = short_family_test_block(source.clone(), plain);
        let mut incumbent = PlannedBlock {
            tokens: source.clone().into(),
            plain: block.plain.clone(),
            representation: Representation::Fixed,
            bits: 0,
            source_type: SourceBlockType::Fixed,
        };
        let options = Options {
            exhaustive: true,
            ..Options::default()
        };
        consider_closed_loop_proven_composition(
            &block,
            0,
            &options,
            &mut Vec::new(),
            &mut SearchStop::never(),
            &mut incumbent,
        );
        assert_eq!(incumbent.bits, 0);
        assert_eq!(incumbent.tokens.as_slice(), source);
    }

    #[test]
    fn proven_composition_deduplicates_header_equivalent_spellings() {
        let left = [Token::Literal(b'a'), Token::Literal(b'b')];
        let right = [Token::Literal(b'b'), Token::Literal(b'a')];
        assert_ne!(left, right);
        assert!(same_proven_composition_spelling_cost(&left, &right));

        let source = test_match(6, 6, 4, 1, 1);
        let mut alternatives = Vec::new();
        let rank = ProvenSubmatchRank {
            highest: true,
            rare: true,
            near_boundary: false,
            transition: false,
            expensive: false,
            code_bits: 4,
            frequency: 1,
            token_index: 0,
        };
        for replacement in [left.to_vec(), right.to_vec()] {
            insert_distinct_proven_composition_alternative(
                &mut alternatives,
                source,
                ProvenSubmatchRewrite {
                    token_index: 0,
                    replacement,
                    estimated_saving: 0,
                    rank,
                },
            );
        }
        assert_eq!(alternatives.len(), 1);
    }

    #[test]
    fn proven_submatch_adoption_requires_a_strict_exact_bit_win() {
        let source = test_match(17, 6, 4, 1, 1);
        let mut source_tokens: Vec<_> = b"abcdef".iter().copied().map(Token::Literal).collect();
        source_tokens.push(source);
        let plain = decode_test_tokens(&source_tokens).unwrap();
        let block = short_family_test_block(source_tokens.clone(), plain);

        let mut candidate_tokens: Vec<_> = b"abcdef".iter().copied().map(Token::Literal).collect();
        candidate_tokens.push(Token::Literal(b'a'));
        candidate_tokens.push(test_match(16, 6, 4, 1, 1));
        assert_eq!(
            decode_test_tokens(&source_tokens),
            decode_test_tokens(&candidate_tokens)
        );

        let options = Options::default();
        let candidate = plan_tokens(
            &block,
            candidate_tokens.clone(),
            0,
            &options,
            &mut SearchStop::never(),
        )
        .unwrap();
        let incumbent = |bits| PlannedBlock {
            tokens: block.tokens.clone(),
            plain: block.plain.clone(),
            representation: Representation::Fixed,
            bits,
            source_type: SourceBlockType::Fixed,
        };

        let mut tie = incumbent(candidate.bits);
        let _ = consider_proven_submatch_tokens(
            &block,
            candidate_tokens.clone(),
            0,
            &options,
            &mut SearchStop::never(),
            &mut tie,
        );
        assert_eq!(tie.tokens.as_slice(), source_tokens);

        let mut strict_win = incumbent(candidate.bits + 1);
        let _ = consider_proven_submatch_tokens(
            &block,
            candidate_tokens.clone(),
            0,
            &options,
            &mut SearchStop::never(),
            &mut strict_win,
        );
        assert_eq!(strict_win.bits, candidate.bits);
        assert_eq!(strict_win.tokens.as_slice(), candidate_tokens);

        let mut priced_candidates = Vec::new();
        let mut deduplicated = incumbent(candidate.bits + 1);
        consider_unique_proven_submatch_tokens(
            &block,
            candidate_tokens.clone(),
            0,
            &options,
            &mut priced_candidates,
            &mut SearchStop::never(),
            &mut deduplicated,
        );
        assert_eq!(priced_candidates.len(), 1);
        consider_unique_proven_submatch_tokens(
            &block,
            candidate_tokens,
            0,
            &options,
            &mut priced_candidates,
            &mut SearchStop::never(),
            &mut deduplicated,
        );
        assert_eq!(priced_candidates.len(), 1);
        assert_eq!(deduplicated.bits, candidate.bits);
    }

    #[test]
    fn same_distance_stats_count_maximal_runs() {
        let tokens = vec![
            Token::Literal(b'x'),
            test_match(3, 1, 0, 0, 0),
            test_match(4, 1, 0, 0, 0),
            test_match(5, 2, 1, 0, 0),
            Token::Literal(b'y'),
            test_match(130, 3, 2, 0, 0),
            test_match(130, 3, 2, 0, 0),
            // Distances five and six share symbol four, but are not one run.
            test_match(3, 5, 4, 0, 1),
            test_match(3, 6, 4, 1, 1),
        ];
        let plain_len: usize = tokens.iter().map(|token| token.decoded_len()).sum();
        let block = short_family_test_block(tokens, vec![0; plain_len]);

        assert_eq!(
            same_distance_opportunities(&[block]),
            SameDistanceOpportunities {
                runs: 2,
                matches: 4,
                decoded_bytes: 267,
                coalescible_runs: 1,
                repartition_runs: 1,
                tokens_removable: 1,
            }
        );
    }

    #[test]
    fn same_distance_stats_join_source_blocks_but_skip_unchanged_minimum_runs() {
        let first = short_family_test_block(vec![test_match(100, 1, 0, 0, 0)], vec![0; 100]);
        let empty = short_family_test_block(Vec::new(), Vec::new());
        let last = short_family_test_block(
            vec![
                test_match(158, 1, 0, 0, 0),
                Token::Literal(b'x'),
                test_match(258, 2, 1, 0, 0),
                test_match(258, 2, 1, 0, 0),
            ],
            vec![0; 675],
        );

        assert_eq!(
            same_distance_opportunities(&[first, empty, last]),
            SameDistanceOpportunities {
                runs: 2,
                matches: 4,
                decoded_bytes: 774,
                coalescible_runs: 1,
                repartition_runs: 0,
                tokens_removable: 1,
            }
        );
    }

    #[test]
    fn same_distance_repacking_coalesces_a_short_run() {
        let source = [test_match(3, 5, 4, 0, 1), test_match(4, 5, 4, 0, 1)];
        let repacked = test_repack(&source, 7, &FIXED_LITERAL_CODE_LENGTHS, false).unwrap();
        assert_eq!(
            repacked,
            [Token::Match {
                length: 7,
                distance: 5,
                length_symbol: 261,
                distance_symbol: 4,
                length_extra: 0,
                distance_extra: 0,
                length_extra_bits: 0,
                distance_extra_bits: 1,
            }]
        );

        let canonical_258 = [test_match(100, 1, 0, 0, 0), test_match(158, 1, 0, 0, 0)];
        assert!(matches!(
            test_repack(&canonical_258, 258, &FIXED_LITERAL_CODE_LENGTHS, false,)
                .unwrap()
                .as_slice(),
            [Token::Match {
                length: 258,
                length_symbol: 285,
                length_extra: 0,
                length_extra_bits: 0,
                ..
            }]
        ));
    }

    #[test]
    fn direct_same_distance_coalescing_does_not_enter_the_dp() {
        let source = [test_match(100, 1, 0, 0, 0), test_match(158, 1, 0, 0, 0)];
        let mut dp_must_not_poll = || -> bool { panic!("direct coalescing entered the DP") };
        let mut stop = SearchStop::callback(&mut dp_must_not_poll);
        let repacked =
            repack_same_distance_runs(&source, 258, &FIXED_LITERAL_CODE_LENGTHS, false, &mut stop)
                .unwrap();
        assert_eq!(repacked.len(), 1);
    }

    #[test]
    fn default_repacking_bounds_pathological_dp_depth_and_max_polls_deadline() {
        let source = vec![test_match(257, 1, 0, 0, 0); 257];
        let decoded_bytes = 257 * 257;
        let mut default_must_not_poll = || -> bool { panic!("default fallback entered the DP") };
        let mut default_stop = SearchStop::callback(&mut default_must_not_poll);
        let repacked = repack_same_distance_runs(
            &source,
            decoded_bytes,
            &FIXED_LITERAL_CODE_LENGTHS,
            false,
            &mut default_stop,
        )
        .unwrap();
        assert_eq!(repacked.len(), 257);
        assert_eq!(
            repacked
                .iter()
                .map(|token| token.decoded_len())
                .sum::<usize>(),
            decoded_bytes
        );

        let mut polls = 0_usize;
        let mut expires_during_dp = || {
            polls += 1;
            polls > 3
        };
        let mut expiring_stop = SearchStop::callback(&mut expires_during_dp);
        assert!(repack_same_distance_runs(
            &source,
            decoded_bytes,
            &FIXED_LITERAL_CODE_LENGTHS,
            true,
            &mut expiring_stop,
        )
        .is_none());
        assert!(polls > 3);
    }

    #[test]
    fn same_distance_partitioning_handles_small_remainders() {
        for total in [259_usize, 260] {
            let left = total / 2;
            let source = [
                test_match(left as u16, 1, 0, 0, 0),
                test_match((total - left) as u16, 1, 0, 0, 0),
            ];
            let repacked = test_repack(&source, total, &FIXED_LITERAL_CODE_LENGTHS, false).unwrap();
            assert_eq!(repacked.len(), 2);
            assert!(repacked
                .iter()
                .all(|token| (3..=258).contains(&token.decoded_len())));
            assert_eq!(
                repacked
                    .iter()
                    .map(|token| token.decoded_len())
                    .sum::<usize>(),
                total
            );
        }
    }

    #[test]
    fn same_distance_partitioning_is_legal_for_all_small_totals() {
        let partitioner = test_partitioner(&FIXED_LITERAL_CODE_LENGTHS, 16, 257).unwrap();
        for total in 6_usize..=4_096 {
            let matches = total.div_ceil(258);
            let deficit = 258 * matches - total;
            let active = matches.min(deficit);
            let active_lengths = partitioner.active_lengths(active, deficit).unwrap();
            let lengths = std::iter::repeat(258_u16)
                .take(matches - active)
                .chain(active_lengths)
                .collect::<Vec<_>>();
            assert_eq!(lengths.len(), matches, "total {total}");
            assert!(
                lengths.iter().all(|length| (3..=258).contains(length)),
                "total {total}: {lengths:?}"
            );
            assert_eq!(
                lengths.iter().copied().map(usize::from).sum::<usize>(),
                total
            );
        }
    }

    #[test]
    fn same_distance_partitioning_uses_huffman_costs() {
        let mut prefer_258 = [15_u8; 286];
        prefer_258[284] = 15;
        prefer_258[285] = 1;
        let first = test_partitioner(&prefer_258, 2, 6)
            .unwrap()
            .active_lengths(2, 6)
            .unwrap();

        let mut avoid_258 = [15_u8; 286];
        avoid_258[284] = 1;
        avoid_258[285] = 15;
        let second = test_partitioner(&avoid_258, 2, 6)
            .unwrap()
            .active_lengths(2, 6)
            .unwrap();

        assert_eq!(first, [258, 252]);
        assert_eq!(second, [257, 253]);
    }

    #[test]
    fn same_distance_repacking_respects_barriers_and_strict_comparison() {
        let tokens = vec![
            Token::Literal(b'a'),
            test_match(3, 1, 0, 0, 0),
            test_match(4, 2, 1, 0, 0),
            Token::Literal(b'b'),
            test_match(3, 1, 0, 0, 0),
        ];
        assert!(test_repack(&tokens, 12, &FIXED_LITERAL_CODE_LENGTHS, false).is_none());

        let already_minimal = [test_match(258, 1, 0, 0, 0); 2];
        assert!(test_repack(&already_minimal, 516, &FIXED_LITERAL_CODE_LENGTHS, false,).is_none());

        let redundant_exact_multiple = [
            test_match(100, 1, 0, 0, 0),
            test_match(158, 1, 0, 0, 0),
            test_match(258, 1, 0, 0, 0),
        ];
        assert_eq!(
            test_repack(
                &redundant_exact_multiple,
                516,
                &FIXED_LITERAL_CODE_LENGTHS,
                false,
            )
            .unwrap(),
            [test_match(258, 1, 0, 0, 0); 2]
        );

        let source = vec![
            Token::Literal(b'a'),
            test_match(3, 1, 0, 0, 0),
            test_match(4, 1, 0, 0, 0),
        ];
        let block = short_family_test_block(source.clone(), vec![b'a'; 8]);
        let incumbent = PlannedBlock {
            tokens: source.into(),
            plain: block.plain.clone(),
            representation: Representation::Fixed,
            bits: 0,
            source_type: SourceBlockType::Fixed,
        };
        let retained =
            improve_plan_with_same_distance_floor(&block, 0, &Options::default(), incumbent);
        assert_eq!(retained.bits, 0);
        assert_eq!(retained.tokens, block.tokens);
    }

    #[test]
    fn large_source_bands_have_explicit_model_bounds() {
        assert!(!large_source_bands_eligible(127_999, 1));
        assert!(large_source_bands_eligible(128_000, 80_000));
        assert!(!large_source_bands_eligible(128_000, 80_001));
    }

    #[test]
    fn all_literal_endpoint_preflights_large_marginal_matches() {
        let mut tokens = vec![Token::Literal(b'a'); 39_997];
        tokens.push(test_match(3, 32_768, 29, 8_191, 13));
        let block = short_family_test_block(tokens, vec![b'a'; 40_000]);
        let options = Options::default();
        let base = plan_block(&block, 0, &options, &mut SearchStop::never());
        let mut improved = base.clone();

        consider_all_literals(&block, &options, &mut SearchStop::never(), &mut improved);

        assert!(improved.bits < base.bits);
        assert_eq!(improved.tokens.len(), block.plain.len());
        assert!(improved
            .tokens
            .iter()
            .all(|token| matches!(token, Token::Literal(b'a'))));
    }

    #[test]
    fn all_literal_endpoint_has_explicit_work_bounds() {
        assert!(all_literal_endpoint_is_bounded(80_000, u64::MAX));
        assert!(!all_literal_endpoint_is_bounded(80_001, 257));
        assert!(all_literal_endpoint_is_bounded(1_000_000, 256));
        assert!(!all_literal_endpoint_is_bounded(1_000_001, 1));
    }

    fn assert_same_plan(left: &PlannedBlock, right: &PlannedBlock) {
        assert_eq!(left.bits, right.bits);
        assert_eq!(left.tokens, right.tokens);
        assert_eq!(left.plain, right.plain);
        assert_eq!(left.source_type, right.source_type);
        match (&left.representation, &right.representation) {
            (Representation::Original(left), Representation::Original(right)) => {
                assert_eq!(left, right);
            }
            (Representation::Stored, Representation::Stored)
            | (Representation::Fixed, Representation::Fixed) => {}
            (Representation::Dynamic(left), Representation::Dynamic(right)) => {
                assert_eq!(left, right);
            }
            pair => panic!("different representations: {pair:?}"),
        }
    }

    fn short_family_test_block(tokens: Vec<Token>, plain: Vec<u8>) -> ParsedBlock {
        let (literal_frequencies, distance_frequencies) = count_frequencies(&tokens);
        ParsedBlock {
            tokens: tokens.into(),
            plain: plain.into(),
            literal_frequencies,
            distance_frequencies,
            original_literal_lengths: None,
            original_distance_lengths: None,
            original_dynamic: None,
            original: None,
            source_splits: Vec::new(),
            source_type: SourceBlockType::Dynamic,
        }
    }

    fn mixed_short_family_block() -> ParsedBlock {
        let mut tokens = vec![Token::Literal(b'z')];
        let mut plain = vec![b'z'];
        for (length, length_symbol, distance, distance_symbol, distance_extra_bits, byte) in [
            (6, 260, 1, 0, 0, b'a'),
            (7, 261, 5, 4, 1, b'b'),
            (8, 262, 9, 6, 2, b'c'),
            (9, 263, 17, 8, 3, b'd'),
            (10, 264, 33, 10, 4, b'e'),
        ] {
            tokens.push(Token::Match {
                length,
                distance,
                length_symbol,
                distance_symbol,
                length_extra: 0,
                distance_extra: 0,
                length_extra_bits: 0,
                distance_extra_bits,
            });
            plain.extend(std::iter::repeat(byte).take(usize::from(length)));
        }
        tokens.push(Token::Literal(b'z'));
        plain.push(b'z');
        short_family_test_block(tokens, plain)
    }

    #[test]
    fn additive_floor_helpers_preserve_independent_candidate_order() {
        let block = mixed_short_family_block();
        let options = Options {
            exhaustive: true,
            ..Options::default()
        };
        let mut floor_options = options.clone();
        floor_options.exhaustive = false;

        let legacy_floor = plan_block_with_floor(&block, 3, &options, true);
        let legacy_short = plan_block_with_short_family_floor(&block, 3, &options);
        let legacy = if legacy_short.bits < legacy_floor.bits {
            legacy_short
        } else {
            legacy_floor
        };

        let base = plan_block(&block, 3, &floor_options, &mut SearchStop::never());
        let reused = improve_plan_with_floor(&block, 3, &options, true, base);
        let reused = improve_plan_with_short_family_floor(&block, &options, reused);
        assert_same_plan(&reused, &legacy);

        let fresh_deft4j_base = plan_block(&block, 3, &floor_options, &mut SearchStop::never());
        let fresh_deft4j =
            improve_plan_with_deft4j_tree_floor(&block, 3, &options, fresh_deft4j_base);
        let cloned_deft4j_base = plan_block(&block, 3, &floor_options, &mut SearchStop::never());
        let cloned_deft4j_base =
            try_clone_planned_block(&cloned_deft4j_base).expect("small plan metadata is cloneable");
        let cloned_deft4j =
            improve_plan_with_deft4j_tree_floor(&block, 3, &options, cloned_deft4j_base);
        assert_same_plan(&cloned_deft4j, &fresh_deft4j);
    }

    fn materialized_short_family_bits(block: &ParsedBlock, min_distance_codes: bool) -> u64 {
        let mut candidates = vec![block.tokens.as_ref().clone()];
        for last_symbol in 260..=264 {
            let tokens = expand_selected_matches(&block.tokens, &block.plain, |_, token, _| {
                matches!(
                    token,
                    Token::Match { length_symbol, .. }
                        if (260..=last_symbol).contains(&length_symbol)
                )
            })
            .expect("every cumulative family changes the synthetic block");
            candidates.push(tokens);
        }

        let mut best = u64::MAX;
        for tokens in candidates {
            let (literal_frequencies, mut distance_frequencies) = count_frequencies(&tokens);
            ensure_floor_distance_symbols(&mut distance_frequencies, min_distance_codes);
            for variant in 0..4 {
                let literal = make_lengths_deflopt_heap(&literal_frequencies, 15, variant);
                let distance = make_lengths_deflopt_heap(&distance_frequencies, 15, variant);
                if let Some(plan) = plan_for_explicit_lengths(&tokens, &literal, &distance, false) {
                    best = best.min(plan.bits);
                }
            }
        }
        best
    }

    #[test]
    fn bounded_replays_return_only_strict_huffman_improvements() {
        let mut tokens = vec![Token::Literal(b'a'); 1_000];
        tokens.push(Token::Match {
            length: 3,
            distance: 1,
            length_symbol: 257,
            distance_symbol: 0,
            length_extra: 0,
            distance_extra: 0,
            length_extra_bits: 0,
            distance_extra_bits: 0,
        });
        let block = short_family_test_block(tokens, vec![b'a'; 1_003]);
        let mut selected = plan_block(&block, 0, &Options::default(), &mut SearchStop::never());
        assert!(matches!(
            selected.representation,
            Representation::Dynamic(_)
        ));

        // Inflating only the comparison price makes either valid rebuilt
        // candidate observable without fabricating an invalid Huffman tree.
        selected.bits = u64::MAX;
        let extended = replay_extended_floor(&selected, 0, &Options::default())
            .expect("the synthetic table admits an extended replay");
        let ladder = replay_table_ladder(&selected, 0, &Options::default())
            .expect("the synthetic table admits a table ladder");
        for replay in [extended, ladder] {
            assert!(replay.bits < selected.bits);
            assert!(!matches!(replay.representation, Representation::Stored));
            assert_eq!(replay.plain, selected.plain);
        }
    }

    #[test]
    fn expansion_budget_counts_tokens_and_decoded_bytes() {
        let source = [Token::Match {
            length: 4,
            distance: 1,
            length_symbol: 258,
            distance_symbol: 0,
            length_extra: 0,
            distance_extra: 0,
            length_extra_bits: 0,
            distance_extra_bits: 0,
        }];
        let plain = b"aaaa";
        let exact_limit = parsed_model_bytes(plain.len(), plain.len(), 1).unwrap();

        // One byte below the complete one-block model budget must reject the
        // optional expansion before allocating its four literal tokens.
        assert!(
            expand_selected_matches_with_limit(&source, plain, exact_limit - 1, |_, _, _| true,)
                .is_none()
        );

        let expanded =
            expand_selected_matches_with_limit(&source, plain, exact_limit, |_, _, _| true)
                .expect("the exact parser-model budget admits the candidate");
        assert_eq!(expanded, vec![Token::Literal(b'a'); 4]);

        // Checked multiplication rejects an impossible token count rather
        // than wrapping it into a small allocation.
        assert!(new_token_candidate_with_limit(usize::MAX, 0, usize::MAX).is_none());
    }

    #[test]
    fn table_replay_preflights_its_per_pass_token_cap() {
        let one_match = Token::Match {
            length: 3,
            distance: 1,
            length_symbol: 257,
            distance_symbol: 0,
            length_extra: 0,
            distance_extra: 0,
            length_extra_bits: 0,
            distance_extra_bits: 0,
        };
        let tokens = [one_match, one_match];
        let plain = b"aaaaaa";
        let mut literal_lengths = vec![15; 286];
        literal_lengths[usize::from(b'a')] = 1;
        let distance_lengths = vec![15; 30];

        assert!(expand_matches_with_token_limit(
            &tokens,
            plain,
            &literal_lengths,
            &distance_lengths,
            true,
            5,
        )
        .is_none());
        assert_eq!(
            expand_matches_with_token_limit(
                &tokens,
                plain,
                &literal_lengths,
                &distance_lengths,
                true,
                6,
            )
            .unwrap(),
            vec![Token::Literal(b'a'); 6]
        );
    }

    #[test]
    fn strict_expansion_preserves_zero_delta_matches() {
        let tokens = vec![
            Token::Match {
                length: 3,
                distance: 1,
                length_symbol: 257,
                distance_symbol: 0,
                length_extra: 0,
                distance_extra: 0,
                length_extra_bits: 0,
                distance_extra_bits: 0,
            },
            Token::Match {
                length: 4,
                distance: 1,
                length_symbol: 258,
                distance_symbol: 0,
                length_extra: 0,
                distance_extra: 0,
                length_extra_bits: 0,
                distance_extra_bits: 0,
            },
        ];
        let plain = b"aaabbbb";
        let mut literal_lengths = vec![15; 288];
        literal_lengths[usize::from(b'a')] = 1;
        literal_lengths[usize::from(b'b')] = 1;
        literal_lengths[257] = 2;
        literal_lengths[258] = 2;
        let distance_lengths = vec![2];

        let strict = expand_matches(&tokens, plain, &literal_lengths, &distance_lengths, false)
            .expect("the first match is strictly more expensive than its literals");
        let non_larger = expand_matches(&tokens, plain, &literal_lengths, &distance_lengths, true)
            .expect("both matches are no cheaper than their literals");

        assert_eq!(strict.len(), 4);
        assert!(matches!(
            strict.last(),
            Some(Token::Match { length: 4, .. })
        ));
        assert_eq!(
            non_larger,
            plain
                .iter()
                .copied()
                .map(Token::Literal)
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn individual_prune_uses_source_tree_when_original_bits_are_best() {
        let tokens = vec![
            Token::Literal(b'a'),
            Token::Match {
                length: 3,
                distance: 1,
                length_symbol: 257,
                distance_symbol: 0,
                length_extra: 0,
                distance_extra: 0,
                length_extra_bits: 0,
                distance_extra_bits: 0,
            },
        ];
        let (literal_frequencies, distance_frequencies) = count_frequencies(&tokens);
        let (fixed_literal, fixed_distance) = fixed_lengths();
        let mut source_literal = [0_u8; 286];
        let mut source_distance = [0_u8; 30];
        source_literal.copy_from_slice(&fixed_literal[..286]);
        source_distance.copy_from_slice(&fixed_distance[..30]);
        let block = ParsedBlock {
            tokens: tokens.clone().into(),
            plain: b"aaaa".to_vec().into(),
            literal_frequencies,
            distance_frequencies,
            original_literal_lengths: Some(source_literal),
            original_distance_lengths: Some(source_distance),
            original_dynamic: None,
            original: None,
            source_splits: Vec::new(),
            source_type: SourceBlockType::Dynamic,
        };
        let mut best = PlannedBlock {
            tokens: tokens.into(),
            plain: block.plain.clone(),
            representation: Representation::Original(OriginalBits {
                start: 0,
                len: u64::MAX,
                alignment: 0,
                block_type: SourceBlockType::Dynamic,
            }),
            bits: u64::MAX,
            source_type: SourceBlockType::Dynamic,
        };

        individual_prune_search(
            &block,
            0,
            &Options::default(),
            &mut SearchStop::never(),
            &mut best,
        );

        assert!(best.bits < u64::MAX);
        assert_eq!(best.tokens.as_slice(), vec![Token::Literal(b'a'); 4]);
    }

    #[test]
    fn short_family_frequency_score_matches_materialized_tokens() {
        let block = mixed_short_family_block();
        let stats = ShortFamilyStats::from_block(&block).expect("the model is internally valid");

        assert_eq!(stats.match_removals, [1; 5]);
        assert_eq!(stats.extra_bits_removed, [0, 1, 2, 3, 4]);
        for family in 0..5 {
            assert_eq!(
                stats.literal_additions[family][usize::from(b'a' + family as u8)],
                6 + family as u32
            );
            assert_eq!(stats.distance_removals[family][[0, 4, 6, 8, 10][family]], 1);
        }

        for min_distance_codes in [false, true] {
            let score = score_short_family_frequencies(
                &block.literal_frequencies,
                &block.distance_frequencies,
                token_extra_bits(&block.tokens),
                &stats,
                min_distance_codes,
            )
            .expect("at least one complete dynamic tree is available");
            assert_eq!(
                score,
                materialized_short_family_bits(&block, min_distance_codes)
            );
        }
    }

    #[test]
    fn short_family_stats_add_across_source_blocks() {
        let combined = mixed_short_family_block();
        let split_token = 4;
        let split_plain: usize = combined.tokens[..split_token]
            .iter()
            .map(|token| token.decoded_len())
            .sum();
        let left = short_family_test_block(
            combined.tokens[..split_token].to_vec(),
            combined.plain[..split_plain].to_vec(),
        );
        let right = short_family_test_block(
            combined.tokens[split_token..].to_vec(),
            combined.plain[split_plain..].to_vec(),
        );

        let mut added = ShortFamilyStats::from_block(&left).expect("valid left model");
        added
            .add_assign(&ShortFamilyStats::from_block(&right).expect("valid right model"))
            .expect("small synthetic counts cannot overflow");
        assert_eq!(
            added,
            ShortFamilyStats::from_block(&combined).expect("valid combined model")
        );

        let mut literal = left.literal_frequencies;
        for (total, &frequency) in literal.iter_mut().zip(&right.literal_frequencies) {
            *total += frequency;
        }
        literal[256] -= 1;
        let mut distance = left.distance_frequencies;
        for (total, &frequency) in distance.iter_mut().zip(&right.distance_frequencies) {
            *total += frequency;
        }
        let aggregate = score_short_family_frequencies(
            &literal,
            &distance,
            token_extra_bits(&left.tokens) + token_extra_bits(&right.tokens),
            &added,
            false,
        );
        let direct = score_short_family_frequencies(
            &combined.literal_frequencies,
            &combined.distance_frequencies,
            token_extra_bits(&combined.tokens),
            &ShortFamilyStats::from_block(&combined).expect("valid combined model"),
            false,
        );
        assert_eq!(aggregate, direct);
    }
}
