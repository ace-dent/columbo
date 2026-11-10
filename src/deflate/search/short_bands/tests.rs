// SPDX-License-Identifier: MIT
// SPDX-FileCopyrightText: 2026 Andrew C.E. Dent <https://github.com/ace-dent>

use super::*;
use crate::deflate::header::plan_for_explicit_lengths;
use crate::deflate::huffman::test_support::make_lengths_deflopt_heap;
use crate::deflate::search::test_support::{mixed_short_family_block, short_family_test_block};

#[test]
fn large_source_bands_have_explicit_model_bounds() {
    assert!(!large_source_bands_eligible(127_999, 1));
    assert!(large_source_bands_eligible(128_000, 80_000));
    assert!(!large_source_bands_eligible(128_000, 80_001));
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
