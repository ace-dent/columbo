// SPDX-License-Identifier: MIT
// SPDX-FileCopyrightText: 2026 Andrew C.E. Dent <https://github.com/ace-dent>

use super::terminal::has_dearer_match;
use super::*;
use crate::deflate::model::{
    OriginalBits, SourceBlockType, DISTANCE_BASE, DISTANCE_EXTRA_BITS, LENGTH_BASE,
    LENGTH_EXTRA_BITS,
};
use crate::deflate::search::test_support::{
    mixed_short_family_block, short_family_test_block, test_match,
};

/// Price the cumulative short-length bands (symbols 260, then 260..=261, up to
/// 260..=264) after the same-distance floor.
fn plan_block_with_short_family_floor(
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

    let aliased =
        rewrite_258_symbols(&[canonical], 258, true).expect("the bounded one-token rewrite fits");
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
    for length in (0..3).chain(259..=u16::MAX) {
        assert!(
            canonical_length_encoding(length).is_none(),
            "length {length}"
        );
    }
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
    let fresh_deft4j = improve_plan_with_deft4j_tree_floor(&block, 3, &options, fresh_deft4j_base);
    let cloned_deft4j_base = plan_block(&block, 3, &floor_options, &mut SearchStop::never());
    let cloned_deft4j_base =
        try_clone_planned_block(&cloned_deft4j_base).expect("small plan metadata is cloneable");
    let cloned_deft4j =
        improve_plan_with_deft4j_tree_floor(&block, 3, &options, cloned_deft4j_base);
    assert_same_plan(&cloned_deft4j, &fresh_deft4j);
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

    // One byte below the budget must reject the expansion before allocation.
    assert!(
        expand_selected_matches_with_limit(&source, plain, exact_limit - 1, |_, _, _| true,)
            .is_none()
    );

    let expanded = expand_selected_matches_with_limit(&source, plain, exact_limit, |_, _, _| true)
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

/// Random tokens over `plain`, with code lengths that leave some symbols absent.
fn random_literal_sum_case(
    next: &mut impl FnMut() -> u32,
) -> (Vec<Token>, Vec<u8>, Vec<u8>, Vec<u8>) {
    let mut tokens = Vec::new();
    let mut plain = Vec::new();
    for _ in 0..(next() % 24 + 1) {
        if next() % 3 == 0 {
            let byte = (next() % 8) as u8;
            tokens.push(Token::Literal(byte));
            plain.push(byte);
        } else {
            let length = [3, 4, 9, 18, 67, 130, 258][(next() % 7) as usize];
            let distance_symbol = (next() % 30) as u8;
            let distance = DISTANCE_BASE[usize::from(distance_symbol)];
            tokens.push(test_match(
                length,
                distance,
                distance_symbol,
                0,
                DISTANCE_EXTRA_BITS[usize::from(distance_symbol)],
            ));
            plain.extend((0..length).map(|_| (next() % 8) as u8));
        }
    }
    let lengths = |count: usize, next: &mut dyn FnMut() -> u32| -> Vec<u8> {
        (0..count)
            .map(|_| {
                if next() % 5 == 0 {
                    0
                } else {
                    (next() % 15 + 1) as u8
                }
            })
            .collect()
    };
    let literal_lengths = lengths(286, next);
    let distance_lengths = lengths(30, next);
    (tokens, plain, literal_lengths, distance_lengths)
}

#[test]
fn early_stopping_literal_sums_match_full_sums() {
    let mut state = 0x6c07_8965_u32;
    let mut next = move || {
        state ^= state << 13;
        state ^= state >> 17;
        state ^= state << 5;
        state
    };
    for case in 0..20_000 {
        let (tokens, plain, literal_lengths, distance_lengths) = random_literal_sum_case(&mut next);
        let match_bits = |length_symbol: u16,
                          distance_symbol: u8,
                          length_extra_bits: u8,
                          distance_extra_bits: u8| {
            estimated_length(&literal_lengths, usize::from(length_symbol))
                + u64::from(length_extra_bits)
                + estimated_length(&distance_lengths, usize::from(distance_symbol))
                + u64::from(distance_extra_bits)
        };
        let full_literal_bits = |decoded: &[u8]| -> u64 {
            decoded
                .iter()
                .map(|&byte| estimated_length(&literal_lengths, usize::from(byte)))
                .sum()
        };
        for include_equal in [false, true] {
            let expected = expand_selected_matches_with_limits(
                &tokens,
                &plain,
                MAX_TOKEN_CANDIDATE_BYTES,
                usize::MAX,
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
                    let literal_bits = full_literal_bits(decoded);
                    let match_bits = match_bits(
                        length_symbol,
                        distance_symbol,
                        length_extra_bits,
                        distance_extra_bits,
                    );
                    literal_bits < match_bits || (include_equal && literal_bits == match_bits)
                },
            );
            let actual = expand_matches_with_token_limit(
                &tokens,
                &plain,
                &literal_lengths,
                &distance_lengths,
                include_equal,
                usize::MAX,
            );
            assert_eq!(
                actual, expected,
                "case {case} include_equal {include_equal}"
            );
        }

        let mut offset = 0;
        let mut expected_dearer = false;
        for &token in &tokens {
            if let Token::Match {
                length,
                length_symbol,
                distance_symbol,
                length_extra_bits,
                distance_extra_bits,
                ..
            } = token
            {
                let decoded = &plain[offset..offset + usize::from(length)];
                expected_dearer |= full_literal_bits(decoded)
                    < match_bits(
                        length_symbol,
                        distance_symbol,
                        length_extra_bits,
                        distance_extra_bits,
                    );
            }
            offset += token.decoded_len();
        }
        assert_eq!(
            has_dearer_match(&tokens, &plain, &literal_lengths, &distance_lengths),
            expected_dearer,
            "case {case}"
        );

        let expected_defluff = expand_selected_matches(&tokens, &plain, |_, token, decoded| {
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
            if !decoded
                .iter()
                .all(|&byte| literal_lengths[usize::from(byte)] != 0)
            {
                return false;
            }
            let literal_bits: u64 = decoded
                .iter()
                .map(|&byte| u64::from(literal_lengths[usize::from(byte)]))
                .sum();
            literal_bits
                < u64::from(match_literal_length)
                    + u64::from(length_extra_bits)
                    + u64::from(match_distance_length)
                    + u64::from(distance_extra_bits)
        });
        assert_eq!(
            expand_defluff_matches(&tokens, &plain, &literal_lengths, &distance_lengths),
            expected_defluff,
            "case {case}"
        );
    }
}

/// Match expansion before it kept the first walk's answers: both walks ask
/// `should_expand`.
fn expand_selected_matches_reference<F>(
    source: &[Token],
    plain: &[u8],
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
        output_count = output_count.checked_add(if expand { decoded.len() } else { 1 })?;
        if output_count > token_limit {
            return None;
        }
        changed |= expand;
        plain_offset = end;
    }
    if !changed || plain_offset != plain.len() {
        return None;
    }
    let mut output = Vec::new();
    plain_offset = 0;
    for (index, &token) in source.iter().enumerate() {
        let end = plain_offset.checked_add(token.decoded_len())?;
        let decoded = plain.get(plain_offset..end)?;
        if matches!(token, Token::Match { .. }) && should_expand(index, token, decoded) {
            output.extend(decoded.iter().copied().map(Token::Literal));
        } else {
            output.push(token);
        }
        plain_offset = end;
    }
    Some(output)
}

#[test]
fn match_expansion_reuses_its_first_walk_answers() {
    let mut state = 0x1b87_3593_u32;
    let mut next = move || {
        state ^= state << 13;
        state ^= state >> 17;
        state ^= state << 5;
        state
    };
    let mut expanded = 0;
    for case in 0..20_000 {
        let (tokens, mut plain, _, _) = random_literal_sum_case(&mut next);
        // A short decoded buffer makes a token overrun.
        if case % 40 == 0 {
            plain.pop();
        }
        let salt = next();
        let should_expand = |index: usize, token: Token, decoded: &[u8]| {
            let Token::Match { length_symbol, .. } = token else {
                return false;
            };
            let mixed = (index as u32 ^ salt)
                .wrapping_mul(0x9e37_79b9)
                .wrapping_add(
                    u32::from(length_symbol) + decoded.first().copied().unwrap_or(0) as u32,
                );
            mixed % [2, 3, 7][case % 3] == 0
        };
        let token_limit = [usize::MAX, 40, 300][case % 3];
        let expected =
            expand_selected_matches_reference(&tokens, &plain, token_limit, should_expand);
        let actual = expand_selected_matches_with_limits(
            &tokens,
            &plain,
            MAX_TOKEN_CANDIDATE_BYTES,
            token_limit,
            should_expand,
        );
        assert_eq!(actual, expected, "case {case}");
        expanded += usize::from(expected.is_some());
    }
    assert!(expanded > 5_000, "only {expanded} cases expanded a match");
}

#[test]
fn one_walk_expands_both_strict_and_non_larger_matches() {
    let mut state = 0x3c6e_f372_u32;
    let mut next = move || {
        state ^= state << 13;
        state ^= state >> 17;
        state ^= state << 5;
        state
    };
    let mut both = 0;
    for case in 0..20_000 {
        let (tokens, mut plain, literal_lengths, distance_lengths) =
            random_literal_sum_case(&mut next);
        // A short decoded buffer makes a token overrun.
        if case % 40 == 0 {
            plain.pop();
        }
        let expected = [false, true].map(|include_equal| {
            expand_matches(
                &tokens,
                &plain,
                &literal_lengths,
                &distance_lengths,
                include_equal,
            )
        });
        assert_eq!(
            expand_matches_strict_and_non_larger(
                &tokens,
                &plain,
                &literal_lengths,
                &distance_lengths
            ),
            expected,
            "case {case}"
        );
        both += usize::from(expected.iter().all(Option::is_some));
    }
    assert!(both > 2_000, "only {both} cases expanded both ways");
}

#[test]
fn two_table_symbol_counts_match_one_pass() {
    let mut state = 0x2545_f491_u32;
    let mut next = move || {
        state ^= state << 13;
        state ^= state >> 17;
        state ^= state << 5;
        state
    };
    let one_pass = |tokens: &[Token]| {
        let mut literal = [0_u32; 286];
        let mut distance = [0_u32; 30];
        for token in tokens {
            match *token {
                Token::Literal(value) => literal[usize::from(value)] += 1,
                Token::Match {
                    length_symbol,
                    distance_symbol,
                    ..
                } => {
                    literal[usize::from(length_symbol)] += 1;
                    distance[usize::from(distance_symbol)] += 1;
                }
            }
        }
        literal[256] += 1;
        (literal, distance)
    };
    assert_eq!(count_frequencies(&[]), one_pass(&[]));
    for case in 0..4_000 {
        // One to 24 tokens: odd counts leave a token past the last pair.
        let (tokens, _, _, _) = random_literal_sum_case(&mut next);
        assert_eq!(count_frequencies(&tokens), one_pass(&tokens), "case {case}");
    }
}
