// SPDX-License-Identifier: MIT

//! Header fixtures and cache observations shared by optimizer tests.

use super::{plan_for_explicit_lengths, HeaderPlanCache, HeaderPlanCacheStats};
use crate::deflate::huffman::make_lengths;
use crate::deflate::model::{count_frequencies, ParsedBlock, SourceBlockType, Token};
use crate::deflate::parse::parse_stream;

impl HeaderPlanCache {
    pub(super) fn with_limit(max_entries: usize) -> Self {
        Self {
            max_entries,
            ..Self::new()
        }
    }

    pub(crate) fn stats(&self) -> HeaderPlanCacheStats {
        self.stats
    }
}

pub(crate) fn literal_span_test_block() -> ParsedBlock {
    // Completed Default basi0g04 parent. Its unchanged payload codes need
    // HLIT=269, but advertising eight unused lengths saves one header bit.
    let raw = [
        0x65, 0x8e, 0x61, 0x11, 0x40, 0x00, 0x0c, 0x85, 0x1f, 0x0d, 0xd0, 0x00, 0x0d, 0xd0, 0x00,
        0x0d, 0xd0, 0x00, 0x0d, 0xd0, 0x00, 0x0d, 0xd0, 0x00, 0x0d, 0xd0, 0x80, 0x08, 0x44, 0x90,
        0x81, 0x3b, 0xbb, 0x73, 0xbb, 0xfd, 0xd8, 0xbd, 0xbd, 0x6f, 0xbb, 0xbd, 0x41, 0x0d, 0x61,
        0xe7, 0x08, 0x5b, 0xe4, 0x33, 0x8c, 0x04, 0x5e, 0x85, 0x64, 0x40, 0xb5, 0x41, 0x77, 0xe3,
        0x12, 0x6f, 0xf5, 0x78, 0x6b, 0xc5, 0x5b, 0x17, 0x14, 0x2b, 0xc8, 0xbe, 0xc1, 0xdb, 0x34,
        0xdf, 0xf4, 0x6d, 0xa6, 0x6f, 0xe5, 0x6d, 0x8e, 0x6f, 0x0f, 0x9a, 0xe9, 0xf8, 0x51, 0x5a,
        0x80, 0xb4, 0x06, 0x69, 0x07, 0xd2, 0x11, 0xa4, 0x0b, 0x48, 0x77, 0x90, 0x9e, 0x20, 0xbd,
        0xff, 0x3b, 0xf2, 0xa0, 0xbc, 0x2c, 0x23, 0x64, 0x96, 0x0c, 0x95, 0xe9, 0xf2, 0x8d, 0xff,
        0x1f, 0x68, 0x9a, 0x69, 0x3a, 0x8e, 0xef, 0x47, 0x51, 0x9a, 0x16, 0x85, 0x04, 0xdc, 0xd6,
        0xb5, 0x04, 0xdc, 0x76, 0x9d, 0x04, 0xdc, 0x8e, 0xa3, 0x04, 0xdc, 0x2e, 0x8b, 0x04, 0xdc,
        0xee, 0xbb, 0x04, 0xdc, 0x9e, 0xa7, 0x04, 0xdc, 0xde, 0xb7, 0x00, 0x0f,
    ];
    parse_stream(&raw, 1024).unwrap().blocks.remove(0)
}

pub(crate) fn payload_tradeoff_test_block() -> ParsedBlock {
    // A small literal-only witness: spending three payload bits saves four
    // header bits even after full repricing of the unchanged tree.
    let counts = [14, 6, 9, 7, 13, 8, 3, 13, 15, 5, 2, 4, 1, 3, 10, 3];
    let plain: Vec<u8> = counts
        .iter()
        .enumerate()
        .flat_map(|(symbol, &count)| std::iter::repeat(symbol as u8).take(count))
        .collect();
    let tokens: Vec<Token> = plain.iter().copied().map(Token::Literal).collect();
    let (literal_frequencies, distance_frequencies) = count_frequencies(&tokens);
    let literal_lengths = make_lengths(&literal_frequencies, 15, 0);
    let dynamic = plan_for_explicit_lengths(&tokens, &literal_lengths, &[1, 1], true).unwrap();
    ParsedBlock {
        tokens: tokens.into(),
        plain: plain.into(),
        literal_frequencies,
        distance_frequencies,
        original_literal_lengths: None,
        original_distance_lengths: None,
        original_dynamic: Some(dynamic),
        original: None,
        source_splits: Vec::new(),
        source_type: SourceBlockType::Dynamic,
    }
}

pub(crate) fn header_tree_test_block() -> ParsedBlock {
    // Completed Default g10n3p04 parent. Exact CL-tree/RLE optimization saves
    // three bits beyond full existing header repricing of these data trees.
    let raw = [
        0xbd, 0xca, 0x1d, 0x17, 0x80, 0x60, 0x10, 0x44, 0xe1, 0xa1, 0x8d, 0x33, 0x0f, 0xbc, 0xca,
        0x3c, 0xf3, 0xfc, 0xa5, 0xe1, 0x6c, 0xcd, 0x33, 0xef, 0xdf, 0x36, 0x7d, 0xef, 0x09, 0xab,
        0xba, 0x78, 0xcf, 0x83, 0x41, 0x75, 0x8a, 0xaa, 0x50, 0x88, 0x03, 0x50, 0xff, 0x88, 0x5c,
        0x8d, 0xea, 0x10, 0x71, 0xfc, 0x28, 0x5a, 0xd5, 0xa8, 0x43, 0xc4, 0xf1, 0x4c, 0x94, 0x0d,
        0x93, 0x45, 0x81, 0x86, 0x96, 0x2e, 0xc2, 0xb0, 0x0a, 0x28, 0xf7, 0xde, 0x89, 0x4d, 0x1c,
        0xc3, 0x12, 0x83, 0x40, 0x66, 0xc9, 0x18, 0x84, 0x3b, 0x49, 0x02, 0x00, 0xe2, 0xb8, 0x29,
        0x26,
    ];
    parse_stream(&raw, 4096).unwrap().blocks.remove(0)
}

/// A generated literal-only parent at a local optimum for fully repriced
/// pair swaps. Rotating lengths at positions 4, 13 and 18 saves one bit.
pub(crate) fn rotation_test_block(distance: &[u8]) -> ParsedBlock {
    use crate::deflate::bitstream::BitWriter;
    use crate::deflate::block::emit_block;
    use crate::deflate::model::{PlannedBlock, Representation};

    let counts = [
        4, 13, 12, 6, 2, 10, 9, 7, 8, 9, 20, 16, 2, 3, 19, 1, 10, 20, 6, 14, 19, 17, 18, 8,
    ];
    let plain: Vec<u8> = counts
        .iter()
        .enumerate()
        .flat_map(|(symbol, &count)| std::iter::repeat(symbol as u8).take(count))
        .collect();
    let tokens: Vec<Token> = plain.iter().copied().map(Token::Literal).collect();
    let mut lengths = [0; 257];
    lengths[..24].copy_from_slice(&[
        6, 4, 4, 5, 7, 5, 5, 5, 5, 5, 4, 4, 7, 6, 4, 7, 4, 4, 5, 4, 4, 4, 4, 5,
    ]);
    lengths[256] = 7;
    let dynamic = plan_for_explicit_lengths(&tokens, &lengths, distance, true).unwrap();
    let plan = PlannedBlock {
        tokens: tokens.into(),
        plain: plain.into(),
        bits: dynamic.bits,
        representation: Representation::Dynamic(dynamic),
        source_type: SourceBlockType::Dynamic,
    };
    let mut writer = BitWriter::default();
    emit_block(&mut writer, &[], &plan, true).unwrap();
    parse_stream(&writer.into_bytes(), 4096)
        .unwrap()
        .blocks
        .remove(0)
}
