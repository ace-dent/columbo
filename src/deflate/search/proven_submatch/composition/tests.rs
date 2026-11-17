// SPDX-License-Identifier: MIT
// SPDX-FileCopyrightText: 2026 Andrew C.E. Dent <https://github.com/ace-dent>

use super::super::ProvenSubmatchRank;
use super::*;
use crate::deflate::model::{Representation, SourceBlockType};
use crate::deflate::search::fixed_lengths;
use crate::deflate::search::test_support::{
    decode_test_tokens, short_family_test_block, test_match,
};

#[test]
fn proven_composition_frequency_state_matches_materialized_tokens() {
    let source_match = test_match(6, 6, 4);
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
    let source_match = test_match(6, 6, 4);
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
    let source_match = test_match(6, 6, 4);
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

    let source = test_match(6, 6, 4);
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
