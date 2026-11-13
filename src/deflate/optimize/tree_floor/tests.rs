// SPDX-License-Identifier: MIT
// SPDX-FileCopyrightText: 2026 Andrew C.E. Dent <https://github.com/ace-dent>

use super::super::emit_plans;
use super::super::test_support::{corpus_file, png_raw_deflate};
use super::*;
use crate::deflate::parse::parse_stream;

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
        replaced_parents: Vec::new(),
    };

    let refined =
        refine_with_compact_payload_tree_floor(&candidate, &Options::default(), 1 << 20, identity)
            .unwrap()
            .1
            .unwrap();

    assert!(refined.bits < candidate.bits);
    let reparsed = parse_validated_rewrite(&refined.data, 1 << 20, identity, &[]).unwrap();
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
        replaced_parents: Vec::new(),
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
