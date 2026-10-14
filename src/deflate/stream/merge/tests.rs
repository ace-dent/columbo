// SPDX-License-Identifier: MIT
// SPDX-FileCopyrightText: 2026 Andrew C.E. Dent <https://github.com/ace-dent>

use super::*;
use crate::deflate::bitstream::BitWriter;
use crate::deflate::block::{emit_block, plan_block};
use crate::deflate::parse::parse_stream;

/// One literal-only block per part, dynamic unless the part is marked stored,
/// emitted and parsed back so every block keeps its encoding.
fn parsed_blocks(parts: &[(&[u8], bool)], strict: bool) -> Vec<ParsedBlock> {
    let options = Options {
        strict,
        ..Options::default()
    };
    let mut writer = BitWriter::default();
    let mut bits = 0_u64;
    for (index, &(plain, stored)) in parts.iter().enumerate() {
        let tokens: Vec<_> = plain.iter().copied().map(Token::Literal).collect();
        let (literal_frequencies, distance_frequencies) = count_frequencies(&tokens);
        let block = ParsedBlock {
            tokens: tokens.into(),
            plain: plain.to_vec().into(),
            literal_frequencies,
            distance_frequencies,
            original_literal_lengths: None,
            original_distance_lengths: None,
            original_dynamic: None,
            original: None,
            source_splits: Vec::new(),
            source_type: if stored {
                SourceBlockType::Stored
            } else {
                SourceBlockType::Dynamic
            },
        };
        let alignment = (bits % 8) as u8;
        let plan = if stored {
            PlannedBlock {
                tokens: block.tokens.clone(),
                plain: block.plain.clone(),
                representation: Representation::Stored,
                bits: stored_block_bits(alignment, plain.len()),
                source_type: SourceBlockType::Stored,
            }
        } else {
            plan_block(&block, alignment, &options, &mut SearchStop::never())
        };
        bits += plan.bits;
        emit_block(&mut writer, &[], &plan, index + 1 == parts.len()).unwrap();
    }
    parse_stream(&writer.into_bytes(), 1 << 20).unwrap().blocks
}

fn regime(range: std::ops::Range<u8>, count: usize) -> Vec<u8> {
    range.cycle().take(count).collect()
}

fn encoded_bits(blocks: &[ParsedBlock], strict: bool) -> u64 {
    let mut bits = 0;
    for block in blocks {
        let alignment = (bits % 8) as u8;
        bits += encoded_price(block, alignment, strict).unwrap().1;
    }
    bits
}

fn decoded(plans: &[PlannedBlock]) -> Vec<u8> {
    plans
        .iter()
        .flat_map(|plan| plan.plain.iter().copied())
        .collect()
}

#[test]
fn merges_neighbors_that_share_their_statistics() {
    let part = regime(0..8, 400);
    for strict in [false, true] {
        let blocks = parsed_blocks(&[(&part, false), (&part, false)], strict);
        assert_eq!(blocks.len(), 2);
        let options = Options {
            strict,
            ..Options::default()
        };
        let plans = plan_terminal_merges(&blocks, &options, &mut SearchStop::never()).unwrap();
        assert_eq!(plans.len(), 1);
        assert!(total_bits(&plans) < encoded_bits(&blocks, strict));
        assert_eq!(decoded(&plans), [part.clone(), part.clone()].concat());
        if let Representation::Dynamic(dynamic) = &plans[0].representation {
            assert!(!strict || dynamic.has_strictly_compatible_huffman_codes());
        }
        assert!(plan_terminal_merges(&blocks, &options, &mut SearchStop::always()).is_none());
    }
}

#[test]
fn keeps_neighbors_whose_statistics_differ() {
    let left = regime(0..2, 400);
    let right = regime(160..176, 400);
    let blocks = parsed_blocks(&[(&left, false), (&right, false)], true);
    assert!(plan_terminal_merges(&blocks, &Options::default(), &mut SearchStop::never()).is_none());
}

#[test]
fn merges_chain_and_stop_at_stored_blocks() {
    let part = regime(0..8, 300);
    let stored = regime(0..255, 64);
    let blocks = parsed_blocks(
        &[
            (&part, false),
            (&part, false),
            (&part, false),
            (&stored, true),
            (&part, false),
            (&part, false),
        ],
        true,
    );
    assert_eq!(blocks.len(), 6);
    let plans =
        plan_terminal_merges(&blocks, &Options::default(), &mut SearchStop::never()).unwrap();
    let lengths: Vec<_> = plans.iter().map(|plan| plan.plain.len()).collect();
    assert_eq!(lengths, [900, 64, 600]);
    assert!(matches!(plans[1].representation, Representation::Stored));
    assert!(total_bits(&plans) < encoded_bits(&blocks, true));
    let expected: Vec<u8> = blocks
        .iter()
        .flat_map(|block| block.plain.iter().copied())
        .collect();
    assert_eq!(decoded(&plans), expected);
}

#[test]
fn completed_merges_survive_an_exhausted_budget() {
    let part = regime(0..8, 300);
    let blocks = parsed_blocks(&[(&part, false), (&part, false), (&part, false)], true);
    let pair_work = 2 * (300 * std::mem::size_of::<Token>() + 300);
    let plans = plan_terminal_merges_with_work(
        &blocks,
        &Options::default(),
        pair_work,
        &mut SearchStop::never(),
    )
    .unwrap();
    let lengths: Vec<_> = plans.iter().map(|plan| plan.plain.len()).collect();
    assert_eq!(lengths, [600, 300]);
    assert!(plan_terminal_merges_with_work(
        &blocks,
        &Options::default(),
        pair_work - 1,
        &mut SearchStop::never(),
    )
    .is_none());
}
