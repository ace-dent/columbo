// SPDX-License-Identifier: MIT
// SPDX-FileCopyrightText: 2026 Andrew C.E. Dent <https://github.com/ace-dent>

//! Cumulative short-length bands.
//!
//! Expanding every match of a set of length symbols removes those symbols from
//! the literal/length alphabet, which frees code space and can shorten the
//! header even when each match alone is cheaper than its literals. Three
//! families price fixed bands of length symbols from the source tokens: the
//! short-family floor (lengths 6 to 10, priced from counts that add across
//! blocks, so block ranges can be scored without rescanning tokens), the
//! compact bands of the extended block floor, and the large-source bands of
//! the narrow search.

use crate::deflate::header::{plan_for_explicit_lengths_with_cost, token_bits_from_frequencies};
use crate::deflate::huffman::{make_lengths_deflopt_heap_into_with_scratch, TreeBuildScratch};
use crate::deflate::model::{
    count_frequencies, distance_extra_bits, frequency_extra_bits, length_extra_bits,
    token_extra_bits, DynamicPlan, ParsedBlock, PlannedBlock, Representation, Token,
};
use crate::deflate::parse::parsed_model_bytes;
use crate::deflate::stop::SearchStop;
use crate::Options;

use super::{
    consider_tokens, ensure_floor_distance_symbols, expand_selected_matches,
    MAX_TOKEN_CANDIDATE_BYTES, SHORT_FAMILY_MAX_TOKENS,
};

const SHORT_FAMILY_MAX_PLAIN: usize = 64_000_000;
const COMPACT_SHORT_BAND_MAX_TOKENS: usize = 12_000;
const COMPACT_SHORT_BAND_MAX_PLAIN: usize = 80_000;
const COMPACT_SHORT_BAND_ENDS: [u16; 6] = [257, 258, 259, 260, 262, 264];

/// For each end in `COMPACT_SHORT_BAND_ENDS`, expand every match whose length
/// symbol is in 257..=end; inspired by deft4j's least-family pruning.
pub(super) fn consider_compact_short_bands(
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

    // Each candidate expands the matches of length symbols 260 up to
    // `last_symbol`. One walk gives every family's count changes, so a
    // candidate is priced from counts, and its tokens are spelled only if it
    // wins.
    debug_assert_eq!(
        (block.literal_frequencies, block.distance_frequencies),
        count_frequencies(&block.tokens)
    );
    let Some(stats) = ShortFamilyStats::from_block(block) else {
        return best;
    };
    let mut counts = ShortFamilyCounts {
        literal: block.literal_frequencies,
        distance: block.distance_frequencies,
        extra_bits: frequency_extra_bits(&block.literal_frequencies, &block.distance_frequencies),
    };
    let mut removed_matches = 0_usize;
    let mut added_literals = 0_usize;
    for last_symbol in 260..=264_u16 {
        let family = usize::from(last_symbol - 260);
        counts.literal[usize::from(last_symbol)] -= stats.match_removals[family];
        for (count, &added) in counts
            .literal
            .iter_mut()
            .zip(&stats.literal_additions[family])
        {
            *count += added;
            added_literals += added as usize;
        }
        for (count, &removed) in counts
            .distance
            .iter_mut()
            .zip(&stats.distance_removals[family])
        {
            *count -= removed;
        }
        counts.extra_bits -= stats.extra_bits_removed[family];
        removed_matches += stats.match_removals[family] as usize;
        // As spelling them would, skip a candidate that expands nothing or
        // whose tokens would exceed the candidate model limit.
        let token_count = block.tokens.len() - removed_matches + added_literals;
        if removed_matches == 0
            || parsed_model_bytes(block.plain.len(), token_count, 1)
                .map_or(true, |bytes| bytes > MAX_TOKEN_CANDIDATE_BYTES)
        {
            continue;
        }
        debug_assert_eq!(
            Some(&counts),
            expand_short_families(block, last_symbol)
                .map(|tokens| ShortFamilyCounts::of(&tokens))
                .as_ref()
        );
        consider_short_family_tokens(block, last_symbol, &counts, &floor_options, &mut best);
    }
    best
}

/// The block's tokens with the matches of length symbols 260 to
/// `last_symbol` spelled as literals.
fn expand_short_families(block: &ParsedBlock, last_symbol: u16) -> Option<Vec<Token>> {
    expand_selected_matches(&block.tokens, &block.plain, |_, token, _| match token {
        Token::Match { length_symbol, .. } => (260..=last_symbol).contains(&length_symbol),
        Token::Literal(_) => false,
    })
}

/// A short-family candidate's symbol counts and extra bits.
#[derive(Debug, PartialEq, Eq)]
struct ShortFamilyCounts {
    literal: [u32; 286],
    distance: [u32; 30],
    extra_bits: u64,
}

impl ShortFamilyCounts {
    fn of(tokens: &[Token]) -> Self {
        let (literal, distance) = count_frequencies(tokens);
        Self {
            literal,
            distance,
            extra_bits: token_extra_bits(tokens),
        }
    }
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
                            u64::from(length_extra_bits(length_symbol))
                                + u64::from(distance_extra_bits(distance_symbol)),
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
    let mut heap_scratch = TreeBuildScratch::default();

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
        // Variants known to build both trees of a variant already priced.
        let mut repeated = 0_u8;
        for variant in 0..4 {
            if repeated & 1 << variant != 0 {
                continue;
            }
            let literal_build = make_lengths_deflopt_heap_into_with_scratch(
                &literal,
                &mut literal_lengths,
                15,
                variant,
                variant == 0,
                &mut heap_scratch,
            );
            let distance_build = make_lengths_deflopt_heap_into_with_scratch(
                &build_distance,
                &mut distance_lengths,
                15,
                variant,
                variant == 0,
                &mut heap_scratch,
            );
            let plan = token_bits_from_frequencies(
                &literal,
                &distance,
                &literal_lengths,
                &distance_lengths,
                remaining_extra_bits,
            )
            .and_then(|data_bits| {
                plan_for_explicit_lengths_with_cost(
                    &literal_lengths,
                    &distance_lengths,
                    data_bits,
                    false,
                )
            });
            if let Some(plan) = plan.filter(|plan| best.map_or(true, |bits| plan.bits < bits)) {
                best = Some(plan.bits);
            }
            // Those variants would price these same trees again.
            repeated |= literal_build.deflopt_variants & distance_build.deflopt_variants;
        }
    }
    best
}

fn consider_short_family_tokens(
    source: &ParsedBlock,
    last_symbol: u16,
    counts: &ShortFamilyCounts,
    options: &Options,
    best: &mut PlannedBlock,
) {
    let literal_frequencies = counts.literal;
    let token_distance_frequencies = counts.distance;
    let mut distance_frequencies = token_distance_frequencies;
    ensure_floor_distance_symbols(&mut distance_frequencies, options.strict);
    // The tokens' own counts, before the floor's added distance codes, price
    // each tree as a pass over the tokens would.
    let extra_bits = counts.extra_bits;
    let mut best_dynamic = None;
    let mut literal_lengths = [0_u8; 286];
    let mut distance_lengths = [0_u8; 30];
    let mut heap_scratch = TreeBuildScratch::default();
    // Variants known to build both trees of a variant already priced.
    let mut repeated = 0_u8;
    for variant in 0..4 {
        if repeated & 1 << variant != 0 {
            continue;
        }
        let literal_build = make_lengths_deflopt_heap_into_with_scratch(
            &literal_frequencies,
            &mut literal_lengths,
            15,
            variant,
            variant == 0,
            &mut heap_scratch,
        );
        let distance_build = make_lengths_deflopt_heap_into_with_scratch(
            &distance_frequencies,
            &mut distance_lengths,
            15,
            variant,
            variant == 0,
            &mut heap_scratch,
        );
        let dynamic = token_bits_from_frequencies(
            &literal_frequencies,
            &token_distance_frequencies,
            &literal_lengths,
            &distance_lengths,
            extra_bits,
        )
        .and_then(|data_bits| {
            plan_for_explicit_lengths_with_cost(
                &literal_lengths,
                &distance_lengths,
                data_bits,
                options.exhaustive,
            )
        });
        if let Some(dynamic) = dynamic.filter(|dynamic| {
            best_dynamic
                .as_ref()
                .map_or(true, |current: &DynamicPlan| dynamic.bits < current.bits)
        }) {
            best_dynamic = Some(dynamic);
        }
        // Those variants would price these same trees again.
        repeated |= literal_build.deflopt_variants & distance_build.deflopt_variants;
    }

    let Some(dynamic) = best_dynamic else {
        return;
    };
    if dynamic.bits < best.bits {
        let Some(tokens) = expand_short_families(source, last_symbol) else {
            return;
        };
        let bits = dynamic.bits;
        best.tokens = tokens.into();
        best.plain = source.plain.clone();
        best.representation = Representation::Dynamic(dynamic);
        best.bits = bits;
        best.source_type = source.source_type;
    }
}

/// Price length-symbol bands for the narrow search on large source blocks:
/// five cumulative bands 257..=end and one sparse set, `LONG_FAMILIES`.
pub(super) fn consider_large_source_bands(
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

#[cfg(test)]
mod tests;
