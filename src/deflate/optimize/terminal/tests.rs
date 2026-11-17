// SPDX-License-Identifier: MIT
// SPDX-FileCopyrightText: 2026 Andrew C.E. Dent <https://github.com/ace-dent>

use std::time::{Duration, Instant};

use super::super::optimize_raw;
use super::super::test_support::{
    corpus_file, deadline_with_grace, png_raw_deflate, refine_with_terminal_header_search,
};
use super::*;
use crate::deflate::bitstream::BitWriter;
use crate::deflate::block::{emit_block, plan_block};
use crate::deflate::header::test_support::{
    header_tree_test_block, literal_span_test_block, payload_tradeoff_test_block,
    rotation_test_block,
};
use crate::deflate::model::Token;
use crate::deflate::parse::parse_stream;
use crate::deflate::stop::Deadline;
use crate::deflate::symbol_set::test_support::{assert_proven_rewrite, symbol_set_test_block};
use crate::progress::StreamProgress;

#[test]
fn terminal_headers_preserve_tokens_and_price_stored_alignment() {
    let mut joint_block = literal_span_test_block();
    joint_block.original_dynamic =
        Some(plan_literal_span(&joint_block, true, &mut 1024, &mut SearchStop::never()).unwrap());
    for (search, block, saving) in [
        (
            TerminalHeaderSearch::PayloadTradeoff,
            payload_tradeoff_test_block(),
            1,
        ),
        (
            TerminalHeaderSearch::LiteralSpan,
            literal_span_test_block(),
            1,
        ),
        (TerminalHeaderSearch::JointTreeRle, joint_block, 7),
        (TerminalHeaderSearch::SymbolSets, symbol_set_test_block(), 4),
    ] {
        let dynamic = block.original_dynamic.as_ref().unwrap();
        for prefix_literals in 1..=8 {
            let mut writer = BitWriter::default();
            let prefix = PlannedBlock {
                tokens: vec![Token::Literal(200); prefix_literals].into(),
                plain: vec![200; prefix_literals].into(),
                representation: Representation::Fixed,
                bits: 10 + 9 * prefix_literals as u64,
                source_type: SourceBlockType::Fixed,
            };
            emit_block(&mut writer, &[], &prefix, false).unwrap();
            let middle = PlannedBlock {
                tokens: block.tokens.clone(),
                plain: block.plain.clone(),
                representation: Representation::Dynamic(dynamic.clone()),
                bits: dynamic.bits,
                source_type: SourceBlockType::Dynamic,
            };
            emit_block(&mut writer, &[], &middle, false).unwrap();
            let stored = PlannedBlock {
                tokens: Vec::new().into(),
                plain: vec![b'X'; 9].into(),
                representation: Representation::Stored,
                bits: stored_block_bits((writer.bit_position() % 8) as u8, 9),
                source_type: SourceBlockType::Stored,
            };
            emit_block(&mut writer, &[], &stored, true).unwrap();
            let data = writer.into_bytes();
            let parsed = parse_stream(&data, 1024).unwrap();
            let identity = StreamIdentity {
                decoded_size: parsed.decoded_size,
                crc32: parsed.crc32,
                adler32: parsed.adler32,
            };
            let parent = Candidate {
                data,
                bits: parsed.meaningful_bits,
                output_max_distance: Some(parsed.max_distance),
                plans: Vec::new(),
                block_report: None,
                route: "test parent",
                max_planner_is_stable: false,
                replaced_parents: Vec::new(),
            };
            for strict in [false, true] {
                let options = Options {
                    strict,
                    ..Options::default()
                };
                let result = refine_with_terminal_header_search(
                    search,
                    &parent,
                    &options,
                    1024,
                    identity,
                    &mut SearchStop::never(),
                )
                .unwrap();
                // Stored padding can absorb a header saving. Check all
                // eight incoming alignments against byte rounding at LEN.
                let before =
                    10 + 9 * prefix_literals as u64 + parsed.blocks[1].original.unwrap().len + 3;
                let bytes_saved = before.div_ceil(8) - (before - saving).div_ceil(8);
                assert_eq!(result.is_some(), bytes_saved != 0);
                if let Some(result) = result {
                    assert_eq!((parent.data.len() - result.data.len()) as u64, bytes_saved);
                    assert_eq!(parent.bits - result.bits, 8 * bytes_saved);
                    let check = parse_validated_rewrite(&result.data, 1024, identity, &[]).unwrap();
                    assert_eq!(result.output_max_distance, Some(check.max_distance));
                    assert_eq!(check.blocks.len(), parsed.blocks.len());
                    for (a, b) in check.blocks.iter().zip(&parsed.blocks) {
                        if matches!(search, TerminalHeaderSearch::SymbolSets) {
                            assert_proven_rewrite(b, &a.tokens);
                        } else {
                            assert_eq!(a.tokens, b.tokens);
                            assert_eq!(a.source_type, b.source_type);
                        }
                        assert_eq!(a.plain, b.plain);
                        if let Some(plan) = &a.original_dynamic {
                            assert!(plan.has_strictly_compatible_huffman_codes());
                        }
                    }
                }
                assert!(refine_with_terminal_header_search(
                    search,
                    &parent,
                    &options,
                    1024,
                    identity,
                    &mut SearchStop::always(),
                )
                .unwrap()
                .is_none());
            }
        }
    }
}

#[test]
fn boundary_slide_runs_as_a_terminal_method_in_both_modes() {
    let literal_block = |bytes: &[u8]| {
        let tokens: Vec<_> = bytes.iter().copied().map(Token::Literal).collect();
        let (literal_frequencies, distance_frequencies) =
            crate::deflate::model::count_frequencies(&tokens);
        ParsedBlock {
            tokens: tokens.into(),
            plain: bytes.to_vec().into(),
            literal_frequencies,
            distance_frequencies,
            original_literal_lengths: None,
            original_distance_lengths: None,
            original_dynamic: None,
            original: None,
            source_splits: Vec::new(),
            source_type: SourceBlockType::Dynamic,
        }
    };
    let mut left = vec![b'a'; 733];
    left.extend(std::iter::repeat(b'z').take(291));
    let mut writer = BitWriter::default();
    for (index, bytes) in [left.as_slice(), &[b'z'; 1_024]].into_iter().enumerate() {
        let alignment = (writer.bit_position() % 8) as u8;
        let plan = plan_block(
            &literal_block(bytes),
            alignment,
            &Options::default(),
            &mut SearchStop::never(),
        );
        emit_block(&mut writer, &[], &plan, index == 1).unwrap();
    }
    let data = writer.into_bytes();
    let parsed = parse_stream(&data, 1 << 20).unwrap();
    let identity = StreamIdentity {
        decoded_size: parsed.decoded_size,
        crc32: parsed.crc32,
        adler32: parsed.adler32,
    };
    let parent = Candidate {
        data,
        bits: parsed.meaningful_bits,
        output_max_distance: Some(parsed.max_distance),
        plans: Vec::new(),
        block_report: None,
        route: "test parent",
        max_planner_is_stable: false,
        replaced_parents: Vec::new(),
    };
    for exhaustive in [false, true] {
        let options = Options {
            exhaustive,
            ..Options::default()
        };
        let result = refine_with_terminal_header_search(
            TerminalHeaderSearch::BoundarySlide,
            &parent,
            &options,
            1 << 20,
            identity,
            &mut SearchStop::never(),
        )
        .unwrap()
        .expect("the z run is cheaper under the right-hand tree");
        assert!(result.is_strictly_smaller_than(&parent));
        let check = parse_validated_rewrite(&result.data, 1 << 20, identity, &[]).unwrap();
        assert_eq!(check.blocks.len(), 2);
        assert_eq!(check.blocks[0].plain.as_slice(), &[b'a'; 733]);
        // The moved left block is re-planned without its z code.
        let left = check.blocks[0].original_dynamic.as_ref().unwrap();
        assert_eq!(
            left.literal_lengths
                .get(usize::from(b'z'))
                .copied()
                .unwrap_or(0),
            0
        );
        for block in &check.blocks {
            let dynamic = block.original_dynamic.as_ref().unwrap();
            assert!(dynamic.has_strictly_compatible_huffman_codes());
        }
        // The slide repeats under its re-planned trees, so sliding its result
        // again finds nothing.
        assert!(refine_with_terminal_header_search(
            TerminalHeaderSearch::BoundarySlide,
            &result,
            &options,
            1 << 20,
            identity,
            &mut SearchStop::never(),
        )
        .unwrap()
        .is_none());
    }

    // The slide is linear finalization: after the soft deadline it may still
    // start until the hard stop it polls, while no search method may start.
    let source = CandidateInput {
        compressed: &parent.data,
        blocks: &parsed.blocks,
        meaningful_bits: parsed.meaningful_bits,
        decoded_limit: 1 << 20,
        identity,
    };
    let options = Options {
        exhaustive: true,
        ..Options::default()
    };
    let finalizing =
        Deadline::with_grace(Instant::now(), Duration::ZERO, Duration::from_secs(3600));
    let stopped = Deadline::with_grace(Instant::now(), Duration::ZERO, Duration::ZERO);
    let progress = Progress::begin(
        &options,
        finalizing.started,
        StreamProgress {
            blocks: parsed.source_block_count,
            compressed_bytes: parent.data.len(),
            decoded_bytes: parsed.decoded_size,
            empty_blocks: parsed.source_empty_block_count,
            meaningful_bits: parsed.meaningful_bits,
            parse_elapsed: Duration::ZERO,
        },
    );
    for (deadline, slides) in [(&finalizing, true), (&stopped, false)] {
        let mut cache = TerminalParseCache::default();
        let slid = improve_with_terminal_header_search(
            TerminalHeaderSearch::BoundarySlide,
            source,
            &options,
            DefaultFloorWork::Timed(deadline),
            progress,
            &mut cache,
            parent.clone(),
        )
        .unwrap();
        assert_eq!(slid.is_strictly_smaller_than(&parent), slides);
        let searched = improve_with_terminal_header_search(
            TerminalHeaderSearch::PayloadTradeoff,
            source,
            &options,
            DefaultFloorWork::Timed(deadline),
            progress,
            &mut cache,
            parent.clone(),
        )
        .unwrap();
        assert_eq!(searched.data, parent.data);
    }
}

#[test]
fn max_terminal_headers_reach_small_blocks_inside_larger_streams() {
    let mut joint = literal_span_test_block();
    joint.original_dynamic =
        Some(plan_literal_span(&joint, true, &mut 1024, &mut SearchStop::never()).unwrap());
    for (search, block, saving) in [
        (
            TerminalHeaderSearch::PayloadTradeoff,
            payload_tradeoff_test_block(),
            1,
        ),
        (
            TerminalHeaderSearch::LiteralSpan,
            literal_span_test_block(),
            1,
        ),
        (TerminalHeaderSearch::JointTreeRle, joint, 7),
        (TerminalHeaderSearch::SymbolSets, symbol_set_test_block(), 4),
    ] {
        for prefix_len in [TERMINAL_HEADER_MAX_BYTES, MAX_TERMINAL_HEADER_MAX_BYTES] {
            // A large stored prefix changes the enclosing stream's size
            // class without changing the generated header problem. Stored
            // emission splits the prefix into legal wire blocks.
            let mut writer = BitWriter::default();
            let prefix = PlannedBlock {
                tokens: Vec::new().into(),
                plain: vec![b'X'; prefix_len].into(),
                representation: Representation::Stored,
                bits: stored_block_bits(0, prefix_len),
                source_type: SourceBlockType::Stored,
            };
            emit_block(&mut writer, &[], &prefix, false).unwrap();
            let dynamic = block.original_dynamic.as_ref().unwrap();
            let tail = PlannedBlock {
                tokens: block.tokens.clone(),
                plain: block.plain.clone(),
                representation: Representation::Dynamic(dynamic.clone()),
                bits: dynamic.bits,
                source_type: SourceBlockType::Dynamic,
            };
            emit_block(&mut writer, &[], &tail, true).unwrap();
            let data = writer.into_bytes();
            let limit = 2 * MAX_TERMINAL_HEADER_MAX_BYTES as u64;
            let parsed = parse_stream(&data, limit).unwrap();
            let identity = StreamIdentity {
                decoded_size: parsed.decoded_size,
                crc32: parsed.crc32,
                adler32: parsed.adler32,
            };
            let parent = Candidate {
                data,
                bits: parsed.meaningful_bits,
                output_max_distance: Some(parsed.max_distance),
                plans: Vec::new(),
                block_report: None,
                route: "generated large terminal parent",
                max_planner_is_stable: false,
                replaced_parents: Vec::new(),
            };
            for exhaustive in [false, true] {
                let options = Options {
                    exhaustive,
                    ..Options::default()
                };
                let result = refine_with_terminal_header_search(
                    search,
                    &parent,
                    &options,
                    limit,
                    identity,
                    &mut SearchStop::never(),
                )
                .unwrap();
                if !exhaustive || prefix_len == MAX_TERMINAL_HEADER_MAX_BYTES {
                    assert!(result.is_none());
                    continue;
                }
                let result = result.expect("Max must reach the admitted header problem");
                assert_eq!(parent.bits - result.bits, saving);
                let checked = parse_validated_rewrite(&result.data, limit, identity, &[]).unwrap();
                assert_eq!(checked.meaningful_bits, result.bits);
                assert_eq!(checked.blocks.len(), parsed.blocks.len());
                for (before, after) in parsed.blocks.iter().zip(&checked.blocks) {
                    assert_eq!(before.plain, after.plain);
                    if matches!(search, TerminalHeaderSearch::SymbolSets) {
                        assert_proven_rewrite(before, &after.tokens);
                    } else {
                        assert_eq!(before.tokens, after.tokens);
                    }
                }
                assert!(refine_with_terminal_header_search(
                    search,
                    &parent,
                    &options,
                    limit,
                    identity,
                    &mut SearchStop::always(),
                )
                .unwrap()
                .is_none());
            }
        }
    }
}

#[test]
fn payload_tradeoff_improves_a_recorded_completed_max_parent() {
    // A completed Max endpoint recorded for a PngSuite image. Fixing the
    // parent tests the header move without a long search or a
    // deadline-dependent result.
    let raw = [
        0x85, 0xd1, 0x0d, 0x06, 0x02, 0x30, 0x18, 0xc6, 0xf1, 0xff, 0xb6, 0x77, 0x5b, 0x67, 0xea,
        0x0c, 0xe9, 0x0c, 0xd1, 0x01, 0x02, 0x22, 0xd0, 0x0d, 0x22, 0xe8, 0x04, 0xd1, 0x2d, 0xba,
        0x56, 0xdf, 0x2d, 0x84, 0xb6, 0xbd, 0xed, 0x03, 0xe3, 0x9d, 0x9f, 0xe7, 0xd9, 0x4c, 0xb6,
        0x93, 0x38, 0x89, 0x31, 0xc6, 0x10, 0x82, 0xf7, 0xde, 0x8b, 0x77, 0xe2, 0x9c, 0x73, 0xd6,
        0x5a, 0x63, 0x8c, 0xc1, 0xc8, 0x85, 0xd6, 0xb2, 0x80, 0x49, 0x72, 0x6b, 0x82, 0xaf, 0x90,
        0x6b, 0x1b, 0x60, 0x81, 0x5e, 0x02, 0xd8, 0x11, 0xc0, 0x8e, 0x00, 0x56, 0xee, 0x7d, 0xa0,
        0x12, 0xe6, 0x9c, 0x2b, 0xa0, 0x12, 0xaa, 0x03, 0x79, 0x14, 0xe3, 0x52, 0x83, 0x3a, 0xe1,
        0x58, 0x75, 0x96, 0x09, 0x1b, 0xa8, 0x5f, 0x55, 0x55, 0xb0, 0x53, 0xe0, 0x99, 0x0d, 0x7b,
        0x1d, 0x50, 0x27, 0xac, 0xd5, 0xd7, 0xe4, 0x09, 0x27, 0xb8, 0xc1, 0x86, 0x43, 0xb3, 0x82,
        0xc5, 0x15, 0xa0, 0x48, 0x91, 0x57, 0x3e, 0xdd, 0xb2, 0xfd, 0x2f, 0x38, 0x01, 0xb0, 0x61,
        0xd7, 0xaa, 0x28, 0x93, 0xbe, 0xe0, 0xfd, 0x13, 0x53, 0x1f, 0x62, 0x88, 0x27, 0x56, 0x31,
        0xbb, 0x85, 0xbc, 0x50, 0x19, 0xe5, 0x25, 0xdf, 0x28, 0x71, 0xab, 0x41, 0x29, 0x66, 0x41,
        0x03, 0xdd, 0x92, 0x81, 0x34, 0x10, 0x92, 0xe8, 0x0b, 0x49, 0xf4, 0x85, 0x24, 0xfa, 0x42,
        0x18, 0x08, 0x61, 0x20, 0x3e,
    ];
    let parsed = parse_stream(&raw, 1 << 20).unwrap();
    let identity = StreamIdentity {
        decoded_size: parsed.decoded_size,
        crc32: parsed.crc32,
        adler32: parsed.adler32,
    };
    let parent = Candidate {
        data: raw.to_vec(),
        bits: parsed.meaningful_bits,
        output_max_distance: Some(parsed.max_distance),
        plans: Vec::new(),
        block_report: None,
        route: "completed Max parent",
        max_planner_is_stable: true,
        replaced_parents: Vec::new(),
    };
    assert_eq!(parent.bits, 1600);
    let mut cache = TerminalParseCache::default();
    let result = refine_with_terminal_header_search_cached(
        TerminalHeaderSearch::PayloadTradeoff,
        &parent,
        &Options {
            exhaustive: true,
            ..Options::default()
        },
        1 << 20,
        identity,
        &mut SearchStop::never(),
        &mut cache,
    )
    .unwrap()
    .unwrap();
    assert_eq!((result.data.len(), result.bits), (200, 1597));
    let checked = parse_validated_rewrite(&result.data, 1 << 20, identity, &[]).unwrap();
    // The cache keeps the validated parse of the smaller candidate, so the next
    // method does not reparse its bytes.
    let (bytes, kept) = cache.entry.as_ref().unwrap();
    assert_eq!(bytes, &result.data);
    assert_eq!(
        (kept.consumed, kept.meaningful_bits, kept.max_distance),
        (
            checked.consumed,
            checked.meaningful_bits,
            checked.max_distance
        )
    );
    assert!(kept
        .blocks
        .iter()
        .zip(&checked.blocks)
        .all(|(a, b)| a.tokens == b.tokens && a.original_dynamic == b.original_dynamic));
    assert_eq!(checked.blocks.len(), parsed.blocks.len());
    for (a, b) in checked.blocks.iter().zip(&parsed.blocks) {
        assert_eq!(a.tokens, b.tokens);
        assert!(a
            .original_dynamic
            .as_ref()
            .unwrap()
            .has_strictly_compatible_huffman_codes());
    }
}

#[test]
fn payload_tradeoff_leaves_discarded_empty_blocks_to_other_routes() {
    let block = payload_tradeoff_test_block();
    let dynamic = block.original_dynamic.as_ref().unwrap();
    let mut writer = BitWriter::default();
    writer.write(2, 3).unwrap();
    writer.write(0, 7).unwrap(); // Redundant empty fixed block.
    emit_block(
        &mut writer,
        &[],
        &PlannedBlock {
            tokens: block.tokens.clone(),
            plain: block.plain.clone(),
            representation: Representation::Dynamic(dynamic.clone()),
            bits: dynamic.bits,
            source_type: SourceBlockType::Dynamic,
        },
        true,
    )
    .unwrap();
    let data = writer.into_bytes();
    let parsed = parse_stream(&data, 1024).unwrap();
    assert_ne!(parsed.blocks.len(), parsed.source_block_count);
    let identity = StreamIdentity {
        decoded_size: parsed.decoded_size,
        crc32: parsed.crc32,
        adler32: parsed.adler32,
    };
    let parent = Candidate {
        data,
        bits: parsed.meaningful_bits,
        output_max_distance: Some(parsed.max_distance),
        plans: Vec::new(),
        block_report: None,
        route: "empty-block parent",
        max_planner_is_stable: false,
        replaced_parents: Vec::new(),
    };
    assert!(refine_with_terminal_header_search(
        TerminalHeaderSearch::PayloadTradeoff,
        &parent,
        &Options::default(),
        1024,
        identity,
        &mut SearchStop::never()
    )
    .unwrap()
    .is_none());
}

#[test]
#[ignore = "requires the private tests/fixtures corpus"]
fn original_match_restoration_improves_a_completed_max_fixed_point() {
    // A Max fixed point recorded from a complete Max run on the corpus image
    // loaded below. Fixing the parent tests original-match restoration
    // without a long timed search.
    const MAX_PARENT: &[u8] = &[
        0x62, 0xa8, 0x6f, 0xef, 0x9b, 0x36, 0x77, 0xc9, 0xea, 0x4d, 0x3b, 0xf6, 0x1d, 0x39, 0x79,
        0xee, 0xf2, 0x8d, 0x3b, 0x0f, 0x9f, 0xbc, 0x78, 0xfd, 0xee, 0xe3, 0x97, 0x6f, 0x3f, 0x7e,
        0xfd, 0xfe, 0xf3, 0xf7, 0xdf, 0x3f, 0x40, 0xa1, 0x74, 0xa0, 0x81, 0x40, 0x14, 0x44, 0x01,
        0xf4, 0x54, 0x49, 0x22, 0x21, 0x22, 0x84, 0x20, 0x04, 0x41, 0x20, 0x08, 0x04, 0x04, 0x02,
        0x02, 0x02, 0x02, 0x04, 0xf4, 0x29, 0xfd, 0x6e, 0xd9, 0xdd, 0x98, 0xf7, 0x0a, 0x03, 0xc0,
        0x61, 0xee, 0xdc, 0x99, 0xb7, 0x67, 0x22, 0x3c, 0x12, 0xe1, 0x9e, 0x08, 0xb7, 0x44, 0xb8,
        0x26, 0xc2, 0x25, 0x11, 0xce, 0x89, 0x70, 0x2a, 0x05, 0x50, 0x09, 0xc7, 0x42, 0x00, 0xb5,
        0x70, 0x08, 0x81, 0x66, 0x4a, 0x2d, 0xec, 0x43, 0xd0, 0xe5, 0xa0, 0xc8, 0x61, 0x17, 0x82,
        0x36, 0x29, 0x8a, 0xa4, 0xb6, 0x21, 0x68, 0x92, 0x42, 0xb1, 0x8b, 0x4d, 0x08, 0xcd, 0xe5,
        0x40, 0xb1, 0xad, 0x75, 0x08, 0x9a, 0x1c, 0x5e, 0x14, 0x7d, 0x58, 0x85, 0xe0, 0x8e, 0x66,
        0xdb, 0xa2, 0x31, 0xcb, 0x10, 0xa0, 0x99, 0xa2, 0xe8, 0xd4, 0x22, 0x04, 0x74, 0x7d, 0x14,
        0xad, 0x9b, 0x87, 0x40, 0xd5, 0x47, 0x2b, 0xcc, 0x42, 0x50, 0xf5, 0xd1, 0x09, 0xd3, 0x10,
        0x3a, 0xa1, 0xeb, 0xe3, 0x2b, 0x4c, 0x6a, 0x01, 0xca, 0xeb, 0x1b, 0xff, 0x8b, 0xea, 0x3f,
        0x8c, 0x2a, 0x71, 0x38, 0x9e, 0x7e, 0x3e, 0xc8, 0x30, 0x11, 0x06, 0x89, 0xd0, 0x4f, 0x84,
        0x5e, 0x22, 0x64, 0x42, 0x26, 0x64, 0xe2, 0x03,
    ];
    let raw = png_raw_deflate(&corpus_file("png/PngSuite/f00n0g08.png"));
    let original = parse_stream(&raw, 1 << 20).unwrap();
    let selected = parse_stream(MAX_PARENT, 1 << 20).unwrap();
    let identity = StreamIdentity {
        decoded_size: original.decoded_size,
        crc32: original.crc32,
        adler32: original.adler32,
    };
    let source = CandidateInput {
        compressed: &raw,
        blocks: &original.blocks,
        meaningful_bits: original.meaningful_bits,
        decoded_limit: 1 << 20,
        identity,
    };
    let parent = Candidate {
        data: MAX_PARENT.to_vec(),
        bits: selected.meaningful_bits,
        output_max_distance: Some(selected.max_distance),
        plans: Vec::new(),
        block_report: None,
        route: "test Max endpoint",
        max_planner_is_stable: true,
        replaced_parents: Vec::new(),
    };
    assert_eq!(parent.bits, 1740);
    let mut writer = BitWriter::default();
    writer.write(2, 3).unwrap(); // Non-final fixed block.
    writer.write(0, 7).unwrap(); // Its empty payload's EOB.
    writer.write_bits_from(MAX_PARENT, 0, parent.bits).unwrap();
    let padded_parent = Candidate {
        data: writer.into_bytes(),
        bits: parent.bits + 10,
        ..parent.clone()
    };
    // The parser removes this empty block from its model. Restoration
    // must not silently drop an unrelated header while rebuilding plans.
    assert!(refine_with_original_match_restoration(
        source,
        &padded_parent,
        &Options::default(),
        &mut SearchStop::never(),
    )
    .unwrap()
    .is_none());
    for strict in [false, true] {
        let options = Options {
            strict,
            ..Options::default()
        };
        let restored = refine_with_original_match_restoration(
            source,
            &parent,
            &options,
            &mut SearchStop::never(),
        )
        .unwrap()
        .unwrap();
        assert_eq!(restored.bits, 1737);
        assert_eq!(restored.data.len(), 218);
        let output = parse_validated_rewrite(&restored.data, 1 << 20, identity, &[]).unwrap();
        assert_eq!(restored.output_max_distance, Some(output.max_distance));
        assert!(output.max_distance <= original.max_distance);
        assert_eq!(output.blocks.len(), selected.blocks.len());
        for (a, b) in selected.blocks.iter().zip(&output.blocks) {
            assert_eq!(a.source_type, b.source_type);
            assert_eq!(a.original_dynamic, b.original_dynamic);
        }
        let selected_source = rewritten_input(&parent, &selected, 1 << 20, identity);
        assert!(refine_with_original_match_restoration(
            selected_source,
            &parent,
            &options,
            &mut SearchStop::never(),
        )
        .unwrap()
        .is_none());
        assert!(refine_with_original_match_restoration(
            source,
            &parent,
            &options,
            &mut SearchStop::always(),
        )
        .unwrap()
        .is_none());
    }
}

#[test]
fn alphabet_boundaries_preserve_history_and_reprice_later_stored_padding() {
    let mut plain: Vec<_> = (0..32).map(|i| (i % 2) as u8).collect();
    plain.extend((0..256).map(|i| 160 + (i % 16) as u8));
    plain.extend((0..35).map(|i| (i % 2) as u8));
    let tokens: Vec<_> = plain.iter().copied().map(Token::Literal).collect();
    let (literal_frequencies, distance_frequencies) =
        crate::deflate::model::count_frequencies(&tokens);
    let block = ParsedBlock {
        tokens: tokens.into(),
        plain: plain.clone().into(),
        literal_frequencies,
        distance_frequencies,
        original_literal_lengths: None,
        original_distance_lengths: None,
        original_dynamic: None,
        original: None,
        source_splits: Vec::new(),
        source_type: SourceBlockType::Dynamic,
    };
    let middle =
        crate::deflate::block::plan_block(&block, 0, &Options::default(), &mut SearchStop::never());
    // A trailing match refers back across the source block boundary. New
    // boundaries must leave that history intact.
    let tail = PlannedBlock {
        tokens: vec![Token::Match {
            length: 3,
            distance: 1,
            length_symbol: 257,
            distance_symbol: 0,
        }]
        .into(),
        plain: vec![0; 3].into(),
        representation: Representation::Fixed,
        bits: 22,
        source_type: SourceBlockType::Fixed,
    };
    for prefix_literals in 1..=8 {
        let prefix = PlannedBlock {
            tokens: vec![Token::Literal(200); prefix_literals].into(),
            plain: vec![200; prefix_literals].into(),
            representation: Representation::Fixed,
            bits: 10 + 9 * prefix_literals as u64,
            source_type: SourceBlockType::Fixed,
        };
        let mut writer = BitWriter::default();
        for plan in [&prefix, &middle, &tail] {
            emit_block(&mut writer, &[], plan, false).unwrap();
        }
        let stored = PlannedBlock {
            tokens: Vec::new().into(),
            plain: vec![b'X'; 9].into(),
            representation: Representation::Stored,
            bits: stored_block_bits((writer.bit_position() & 7) as u8, 9),
            source_type: SourceBlockType::Stored,
        };
        emit_block(&mut writer, &[], &stored, true).unwrap();
        let data = writer.into_bytes();
        let parsed = parse_stream(&data, 1024).unwrap();
        let identity = StreamIdentity {
            decoded_size: parsed.decoded_size,
            crc32: parsed.crc32,
            adler32: parsed.adler32,
        };
        let parent = Candidate {
            data,
            bits: parsed.meaningful_bits,
            output_max_distance: Some(parsed.max_distance),
            plans: Vec::new(),
            block_report: None,
            route: "test parent",
            max_planner_is_stable: false,
            replaced_parents: Vec::new(),
        };
        for strict in [false, true] {
            let result = refine_with_terminal_header_search(
                TerminalHeaderSearch::AlphabetBoundaries,
                &parent,
                &Options {
                    strict,
                    ..Options::default()
                },
                1024,
                identity,
                &mut SearchStop::never(),
            )
            .unwrap()
            .unwrap();
            assert!(result.data.len() < parent.data.len());
            let check = parse_validated_rewrite(&result.data, 1024, identity, &[]).unwrap();
            assert!(check.blocks.len() > parsed.blocks.len());
            assert_eq!(
                check.blocks.last().unwrap().source_type,
                SourceBlockType::Stored
            );
            assert_eq!(check.max_distance, 1);
            assert_eq!(result.output_max_distance, Some(1));
            assert_eq!(
                check
                    .blocks
                    .iter()
                    .flat_map(|b| b.tokens.iter())
                    .copied()
                    .collect::<Vec<_>>(),
                parsed
                    .blocks
                    .iter()
                    .flat_map(|b| b.tokens.iter())
                    .copied()
                    .collect::<Vec<_>>()
            );
            assert_eq!(check.meaningful_bits & 7, 0);
        }
        let ordinary = optimize_raw(&parent.data, &Options::default()).unwrap();
        let zero_max = optimize_raw(
            &parent.data,
            &Options {
                exhaustive: true,
                timeout: Duration::ZERO,
                ..Options::default()
            },
        )
        .unwrap();
        assert_eq!(
            zero_max.data, ordinary.data,
            "the alphabet sibling must not become mandatory Default-floor work"
        );
    }
}

#[test]
fn header_tree_search_preserves_payload_and_stored_alignment() {
    let block = header_tree_test_block();
    let middle = PlannedBlock {
        tokens: block.tokens.clone(),
        plain: block.plain.clone(),
        representation: Representation::Dynamic(block.original_dynamic.clone().unwrap()),
        bits: block.original.unwrap().len,
        source_type: SourceBlockType::Dynamic,
    };
    let tail = PlannedBlock {
        tokens: vec![Token::Match {
            length: 3,
            distance: 1,
            length_symbol: 257,
            distance_symbol: 0,
        }]
        .into(),
        plain: vec![*block.plain.last().unwrap(); 3].into(),
        representation: Representation::Fixed,
        bits: 22,
        source_type: SourceBlockType::Fixed,
    };
    let mut gains = 0;
    for count in 1..=8 {
        let prefix = PlannedBlock {
            tokens: vec![Token::Literal(200); count].into(),
            plain: vec![200; count].into(),
            representation: Representation::Fixed,
            bits: 10 + 9 * count as u64,
            source_type: SourceBlockType::Fixed,
        };
        let mut writer = BitWriter::default();
        for plan in [&prefix, &middle, &tail] {
            emit_block(&mut writer, &[], plan, false).unwrap();
        }
        let stored = PlannedBlock {
            tokens: Vec::new().into(),
            plain: vec![b'X'; 9].into(),
            representation: Representation::Stored,
            bits: stored_block_bits((writer.bit_position() & 7) as u8, 9),
            source_type: SourceBlockType::Stored,
        };
        emit_block(&mut writer, &[], &stored, true).unwrap();
        let data = writer.into_bytes();
        let parsed = parse_stream(&data, 4096).unwrap();
        let identity = StreamIdentity {
            decoded_size: parsed.decoded_size,
            crc32: parsed.crc32,
            adler32: parsed.adler32,
        };
        let parent = Candidate {
            data,
            bits: parsed.meaningful_bits,
            output_max_distance: Some(parsed.max_distance),
            plans: Vec::new(),
            block_report: None,
            route: "test parent",
            max_planner_is_stable: false,
            replaced_parents: Vec::new(),
        };
        for strict in [false, true] {
            let result = refine_with_terminal_header_search(
                TerminalHeaderSearch::HeaderTree,
                &parent,
                &Options {
                    strict,
                    ..Options::default()
                },
                4096,
                identity,
                &mut SearchStop::never(),
            )
            .unwrap();
            let selected = result.as_ref().unwrap_or(&parent);
            gains += usize::from(result.is_some());
            let check = parse_validated_rewrite(&selected.data, 4096, identity, &[]).unwrap();
            assert_eq!(check.blocks.len(), parsed.blocks.len());
            assert_eq!(check.max_distance, parsed.max_distance);
            assert_eq!(selected.output_max_distance, Some(parsed.max_distance));
            for (before, after) in parsed.blocks.iter().zip(&check.blocks) {
                assert_eq!(before.tokens, after.tokens);
                assert_eq!(before.plain, after.plain);
                if let Some(tree) = &before.original_dynamic {
                    let next = after.original_dynamic.as_ref().unwrap();
                    assert_eq!(tree.literal_lengths, next.literal_lengths);
                    assert_eq!(tree.distance_lengths, next.distance_lengths);
                }
            }
            assert_eq!(check.meaningful_bits & 7, 0);
        }
    }
    assert!(
        gains > 0 && gains < 16,
        "stored padding must absorb only some header wins"
    );
}

#[test]
fn code_length_rotations_preserve_tokens_and_stored_alignment() {
    let block = rotation_test_block(&[1, 1]);
    let middle = PlannedBlock {
        tokens: block.tokens.clone(),
        plain: block.plain.clone(),
        representation: Representation::Dynamic(block.original_dynamic.clone().unwrap()),
        bits: block.original.unwrap().len,
        source_type: SourceBlockType::Dynamic,
    };
    let tail = PlannedBlock {
        tokens: vec![Token::Match {
            length: 3,
            distance: 1,
            length_symbol: 257,
            distance_symbol: 0,
        }]
        .into(),
        plain: vec![*block.plain.last().unwrap(); 3].into(),
        representation: Representation::Fixed,
        bits: 22,
        source_type: SourceBlockType::Fixed,
    };
    let mut gains = 0;
    for count in 1..=8 {
        let prefix = PlannedBlock {
            tokens: vec![Token::Literal(200); count].into(),
            plain: vec![200; count].into(),
            representation: Representation::Fixed,
            bits: 10 + 9 * count as u64,
            source_type: SourceBlockType::Fixed,
        };
        let mut writer = BitWriter::default();
        for plan in [&prefix, &middle, &tail] {
            emit_block(&mut writer, &[], plan, false).unwrap();
        }
        let stored = PlannedBlock {
            tokens: Vec::new().into(),
            plain: vec![b'X'; 9].into(),
            representation: Representation::Stored,
            bits: stored_block_bits((writer.bit_position() & 7) as u8, 9),
            source_type: SourceBlockType::Stored,
        };
        emit_block(&mut writer, &[], &stored, true).unwrap();
        let data = writer.into_bytes();
        let parsed = parse_stream(&data, 4096).unwrap();
        let identity = StreamIdentity {
            decoded_size: parsed.decoded_size,
            crc32: parsed.crc32,
            adler32: parsed.adler32,
        };
        let parent = Candidate {
            data,
            bits: parsed.meaningful_bits,
            output_max_distance: Some(parsed.max_distance),
            plans: Vec::new(),
            block_report: None,
            route: "test parent",
            max_planner_is_stable: false,
            replaced_parents: Vec::new(),
        };
        for strict in [false, true] {
            let result = refine_with_terminal_header_search(
                TerminalHeaderSearch::CodeLengthRotations,
                &parent,
                &Options {
                    strict,
                    ..Options::default()
                },
                4096,
                identity,
                &mut SearchStop::never(),
            )
            .unwrap();
            let selected = result.as_ref().unwrap_or(&parent);
            gains += usize::from(result.is_some());
            let check = parse_validated_rewrite(&selected.data, 4096, identity, &[]).unwrap();
            assert_eq!(check.blocks.len(), parsed.blocks.len());
            assert_eq!(check.max_distance, parsed.max_distance);
            assert_eq!(selected.output_max_distance, Some(parsed.max_distance));
            for (before, after) in parsed.blocks.iter().zip(&check.blocks) {
                assert_eq!(before.tokens, after.tokens);
                assert_eq!(before.plain, after.plain);
                if let Some(tree) = &before.original_dynamic {
                    let next = after.original_dynamic.as_ref().unwrap();
                    let mut before_lengths = tree.literal_lengths.clone();
                    let mut after_lengths = next.literal_lengths.clone();
                    before_lengths.sort_unstable();
                    after_lengths.sort_unstable();
                    assert_eq!(before_lengths, after_lengths);
                    assert_eq!(tree.distance_lengths, next.distance_lengths);
                    assert_eq!((tree.hlit, tree.hdist), (next.hlit, next.hdist));
                }
            }
            assert_eq!(check.meaningful_bits & 7, 0);
        }
    }
    assert!(
        gains > 0 && gains < 16,
        "stored padding must absorb only some header wins"
    );
}

#[test]
fn coupled_swaps_preserve_tokens_histograms_and_stored_alignment() {
    let raw = crate::deflate::header::test_support::coupled_swap_test_stream();
    let block = parse_stream(&raw, 16384).unwrap().blocks.remove(1);
    let history = PlannedBlock {
        tokens: vec![Token::Literal(0); 4096].into(),
        plain: vec![0; 4096].into(),
        representation: Representation::Fixed,
        bits: 32778,
        source_type: SourceBlockType::Fixed,
    };
    let middle = PlannedBlock {
        tokens: block.tokens.clone(),
        plain: block.plain.clone(),
        representation: Representation::Dynamic(block.original_dynamic.clone().unwrap()),
        bits: block.original.unwrap().len,
        source_type: SourceBlockType::Dynamic,
    };
    let tail = PlannedBlock {
        tokens: vec![Token::Match {
            length: 3,
            distance: 1,
            length_symbol: 257,
            distance_symbol: 0,
        }]
        .into(),
        plain: vec![*block.plain.last().unwrap(); 3].into(),
        representation: Representation::Fixed,
        bits: 22,
        source_type: SourceBlockType::Fixed,
    };
    let mut gains = 0;
    for count in 1..=8 {
        let prefix = PlannedBlock {
            tokens: vec![Token::Literal(200); count].into(),
            plain: vec![200; count].into(),
            representation: Representation::Fixed,
            bits: 10 + 9 * count as u64,
            source_type: SourceBlockType::Fixed,
        };
        let mut writer = BitWriter::default();
        for plan in [&prefix, &history, &middle, &tail] {
            emit_block(&mut writer, &[], plan, false).unwrap();
        }
        let stored = PlannedBlock {
            tokens: Vec::new().into(),
            plain: vec![b'X'; 9].into(),
            representation: Representation::Stored,
            bits: stored_block_bits((writer.bit_position() & 7) as u8, 9),
            source_type: SourceBlockType::Stored,
        };
        emit_block(&mut writer, &[], &stored, true).unwrap();
        let data = writer.into_bytes();
        let parsed = parse_stream(&data, 16384).unwrap();
        let identity = StreamIdentity {
            decoded_size: parsed.decoded_size,
            crc32: parsed.crc32,
            adler32: parsed.adler32,
        };
        let parent = Candidate {
            data,
            bits: parsed.meaningful_bits,
            output_max_distance: Some(parsed.max_distance),
            plans: Vec::new(),
            block_report: None,
            route: "test parent",
            max_planner_is_stable: false,
            replaced_parents: Vec::new(),
        };
        for strict in [false, true] {
            let result = refine_with_terminal_header_search(
                TerminalHeaderSearch::CoupledLengthSwaps,
                &parent,
                &Options {
                    strict,
                    ..Options::default()
                },
                16384,
                identity,
                &mut SearchStop::never(),
            )
            .unwrap();
            let selected = result.as_ref().unwrap_or(&parent);
            gains += usize::from(result.is_some());
            let check = parse_validated_rewrite(&selected.data, 16384, identity, &[]).unwrap();
            assert_eq!(check.blocks.len(), parsed.blocks.len());
            assert_eq!(check.max_distance, parsed.max_distance);
            assert_eq!(selected.output_max_distance, Some(parsed.max_distance));
            for (before, after) in parsed.blocks.iter().zip(&check.blocks) {
                assert_eq!(before.tokens, after.tokens);
                assert_eq!(before.plain, after.plain);
                if let Some(tree) = &before.original_dynamic {
                    let next = after.original_dynamic.as_ref().unwrap();
                    let mut before_lengths = tree.literal_lengths.clone();
                    let mut after_lengths = next.literal_lengths.clone();
                    before_lengths.sort_unstable();
                    after_lengths.sort_unstable();
                    assert_eq!(before_lengths, after_lengths);
                    let mut before_distance = tree.distance_lengths.clone();
                    let mut after_distance = next.distance_lengths.clone();
                    before_distance.sort_unstable();
                    after_distance.sort_unstable();
                    assert_eq!(before_distance, after_distance);
                    assert!(tree
                        .literal_lengths
                        .iter()
                        .zip(&next.literal_lengths)
                        .all(|(&a, &b)| (a == 0) == (b == 0)));
                    assert!(tree
                        .distance_lengths
                        .iter()
                        .zip(&next.distance_lengths)
                        .all(|(&a, &b)| (a == 0) == (b == 0)));
                    assert_eq!((tree.hlit, tree.hdist), (next.hlit, next.hdist));
                }
            }
            assert_eq!(check.meaningful_bits & 7, 0);
        }
    }
    assert!(
        gains > 0 && gains < 16,
        "stored padding must absorb only some header wins"
    );
}

#[test]
fn max_terminal_sweep_keeps_what_route_gated_methods_replace() {
    // The same generated stream, whose R1–R9 sweep wins.
    let input = crate::deflate::header::test_support::coupled_swap_test_stream();
    let parsed = parse_stream(&input, 16384).unwrap();
    let identity = StreamIdentity {
        decoded_size: parsed.decoded_size,
        crc32: parsed.crc32,
        adler32: parsed.adler32,
    };
    let source = CandidateInput {
        compressed: &input,
        blocks: &parsed.blocks,
        meaningful_bits: parsed.meaningful_bits,
        decoded_limit: 16384,
        identity,
    };
    let parent = Candidate {
        data: input.clone(),
        bits: parsed.meaningful_bits,
        output_max_distance: Some(parsed.max_distance),
        plans: Vec::new(),
        block_report: None,
        route: "generated terminal parent",
        max_planner_is_stable: false,
        replaced_parents: Vec::new(),
    };
    let options = Options {
        exhaustive: true,
        timeout: Duration::MAX,
        ..Options::default()
    };
    let deadline = deadline_with_grace(Instant::now(), Duration::MAX);
    let progress = Progress::begin(
        &options,
        deadline.started,
        StreamProgress {
            blocks: parsed.source_block_count,
            compressed_bytes: input.len(),
            decoded_bytes: parsed.decoded_size,
            empty_blocks: parsed.source_empty_block_count,
            meaningful_bits: parsed.meaningful_bits,
            parse_elapsed: Duration::ZERO,
        },
    );
    let work = DefaultFloorWork::Timed(&deadline);
    let sweep = |keep_replaced| {
        improve_with_terminal_searches(
            source,
            &options,
            work,
            work,
            progress,
            HeaderFit::InSweep,
            keep_replaced,
            parent.clone(),
        )
        .unwrap()
    };
    let plain = sweep(false);
    let kept = sweep(true);
    // Keeping the replaced candidates changes no step.
    assert_eq!((&kept.data, kept.bits), (&plain.data, plain.bits));
    assert!(plain.replaced_parents.is_empty());
    assert!(!kept.replaced_parents.is_empty());
    assert_eq!(kept.replaced_parents[0].data, parent.data);
    for pair in kept.replaced_parents.windows(2) {
        assert!(pair[1].is_strictly_smaller_than(&pair[0]));
    }
    for replaced in &kept.replaced_parents {
        assert!(kept.is_strictly_smaller_than(replaced));
        assert!(replaced.plans.is_empty() && replaced.block_report.is_none());
    }
}

#[test]
fn terminal_max_closure_revisits_methods_after_a_later_tree_or_split_win() {
    // Generated frequencies and source-certified matches, with a stored
    // history prefix. One R1–R9 sweep stops at 36,729 bits: its alphabet
    // splits expose another joint-tree improvement and another useful cut.
    let input = crate::deflate::header::test_support::coupled_swap_test_stream();
    let parsed = parse_stream(&input, 16384).unwrap();
    let identity = StreamIdentity {
        decoded_size: parsed.decoded_size,
        crc32: parsed.crc32,
        adler32: parsed.adler32,
    };
    let source = CandidateInput {
        compressed: &input,
        blocks: &parsed.blocks,
        meaningful_bits: parsed.meaningful_bits,
        decoded_limit: 16384,
        identity,
    };
    let parent = Candidate {
        data: input.clone(),
        bits: parsed.meaningful_bits,
        output_max_distance: Some(parsed.max_distance),
        plans: Vec::new(),
        block_report: None,
        route: "generated terminal parent",
        max_planner_is_stable: false,
        replaced_parents: Vec::new(),
    };
    let options = Options {
        exhaustive: true,
        timeout: Duration::MAX,
        ..Options::default()
    };
    let deadline = deadline_with_grace(Instant::now(), Duration::MAX);
    let progress = Progress::begin(
        &options,
        deadline.started,
        StreamProgress {
            blocks: parsed.source_block_count,
            compressed_bytes: input.len(),
            decoded_bytes: parsed.decoded_size,
            empty_blocks: parsed.source_empty_block_count,
            meaningful_bits: parsed.meaningful_bits,
            parse_elapsed: Duration::ZERO,
        },
    );
    let closed = improve_with_terminal_searches(
        source,
        &options,
        DefaultFloorWork::Timed(&deadline),
        DefaultFloorWork::Timed(&deadline),
        progress,
        HeaderFit::InSweep,
        false,
        parent.clone(),
    )
    .unwrap();
    assert!(
        closed.bits <= 36_637,
        "closure stopped at {} bits",
        closed.bits
    );
    assert!(closed.is_strictly_smaller_than(&parent));
    let check = parse_validated_rewrite(&closed.data, 16384, identity, &[]).unwrap();
    assert_eq!(closed.bits, check.meaningful_bits);
    assert_eq!(closed.output_max_distance, Some(check.max_distance));
    assert!(check
        .blocks
        .iter()
        .filter_map(|b| b.original_dynamic.as_ref())
        .all(|p| p.has_strictly_compatible_huffman_codes()));
    let stable = improve_with_terminal_searches(
        source,
        &options,
        DefaultFloorWork::Timed(&deadline),
        DefaultFloorWork::Timed(&deadline),
        progress,
        HeaderFit::InSweep,
        false,
        closed.clone(),
    )
    .unwrap();
    assert_eq!(stable.data, closed.data);
    assert_eq!(stable.bits, closed.bits);

    // An expired optional budget must not turn the mandatory Default sweep
    // into repeated work. It retains exactly the ordinary terminal endpoint.
    let expired = deadline_with_grace(Instant::now(), Duration::ZERO);
    let ordinary_options = Options {
        exhaustive: false,
        ..options.clone()
    };
    let ordinary = improve_with_terminal_searches(
        source,
        &ordinary_options,
        DefaultFloorWork::Mandatory,
        DefaultFloorWork::Timed(&expired),
        progress,
        HeaderFit::InSweep,
        false,
        parent.clone(),
    )
    .unwrap();
    let expired_max = improve_with_terminal_searches(
        source,
        &options,
        DefaultFloorWork::Mandatory,
        DefaultFloorWork::Timed(&expired),
        progress,
        HeaderFit::InSweep,
        false,
        parent.clone(),
    )
    .unwrap();
    assert_eq!(expired_max.data, ordinary.data);
    assert_eq!(expired_max.bits, ordinary.bits);
    let stopped = improve_with_terminal_searches(
        source,
        &options,
        DefaultFloorWork::Timed(&expired),
        DefaultFloorWork::Timed(&expired),
        progress,
        HeaderFit::InSweep,
        false,
        parent.clone(),
    )
    .unwrap();
    assert_eq!(stopped.data, parent.data);
    assert_eq!(stopped.bits, parent.bits);
}

#[test]
fn tree_response_searches_keep_history_and_stored_alignment_at_every_bit_offset() {
    for (search, original) in [
        (
            TerminalHeaderSearch::HeaderResponse,
            crate::deflate::header::test_support::header_response_test_stream(),
        ),
        (
            TerminalHeaderSearch::LengthExchange,
            crate::deflate::header::test_support::length_exchange_test_stream(),
        ),
    ] {
        let mut wins = 0;
        let block = parse_stream(&original, 1024).unwrap().blocks.remove(0);
        for prefix in 1..=8 {
            let prefix = PlannedBlock {
                tokens: vec![Token::Literal(200); prefix].into(),
                plain: vec![200; prefix].into(),
                bits: 0,
                representation: Representation::Fixed,
                source_type: SourceBlockType::Fixed,
            };
            let middle = PlannedBlock {
                tokens: block.tokens.clone(),
                plain: block.plain.clone(),
                bits: block.original.unwrap().len,
                representation: Representation::Dynamic(block.original_dynamic.clone().unwrap()),
                source_type: SourceBlockType::Dynamic,
            };
            let stored = PlannedBlock {
                tokens: Vec::new().into(),
                plain: vec![b'X'; 9].into(),
                bits: 0,
                representation: Representation::Stored,
                source_type: SourceBlockType::Stored,
            };
            let mut w = BitWriter::default();
            for p in [&prefix, &middle] {
                emit_block(&mut w, &[], p, false).unwrap();
            }
            emit_block(&mut w, &[], &stored, true).unwrap();
            let data = w.into_bytes();
            let parsed = parse_stream(&data, 1024).unwrap();
            let identity = StreamIdentity {
                decoded_size: parsed.decoded_size,
                crc32: parsed.crc32,
                adler32: parsed.adler32,
            };
            let parent = Candidate {
                data,
                bits: parsed.meaningful_bits,
                output_max_distance: Some(parsed.max_distance),
                plans: Vec::new(),
                block_report: None,
                route: "response alignment parent",
                max_planner_is_stable: false,
                replaced_parents: Vec::new(),
            };
            for strict in [false, true] {
                let result = refine_with_terminal_header_search(
                    search,
                    &parent,
                    &Options {
                        strict,
                        ..Options::default()
                    },
                    1024,
                    identity,
                    &mut SearchStop::never(),
                )
                .unwrap();
                // A sub-byte improvement can disappear into the following stored
                // block's alignment. Only a whole-stream improvement is retained.
                if let Some(candidate) = result {
                    wins += 1;
                    assert!(candidate.is_strictly_smaller_than(&parent));
                    let check =
                        parse_validated_rewrite(&candidate.data, 1024, identity, &[]).unwrap();
                    assert_eq!(check.blocks.len(), 3);
                    assert_eq!(check.blocks[2].plain, stored.plain);
                    assert_proven_rewrite(&parsed.blocks[1], &check.blocks[1].tokens);
                }
                assert!(refine_with_terminal_header_search(
                    search,
                    &parent,
                    &Options {
                        strict,
                        ..Options::default()
                    },
                    1024,
                    identity,
                    &mut SearchStop::always()
                )
                .unwrap()
                .is_none());
            }
        }
        assert!(wins > 0);
    }
}

#[test]
fn terminal_parse_cache_reuses_only_identical_validated_bytes() {
    let empty = StreamIdentity {
        decoded_size: 0,
        crc32: 0,
        adler32: 1,
    };
    // Two encodings of the same empty stream share an identity, so only the
    // exact bytes can decide whether a cached model applies.
    let fixed = [0x03, 0x00];
    let stored = [0x01, 0x00, 0x00, 0xff, 0xff];
    let mut cache = TerminalParseCache::default();

    let first = &*cache.parse(&fixed, 1, empty, &[]).unwrap() as *const ParsedStream;
    let again = &*cache.parse(&fixed, 1, empty, &[]).unwrap() as *const ParsedStream;
    assert_eq!(first, again);

    assert_eq!(
        cache.parse(&stored, 1, empty, &[]).unwrap().consumed,
        stored.len()
    );
    assert_eq!(
        cache.parse(&fixed, 1, empty, &[]).unwrap().consumed,
        fixed.len()
    );

    // Invalid bytes are still rejected, and leave no stale entry behind.
    assert!(cache.parse(&[0x03, 0x00, 0x00], 1, empty, &[]).is_err());
    assert!(cache.entry.is_none());
}
