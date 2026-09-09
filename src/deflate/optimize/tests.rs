// SPDX-License-Identifier: MIT

use std::sync::Arc;
use std::time::Duration;

use super::super::stop::{initial_bounded_phase_share, TIMEOUT_GRACE_BASE, TIMEOUT_GRACE_DIVISOR};
use super::*;
use crate::deflate::bitstream::BitWriter;
use crate::deflate::huffman::{fixed_trees, huffman_tree_shape_is_complete};
use crate::deflate::model::Token;

fn compact_source_split_floor_eligible(decoded_size: u64, blocks: &[ParsedBlock]) -> bool {
    compact_source_split_floor_eligible_with_limits(
        decoded_size,
        blocks,
        2,
        COMPACT_SPLIT_FLOOR_MAX_DECODED,
    )
}

#[test]
fn terminal_headers_preserve_tokens_and_price_stored_alignment() {
    let mut joint_block = super::super::header::literal_span_test_block();
    joint_block.original_dynamic =
        Some(plan_literal_span(&joint_block, true, &mut 1024, &mut SearchStop::never()).unwrap());
    for (search, block, saving) in [
        (
            TerminalHeaderSearch::PayloadTradeoff,
            super::super::header::payload_tradeoff_test_block(),
            1,
        ),
        (
            TerminalHeaderSearch::LiteralSpan,
            super::super::header::literal_span_test_block(),
            1,
        ),
        (TerminalHeaderSearch::JointTreeRle, joint_block, 7),
        (
            TerminalHeaderSearch::SymbolSets,
            super::super::symbol_set::symbol_set_test_block(),
            4,
        ),
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
                    let check = parse_validated_rewrite(&result.data, 1024, identity).unwrap();
                    assert_eq!(result.output_max_distance, Some(check.max_distance));
                    assert_eq!(check.blocks.len(), parsed.blocks.len());
                    for (a, b) in check.blocks.iter().zip(&parsed.blocks) {
                        if matches!(search, TerminalHeaderSearch::SymbolSets) {
                            super::super::symbol_set::assert_proven_rewrite(b, &a.tokens);
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
#[ignore = "requires the private tests/fixtures corpus"]
fn joint_tree_rle_reaches_the_final_stream_and_png_header() {
    let source = &corpus_file("png/PngSuite/basi0g04.png");
    let result = crate::optimize(source, crate::Format::Png, &Options::default()).unwrap();
    let raw = png_raw_deflate(&result.data);
    let parsed = parse_stream(&raw, 1024).unwrap();
    assert_eq!(parsed.meaningful_bits, 1286);
    assert_eq!(
        parsed.blocks[0].original_dynamic.as_ref().unwrap().hlit,
        277
    );
}

#[test]
fn payload_tradeoff_improves_a_recorded_completed_max_parent() {
    // f04n0g08's completed Max endpoint. Freezing the parent isolates the
    // new header move without a long search or deadline-dependent result.
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
    };
    assert_eq!(parent.bits, 1600);
    let result = refine_with_terminal_header_search(
        TerminalHeaderSearch::PayloadTradeoff,
        &parent,
        &Options {
            exhaustive: true,
            ..Options::default()
        },
        1 << 20,
        identity,
        &mut SearchStop::never(),
    )
    .unwrap()
    .unwrap();
    assert_eq!((result.data.len(), result.bits), (200, 1597));
    let checked = parse_validated_rewrite(&result.data, 1 << 20, identity).unwrap();
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
    let block = super::super::header::payload_tradeoff_test_block();
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
    // Recorded Max endpoint of f00n0g08: another complete Max invocation
    // retained these exact 1,740 bits. Keep the parent fixed to isolate
    // original-proof restoration without running a long timed search.
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
        let output = parse_validated_rewrite(&restored.data, 1 << 20, identity).unwrap();
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

const FEEDBACK_RAW: &[u8] = &[
    0x25, 0xc0, 0x01, 0x01, 0xc0, 0x30, 0x0c, 0xc3, 0x30, 0x6c, 0xb5, 0x9b, 0xf0, 0x87, 0xf4, 0x7d,
    0xd3, 0xcc, 0xcc, 0xcc, 0xcc, 0x01, 0x00, 0x00, 0xc0, 0x71, 0x5d, 0xaa, 0xaa, 0xaa, 0xfe, 0x76,
    0x77, 0x93, 0x24, 0x49, 0x9e, 0xa7, 0x6d, 0xdb, 0xf6, 0x03,
];
fn corpus_file(relative: &str) -> Vec<u8> {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(relative);
    std::fs::read(path).unwrap_or_else(|_| panic!("missing local corpus fixture: {relative}"))
}

fn png_raw_deflate(input: &[u8]) -> Vec<u8> {
    assert_eq!(&input[..8], b"\x89PNG\r\n\x1a\n");
    let mut offset = 8;
    let mut zlib = Vec::new();
    while offset + 12 <= input.len() {
        let length = u32::from_be_bytes(input[offset..offset + 4].try_into().unwrap()) as usize;
        let kind = &input[offset + 4..offset + 8];
        let data_start = offset + 8;
        let data_end = data_start + length;
        assert!(data_end + 4 <= input.len());
        if kind == b"IDAT" {
            zlib.extend_from_slice(&input[data_start..data_end]);
        }
        offset = data_end + 4;
        if kind == b"IEND" {
            break;
        }
    }
    assert!(zlib.len() >= 6);
    zlib[2..zlib.len() - 4].to_vec()
}

fn comparison_candidate(bytes: usize, bits: u64, marker: u8) -> Candidate {
    Candidate {
        data: vec![marker; bytes],
        bits,
        output_max_distance: None,
        plans: Vec::new(),
        block_report: None,
        route: "test",
        max_planner_is_stable: false,
    }
}

#[test]
fn candidate_replacement_is_byte_first_strict_and_stable_on_ties() {
    let mut incumbent = comparison_candidate(2, 8, 1);

    assert!(incumbent.replace_if_smaller(comparison_candidate(2, 7, 2)));
    assert_eq!(incumbent.data, vec![2; 2]);
    assert_eq!(incumbent.bits, 7);

    // An exact size tie retains the earlier route's byte spelling.
    assert!(!incumbent.replace_if_smaller(comparison_candidate(2, 7, 3)));
    assert_eq!(incumbent.data, vec![2; 2]);

    assert!(!incumbent.replace_if_smaller(comparison_candidate(2, 8, 4)));
    // Byte count is authoritative; bits only break equal-byte ties.
    assert!(incumbent.replace_if_smaller(comparison_candidate(1, 8, 5)));
    assert_eq!(incumbent.data, vec![5]);

    let mut optional = None;
    assert!(replace_optional_if_smaller(
        &mut optional,
        comparison_candidate(1, 8, 6)
    ));
    assert!(!replace_optional_if_smaller(
        &mut optional,
        comparison_candidate(1, 8, 7)
    ));
    assert_eq!(optional.as_ref().unwrap().data, vec![6]);
    assert!(replace_optional_if_smaller(
        &mut optional,
        comparison_candidate(1, 7, 8)
    ));
    assert_eq!(optional.unwrap().data, vec![8]);

    let incumbent = comparison_candidate(2, 7, 9);
    let mut stable = comparison_candidate(2, 7, 9);
    stable.max_planner_is_stable = true;
    assert!(incumbent.is_encoding_stabilized_by(&stable));
    stable.data[0] = 10;
    assert!(!incumbent.is_encoding_stabilized_by(&stable));

    let source_bytes = [0_u8; 2];
    let source = CandidateInput {
        compressed: &source_bytes,
        blocks: &[],
        meaningful_bits: 7,
        decoded_limit: 0,
        identity: StreamIdentity {
            decoded_size: 0,
            crc32: 0,
            adler32: 0,
        },
    };
    assert!(comparison_candidate(1, 8, 9).is_strictly_smaller_than_source(source));
    assert!(!comparison_candidate(2, 7, 10).is_strictly_smaller_than_source(source));
}

#[test]
#[ignore = "requires the private tests/fixtures corpus"]
fn smoothed_tree_floor_rebuilds_multiple_huffman_blocks_exactly() {
    let raw = png_raw_deflate(&corpus_file("png/PngSuite/tbbn2c16.png"));
    let parsed = parse_stream(&raw, 1 << 20).unwrap();
    let [block] = parsed.blocks.as_slice() else {
        panic!("fixture must contain one source block");
    };
    assert_eq!(block.source_type, SourceBlockType::Dynamic);
    let original = block.original.unwrap();
    let plan = PlannedBlock {
        tokens: block.tokens.clone(),
        plain: block.plain.clone(),
        bits: original.len,
        representation: Representation::Original(original),
        source_type: block.source_type,
    };
    let (data, bits) = emit_plans(&raw, &[plan.clone(), plan.clone()], true).unwrap();
    let combined = parse_stream(&data, 1 << 20).unwrap();
    assert_eq!(combined.blocks.len(), 2);
    let identity = StreamIdentity {
        decoded_size: combined.decoded_size,
        crc32: combined.crc32,
        adler32: combined.adler32,
    };
    let candidate = Candidate {
        data,
        bits,
        output_max_distance: Some(combined.max_distance),
        plans: Vec::new(),
        block_report: None,
        route: "test",
        max_planner_is_stable: false,
    };

    let refined =
        refine_with_compact_payload_tree_floor(&candidate, &Options::default(), 1 << 20, identity)
            .unwrap()
            .1
            .unwrap();

    assert!(refined.bits < candidate.bits);
    let reparsed = parse_validated_rewrite(&refined.data, 1 << 20, identity).unwrap();
    assert_eq!(reparsed.blocks.len(), 2);
    assert!(reparsed
        .blocks
        .iter()
        .all(|block| block.source_type == SourceBlockType::Dynamic));

    let (data, bits) = emit_plans(&raw, &vec![plan; 9], true).unwrap();
    let combined = parse_stream(&data, 1 << 20).unwrap();
    let identity = StreamIdentity {
        decoded_size: combined.decoded_size,
        crc32: combined.crc32,
        adler32: combined.adler32,
    };
    let candidate = Candidate {
        data,
        bits,
        output_max_distance: Some(combined.max_distance),
        plans: Vec::new(),
        block_report: None,
        route: "test",
        max_planner_is_stable: false,
    };
    let (covered, compact) =
        refine_with_compact_payload_tree_floor(&candidate, &Options::default(), 1 << 20, identity)
            .unwrap();
    assert!(!covered);
    assert!(compact.is_none());
    refine_with_bounded_depth_tree_floor(
        &candidate,
        &Options::default(),
        1 << 20,
        identity,
        &mut SearchStop::never(),
    )
    .unwrap();
}

#[test]
fn changed_parent_no_split_requires_a_new_best_topology() {
    let parent = comparison_candidate(12, 90, 1);
    let no_split = comparison_candidate(10, 80, 2);
    let refined = comparison_candidate(9, 79, 3);

    assert!(changed_parent_no_split_should_continue(
        true,
        &refined,
        &parent,
        Some(&no_split),
    ));
    assert!(!changed_parent_no_split_should_continue(
        false,
        &refined,
        &parent,
        Some(&no_split),
    ));
    assert!(!changed_parent_no_split_should_continue(
        true,
        &no_split,
        &parent,
        Some(&refined),
    ));
    assert!(!changed_parent_no_split_should_continue(
        true, &refined, &parent, None,
    ));
}

#[test]
fn changed_narrow_parent_continues_after_a_source_improvement() {
    let compressed = [0_u8; 12];
    let source = CandidateInput {
        compressed: &compressed,
        blocks: &[],
        meaningful_bits: 90,
        decoded_limit: 0,
        identity: StreamIdentity {
            decoded_size: 0,
            crc32: 0,
            adler32: 0,
        },
    };
    let narrow = comparison_candidate(10, 80, 1);

    assert!(changed_narrow_parent_should_continue(true, &narrow, source));
    assert!(!changed_narrow_parent_should_continue(
        false, &narrow, source
    ));

    let unchanged = comparison_candidate(12, 90, 5);
    assert!(!changed_narrow_parent_should_continue(
        true, &unchanged, source
    ));
}

#[test]
fn no_split_policy_prioritizes_cumulative_work_for_long_lists() {
    assert!(!cumulative_no_split_count_has_priority(0));
    assert!(!cumulative_no_split_count_has_priority(3));
    assert!(cumulative_no_split_count_has_priority(4));
    assert!(cumulative_no_split_count_has_priority(
        NARROW_SOURCE_LIST_MAX_BLOCKS,
    ));
}

#[test]
fn no_split_replay_requires_a_new_parent_state() {
    let compressed = [0_u8];
    let source = CandidateInput {
        compressed: &compressed,
        blocks: &[],
        meaningful_bits: 8,
        decoded_limit: 0,
        identity: StreamIdentity {
            decoded_size: 0,
            crc32: 0,
            adler32: 0,
        },
    };
    let mut candidate = comparison_candidate(1, 7, 1);

    // Different output bytes alone can be only a better Huffman header;
    // Max already priced that token state, so Default must not repeat it.
    assert!(!candidate_exposes_new_parent(&candidate, source));

    candidate.plans.push(PlannedBlock {
        tokens: Arc::new(vec![Token::Literal(0)]),
        plain: Arc::new(vec![0]),
        representation: Representation::Fixed,
        bits: 10,
        source_type: SourceBlockType::Fixed,
    });
    assert!(candidate_exposes_new_parent(&candidate, source));

    candidate.data.copy_from_slice(&compressed);
    assert!(!candidate_exposes_new_parent(&candidate, source));
}

#[test]
fn one_pass_closes_repeated_deflopt_defluff_feedback() {
    // This small synthetic dynamic block witnesses the interaction between
    // the recovered DeflOpt and Defluff methods. DeflOpt expands one
    // length-three match into literals and rebuilds a stream that is five
    // bits smaller overall. On that new frequency state, Defluff swaps the
    // five- and six-bit assignments of equal-frequency length symbols 259
    // and 260. The payload remains 196 bits while the dynamic header
    // shrinks from 130 to 129 bits. The original tools therefore need two
    // programs/passes: 331 -> 326 -> 325 meaningful bits.
    //
    // Columbo's broader feedback route must close the same state graph in
    // one invocation. A second invocation must not find another saving.
    let input = FEEDBACK_RAW;
    let source = parse_stream(input, 86).unwrap();
    assert_eq!(source.meaningful_bits, 331);
    assert_eq!(source.decoded_size, 86);

    let options = Options {
        strict: false,
        ..Options::default()
    };
    let first = optimize_raw(input, &options).unwrap();
    let reparsed_first = parse_stream(&first.data, 86).unwrap();
    assert_eq!(first.output_max_distance, reparsed_first.max_distance);
    // Later additive structural routes may beat the recovered 325-bit
    // fixed point, but must never lose it.
    assert!(first.info.deflate_bits <= 325);
    let second = optimize_raw(&first.data, &options).unwrap();
    assert_eq!(second.info.deflate_bits, first.info.deflate_bits);
    assert_eq!(second.data, first.data);

    // Even a zero-budget max run must finish and retain Default. The zero
    // allowance disables Max-exclusive routes; it cannot weaken the
    // mandatory comparison floor.
    let zero_budget_max = Options {
        exhaustive: true,
        strict: false,
        timeout: Duration::ZERO,
        ..Options::default()
    };
    let stopped = optimize_raw(input, &zero_budget_max).unwrap();
    assert!(stopped.timed_out);
    assert!(stopped.data.len() <= first.data.len());
    assert!(stopped.info.deflate_bits <= first.info.deflate_bits);

    // With sufficient time, max mode closes the same feedback graph in one
    // invocation and is itself at a fixed point.
    let max_options = Options {
        timeout: Duration::from_secs(1),
        ..zero_budget_max
    };
    let maximum = optimize_raw(input, &max_options).unwrap();
    let reparsed_maximum = parse_stream(&maximum.data, 86).unwrap();
    assert_eq!(maximum.output_max_distance, reparsed_maximum.max_distance);
    assert!(maximum.info.deflate_bits <= first.info.deflate_bits);
    let repeated_max = optimize_raw(&maximum.data, &max_options).unwrap();
    assert_eq!(repeated_max.data, maximum.data);
}

#[test]
fn zero_budget_max_floors_retain_their_exact_default_endpoint() {
    for strict in [true, false] {
        let default_options = Options {
            strict,
            timeout: Duration::from_secs(1),
            ..Options::default()
        };
        let default = optimize_raw_prefix_with_floor(
            FEEDBACK_RAW,
            &default_options,
            86,
            DefaultFloor::Shared,
        )
        .unwrap();
        let apng_default = optimize_raw_prefix_with_floor(
            FEEDBACK_RAW,
            &default_options,
            86,
            DefaultFloor::ApngDefault,
        )
        .unwrap();
        let max_options = Options {
            exhaustive: true,
            timeout: Duration::ZERO,
            ..default_options
        };

        for (floor, expected) in [
            (DefaultFloor::Complete, &default),
            (DefaultFloor::SharedExact, &default),
            (DefaultFloor::ApngMax, &apng_default),
        ] {
            let maximum =
                optimize_raw_prefix_with_floor(FEEDBACK_RAW, &max_options, 86, floor).unwrap();
            assert!(
                maximum.data.len() <= expected.data.len(),
                "{floor:?}, strict={strict}"
            );
            assert!(
                maximum.info.deflate_bits <= expected.info.deflate_bits,
                "{floor:?}, strict={strict}"
            );
        }
    }
}

#[test]
fn optimizes_a_dynamic_header_with_all_32_distance_code_lengths() {
    // This empty dynamic block advertises the full RFC 1951 HDIST range
    // with complete literal/length and distance trees. Reserved distance
    // symbol 31 participates in tree construction but is never decoded.
    let input = [
        0x05, 0xdf, 0x01, 0x04, 0x00, 0x00, 0x00, 0x00, 0x90, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x80, 0x01, 0x00, 0x00, 0x80,
        0x01,
    ];

    let optimized = optimize_raw(&input, &Options::default()).unwrap();
    let reparsed = parse_stream(&optimized.data, 0).unwrap();
    assert_eq!(reparsed.decoded_size, 0);
    assert!(optimized.data.len() <= input.len());
}

#[test]
fn strict_mode_never_creates_the_non_rfc_258_alias() {
    let (literal, distance) = fixed_trees();
    let mut writer = BitWriter::default();
    writer.write(1, 1).unwrap(); // Final block.
    writer.write(1, 2).unwrap(); // Fixed Huffman block.
    for code in [
        literal.code(usize::from(b'A')).unwrap(),
        literal.code(285).unwrap(), // RFC length-258 spelling.
        distance.code(0).unwrap(),
        literal.code(256).unwrap(),
    ] {
        writer.write(u32::from(code.code), code.length).unwrap();
    }
    let input = writer.into_bytes();

    let optimized = optimize_raw(&input, &Options::default()).unwrap();
    let reparsed = parse_stream(&optimized.data, 259).unwrap();
    assert_eq!(reparsed.decoded_size, 259);
    assert!(reparsed.blocks.iter().all(|block| {
        block.tokens.iter().all(|token| {
            !matches!(
                token,
                Token::Match {
                    length_symbol: 284,
                    length_extra: 31,
                    ..
                }
            )
        })
    }));
}

#[test]
fn default_route_coalesces_adjacent_same_distance_matches() {
    let (literal, distance) = fixed_trees();
    let mut writer = BitWriter::default();
    writer.write(1, 1).unwrap(); // Final block.
    writer.write(1, 2).unwrap(); // Fixed Huffman block.
    let literal_a = literal.code(usize::from(b'A')).unwrap();
    writer
        .write(u32::from(literal_a.code), literal_a.length)
        .unwrap();
    for length_symbol in [257, 258] {
        let length = literal.code(length_symbol).unwrap();
        writer.write(u32::from(length.code), length.length).unwrap();
        let distance_one = distance.code(0).unwrap();
        writer
            .write(u32::from(distance_one.code), distance_one.length)
            .unwrap();
    }
    let end = literal.code(256).unwrap();
    writer.write(u32::from(end.code), end.length).unwrap();
    let input = writer.into_bytes();

    for strict in [true, false] {
        let options = Options {
            strict,
            ..Options::default()
        };
        let optimized = optimize_raw(&input, &options).unwrap();
        let reparsed = parse_stream(&optimized.data, 8).unwrap();
        assert_eq!(reparsed.decoded_size, 8);
        let tokens = reparsed
            .blocks
            .iter()
            .flat_map(|block| block.tokens.iter())
            .copied()
            .collect::<Vec<_>>();
        assert!(matches!(
            tokens.as_slice(),
            [
                Token::Literal(b'A'),
                Token::Match {
                    length: 7,
                    distance: 1,
                    ..
                }
            ]
        ));
        assert!(optimized.data.len() <= input.len());
        let repeated = optimize_raw(&optimized.data, &options).unwrap();
        assert_eq!(repeated.data, optimized.data);
    }

    // A coalesced length 258 must use canonical symbol 285 in strict mode,
    // even though symbol 284 plus extra value 31 decodes to the same size
    // in Columbo's relaxed compatibility extension.
    let mut writer = BitWriter::default();
    writer.write(1, 1).unwrap();
    writer.write(1, 2).unwrap();
    writer
        .write(u32::from(literal_a.code), literal_a.length)
        .unwrap();
    for (length_symbol, extra, extra_bits) in [(279, 1, 4), (281, 27, 5)] {
        let length = literal.code(length_symbol).unwrap();
        writer.write(u32::from(length.code), length.length).unwrap();
        writer.write(extra, extra_bits).unwrap();
        let distance_one = distance.code(0).unwrap();
        writer
            .write(u32::from(distance_one.code), distance_one.length)
            .unwrap();
    }
    writer.write(u32::from(end.code), end.length).unwrap();
    let input = writer.into_bytes();
    let optimized = optimize_raw(&input, &Options::default()).unwrap();
    let reparsed = parse_stream(&optimized.data, 259).unwrap();
    assert!(matches!(
        reparsed.blocks.as_slice(),
        [ParsedBlock { tokens, .. }]
            if matches!(
                tokens.as_slice(),
                [
                    Token::Literal(b'A'),
                    Token::Match {
                        length: 258,
                        length_symbol: 285,
                        length_extra: 0,
                        length_extra_bits: 0,
                        ..
                    }
                ]
            )
    ));
}

#[test]
fn default_route_resegments_a_proven_match_without_changing_its_distance() {
    let input = [
        0x65, 0xc1, 0x31, 0x01, 0x00, 0x00, 0x00, 0xc2, 0xa0, 0x6c, 0xf4, 0x2f, 0xe5, 0x3f, 0x41,
        0x29, 0xa5, 0x94, 0x72, 0x06,
    ];
    let source = parse_stream(&input, 120).unwrap();
    assert_eq!(source.meaningful_bits, 156);
    let source_matches = source
        .blocks
        .iter()
        .flat_map(|block| block.tokens.iter())
        .filter_map(|token| match *token {
            Token::Match {
                length, distance, ..
            } => Some((length, distance)),
            Token::Literal(_) => None,
        })
        .collect::<Vec<_>>();
    let mut expected_source_matches = vec![(16, 1); 6];
    expected_source_matches.push((17, 1));
    assert_eq!(source_matches, expected_source_matches);

    for strict in [true, false] {
        let options = Options {
            strict,
            ..Options::default()
        };
        let optimized = optimize_raw(&input, &options).unwrap();
        assert!(optimized.info.deflate_bits <= 151);
        assert!(optimized.info.deflate_bits < source.meaningful_bits);
        assert!(optimized.data.len() < input.len());

        let reparsed = parse_stream(&optimized.data, 120).unwrap();
        assert_eq!(reparsed.decoded_size, source.decoded_size);
        assert_eq!(reparsed.crc32, source.crc32);
        assert_eq!(reparsed.adler32, source.adler32);
        let tokens = reparsed
            .blocks
            .iter()
            .flat_map(|block| block.tokens.iter())
            .copied()
            .collect::<Vec<_>>();
        assert_eq!(
            tokens
                .iter()
                .filter(|token| matches!(token, Token::Literal(b'A')))
                .count(),
            8
        );
        assert_eq!(
            tokens
                .iter()
                .filter(|token| {
                    matches!(
                        token,
                        Token::Match {
                            length: 16,
                            distance: 1,
                            length_symbol: 267,
                            ..
                        }
                    )
                })
                .count(),
            7
        );
        assert!(matches!(
            tokens.as_slice(),
            [
                ..,
                Token::Literal(b'A'),
                Token::Match {
                    length: 16,
                    distance: 1,
                    length_symbol: 267,
                    ..
                }
            ]
        ));

        let repeated = optimize_raw(&optimized.data, &options).unwrap();
        assert_eq!(repeated.info.deflate_bits, optimized.info.deflate_bits);
        assert_eq!(repeated.data, optimized.data);
    }
}

#[test]
fn default_route_exact_prices_a_nonhighest_source_symbol_tie() {
    let input = [
        0x75, 0xc1, 0x41, 0x0d, 0x00, 0x00, 0x0c, 0x03, 0x21, 0x6d, 0xf8, 0x37, 0xb5, 0x7f, 0x97,
        0x03, 0xcb, 0xb2, 0x3c, 0x82, 0x20, 0x08, 0x0e,
    ];
    let source = parse_stream(&input, 166).unwrap();
    assert_eq!(source.meaningful_bits, 181);
    assert_eq!(source.blocks.len(), 1);
    assert_eq!(source.blocks[0].literal_frequencies[268], 1);
    assert_eq!(source.blocks[0].literal_frequencies[270], 4);
    assert_eq!(
        source.blocks[0]
            .tokens
            .iter()
            .filter(|token| {
                matches!(
                    token,
                    Token::Match {
                        length: 16,
                        distance: 1,
                        ..
                    }
                )
            })
            .count(),
        3
    );

    let optimized = optimize_raw(&input, &Options::default()).unwrap();
    assert_eq!(optimized.info.deflate_bits, 178);
    assert_eq!(optimized.data.len(), input.len());
    let reparsed = parse_stream(&optimized.data, 166).unwrap();
    assert_eq!(reparsed.decoded_size, source.decoded_size);
    assert_eq!(reparsed.crc32, source.crc32);
    assert_eq!(reparsed.adler32, source.adler32);
    assert_eq!(reparsed.blocks.len(), 1);
    assert_eq!(reparsed.blocks[0].literal_frequencies[268], 0);
    assert_eq!(reparsed.blocks[0].literal_frequencies[270], 4);
    assert_eq!(
        reparsed.blocks[0]
            .tokens
            .iter()
            .filter(|token| matches!(token, Token::Literal(b'A')))
            .count(),
        10
    );
    assert_eq!(
        reparsed.blocks[0]
            .tokens
            .iter()
            .filter(|token| {
                matches!(
                    token,
                    Token::Match {
                        length: 16,
                        distance: 1,
                        length_symbol: 267,
                        length_extra: 1,
                        length_extra_bits: 1,
                        distance_symbol: 0,
                        distance_extra: 0,
                        distance_extra_bits: 0,
                    }
                )
            })
            .count(),
        4
    );

    let repeated = optimize_raw(&optimized.data, &Options::default()).unwrap();
    assert_eq!(repeated.info.deflate_bits, optimized.info.deflate_bits);
    assert_eq!(repeated.data, optimized.data);
}

#[test]
fn strict_mode_repairs_alias_and_single_distance_code_inputs() {
    // This valid compatibility-extension input is a relaxed fixed point:
    // its dynamic tree has one usable distance code and its final
    // length-258 match uses Defluff's non-standard symbol-284 alias.
    // Canonicalizing either detail costs bits, so this catches accidental
    // source reuse in strict mode.
    let input = [
        0xe5, 0xc0, 0x81, 0x00, 0x00, 0x00, 0x00, 0x80, 0x20, 0xb6, 0xfd, 0xa5, 0x06, 0xa9, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0xbe, 0x01,
    ];
    let source = parse_stream(&input, 2_302).unwrap();
    assert!(stream_uses_258_alias(&source));
    assert!(source.blocks.iter().any(|block| {
        block
            .original_dynamic
            .as_ref()
            .is_some_and(|dynamic| !dynamic.has_two_usable_distance_codes())
    }));

    let strict = optimize_raw(&input, &Options::default()).unwrap();
    let strict_stream = parse_stream(&strict.data, 2_302).unwrap();
    assert_eq!(strict_stream.decoded_size, source.decoded_size);
    assert!(!stream_uses_258_alias(&strict_stream));
    let strict_dynamic: Vec<_> = strict_stream
        .blocks
        .iter()
        .filter_map(|block| block.original_dynamic.as_ref())
        .collect();
    // A cheaper fixed rewrite is also strictly compatible. If the
    // selected stream remains dynamic, all three alphabets must be
    // complete and its distance tree must contain two usable symbols.
    for dynamic in strict_dynamic {
        assert!(dynamic.has_strictly_compatible_huffman_codes());
        assert!(huffman_tree_shape_is_complete(&dynamic.literal_lengths));
        assert!(huffman_tree_shape_is_complete(&dynamic.distance_lengths));
        assert!(huffman_tree_shape_is_complete(&dynamic.code_length_lengths));
    }

    let relaxed_options = Options {
        strict: false,
        ..Options::default()
    };
    let relaxed = optimize_raw(&input, &relaxed_options).unwrap();
    let relaxed_stream = parse_stream(&relaxed.data, 2_302).unwrap();
    assert_eq!(relaxed_stream.decoded_size, source.decoded_size);
    assert!(relaxed.data.len() <= input.len());
    // Relaxed mode permits the source exceptions but may still select an
    // independently cheaper canonical fixed or dynamic representation.
    // Parsing and explicit alias-rewrite tests cover retention of those
    // extensions when they remain the winning spelling.
}

fn stream_uses_258_alias(stream: &ParsedStream) -> bool {
    stream.blocks.iter().any(|block| {
        block.tokens.iter().any(|token| {
            matches!(
                token,
                Token::Match {
                    length: 258,
                    length_symbol: 284,
                    length_extra: 31,
                    length_extra_bits: 5,
                    ..
                }
            )
        })
    })
}

#[test]
fn optimization_never_discovers_new_lz77_matches() {
    // A stored block has only literal source tokens. Even though this
    // deliberately repetitive payload would be easy to recompress with
    // new matches, Columbo may only choose a cheaper serialization of the
    // existing parse.
    let plain = vec![b'A'; 1_024];
    let length = plain.len() as u16;
    let mut input = vec![0x01]; // Final stored block, then byte alignment.
    input.extend_from_slice(&length.to_le_bytes());
    input.extend_from_slice(&(!length).to_le_bytes());
    input.extend_from_slice(&plain);

    let optimized = optimize_raw(&input, &Options::default()).unwrap();
    let reparsed = parse_stream(&optimized.data, plain.len() as u64).unwrap();

    assert_eq!(reparsed.decoded_size, plain.len() as u64);
    assert!(reparsed
        .blocks
        .iter()
        .flat_map(|block| block.tokens.iter())
        .all(|token| matches!(token, Token::Literal(_))));
}

#[test]
fn timeout_still_returns_one_complete_stream() {
    // A final empty fixed block occupies ten meaningful bits. A zero
    // deadline skips optional searches, but must never truncate parsing
    // or emission halfway through that block.
    let input = [0x03, 0x00];
    let options = Options {
        timeout: Duration::ZERO,
        ..Options::default()
    };

    let optimized = optimize_raw(&input, &options).unwrap();
    assert_eq!(optimized.consumed, input.len());
    assert!(optimized.timed_out);

    let reparsed = parse_stream(&optimized.data, 1).unwrap();
    assert_eq!(reparsed.consumed, optimized.data.len());
    assert_eq!(reparsed.decoded_size, 0);
}

#[test]
fn max_keeps_the_finished_default_floor_after_timeout() {
    let input = [0x03, 0x00];
    let default = optimize_raw(&input, &Options::default()).unwrap();
    let max_options = Options {
        exhaustive: true,
        timeout: Duration::ZERO,
        ..Options::default()
    };

    let maximum = optimize_raw(&input, &max_options).unwrap();
    assert!(maximum.timed_out);
    assert!(
        maximum.data.len() < default.data.len()
            || (maximum.data.len() == default.data.len()
                && maximum.info.deflate_bits <= default.info.deflate_bits)
    );
}

#[test]
fn shared_max_floor_still_returns_a_complete_stream() {
    let input = [0x03, 0x00];
    let options = Options {
        exhaustive: true,
        timeout: Duration::ZERO,
        ..Options::default()
    };

    let optimized = optimize_raw_prefix_with_floor(
        &input,
        &options,
        options.max_decoded_bytes,
        DefaultFloor::Shared,
    )
    .unwrap();
    assert!(optimized.timed_out);
    assert_eq!(optimized.consumed, input.len());

    let reparsed = parse_stream(&optimized.data, 1).unwrap();
    assert_eq!(reparsed.consumed, optimized.data.len());
    assert_eq!(reparsed.decoded_size, 0);
}

#[test]
fn every_potentially_improvable_source_topology_admits_the_deft4j_route() {
    // A one-literal fixed stream represents the single-block members used
    // by GZIP, ZIP, and compressed PNG metadata. Container floor policy
    // must not make its direct deft4j endpoint unreachable.
    let parsed = parse_stream(&[0x4b, 0x04, 0x00], 1).unwrap();
    assert_eq!(parsed.decoded_size, 1);
    assert!(deft4j_source_route_eligible(&parsed.blocks));

    // Route memory accounting, rather than an arbitrary source-list cap,
    // controls very fragmented streams.
    let many = (0..129)
        .map(|_| parsed.blocks[0].try_clone_shared().unwrap())
        .collect::<Vec<_>>();
    assert!(deft4j_source_route_eligible(&many));
}

#[test]
fn no_split_route_uses_bounded_source_size_and_topology() {
    let parsed = parse_stream(&[0x4b, 0x04, 0x00], 1).unwrap();
    let blocks = vec![
        parsed.blocks[0].try_clone_shared().unwrap(),
        parsed.blocks[0].try_clone_shared().unwrap(),
    ];

    assert!(narrow_source_route_eligible(
        &blocks,
        NARROW_SOURCE_MAX_COMPRESSED
    ));
    assert!(!narrow_source_route_eligible(
        &blocks,
        NARROW_SOURCE_MAX_COMPRESSED + 1
    ));

    let too_many = (0..=NARROW_SOURCE_LIST_MAX_BLOCKS)
        .map(|_| parsed.blocks[0].try_clone_shared().unwrap())
        .collect::<Vec<_>>();
    assert!(!narrow_source_route_eligible(&too_many, 1));

    let mut stored = blocks;
    stored[0].source_type = SourceBlockType::Stored;
    assert!(!narrow_source_route_eligible(&stored, 1));
}

#[test]
fn max_replay_ceiling_covers_every_strict_stream_score() {
    // Meaningful bits can occupy only the eight residues belonging to each
    // emitted byte length. Max therefore gets a complete metric bound,
    // while Default retains its small consistent replay budget.
    assert_eq!(resolved_replay_limit(MAX_RAW_REPLAY_LIMIT, 10), 80);
    assert_eq!(resolved_replay_limit(DEFAULT_RAW_REPLAY_LIMIT, 10), 3);
    assert_eq!(resolved_replay_limit(MAX_RAW_REPLAY_LIMIT, 0), 1);
    assert_eq!(
        resolved_replay_limit(MAX_RAW_REPLAY_LIMIT, usize::MAX),
        usize::MAX
    );
}

#[test]
fn complete_then_bounded_floor_keeps_the_finished_default_at_zero_timeout() {
    let input = [0x03, 0x00];
    let default = optimize_raw(&input, &Options::default()).unwrap();
    let options = Options {
        exhaustive: true,
        timeout: Duration::ZERO,
        ..Options::default()
    };

    let maximum = optimize_raw_prefix_with_floor(
        &input,
        &options,
        options.max_decoded_bytes,
        DefaultFloor::CompleteThenBounded,
    )
    .unwrap();

    assert!(maximum.timed_out);
    assert!(
        maximum.data.len() < default.data.len()
            || (maximum.data.len() == default.data.len()
                && maximum.info.deflate_bits <= default.info.deflate_bits)
    );
}

#[test]
fn completed_png_floor_is_reused_without_rebuilding() {
    let input = [0x03, 0x00];
    let parsed = parse_stream(&input, 1).unwrap();
    let source = CandidateInput {
        compressed: &input,
        blocks: &parsed.blocks,
        meaningful_bits: parsed.meaningful_bits,
        decoded_limit: 1,
        identity: StreamIdentity {
            decoded_size: parsed.decoded_size,
            crc32: parsed.crc32,
            adler32: parsed.adler32,
        },
    };
    let options = Options::default();
    let completed = build_candidate(
        source,
        &options,
        DEFAULT_RAW_REPLAY_LIMIT,
        &mut SearchStop::never(),
    )
    .unwrap();
    let expected_data = completed.data.clone();
    let expected_bits = completed.bits;

    let mut must_not_build = || panic!("reusing a completed floor must not start another build");
    let reused = completed_or_bounded_floor(
        source,
        &options,
        Some(completed),
        &mut SearchStop::callback(&mut must_not_build),
    )
    .unwrap();

    assert_eq!(reused.data, expected_data);
    assert_eq!(reused.bits, expected_bits);
    assert_eq!(reused.route, "Normal floor");
}

#[test]
fn established_floor_is_a_complete_strict_max_candidate() {
    let source = [0x03, 0x00];
    let established = optimize_raw(&source, &Options::default()).unwrap();
    let options = Options {
        exhaustive: true,
        timeout: Duration::ZERO,
        ..Options::default()
    };

    let maximum = optimize_raw_prefix_with_floor(
        &established.data,
        &options,
        options.max_decoded_bytes,
        DefaultFloor::Established,
    )
    .unwrap();

    assert!(maximum.timed_out);
    assert_eq!(maximum.data, established.data);
    assert_eq!(maximum.info.deflate_bits, established.info.deflate_bits);
}

#[test]
fn bounded_parallel_floors_preserve_a_complete_stream() {
    // Two nonempty Huffman blocks activate both independent phase-one
    // routes. They share the parsed token/plain buffers and wall clock,
    // then rejoin before any deft4j refinement is considered.
    let (literal, _) = fixed_trees();
    let end = literal.code(256).unwrap();
    let mut writer = BitWriter::default();
    for (index, value) in b"ab".iter().copied().enumerate() {
        let code = literal.code(usize::from(value)).unwrap();
        writer.write(u32::from(index == 1), 1).unwrap();
        writer.write(1, 2).unwrap();
        writer.write(u32::from(code.code), code.length).unwrap();
        writer.write(u32::from(end.code), end.length).unwrap();
    }
    let input = writer.into_bytes();
    let source = parse_stream(&input, 2).unwrap();
    assert_eq!(source.source_block_count, 2);

    let options = Options {
        exhaustive: true,
        timeout: Duration::from_millis(100),
        ..Options::default()
    };
    let optimized = optimize_raw_prefix_with_floor(
        &input,
        &options,
        options.max_decoded_bytes,
        DefaultFloor::CompleteThenBounded,
    )
    .unwrap();
    let reparsed = parse_stream(&optimized.data, 2).unwrap();
    let plain: Vec<_> = reparsed
        .blocks
        .iter()
        .flat_map(|block| block.plain.iter().copied())
        .collect();

    assert_eq!(plain, b"ab");
    assert_eq!(optimized.consumed, input.len());
    assert!(
        optimized.data.len() < input.len()
            || (optimized.data.len() == input.len()
                && optimized.info.deflate_bits <= source.meaningful_bits)
    );
}

#[test]
fn deferred_bounded_png_floor_retains_exact_default_at_zero_timeout() {
    // Exercise the path used by medium multi-block PNG streams, where the
    // exact Default dependency is established inside the bounded phase
    // rather than by the earlier compact/large-stream prebuild.
    let (literal, _) = fixed_trees();
    let end = literal.code(256).unwrap();
    let mut writer = BitWriter::default();
    for (index, value) in b"ab".iter().copied().enumerate() {
        let code = literal.code(usize::from(value)).unwrap();
        writer.write(u32::from(index == 1), 1).unwrap();
        writer.write(1, 2).unwrap();
        writer.write(u32::from(code.code), code.length).unwrap();
        writer.write(u32::from(end.code), end.length).unwrap();
    }
    let input = writer.into_bytes();
    let parsed = parse_stream(&input, 2).unwrap();
    let source = CandidateInput {
        compressed: &input,
        blocks: &parsed.blocks,
        meaningful_bits: parsed.meaningful_bits,
        decoded_limit: 2,
        identity: StreamIdentity {
            decoded_size: parsed.decoded_size,
            crc32: parsed.crc32,
            adler32: parsed.adler32,
        },
    };
    let options = Options {
        exhaustive: true,
        timeout: Duration::ZERO,
        ..Options::default()
    };
    let deadline = Deadline::new(Instant::now(), Duration::ZERO);
    let candidates = build_bounded_phase_candidates(
        source,
        &options,
        true,
        false,
        false,
        false,
        false,
        false,
        true,
        &deadline,
        Progress::begin(
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
            None,
        ),
        None,
    )
    .unwrap();
    let floor = candidates.floor.unwrap();
    let ordinary = optimize_raw(&input, &Options::default()).unwrap();

    assert!(floor.data.len() <= ordinary.data.len());
    assert!(floor.bits <= ordinary.info.deflate_bits);
}

#[test]
fn compact_split_floor_gate_is_tightly_bounded() {
    let (literal, _) = fixed_trees();
    let value = literal.code(usize::from(b'a')).unwrap();
    let end = literal.code(256).unwrap();
    let mut writer = BitWriter::default();
    for block_index in 0..2 {
        writer.write(u32::from(block_index == 1), 1).unwrap();
        writer.write(1, 2).unwrap();
        for _ in 0..128 {
            writer.write(u32::from(value.code), value.length).unwrap();
        }
        writer.write(u32::from(end.code), end.length).unwrap();
    }
    let parsed = parse_stream(&writer.into_bytes(), 256).unwrap();
    assert!(compact_source_split_floor_eligible(
        parsed.decoded_size,
        &parsed.blocks
    ));
    let plans: Vec<_> = parsed
        .blocks
        .iter()
        .map(|block| {
            let original = block.original.expect("parsed block has source bits");
            PlannedBlock {
                tokens: Arc::clone(&block.tokens),
                plain: Arc::clone(&block.plain),
                representation: Representation::Original(original),
                bits: original.len,
                source_type: block.source_type,
            }
        })
        .collect();
    assert!(compact_split_preserves_source_blocks(
        &parsed.blocks,
        &plans
    ));
    let mut changed_plans = plans.clone();
    changed_plans[0].plain = vec![b'a'; 127].into();
    assert!(!compact_split_preserves_source_blocks(
        &parsed.blocks,
        &changed_plans
    ));
    let mut changed_type = plans;
    changed_type[0].representation = Representation::Stored;
    assert!(!compact_split_preserves_source_blocks(
        &parsed.blocks,
        &changed_type
    ));
    assert!(!compact_source_split_floor_eligible(
        COMPACT_SPLIT_FLOOR_MAX_DECODED + 1,
        &parsed.blocks
    ));
    assert!(!compact_source_split_floor_eligible(
        parsed.decoded_size,
        &parsed.blocks[..1]
    ));
    assert!(compact_source_split_floor_eligible_with_limits(
        parsed.blocks[0].plain.len() as u64,
        &parsed.blocks[..1],
        1,
        TERMINAL_SOURCE_SPLIT_MAX_DECODED,
    ));
    assert!(!compact_source_split_floor_eligible_with_limits(
        TERMINAL_SOURCE_SPLIT_MAX_DECODED + 1,
        &parsed.blocks[..1],
        1,
        TERMINAL_SOURCE_SPLIT_MAX_DECODED,
    ));

    let mut stored = parsed.blocks.clone();
    stored[0].source_type = SourceBlockType::Stored;
    assert!(!compact_source_split_floor_eligible(
        parsed.decoded_size,
        &stored
    ));

    let mut too_many_tokens = parsed.blocks.clone();
    too_many_tokens[0].tokens = vec![Token::Literal(b'a'); COMPACT_SPLIT_FLOOR_MAX_TOKENS].into();
    assert!(!compact_source_split_floor_eligible(
        parsed.decoded_size,
        &too_many_tokens
    ));

    let mut tree_block = parsed.blocks[0].clone();
    tree_block.source_type = SourceBlockType::Dynamic;
    tree_block.tokens = vec![Token::Literal(b'a')].into();
    assert!(compact_balanced_tree_source_eligible(
        1,
        tree_block.plain.len() as u64,
        std::slice::from_ref(&tree_block),
    ));
    assert!(!compact_balanced_tree_source_eligible(
        COMPACT_TREE_MAX_COMPRESSED + 1,
        tree_block.plain.len() as u64,
        std::slice::from_ref(&tree_block),
    ));
    assert!(!compact_balanced_tree_source_eligible(
        1,
        tree_block.plain.len() as u64,
        &[tree_block.clone(), tree_block.clone()],
    ));

    tree_block.distance_frequencies = [0; 30];
    assert!(compact_strict_literal_tree_eligible(
        1,
        std::slice::from_ref(&tree_block),
    ));
    tree_block.distance_frequencies[0] = 1;
    assert!(!compact_strict_literal_tree_eligible(
        1,
        std::slice::from_ref(&tree_block),
    ));
}

#[test]
fn compact_split_parents_are_ordered_by_complete_stream_score() {
    fn seed(bytes: usize, bits: u64) -> CompactSplitSeed {
        CompactSplitSeed {
            data: vec![0; bytes],
            bits,
            stream: parse_stream(&[0x03, 0x00], 1).unwrap(),
        }
    }

    let largest = seed(12, 80);
    let bit_tie = seed(10, 79);
    let smallest = seed(10, 78);
    let ordered = ordered_compact_split_seeds([Some(&largest), Some(&bit_tie), Some(&smallest)]);

    assert_eq!(
        ordered
            .iter()
            .map(|seed| (seed.data.len(), seed.bits))
            .collect::<Vec<_>>(),
        [(10, 78), (10, 79), (12, 80)]
    );

    let tied_first = seed(10, 78);
    let tied_second = seed(10, 78);
    let ordered =
        ordered_compact_split_seeds([Some(&tied_first), Some(&tied_second), Some(&largest)]);
    assert!(std::ptr::eq(ordered[0], &tied_first));
    assert!(std::ptr::eq(ordered[1], &tied_second));
}

#[test]
fn completed_compact_split_parent_requires_exact_encoded_identity() {
    let candidate = comparison_candidate(4, 29, 7);

    assert!(compact_split_parent_is_completed(
        &candidate,
        Some(&[7, 7, 7, 7])
    ));
    assert!(!compact_split_parent_is_completed(
        &candidate,
        Some(&[7, 7, 7, 6])
    ));
    assert!(!compact_split_parent_is_completed(&candidate, None));
}

#[test]
fn bounded_generic_routes_preserve_complete_candidates() {
    // Empty stored blocks have no useful deft4j or narrow source route.
    // They make a small generic-only stream whose floor and original-source
    // max routes can be checked independently after their parallel phase
    // rejoins.
    let input = [
        0x00, 0x00, 0x00, 0xff, 0xff, // Non-final empty stored block.
        0x01, 0x00, 0x00, 0xff, 0xff, // Final empty stored block.
    ];
    let parsed = parse_stream(&input, 1).unwrap();
    assert_eq!(parsed.source_block_count, 2);
    assert!(!deft4j_source_route_eligible(&parsed.blocks));
    assert!(!narrow_source_route_eligible(&parsed.blocks, input.len()));

    let options = Options {
        exhaustive: true,
        timeout: Duration::MAX,
        ..Options::default()
    };
    let identity = StreamIdentity {
        decoded_size: parsed.decoded_size,
        crc32: parsed.crc32,
        adler32: parsed.adler32,
    };
    let source = CandidateInput {
        compressed: &input,
        blocks: &parsed.blocks,
        meaningful_bits: parsed.meaningful_bits,
        decoded_limit: 1,
        identity,
    };
    let deadline = Deadline::new(Instant::now(), Duration::MAX);
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
        None,
    );
    let candidates =
        build_bounded_generic_max_candidates(source, &options, &deadline, progress, None).unwrap();

    assert!(candidates.floor_seeded.is_some());
    assert!(candidates.source_max.is_some());
    assert_eq!(
        candidates
            .source_max
            .as_ref()
            .map(|candidate| candidate.route),
        Some("Columbo source max route")
    );
    assert!(candidates.suppress_later_source_max);
    assert!(!candidates.suppress_later_optional_routes);
    assert!(candidates.deft4j.is_none());
    assert!(candidates.narrow.is_none());
    for candidate in [
        candidates.floor.as_ref(),
        candidates.floor_seeded.as_ref(),
        candidates.source_max.as_ref(),
    ]
    .into_iter()
    .flatten()
    {
        let reparsed = parse_stream(&candidate.data, 2).unwrap();
        validate_replayed_stream(&reparsed, candidate.data.len(), identity).unwrap();
    }
    assert!(!deadline.was_triggered());
}

#[test]
fn parallel_routes_require_small_input_and_model_sizes() {
    assert!(parallel_route_sizes_are_bounded(
        PARALLEL_ROUTE_MAX_COMPRESSED,
        1,
        1,
        1,
    ));
    assert!(!parallel_route_sizes_are_bounded(
        PARALLEL_ROUTE_MAX_COMPRESSED + 1,
        1,
        1,
        1,
    ));
    assert!(!parallel_route_sizes_are_bounded(
        1,
        PARALLEL_ROUTE_MAX_DECODED + 1,
        1,
        1,
    ));

    let oversized_token_count = PARALLEL_ROUTE_MAX_MODEL / std::mem::size_of::<Token>() + 1;
    assert!(!parallel_route_sizes_are_bounded(
        1,
        1,
        oversized_token_count,
        1,
    ));
}

#[test]
fn bounded_depth_rescue_has_explicit_reparse_and_emission_limits() {
    assert!(bounded_depth_rescue_sizes_are_bounded(
        BOUNDED_DEPTH_RESCUE_MAX_COMPRESSED,
        BOUNDED_DEPTH_RESCUE_MAX_DECODED,
    ));
    assert!(!bounded_depth_rescue_sizes_are_bounded(
        BOUNDED_DEPTH_RESCUE_MAX_COMPRESSED + 1,
        1,
    ));
    assert!(!bounded_depth_rescue_sizes_are_bounded(
        1,
        BOUNDED_DEPTH_RESCUE_MAX_DECODED + 1,
    ));
}

#[test]
fn bounded_floor_prebuild_uses_topology_and_route_work() {
    assert!(prebuild_bounded_floor(1, 1));
    assert!(prebuild_bounded_floor(
        2,
        PREBUILD_BOUNDED_FLOOR_MAX_DECODED
    ));
    assert!(!prebuild_bounded_floor(
        2,
        PREBUILD_BOUNDED_FLOOR_MAX_DECODED + 1
    ));
    assert!(!prebuild_bounded_floor(
        2,
        CONCURRENT_BOUNDED_FLOOR_MAX_DECODED
    ));
    assert!(prebuild_bounded_floor(
        2,
        CONCURRENT_BOUNDED_FLOOR_MAX_DECODED + 1
    ));

    let source_match = Token::Match {
        length: 3,
        distance: 1,
        length_symbol: 257,
        distance_symbol: 0,
        length_extra: 0,
        distance_extra: 0,
        length_extra_bits: 0,
        distance_extra_bits: 0,
    };
    let block = ParsedBlock {
        tokens: Arc::new(vec![source_match; PROVEN_SUBMATCH_FULL_MATCH_LIMIT + 1]),
        plain: Arc::new(Vec::new()),
        literal_frequencies: [0; 286],
        distance_frequencies: [0; 30],
        original_literal_lengths: None,
        original_distance_lengths: None,
        original_dynamic: None,
        original: None,
        source_splits: Vec::new(),
        source_type: SourceBlockType::Dynamic,
    };
    assert!(source_run_match_count_exceeds(
        std::slice::from_ref(&block),
        PROVEN_SUBMATCH_FULL_MATCH_LIMIT
    ));
}

#[test]
fn early_max_lineage_probe_rejects_trailing_data() {
    assert!(!raw_source_benefits_from_early_max_lineage(&[0x03, 0x00], 0).unwrap());
    let error = raw_source_benefits_from_early_max_lineage(&[0x03, 0x00, 0xff], 0).unwrap_err();
    assert_eq!(error.message(), "trailing data after Deflate stream");
}

#[test]
fn early_max_lineage_covers_only_distinct_or_serial_work() {
    assert!(early_transformed_lineage_is_useful(1, 1, true));
    assert!(early_transformed_lineage_is_useful(
        2,
        PREBUILD_BOUNDED_FLOOR_MAX_DECODED,
        false,
    ));
    assert!(!early_transformed_lineage_is_useful(
        2,
        PREBUILD_BOUNDED_FLOOR_MAX_DECODED + 1,
        false,
    ));
    assert!(!early_transformed_lineage_is_useful(1, 1, false));
}

#[test]
fn complete_png_floor_reuses_the_full_default_route_sequence() {
    let input = [
        0x65, 0xc1, 0x31, 0x01, 0x00, 0x00, 0x00, 0xc2, 0xa0, 0x6c, 0xf4, 0x2f, 0xe5, 0x3f, 0x41,
        0x29, 0xa5, 0x94, 0x72, 0x06,
    ];
    let parsed = parse_stream(&input, 120).unwrap();
    let source = CandidateInput {
        compressed: &input,
        blocks: &parsed.blocks,
        meaningful_bits: parsed.meaningful_bits,
        decoded_limit: 120,
        identity: StreamIdentity {
            decoded_size: parsed.decoded_size,
            crc32: parsed.crc32,
            adler32: parsed.adler32,
        },
    };
    let options = Options {
        exhaustive: true,
        timeout: Duration::MAX,
        ..Options::default()
    };
    let deadline = Deadline::new(Instant::now(), Duration::MAX);
    let base = build_bounded_floor_candidate(source, &options, &mut SearchStop::never()).unwrap();
    let complete = build_complete_default_floor_candidate(
        source,
        &options,
        Progress::begin(
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
            None,
        ),
        DefaultFloorWork::Timed(&deadline),
    )
    .unwrap()
    .complete;
    let ordinary = optimize_raw(&input, &Options::default()).unwrap();

    assert!(
        complete.data.len() < base.data.len()
            || (complete.data.len() == base.data.len() && complete.bits <= base.bits)
    );
    assert_eq!(complete.data, ordinary.data);
    assert_eq!(complete.bits, ordinary.info.deflate_bits);
}

#[test]
#[ignore = "requires the private tests/fixtures corpus"]
fn complete_png_floor_includes_terminal_tree_methods() {
    // This stream receives a material terminal payload-tree improvement.
    // It guards the architectural boundary that previously let Default
    // apply the method only after Max had captured its comparison floor.
    let input = png_raw_deflate(&corpus_file("png/PngSuite/tbbn2c16.png"));
    let parsed = parse_stream(&input, 1 << 20).unwrap();
    let source = CandidateInput {
        compressed: &input,
        blocks: &parsed.blocks,
        meaningful_bits: parsed.meaningful_bits,
        decoded_limit: 1 << 20,
        identity: StreamIdentity {
            decoded_size: parsed.decoded_size,
            crc32: parsed.crc32,
            adler32: parsed.adler32,
        },
    };
    let options = Options {
        exhaustive: true,
        timeout: Duration::MAX,
        ..Options::default()
    };
    let deadline = Deadline::new(Instant::now(), Duration::MAX);
    let complete = build_complete_default_floor_candidate(
        source,
        &options,
        Progress::begin(
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
            None,
        ),
        DefaultFloorWork::Timed(&deadline),
    )
    .unwrap()
    .complete;
    let ordinary = optimize_raw(&input, &Options::default()).unwrap();

    assert_eq!(complete.data, ordinary.data);
    assert_eq!(complete.bits, ordinary.info.deflate_bits);
    assert!(complete.bits < parsed.meaningful_bits);
}

#[test]
fn route_errors_cancel_siblings_without_marking_a_timeout() {
    let deadline = Deadline::new(Instant::now(), Duration::MAX);
    let failed: Result<()> =
        run_route_with_cancellation(&deadline, || Err(Error::new("synthetic route failure")));

    assert!(failed.is_err());
    assert!(deadline.route_should_stop());
    assert!(!deadline.was_triggered());

    let successful = Deadline::new(Instant::now(), Duration::MAX);
    let completed: Result<()> = run_route_with_cancellation(&successful, || Ok(()));
    assert!(completed.is_ok());
    assert!(!successful.route_should_stop());
}

#[test]
fn timeout_grace_is_ten_percent_plus_one_second_and_disabled_at_zero() {
    assert_eq!(timeout_grace(Duration::ZERO), Duration::ZERO);
    assert_eq!(
        timeout_grace(Duration::from_secs(10)),
        TIMEOUT_GRACE_BASE + Duration::from_secs(10) / TIMEOUT_GRACE_DIVISOR
    );
    assert_eq!(
        timeout_grace(Duration::from_secs(4_000)),
        Duration::from_secs(401)
    );
}

#[test]
fn initial_bounded_routes_leave_one_fifth_for_follow_up_work() {
    assert_eq!(
        initial_bounded_phase_share(Duration::from_secs(100)),
        Duration::from_secs(80)
    );
    assert_eq!(
        initial_bounded_phase_share(Duration::from_secs(10)),
        Duration::from_secs(8)
    );
}

#[test]
fn soft_deadline_stops_new_routes_before_active_work() {
    let duration = Duration::from_secs(10);
    let grace = timeout_grace(duration);
    let inside_grace = Deadline::new(
        Instant::now()
            .checked_sub(duration + grace / 2)
            .expect("test duration fits in Instant"),
        duration,
    );
    assert!(!inside_grace.can_start_route());
    assert!(!inside_grace.expired());
    assert!(inside_grace.was_triggered());

    let past_hard_deadline = Deadline::new(
        Instant::now()
            .checked_sub(duration + grace + Duration::from_millis(1))
            .expect("test duration fits in Instant"),
        duration,
    );
    assert!(past_hard_deadline.expired());
    assert!(past_hard_deadline.was_triggered());
}

#[test]
fn weak_deft4j_threshold_is_strictly_below_two_percent() {
    assert!(gain_is_below(10_000, 9_801, 200));
    assert!(!gain_is_below(10_000, 9_800, 200));
    assert!(!gain_is_below(10_000, 9_000, 200));
}

#[test]
fn best_first_floor_seed_respects_the_structural_split_signal() {
    assert!(!floor_seeded_priority_with_structural_sibling(
        10_000, 9_801, true, false,
    ));
    assert!(floor_seeded_priority_with_structural_sibling(
        10_000, 9_800, true, false,
    ));
    assert!(floor_seeded_priority_with_structural_sibling(
        10_000, 9_999, false, false,
    ));
    assert!(floor_seeded_priority_with_structural_sibling(
        10_000, 9_999, true, true,
    ));
}

#[test]
fn independent_refinement_overlaps_only_inside_the_bounded_parallel_class() {
    assert!(independent_deft4j_refinement_can_overlap(
        true, true, true, true
    ));
    assert!(!independent_deft4j_refinement_can_overlap(
        false, true, true, true
    ));
    assert!(!independent_deft4j_refinement_can_overlap(
        true, false, true, true
    ));
    assert!(!independent_deft4j_refinement_can_overlap(
        true, true, false, true
    ));
    assert!(!independent_deft4j_refinement_can_overlap(
        true, true, true, false
    ));
}

#[test]
fn bounded_png_max_policy_follows_available_route_families() {
    assert_eq!(
        bounded_png_max_policy(1, false, true, true),
        BoundedPngMaxPolicy::Standard
    );
    assert_eq!(
        bounded_png_max_policy(1, true, true, true),
        BoundedPngMaxPolicy::FloorExpansion
    );
    for blocks in [2, 4, 13, 32, 33] {
        assert_eq!(
            bounded_png_max_policy(blocks, false, true, true),
            BoundedPngMaxPolicy::FloorExpansion
        );
    }

    // Generic-only streams keep source max beside the floor lineage,
    // regardless of source block count.
    for blocks in [2, 13] {
        assert_eq!(
            bounded_png_max_policy(blocks, false, false, false),
            BoundedPngMaxPolicy::GenericParallel
        );
    }
}

#[test]
fn floor_state_probe_detects_token_and_boundary_changes() {
    fn parsed(tokens: Vec<Token>, plain: Vec<u8>) -> ParsedBlock {
        ParsedBlock {
            tokens: Arc::new(tokens),
            plain: Arc::new(plain),
            literal_frequencies: [0; 286],
            distance_frequencies: [0; 30],
            original_literal_lengths: None,
            original_distance_lengths: None,
            original_dynamic: None,
            original: None,
            source_splits: Vec::new(),
            source_type: SourceBlockType::Dynamic,
        }
    }
    fn planned(tokens: Vec<Token>, plain: Vec<u8>) -> PlannedBlock {
        PlannedBlock {
            tokens: Arc::new(tokens),
            plain: Arc::new(plain),
            representation: Representation::Fixed,
            bits: 0,
            source_type: SourceBlockType::Dynamic,
        }
    }

    let source = [parsed(
        vec![Token::Literal(b'a'), Token::Literal(b'b')],
        b"ab".to_vec(),
    )];
    let same = [planned(
        vec![Token::Literal(b'a'), Token::Literal(b'b')],
        b"ab".to_vec(),
    )];
    assert!(!floor_exposes_new_search_states(&source, &same));

    let changed_tokens = [planned(
        vec![Token::Literal(b'a'), Token::Literal(b'c')],
        b"ab".to_vec(),
    )];
    assert!(floor_exposes_new_search_states(&source, &changed_tokens));

    let changed_boundaries = [
        planned(vec![Token::Literal(b'a')], b"a".to_vec()),
        planned(vec![Token::Literal(b'b')], b"b".to_vec()),
    ];
    assert!(floor_exposes_new_search_states(
        &source,
        &changed_boundaries
    ));
}

#[test]
fn compact_max_scheduling_follows_dependency_topology() {
    assert!(dense_repartition_graph_justifies_parallel_source_max(
        DENSE_REPARTITION_TOKENS_PER_RUN,
        1,
    ));
    assert!(!dense_repartition_graph_justifies_parallel_source_max(
        DENSE_REPARTITION_TOKENS_PER_RUN + 1,
        1,
    ));
    assert!(near_max_match_source_graph_is_cheap(
        NEAR_MAX_MATCH_MEAN_DECODED_PER_TOKEN
            * u64::try_from(COMPACT_SINGLE_SOURCE_ROUTE_MAX_TOKENS).unwrap(),
        COMPACT_SINGLE_SOURCE_ROUTE_MAX_TOKENS,
    ));
    assert!(!near_max_match_source_graph_is_cheap(
        NEAR_MAX_MATCH_MEAN_DECODED_PER_TOKEN
            * u64::try_from(COMPACT_SINGLE_SOURCE_ROUTE_MAX_TOKENS).unwrap()
            - 1,
        COMPACT_SINGLE_SOURCE_ROUTE_MAX_TOKENS,
    ));
    assert!(!near_max_match_source_graph_is_cheap(
        u64::MAX,
        COMPACT_SINGLE_SOURCE_ROUTE_MAX_TOKENS + 1,
    ));

    assert!(DefaultFloor::Complete.owns_terminal_stream_time());
    assert!(DefaultFloor::CompleteThenBounded.owns_terminal_stream_time());
    for shared in [
        DefaultFloor::Shared,
        DefaultFloor::SharedExact,
        DefaultFloor::ApngDefault,
        DefaultFloor::ApngMax,
        DefaultFloor::Established,
        DefaultFloor::MandatoryComplete,
    ] {
        assert!(!shared.owns_terminal_stream_time());
    }
    assert!(DefaultFloor::CompleteThenBounded.allows_parallel_source_follow_up());
    assert!(DefaultFloor::ApngMax.allows_parallel_source_follow_up());
    for serial in [
        DefaultFloor::Complete,
        DefaultFloor::Shared,
        DefaultFloor::SharedExact,
        DefaultFloor::ApngDefault,
        DefaultFloor::Established,
        DefaultFloor::MandatoryComplete,
    ] {
        assert!(!serial.allows_parallel_source_follow_up());
    }

    let block = ParsedBlock {
        tokens: Arc::new(vec![Token::Literal(0)]),
        plain: Arc::new(vec![0]),
        literal_frequencies: [0; 286],
        distance_frequencies: [0; 30],
        original_literal_lengths: None,
        original_distance_lengths: None,
        original_dynamic: None,
        original: None,
        source_splits: Vec::new(),
        source_type: SourceBlockType::Dynamic,
    };
    let compressed = [0_u8];
    let identity = StreamIdentity {
        decoded_size: 2,
        crc32: 0,
        adler32: 0,
    };
    let one_block = [block.clone()];
    let one_block_source = CandidateInput {
        compressed: &compressed,
        blocks: &one_block,
        meaningful_bits: 1,
        decoded_limit: 2,
        identity,
    };
    assert!(compact_parallel_source_max_work_class(one_block_source));
    assert!(compact_complementary_source_max_is_cheap(one_block_source));
    assert!(bounded_parallel_source_max_work_class(one_block_source));
    assert!(complete_png_parallel_source_max_work_class(
        one_block_source
    ));
    assert!(!compact_dependent_deft4j_work_class(one_block_source));

    let two_blocks = [block.clone(), block];
    let two_block_source = CandidateInput {
        compressed: &compressed,
        blocks: &two_blocks,
        meaningful_bits: 1,
        decoded_limit: 2,
        identity,
    };
    assert!(!compact_parallel_source_max_work_class(two_block_source));
    assert!(compact_complementary_source_max_is_cheap(two_block_source));
    assert!(bounded_parallel_source_max_work_class(two_block_source));
    assert!(complete_png_parallel_source_max_work_class(
        two_block_source
    ));
    assert!(compact_dependent_deft4j_work_class(two_block_source));
    assert!(!repartition_graph_covers_source_blocks(&two_blocks, 1));
    assert!(repartition_graph_covers_source_blocks(&two_blocks, 2));

    let large_token_block = ParsedBlock {
        tokens: Arc::new(vec![
            Token::Literal(0);
            COMPACT_COMPLEMENTARY_SOURCE_MAX_TOKENS + 1
        ]),
        ..two_blocks[0].clone()
    };
    let large_token_blocks = [large_token_block];
    let large_token_source = CandidateInput {
        compressed: &compressed,
        blocks: &large_token_blocks,
        meaningful_bits: 1,
        decoded_limit: 2,
        identity,
    };
    assert!(!compact_complementary_source_max_is_cheap(
        large_token_source
    ));
    let large_decoded_source = CandidateInput {
        identity: StreamIdentity {
            decoded_size: COMPACT_SPLIT_FLOOR_MAX_DECODED + 1,
            ..identity
        },
        ..large_token_source
    };
    assert!(!compact_parallel_source_max_work_class(
        large_decoded_source
    ));
    assert!(!bounded_parallel_source_max_work_class(
        large_decoded_source
    ));
    assert!(!complete_png_parallel_source_max_work_class(
        large_decoded_source
    ));

    let repartition_match = Token::Match {
        length: 130,
        distance: 1,
        length_symbol: 281,
        distance_symbol: 0,
        length_extra: 0,
        distance_extra: 0,
        length_extra_bits: 5,
        distance_extra_bits: 0,
    };
    let mut repartition_tokens = vec![Token::Literal(0); 2_049];
    repartition_tokens.extend([
        repartition_match,
        repartition_match,
        Token::Literal(0),
        repartition_match,
        repartition_match,
    ]);
    let repartition_block = ParsedBlock {
        tokens: Arc::new(repartition_tokens),
        ..large_token_blocks[0].clone()
    };
    let repartition_blocks = [repartition_block];
    let large_decoded_repartition_source = CandidateInput {
        blocks: &repartition_blocks,
        ..large_decoded_source
    };
    assert!(compact_parallel_source_max_work_class(
        large_decoded_repartition_source
    ));
    assert!(bounded_parallel_source_max_work_class(
        large_decoded_repartition_source
    ));
    assert!(!complete_png_parallel_source_max_work_class(
        large_decoded_repartition_source
    ));

    let mut dense_repartition_tokens = Vec::with_capacity(2_100);
    for _ in 0..700 {
        dense_repartition_tokens.extend([repartition_match, repartition_match, Token::Literal(0)]);
    }
    let dense_repartition_block = ParsedBlock {
        tokens: Arc::new(dense_repartition_tokens),
        ..large_token_blocks[0].clone()
    };
    let dense_repartition_blocks = [dense_repartition_block];
    let dense_repartition_source = CandidateInput {
        blocks: &dense_repartition_blocks,
        ..large_decoded_source
    };
    assert!(compact_parallel_source_max_work_class(
        dense_repartition_source
    ));
    assert!(complete_png_parallel_source_max_work_class(
        dense_repartition_source
    ));
    assert!(!compact_single_source_route_work_class(large_token_source));
    assert!(repartition_graph_covers_source_blocks(
        &large_token_blocks,
        1
    ));

    let single_route_token_block = ParsedBlock {
        tokens: Arc::new(vec![
            Token::Literal(0);
            COMPACT_SINGLE_SOURCE_ROUTE_MIN_TOKENS
        ]),
        ..large_token_blocks[0].clone()
    };
    let single_route_token_blocks = [single_route_token_block];
    let single_route_token_source = CandidateInput {
        compressed: &compressed,
        blocks: &single_route_token_blocks,
        meaningful_bits: 1,
        decoded_limit: 2,
        identity,
    };
    assert!(compact_single_source_route_work_class(
        single_route_token_source
    ));

    let oversized_token_block = ParsedBlock {
        tokens: Arc::new(vec![
            Token::Literal(0);
            COMPACT_SINGLE_SOURCE_ROUTE_MAX_TOKENS + 1
        ]),
        ..large_token_blocks[0].clone()
    };
    let oversized_token_blocks = [oversized_token_block];
    let oversized_token_source = CandidateInput {
        compressed: &compressed,
        blocks: &oversized_token_blocks,
        meaningful_bits: 1,
        decoded_limit: 2,
        identity,
    };
    assert!(!compact_single_source_route_work_class(
        oversized_token_source
    ));
    assert!(!repartition_graph_covers_source_blocks(
        &oversized_token_blocks,
        1
    ));
}

#[test]
fn replay_validation_is_enforced_in_release_builds() {
    let parsed = parse_stream(&[0x03, 0x00], 1).unwrap();
    let empty = StreamIdentity {
        decoded_size: 0,
        crc32: 0,
        adler32: 1,
    };
    assert!(validate_replayed_stream(&parsed, 2, empty).is_ok());
    assert!(parse_validated_rewrite(&[0x03, 0x00], 1, empty).is_ok());

    // A complete stream followed by another byte is not a valid rewrite
    // of the exact candidate byte range.
    assert_eq!(
        parse_validated_rewrite(&[0x03, 0x00, 0x00], 1, empty)
            .unwrap_err()
            .message(),
        "internal error: rewritten Deflate stream changed decoded data"
    );

    let changed = StreamIdentity {
        decoded_size: 1,
        ..empty
    };
    assert_eq!(
        validate_replayed_stream(&parsed, 2, changed)
            .unwrap_err()
            .message(),
        "internal error: rewritten Deflate stream changed decoded data"
    );
}

#[test]
fn alphabet_boundaries_preserve_history_and_reprice_later_stored_padding() {
    let mut plain: Vec<_> = (0..32).map(|i| (i % 2) as u8).collect();
    plain.extend((0..256).map(|i| 160 + (i % 16) as u8));
    plain.extend((0..35).map(|i| (i % 2) as u8));
    let tokens: Vec<_> = plain.iter().copied().map(Token::Literal).collect();
    let (literal_frequencies, distance_frequencies) =
        super::super::model::count_frequencies(&tokens);
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
        super::super::block::plan_block(&block, 0, &Options::default(), &mut SearchStop::never());
    // A trailing match refers back across the source block boundary. New
    // boundaries must leave that history intact.
    let tail = PlannedBlock {
        tokens: vec![Token::Match {
            length: 3,
            distance: 1,
            length_symbol: 257,
            distance_symbol: 0,
            length_extra: 0,
            distance_extra: 0,
            length_extra_bits: 0,
            distance_extra_bits: 0,
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
            let check = parse_validated_rewrite(&result.data, 1024, identity).unwrap();
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
