// SPDX-License-Identifier: MIT
// SPDX-FileCopyrightText: 2026 Andrew C.E. Dent <https://github.com/ace-dent>

use super::*;
use crate::deflate::header::plan_for_explicit_lengths;
use crate::deflate::header::test_support::{literal_span_test_block, payload_tradeoff_test_block};
use crate::deflate::huffman::{huffman_tree_shape_is_complete, make_lengths};
use crate::deflate::model::{count_frequencies, Token};

#[test]
fn literal_span_search_covers_shorter_and_longer_advertised_counts() {
    let base = literal_span_test_block();
    let original = base.original_dynamic.as_ref().unwrap();
    let data_bits = token_bits(
        &base.tokens,
        &original.literal_lengths,
        &original.distance_lengths,
    )
    .unwrap();
    for advertised in [269, 275, 286] {
        let mut block = base.clone();
        let mut literal = original.literal_lengths.clone();
        literal.resize(advertised, 0);
        let mut distance = original.distance_lengths.clone();
        distance.resize(32, 0); // Keep even a non-minimal distance span fixed.
        let parent =
            plan_for_trimmed_lengths_uncached(&literal, &distance, data_bits, true, 0xff).unwrap();
        let parent_bits = parent.bits;
        block.original_dynamic = Some(parent);
        let mut oracle = parent_bits;
        literal.resize(286, 0);
        for count in 269..=286 {
            oracle = oracle.min(
                plan_for_trimmed_lengths_uncached(
                    &literal[..count],
                    &distance,
                    data_bits,
                    true,
                    0xff,
                )
                .unwrap()
                .bits,
            );
        }
        let result = plan_literal_span(&block, true, &mut 1024, &mut SearchStop::never());
        assert_eq!(result.as_ref().map_or(parent_bits, |p| p.bits), oracle);
        if let Some(result) = result {
            assert_eq!(result.distance_lengths, distance);
            assert_eq!(&result.literal_lengths[..269], original.literal_lengths);
            assert!(result.has_strictly_compatible_huffman_codes());
        }
    }
}

#[test]
fn literal_span_search_respects_limits_and_retains_finished_prices() {
    let block = literal_span_test_block();
    assert!(plan_literal_span(&block, true, &mut 0, &mut SearchStop::never()).is_none());
    assert!(plan_literal_span(&block, true, &mut 1024, &mut SearchStop::always()).is_none());
    let mut one = 1;
    assert!(plan_literal_span(&block, true, &mut one, &mut SearchStop::never()).is_none());
    assert_eq!(one, 0);
    // The first eight prices reach 277 literal/length codes. Running out of
    // prices keeps that complete winner and tries no later count.
    let mut eight = 8;
    let result = plan_literal_span(&block, true, &mut eight, &mut SearchStop::never()).unwrap();
    assert_eq!((result.hlit, result.bits, eight), (277, 1293, 0));
    let mut polls = 0;
    let mut stop = || {
        polls += 1;
        polls > 10
    };
    let result = plan_literal_span(
        &block,
        true,
        &mut 1024,
        &mut SearchStop::callback(&mut stop),
    )
    .unwrap();
    assert_eq!((result.hlit, result.bits), (277, 1293));
    // A used symbol 285 leaves no legal larger count. Keep all payload
    // trees complete so this exercises the format bound, not rejection.
    let mut full = block.clone();
    let mut literal = [0_u8; 286];
    literal[0] = 1;
    literal[256] = 2;
    literal[285] = 2;
    full.tokens = vec![Token::Literal(0)].into();
    full.recount_frequencies();
    full.original_dynamic =
        Some(plan_for_explicit_lengths(&full.tokens, &literal, &[1, 1], true).unwrap());
    let mut budget = 1024;
    assert!(plan_literal_span(&full, true, &mut budget, &mut SearchStop::never()).is_none());
    assert_eq!(budget, 1024);
}

#[test]
fn positive_payload_swap_saves_more_in_the_header() {
    let block = payload_tradeoff_test_block();
    let parent = block.original_dynamic.as_ref().unwrap();
    let mut budget = 1024;
    let result =
        plan_payload_header_tradeoff(&block, true, &mut budget, &mut SearchStop::never()).unwrap();
    let payload = |plan: &DynamicPlan| {
        token_bits(&block.tokens, &plan.literal_lengths, &plan.distance_lengths).unwrap()
    };
    assert_eq!((parent.bits, payload(parent)), (582, 441));
    assert_eq!((result.bits, payload(&result)), (581, 444));
    assert!(result.has_strictly_compatible_huffman_codes());
    assert_eq!(parent.distance_lengths, result.distance_lengths);
    let mut old_lengths = parent.literal_lengths.clone();
    let mut new_lengths = result.literal_lengths.clone();
    old_lengths.sort_unstable();
    new_lengths.sort_unstable();
    assert_eq!(old_lengths, new_lengths);
    for (old, new) in parent.literal_lengths.iter().zip(&result.literal_lengths) {
        assert_eq!(*old == 0, *new == 0);
    }
}

#[test]
fn payload_swap_search_respects_stop_and_shared_price_budget() {
    let block = payload_tradeoff_test_block();
    assert!(plan_payload_header_tradeoff(&block, true, &mut 0, &mut SearchStop::never()).is_none());
    assert!(
        plan_payload_header_tradeoff(&block, true, &mut 1024, &mut SearchStop::always()).is_none()
    );
    // One price covers only the unchanged-tree repack, which ties the fully
    // priced parent, so no swap result may be returned.
    let mut budget = 1;
    assert!(
        plan_payload_header_tradeoff(&block, true, &mut budget, &mut SearchStop::never()).is_none()
    );
    assert_eq!(budget, 0);
    let mut polls = 0;
    let mut callback = || {
        polls += 1;
        polls > 4
    };
    assert!(plan_payload_header_tradeoff(
        &block,
        true,
        &mut 1024,
        &mut SearchStop::callback(&mut callback)
    )
    .is_none());
    assert!(polls > 4);
}

#[test]
fn local_swap_transition_delta_matches_full_sequence_oracle() {
    // Exhaustive short sequences include zero and nonzero runs, adjacent
    // swaps, endpoints and positions on either side of an alphabet boundary.
    for mut code in 0..3_usize.pow(6) {
        let mut lengths = [0_u8; 6];
        for length in &mut lengths {
            *length = (code % 3) as u8;
            code /= 3;
        }
        let transitions = |v: &[u8]| v.windows(2).filter(|p| p[0] != p[1]).count() as i64;
        for a in 0..lengths.len() {
            for b in a + 1..lengths.len() {
                let mut swapped = lengths;
                swapped.swap(a, b);
                assert_eq!(
                    swap_transitions_removed(&lengths, a, b),
                    transitions(&lengths) - transitions(&swapped)
                );
            }
        }
    }
}

#[test]
fn strict_distance_completions_are_complete_uniform_and_cover_the_used_symbol() {
    let empty = strict_distance_completions(&[0; 30]);
    assert_eq!(empty.len(), 4);
    for (depth, lengths) in (1..=4_u8).zip(&empty) {
        assert!(huffman_tree_shape_is_complete(lengths));
        assert_eq!(&lengths[..1 << depth], &vec![depth; 1 << depth][..]);
        assert!(lengths[1 << depth..].iter().all(|&length| length == 0));
    }

    for used in [0, 7, 29] {
        let mut frequencies = [0_u32; 30];
        frequencies[used] = 5;
        let completions = strict_distance_completions(&frequencies);
        assert_eq!(completions.len(), 4);
        for lengths in &completions {
            assert!(huffman_tree_shape_is_complete(lengths));
            assert_ne!(lengths[used], 0);
        }
    }

    let mut two_used = [0_u32; 30];
    two_used[3] = 1;
    two_used[4] = 1;
    assert!(strict_distance_completions(&two_used).is_empty());
}

#[test]
fn strict_distance_completion_replaces_a_costly_one_bit_pair() {
    // A literal-only block whose planned `[1, 1]` distance completion is the
    // only reason code-length symbol 1 is sent, and so 18 code-length code
    // lengths.
    let block = payload_tradeoff_test_block();
    let original = block.original_dynamic.as_ref().unwrap();
    assert_eq!(&original.distance_lengths[..2], &[1, 1]);
    assert_eq!(original.hclen, 18);

    let mut prices = 4;
    let completed =
        plan_strict_distance_completion(&block, true, &mut prices, &mut SearchStop::never())
            .unwrap();
    assert!(completed.bits < original.bits);
    assert!(completed.has_strictly_compatible_huffman_codes());
    assert_eq!(completed.literal_lengths, original.literal_lengths);
    assert!(completed.hclen < original.hclen);
    assert_eq!(prices, 0);

    let mut prices = 4;
    assert!(
        plan_strict_distance_completion(&block, false, &mut prices, &mut SearchStop::never())
            .is_none()
    );
    assert_eq!(prices, 4);
}

#[test]
fn strict_completion_payload_adjustment_matches_a_full_token_scan() {
    let mut tokens: Vec<Token> = (0..40_u8).map(|byte| Token::Literal(byte % 7)).collect();
    tokens.extend(
        std::iter::repeat(Token::Match {
            length: 3,
            distance: 4,
            length_symbol: 257,
            distance_symbol: 3,
            length_extra: 0,
            distance_extra: 0,
            length_extra_bits: 0,
            distance_extra_bits: 0,
        })
        .take(5),
    );
    let (literal_frequencies, distance_frequencies) = count_frequencies(&tokens);
    let literal = make_lengths(&literal_frequencies, 15, 0);
    let literal = &literal[..trim_literal(&literal)];
    for original in [&[0, 0, 1, 1][..], &[0, 0, 0, 1, 1][..]] {
        let base = token_bits(&tokens, literal, original).unwrap();
        for completion in strict_distance_completions(&distance_frequencies) {
            let completion = &completion[..trim_distance(&completion)];
            assert_eq!(
                completed_distance_data_bits(base, &distance_frequencies, original, completion),
                token_bits(&tokens, literal, completion),
            );
        }
    }

    let literal_only: Vec<Token> = (0..20_u8).map(Token::Literal).collect();
    let (literal_frequencies, distance_frequencies) = count_frequencies(&literal_only);
    let literal = make_lengths(&literal_frequencies, 15, 0);
    let literal = &literal[..trim_literal(&literal)];
    let base = token_bits(&literal_only, literal, &[1, 1]).unwrap();
    for completion in strict_distance_completions(&distance_frequencies) {
        let completion = &completion[..trim_distance(&completion)];
        assert_eq!(
            completed_distance_data_bits(base, &distance_frequencies, &[1, 1], completion),
            token_bits(&literal_only, literal, completion),
        );
    }
}
