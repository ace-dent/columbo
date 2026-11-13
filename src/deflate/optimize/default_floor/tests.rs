// SPDX-License-Identifier: MIT
// SPDX-FileCopyrightText: 2026 Andrew C.E. Dent <https://github.com/ace-dent>

use super::super::test_support::{corpus_file, deadline_with_grace, png_raw_deflate};
use super::super::{optimize_raw, parse_validated_rewrite, StreamIdentity};
use super::*;
use crate::deflate::bitstream::BitWriter;
use crate::deflate::block::emit_block;
use crate::deflate::header::test_support::payload_tradeoff_test_block;
use crate::deflate::model::{PlannedBlock, Representation, SourceBlockType};
use crate::deflate::parse::parse_stream;
use crate::progress::StreamProgress;

#[test]
fn strict_completion_competes_only_after_max_searches_its_parent() {
    // A literal-only block whose planned `[1, 1]` distance completion needs
    // 18 code-length codes, so R1b wins on Default's endpoint.
    let block = payload_tradeoff_test_block();
    let dynamic = block.original_dynamic.as_ref().unwrap();
    let mut writer = BitWriter::default();
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
    let input = writer.into_bytes();
    let parsed = parse_stream(&input, 1024).unwrap();
    let source = CandidateInput {
        compressed: &input,
        blocks: &parsed.blocks,
        meaningful_bits: parsed.meaningful_bits,
        decoded_limit: 1024,
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
    let deadline = deadline_with_grace(Instant::now(), Duration::MAX);
    let floors = build_complete_default_floor_candidate(
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
        ),
        DefaultFloorWork::Timed(&deadline),
    )
    .unwrap();
    let ordinary = optimize_raw(&input, &Options::default()).unwrap();

    // Default's exact endpoint survives as the final competitor.
    let competitor = floors.slid.expect("R1b wins on this block");
    assert_eq!(competitor.data, ordinary.data);
    assert_eq!(competitor.bits, ordinary.info.deflate_bits);
    let completed = parse_validated_rewrite(&competitor.data, 1024, source.identity, &[]).unwrap();
    assert_ne!(
        &completed.blocks[0]
            .original_dynamic
            .as_ref()
            .unwrap()
            .distance_lengths[..2],
        &[1, 1]
    );
    // Max's parent is searched without it and keeps the planned completion.
    let parent =
        parse_validated_rewrite(&floors.complete.data, 1024, source.identity, &[]).unwrap();
    assert_eq!(
        &parent.blocks[0]
            .original_dynamic
            .as_ref()
            .unwrap()
            .distance_lengths[..2],
        &[1, 1]
    );
    // Max's own terminal sweep and the competitor keep Max at or below Default.
    let max = optimize_raw(&input, &options).unwrap();
    assert!(
        max.data.len() < ordinary.data.len()
            || (max.data.len() == ordinary.data.len()
                && max.info.deflate_bits <= ordinary.info.deflate_bits)
    );
}

#[test]
#[ignore = "requires the private tests/fixtures corpus"]
fn complete_png_floor_includes_terminal_tree_methods() {
    // This stream gains from a terminal payload-tree method. Max's complete
    // Default floor must include that gain, exactly as Default does.
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
    let deadline = deadline_with_grace(Instant::now(), Duration::MAX);
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
