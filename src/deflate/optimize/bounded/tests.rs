// SPDX-License-Identifier: MIT
// SPDX-FileCopyrightText: 2026 Andrew C.E. Dent <https://github.com/ace-dent>

use std::time::{Duration, Instant};

use super::super::optimize_raw;
use super::super::test_support::deadline_with_grace;
use super::*;
use crate::deflate::bitstream::BitWriter;
use crate::deflate::huffman::fixed_trees;
use crate::deflate::parse::parse_stream;
use crate::progress::StreamProgress;

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
    let deadline = deadline_with_grace(Instant::now(), Duration::ZERO);
    let candidates = build_bounded_phase_candidates(
        source,
        &options,
        BoundedRoutes {
            preserve_complete_default: true,
            parallel: true,
            ..BoundedRoutes::default()
        },
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
        ),
        None,
    )
    .unwrap();
    let floor = candidates.floor.unwrap();
    let ordinary = optimize_raw(&input, &Options::default()).unwrap();

    assert!(floor.data.len() <= ordinary.data.len());
    assert!(floor.bits <= ordinary.info.deflate_bits);
}
