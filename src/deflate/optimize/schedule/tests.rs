// SPDX-License-Identifier: MIT
// SPDX-FileCopyrightText: 2026 Andrew C.E. Dent <https://github.com/ace-dent>

use std::time::{Duration, Instant};

use super::*;
use crate::deflate::stop::timeout_grace;

/// One stored byte followed by an empty final fixed block.
const TWO_BLOCK_RAW: &[u8] = &[0x00, 0x01, 0x00, 0xfe, 0xff, b'x', 0x03, 0x00];

fn parsed_source() -> ParsedStream {
    parse_stream(TWO_BLOCK_RAW, 1 << 20).unwrap()
}

fn max_options(timeout: Duration) -> Options {
    Options {
        exhaustive: true,
        timeout,
        ..Options::default()
    }
}

#[test]
fn reserved_terminal_work_gives_the_primary_window_no_grace() {
    let parsed = parsed_source();
    let started = Instant::now();
    let options = max_options(Duration::from_secs(100));
    let grace = timeout_grace(options.timeout);
    for (floor, primary) in [
        (DefaultFloor::Complete, Duration::from_secs(80)),
        (DefaultFloor::CompleteThenBounded, Duration::from_secs(80)),
        (DefaultFloor::ApngMax, Duration::from_secs(95)),
    ] {
        let deadlines = StreamDeadlines::new(started, grace, &options, floor, &parsed);
        assert!(deadlines.reserve_terminal, "{floor:?}");
        assert_eq!(deadlines.primary.duration, primary, "{floor:?}");
        assert_eq!(deadlines.primary.grace, Duration::ZERO, "{floor:?}");
        assert_eq!(deadlines.terminal.duration, options.timeout, "{floor:?}");
        assert_eq!(deadlines.terminal.grace, grace, "{floor:?}");
        assert!(std::ptr::eq(deadlines.terminal_work(), &deadlines.terminal));
    }
}

#[test]
fn unreserved_streams_use_the_primary_deadline_for_every_phase() {
    let parsed = parsed_source();
    let started = Instant::now();
    let options = max_options(Duration::from_secs(100));
    let grace = timeout_grace(options.timeout);
    for floor in [
        DefaultFloor::Shared,
        DefaultFloor::SharedExact,
        DefaultFloor::ApngDefault,
        DefaultFloor::Established,
        DefaultFloor::MandatoryComplete,
    ] {
        let deadlines = StreamDeadlines::new(started, grace, &options, floor, &parsed);
        assert!(!deadlines.reserve_terminal, "{floor:?}");
        assert_eq!(deadlines.primary.duration, options.timeout, "{floor:?}");
        assert_eq!(deadlines.primary.grace, grace, "{floor:?}");
        assert!(std::ptr::eq(deadlines.terminal_work(), &deadlines.primary));
    }
}

#[test]
fn route_families_require_max_and_a_matching_source_topology() {
    let parsed = parsed_source();
    let started = Instant::now();
    let decoded_limit = 1 << 20;
    let source = CandidateInput {
        compressed: TWO_BLOCK_RAW,
        blocks: &parsed.blocks,
        meaningful_bits: parsed.meaningful_bits,
        decoded_limit,
        identity: StreamIdentity {
            decoded_size: parsed.decoded_size,
            crc32: parsed.crc32,
            adler32: parsed.adler32,
        },
    };
    let deadline = Deadline::with_grace(started, Duration::from_secs(100), Duration::ZERO);
    for exhaustive in [false, true] {
        let options = Options {
            exhaustive,
            ..max_options(Duration::from_secs(100))
        };
        let progress = Progress::begin(
            &options,
            started,
            StreamProgress {
                blocks: parsed.source_block_count,
                compressed_bytes: parsed.consumed,
                decoded_bytes: parsed.decoded_size,
                empty_blocks: parsed.source_empty_block_count,
                meaningful_bits: parsed.meaningful_bits,
                parse_elapsed: Duration::ZERO,
            },
            None,
        );
        let context = RouteContext {
            source,
            options: &options,
            default_floor: DefaultFloor::CompleteThenBounded,
            deadline: &deadline,
            progress,
        };
        let eligibility = RouteEligibility::new(&context);
        // The source is small enough for parallel Max routes, but its only
        // nonempty block is stored, so no source-graph family applies.
        assert_eq!(eligibility.parallel_routes, exhaustive);
        assert!(!eligibility.deft4j);
        assert!(!eligibility.narrow_source);
        assert!(!eligibility.compact_tree);
        assert!(!eligibility.compact_proven_feedback);
    }
}
