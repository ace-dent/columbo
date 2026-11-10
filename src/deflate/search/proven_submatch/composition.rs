// SPDX-License-Identifier: MIT
// SPDX-FileCopyrightText: 2026 Andrew C.E. Dent <https://github.com/ace-dent>

//! Header-aware compositions of proven-submatch spellings (Max, inside M3).
//!
//! Rewrites that each lose alone can together drop symbols or shorten an
//! alphabet span, saving header bits. For a few ranked matches, a menu holds
//! the source spelling and up to three proven alternatives. A beam over the
//! menus tracks each state's symbol counts, extra bits and estimated payload
//! change, and its final states are priced as complete blocks. The
//! closed-loop sibling applies the best moves cumulatively, undoes them in
//! reverse under the endpoint's codes, and prices each state exactly.

use std::sync::Arc;

use crate::deflate::model::{
    count_frequencies, same_tokens, token_extra_bits, try_clone_slice, ParsedBlock, PlannedBlock,
    Token,
};
use crate::deflate::stop::SearchStop;
use crate::sort::SortShared;
use crate::Options;

use super::super::{new_token_candidate, plan_lengths, plan_tokens};
use super::{
    build_proven_submatch_rewrites, consider_unique_proven_submatch_tokens,
    estimated_match_token_bits, estimated_tokens_bits, proven_submatch_seed_lengths,
    select_proven_submatch_targets, ProvenSubmatchRestriction, ProvenSubmatchRewrite,
    ProvenSubmatchTarget, MAX_PROVEN_SUBMATCH_FULL_PLAIN, PROVEN_COMPOSITION_BEAM_WIDTH,
};

const PROVEN_COMPOSITION_MAX_SOURCE_MATCHES: usize = 128;
const PROVEN_COMPOSITION_MAX_TOKENS: usize = 8_000;
const PROVEN_COMPOSITION_MAX_TARGETS: usize = 8;
const PROVEN_COMPOSITION_MAX_SPELLINGS: usize = 4;
const PROVEN_COMPOSITION_PAYLOAD_WINDOW: i64 = 24;
const PROVEN_CLOSED_LOOP_MAX_PLAIN: usize = 1_024;
/// The second round runs only after a strict win and shares the exact-price
/// limit, so a block without a gain is priced at most fifteen times.
const PROVEN_CLOSED_LOOP_ROUNDS: usize = 2;
const PROVEN_CLOSED_LOOP_EXACT_LIMIT: usize = 24;

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
        .try_reserve_exact(PROVEN_COMPOSITION_BEAM_WIDTH)
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

    // `states` is the root or a beam step sorted by `proven_composition_state_key`,
    // so the beam width bounds the exact prices.
    for state in states.iter().filter(|state| state.rewrite_count >= 2) {
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

#[cfg(test)]
mod tests;
