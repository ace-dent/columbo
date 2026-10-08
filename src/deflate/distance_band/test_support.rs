// SPDX-License-Identifier: MIT
// SPDX-FileCopyrightText: 2026 Andrew C.E. Dent <https://github.com/ace-dent>

//! Distance-ladder fixtures shared by tests.

use crate::deflate::bitstream::BitWriter;
use crate::deflate::block::{emit_block, plan_block};
use crate::deflate::model::{
    canonical_length_encoding, count_frequencies, ParsedBlock, SourceBlockType, Token,
    DISTANCE_BASE, DISTANCE_EXTRA_BITS,
};
use crate::deflate::stop::SearchStop;
use crate::Options;

pub(crate) fn matched(length: u16, distance: u16) -> Token {
    let symbol = DISTANCE_BASE
        .iter()
        .rposition(|&base| base <= distance)
        .unwrap();
    let (length_symbol, length_extra, length_extra_bits) =
        canonical_length_encoding(length).unwrap();
    Token::Match {
        length,
        distance,
        length_symbol,
        length_extra,
        length_extra_bits,
        distance_symbol: symbol as u8,
        distance_extra: distance - DISTANCE_BASE[symbol],
        distance_extra_bits: DISTANCE_EXTRA_BITS[symbol],
    }
}

/// Decode `tokens` into a block, copying matches byte by byte as a decoder
/// does, so every match is valid by construction.
pub(crate) fn token_block(tokens: Vec<Token>) -> ParsedBlock {
    let mut plain = Vec::new();
    for &token in &tokens {
        match token {
            Token::Literal(value) => plain.push(value),
            Token::Match {
                length, distance, ..
            } => {
                for _ in 0..length {
                    plain.push(plain[plain.len() - usize::from(distance)]);
                }
            }
        }
    }
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

/// A period-4 block carried by distance-4 runs, with one length-3 match at a
/// multiple of four in each of distance symbols 5 through 29.
pub(crate) fn periodic_block() -> ParsedBlock {
    let mut tokens: Vec<Token> = b"abcd".iter().copied().map(Token::Literal).collect();
    let mut position = 4_usize;
    while position < 24_600 {
        tokens.push(matched(258, 4));
        position += 258;
    }
    for base in &DISTANCE_BASE[5..] {
        tokens.push(matched(3, base.next_multiple_of(4)));
        tokens.push(matched(258, 4));
    }
    token_block(tokens)
}

/// The periodic block emitted with Columbo's ordinary plan as one final
/// dynamic block.
pub(crate) fn distance_ladder_test_stream() -> Vec<u8> {
    let block = periodic_block();
    let plan = plan_block(&block, 0, &Options::default(), &mut SearchStop::never());
    let mut writer = BitWriter::default();
    emit_block(&mut writer, &[], &plan, true).unwrap();
    writer.into_bytes()
}
