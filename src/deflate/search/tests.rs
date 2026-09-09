// SPDX-License-Identifier: MIT

use super::*;
use crate::deflate::huffman::make_lengths_deflopt_heap;
use crate::deflate::model::{
    token_extra_bits, OriginalBits, SourceBlockType, LENGTH_BASE, LENGTH_EXTRA_BITS,
};

/// Price Columbo's bounded cumulative length-symbol-family states.
///
/// Some merged photographic blocks get a smaller table only after all
/// existing matches in symbol 260, then symbols 260..=261, and so on through
/// symbol 264 are written as literals. The family is inspired by repeated
/// deft4j least-family pruning, but the fixed cumulative bands are Columbo's.
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

fn test_match(
    length: u16,
    distance: u16,
    distance_symbol: u8,
    distance_extra: u16,
    distance_extra_bits: u8,
) -> Token {
    let (length_symbol, length_extra, length_extra_bits) =
        canonical_length_encoding(length).expect("test length is legal");
    Token::Match {
        length,
        distance,
        length_symbol,
        distance_symbol,
        length_extra,
        distance_extra,
        length_extra_bits,
        distance_extra_bits,
    }
}

fn test_repack(
    tokens: &[Token],
    decoded_bytes: usize,
    literal_lengths: &[u8],
    exhaustive: bool,
) -> Option<Vec<Token>> {
    repack_same_distance_runs(
        tokens,
        decoded_bytes,
        literal_lengths,
        exhaustive,
        &mut SearchStop::never(),
    )
}

fn test_partitioner(
    literal_lengths: &[u8],
    max_active: usize,
    max_deficit: usize,
) -> Option<SameDistancePartitioner> {
    SameDistancePartitioner::new(
        literal_lengths,
        max_active,
        max_deficit,
        &mut SearchStop::never(),
    )
}

fn decode_test_tokens(tokens: &[Token]) -> Option<Vec<u8>> {
    let decoded_len = tokens.iter().try_fold(0_usize, |total, token| {
        total.checked_add(token.decoded_len())
    })?;
    let mut decoded = Vec::new();
    decoded.try_reserve_exact(decoded_len).ok()?;
    for &token in tokens {
        match token {
            Token::Literal(value) => decoded.push(value),
            Token::Match {
                length, distance, ..
            } => {
                let distance = usize::from(distance);
                if distance == 0 || distance > decoded.len() {
                    return None;
                }
                for _ in 0..length {
                    let source = decoded.len().checked_sub(distance)?;
                    let value = *decoded.get(source)?;
                    decoded.push(value);
                }
            }
        }
    }
    Some(decoded)
}

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
        distance_extra,
        distance_extra_bits,
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
            length_extra,
            distance_extra: rewritten_distance_extra,
            length_extra_bits,
            distance_extra_bits: rewritten_distance_extra_bits,
        } = *token
        {
            assert_eq!(
                (length_symbol, length_extra, length_extra_bits),
                canonical_length_encoding(length).unwrap()
            );
            assert_eq!(
                (
                    rewritten_distance,
                    rewritten_distance_symbol,
                    rewritten_distance_extra,
                    rewritten_distance_extra_bits,
                ),
                (
                    distance,
                    distance_symbol,
                    distance_extra,
                    distance_extra_bits,
                )
            );
        }
    }
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
            prefix.push(test_match(length as u16, 6, 4, 1, 1));
            enumerate(&plain[length..], prefix, all);
            prefix.pop();
        }
    }

    for length in 3..=10 {
        let decoded = &b"abcdefabcd"[..length];
        let source = test_match(length as u16, 6, 4, 1, 1);
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
            prefix.push(test_match(length as u16, 6, 4, 1, 1));
            enumerate(&plain[length..], prefix, all);
            prefix.pop();
        }
    }

    for length in 3..=10 {
        let decoded = &b"abcdefabcd"[..length];
        let source = test_match(length as u16, 6, 4, 1, 1);
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
fn proven_submatch_graph_can_keep_a_literal_prefix_and_suffix_match() {
    let source = test_match(17, 6, 4, 1, 1);
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
        &[Token::Literal(b'a'), test_match(16, 6, 4, 1, 1)],
    );
}

#[test]
fn proven_submatch_graph_can_keep_a_prefix_match_and_literal_suffix() {
    let source = test_match(17, 6, 4, 1, 1);
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
        &[test_match(16, 6, 4, 1, 1), Token::Literal(b'e')],
    );
}

#[test]
fn proven_submatch_graph_can_emit_multiple_matches_at_the_proven_distance() {
    let source = test_match(17, 6, 4, 1, 1);
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
        &[test_match(8, 6, 4, 1, 1), test_match(9, 6, 4, 1, 1)],
    );
}

#[test]
fn proven_submatch_source_symbol_free_path_exposes_payload_ties() {
    let source = test_match(17, 6, 4, 1, 1);
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
        &[Token::Literal(b'a'), test_match(16, 6, 4, 1, 1)],
    );
}

#[test]
fn proven_submatch_graph_handles_overlap_and_deadline_expiry() {
    let source = test_match(258, 1, 0, 0, 0);
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
    let matching = [test_match(4, 1, 0, 0, 0)];
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
    let common = test_match(39, 1, 0, 0, 0);
    let highest = test_match(258, 1, 0, 0, 0);
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
    let symbol = match test_match(39, 1, 0, 0, 0) {
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
    let common_highest = test_match(258, 1, 0, 0, 0);
    let rare_lower = test_match(17, 1, 0, 0, 0);
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
    let source = test_match(17, 6, 4, 1, 1);
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
        replacement: vec![test_match(16, 6, 4, 1, 1)],
        estimated_saving: 0,
        rank,
    };
    assert!(apply_proven_submatch_rewrites(&tokens, 34, &[wrong_length]).is_none());
}

#[test]
fn proven_composition_frequency_state_matches_materialized_tokens() {
    let source_match = test_match(6, 6, 4, 1, 1);
    let mut source: Vec<_> = b"abcdef".iter().copied().map(Token::Literal).collect();
    source.extend([source_match, source_match]);
    let decoded = b"abcdefabcdefabcdef";
    let rank = ProvenSubmatchRank {
        highest: true,
        rare: true,
        near_boundary: false,
        transition: true,
        expensive: false,
        code_bits: 4,
        frequency: 2,
        token_index: 6,
    };
    let literal_replacement: Vec<_> = b"abcdef".iter().copied().map(Token::Literal).collect();
    let menus = vec![
        ProvenCompositionMenu {
            token_index: 6,
            source: source_match,
            alternatives: vec![ProvenSubmatchRewrite {
                token_index: 6,
                replacement: literal_replacement.clone(),
                estimated_saving: -3,
                rank,
            }],
        },
        ProvenCompositionMenu {
            token_index: 7,
            source: source_match,
            alternatives: vec![ProvenSubmatchRewrite {
                token_index: 7,
                replacement: literal_replacement,
                estimated_saving: -3,
                rank: ProvenSubmatchRank {
                    token_index: 7,
                    ..rank
                },
            }],
        },
    ];
    let (literal_frequencies, distance_frequencies) = count_frequencies(&source);
    let mut state = ProvenCompositionState {
        literal_frequencies,
        distance_frequencies,
        extra_bits: token_extra_bits(&source),
        estimated_delta: 6,
        choices: [1, 1, 0, 0, 0, 0, 0, 0],
        rewrite_count: 2,
    };
    for menu in &menus {
        assert!(apply_proven_composition_frequency_delta(
            &mut state,
            menu.source,
            &menu.alternatives[0].replacement,
        ));
    }

    let materialized = apply_proven_composition_state(&source, decoded.len(), &menus, &state)
        .expect("the two nonoverlapping menu choices materialize");
    assert_eq!(
        decode_test_tokens(&materialized).as_deref(),
        Some(decoded.as_slice())
    );
    assert_eq!(
        count_frequencies(&materialized).0,
        state.literal_frequencies
    );
    assert_eq!(
        count_frequencies(&materialized).1,
        state.distance_frequencies
    );
    assert_eq!(token_extra_bits(&materialized), state.extra_bits);
}

#[test]
fn proven_composition_forward_and_reverse_moves_restore_the_exact_state() {
    let source_match = test_match(6, 6, 4, 1, 1);
    let mut source: Vec<_> = b"abcdef".iter().copied().map(Token::Literal).collect();
    source.extend([source_match, source_match, source_match]);
    let decoded = b"abcdefabcdefabcdefabcdef";
    let literal_replacement: Vec<_> = b"abcdef".iter().copied().map(Token::Literal).collect();
    let menus: Vec<_> = (0..3)
        .map(|index| {
            let token_index = 6 + index;
            ProvenCompositionMenu {
                token_index,
                source: source_match,
                alternatives: vec![ProvenSubmatchRewrite {
                    token_index,
                    replacement: literal_replacement.clone(),
                    estimated_saving: index as i64 - 1,
                    rank: ProvenSubmatchRank {
                        highest: true,
                        rare: true,
                        near_boundary: false,
                        transition: true,
                        expensive: false,
                        code_bits: 4,
                        frequency: 3,
                        token_index,
                    },
                }],
            }
        })
        .collect();

    let (root, moves) = ranked_forward_proven_composition_moves(&source, &menus)
        .expect("the three independent moves are rankable");
    assert_eq!(moves.len(), 3);
    let mut aggressive = root.clone();
    for &movement in &moves {
        assert!(apply_proven_composition_move(
            &mut aggressive,
            &menus,
            movement,
            true,
        ));
    }
    let materialized = apply_proven_composition_state(&source, decoded.len(), &menus, &aggressive)
        .expect("the aggressive endpoint materializes");
    assert_eq!(
        decode_test_tokens(&materialized).as_deref(),
        Some(decoded.as_slice())
    );
    assert_eq!(
        count_frequencies(&materialized).0,
        aggressive.literal_frequencies
    );
    assert_eq!(
        count_frequencies(&materialized).1,
        aggressive.distance_frequencies
    );
    assert_eq!(token_extra_bits(&materialized), aggressive.extra_bits);

    let (literal_lengths, distance_lengths) = fixed_lengths();
    let reverse = ranked_reverse_proven_composition_moves(
        &menus,
        &moves,
        &aggressive,
        &literal_lengths,
        &distance_lengths,
    )
    .expect("the aggressive endpoint has a reverse ranking");
    assert_eq!(reverse.len(), moves.len());
    let mut repaired = aggressive;
    for movement in reverse {
        assert!(apply_proven_composition_move(
            &mut repaired,
            &menus,
            movement,
            false,
        ));
    }
    assert_eq!(repaired.literal_frequencies, root.literal_frequencies);
    assert_eq!(repaired.distance_frequencies, root.distance_frequencies);
    assert_eq!(repaired.extra_bits, root.extra_bits);
    assert_eq!(repaired.estimated_delta, root.estimated_delta);
    assert_eq!(repaired.choices, root.choices);
    assert_eq!(repaired.rewrite_count, root.rewrite_count);
}

#[test]
fn closed_loop_proven_composition_retains_a_complete_strict_incumbent() {
    let source_match = test_match(6, 6, 4, 1, 1);
    let mut source: Vec<_> = b"abcdef".iter().copied().map(Token::Literal).collect();
    source.extend([source_match, source_match, source_match]);
    let plain = decode_test_tokens(&source).expect("the source token stream is valid");
    let block = short_family_test_block(source.clone(), plain);
    let mut incumbent = PlannedBlock {
        tokens: source.clone().into(),
        plain: block.plain.clone(),
        representation: Representation::Fixed,
        bits: 0,
        source_type: SourceBlockType::Fixed,
    };
    let options = Options {
        exhaustive: true,
        ..Options::default()
    };
    consider_closed_loop_proven_composition(
        &block,
        0,
        &options,
        &mut Vec::new(),
        &mut SearchStop::never(),
        &mut incumbent,
    );
    assert_eq!(incumbent.bits, 0);
    assert_eq!(incumbent.tokens.as_slice(), source);
}

#[test]
fn proven_composition_deduplicates_header_equivalent_spellings() {
    let left = [Token::Literal(b'a'), Token::Literal(b'b')];
    let right = [Token::Literal(b'b'), Token::Literal(b'a')];
    assert_ne!(left, right);
    assert!(same_proven_composition_spelling_cost(&left, &right));

    let source = test_match(6, 6, 4, 1, 1);
    let mut alternatives = Vec::new();
    let rank = ProvenSubmatchRank {
        highest: true,
        rare: true,
        near_boundary: false,
        transition: false,
        expensive: false,
        code_bits: 4,
        frequency: 1,
        token_index: 0,
    };
    for replacement in [left.to_vec(), right.to_vec()] {
        insert_distinct_proven_composition_alternative(
            &mut alternatives,
            source,
            ProvenSubmatchRewrite {
                token_index: 0,
                replacement,
                estimated_saving: 0,
                rank,
            },
        );
    }
    assert_eq!(alternatives.len(), 1);
}

#[test]
fn proven_submatch_adoption_requires_a_strict_exact_bit_win() {
    let source = test_match(17, 6, 4, 1, 1);
    let mut source_tokens: Vec<_> = b"abcdef".iter().copied().map(Token::Literal).collect();
    source_tokens.push(source);
    let plain = decode_test_tokens(&source_tokens).unwrap();
    let block = short_family_test_block(source_tokens.clone(), plain);

    let mut candidate_tokens: Vec<_> = b"abcdef".iter().copied().map(Token::Literal).collect();
    candidate_tokens.push(Token::Literal(b'a'));
    candidate_tokens.push(test_match(16, 6, 4, 1, 1));
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

#[test]
fn same_distance_stats_count_maximal_runs() {
    let tokens = vec![
        Token::Literal(b'x'),
        test_match(3, 1, 0, 0, 0),
        test_match(4, 1, 0, 0, 0),
        test_match(5, 2, 1, 0, 0),
        Token::Literal(b'y'),
        test_match(130, 3, 2, 0, 0),
        test_match(130, 3, 2, 0, 0),
        // Distances five and six share symbol four, but are not one run.
        test_match(3, 5, 4, 0, 1),
        test_match(3, 6, 4, 1, 1),
    ];
    let plain_len: usize = tokens.iter().map(|token| token.decoded_len()).sum();
    let block = short_family_test_block(tokens, vec![0; plain_len]);

    assert_eq!(
        same_distance_opportunities(&[block]),
        SameDistanceOpportunities {
            runs: 2,
            matches: 4,
            decoded_bytes: 267,
            coalescible_runs: 1,
            repartition_runs: 1,
            tokens_removable: 1,
        }
    );
}

#[test]
fn same_distance_stats_join_source_blocks_but_skip_unchanged_minimum_runs() {
    let first = short_family_test_block(vec![test_match(100, 1, 0, 0, 0)], vec![0; 100]);
    let empty = short_family_test_block(Vec::new(), Vec::new());
    let last = short_family_test_block(
        vec![
            test_match(158, 1, 0, 0, 0),
            Token::Literal(b'x'),
            test_match(258, 2, 1, 0, 0),
            test_match(258, 2, 1, 0, 0),
        ],
        vec![0; 675],
    );

    assert_eq!(
        same_distance_opportunities(&[first, empty, last]),
        SameDistanceOpportunities {
            runs: 2,
            matches: 4,
            decoded_bytes: 774,
            coalescible_runs: 1,
            repartition_runs: 0,
            tokens_removable: 1,
        }
    );
}

#[test]
fn same_distance_repacking_coalesces_a_short_run() {
    let source = [test_match(3, 5, 4, 0, 1), test_match(4, 5, 4, 0, 1)];
    let repacked = test_repack(&source, 7, &FIXED_LITERAL_CODE_LENGTHS, false).unwrap();
    assert_eq!(
        repacked,
        [Token::Match {
            length: 7,
            distance: 5,
            length_symbol: 261,
            distance_symbol: 4,
            length_extra: 0,
            distance_extra: 0,
            length_extra_bits: 0,
            distance_extra_bits: 1,
        }]
    );

    let canonical_258 = [test_match(100, 1, 0, 0, 0), test_match(158, 1, 0, 0, 0)];
    assert!(matches!(
        test_repack(&canonical_258, 258, &FIXED_LITERAL_CODE_LENGTHS, false,)
            .unwrap()
            .as_slice(),
        [Token::Match {
            length: 258,
            length_symbol: 285,
            length_extra: 0,
            length_extra_bits: 0,
            ..
        }]
    ));
}

#[test]
fn direct_same_distance_coalescing_does_not_enter_the_dp() {
    let source = [test_match(100, 1, 0, 0, 0), test_match(158, 1, 0, 0, 0)];
    let mut dp_must_not_poll = || -> bool { panic!("direct coalescing entered the DP") };
    let mut stop = SearchStop::callback(&mut dp_must_not_poll);
    let repacked =
        repack_same_distance_runs(&source, 258, &FIXED_LITERAL_CODE_LENGTHS, false, &mut stop)
            .unwrap();
    assert_eq!(repacked.len(), 1);
}

#[test]
fn default_repacking_bounds_pathological_dp_depth_and_max_polls_deadline() {
    let source = vec![test_match(257, 1, 0, 0, 0); 257];
    let decoded_bytes = 257 * 257;
    let mut default_must_not_poll = || -> bool { panic!("default fallback entered the DP") };
    let mut default_stop = SearchStop::callback(&mut default_must_not_poll);
    let repacked = repack_same_distance_runs(
        &source,
        decoded_bytes,
        &FIXED_LITERAL_CODE_LENGTHS,
        false,
        &mut default_stop,
    )
    .unwrap();
    assert_eq!(repacked.len(), 257);
    assert_eq!(
        repacked
            .iter()
            .map(|token| token.decoded_len())
            .sum::<usize>(),
        decoded_bytes
    );

    let mut polls = 0_usize;
    let mut expires_during_dp = || {
        polls += 1;
        polls > 3
    };
    let mut expiring_stop = SearchStop::callback(&mut expires_during_dp);
    assert!(repack_same_distance_runs(
        &source,
        decoded_bytes,
        &FIXED_LITERAL_CODE_LENGTHS,
        true,
        &mut expiring_stop,
    )
    .is_none());
    assert!(polls > 3);
}

#[test]
fn same_distance_partitioning_handles_small_remainders() {
    for total in [259_usize, 260] {
        let left = total / 2;
        let source = [
            test_match(left as u16, 1, 0, 0, 0),
            test_match((total - left) as u16, 1, 0, 0, 0),
        ];
        let repacked = test_repack(&source, total, &FIXED_LITERAL_CODE_LENGTHS, false).unwrap();
        assert_eq!(repacked.len(), 2);
        assert!(repacked
            .iter()
            .all(|token| (3..=258).contains(&token.decoded_len())));
        assert_eq!(
            repacked
                .iter()
                .map(|token| token.decoded_len())
                .sum::<usize>(),
            total
        );
    }
}

#[test]
fn same_distance_partitioning_is_legal_for_all_small_totals() {
    let partitioner = test_partitioner(&FIXED_LITERAL_CODE_LENGTHS, 16, 257).unwrap();
    for total in 6_usize..=4_096 {
        let matches = total.div_ceil(258);
        let deficit = 258 * matches - total;
        let active = matches.min(deficit);
        let active_lengths = partitioner.active_lengths(active, deficit).unwrap();
        let lengths = std::iter::repeat(258_u16)
            .take(matches - active)
            .chain(active_lengths)
            .collect::<Vec<_>>();
        assert_eq!(lengths.len(), matches, "total {total}");
        assert!(
            lengths.iter().all(|length| (3..=258).contains(length)),
            "total {total}: {lengths:?}"
        );
        assert_eq!(
            lengths.iter().copied().map(usize::from).sum::<usize>(),
            total
        );
    }
}

#[test]
fn same_distance_partitioning_uses_huffman_costs() {
    let mut prefer_258 = [15_u8; 286];
    prefer_258[284] = 15;
    prefer_258[285] = 1;
    let first = test_partitioner(&prefer_258, 2, 6)
        .unwrap()
        .active_lengths(2, 6)
        .unwrap();

    let mut avoid_258 = [15_u8; 286];
    avoid_258[284] = 1;
    avoid_258[285] = 15;
    let second = test_partitioner(&avoid_258, 2, 6)
        .unwrap()
        .active_lengths(2, 6)
        .unwrap();

    assert_eq!(first, [258, 252]);
    assert_eq!(second, [257, 253]);
}

#[test]
fn same_distance_repacking_respects_barriers_and_strict_comparison() {
    let tokens = vec![
        Token::Literal(b'a'),
        test_match(3, 1, 0, 0, 0),
        test_match(4, 2, 1, 0, 0),
        Token::Literal(b'b'),
        test_match(3, 1, 0, 0, 0),
    ];
    assert!(test_repack(&tokens, 12, &FIXED_LITERAL_CODE_LENGTHS, false).is_none());

    let already_minimal = [test_match(258, 1, 0, 0, 0); 2];
    assert!(test_repack(&already_minimal, 516, &FIXED_LITERAL_CODE_LENGTHS, false,).is_none());

    let redundant_exact_multiple = [
        test_match(100, 1, 0, 0, 0),
        test_match(158, 1, 0, 0, 0),
        test_match(258, 1, 0, 0, 0),
    ];
    assert_eq!(
        test_repack(
            &redundant_exact_multiple,
            516,
            &FIXED_LITERAL_CODE_LENGTHS,
            false,
        )
        .unwrap(),
        [test_match(258, 1, 0, 0, 0); 2]
    );

    let source = vec![
        Token::Literal(b'a'),
        test_match(3, 1, 0, 0, 0),
        test_match(4, 1, 0, 0, 0),
    ];
    let block = short_family_test_block(source.clone(), vec![b'a'; 8]);
    let incumbent = PlannedBlock {
        tokens: source.into(),
        plain: block.plain.clone(),
        representation: Representation::Fixed,
        bits: 0,
        source_type: SourceBlockType::Fixed,
    };
    let retained = improve_plan_with_same_distance_floor(&block, 0, &Options::default(), incumbent);
    assert_eq!(retained.bits, 0);
    assert_eq!(retained.tokens, block.tokens);
}

#[test]
fn large_source_bands_have_explicit_model_bounds() {
    assert!(!large_source_bands_eligible(127_999, 1));
    assert!(large_source_bands_eligible(128_000, 80_000));
    assert!(!large_source_bands_eligible(128_000, 80_001));
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

fn short_family_test_block(tokens: Vec<Token>, plain: Vec<u8>) -> ParsedBlock {
    let (literal_frequencies, distance_frequencies) = count_frequencies(&tokens);
    ParsedBlock {
        tokens: tokens.into(),
        plain: plain.into(),
        literal_frequencies,
        distance_frequencies,
        original_literal_lengths: None,
        original_distance_lengths: None,
        original_dynamic: None,
        original: None,
        source_splits: Vec::new(),
        source_type: SourceBlockType::Dynamic,
    }
}

fn mixed_short_family_block() -> ParsedBlock {
    let mut tokens = vec![Token::Literal(b'z')];
    let mut plain = vec![b'z'];
    for (length, length_symbol, distance, distance_symbol, distance_extra_bits, byte) in [
        (6, 260, 1, 0, 0, b'a'),
        (7, 261, 5, 4, 1, b'b'),
        (8, 262, 9, 6, 2, b'c'),
        (9, 263, 17, 8, 3, b'd'),
        (10, 264, 33, 10, 4, b'e'),
    ] {
        tokens.push(Token::Match {
            length,
            distance,
            length_symbol,
            distance_symbol,
            length_extra: 0,
            distance_extra: 0,
            length_extra_bits: 0,
            distance_extra_bits,
        });
        plain.extend(std::iter::repeat(byte).take(usize::from(length)));
    }
    tokens.push(Token::Literal(b'z'));
    plain.push(b'z');
    short_family_test_block(tokens, plain)
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
fn bounded_replays_return_only_strict_huffman_improvements() {
    let mut tokens = vec![Token::Literal(b'a'); 1_000];
    tokens.push(Token::Match {
        length: 3,
        distance: 1,
        length_symbol: 257,
        distance_symbol: 0,
        length_extra: 0,
        distance_extra: 0,
        length_extra_bits: 0,
        distance_extra_bits: 0,
    });
    let block = short_family_test_block(tokens, vec![b'a'; 1_003]);
    let mut selected = plan_block(&block, 0, &Options::default(), &mut SearchStop::never());
    assert!(matches!(
        selected.representation,
        Representation::Dynamic(_)
    ));

    // Inflating only the comparison price makes either valid rebuilt
    // candidate observable without fabricating an invalid Huffman tree.
    selected.bits = u64::MAX;
    let extended = replay_extended_floor(&selected, 0, &Options::default())
        .expect("the synthetic table admits an extended replay");
    let ladder = replay_table_ladder(&selected, 0, &Options::default())
        .expect("the synthetic table admits a table ladder");
    for replay in [extended, ladder] {
        assert!(replay.bits < selected.bits);
        assert!(!matches!(replay.representation, Representation::Stored));
        assert_eq!(replay.plain, selected.plain);
    }
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

    // One byte below the complete one-block model budget must reject the
    // optional expansion before allocating its four literal tokens.
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
