// SPDX-License-Identifier: MIT

//! Proven-LZ77 structural searches.
//!
//! These routes never search the history window or choose a new distance. They
//! selectively spell an existing match as its already-decoded literals, or
//! move boundaries inside an adjacent run already proven at one distance, then
//! rebuild the Huffman representation. Equal-cost token transformations are
//! useful intermediate states because their changed frequencies can make the
//! next tree smaller.
//!
//! Names identify recovered primitives precisely. DeflOpt and Defluff labels
//! apply only to behavior mapped to those programs; bounded floors, cumulative
//! length-family bands, match groups, queues, and repeated replay are Columbo
//! compositions, even when they use a recovered primitive internally.

use std::sync::Arc;

use crate::Options;

use super::block::{plan_block, plan_owned_block};
use super::header::{
    plan_for_explicit_lengths, plan_for_explicit_lengths_with_cost, token_bits_from_frequencies,
};
use super::huffman::{
    make_lengths_columbo_defluff_limited, make_lengths_deflopt_heap, make_lengths_defluff_exact,
    make_lengths_deft4j_java_heap, FIXED_DISTANCE_CODE_LENGTHS, FIXED_LITERAL_CODE_LENGTHS,
};
use super::model::{
    canonical_length_encoding, count_frequencies, try_clone_slice, DynamicPlan, ParsedBlock,
    PlannedBlock, Representation, Token,
};
use super::parse::{parsed_model_bytes, MAX_PARSED_MODEL_BYTES};

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
// The exhaustive deficit DP can require 257 layers. Default mode keeps the
// exact cost search for ordinary runs, but uses a legal minimum-token fallback
// once a run would make this cheap candidate family disproportionate.
const DEFAULT_SAME_DISTANCE_MAX_DP_ACTIVE: usize = 16;
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
    consider_same_distance_runs(block, alignment, &floor_options, &mut || false, &mut best);
    best
}

fn consider_same_distance_runs<F>(
    block: &ParsedBlock,
    alignment: u8,
    options: &Options,
    expired: &mut F,
    best: &mut PlannedBlock,
) where
    F: FnMut() -> bool,
{
    if block.tokens.len() < 2 || expired() {
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
        expired,
    ) else {
        return;
    };

    // Price the canonical candidate first. Its planned token storage is
    // reference-counted, so relaxed mode can derive an alias sibling lazily
    // without retaining or eagerly allocating a second large token vector.
    let Some(candidate) = plan_tokens(block, repacked, alignment, options, expired) else {
        return;
    };
    let canonical_tokens = Arc::clone(&candidate.tokens);
    if candidate.bits < best.bits {
        *best = candidate;
    }
    if !options.strict
        && !expired()
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
            consider_tokens(block, aliased, alignment, options, expired, best);
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
fn repack_same_distance_runs<F>(
    tokens: &[Token],
    decoded_bytes: usize,
    literal_lengths: &[u8],
    exhaustive: bool,
    expired: &mut F,
) -> Option<Vec<Token>>
where
    F: FnMut() -> bool,
{
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
            expired,
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
    fn new<F>(
        literal_lengths: &[u8],
        max_active: usize,
        max_deficit: usize,
        expired: &mut F,
    ) -> Option<Self>
    where
        F: FnMut() -> bool,
    {
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
            if expired() {
                return None;
            }
            current[..width].fill(u32::MAX);
            for used_deficit in 0..=max_deficit {
                if used_deficit & 31 == 0 && expired() {
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

/// Complete the cheap token-preserving floor even when a container's shared
/// search deadline has already elapsed.
///
/// This is intentionally much smaller than [`plan_block_with_search`]: it
/// prices strict match-to-literal rewrites against the source, best, and fixed
/// trees exactly once. Compact block lists may request Columbo's `extended`
/// feedback-tree/replay floor. There are no beams, match-group combinations,
/// or newly discovered LZ77 matches. The bound lets ZIP/APNG give every member
/// one useful pass before optional search time is concentrated on harder
/// streams.
pub(crate) fn plan_block_with_floor(
    block: &ParsedBlock,
    alignment: u8,
    options: &Options,
    extended: bool,
) -> PlannedBlock {
    let mut floor_options = options.clone();
    floor_options.exhaustive = false;
    let base = plan_block(block, alignment, &floor_options, || false);
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

    let mut never_expired = || false;
    consider_same_distance_runs(
        block,
        alignment,
        &floor_options,
        &mut never_expired,
        &mut best,
    );
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

    let sparse_literal_endpoint = block.tokens.len() <= 20_000 && match_count <= 32;
    if block.plain.len() <= 12_000 && (block.tokens.len() <= 4_000 || sparse_literal_endpoint) {
        if let Some(literals) = literal_token_candidate(&block.plain) {
            consider_tokens(
                block,
                literals,
                alignment,
                &floor_options,
                &mut never_expired,
                &mut best,
            );
        }
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
                let mut replay = plan_block(&replay_block, alignment, &floor_options, || false);
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
        &mut || false,
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
fn consider_compact_short_bands<F>(
    block: &ParsedBlock,
    alignment: u8,
    options: &Options,
    expired: &mut F,
    best: &mut PlannedBlock,
) where
    F: FnMut() -> bool,
{
    if block.tokens.len() > COMPACT_SHORT_BAND_MAX_TOKENS
        || block.plain.len() > COMPACT_SHORT_BAND_MAX_PLAIN
    {
        return;
    }

    for last_symbol in COMPACT_SHORT_BAND_ENDS {
        if expired() {
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
        consider_tokens(block, tokens, alignment, options, expired, best);
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
    let mut candidate = plan_block(&transformed, 0, &floor_options, || false);
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
        let Some(candidate) = plan_tokens(&block, tokens, alignment, &floor_options, &mut || false)
        else {
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
    let base = plan_block(block, alignment, &floor_options, || false);
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
            let literal_lengths = make_lengths_deflopt_heap(&literal, 15, variant);
            let distance_lengths = make_lengths_deflopt_heap(&build_distance, 15, variant);
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
    for variant in 0..4 {
        let literal = make_lengths_deflopt_heap(&literal_frequencies, 15, variant);
        let distance = make_lengths_deflopt_heap(&distance_frequencies, 15, variant);
        let Some(dynamic) =
            plan_for_explicit_lengths(&tokens, &literal, &distance, options.exhaustive)
        else {
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

fn consider_deft4j_trees<F>(
    block: &ParsedBlock,
    alignment: u8,
    options: &Options,
    expired: &mut F,
    best: &mut PlannedBlock,
    individual_trial_limit: usize,
) where
    F: FnMut() -> bool,
{
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
        consider_deft4j_rebuild(block, tokens, alignment, options, expired, best);
        if !duplicate {
            if let Some(tokens) = non_larger {
                consider_deft4j_rebuild(block, tokens, alignment, options, expired, best);
            }
        }
    } else if let Some(tokens) = non_larger {
        consider_deft4j_rebuild(block, tokens, alignment, options, expired, best);
    }
    if individual_trial_limit != 0 {
        individual_prune_from_lengths(
            block,
            alignment,
            options,
            expired,
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
fn consider_deft4j_rebuild<F>(
    source: &ParsedBlock,
    tokens: Vec<Token>,
    alignment: u8,
    options: &Options,
    expired: &mut F,
    best: &mut PlannedBlock,
) where
    F: FnMut() -> bool,
{
    let Some(candidate) = try_transformed_block(source, tokens) else {
        return;
    };
    let mut distance_frequencies = candidate.distance_frequencies;
    ensure_floor_distance_symbols(&mut distance_frequencies, options.strict);
    let literal = make_lengths_deft4j_java_heap(&candidate.literal_frequencies, 15);
    let distance = make_lengths_deft4j_java_heap(&distance_frequencies, 15);
    let deft4j =
        plan_for_explicit_lengths(&candidate.tokens, &literal, &distance, options.exhaustive);

    let mut planned = plan_owned_block(candidate, alignment, options, expired);
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

pub(crate) fn plan_block_with_search<F>(
    block: &ParsedBlock,
    alignment: u8,
    options: &Options,
    expired: &mut F,
) -> PlannedBlock
where
    F: FnMut() -> bool,
{
    plan_block_with_search_policy(
        block,
        alignment,
        options,
        SearchPolicy::FULL,
        SearchBase::Price,
        expired,
    )
}

/// Continue the full token search from an already completed ordinary plan.
///
/// Stream split discovery prices the unsplit block before it ranks any
/// boundaries. Passing that exact plan back here avoids rebuilding the same
/// exhaustive Huffman representation when the token search begins.
pub(crate) fn plan_block_with_complete_base_search<F>(
    block: &ParsedBlock,
    alignment: u8,
    options: &Options,
    base: PlannedBlock,
    expired: &mut F,
) -> PlannedBlock
where
    F: FnMut() -> bool,
{
    plan_block_with_search_policy(
        block,
        alignment,
        options,
        SearchPolicy::FULL,
        SearchBase::Complete(base),
        expired,
    )
}

/// Search one complete source block without stream-split or iterative siblings.
///
/// The direct source route uses the same table-feedback ladder as the ordinary
/// block planner, but deliberately leaves match-group beams, the ordered state
/// queue, and post-search replay to their independent candidates. This keeps a
/// long source-block chain moving forward instead of spending its entire wall
/// budget on the first locally interesting block.
pub(crate) fn plan_block_with_narrow_search<F>(
    block: &ParsedBlock,
    alignment: u8,
    options: &Options,
    allow_individual_prune: bool,
    expired: &mut F,
) -> PlannedBlock
where
    F: FnMut() -> bool,
{
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
        },
        SearchBase::Price,
        expired,
    )
}

/// Continue the narrow whole-block route from an independently completed
/// exact candidate while retaining the original source tokens as another
/// transformation parent.
pub(crate) fn plan_block_with_seeded_narrow_search<F>(
    block: &ParsedBlock,
    alignment: u8,
    options: &Options,
    allow_individual_prune: bool,
    seed: PlannedBlock,
    expired: &mut F,
) -> PlannedBlock
where
    F: FnMut() -> bool,
{
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
        },
        SearchBase::Additional(seed),
        expired,
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
}

impl SearchPolicy {
    const FULL: Self = Self {
        match_groups: true,
        individual_prune: true,
        ordered_queue: true,
        replay: true,
        large_source_bands: false,
    };
}

fn plan_block_with_search_policy<F>(
    block: &ParsedBlock,
    alignment: u8,
    options: &Options,
    policy: SearchPolicy,
    base: SearchBase,
    expired: &mut F,
) -> PlannedBlock
where
    F: FnMut() -> bool,
{
    let mut best = match base {
        SearchBase::Price => plan_block(block, alignment, options, &mut *expired),
        SearchBase::Additional(seed) => {
            let ordinary = plan_block(block, alignment, options, &mut *expired);
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
    if expired() {
        return best;
    }
    consider_same_distance_runs(block, alignment, options, expired, &mut best);
    if expired() {
        return best;
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
                consider_tokens(block, normalized, alignment, options, expired, &mut best);
            }
        }
    }
    if !options.strict && block.literal_frequencies[285] != 0 {
        if let Some(aliased) = rewrite_258_symbols(&block.tokens, block.plain.len(), true) {
            if aliased.as_slice() != block.tokens.as_slice() {
                consider_tokens(block, aliased, alignment, options, expired, &mut best);
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

    // Columbo directly prices the literal-only endpoint. Neither DeflOpt nor
    // Defluff implements this repeated match-group shortcut: they contribute
    // narrower strict/two-stage primitives used elsewhere in this planner.
    let sparse_literal_endpoint = block.tokens.len() <= 20_000 && match_count <= 32;
    if block.plain.len() <= 12_000
        && (block.tokens.len() <= 4_000 || sparse_literal_endpoint)
        && !expired()
    {
        if let Some(literals) = literal_token_candidate(&block.plain) {
            consider_tokens(block, literals, alignment, options, expired, &mut best);
        }
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

    if !expired() {
        consider_columbo_defluff_derived_rescan(
            block,
            alignment,
            options,
            &feedback_seeds,
            expired,
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
        if expired() {
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
                    expired,
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
            let Some(candidate) = plan_tokens(block, expanded, alignment, options, expired) else {
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
            if expired() {
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
        consider_tokens(block, tokens, alignment, options, expired, &mut best);
    }

    if policy.large_source_bands && !expired() {
        consider_large_source_bands(block, alignment, options, expired, &mut best);
    }

    if policy.match_groups
        && options.exhaustive
        && block.tokens.len() <= 250_000
        && block.plain.len() <= 10_000_000
        && !expired()
    {
        match_group_search(block, alignment, options, expired, &mut best);
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
    if try_individual_prune && match_count != 0 && !expired() {
        individual_prune_search(block, alignment, options, expired, &mut best);
    }

    if try_ordered_queue && !expired() {
        if let Some(mut alternate) = pre_individual {
            if alternate.tokens != best.tokens {
                if let Some(mut post_individual) = try_clone_planned_block(&best) {
                    // Explore the missing sibling first, then preserve the
                    // original post-prune queue as a second bounded seed.
                    ordered_state_queue(block, alignment, options, expired, &mut alternate);
                    if alternate.bits < best.bits {
                        best = alternate;
                    }
                    if !expired() {
                        ordered_state_queue(
                            block,
                            alignment,
                            options,
                            expired,
                            &mut post_individual,
                        );
                        if post_individual.bits < best.bits {
                            best = post_individual;
                        }
                    }
                } else {
                    // Allocation failure keeps the established single-seed
                    // route instead of making optional search an error.
                    ordered_state_queue(block, alignment, options, expired, &mut best);
                }
            } else {
                ordered_state_queue(block, alignment, options, expired, &mut best);
            }
        } else {
            ordered_state_queue(block, alignment, options, expired, &mut best);
        }
    }

    if policy.replay && options.exhaustive && best.tokens != block.tokens && !expired() {
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
            let replay = plan_block_with_search(&replay_block, alignment, &replay_options, expired);
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
            if expired() {
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

fn ordered_state_queue<F>(
    block: &ParsedBlock,
    alignment: u8,
    options: &Options,
    expired: &mut F,
    best: &mut PlannedBlock,
) where
    F: FnMut() -> bool,
{
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
    // Every edge expands at least one match of length three or more. Token
    // counts therefore increase monotonically, so no child can return to the
    // unexpanded initial state.
    let mut seen = Vec::new();

    for _ in 0..DEPTH {
        let mut next = Vec::new();
        for state in &current {
            if expired() {
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
                    let state_plan = plan_block(&state_block, alignment, options, &mut *expired);
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
                if expired() {
                    return;
                }
                let Some(tokens) =
                    expand_groups(&state.tokens, &block.plain, std::slice::from_ref(group))
                else {
                    continue;
                };
                if seen.iter().any(|known| known == &tokens) {
                    continue;
                }
                let Some(seen_tokens) = try_clone_token_candidate(&tokens, block.plain.len())
                else {
                    return;
                };
                let Some(planned_tokens) = try_clone_token_candidate(&tokens, block.plain.len())
                else {
                    return;
                };
                seen.push(seen_tokens);
                let Some(plan) = plan_tokens(block, planned_tokens, alignment, options, expired)
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
fn consider_columbo_defluff_derived_rescan<F>(
    block: &ParsedBlock,
    alignment: u8,
    options: &Options,
    seeds: &[FeedbackTreeSeed; 2],
    expired: &mut F,
    best: &mut PlannedBlock,
) where
    F: FnMut() -> bool,
{
    for seed in seeds {
        if expired() {
            return;
        }
        let Some(fresh_tokens) =
            expand_defluff_matches(&block.tokens, &block.plain, &seed.literal, &seed.distance)
        else {
            continue;
        };
        let (fresh_literal_frequencies, fresh_distance_frequencies) =
            count_frequencies(&fresh_tokens);
        consider_tokens(block, fresh_tokens, alignment, options, expired, best);

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
            consider_tokens(block, adjusted_tokens, alignment, options, expired, best);
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
fn consider_large_source_bands<F>(
    block: &ParsedBlock,
    alignment: u8,
    options: &Options,
    expired: &mut F,
    best: &mut PlannedBlock,
) where
    F: FnMut() -> bool,
{
    if !large_source_bands_eligible(block.plain.len(), block.tokens.len()) {
        return;
    }

    const CUMULATIVE_ENDS: [u16; 5] = [262, 265, 267, 269, 270];
    for end in CUMULATIVE_ENDS {
        if expired() {
            return;
        }
        let Some(tokens) = expand_selected_matches(
            &block.tokens,
            &block.plain,
            |_, token, _| matches!(token, Token::Match { length_symbol, .. } if (257..=end).contains(&length_symbol)),
        ) else {
            continue;
        };
        consider_tokens(block, tokens, alignment, options, expired, best);
    }

    if expired() {
        return;
    }
    const LONG_FAMILIES: [u16; 12] = [260, 261, 262, 263, 264, 265, 266, 267, 268, 269, 270, 280];
    if let Some(tokens) = expand_selected_matches(
        &block.tokens,
        &block.plain,
        |_, token, _| matches!(token, Token::Match { length_symbol, .. } if LONG_FAMILIES.contains(&length_symbol)),
    ) {
        consider_tokens(block, tokens, alignment, options, expired, best);
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

fn match_group_search<F>(
    block: &ParsedBlock,
    alignment: u8,
    options: &Options,
    expired: &mut F,
    best: &mut PlannedBlock,
) where
    F: FnMut() -> bool,
{
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
            if expired() {
                return;
            }
            if let Some(tokens) =
                expand_groups(&block.tokens, &block.plain, std::slice::from_ref(group))
            {
                consider_tokens(block, tokens, alignment, options, expired, best);
            }
        }

        // Ordered-queue searches retain combinations of the locally cheapest
        // length/distance families. Test the first few prefixes directly.
        let combined_cap = if options.exhaustive { 5 } else { 3 };
        for count in 2..=combined_cap.min(groups.len()) {
            if expired() {
                return;
            }
            if let Some(tokens) = expand_groups(&block.tokens, &block.plain, &groups[..count]) {
                consider_tokens(block, tokens, alignment, options, expired, best);
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

fn consider_tokens<F>(
    source: &ParsedBlock,
    tokens: Vec<Token>,
    alignment: u8,
    options: &Options,
    expired: &mut F,
    best: &mut PlannedBlock,
) where
    F: FnMut() -> bool,
{
    if let Some(candidate) = plan_tokens(source, tokens, alignment, options, expired) {
        if candidate.bits < best.bits {
            *best = candidate;
        }
    }
}

fn plan_tokens<F>(
    source: &ParsedBlock,
    tokens: Vec<Token>,
    alignment: u8,
    options: &Options,
    expired: &mut F,
) -> Option<PlannedBlock>
where
    F: FnMut() -> bool,
{
    let candidate = try_transformed_block(source, tokens)?;
    Some(plan_owned_block(candidate, alignment, options, expired))
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

fn individual_prune_search<F>(
    block: &ParsedBlock,
    alignment: u8,
    options: &Options,
    expired: &mut F,
    best: &mut PlannedBlock,
) where
    F: FnMut() -> bool,
{
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
        expired,
        best,
        &literal_lengths,
        &distance_lengths,
        32,
    );
}

#[allow(clippy::too_many_arguments)]
fn individual_prune_from_lengths<F>(
    block: &ParsedBlock,
    alignment: u8,
    options: &Options,
    expired: &mut F,
    best: &mut PlannedBlock,
    literal_lengths: &[u8],
    distance_lengths: &[u8],
    trial_limit: usize,
) where
    F: FnMut() -> bool,
{
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
        if expired() {
            break;
        }
        if let Some(tokens) = expand_selected_matches(&block.tokens, &block.plain, |index, _, _| {
            index == token_index
        }) {
            consider_tokens(block, tokens, alignment, options, expired, best);
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
            &mut || false,
        )
    }

    fn test_partitioner(
        literal_lengths: &[u8],
        max_active: usize,
        max_deficit: usize,
    ) -> Option<SameDistancePartitioner> {
        SameDistancePartitioner::new(literal_lengths, max_active, max_deficit, &mut || false)
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
        let repacked = repack_same_distance_runs(
            &source,
            258,
            &FIXED_LITERAL_CODE_LENGTHS,
            false,
            &mut dp_must_not_poll,
        )
        .unwrap();
        assert_eq!(repacked.len(), 1);
    }

    #[test]
    fn default_repacking_bounds_pathological_dp_depth_and_max_polls_deadline() {
        let source = vec![test_match(257, 1, 0, 0, 0); 257];
        let decoded_bytes = 257 * 257;
        let mut default_must_not_poll = || -> bool { panic!("default fallback entered the DP") };
        let repacked = repack_same_distance_runs(
            &source,
            decoded_bytes,
            &FIXED_LITERAL_CODE_LENGTHS,
            false,
            &mut default_must_not_poll,
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
        assert!(repack_same_distance_runs(
            &source,
            decoded_bytes,
            &FIXED_LITERAL_CODE_LENGTHS,
            true,
            &mut expires_during_dp,
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

        let base = plan_block(&block, 3, &floor_options, || false);
        let reused = improve_plan_with_floor(&block, 3, &options, true, base);
        let reused = improve_plan_with_short_family_floor(&block, &options, reused);
        assert_same_plan(&reused, &legacy);

        let fresh_deft4j_base = plan_block(&block, 3, &floor_options, || false);
        let fresh_deft4j =
            improve_plan_with_deft4j_tree_floor(&block, 3, &options, fresh_deft4j_base);
        let cloned_deft4j_base = plan_block(&block, 3, &floor_options, || false);
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
        let mut selected = plan_block(&block, 0, &Options::default(), || false);
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

        individual_prune_search(&block, 0, &Options::default(), &mut || false, &mut best);

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
