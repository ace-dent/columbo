// SPDX-License-Identifier: MIT
// SPDX-FileCopyrightText: 2026 Andrew C.E. Dent <https://github.com/ace-dent>

use std::sync::Arc;
use std::time::Duration;

use super::super::stop::{initial_bounded_phase_share, TIMEOUT_GRACE_BASE, TIMEOUT_GRACE_DIVISOR};
use super::compact_split::{
    compact_source_split_floor_eligible_with_limits, compact_split_preserves_source_blocks,
    TERMINAL_SOURCE_SPLIT_MAX_DECODED,
};
use super::default_floor::slid_default_endpoint;
use super::gates::compact_strict_literal_tree_eligible;
use super::terminal::TerminalHeaderSearch;
use super::test_support::{
    comparison_candidate, corpus_file, deadline_with_grace, png_raw_deflate,
    refine_with_terminal_header_search,
};
use super::tree_floor::{
    refine_with_bounded_depth_tree_floor, refine_with_compact_payload_tree_floor,
};
use super::*;
use crate::deflate::block::plan_block;
use crate::deflate::header::test_support::{header_tree_test_block, rotation_test_block};
use crate::deflate::header::{plan_bounded_depth_tree_candidate, plan_rle_smoothed_tree_candidate};
use crate::deflate::huffman::{fixed_trees, huffman_tree_shape_is_complete};
use crate::deflate::model::count_frequencies;
use crate::progress::{Progress, StreamProgress};

#[test]
fn terminal_reservation_requires_max_owned_time_and_the_full_terminal_work_class() {
    let mut options = Options {
        exhaustive: true,
        timeout: Duration::from_secs(10),
        ..Options::default()
    };
    for floor in [
        DefaultFloor::Complete,
        DefaultFloor::CompleteThenBounded,
        DefaultFloor::Shared,
        DefaultFloor::SharedExact,
        DefaultFloor::ApngDefault,
        DefaultFloor::ApngMax,
        DefaultFloor::Established,
        DefaultFloor::MandatoryComplete,
    ] {
        let owner = matches!(
            floor,
            DefaultFloor::Complete | DefaultFloor::CompleteThenBounded | DefaultFloor::ApngMax
        );
        assert_eq!(
            floor.terminal_share(&options, 1024, 4096, 2),
            if owner {
                TerminalShare::Search
            } else {
                TerminalShare::None
            }
        );
    }
    let share = |options: &Options, compressed, decoded, blocks| {
        DefaultFloor::Complete.terminal_share(options, compressed, decoded, blocks)
    };
    let bytes = MAX_TERMINAL_HEADER_MAX_BYTES;
    let decoded = bytes as u64;
    let blocks = TERMINAL_HEADER_MAX_BLOCKS;
    assert_eq!(
        share(&options, bytes, decoded, blocks),
        TerminalShare::Search
    );
    // Outside the header size class, only linear finalization keeps a share.
    for (compressed, decoded, blocks) in [
        (bytes + 1, decoded, blocks),
        (bytes, decoded + 1, blocks),
        (bytes, decoded, blocks + 1),
    ] {
        assert_eq!(
            share(&options, compressed, decoded, blocks),
            TerminalShare::Finalization
        );
    }
    // An established continuation runs in its own window, but reserves only
    // the finalization share, and only outside the header size class.
    let established = |compressed, decoded| {
        DefaultFloor::Established.terminal_share(&options, compressed, decoded, 2)
    };
    assert_eq!(established(1024, 4096), TerminalShare::None);
    assert_eq!(established(bytes + 1, 4096), TerminalShare::Finalization);
    options.exhaustive = false;
    assert_eq!(share(&options, 1024, 4096, 2), TerminalShare::None);
    options.exhaustive = true;
    options.timeout = Duration::ZERO;
    assert_eq!(share(&options, 1024, 4096, 2), TerminalShare::None);
}

#[test]
fn terminal_phase_retains_the_original_clock_and_does_not_multiply_grace() {
    let started = Instant::now() - Duration::from_secs(90);
    let allowance = Duration::from_secs(100);
    let terminal = deadline_with_grace(started, allowance);
    let primary = Deadline::with_grace(
        started,
        initial_bounded_phase_share(allowance),
        Duration::ZERO,
    );
    assert!(primary.hard_stop().reached());
    assert!(!primary.hard_stop().permits_bounded_finalization());
    assert!(terminal.can_start_route());
    assert!(!terminal.was_triggered());
    assert_eq!(terminal.started, primary.started);
    assert_eq!(terminal.duration, allowance);
    assert_eq!(terminal.grace, timeout_grace(allowance));
    assert_eq!(initial_bounded_phase_share(Duration::ZERO), Duration::ZERO);
}

fn compact_source_split_floor_eligible(decoded_size: u64, blocks: &[ParsedBlock]) -> bool {
    compact_source_split_floor_eligible_with_limits(
        decoded_size,
        blocks,
        2,
        COMPACT_SPLIT_FLOOR_MAX_DECODED,
    )
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

const FEEDBACK_RAW: &[u8] = &[
    0x25, 0xc0, 0x01, 0x01, 0xc0, 0x30, 0x0c, 0xc3, 0x30, 0x6c, 0xb5, 0x9b, 0xf0, 0x87, 0xf4, 0x7d,
    0xd3, 0xcc, 0xcc, 0xcc, 0xcc, 0x01, 0x00, 0x00, 0xc0, 0x71, 0x5d, 0xaa, 0xaa, 0xaa, 0xfe, 0x76,
    0x77, 0x93, 0x24, 0x49, 0x9e, 0xa7, 0x6d, 0xdb, 0xf6, 0x03,
];

#[test]
fn a_kept_validated_parse_is_the_parse_it_replaces() {
    let fresh = parse_stream(FEEDBACK_RAW, 1 << 20).unwrap();
    let identity = StreamIdentity {
        decoded_size: fresh.decoded_size,
        crc32: fresh.crc32,
        adler32: fresh.adler32,
    };
    let same = |left: &ParsedStream, right: &ParsedStream| {
        let scalars = |stream: &ParsedStream| {
            (
                stream.consumed,
                stream.meaningful_bits,
                stream.crc32,
                stream.adler32,
                stream.decoded_size,
                stream.max_distance,
            )
        };
        assert_eq!(scalars(left), scalars(right));
        assert_eq!(left.source_block_count, right.source_block_count);
        assert_eq!(
            left.source_empty_block_count,
            right.source_empty_block_count
        );
        assert_eq!(left.blocks.len(), right.blocks.len());
        for (left, right) in left.blocks.iter().zip(&right.blocks) {
            assert_eq!(left.tokens, right.tokens);
            assert_eq!(left.plain, right.plain);
            assert_eq!(left.literal_frequencies, right.literal_frequencies);
            assert_eq!(left.distance_frequencies, right.distance_frequencies);
            assert_eq!(left.original, right.original);
            assert_eq!(left.original_dynamic, right.original_dynamic);
            assert_eq!(left.source_type, right.source_type);
        }
    };

    release_kept_validated_parse();
    let first = parse_validated_rewrite(FEEDBACK_RAW, 1 << 20, identity, &[]).unwrap();
    // The second parse is the kept one, sharing the first's buffers.
    let kept = parse_validated_rewrite(FEEDBACK_RAW, 1 << 20, identity, &[]).unwrap();
    same(&first, &fresh);
    same(&kept, &fresh);
    assert!(Arc::ptr_eq(&first.blocks[0].tokens, &kept.blocks[0].tokens));

    // A hit still validates against the caller's identity, and another
    // decoded limit parses again, failing as a fresh parse does.
    let changed = StreamIdentity {
        crc32: identity.crc32 ^ 1,
        ..identity
    };
    assert!(parse_validated_rewrite(FEEDBACK_RAW, 1 << 20, changed, &[]).is_err());
    let short = fresh.decoded_size - 1;
    assert_eq!(
        parse_validated_rewrite(FEEDBACK_RAW, short, identity, &[])
            .unwrap_err()
            .message(),
        parse_stream(FEEDBACK_RAW, short).unwrap_err().message()
    );
    release_kept_validated_parse();
    let after = parse_validated_rewrite(FEEDBACK_RAW, 1 << 20, identity, &[]).unwrap();
    same(&after, &fresh);
    assert!(!Arc::ptr_eq(
        &after.blocks[0].tokens,
        &first.blocks[0].tokens
    ));
    release_kept_validated_parse();
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

/// A literal-only block whose 40 symbols each occur 20–22 times. Raw-count
/// trees mix two code lengths irregularly; both smoothers equalize the counts,
/// which makes the code-length header cheaper than the payload it costs.
fn near_uniform_literal_tokens() -> Vec<Token> {
    let mut state = 0_u64;
    let mut tokens = Vec::new();
    for symbol in 0..40_u8 {
        state = state
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        let count = 20 + ((state >> 33) as u32) % 3;
        tokens.extend(std::iter::repeat(Token::Literal(symbol)).take(count as usize));
    }
    tokens
}

#[test]
fn smoothed_tree_finish_follows_the_terminal_searches() {
    let tokens = near_uniform_literal_tokens();
    let plain: Vec<u8> = tokens
        .iter()
        .map(|token| match token {
            Token::Literal(value) => *value,
            Token::Match { .. } => unreachable!(),
        })
        .collect();
    let (literal_frequencies, distance_frequencies) = count_frequencies(&tokens);
    let block = ParsedBlock {
        tokens: tokens.clone().into(),
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
    let ordinary = plan_block(&block, 0, &Options::default(), &mut SearchStop::never());
    let smoothed = plan_rle_smoothed_tree_candidate(
        &tokens,
        &literal_frequencies,
        &distance_frequencies,
        true,
    )
    .unwrap();
    let frontier = plan_bounded_depth_tree_candidate(
        &tokens,
        &literal_frequencies,
        &distance_frequencies,
        true,
        false,
        false,
        &mut SearchStop::never(),
    )
    .unwrap();
    assert!(smoothed.bits < ordinary.bits.min(frontier.bits));

    // Nine blocks exceed the compact payload-tree floor's eight-block limit,
    // so only R14 prices the smoothed trees for this stream.
    let saving = 9 * (ordinary.bits - smoothed.bits);
    let (data, bits) = emit_plans(&[], &vec![ordinary; 9], true).unwrap();
    let parsed = parse_stream(&data, 1 << 20).unwrap();
    let identity = StreamIdentity {
        decoded_size: parsed.decoded_size,
        crc32: parsed.crc32,
        adler32: parsed.adler32,
    };
    let parent = Candidate {
        data: data.clone(),
        bits,
        output_max_distance: Some(parsed.max_distance),
        plans: Vec::new(),
        block_report: None,
        route: "test",
        max_planner_is_stable: false,
        replaced_parents: Vec::new(),
    };
    let (covered, compact) =
        refine_with_compact_payload_tree_floor(&parent, &Options::default(), 1 << 20, identity)
            .unwrap();
    assert!(!covered);
    assert!(compact.is_none());
    // R15 may then split blocks of a whole sweep.
    let smoothed_blocks = |candidate: &Candidate, strict: bool, cut: bool| {
        let reparsed = parse_validated_rewrite(&candidate.data, 1 << 20, identity, &[]).unwrap();
        assert_eq!(candidate.bits, reparsed.meaningful_bits);
        if cut {
            assert!(reparsed.blocks.len() >= 9);
        } else {
            assert_eq!(reparsed.blocks.len(), 9);
        }
        for block in &reparsed.blocks {
            assert!(cut || block.original.unwrap().len <= smoothed.bits);
            assert!(
                !strict
                    || block
                        .original_dynamic
                        .as_ref()
                        .unwrap()
                        .has_strictly_compatible_huffman_codes()
            );
        }
    };

    for strict in [false, true] {
        let options = Options {
            strict,
            ..Options::default()
        };
        let refined = refine_with_terminal_header_search(
            TerminalHeaderSearch::SmoothedTrees,
            &parent,
            &options,
            1 << 20,
            identity,
            &mut SearchStop::never(),
        )
        .unwrap()
        .unwrap();
        assert!(refined.bits <= parent.bits - saving);
        smoothed_blocks(&refined, strict, false);
        assert!(refine_with_terminal_header_search(
            TerminalHeaderSearch::SmoothedTrees,
            &parent,
            &options,
            1 << 20,
            identity,
            &mut SearchStop::always(),
        )
        .unwrap()
        .is_none());
    }

    // Default's single sweep and Max's Default endpoint both end with it.
    let source = CandidateInput {
        compressed: &data,
        blocks: &parsed.blocks,
        meaningful_bits: parsed.meaningful_bits,
        decoded_limit: 1 << 20,
        identity,
    };
    let options = Options::default();
    let deadline = deadline_with_grace(Instant::now(), Duration::MAX);
    let progress = Progress::begin(
        &options,
        deadline.started,
        StreamProgress {
            blocks: parsed.source_block_count,
            compressed_bytes: data.len(),
            decoded_bytes: parsed.decoded_size,
            empty_blocks: parsed.source_empty_block_count,
            meaningful_bits: parsed.meaningful_bits,
            parse_elapsed: Duration::ZERO,
        },
    );
    let ordinary_sweep = improve_with_terminal_searches(
        source,
        &options,
        DefaultFloorWork::Mandatory,
        DefaultFloorWork::Timed(&deadline),
        progress,
        HeaderFit::InSweep,
        false,
        parent.clone(),
    )
    .unwrap();
    assert!(ordinary_sweep.bits <= parent.bits - saving);
    smoothed_blocks(&ordinary_sweep, true, true);
    // On this stream the repeated finish ends where none of R13–R15
    // improves it.
    for search in [
        TerminalHeaderSearch::DistanceBands,
        TerminalHeaderSearch::SmoothedTrees,
        TerminalHeaderSearch::EntropyCuts,
    ] {
        assert!(refine_with_terminal_header_search(
            search,
            &ordinary_sweep,
            &options,
            1 << 20,
            identity,
            &mut SearchStop::never(),
        )
        .unwrap()
        .is_none());
    }
    let slid = slid_default_endpoint(
        source,
        &options,
        progress,
        &mut TerminalParseCache::default(),
        &parent,
        HeaderFit::InSweep,
    )
    .unwrap()
    .unwrap();
    assert!(slid.bits <= parent.bits - saving);
    smoothed_blocks(&slid, true, true);
}

#[test]
fn terminal_merges_follow_the_settled_finish() {
    // Two dynamic blocks with the same statistics: R1c can move their
    // boundary but not remove it, and R15 finds nothing to cut, so only R16
    // joins them.
    let plain: Vec<u8> = (0..300).map(|i| (i % 8) as u8).collect();
    let tokens: Vec<_> = plain.iter().copied().map(Token::Literal).collect();
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
    let half = plan_block(&block, 0, &Options::default(), &mut SearchStop::never());
    let (data, bits) = emit_plans(&[], &[half.clone(), half], true).unwrap();
    let parsed = parse_stream(&data, 1 << 20).unwrap();
    assert_eq!(parsed.blocks.len(), 2);
    let identity = StreamIdentity {
        decoded_size: parsed.decoded_size,
        crc32: parsed.crc32,
        adler32: parsed.adler32,
    };
    let parent = Candidate {
        data: data.clone(),
        bits,
        output_max_distance: Some(parsed.max_distance),
        plans: Vec::new(),
        block_report: None,
        route: "test",
        max_planner_is_stable: false,
        replaced_parents: Vec::new(),
    };
    let merged_blocks = |candidate: &Candidate| {
        let reparsed = parse_validated_rewrite(&candidate.data, 1 << 20, identity, &[]).unwrap();
        assert_eq!(candidate.bits, reparsed.meaningful_bits);
        assert!(candidate.bits < parent.bits);
        reparsed.blocks.len()
    };
    let options = Options::default();
    let merged = refine_with_terminal_header_search(
        TerminalHeaderSearch::Merges,
        &parent,
        &options,
        1 << 20,
        identity,
        &mut SearchStop::never(),
    )
    .unwrap()
    .unwrap();
    assert_eq!(merged_blocks(&merged), 1);

    let source = CandidateInput {
        compressed: &data,
        blocks: &parsed.blocks,
        meaningful_bits: parsed.meaningful_bits,
        decoded_limit: 1 << 20,
        identity,
    };
    let deadline = deadline_with_grace(Instant::now(), Duration::MAX);
    let progress = Progress::begin(
        &options,
        deadline.started,
        StreamProgress {
            blocks: parsed.source_block_count,
            compressed_bytes: data.len(),
            decoded_bytes: parsed.decoded_size,
            empty_blocks: parsed.source_empty_block_count,
            meaningful_bits: parsed.meaningful_bits,
            parse_elapsed: Duration::ZERO,
        },
    );
    let ordinary_sweep = improve_with_terminal_searches(
        source,
        &options,
        DefaultFloorWork::Mandatory,
        DefaultFloorWork::Timed(&deadline),
        progress,
        HeaderFit::InSweep,
        false,
        parent.clone(),
    )
    .unwrap();
    assert_eq!(merged_blocks(&ordinary_sweep), 1);
    // Max's Default endpoint ends the same way.
    let slid = slid_default_endpoint(
        source,
        &options,
        progress,
        &mut TerminalParseCache::default(),
        &parent,
        HeaderFit::InSweep,
    )
    .unwrap()
    .unwrap();
    assert_eq!(merged_blocks(&slid), 1);
    // R18 ends the finish at the bounded-depth tree floor's fixed point, so
    // the merged block has no strictly smaller depth-limited tree left.
    assert!(refine_with_bounded_depth_tree_floor(
        &slid,
        &options,
        1 << 20,
        identity,
        &mut SearchStop::never(),
    )
    .unwrap()
    .is_none());
    // With `ApngDefault`'s order the endpoint still merges, and R2–R5 run
    // last, so R4 has nothing left to fit.
    let apng_slid = slid_default_endpoint(
        source,
        &options,
        progress,
        &mut TerminalParseCache::default(),
        &parent,
        HeaderFit::AfterTreeFloors,
    )
    .unwrap()
    .unwrap();
    assert_eq!(merged_blocks(&apng_slid), 1);
    assert!(refine_with_terminal_header_search(
        TerminalHeaderSearch::JointTreeRle,
        &apng_slid,
        &options,
        1 << 20,
        identity,
        &mut SearchStop::never(),
    )
    .unwrap()
    .is_none());
}

#[test]
fn one_pass_closes_repeated_deflopt_defluff_feedback() {
    // A synthetic dynamic block that DeflOpt and Defluff need two passes for
    // (331 -> 326 -> 325 meaningful bits): DeflOpt expands one length-3 match
    // into literals, then Defluff swaps the five- and six-bit codes of the
    // equal-frequency length symbols 259 and 260, shrinking the dynamic header
    // from 130 to 129 bits while the payload stays 196 bits. Columbo must
    // close this in one invocation; a second must find nothing.
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

    // A zero-budget Max run must still finish and keep the mandatory Default
    // floor.
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

    // With enough time, Max also closes the graph in one invocation and is
    // itself at a fixed point.
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
fn shared_exact_max_returns_the_exact_default_result() {
    // A stored block Default rewrites, and a stream with feedback wins.
    let stored = [0x01, 0x05, 0x00, 0xfa, 0xff, b'h', b'e', b'l', b'l', b'o'];
    for (input, size) in [(FEEDBACK_RAW, 86), (stored.as_slice(), 5)] {
        for strict in [true, false] {
            for strip_metadata in [true, false] {
                let default_options = Options {
                    strict,
                    strip_metadata,
                    timeout: Duration::from_secs(10),
                    ..Options::default()
                };
                let default = optimize_raw_prefix_with_floor(
                    input,
                    &default_options,
                    size,
                    DefaultFloor::Shared,
                )
                .unwrap();
                assert!(!default.timed_out);
                assert!(default.default_endpoint.is_none());
                // The endpoint is mandatory work, so even a zero Max budget
                // returns it.
                let max_options = Options {
                    exhaustive: true,
                    timeout: Duration::ZERO,
                    ..default_options
                };
                let endpoint = optimize_raw_prefix_with_floor(
                    input,
                    &max_options,
                    size,
                    DefaultFloor::SharedExact,
                )
                .unwrap()
                .default_endpoint
                .expect("SharedExact returns Default's endpoint");
                assert_eq!(endpoint.data, default.data, "strict={strict}");
                assert_eq!(endpoint.deflate_bits, default.info.deflate_bits);
                assert!(optimize_raw_prefix_with_floor(
                    input,
                    &max_options,
                    size,
                    DefaultFloor::Complete,
                )
                .unwrap()
                .default_endpoint
                .is_none());
            }
        }
    }
}

/// A short appended log line pair, as a GZIP member (synthetic): Default's
/// finish alone, after a deadline during its first route, ends smaller than
/// its complete route.
const FIRST_ROUTE_CUT_RAW: &[u8] = &[
    0x33, 0x32, 0x30, 0x32, 0xd3, 0x35, 0x34, 0xd4, 0x35, 0x32, 0x51, 0x30, 0x32, 0xb6, 0x32, 0x36,
    0x54, 0xc8, 0x2c, 0xb0, 0x35, 0xb4, 0x34, 0xd2, 0x33, 0x34, 0xb3, 0xd0, 0x33, 0xd4, 0x33, 0x57,
    0x30, 0x32, 0x30, 0x50, 0x30, 0x31, 0x30, 0x51, 0x48, 0x4e, 0x4c, 0xce, 0x48, 0xb5, 0xcd, 0xcd,
    0x2c, 0x2e, 0x56, 0xc8, 0x49, 0x2c, 0x49, 0xcd, 0x4b, 0xae, 0xb4, 0x35, 0x32, 0x35, 0xc8, 0x2d,
    0xe6, 0x32, 0x42, 0x18, 0x60, 0x68, 0x66, 0x65, 0x62, 0xa6, 0x10, 0xe0, 0x1f, 0x1c, 0xa2, 0x60,
    0x0c, 0xd4, 0x02, 0xd2, 0x8a, 0x66, 0x1a, 0x88, 0x6b, 0xa0, 0x07, 0x82, 0x86, 0x5c, 0x00,
];

#[test]
fn max_keeps_defaults_first_route_cut_endpoint() {
    let size = 122;
    for strict in [true, false] {
        let complete_options = Options {
            strict,
            timeout: Duration::from_secs(10),
            ..Options::default()
        };
        let complete = optimize_raw_prefix_with_floor(
            FIRST_ROUTE_CUT_RAW,
            &complete_options,
            size,
            DefaultFloor::Shared,
        )
        .unwrap();
        assert!(!complete.timed_out);
        // The soft deadline passes during the first route and the hard stop
        // is far off, as when a container's time slice is shorter than that
        // route.
        let cut_options = Options {
            timeout: Duration::from_micros(1),
            ..complete_options
        };
        let cut = optimize_raw_prefix_with_floor_and_grace(
            FIRST_ROUTE_CUT_RAW,
            &cut_options,
            size,
            DefaultFloor::Shared,
            Duration::from_secs(3600),
        )
        .unwrap();
        assert!(
            is_strictly_better(
                cut.data.len(),
                cut.info.deflate_bits,
                complete.data.len(),
                complete.info.deflate_bits,
            ),
            "strict={strict}"
        );
        let max_options = Options {
            exhaustive: true,
            ..cut_options.clone()
        };
        for floor in [DefaultFloor::Complete, DefaultFloor::SharedExact] {
            let maximum =
                optimize_raw_prefix_with_floor(FIRST_ROUTE_CUT_RAW, &max_options, size, floor)
                    .unwrap();
            assert!(
                !is_strictly_better(
                    cut.data.len(),
                    cut.info.deflate_bits,
                    maximum.data.len(),
                    maximum.info.deflate_bits,
                ),
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
    // Relaxed mode permits the source exceptions but may still select a
    // cheaper canonical fixed or dynamic representation.
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
    // repetitive payload would be easy to recompress with new matches,
    // Columbo may only choose a cheaper serialization of the existing parse.
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
fn source_max_replays_keep_each_replaced_parent() {
    // Seeded with the source's own tokens under fixed codes, the first Max
    // replay finds dynamic codes, so it is accepted and the seed becomes the
    // oldest replaced parent. Strict output replays even a losing seed.
    let parsed = parse_stream(FEEDBACK_RAW, 86).unwrap();
    let source = CandidateInput {
        compressed: FEEDBACK_RAW,
        blocks: &parsed.blocks,
        meaningful_bits: parsed.meaningful_bits,
        decoded_limit: 86,
        identity: StreamIdentity {
            decoded_size: parsed.decoded_size,
            crc32: parsed.crc32,
            adler32: parsed.adler32,
        },
    };
    let fixed_seed = || {
        parsed
            .blocks
            .iter()
            .map(|block| PlannedBlock {
                tokens: block.tokens.clone(),
                plain: block.plain.clone(),
                representation: Representation::Fixed,
                bits: crate::deflate::block::fixed_block_bits(&block.tokens).unwrap(),
                source_type: block.source_type,
            })
            .collect::<Vec<_>>()
    };
    let options = Options {
        strict: true,
        exhaustive: true,
        ..Options::default()
    };
    for keep in [false, true] {
        let (candidate, _) = build_candidate_and_parse(
            source,
            fixed_seed(),
            &options,
            MAX_RAW_REPLAY_LIMIT,
            ReplayPlanner::Full,
            &mut SearchStop::never(),
            None,
            keep,
        )
        .unwrap();
        if !keep {
            assert!(candidate.replaced_parents.is_empty());
            continue;
        }
        let parents = &candidate.replaced_parents;
        let (seed_data, seed_bits) = emit_plans(FEEDBACK_RAW, &fixed_seed(), true).unwrap();
        assert_eq!(parents[0].data, seed_data);
        assert_eq!(parents[0].bits, seed_bits);
        for pair in parents.windows(2) {
            assert!(pair[1].is_strictly_smaller_than(&pair[0]));
        }
        assert!(candidate.is_strictly_smaller_than(parents.last().unwrap()));
        for parent in parents {
            let reparsed = parse_validated_rewrite(&parent.data, 86, source.identity, &[]).unwrap();
            assert_eq!(parent.bits, reparsed.meaningful_bits);
            assert_eq!(parent.output_max_distance, Some(reparsed.max_distance));
        }
    }
}

#[test]
fn max_replay_ceiling_covers_every_strict_stream_score() {
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
fn established_floor_is_rebuilt_when_strict_mode_must_repair_the_source() {
    // The input of `strict_mode_repairs_alias_and_single_distance_code_inputs`.
    // Every strict rewrite costs bits, so no route could strictly beat a
    // copied floor, and strict output would return the source unchanged.
    let input = [
        0xe5, 0xc0, 0x81, 0x00, 0x00, 0x00, 0x00, 0x80, 0x20, 0xb6, 0xfd, 0xa5, 0x06, 0xa9, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0xbe, 0x01,
    ];
    for timeout in [Duration::ZERO, Duration::from_secs(10)] {
        let options = Options {
            exhaustive: true,
            timeout,
            ..Options::default()
        };
        let maximum = optimize_raw_prefix_with_floor(
            &input,
            &options,
            options.max_decoded_bytes,
            DefaultFloor::Established,
        )
        .unwrap();
        let stream = parse_stream(&maximum.data, 2_302).unwrap();
        assert!(!stream_uses_258_alias(&stream));
        assert!(stream.blocks.iter().all(|block| {
            block.original_dynamic.as_ref().map_or(true, |dynamic| {
                dynamic.has_strictly_compatible_huffman_codes()
            })
        }));
    }

    // Relaxed mode still takes the source as its floor.
    let relaxed_options = Options {
        exhaustive: true,
        timeout: Duration::ZERO,
        strict: false,
        ..Options::default()
    };
    let relaxed = optimize_raw_prefix_with_floor(
        &input,
        &relaxed_options,
        relaxed_options.max_decoded_bytes,
        DefaultFloor::Established,
    )
    .unwrap();
    assert_eq!(relaxed.data, input);
}

#[test]
fn bounded_parallel_floors_preserve_a_complete_stream() {
    // Two nonempty Huffman blocks activate both phase-one routes, which
    // rejoin before any deft4j refinement.
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
fn bounded_generic_routes_preserve_complete_candidates() {
    // Empty stored blocks have no useful deft4j or no-split route, so this
    // small stream runs only the floor and original-source max routes.
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
fn early_max_lineage_probe_reports_the_consumed_prefix() {
    assert_eq!(
        inspect_early_max_lineage(&[0x03, 0x00], 0).unwrap(),
        (2, false)
    );
    // Bytes after the stream belong to the caller's wrapper, such as the
    // zlib Adler-32 trailer.
    assert_eq!(
        inspect_early_max_lineage(&[0x03, 0x00, 0xff], 0).unwrap(),
        (2, false)
    );
    // BTYPE 3 is reserved, so malformed input still fails the single parse.
    assert!(inspect_early_max_lineage(&[0x07], 0).is_err());
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
    let deadline = deadline_with_grace(Instant::now(), Duration::MAX);
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

/// One dynamic block of `decoded` bytes, 90% zeros and the rest 1–3, greedily
/// matched against the latest earlier position with the same three bytes.
fn skewed_greedy_block(mut state: u64, decoded: usize) -> ParsedBlock {
    use crate::deflate::distance_band::test_support::{matched, token_block};
    let data: Vec<u8> = (0..decoded)
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            match state % 100 {
                0..=89 => 0,
                90..=95 => 1,
                96..=98 => 2,
                _ => 3,
            }
        })
        .collect();
    let key = |index: usize| {
        (usize::from(data[index]) << 4)
            | (usize::from(data[index + 1]) << 2)
            | usize::from(data[index + 2])
    };
    let mut latest = [usize::MAX; 64];
    let mut tokens = Vec::new();
    let mut index = 0;
    while index < data.len() {
        let candidate = if index + 3 <= data.len() {
            latest[key(index)]
        } else {
            usize::MAX
        };
        let length = if candidate == usize::MAX {
            0
        } else {
            (0..(data.len() - index).min(258))
                .take_while(|&offset| data[candidate + offset] == data[index + offset])
                .count()
        };
        let length = if length >= 3 {
            tokens.push(matched(length as u16, (index - candidate) as u16));
            length
        } else {
            tokens.push(Token::Literal(data[index]));
            1
        };
        for position in index..(index + length).min(data.len().saturating_sub(2)) {
            latest[key(position)] = position;
        }
        index += length;
    }
    token_block(tokens)
}

#[test]
fn apng_max_endpoint_keeps_default_source_topology() {
    // Above the planner's compact search size, its 85,000-byte blocks keep
    // most of their short scattered matches, which D4's chains drop. D4 thus
    // wins under `ApngDefault`, and the APNG endpoint must keep that result.
    let mut writer = BitWriter::default();
    let mut alignment = 0_u8;
    for (index, state) in [0x9e37_79b9_7f4a_7c15_u64, 0xd1b5_4a32_d192_ed03]
        .into_iter()
        .enumerate()
    {
        let block = skewed_greedy_block(state, 85_000);
        let plan = plan_block(
            &block,
            alignment,
            &Options::default(),
            &mut SearchStop::never(),
        );
        alignment = ((u64::from(alignment) + plan.bits) & 7) as u8;
        emit_block(&mut writer, &[], &plan, index == 1).unwrap();
    }
    let input = writer.into_bytes();
    let parsed = parse_stream(&input, 170_000).unwrap();
    assert!(default_source_topology_eligible(
        &parsed.blocks,
        input.len(),
        parsed.decoded_size
    ));
    let default = optimize_raw_prefix_with_floor(
        &input,
        &Options::default(),
        170_000,
        DefaultFloor::ApngDefault,
    )
    .unwrap();

    let source = CandidateInput {
        compressed: &input,
        blocks: &parsed.blocks,
        meaningful_bits: parsed.meaningful_bits,
        decoded_limit: 170_000,
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
    let (complete, slid) = build_complete_apng_default_floor_candidate(
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
    let endpoint = slid
        .filter(|slid| slid.is_strictly_smaller_than(&complete))
        .unwrap_or(complete);
    assert!(!is_strictly_better(
        default.data.len(),
        default.info.deflate_bits,
        endpoint.data.len(),
        endpoint.bits,
    ));
}

#[test]
fn route_errors_cancel_siblings_without_marking_a_timeout() {
    let deadline = deadline_with_grace(Instant::now(), Duration::MAX);
    let failed: Result<()> =
        run_route_with_cancellation(&deadline, || Err(Error::new("synthetic route failure")));

    assert!(failed.is_err());
    assert!(deadline.route_should_stop());
    assert!(!deadline.was_triggered());

    let successful = deadline_with_grace(Instant::now(), Duration::MAX);
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
    let inside_grace = deadline_with_grace(
        Instant::now()
            .checked_sub(duration + grace / 2)
            .expect("test duration fits in Instant"),
        duration,
    );
    assert!(!inside_grace.can_start_route());
    assert!(!inside_grace.expired());
    assert!(inside_grace.was_triggered());

    let past_hard_deadline = deadline_with_grace(
        Instant::now()
            .checked_sub(duration + grace + Duration::from_millis(1))
            .expect("test duration fits in Instant"),
        duration,
    );
    assert!(past_hard_deadline.expired());
    assert!(past_hard_deadline.was_triggered());
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
    assert!(parse_validated_rewrite(&[0x03, 0x00], 1, empty, &[]).is_ok());

    // A complete stream followed by another byte is not a valid rewrite
    // of the exact candidate byte range.
    assert_eq!(
        parse_validated_rewrite(&[0x03, 0x00, 0x00], 1, empty, &[])
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
fn header_tree_search_is_not_mandatory_at_zero_max_budget() {
    let block = header_tree_test_block();
    let plan = PlannedBlock {
        tokens: block.tokens.clone(),
        plain: block.plain.clone(),
        representation: Representation::Dynamic(block.original_dynamic.unwrap()),
        bits: block.original.unwrap().len,
        source_type: SourceBlockType::Dynamic,
    };
    let mut writer = BitWriter::default();
    emit_block(&mut writer, &[], &plan, true).unwrap();
    let data = writer.into_bytes();
    let ordinary = optimize_raw(&data, &Options::default()).unwrap();
    let parsed = parse_stream(&ordinary.data, 4096).unwrap();
    assert!(
        super::super::header::plan_header_tree(
            &parsed.blocks[0],
            true,
            &mut super::super::header::HeaderTreeBudget::new(),
            &mut SearchStop::never()
        )
        .is_some(),
        "the completed Default endpoint must expose the extra work this guard forbids"
    );
    let zero = optimize_raw(
        &data,
        &Options {
            exhaustive: true,
            timeout: Duration::ZERO,
            ..Options::default()
        },
    )
    .unwrap();
    assert_eq!(zero.data, ordinary.data);
}

#[test]
fn code_length_rotations_do_not_change_zero_budget_default_floor() {
    let block = rotation_test_block(&[1, 1]);
    let plan = PlannedBlock {
        tokens: block.tokens.clone(),
        plain: block.plain.clone(),
        representation: Representation::Dynamic(block.original_dynamic.unwrap()),
        bits: block.original.unwrap().len,
        source_type: SourceBlockType::Dynamic,
    };
    let mut writer = BitWriter::default();
    emit_block(&mut writer, &[], &plan, true).unwrap();
    let data = writer.into_bytes();
    for strict in [false, true] {
        let ordinary = optimize_raw(
            &data,
            &Options {
                strict,
                ..Options::default()
            },
        )
        .unwrap();
        let zero = optimize_raw(
            &data,
            &Options {
                strict,
                exhaustive: true,
                timeout: Duration::ZERO,
                ..Options::default()
            },
        )
        .unwrap();
        assert_eq!(ordinary.data, zero.data);
    }
}

#[test]
fn coupled_swaps_do_not_change_zero_budget_default_floor() {
    let data = crate::deflate::header::test_support::coupled_swap_test_stream();
    let decoded_size = parse_stream(&data, 1 << 20).unwrap().decoded_size;
    for strict in [false, true] {
        let ordinary = optimize_raw(
            &data,
            &Options {
                strict,
                ..Options::default()
            },
        )
        .unwrap();
        // Default's first-route cut at the same zero timeout, which zero-budget
        // Max keeps as a final competitor.
        let cut = optimize_raw_prefix_with_floor_and_grace(
            &data,
            &Options {
                strict,
                timeout: Duration::ZERO,
                ..Options::default()
            },
            decoded_size,
            DefaultFloor::Complete,
            Duration::from_secs(3600),
        )
        .unwrap();
        let zero = optimize_raw(
            &data,
            &Options {
                strict,
                exhaustive: true,
                timeout: Duration::ZERO,
                ..Options::default()
            },
        )
        .unwrap();
        let expected = if is_strictly_better(
            cut.data.len(),
            cut.info.deflate_bits,
            ordinary.data.len(),
            ordinary.info.deflate_bits,
        ) {
            &cut.data
        } else {
            &ordinary.data
        };
        assert_eq!(&zero.data, expected, "strict={strict}");
    }
}

#[test]
fn default_sweep_finishes_with_the_distance_ladder() {
    // One block carried by distance-4 runs, plus one length-3 match in each
    // of distance symbols 5 through 29. Only the ladder removes them all.
    let input = crate::deflate::distance_band::test_support::distance_ladder_test_stream();
    let parsed = parse_stream(&input, 1 << 20).unwrap();
    let identity = StreamIdentity {
        decoded_size: parsed.decoded_size,
        crc32: parsed.crc32,
        adler32: parsed.adler32,
    };
    let source = CandidateInput {
        compressed: &input,
        blocks: &parsed.blocks,
        meaningful_bits: parsed.meaningful_bits,
        decoded_limit: 1 << 20,
        identity,
    };
    let parent = Candidate {
        data: input.clone(),
        bits: parsed.meaningful_bits,
        output_max_distance: Some(parsed.max_distance),
        plans: Vec::new(),
        block_report: None,
        route: "generated distance-ladder parent",
        max_planner_is_stable: false,
        replaced_parents: Vec::new(),
    };
    let options = Options::default();
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
    let only_distance_four = |candidate: &Candidate| {
        let check = parse_validated_rewrite(&candidate.data, 1 << 20, identity, &[]).unwrap();
        assert_eq!(candidate.bits, check.meaningful_bits);
        assert!(check
            .blocks
            .iter()
            .flat_map(|block| block.tokens.iter())
            .all(|token| !matches!(token, Token::Match { distance, .. } if *distance != 4)));
    };

    let ordinary = improve_with_terminal_searches(
        source,
        &options,
        DefaultFloorWork::Mandatory,
        DefaultFloorWork::Timed(&deadline),
        progress,
        HeaderFit::InSweep,
        false,
        parent.clone(),
    )
    .unwrap();
    assert!(ordinary.is_strictly_smaller_than(&parent));
    only_distance_four(&ordinary);

    // Max's mandatory Default endpoint keeps the ladder in its final
    // competitor even when the slide itself finds nothing.
    let slid = slid_default_endpoint(
        source,
        &options,
        progress,
        &mut TerminalParseCache::default(),
        &parent,
        HeaderFit::InSweep,
    )
    .unwrap()
    .unwrap();
    only_distance_four(&slid);
}

#[test]
fn source_max_comparisons_keep_a_losing_contender_after_its_parents() {
    let candidate = |bytes: usize, bits: u64, route: &'static str| Candidate {
        data: vec![0; bytes],
        bits,
        output_max_distance: None,
        plans: Vec::new(),
        block_report: None,
        route,
        max_planner_is_stable: false,
        replaced_parents: Vec::new(),
    };

    // Default keeps nothing, win or lose.
    let mut incumbent = candidate(10, 80, "incumbent");
    assert!(!incumbent.replace_keeping_both(candidate(11, 88, "larger"), false));
    assert!(incumbent.replaced_parents.is_empty());

    // A loser is kept after its own parents, both at the oldest end, without
    // its plans.
    let mut incumbent = candidate(10, 80, "incumbent");
    incumbent.replaced_parents.push(candidate(12, 96, "older"));
    let mut loser = candidate(11, 88, "loser");
    loser
        .replaced_parents
        .push(candidate(13, 104, "loser's parent"));
    assert!(!incumbent.replace_keeping_both(loser, true));
    assert_eq!(incumbent.route, "incumbent");
    let routes: Vec<_> = incumbent
        .replaced_parents
        .iter()
        .map(|parent| parent.route)
        .collect();
    assert_eq!(routes, ["loser's parent", "loser", "older"]);
    assert!(incumbent
        .replaced_parents
        .iter()
        .all(|parent| parent.plans.is_empty()));

    // A winner replaces the incumbent and keeps it, as with
    // `replace_keeping_parent`.
    assert!(incumbent.replace_keeping_both(candidate(9, 72, "winner"), true));
    assert_eq!(incumbent.route, "winner");
    assert_eq!(
        incumbent.replaced_parents.last().map(|parent| parent.route),
        Some("incumbent")
    );
}

#[test]
fn max_replacements_keep_the_replaced_candidate_and_both_histories() {
    let candidate = |bytes: usize, bits: u64, route: &'static str| Candidate {
        data: vec![0; bytes],
        bits,
        output_max_distance: None,
        plans: Vec::new(),
        block_report: None,
        route,
        max_planner_is_stable: false,
        replaced_parents: Vec::new(),
    };

    // Default keeps nothing.
    let mut incumbent = candidate(10, 80, "incumbent");
    assert!(incumbent.replace_keeping_parent(candidate(9, 72, "smaller"), false));
    assert!(incumbent.replaced_parents.is_empty());

    // A winner keeps its own parents, then the incumbent's, then the
    // incumbent itself.
    let mut incumbent = candidate(10, 80, "incumbent");
    incumbent.replaced_parents.push(candidate(12, 96, "older"));
    let mut winner = candidate(9, 72, "winner");
    winner
        .replaced_parents
        .push(candidate(11, 88, "winner's parent"));
    assert!(incumbent.replace_keeping_parent(winner, true));
    assert_eq!(incumbent.route, "winner");
    let routes: Vec<_> = incumbent
        .replaced_parents
        .iter()
        .map(|parent| parent.route)
        .collect();
    assert_eq!(routes, ["winner's parent", "older", "incumbent"]);

    // A tie keeps the incumbent but still hands over the contender's parents,
    // at the oldest end, so they are finished last.
    let mut tie = candidate(9, 72, "tie");
    tie.replaced_parents
        .push(candidate(13, 104, "tie's parent"));
    assert!(!incumbent.replace_keeping_parent(tie, true));
    assert_eq!(incumbent.route, "winner");
    let routes: Vec<_> = incumbent
        .replaced_parents
        .iter()
        .map(|parent| parent.route)
        .collect();
    assert_eq!(
        routes,
        ["tie's parent", "winner's parent", "older", "incumbent"]
    );

    // An empty lineage takes its first candidate; a later winner keeps it.
    let mut lineage = None;
    assert!(replace_optional_keeping_parent(
        &mut lineage,
        candidate(10, 80, "first"),
        true
    ));
    assert!(replace_optional_keeping_parent(
        &mut lineage,
        candidate(9, 72, "second"),
        true
    ));
    let lineage = lineage.unwrap();
    assert_eq!(lineage.route, "second");
    let routes: Vec<_> = lineage
        .replaced_parents
        .iter()
        .map(|parent| parent.route)
        .collect();
    assert_eq!(routes, ["first"]);
}
