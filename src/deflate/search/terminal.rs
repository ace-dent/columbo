// SPDX-License-Identifier: MIT
// SPDX-FileCopyrightText: 2026 Andrew C.E. Dent <https://github.com/ace-dent>

//! Replays from a finished plan's own codes, including terminal token replay
//! (R17).
//!
//! Once a block's plan is chosen, its codes can make further matches dearer
//! than their literals. R17 replays the extended block floor from a block's
//! final codes after R13–R16 have re-cut, merged and re-treed it. Terminal
//! tightening gives the finished Huffman blocks the feedback trees and
//! DeflOpt's strict table replay, since a cheaper layout can reach the
//! deadline before its last blocks get optional feedback. The extended-floor
//! replay and the table replay ladder restart from a selected plan's codes for
//! the whole-stream merged replay.

use crate::deflate::block::{plan_block, reusable_original_bits};
use crate::deflate::model::{
    count_frequencies, distance_extra_bits, length_extra_bits, ParsedBlock, PlannedBlock,
    Representation, Token,
};
use crate::deflate::stop::SearchStop;
use crate::Options;

use super::{
    consider_feedback_seed_trees, estimated_length, expand_matches,
    expand_matches_with_token_limit, feedback_tree_seeds, plan_block_with_floor, plan_lengths,
    plan_tokens, try_transformed_block, LiteralByteCosts, MANDATORY_FLOOR_MAX_PLAIN,
    MANDATORY_FLOOR_MAX_TOKENS,
};

const TABLE_REPLAY_MAX_TOKENS: usize = 100_000;
const TABLE_REPLAY_PASSES: usize = 4;
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

/// Whether any match costs strictly more than its literals under the given
/// code lengths, priced as the table replay prices them.
pub(super) fn has_dearer_match(
    tokens: &[Token],
    plain: &[u8],
    literal_lengths: &[u8],
    distance_lengths: &[u8],
) -> bool {
    let byte_costs = LiteralByteCosts::new(literal_lengths);
    let mut offset = 0_usize;
    for token in tokens {
        match *token {
            Token::Literal(_) => offset += 1,
            Token::Match {
                length,
                length_symbol,
                distance_symbol,
                ..
            } => {
                let end = offset + usize::from(length);
                let Some(decoded) = plain.get(offset..end) else {
                    return false;
                };
                let match_bits = estimated_length(literal_lengths, usize::from(length_symbol))
                    + u64::from(length_extra_bits(length_symbol))
                    + estimated_length(distance_lengths, usize::from(distance_symbol))
                    + u64::from(distance_extra_bits(distance_symbol));
                if byte_costs
                    .up_to(decoded, match_bits)
                    .is_some_and(|literal_bits| literal_bits < match_bits)
                {
                    return true;
                }
                offset = end;
            }
        }
    }
    false
}

#[cfg(test)]
mod tests;
