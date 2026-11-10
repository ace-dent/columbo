// SPDX-License-Identifier: MIT
// SPDX-FileCopyrightText: 2026 Andrew C.E. Dent <https://github.com/ace-dent>

//! Token and block fixtures shared by the token-spelling search tests.

use crate::deflate::header::plan_for_explicit_lengths;
use crate::deflate::model::{
    canonical_length_encoding, count_frequencies, ParsedBlock, PlannedBlock, Representation,
    SourceBlockType, Token,
};

pub(crate) fn test_match(
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

pub(crate) fn decode_test_tokens(tokens: &[Token]) -> Option<Vec<u8>> {
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

pub(crate) fn short_family_test_block(tokens: Vec<Token>, plain: Vec<u8>) -> ParsedBlock {
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

pub(crate) fn mixed_short_family_block() -> ParsedBlock {
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

/// A 3-byte match at distance 2 copying "aba".
pub(crate) fn aba_match() -> Token {
    Token::Match {
        length: 3,
        distance: 2,
        length_symbol: 257,
        distance_symbol: 1,
        length_extra: 0,
        distance_extra: 0,
        length_extra_bits: 0,
        distance_extra_bits: 0,
    }
}

/// Literal codes a = 1 bit, b = 2, end-of-block = 3; the length code 4 bits,
/// with symbol 258 completing the tree; two 1-bit distance codes.
pub(crate) fn aba_codes() -> (Vec<u8>, Vec<u8>) {
    let mut literal = vec![0_u8; 259];
    literal[usize::from(b'a')] = 1;
    literal[usize::from(b'b')] = 2;
    literal[256] = 3;
    literal[257] = 4;
    literal[258] = 4;
    (literal, vec![1, 1])
}

/// Emit `tokens` as one final dynamic block under [`aba_codes`] and parse it
/// back, so the block carries those codes as its source codes.
pub(crate) fn parsed_aba_block(tokens: Vec<Token>) -> ParsedBlock {
    let (literal, distance) = aba_codes();
    let mut plain = Vec::new();
    for token in &tokens {
        match *token {
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
    let dynamic = plan_for_explicit_lengths(&tokens, &literal, &distance, false).unwrap();
    let (literal_frequencies, distance_frequencies) = count_frequencies(&tokens);
    let block = ParsedBlock {
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
    };
    let plan = PlannedBlock {
        tokens: block.tokens.clone(),
        plain: block.plain.clone(),
        bits: dynamic.bits,
        representation: Representation::Dynamic(dynamic),
        source_type: SourceBlockType::Dynamic,
    };
    let mut writer = crate::deflate::bitstream::BitWriter::default();
    crate::deflate::block::emit_block(&mut writer, &[], &plan, true).unwrap();
    crate::deflate::parse::parse_stream(&writer.into_bytes(), 1 << 20)
        .unwrap()
        .blocks
        .remove(0)
}

/// Mostly a and b, so [`aba_codes`] fit, with matches that cost more than
/// their literals under them.
pub(crate) fn aba_tokens_with_dearer_matches() -> Vec<Token> {
    let mut tokens = Vec::new();
    for round in 0..200 {
        tokens.extend([
            Token::Literal(b'a'),
            Token::Literal(b'a'),
            Token::Literal(b'b'),
        ]);
        if round % 10 == 9 {
            tokens.push(aba_match());
        }
    }
    tokens
}
