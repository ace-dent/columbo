// SPDX-License-Identifier: MIT
// SPDX-FileCopyrightText: 2026 Andrew C.E. Dent <https://github.com/ace-dent>

//! Match groups and the ordered token-state queue built on them (Max).
//!
//! A group holds the matches of one length symbol or one distance symbol.
//! Expanding a whole group removes that symbol from its alphabet, which can
//! shorten the header and the other codes even when no single match is dearer
//! than its literals. Max ranks the groups by their literal bits minus match
//! bits and prices the first few alone and together. The queue is a bounded
//! beam search over sequences of whole-group expansions, whose intermediate
//! states need not win.

use std::collections::HashSet;
use std::hash::{BuildHasherDefault, Hasher};

use crate::deflate::block::plan_block;
use crate::deflate::model::{ParsedBlock, PlannedBlock, Token};
use crate::deflate::stop::SearchStop;
use crate::sort::SortShared;
use crate::Options;

use super::{
    consider_tokens, deduplicate_seeds, expand_selected_matches, plan_lengths, plan_tokens,
    try_clone_token_candidate, try_transformed_block,
};

/// Token sequences the ordered queue has visited. The set only answers
/// membership and compares whole sequences, so any hash gives the same
/// results; it holds at most a few hundred, so collisions cost little.
pub(super) type SeenTokens = HashSet<Vec<Token>, BuildHasherDefault<TokenSequenceHasher>>;

/// A multiply-rotate hash of each field, much cheaper than the default
/// SipHash over thousands of tokens.
#[derive(Default)]
pub(super) struct TokenSequenceHasher(u64);

impl Hasher for TokenSequenceHasher {
    fn finish(&self) -> u64 {
        self.0
    }

    fn write(&mut self, bytes: &[u8]) {
        for &byte in bytes {
            self.write_u64(u64::from(byte));
        }
    }

    fn write_u8(&mut self, value: u8) {
        self.write_u64(u64::from(value));
    }

    fn write_u16(&mut self, value: u16) {
        self.write_u64(u64::from(value));
    }

    fn write_u32(&mut self, value: u32) {
        self.write_u64(u64::from(value));
    }

    fn write_u64(&mut self, value: u64) {
        self.0 = (self.0.rotate_left(5) ^ value).wrapping_mul(0x517c_c1b7_2722_0a95);
    }

    fn write_usize(&mut self, value: usize) {
        self.write_u64(value as u64);
    }

    fn write_isize(&mut self, value: isize) {
        self.write_u64(value as u64);
    }
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

pub(super) fn ordered_state_queue(
    block: &ParsedBlock,
    alignment: u8,
    options: &Options,
    stop: &mut SearchStop<'_>,
    best: &mut PlannedBlock,
    seen: &mut SeenTokens,
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

/// Max only: the one caller checks `options.exhaustive`.
pub(super) fn match_group_search(
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
        for group in groups.iter().take(20) {
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
        for count in 2..=groups.len().min(5) {
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
