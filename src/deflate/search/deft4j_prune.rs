// SPDX-License-Identifier: MIT
// SPDX-FileCopyrightText: 2026 Andrew C.E. Dent <https://github.com/ace-dent>

//! deft4j's heap trees with prune and rebuild, and the D4 seeds built on them.
//!
//! deft4j prunes a block's matches, spelling them as literals, under trees
//! built from its counts, then rebuilds the trees from the new counts. The
//! deft4j tree floor prices deft4j's Java-heap tree on the unchanged tokens,
//! then a strict and a tie-including expansion under that tree, each rebuilt
//! by Columbo's planner and by deft4j's heap. D4's length-family chain repeats
//! that prune and rebuild after expanding whole length families; its re-spell
//! from literals starts from an all-literal spelling's codes and keeps only
//! the source matches that pay.

use crate::deflate::block::plan_owned_block;
use crate::deflate::header::plan_for_explicit_lengths;
use crate::deflate::huffman::make_lengths_deft4j_java_heap;
use crate::deflate::model::{
    count_frequencies, same_tokens, ParsedBlock, PlannedBlock, Representation, Token,
};
use crate::deflate::stop::SearchStop;
use crate::Options;

use super::{
    ensure_floor_distance_symbols, estimated_length, expand_matches,
    expand_matches_strict_and_non_larger, expand_selected_matches_with_limit,
    individual_prune_from_lengths, try_clone_token_candidate, try_transformed_block,
    MANDATORY_FLOOR_MAX_PLAIN, MANDATORY_FLOOR_MAX_TOKENS, MAX_TOKEN_CANDIDATE_BYTES,
};

/// Whole length-family expansions in one length-family chain (D4).
const LENGTH_FAMILY_STEPS: usize = 3;
/// Most expand-and-rebuild passes in one prune chain.
const PRUNE_CHAIN_PASSES: usize = 32;

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

pub(super) fn consider_deft4j_trees(
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
    let [strict, non_larger] =
        expand_matches_strict_and_non_larger(&block.tokens, &block.plain, &literal, &distance);
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

#[cfg(test)]
mod tests;
