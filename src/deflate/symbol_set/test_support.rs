// SPDX-License-Identifier: MIT

//! Symbol-set fixtures and source-proof assertions shared by tests.

use crate::deflate::model::{canonical_length_encoding, ParsedBlock, SourceBlockType, Token};
use crate::deflate::parse::parse_stream;

pub(crate) fn symbol_set_test_block() -> ParsedBlock {
    // Completed R4 g10n3p04 parent: grouped removal saves four further bits.
    let raw = [
        0xbd, 0xce, 0xb1, 0x09, 0x80, 0x00, 0x0c, 0x44, 0xd1, 0x54, 0xb1, 0x76, 0x05, 0x27, 0x10,
        0xb2, 0x82, 0x2b, 0xd8, 0xa7, 0x4a, 0x6d, 0x97, 0x15, 0x5c, 0xc1, 0x6d, 0x3d, 0x51, 0xc2,
        0x61, 0xa9, 0x90, 0x5f, 0x1e, 0x2f, 0x10, 0xd9, 0xd1, 0x8a, 0x02, 0x4d, 0x48, 0x78, 0x90,
        0xab, 0x1e, 0x31, 0xa2, 0x03, 0x95, 0xe0, 0xa1, 0x51, 0x2c, 0xc8, 0x50, 0x09, 0x1e, 0xfe,
        0x89, 0xd9, 0xc2, 0x95, 0x85, 0x58, 0xa8, 0xbf, 0x84, 0xca, 0x2d, 0xae, 0x32, 0xb7, 0x0c,
        0x79, 0x44, 0x0d, 0x38, 0x21, 0x21, 0x83, 0xba, 0x06, 0x89, 0xcc, 0xfa, 0x83, 0x87, 0x8f,
        0xe2, 0x04,
    ];
    parse_stream(&raw, 2048).unwrap().blocks.remove(0)
}

pub(crate) fn assert_proven_rewrite(source: &ParsedBlock, tokens: &[Token]) {
    if source.source_type == SourceBlockType::Stored {
        assert_eq!(tokens, source.tokens.as_slice());
        return;
    }
    if source.tokens.is_empty() {
        assert!(tokens.is_empty());
        assert!(source.plain.is_empty());
        return;
    }
    let mut source_index = 0;
    let mut source_end = source.tokens[0].decoded_len();
    let mut position = 0;
    for &token in tokens {
        while position >= source_end {
            source_index += 1;
            source_end += source.tokens[source_index].decoded_len();
        }
        let end = position + token.decoded_len();
        match token {
            Token::Literal(value) => assert_eq!(value, source.plain[position]),
            Token::Match {
                distance,
                length,
                length_symbol,
                length_extra,
                length_extra_bits,
                ..
            } => {
                assert!(end <= source_end);
                assert!(
                    matches!(source.tokens[source_index], Token::Match { distance: original, .. } if original == distance)
                );
                assert_eq!(
                    canonical_length_encoding(length),
                    Some((length_symbol, length_extra, length_extra_bits))
                );
            }
        }
        position = end;
    }
    assert_eq!(position, source.plain.len());
}
