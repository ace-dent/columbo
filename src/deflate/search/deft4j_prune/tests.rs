// SPDX-License-Identifier: MIT
// SPDX-FileCopyrightText: 2026 Andrew C.E. Dent <https://github.com/ace-dent>

use super::*;
use crate::deflate::block::plan_block;
use crate::deflate::search::test_support::{
    aba_codes, aba_match, aba_tokens_with_dearer_matches, parsed_aba_block,
};

#[test]
fn least_expensive_length_symbol_totals_each_family() {
    let (literal, distance) = aba_codes();
    let abab_match = Token::Match {
        length: 4,
        distance: 2,
        length_symbol: 258,
        distance_symbol: 1,
    };
    // The length-3 match costs 4 + 1 bits against literals a, b, a at 4; the
    // length-4 match 4 + 1 against b, a, b, a at 6.
    let tokens = [
        Token::Literal(b'a'),
        Token::Literal(b'b'),
        aba_match(),
        abab_match,
    ];
    let plain = b"ababababa";
    assert_eq!(
        least_expensive_length_symbol(&tokens, plain, &literal, &distance),
        Some(257)
    );
    let mut cheap_literals = literal.clone();
    cheap_literals[usize::from(b'b')] = 1;
    // Now a, b, a cost 3 and b, a, b, a cost 4: both families save, the
    // length-3 match by 2 bits and the length-4 match by 1.
    assert_eq!(
        least_expensive_length_symbol(&tokens, plain, &cheap_literals, &distance),
        Some(257)
    );
    let literals_only = [Token::Literal(b'a'), Token::Literal(b'b')];
    assert_eq!(
        least_expensive_length_symbol(&literals_only, b"ab", &literal, &distance),
        None
    );
}

#[test]
fn length_family_chain_respells_a_block_below_its_bound() {
    let block = parsed_aba_block(aba_tokens_with_dearer_matches());
    let options = Options::default();
    let ordinary = plan_block(&block, 0, &options, &mut SearchStop::never());
    let plan =
        plan_length_family_chain(&block, 0, &options, ordinary.bits, &mut SearchStop::never())
            .unwrap();
    assert!(plan.bits < ordinary.bits);
    assert_eq!(&plan.plain[..], &block.plain[..]);
    let decoded_len: usize = plan.tokens.iter().map(|token| token.decoded_len()).sum();
    assert_eq!(decoded_len, block.plain.len());
    // Nothing is returned at or above the bound, or after a reached stop.
    assert!(
        plan_length_family_chain(&block, 0, &options, plan.bits, &mut SearchStop::never())
            .is_none()
    );
    assert!(plan_length_family_chain(
        &block,
        0,
        &options,
        ordinary.bits,
        &mut SearchStop::always()
    )
    .is_none());
}

#[test]
fn respell_from_literals_keeps_only_paying_matches_below_its_bound() {
    let block = parsed_aba_block(aba_tokens_with_dearer_matches());
    let options = Options::default();
    let ordinary = plan_block(&block, 0, &options, &mut SearchStop::never());
    let plan =
        plan_respell_from_literals(&block, 0, &options, ordinary.bits, &mut SearchStop::never())
            .unwrap();
    assert!(plan.bits < ordinary.bits);
    assert_eq!(&plan.plain[..], &block.plain[..]);
    let decoded_len: usize = plan.tokens.iter().map(|token| token.decoded_len()).sum();
    assert_eq!(decoded_len, block.plain.len());
    assert!(
        plan_respell_from_literals(&block, 0, &options, plan.bits, &mut SearchStop::never())
            .is_none()
    );
    assert!(plan_respell_from_literals(
        &block,
        0,
        &options,
        ordinary.bits,
        &mut SearchStop::always()
    )
    .is_none());
    // A block without matches has nothing to re-spell.
    let literals_only: Vec<_> = (0..600)
        .map(|i| Token::Literal(if i % 3 == 2 { b'b' } else { b'a' }))
        .collect();
    let block = parsed_aba_block(literals_only);
    assert!(
        plan_respell_from_literals(&block, 0, &options, u64::MAX, &mut SearchStop::never())
            .is_none()
    );
}
