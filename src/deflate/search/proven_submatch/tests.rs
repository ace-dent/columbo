// SPDX-License-Identifier: MIT
// SPDX-FileCopyrightText: 2026 Andrew C.E. Dent <https://github.com/ace-dent>

use super::*;
use crate::deflate::huffman::FIXED_LITERAL_CODE_LENGTHS;
use crate::deflate::model::{canonical_length_encoding, Representation, SourceBlockType};
use crate::deflate::search::test_support::{
    decode_test_tokens, short_family_test_block, test_match,
};

fn assert_proven_submatch_rewrite(source: Token, replacement: &[Token], expected: &[Token]) {
    assert_eq!(replacement, expected);
    let seed: Vec<_> = b"abcdef".iter().copied().map(Token::Literal).collect();
    let mut source_tokens = seed.clone();
    source_tokens.push(source);
    let mut rewritten_tokens = seed;
    rewritten_tokens.extend_from_slice(replacement);
    assert_eq!(
        decode_test_tokens(&source_tokens),
        decode_test_tokens(&rewritten_tokens)
    );

    let Token::Match {
        distance,
        distance_symbol,
        ..
    } = source
    else {
        panic!("the test source must be a match");
    };
    for token in replacement {
        if let Token::Match {
            length,
            distance: rewritten_distance,
            length_symbol,
            distance_symbol: rewritten_distance_symbol,
        } = *token
        {
            assert_eq!(length_symbol, canonical_length_encoding(length).unwrap().0);
            assert_eq!(
                (rewritten_distance, rewritten_distance_symbol),
                (distance, distance_symbol)
            );
        }
    }
}

#[test]
fn proven_submatch_prices_match_exhaustive_spellings() {
    fn enumerate(plain: &[u8], prefix: &mut Vec<Token>, all: &mut Vec<Vec<Token>>) {
        if plain.is_empty() {
            all.push(prefix.clone());
            return;
        }
        prefix.push(Token::Literal(plain[0]));
        enumerate(&plain[1..], prefix, all);
        prefix.pop();
        for length in 3..=plain.len() {
            prefix.push(test_match(length as u16, 6, 4));
            enumerate(&plain[length..], prefix, all);
            prefix.pop();
        }
    }

    for length in 3..=10 {
        let decoded = &b"abcdefabcd"[..length];
        let source = test_match(length as u16, 6, 4);
        // Source wins a whole-span tie; other paths retain literal-first,
        // then ascending match-length order at each position.
        let mut spellings = vec![vec![source]];
        enumerate(decoded, &mut Vec::new(), &mut spellings);
        for profile in 0..32 {
            let mut literal = [0; 286];
            for (symbol, bits) in literal.iter_mut().enumerate() {
                *bits = ((profile + symbol * symbol) % 16) as u8;
            }
            let distances = [1; 30];
            for forbidden in [None, Some(257), Some(254 + length as u16)] {
                let expected = spellings
                    .iter()
                    .filter(|tokens| {
                        !tokens.iter().any(|token| {
                            matches!(token, Token::Match { length_symbol, .. }
                                if Some(*length_symbol) == forbidden)
                        })
                    })
                    .min_by_key(|tokens| estimated_tokens_bits(tokens, &literal, &distances))
                    .unwrap();
                let selected = solve_proven_submatch(
                    source,
                    decoded,
                    &literal,
                    &distances,
                    forbidden,
                    &mut SearchStop::never(),
                )
                .unwrap_or_else(|| vec![source]);
                assert_eq!(
                    &selected, expected,
                    "length {length}, profile {profile}, forbidden {forbidden:?}"
                );
            }
        }
    }
}

#[test]
fn forbidden_symbol_sets_match_exhaustive_spellings() {
    fn enumerate(plain: &[u8], prefix: &mut Vec<Token>, all: &mut Vec<Vec<Token>>) {
        if plain.is_empty() {
            all.push(prefix.clone());
            return;
        }
        prefix.push(Token::Literal(plain[0]));
        enumerate(&plain[1..], prefix, all);
        prefix.pop();
        for length in 3..=plain.len() {
            prefix.push(test_match(length as u16, 6, 4));
            enumerate(&plain[length..], prefix, all);
            prefix.pop();
        }
    }

    for length in 3..=10 {
        let decoded = &b"abcdefabcd"[..length];
        let source = test_match(length as u16, 6, 4);
        let mut spellings = vec![vec![source]];
        enumerate(decoded, &mut Vec::new(), &mut spellings);
        for profile in 0..8 {
            let mut literal = [0; 286];
            for (symbol, bits) in literal.iter_mut().enumerate() {
                *bits = ((3 * profile + symbol * symbol) % 16) as u8;
            }
            let distances = [1; 30];
            // Enumerate every subset of length symbols that can occur in
            // these short intervals, including disjoint and complete bans.
            for forbidden in 0..(1_u32 << (length - 2)) {
                let expected = spellings
                    .iter()
                    .filter(|tokens| {
                        tokens.iter().all(|token| match token {
                            Token::Match { length_symbol, .. } => {
                                forbidden & (1 << (length_symbol - 257)) == 0
                            }
                            Token::Literal(_) => true,
                        })
                    })
                    .min_by_key(|tokens| estimated_tokens_bits(tokens, &literal, &distances))
                    .unwrap();
                let actual = solve_proven_submatch_avoiding(
                    source,
                    decoded,
                    &literal,
                    &distances,
                    forbidden,
                    &mut SearchStop::never(),
                )
                .unwrap_or_else(|| vec![source]);
                assert_eq!(
                    &actual, expected,
                    "length={length}, profile={profile}, ban={forbidden}"
                );
            }
        }
    }
}

#[test]
fn long_submatch_families_match_the_direct_recurrence() {
    // Visit every length at every position, keeping the source on a
    // whole-span tie, then the literal, then ascending lengths.
    fn direct(
        source: Token,
        plain: &[u8],
        literal: &[u8],
        distances: &[u8],
        ban: u32,
    ) -> Vec<Token> {
        let allowed = |token: Token| match token {
            Token::Match { length_symbol, .. } => ban & (1 << (length_symbol - 257)) == 0,
            Token::Literal(_) => true,
        };
        let n = plain.len();
        let mut costs = vec![u64::MAX; n + 1];
        let mut choices = vec![source; n + 1];
        costs[n] = 0;
        for start in (0..n).rev() {
            let mut edges = Vec::new();
            if start == 0 {
                edges.push(source);
            }
            edges.push(Token::Literal(plain[start]));
            edges.extend(
                (3..=n - start).map(|length| repacked_match(source, length as u16).unwrap()),
            );
            for token in edges.into_iter().filter(|&token| allowed(token)) {
                let cost = estimated_tokens_bits(&[token], literal, distances).unwrap()
                    + costs[start + token.decoded_len()];
                if cost < costs[start] {
                    costs[start] = cost;
                    choices[start] = token;
                }
            }
        }
        let mut spelling = Vec::new();
        let mut at = 0;
        while at < n {
            spelling.push(choices[at]);
            at += choices[at].decoded_len();
        }
        spelling
    }

    let mut seed = 0x9e37_79b9_7f4a_7c15_u64;
    let mut next = move || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed
    };
    for round in 0..400 {
        let length = 3 + (next() % 256) as usize;
        let distance = 1 + (next() % 8) as usize;
        let mut plain = vec![0_u8; length];
        for at in 0..length {
            plain[at] = if at < distance {
                b'a' + (next() % 3) as u8
            } else {
                plain[at - distance]
            };
        }
        let mut literal = [0_u8; 286];
        let span = 1 + next() % 15;
        for bits in &mut literal {
            *bits = 1 + (next() % span) as u8;
        }
        let distances = [1 + (next() % 15) as u8; 30];
        let source = test_match(length as u16, distance as u16, 0);
        let ban = match round % 4 {
            0 => 0,
            1 => 1 << (next() % 29),
            2 => next() as u32 & 0x1fff_ffff,
            // Ban the source's own symbol so the whole span must be respelled.
            _ => match source {
                Token::Match { length_symbol, .. } => 1 << (length_symbol - 257),
                Token::Literal(_) => unreachable!(),
            },
        };
        let expected = direct(source, &plain, &literal, &distances, ban);
        let actual = solve_proven_submatch_avoiding(
            source,
            &plain,
            &literal,
            &distances,
            ban,
            &mut SearchStop::never(),
        )
        .unwrap_or_else(|| vec![source]);
        assert_eq!(
            actual, expected,
            "round={round}, length={length}, ban={ban:#x}"
        );
    }
}

#[test]
fn proven_submatch_graph_can_keep_a_literal_prefix_and_suffix_match() {
    let source = test_match(17, 6, 4);
    let decoded = b"abcdefabcdefabcde";
    let mut literal_lengths = [0_u8; 286];
    literal_lengths[usize::from(b'a')] = 1;
    for &byte in b"bcdef" {
        literal_lengths[usize::from(byte)] = 10;
    }
    literal_lengths[267] = 1;
    literal_lengths[268] = 15;
    let mut distance_lengths = [0_u8; 30];
    distance_lengths[4] = 1;

    let replacement = solve_proven_submatch(
        source,
        decoded,
        &literal_lengths,
        &distance_lengths,
        None,
        &mut SearchStop::never(),
    )
    .unwrap();
    assert_proven_submatch_rewrite(
        source,
        &replacement,
        &[Token::Literal(b'a'), test_match(16, 6, 4)],
    );
}

#[test]
fn proven_submatch_graph_can_keep_a_prefix_match_and_literal_suffix() {
    let source = test_match(17, 6, 4);
    let decoded = b"abcdefabcdefabcde";
    let mut literal_lengths = [0_u8; 286];
    for &byte in b"abcdf" {
        literal_lengths[usize::from(byte)] = 10;
    }
    literal_lengths[usize::from(b'e')] = 1;
    literal_lengths[267] = 1;
    literal_lengths[268] = 15;
    let mut distance_lengths = [0_u8; 30];
    distance_lengths[4] = 1;

    let replacement = solve_proven_submatch(
        source,
        decoded,
        &literal_lengths,
        &distance_lengths,
        None,
        &mut SearchStop::never(),
    )
    .unwrap();
    assert_proven_submatch_rewrite(
        source,
        &replacement,
        &[test_match(16, 6, 4), Token::Literal(b'e')],
    );
}

#[test]
fn proven_submatch_graph_can_emit_multiple_matches_at_the_proven_distance() {
    let source = test_match(17, 6, 4);
    let decoded = b"abcdefabcdefabcde";
    let mut literal_lengths = [0_u8; 286];
    literal_lengths.fill(15);
    literal_lengths[262] = 1;
    literal_lengths[263] = 1;
    literal_lengths[268] = 15;
    let mut distance_lengths = [0_u8; 30];
    distance_lengths[4] = 1;

    let replacement = solve_proven_submatch(
        source,
        decoded,
        &literal_lengths,
        &distance_lengths,
        None,
        &mut SearchStop::never(),
    )
    .unwrap();
    assert_proven_submatch_rewrite(
        source,
        &replacement,
        &[test_match(8, 6, 4), test_match(9, 6, 4)],
    );
}

#[test]
fn proven_submatch_source_symbol_free_path_exposes_payload_ties() {
    let source = test_match(17, 6, 4);
    let decoded = b"abcdefabcdefabcde";
    let mut literal_lengths = [0_u8; 286];
    literal_lengths.fill(15);
    literal_lengths[usize::from(b'a')] = 1;
    literal_lengths[267] = 4;
    literal_lengths[268] = 5;
    let mut distance_lengths = [0_u8; 30];
    distance_lengths[4] = 1;

    // Both spellings cost seven estimated payload bits. The exact source
    // remains the deterministic unrestricted winner.
    assert!(solve_proven_submatch(
        source,
        decoded,
        &literal_lengths,
        &distance_lengths,
        None,
        &mut SearchStop::never(),
    )
    .is_none());

    let replacement = solve_proven_submatch(
        source,
        decoded,
        &literal_lengths,
        &distance_lengths,
        Some(268),
        &mut SearchStop::never(),
    )
    .unwrap();
    assert_proven_submatch_rewrite(
        source,
        &replacement,
        &[Token::Literal(b'a'), test_match(16, 6, 4)],
    );
}

#[test]
fn proven_submatch_graph_handles_overlap_and_deadline_expiry() {
    let source = test_match(258, 1, 0);
    let decoded = [b'A'; 258];
    let mut literal_lengths = [0_u8; 286];
    literal_lengths[usize::from(b'A')] = 1;
    literal_lengths[285] = 15;
    let mut distance_lengths = [0_u8; 30];
    distance_lengths[0] = 1;
    let replacement = solve_proven_submatch(
        source,
        &decoded,
        &literal_lengths,
        &distance_lengths,
        Some(285),
        &mut SearchStop::never(),
    )
    .unwrap();
    let seed = [Token::Literal(b'A')];
    let mut source_tokens = seed.to_vec();
    source_tokens.push(source);
    let mut rewritten_tokens = seed.to_vec();
    rewritten_tokens.extend_from_slice(&replacement);
    assert_eq!(
        decode_test_tokens(&source_tokens),
        decode_test_tokens(&rewritten_tokens)
    );

    let mut checks = 0_usize;
    let mut expires = || {
        checks += 1;
        checks > 2
    };
    let result = solve_proven_submatch(
        source,
        &decoded,
        &literal_lengths,
        &distance_lengths,
        None,
        &mut SearchStop::callback(&mut expires),
    );
    assert!(result.is_none());
    assert!(checks > 2);
}

#[test]
fn compact_proven_feedback_uses_token_and_plain_work_bounds() {
    let matching = [test_match(4, 1, 0)];
    assert!(compact_proven_submatch_route_eligible(&matching, 4));
    assert!(!compact_proven_submatch_route_eligible(
        &[Token::Literal(0)],
        1
    ));
    assert!(!compact_proven_submatch_route_eligible(
        &vec![matching[0]; COMPACT_PROVEN_SUBMATCH_TOKENS + 1],
        4
    ));
    assert!(!compact_proven_submatch_route_eligible(
        &matching,
        MAX_PROVEN_SUBMATCH_FULL_PLAIN + 1
    ));
}

#[test]
fn proven_submatch_default_selection_is_ranked_and_bounded() {
    let common = test_match(39, 1, 0);
    let highest = test_match(258, 1, 0);
    let mut tokens = vec![common; 12];
    tokens.push(highest);
    let decoded_bytes: usize = tokens.iter().map(|token| token.decoded_len()).sum();
    let (literal_frequencies, _) = count_frequencies(&tokens);

    let default = select_proven_submatch_targets(
        &tokens,
        decoded_bytes,
        &[],
        &literal_frequencies,
        &FIXED_LITERAL_CODE_LENGTHS,
        false,
        false,
        &mut SearchStop::never(),
    )
    .unwrap();
    assert!(default.len() <= DEFAULT_PROVEN_SUBMATCH_TARGETS);
    assert_eq!(default[0].token_index, 12);
    assert!(default.iter().all(|target| target.token_index == 12));

    let exhaustive = select_proven_submatch_targets(
        &tokens,
        decoded_bytes,
        &[],
        &literal_frequencies,
        &FIXED_LITERAL_CODE_LENGTHS,
        true,
        true,
        &mut SearchStop::never(),
    )
    .unwrap();
    assert_eq!(exhaustive.len(), tokens.len());
    assert_eq!(
        exhaustive
            .iter()
            .map(|target| target.token_index)
            .collect::<Vec<_>>(),
        (0..tokens.len()).collect::<Vec<_>>()
    );
}

#[test]
fn proven_submatch_transition_targeting_checks_both_code_edges() {
    let symbol = match test_match(39, 1, 0) {
        Token::Match { length_symbol, .. } => length_symbol,
        Token::Literal(_) => unreachable!(),
    };
    assert!(length_near_code_transition(37, symbol));
    assert!(!length_near_code_transition(38, symbol));
    assert!(!length_near_code_transition(40, symbol));
    assert!(length_near_code_transition(41, symbol));
}

#[test]
fn proven_submatch_selection_reserves_space_across_length_symbols() {
    let common_highest = test_match(258, 1, 0);
    let rare_lower = test_match(17, 1, 0);
    let mut tokens = vec![common_highest; 12];
    tokens.push(rare_lower);
    let decoded_bytes: usize = tokens.iter().map(|token| token.decoded_len()).sum();
    let (literal_frequencies, _) = count_frequencies(&tokens);

    let targets = select_proven_submatch_targets(
        &tokens,
        decoded_bytes,
        &[],
        &literal_frequencies,
        &FIXED_LITERAL_CODE_LENGTHS,
        false,
        false,
        &mut SearchStop::never(),
    )
    .unwrap();
    assert!(targets.len() <= DEFAULT_PROVEN_SUBMATCH_TARGETS);
    assert!(targets.iter().any(|target| target.token_index == 12));
    assert!(
        targets
            .iter()
            .filter(|target| {
                matches!(
                    target.source,
                    Token::Match {
                        length_symbol: 285,
                        ..
                    }
                )
            })
            .count()
            <= DEFAULT_PROVEN_SUBMATCH_TARGETS_PER_SYMBOL
    );
}

#[test]
fn proven_submatch_single_rewrite_skips_a_duplicate_combined_price() {
    assert!(!should_price_combined_proven_submatch_candidate(0, 0));
    assert!(!should_price_combined_proven_submatch_candidate(1, 1));
    assert!(!should_price_combined_proven_submatch_candidate(1, 4));
    assert!(should_price_combined_proven_submatch_candidate(1, 0));
    assert!(should_price_combined_proven_submatch_candidate(2, 1));
}

#[test]
fn proven_submatch_materialization_rejects_invalid_rewrite_sets() {
    let source = test_match(17, 6, 4);
    let tokens = [source, source];
    let rank = ProvenSubmatchRank {
        highest: false,
        rare: false,
        near_boundary: false,
        transition: false,
        expensive: false,
        code_bits: 0,
        frequency: 0,
        token_index: 0,
    };
    let first = ProvenSubmatchRewrite {
        token_index: 0,
        replacement: vec![source],
        estimated_saving: 0,
        rank,
    };
    let duplicate = ProvenSubmatchRewrite {
        token_index: 0,
        replacement: vec![source],
        estimated_saving: 0,
        rank,
    };
    assert!(apply_proven_submatch_rewrites(&tokens, 34, &[first, duplicate]).is_none());

    let wrong_length = ProvenSubmatchRewrite {
        token_index: 0,
        replacement: vec![test_match(16, 6, 4)],
        estimated_saving: 0,
        rank,
    };
    assert!(apply_proven_submatch_rewrites(&tokens, 34, &[wrong_length]).is_none());
}

#[test]
fn proven_submatch_adoption_requires_a_strict_exact_bit_win() {
    let source = test_match(17, 6, 4);
    let mut source_tokens: Vec<_> = b"abcdef".iter().copied().map(Token::Literal).collect();
    source_tokens.push(source);
    let plain = decode_test_tokens(&source_tokens).unwrap();
    let block = short_family_test_block(source_tokens.clone(), plain);

    let mut candidate_tokens: Vec<_> = b"abcdef".iter().copied().map(Token::Literal).collect();
    candidate_tokens.push(Token::Literal(b'a'));
    candidate_tokens.push(test_match(16, 6, 4));
    assert_eq!(
        decode_test_tokens(&source_tokens),
        decode_test_tokens(&candidate_tokens)
    );

    let options = Options::default();
    let candidate = plan_tokens(
        &block,
        candidate_tokens.clone(),
        0,
        &options,
        &mut SearchStop::never(),
    )
    .unwrap();
    let incumbent = |bits| PlannedBlock {
        tokens: block.tokens.clone(),
        plain: block.plain.clone(),
        representation: Representation::Fixed,
        bits,
        source_type: SourceBlockType::Fixed,
    };

    let mut tie = incumbent(candidate.bits);
    let _ = consider_proven_submatch_tokens(
        &block,
        candidate_tokens.clone(),
        0,
        &options,
        &mut SearchStop::never(),
        &mut tie,
    );
    assert_eq!(tie.tokens.as_slice(), source_tokens);

    let mut strict_win = incumbent(candidate.bits + 1);
    let _ = consider_proven_submatch_tokens(
        &block,
        candidate_tokens.clone(),
        0,
        &options,
        &mut SearchStop::never(),
        &mut strict_win,
    );
    assert_eq!(strict_win.bits, candidate.bits);
    assert_eq!(strict_win.tokens.as_slice(), candidate_tokens);

    let mut priced_candidates = Vec::new();
    let mut deduplicated = incumbent(candidate.bits + 1);
    consider_unique_proven_submatch_tokens(
        &block,
        candidate_tokens.clone(),
        0,
        &options,
        &mut priced_candidates,
        &mut SearchStop::never(),
        &mut deduplicated,
    );
    assert_eq!(priced_candidates.len(), 1);
    consider_unique_proven_submatch_tokens(
        &block,
        candidate_tokens,
        0,
        &options,
        &mut priced_candidates,
        &mut SearchStop::never(),
        &mut deduplicated,
    );
    assert_eq!(priced_candidates.len(), 1);
    assert_eq!(deduplicated.bits, candidate.bits);
}
