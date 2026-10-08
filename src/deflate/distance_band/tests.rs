// SPDX-License-Identifier: MIT
// SPDX-FileCopyrightText: 2026 Andrew C.E. Dent <https://github.com/ace-dent>

use super::test_support::{distance_ladder_test_stream, matched, periodic_block, token_block};
use super::*;
use crate::deflate::bitstream::BitWriter;
use crate::deflate::block::emit_block;
use crate::deflate::model::{count_frequencies, ParsedStream};
use crate::deflate::parse::parse_stream;

/// Emit Columbo's ordinary plan and parse it, so the block carries its
/// transmitted trees and original bit range.
fn planned(source: &ParsedBlock) -> ParsedStream {
    let plan =
        super::super::block::plan_block(source, 0, &Options::default(), &mut SearchStop::never());
    let mut writer = BitWriter::default();
    emit_block(&mut writer, &[], &plan, true).unwrap();
    parse_stream(&writer.into_bytes(), 1 << 20).unwrap()
}

/// Deterministic pseudo-random block over a four-byte alphabet.
fn random_block(seed: u64) -> ParsedBlock {
    let mut state = seed;
    let mut next = |bound: usize| {
        state = state
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        (state >> 33) as usize % bound
    };
    let target = 200 + next(3_000);
    let mut tokens = Vec::new();
    let mut position = 0;
    while position < target {
        if position >= 3 && next(3) != 0 {
            let distance = if next(2) == 0 {
                1 + next(position.min(8))
            } else {
                1 + next(position)
            };
            let length = if next(16) == 0 { 258 } else { 3 + next(40) };
            tokens.push(matched(length as u16, distance as u16));
            position += length;
        } else {
            tokens.push(Token::Literal(b"abcd"[next(4)]));
            position += 1;
        }
    }
    token_block(tokens)
}

fn kept_matches_only(source: &ParsedBlock, tokens: &[Token], keep: u32) {
    let mut expected = Vec::new();
    let mut position = 0;
    for &token in source.tokens.iter() {
        let end = position + token.decoded_len();
        match token {
            Token::Match {
                distance_symbol, ..
            } if keep & (1 << distance_symbol) == 0 => expected.extend(
                source.plain[position..end]
                    .iter()
                    .copied()
                    .map(Token::Literal),
            ),
            _ => expected.push(token),
        }
        position = end;
    }
    assert_eq!(tokens, expected.as_slice());
}

#[test]
fn rungs_cover_prefix_single_and_frequency_ladders() {
    let mut tokens = vec![Token::Literal(b'a'); 16];
    tokens.extend(std::iter::repeat(matched(3, 1)).take(5));
    tokens.extend(std::iter::repeat(matched(3, 4)).take(10));
    tokens.push(matched(3, 14));
    let (masks, count) = rungs(&token_block(tokens)).unwrap();
    // Prefix {0}, {0,3}; singles {0}, {3}, {7}; frequency {3}, {3,0}.
    assert_eq!(&masks[..count], &[1, 1 << 3, 1 | 1 << 3, 1 << 7]);

    let mut tokens = vec![Token::Literal(b'a'); 4];
    tokens.extend(std::iter::repeat(matched(3, 1)).take(3));
    assert!(rungs(&token_block(tokens)).is_none());
}

#[test]
fn rung_estimates_match_materialized_tokens() {
    let mut rungs_checked = 0;
    for seed in 0..200 {
        let source = random_block(seed);
        let Some((masks, count)) = rungs(&source) else {
            continue;
        };
        let removals = Removals::collect(&source, &mut SearchStop::never()).unwrap();
        let extra_bits = token_extra_bits(&source.tokens);
        for &keep in &masks[..count] {
            let tokens =
                expand_selected_matches(&source.tokens, &source.plain, |_, token, _| match token {
                    Token::Match {
                        distance_symbol, ..
                    } => keep & (1 << distance_symbol) == 0,
                    Token::Literal(_) => false,
                })
                .unwrap();
            kept_matches_only(&source, &tokens, keep);
            let (literal, distance) = count_frequencies(&tokens);
            for strict in [false, true] {
                assert_eq!(
                    removals.estimate(&source, keep, extra_bits, strict),
                    estimate_boundary_block_bits(
                        &literal,
                        &distance,
                        token_extra_bits(&tokens),
                        strict
                    ),
                    "seed {seed}, keep {keep:#x}"
                );
            }
            rungs_checked += 1;
        }
    }
    assert!(rungs_checked > 1_000);
}

#[test]
fn collapses_far_distances_in_a_periodic_block() {
    let parsed = parse_stream(&distance_ladder_test_stream(), 1 << 20).unwrap();
    let parent = &parsed.blocks[0];
    let original = parent.original.unwrap();
    for strict in [false, true] {
        let options = Options {
            strict,
            ..Options::default()
        };
        let plan = plan_distance_bands(
            parent,
            original.alignment,
            &options,
            &mut SearchStop::never(),
        )
        .unwrap();
        assert!(plan.bits < original.len);
        // Only the distance-4 runs survive.
        kept_matches_only(parent, &plan.tokens, 1 << 3);

        let mut writer = BitWriter::default();
        emit_block(&mut writer, &[], &plan, true).unwrap();
        assert_eq!(writer.bit_position(), plan.bits);
        let output = parse_stream(&writer.into_bytes(), 1 << 20).unwrap();
        assert_eq!(output.blocks.len(), 1);
        assert_eq!(output.blocks[0].plain, parent.plain);
        if strict {
            let dynamic = output.blocks[0].original_dynamic.as_ref().unwrap();
            assert!(dynamic.has_strictly_compatible_huffman_codes());
        }
    }
}

#[test]
fn declines_stored_single_symbol_and_stopped_blocks() {
    let options = Options::default();
    let periodic = planned(&periodic_block()).blocks.remove(0);
    let alignment = periodic.original.unwrap().alignment;
    assert!(
        plan_distance_bands(&periodic, alignment, &options, &mut SearchStop::always()).is_none()
    );

    let mut stored = periodic.clone();
    stored.source_type = SourceBlockType::Stored;
    assert!(plan_distance_bands(&stored, alignment, &options, &mut SearchStop::never()).is_none());

    let mut tokens = vec![Token::Literal(b'a')];
    tokens.extend(std::iter::repeat(matched(258, 1)).take(8));
    let single = planned(&token_block(tokens)).blocks.remove(0);
    assert!(plan_distance_bands(&single, 0, &options, &mut SearchStop::never()).is_none());
}
