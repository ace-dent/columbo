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

use std::sync::Arc;

use crate::Options;

use super::block::{plan_block, plan_owned_block};
use super::header::{
    best_dynamic_plan_cached, estimate_boundary_block_bits, plan_for_explicit_lengths,
    token_bits_from_frequencies, HeaderPlanCache,
};
use super::huffman::{
    make_lengths_columbo_defluff_limited, make_lengths_defluff_exact, FIXED_DISTANCE_CODE_LENGTHS,
    FIXED_LITERAL_CODE_LENGTHS,
};
use super::model::{
    canonical_length_encoding, count_frequencies, same_tokens, try_clone_slice, ParsedBlock,
    PlannedBlock, Representation, Token,
};
use super::parse::{parsed_model_bytes, BASE_PARSED_MODEL_BYTES};
use super::stop::SearchStop;

mod deft4j_prune;
mod match_groups;
mod proven_submatch;
mod same_distance;
mod short_bands;
mod terminal;
use deft4j_prune::consider_deft4j_trees;
pub(crate) use deft4j_prune::{
    improve_plan_with_deft4j_tree_floor, plan_length_family_chain, plan_respell_from_literals,
};
use match_groups::{match_group_search, ordered_state_queue, SeenTokens};
use proven_submatch::consider_proven_submatches;
pub(super) use proven_submatch::solve_proven_submatch_avoiding;
pub(crate) use proven_submatch::{
    compact_proven_submatch_route_eligible, improve_plan_with_header_aware_proven_composition,
    improve_plan_with_proven_submatches, proven_submatch_route_eligible,
    PROVEN_SUBMATCH_FULL_MATCH_LIMIT,
};
use same_distance::consider_same_distance_runs;
pub(crate) use same_distance::{
    improve_plan_with_same_distance_floor, same_distance_opportunities,
};
use short_bands::{consider_compact_short_bands, consider_large_source_bands};
pub(crate) use short_bands::{
    improve_plan_with_short_family_floor, score_short_family_frequencies, ShortFamilyStats,
};
pub(crate) use terminal::{
    plan_terminal_token_replay, replay_extended_floor, replay_table_ladder, tighten_terminal_plan,
};

/// Byte limit for one temporary token candidate: expanding one match can
/// produce up to 258 literal tokens.
const MAX_TOKEN_CANDIDATE_BYTES: usize = BASE_PARSED_MODEL_BYTES;
const MANDATORY_FLOOR_MAX_TOKENS: usize = 50_000;
const MANDATORY_FLOOR_MAX_PLAIN: usize = 10_000_000;
const SHORT_FAMILY_MAX_TOKENS: usize = 250_000;
const ALL_LITERAL_DENSE_MAX_PLAIN: usize = 80_000;
const ALL_LITERAL_SPARSE_MAX_PLAIN: usize = 1_000_000;
const ALL_LITERAL_SPARSE_MAX_MATCHES: u64 = 256;
/// Ranked single-match trials in the compact floor; the small limit stops a
/// many-frame container from spending its shared budget on nearly identical
/// candidates.
const COLUMBO_SINGLE_MATCH_TRIALS: usize = 8;

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
        // The first pass expands the block's own tokens under the seed codes,
        // as the strict expansion does, so both come from one walk.
        let mut first_pass = None;
        if !is_fixed_seed {
            let [strictly_expanded, non_larger] = expand_matches_strict_and_non_larger(
                &block.tokens,
                &block.plain,
                &literal_lengths,
                &distance_lengths,
            );
            first_pass = Some(non_larger);
            if let Some(strictly_expanded) = strictly_expanded {
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
            let expanded = first_pass.take().unwrap_or_else(|| {
                expand_matches(
                    &seed_tokens,
                    &block.plain,
                    &literal_lengths,
                    &distance_lengths,
                    true,
                )
            });
            let Some(expanded) = expanded else {
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
        let mut seen = SeenTokens::default();
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

type LengthBuilder = fn(&[u32], u8, u32) -> Vec<u8>;

struct FeedbackTreeSeed {
    builder: LengthBuilder,
    literal: Vec<u8>,
    distance: Vec<u8>,
}

/// Build the two feedback seed trees: Defluff's exact builder, and Columbo's
/// ordinary tree with only Defluff's package-merge limiter.
// Out of line: inlined, it would be copied into each of its callers.
#[inline(never)]
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

        let match_bits = u64::from(match_literal_length)
            + u64::from(length_extra_bits)
            + u64::from(match_distance_length)
            + u64::from(distance_extra_bits);
        // An unavailable literal or a sum reaching the match both keep it, so
        // the scan can stop at either.
        let mut literal_bits = 0_u64;
        for &byte in decoded {
            let length = literal_lengths.get(usize::from(byte)).copied().unwrap_or(0);
            if length == 0 {
                return false;
            }
            literal_bits += u64::from(length);
            if literal_bits >= match_bits {
                return false;
            }
        }
        true
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
        0,
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
    // The expanded matches' indices: `should_expand` is pure, so the second
    // walk reuses its answers instead of asking again.
    let mut expanded = Vec::new();
    for (index, &token) in source.iter().enumerate() {
        let end = plain_offset.checked_add(token.decoded_len())?;
        let decoded = plain.get(plain_offset..end)?;
        let expand = matches!(token, Token::Match { .. }) && should_expand(index, token, decoded);
        let added = if expand { decoded.len() } else { 1 };
        output_count = output_count.checked_add(added)?;
        if output_count > token_limit {
            return None;
        }
        if expand {
            expanded.try_reserve(1).ok()?;
            expanded.push(index);
        }
        plain_offset = end;
    }
    if expanded.is_empty() || plain_offset != plain.len() {
        return None;
    }
    spell_expanded_matches(source, plain, &expanded, output_count, byte_limit)
}

/// Spell `source` with the matches at `expanded`, ascending token indices, as
/// literals, `output_count` tokens in all. Out of line, so each caller's
/// `should_expand` copy keeps only the first walk.
#[inline(never)]
fn spell_expanded_matches(
    source: &[Token],
    plain: &[u8],
    expanded: &[usize],
    output_count: usize,
    byte_limit: usize,
) -> Option<Vec<Token>> {
    debug_assert!(expanded.windows(2).all(|pair| pair[0] < pair[1]));
    let mut output = new_token_candidate_with_limit(output_count, plain.len(), byte_limit)?;
    // The tokens between expanded matches are copied whole; only their decoded
    // length is needed. Decoded offsets only grow, so a token past the end of
    // `plain` shows at the next expanded match or at the end.
    let decoded_end = |offset: usize, tokens: &[Token]| {
        tokens.iter().try_fold(offset, |offset, token| {
            offset.checked_add(token.decoded_len())
        })
    };
    let mut copied = 0;
    let mut plain_offset = 0_usize;
    for &index in expanded {
        let unchanged = source.get(copied..index)?;
        plain_offset = decoded_end(plain_offset, unchanged)?;
        output.extend_from_slice(unchanged);
        let end = plain_offset.checked_add(source.get(index)?.decoded_len())?;
        output.extend(
            plain
                .get(plain_offset..end)?
                .iter()
                .copied()
                .map(Token::Literal),
        );
        plain_offset = end;
        copied = index + 1;
    }
    let rest = source.get(copied..)?;
    if decoded_end(plain_offset, rest)? > plain.len() {
        return None;
    }
    output.extend_from_slice(rest);
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
    let byte_costs = LiteralByteCosts::new(literal_lengths);
    expand_selected_matches_with_limits(
        tokens,
        plain,
        MAX_TOKEN_CANDIDATE_BYTES,
        token_limit,
        |_, token, decoded| {
            literals_against_match(
                token,
                decoded,
                &byte_costs,
                literal_lengths,
                distance_lengths,
            )
            .is_some_and(|order| order.is_lt() || (include_equal && order.is_eq()))
        },
    )
}

/// How a match's literals compare in cost with the match under the given code
/// lengths, or `None` when the literals cost more or the token is a literal.
fn literals_against_match(
    token: Token,
    decoded: &[u8],
    byte_costs: &LiteralByteCosts,
    literal_lengths: &[u8],
    distance_lengths: &[u8],
) -> Option<std::cmp::Ordering> {
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
    let match_bits = estimated_length(literal_lengths, usize::from(length_symbol))
        + u64::from(length_extra_bits)
        + estimated_length(distance_lengths, usize::from(distance_symbol))
        + u64::from(distance_extra_bits);
    let literal_bits = byte_costs.up_to(decoded, match_bits)?;
    Some(literal_bits.cmp(&match_bits))
}

/// [`expand_matches`] without and then with `include_equal`, from one walk:
/// each match's literal sum, up to the match's own cost, decides both.
fn expand_matches_strict_and_non_larger(
    tokens: &[Token],
    plain: &[u8],
    literal_lengths: &[u8],
    distance_lengths: &[u8],
) -> [Option<Vec<Token>>; 2] {
    let byte_costs = LiteralByteCosts::new(literal_lengths);
    let mut expanded: [Vec<usize>; 2] = [Vec::new(), Vec::new()];
    let mut output_counts = [0_usize; 2];
    let mut plain_offset = 0_usize;
    for (index, &token) in tokens.iter().enumerate() {
        let Some(decoded) = plain_offset
            .checked_add(token.decoded_len())
            .and_then(|end| plain.get(plain_offset..end))
        else {
            return [None, None];
        };
        let order = literals_against_match(
            token,
            decoded,
            &byte_costs,
            literal_lengths,
            distance_lengths,
        );
        let expands = [
            order.is_some_and(|order| order.is_lt()),
            order.is_some_and(|order| order.is_le()),
        ];
        for ((expand, indices), count) in expands
            .into_iter()
            .zip(&mut expanded)
            .zip(&mut output_counts)
        {
            let Some(added) = count.checked_add(if expand { decoded.len() } else { 1 }) else {
                return [None, None];
            };
            *count = added;
            if expand {
                if indices.try_reserve(1).is_err() {
                    return [None, None];
                }
                indices.push(index);
            }
        }
        plain_offset += decoded.len();
    }
    if plain_offset != plain.len() {
        return [None, None];
    }
    let spell = |indices: &[usize], count: usize| {
        if indices.is_empty() {
            return None;
        }
        spell_expanded_matches(tokens, plain, indices, count, MAX_TOKEN_CANDIDATE_BYTES)
    };
    [
        spell(&expanded[0], output_counts[0]),
        spell(&expanded[1], output_counts[1]),
    ]
}

fn estimated_length(lengths: &[u8], symbol: usize) -> u64 {
    lengths
        .get(symbol)
        .copied()
        .filter(|&length| length != 0)
        .map_or(15, u64::from)
}

/// Each byte's [`estimated_length`] as a literal, found once for a walk that
/// prices many matches' bytes.
struct LiteralByteCosts {
    costs: [u8; 256],
    /// The least of `costs`: no byte costs less as a literal.
    cheapest: u8,
}

impl LiteralByteCosts {
    fn new(literal_lengths: &[u8]) -> Self {
        // An estimated length is a code length or 15, so it fits a byte.
        let costs: [u8; 256] =
            std::array::from_fn(|byte| estimated_length(literal_lengths, byte) as u8);
        let cheapest = costs.iter().copied().min().unwrap_or(0);
        Self { costs, cheapest }
    }

    /// The decoded bytes' total as literals, or `None` when it exceeds
    /// `limit`. A match whose bytes would exceed it even at the cheapest cost
    /// is not priced byte by byte; most matches cost less than their literals.
    fn up_to(&self, decoded: &[u8], limit: u64) -> Option<u64> {
        if decoded.len() as u64 * u64::from(self.cheapest) > limit {
            return None;
        }
        let bits: u64 = decoded
            .iter()
            .map(|&byte| u64::from(self.costs[usize::from(byte)]))
            .sum();
        (bits <= limit).then_some(bits)
    }
}

/// Respell every length-258 match with Defluff's alias (symbol 284) or with
/// symbol 285, as an optional candidate within the candidate limit.
pub(crate) fn rewrite_258_symbols(
    tokens: &[Token],
    decoded_bytes: usize,
    use_alias: bool,
) -> Option<Vec<Token>> {
    let mut rewritten = new_token_candidate(tokens.len(), decoded_bytes)?;
    rewritten.extend(tokens.iter().map(|&token| spell_258(token, use_alias)));
    Some(rewritten)
}

/// Strict mode's mandatory respelling of every alias as symbol 285. The copy
/// is exactly as large as the source block the parser admitted, which may
/// exceed the limit for optional candidates, so only allocation can fail.
pub(crate) fn normalize_258_symbols(tokens: &[Token]) -> Option<Vec<Token>> {
    let mut normalized = Vec::new();
    normalized.try_reserve_exact(tokens.len()).ok()?;
    normalized.extend(tokens.iter().map(|&token| spell_258(token, false)));
    Some(normalized)
}

fn spell_258(token: Token, use_alias: bool) -> Token {
    match token {
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
    }
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
pub(crate) mod test_support;
#[cfg(test)]
mod tests;
