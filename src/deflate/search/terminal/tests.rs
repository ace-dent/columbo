// SPDX-License-Identifier: MIT
// SPDX-FileCopyrightText: 2026 Andrew C.E. Dent <https://github.com/ace-dent>

use super::*;
use crate::deflate::search::test_support::{
    aba_codes, aba_match, aba_tokens_with_dearer_matches, parsed_aba_block, short_family_test_block,
};

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
fn dearer_matches_are_found_with_the_table_replay_estimate() {
    let tokens = [Token::Literal(b'a'), Token::Literal(b'b'), aba_match()];
    let plain = b"ababa";
    let (literal, distance) = aba_codes();
    // The match costs 4 + 1 bits; its literals a, b, a cost 1 + 2 + 1.
    assert!(has_dearer_match(&tokens, plain, &literal, &distance));
    let mut dear_literals = literal.clone();
    dear_literals[usize::from(b'a')] = 2;
    assert!(!has_dearer_match(&tokens, plain, &dear_literals, &distance));
    // An absent literal is priced at 15 bits.
    let mut absent = literal;
    absent[usize::from(b'b')] = 0;
    assert!(!has_dearer_match(&tokens, plain, &absent, &distance));
}

#[test]
fn terminal_token_replay_respells_a_block_under_its_codes() {
    let block = parsed_aba_block(aba_tokens_with_dearer_matches());
    let current = block.original.unwrap().len;
    let options = Options::default();
    let plan = plan_terminal_token_replay(&block, 0, &options, &mut SearchStop::never()).unwrap();
    assert!(plan.bits < current);
    assert_eq!(&plan.plain[..], &block.plain[..]);
    assert!(plan_terminal_token_replay(&block, 0, &options, &mut SearchStop::always()).is_none());

    // Without a dearer match, a block below the full-floor size is left alone.
    let literals_only: Vec<_> = (0..600)
        .map(|i| Token::Literal(if i % 3 == 2 { b'b' } else { b'a' }))
        .collect();
    let block = parsed_aba_block(literals_only);
    assert!(plan_terminal_token_replay(&block, 0, &options, &mut SearchStop::never()).is_none());
}
