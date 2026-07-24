// SPDX-License-Identifier: MIT

use std::sync::atomic::{AtomicU8, Ordering};
use std::thread;
use std::time::Instant;

use crate::{Error, Options, Result};

use super::bitstream::BitWriter;
use super::block::emit_block;
use super::deft4j::plan_source_blocks;
use super::model::{ParsedBlock, ParsedStream, PlannedBlock, Representation, SourceBlockType};
use super::parse::{parse_stream, parsed_model_bytes};
use super::stream::{
    fragmented_collect_seed, plan_fragmented_replay, plan_source_no_split_route, plan_stream,
    plan_terminal_merge_route,
};

// Long source-block chains can need one pass to establish profitable adjacent
// groups, then two inexpensive passes over that much simpler block layout to
// settle their boundaries and tables. Every round below must strictly improve
// the complete stream, so the extra slot cannot oscillate or grow the output.
const DEFAULT_RAW_REPLAY_LIMIT: usize = 3;
const MAX_RAW_REPLAY_LIMIT: usize = 8;
// A tiny single Huffman block can still benefit from the source-ordered deft4j
// candidate graph: its selected token spelling can expose a second win to
// ordinary refinement. Keep that extra route within the same 4 KiB
// compact-stream band used by the original Columbo C implementation. On larger
// one-block streams it can consume the deadline before broader routes that
// already have stronger incumbents. These thresholds are Columbo scheduling
// policy, not limits in deft4j itself.
const DEFT4J_SINGLE_BLOCK_MAX_COMPRESSED: usize = 4_096;
const DEFT4J_MULTI_BLOCK_MIN_HUFFMAN_BLOCKS: usize = 2;
const DEFT4J_SOURCE_LIST_MAX_BLOCKS: usize = 128;
const WEAK_DEFT4J_GAIN_BASIS_POINTS: u64 = 200;
const NARROW_SOURCE_MIN_COMPRESSED: usize = 32 * 1_024;
const NARROW_SOURCE_MAX_COMPRESSED: usize = 512 * 1_024;
const TERMINAL_MERGE_MIN_COMPRESSED: usize = 100_001;
// Parallel routes shorten a container's wall-clock search without making its
// peak memory proportional to every individually valid route budget. Larger
// streams retain the same candidates, but evaluate them serially.
const PARALLEL_ROUTE_MAX_COMPRESSED: usize = 8 * 1_024 * 1_024;
const PARALLEL_ROUTE_MAX_DECODED: u64 = 64 * 1_024 * 1_024;
const PARALLEL_ROUTE_MAX_MODEL: usize = 64 * 1_024 * 1_024;

/// Facts collected while decoding the source stream. Container handlers use
/// these values to validate their checksums without inflating a second time.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct RawInfo {
    pub(crate) crc32: u32,
    pub(crate) adler32: u32,
    pub(crate) size: u64,
    pub(crate) max_distance: u16,
    pub(crate) deflate_bits: u64,
    pub(crate) source_block_count: usize,
    pub(crate) source_empty_block_count: usize,
}

pub(crate) struct RawOptimization {
    pub(crate) data: Vec<u8>,
    pub(crate) consumed: usize,
    pub(crate) info: RawInfo,
    pub(crate) timed_out: bool,
}

/// Decide whether max mode must finish its ordinary-mode comparison floor.
///
/// A standalone stream uses [`DefaultFloor::Complete`] so `--max` cannot lose
/// an optimization found by normal mode. A single scheduled PNG image uses
/// [`DefaultFloor::Bounded`]. Multi-stream containers use
/// [`DefaultFloor::Shared`], which also prevents a tiny single-block deft4j
/// route from consuming time needed by later members.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DefaultFloor {
    Complete,
    Bounded,
    Shared,
}

impl DefaultFloor {
    fn is_bounded(self) -> bool {
        !matches!(self, Self::Complete)
    }

    fn allows_single_block_deft4j(self) -> bool {
        !matches!(self, Self::Shared)
    }
}

pub(crate) fn optimize_raw(input: &[u8], options: &Options) -> Result<RawOptimization> {
    let optimized = optimize_raw_prefix(input, options, options.max_decoded_bytes)?;
    if optimized.consumed != input.len() {
        return Err(Error::new("trailing data after Deflate stream"));
    }
    Ok(optimized)
}

/// Optimize the first complete Deflate stream in `input`.
///
/// Prefix decoding is essential for concatenated GZIP members, whose trailer
/// immediately follows a non-byte-length-delimited raw Deflate stream.
pub(crate) fn optimize_raw_prefix(
    input: &[u8],
    options: &Options,
    decoded_limit: u64,
) -> Result<RawOptimization> {
    optimize_raw_prefix_with_floor(input, options, decoded_limit, DefaultFloor::Complete)
}

/// Optimize one raw stream with an explicit max-mode default-floor policy.
///
/// This is kept crate-private because the distinction belongs to container
/// scheduling, not to Columbo's public optimization options.
pub(crate) fn optimize_raw_prefix_with_floor(
    input: &[u8],
    options: &Options,
    decoded_limit: u64,
    default_floor: DefaultFloor,
) -> Result<RawOptimization> {
    if input.len() as u64 > options.max_input_bytes {
        return Err(Error::new("input exceeds configured safety limit"));
    }

    let started = Instant::now();
    let parsed = parse_stream(input, decoded_limit.min(options.max_decoded_bytes))?;
    let blocks = parsed.blocks;
    let deadline = Deadline {
        started,
        duration: options.timeout,
        state: AtomicU8::new(0),
    };

    // Prefix callers need the exact bytes occupied by the first stream. Any
    // unused high bits in its final byte belong to that stream's byte-level
    // representation and are retained when the source wins.
    let original = &input[..parsed.consumed];
    let decoded_limit = decoded_limit.min(options.max_decoded_bytes);
    let identity = StreamIdentity {
        decoded_size: parsed.decoded_size,
        crc32: parsed.crc32,
        adler32: parsed.adler32,
    };
    let source = CandidateInput {
        compressed: original,
        blocks: &blocks,
        meaningful_bits: parsed.meaningful_bits,
        decoded_limit,
        identity,
    };
    let deft4j_eligible = options.exhaustive
        && deft4j_source_route_eligible(
            &blocks,
            original.len(),
            default_floor.allows_single_block_deft4j(),
        );

    // The bounded floor, source-ordered deft4j route, and narrow no-split route
    // are independent reads of the parsed stream. Small models may run them
    // together so no route consumes the whole container slice; larger models
    // retain the same fixed candidate order without overlapping their arenas.
    // Payload buffers remain shared by `Arc`, while plans and output bytes are
    // route-local. Complete standalone floors keep their unbounded order.
    let run_narrow_source = options.exhaustive
        && default_floor == DefaultFloor::Bounded
        && narrow_source_route_eligible(&blocks, original.len());
    let (mut bounded_floor_candidate, mut deft4j_candidate, mut narrow_candidate) =
        if options.exhaustive && default_floor.is_bounded() {
            let run_deft4j = deft4j_eligible && !deadline.expired();
            let (floor, deft4j, narrow) = build_bounded_phase_candidates(
                source,
                options,
                run_deft4j,
                run_narrow_source,
                &deadline,
            )?;
            (Some(floor), deft4j, narrow)
        } else {
            (None, None, None)
        };
    if options.inspect {
        if let Some(floor) = &bounded_floor_candidate {
            eprintln!(
                "inspect: route=bounded-floor bytes={} bits={}",
                floor.data.len(),
                floor.bits
            );
        }
        if let Some(deft4j) = &deft4j_candidate {
            eprintln!(
                "inspect: route=columbo-deft4j-derived bytes={} bits={}",
                deft4j.data.len(),
                deft4j.bits
            );
        }
        if let Some(narrow) = &narrow_candidate {
            eprintln!(
                "inspect: route=no-split bytes={} bits={}",
                narrow.data.len(),
                narrow.bits
            );
        }
    }
    let seed_weak_deft4j = default_floor == DefaultFloor::Bounded
        && deft4j_candidate.as_ref().is_some_and(|deft4j| {
            has_multiple_nonempty_blocks(&blocks)
                && deft4j_gain_is_below(
                    parsed.meaningful_bits,
                    deft4j.bits,
                    WEAK_DEFT4J_GAIN_BASIS_POINTS,
                )
        });
    if !options.inspect {
        if let Some(floor) = &mut bounded_floor_candidate {
            // The later deft4j refinement needs only the encoded floor for its
            // strict comparison. Release transformed floor plans before it
            // reparses another complete candidate.
            floor.plans.clear();
        }
        if let Some(deft4j) = &mut deft4j_candidate {
            // Refinement needs only the encoded stream. Release retained
            // deft4j-route token plans before reparsing it so the models do not
            // overlap at peak memory.
            deft4j.plans.clear();
        }
        if let Some(narrow) = &mut narrow_candidate {
            narrow.plans.clear();
        }
    }
    if default_floor.is_bounded() && !deadline.expired() {
        if seed_weak_deft4j {
            if let Some(floor) = bounded_floor_candidate.as_ref().filter(|floor| {
                is_strictly_better(
                    floor.data.len(),
                    floor.bits,
                    original.len(),
                    parsed.meaningful_bits,
                )
            }) {
                if let Some(mut seeded) = build_deft4j_seed_candidate(
                    floor,
                    options,
                    decoded_limit,
                    identity,
                    default_floor.allows_single_block_deft4j(),
                    &mut || deadline.expired(),
                )? {
                    if !deadline.expired() {
                        let refined = refine_with_default_planner(
                            &seeded,
                            options,
                            decoded_limit,
                            identity,
                            &mut || deadline.expired(),
                        )?;
                        if is_strictly_better(
                            refined.data.len(),
                            refined.bits,
                            seeded.data.len(),
                            seeded.bits,
                        ) {
                            seeded = refined;
                        }
                    }
                    let narrow_already_wins = narrow_candidate.as_ref().is_some_and(|narrow| {
                        is_strictly_better(
                            narrow.data.len(),
                            narrow.bits,
                            seeded.data.len(),
                            seeded.bits,
                        )
                    });
                    if !narrow_already_wins
                        && !deadline.expired()
                        && (TERMINAL_MERGE_MIN_COMPRESSED..=NARROW_SOURCE_MAX_COMPRESSED)
                            .contains(&seeded.data.len())
                    {
                        if let Some(terminal) = refine_with_terminal_merge(
                            &seeded,
                            options,
                            decoded_limit,
                            identity,
                            &mut || deadline.expired(),
                        )? {
                            if is_strictly_better(
                                terminal.data.len(),
                                terminal.bits,
                                seeded.data.len(),
                                seeded.bits,
                            ) {
                                seeded = terminal;
                            }
                        }
                    }
                    let seeded_wins = match deft4j_candidate.as_ref() {
                        Some(deft4j) => is_strictly_better(
                            seeded.data.len(),
                            seeded.bits,
                            deft4j.data.len(),
                            deft4j.bits,
                        ),
                        None => true,
                    };
                    if seeded_wins {
                        deft4j_candidate = Some(seeded);
                    }
                }
            }
        } else if let Some(deft4j) = deft4j_candidate.as_ref() {
            let refined =
                refine_with_default_planner(deft4j, options, decoded_limit, identity, &mut || {
                    deadline.expired()
                })?;
            if is_strictly_better(
                refined.data.len(),
                refined.bits,
                deft4j.data.len(),
                deft4j.bits,
            ) {
                deft4j_candidate = Some(refined);
            }
        }
    }
    if !options.inspect {
        if let Some(deft4j) = &mut deft4j_candidate {
            // Keep only the encoded incumbent for comparison and later routes;
            // refinement can otherwise retain another expanded token graph.
            deft4j.plans.clear();
        }
    }
    if let Some(narrow) = narrow_candidate {
        let narrow_wins = match deft4j_candidate.as_ref() {
            Some(deft4j) => is_strictly_better(
                narrow.data.len(),
                narrow.bits,
                deft4j.data.len(),
                deft4j.bits,
            ),
            None => true,
        };
        if narrow_wins {
            deft4j_candidate = Some(narrow);
        }
    }

    let mut candidate = if let Some(floor) = bounded_floor_candidate {
        floor
    } else if options.exhaustive {
        // Max mode starts from the genuine normal-mode result. Standalone
        // streams finish that floor so --max cannot lose to normal mode;
        // locally scheduled container streams keep it within their allotted
        // time. Either way, elapsed floor time advances the same deadline.
        let mut floor_options = options.clone();
        floor_options.exhaustive = false;
        match default_floor {
            DefaultFloor::Complete => {
                let mut never_expires = || false;
                build_candidate(
                    source,
                    &floor_options,
                    DEFAULT_RAW_REPLAY_LIMIT,
                    &mut never_expires,
                )?
            }
            DefaultFloor::Bounded | DefaultFloor::Shared => {
                build_bounded_floor_candidate(source, options, &mut || deadline.expired())?
            }
        }
    } else {
        build_candidate(source, options, DEFAULT_RAW_REPLAY_LIMIT, &mut || {
            deadline.expired()
        })?
    };
    if deft4j_eligible && default_floor == DefaultFloor::Complete && !deadline.expired() {
        deft4j_candidate =
            build_deft4j_source_candidate(source, options, &mut || deadline.expired())?;
    }

    if !options.inspect {
        if let Some(deft4j) = &mut deft4j_candidate {
            deft4j.plans.clear();
        }
    }
    let mut deft4j_lineage = false;
    if let Some(deft4j) = deft4j_candidate {
        if is_strictly_better(
            deft4j.data.len(),
            deft4j.bits,
            candidate.data.len(),
            candidate.bits,
        ) {
            candidate = deft4j;
            deft4j_lineage = true;
        }
    }

    // A 4,096-token collection can start slightly larger but converge to a
    // better fragmented-stream layout after strict replays. It is additive to
    // the normal-mode comparison floor above: a short container slice must not
    // spend its whole budget here before securing that floor. The collection
    // seed itself remains deadline-independent and can still win when there is
    // no time left for its optional replay rounds.
    let mut fragmented_candidate = if options.exhaustive
        // Once the source-ordered deft4j route has won and spent the deadline,
        // another source-derived seed cannot receive any replay work. Avoid
        // that duplicate post-deadline collection.
        && (!deft4j_lineage || !deadline.expired())
    {
        fragmented_collect_seed(&blocks, 0, options)
            .map(|plans| {
                let mut replay_options = options.clone();
                replay_options.exhaustive = false;
                let source = CandidateInput {
                    compressed: original,
                    blocks: &blocks,
                    meaningful_bits: parsed.meaningful_bits,
                    decoded_limit,
                    identity,
                };
                build_candidate_from_plans(
                    source,
                    plans,
                    &replay_options,
                    MAX_RAW_REPLAY_LIMIT,
                    ReplayPlanner::Fragmented,
                    &mut || deadline.expired(),
                )
            })
            .transpose()?
    } else {
        None
    };
    if !options.inspect {
        if let Some(fragmented) = &mut fragmented_candidate {
            fragmented.plans.clear();
        }
    }

    if let Some(fragmented) = fragmented_candidate {
        if is_strictly_better(
            fragmented.data.len(),
            fragmented.bits,
            candidate.data.len(),
            candidate.bits,
        ) {
            candidate = fragmented;
        }
    }

    if options.exhaustive {
        if !options.inspect {
            // The encoded floor is all we need for comparison. Releasing its
            // copied tokens before max search keeps peak memory predictable.
            candidate.plans.clear();
        }

        // Generic max also explores source-boundary and table families outside
        // deft4j's source-ordered graph. Run it before a rewritten seed can
        // spend the remainder on one large merged block.
        if !deadline.expired() {
            let max_candidate =
                build_candidate(source, options, MAX_RAW_REPLAY_LIMIT, &mut || {
                    deadline.expired()
                })?;
            if is_strictly_better(
                max_candidate.data.len(),
                max_candidate.bits,
                candidate.data.len(),
                candidate.bits,
            ) {
                candidate = max_candidate;
            }
        }

        if !options.inspect {
            // A winning source restart can own another full plan graph. Only
            // its encoded bytes are needed as the optional replay seed.
            candidate.plans.clear();
        }

        let seed_selected = options.min_distance_codes
            || is_strictly_better(
                candidate.data.len(),
                candidate.bits,
                original.len(),
                parsed.meaningful_bits,
            );
        if seed_selected && !deadline.expired() {
            // Rewritten match choices and boundaries can expose later max
            // transformations, so retain one additive seeded pass after both
            // source-shaped routes. Its incumbent remains available if this
            // final route times out or fails to improve it.
            let seed_stream = parse_stream(&candidate.data, decoded_limit)?;
            validate_replayed_stream(&seed_stream, candidate.data.len(), identity)?;
            let source = CandidateInput {
                compressed: &candidate.data,
                blocks: &seed_stream.blocks,
                meaningful_bits: candidate.bits,
                decoded_limit,
                identity,
            };
            let seeded_candidate =
                build_candidate(source, options, MAX_RAW_REPLAY_LIMIT, &mut || {
                    deadline.expired()
                })?;
            if is_strictly_better(
                seeded_candidate.data.len(),
                seeded_candidate.bits,
                candidate.data.len(),
                candidate.bits,
            ) {
                candidate = seeded_candidate;
            }
        }
    }

    if options.inspect {
        inspect_plans(&candidate.plans);
    }

    let keep_original = !options.min_distance_codes
        && !is_strictly_better(
            candidate.data.len(),
            candidate.bits,
            original.len(),
            parsed.meaningful_bits,
        );
    let deflate_bits = if keep_original {
        parsed.meaningful_bits
    } else {
        candidate.bits
    };

    // Planning is complete. Drop model storage before copying a winning source
    // stream, then reuse the generated output allocation where possible. This
    // keeps the no-growth guarantee from briefly requiring two source-sized
    // outputs plus the full parsed model.
    drop(blocks);
    drop(std::mem::take(&mut candidate.plans));
    if keep_original {
        candidate.data.clear();
        candidate
            .data
            .try_reserve_exact(original.len())
            .map_err(|_| Error::new("could not allocate Deflate output"))?;
        candidate.data.extend_from_slice(original);
    }
    let data = candidate.data;

    Ok(RawOptimization {
        data,
        consumed: parsed.consumed,
        info: RawInfo {
            crc32: parsed.crc32,
            adler32: parsed.adler32,
            size: parsed.decoded_size,
            max_distance: parsed.max_distance,
            deflate_bits,
            source_block_count: parsed.source_block_count,
            source_empty_block_count: parsed.source_empty_block_count,
        },
        timed_out: deadline.was_triggered(),
    })
}

fn deft4j_source_route_eligible(
    blocks: &[ParsedBlock],
    compressed_len: usize,
    allow_single_block: bool,
) -> bool {
    let mut nonempty = 0_usize;
    let mut huffman = 0_usize;
    for block in blocks.iter().filter(|block| !block.plain.is_empty()) {
        nonempty += 1;
        if matches!(
            block.source_type,
            SourceBlockType::Fixed | SourceBlockType::Dynamic
        ) {
            huffman += 1;
        }
    }
    match nonempty {
        1 => {
            allow_single_block
                && huffman == 1
                && compressed_len <= DEFT4J_SINGLE_BLOCK_MAX_COMPRESSED
        }
        2..=DEFT4J_SOURCE_LIST_MAX_BLOCKS => huffman >= DEFT4J_MULTI_BLOCK_MIN_HUFFMAN_BLOCKS,
        _ => false,
    }
}

fn narrow_source_route_eligible(blocks: &[ParsedBlock], compressed_len: usize) -> bool {
    (NARROW_SOURCE_MIN_COMPRESSED..=NARROW_SOURCE_MAX_COMPRESSED).contains(&compressed_len)
        && (2..=DEFT4J_SOURCE_LIST_MAX_BLOCKS).contains(
            &blocks
                .iter()
                .filter(|block| !block.plain.is_empty())
                .count(),
        )
        && blocks.iter().all(|block| {
            block.plain.is_empty()
                || matches!(
                    block.source_type,
                    SourceBlockType::Fixed | SourceBlockType::Dynamic
                )
        })
}

fn has_multiple_nonempty_blocks(blocks: &[ParsedBlock]) -> bool {
    blocks
        .iter()
        .filter(|block| !block.plain.is_empty())
        .take(2)
        .count()
        == 2
}

fn deft4j_gain_is_below(source_bits: u64, candidate_bits: u64, basis_points: u64) -> bool {
    let saved = source_bits.saturating_sub(candidate_bits);
    saved.saturating_mul(10_000) < source_bits.saturating_mul(basis_points)
}

#[derive(Clone, Copy)]
struct StreamIdentity {
    decoded_size: u64,
    crc32: u32,
    adler32: u32,
}

struct Candidate {
    data: Vec<u8>,
    bits: u64,
    plans: Vec<PlannedBlock>,
}

/// One complete compressed stream together with the facts needed to verify
/// every accepted reparse/replan round.
#[derive(Clone, Copy)]
struct CandidateInput<'a> {
    compressed: &'a [u8],
    blocks: &'a [ParsedBlock],
    meaningful_bits: u64,
    decoded_limit: u64,
    identity: StreamIdentity,
}

/// Run the independent bounded comparison routes under one wall clock.
///
/// Small inputs can safely share their immutable parsed blocks across worker
/// threads. Larger inputs use the same fixed route order serially, preventing
/// otherwise bounded per-route arenas from adding up to an excessive peak.
fn build_bounded_phase_candidates(
    source: CandidateInput<'_>,
    options: &Options,
    run_deft4j: bool,
    run_narrow: bool,
    deadline: &Deadline,
) -> Result<(Candidate, Option<Candidate>, Option<Candidate>)> {
    if !run_deft4j && !run_narrow {
        return Ok((
            build_bounded_floor_candidate(source, options, &mut || deadline.expired())?,
            None,
            None,
        ));
    }

    if !parallel_route_is_bounded(source) {
        return build_bounded_phase_candidates_sequential(
            source, options, run_deft4j, run_narrow, deadline,
        );
    }

    thread::scope(|scope| {
        let deft4j_worker = run_deft4j.then(|| {
            thread::Builder::new()
                .name("columbo-deft4j-derived".into())
                .spawn_scoped(scope, || {
                    run_route_with_cancellation(deadline, || {
                        build_deft4j_source_candidate(source, options, &mut || {
                            deadline.route_should_stop()
                        })
                    })
                })
                .ok()
        });
        let narrow_worker = run_narrow.then(|| {
            thread::Builder::new()
                .name("columbo-no-split".into())
                .spawn_scoped(scope, || {
                    run_route_with_cancellation(deadline, || {
                        build_narrow_source_candidate(source, options, &mut || {
                            deadline.route_should_stop()
                        })
                    })
                })
                .ok()
        });

        // A route error (or unwind) asks its siblings to stop at their next
        // ordinary deadline check. Join every successfully spawned worker
        // before choosing the fixed deft4j/narrow/floor error order below.
        let floor = run_route_with_cancellation(deadline, || {
            build_bounded_floor_candidate(source, options, &mut || deadline.route_should_stop())
        });
        let deft4j = match deft4j_worker.flatten() {
            Some(worker) => worker.join(),
            None if run_deft4j => Ok(run_route_with_cancellation(deadline, || {
                build_deft4j_source_candidate(source, options, &mut || deadline.route_should_stop())
            })),
            None => Ok(Ok(None)),
        };
        let narrow = match narrow_worker.flatten() {
            Some(worker) => worker.join(),
            None if run_narrow => Ok(run_route_with_cancellation(deadline, || {
                build_narrow_source_candidate(source, options, &mut || deadline.route_should_stop())
            })),
            None => Ok(Ok(None)),
        };

        // A panic still denotes an internal invariant failure. Both joins are
        // complete now, so resuming it cannot strand a sibling worker.
        let deft4j = match deft4j {
            Ok(result) => result,
            Err(payload) => std::panic::resume_unwind(payload),
        }?;
        let narrow = match narrow {
            Ok(result) => result,
            Err(payload) => std::panic::resume_unwind(payload),
        }?;
        let floor = floor?;
        Ok((floor, deft4j, narrow))
    })
}

/// Retain the deft4j/narrow/floor deadline order without overlapping arenas.
fn build_bounded_phase_candidates_sequential(
    source: CandidateInput<'_>,
    options: &Options,
    run_deft4j: bool,
    run_narrow: bool,
    deadline: &Deadline,
) -> Result<(Candidate, Option<Candidate>, Option<Candidate>)> {
    let deft4j = if run_deft4j {
        build_deft4j_source_candidate(source, options, &mut || deadline.expired())?
    } else {
        None
    };
    let narrow = if run_narrow {
        build_narrow_source_candidate(source, options, &mut || deadline.expired())?
    } else {
        None
    };
    let floor = build_bounded_floor_candidate(source, options, &mut || deadline.expired())?;
    Ok((floor, deft4j, narrow))
}

fn parallel_route_is_bounded(source: CandidateInput<'_>) -> bool {
    let Some(token_count) = source.blocks.iter().try_fold(0_usize, |total, block| {
        total.checked_add(block.tokens.len())
    }) else {
        return false;
    };
    parallel_route_sizes_are_bounded(
        source.compressed.len(),
        source.identity.decoded_size,
        token_count,
        source.blocks.len(),
    )
}

fn parallel_route_sizes_are_bounded(
    compressed_bytes: usize,
    decoded_bytes: u64,
    token_count: usize,
    block_count: usize,
) -> bool {
    if compressed_bytes > PARALLEL_ROUTE_MAX_COMPRESSED
        || decoded_bytes > PARALLEL_ROUTE_MAX_DECODED
    {
        return false;
    }
    usize::try_from(decoded_bytes)
        .ok()
        .and_then(|decoded| parsed_model_bytes(decoded, token_count, block_count))
        .is_some_and(|model| model <= PARALLEL_ROUTE_MAX_MODEL)
}

/// Request sibling cancellation when a route returns an error or unwinds.
fn run_route_with_cancellation<T>(
    deadline: &Deadline,
    route: impl FnOnce() -> Result<T>,
) -> Result<T> {
    struct CancelOnFailure<'a> {
        deadline: &'a Deadline,
        succeeded: bool,
    }

    impl Drop for CancelOnFailure<'_> {
        fn drop(&mut self) {
            if !self.succeeded {
                self.deadline.cancel_routes();
            }
        }
    }

    let mut guard = CancelOnFailure {
        deadline,
        succeeded: false,
    };
    let result = route();
    guard.succeeded = result.is_ok();
    result
}

fn build_bounded_floor_candidate<F>(
    source: CandidateInput<'_>,
    options: &Options,
    expired: &mut F,
) -> Result<Candidate>
where
    F: FnMut() -> bool,
{
    let mut floor_options = options.clone();
    floor_options.exhaustive = false;
    build_candidate(source, &floor_options, DEFAULT_RAW_REPLAY_LIMIT, expired)
}

/// Build a bounded, source-ordered deft4j candidate without replaying it
/// through the broader planner.
///
/// Its fixed-point work remains subject to the caller's Columbo deadline and
/// memory policies. Any later cross-route replay is scheduled explicitly by
/// the caller.
fn build_deft4j_source_candidate<F>(
    source: CandidateInput<'_>,
    options: &Options,
    expired: &mut F,
) -> Result<Option<Candidate>>
where
    F: FnMut() -> bool,
{
    let Some(plans) = plan_source_blocks(source.blocks, 0, options, &mut *expired) else {
        return Ok(None);
    };
    build_candidate_from_plans(source, plans, options, 0, ReplayPlanner::Full, expired).map(Some)
}

/// Build one direct source-order candidate without the broader split graph.
///
/// This mirrors the useful no-split route family while sharing the caller's
/// parsed payloads and deadline. It is intentionally not replayed through
/// `plan_stream`; doing so would immediately repeat the grouping and split
/// work this sibling exists to bypass.
fn build_narrow_source_candidate<F>(
    source: CandidateInput<'_>,
    options: &Options,
    expired: &mut F,
) -> Result<Option<Candidate>>
where
    F: FnMut() -> bool,
{
    let Some(plans) = plan_source_no_split_route(source.blocks, 0, options, true, &mut *expired)
    else {
        return Ok(None);
    };
    build_candidate_from_plans(source, plans, options, 0, ReplayPlanner::Full, expired).map(Some)
}

/// Apply the source-ordered deft4j route to a genuinely rewritten floor state.
///
/// Weak gains from the original deft4j route can indicate that Columbo's
/// ordinary bounded floor is the more useful parent in its route graph.
/// Reparse that encoded incumbent once, validate its identity, and keep the
/// seeded deft4j walk additive to the original deft4j candidate rather than
/// replaying duplicate work on the original blocks.
fn build_deft4j_seed_candidate<F>(
    candidate: &Candidate,
    options: &Options,
    decoded_limit: u64,
    identity: StreamIdentity,
    allow_single_block: bool,
    expired: &mut F,
) -> Result<Option<Candidate>>
where
    F: FnMut() -> bool,
{
    let stream = parse_stream(&candidate.data, decoded_limit)?;
    validate_replayed_stream(&stream, candidate.data.len(), identity)?;
    if !deft4j_source_route_eligible(&stream.blocks, candidate.data.len(), allow_single_block) {
        return Ok(None);
    }
    let source = CandidateInput {
        compressed: &candidate.data,
        blocks: &stream.blocks,
        meaningful_bits: candidate.bits,
        decoded_limit,
        identity,
    };
    build_deft4j_source_candidate(source, options, expired)
}

/// Apply the ordinary planner to boundaries and token spellings produced by
/// the source-ordered deft4j route while the same container deadline has room.
///
/// This is not duplicate source work: the reparsed deft4j candidate contains
/// merged blocks and expanded token states that the original normal floor
/// cannot see. The original floor remains an independent comparison below.
fn refine_with_default_planner<F>(
    candidate: &Candidate,
    options: &Options,
    decoded_limit: u64,
    identity: StreamIdentity,
    expired: &mut F,
) -> Result<Candidate>
where
    F: FnMut() -> bool,
{
    let stream = parse_stream(&candidate.data, decoded_limit)?;
    validate_replayed_stream(&stream, candidate.data.len(), identity)?;
    let source = CandidateInput {
        compressed: &candidate.data,
        blocks: &stream.blocks,
        meaningful_bits: candidate.bits,
        decoded_limit,
        identity,
    };
    let mut floor_options = options.clone();
    floor_options.exhaustive = false;
    build_candidate(source, &floor_options, DEFAULT_RAW_REPLAY_LIMIT, expired)
}

/// Apply one linear, deterministic merge cleanup to a selected max seed.
///
/// The timed route may end immediately after producing a substantially better
/// block list. Reparse that completed stream and price only adjacent merges
/// with bounded table floors. If time remains, at most two ordinary replays
/// may stabilize the smaller list. Optional probes observe the caller's
/// deadline; complete candidate emission and validation still finish.
fn refine_with_terminal_merge<F>(
    candidate: &Candidate,
    options: &Options,
    decoded_limit: u64,
    identity: StreamIdentity,
    expired: &mut F,
) -> Result<Option<Candidate>>
where
    F: FnMut() -> bool,
{
    if expired() {
        return Ok(None);
    }
    let stream = parse_stream(&candidate.data, decoded_limit)?;
    validate_replayed_stream(&stream, candidate.data.len(), identity)?;
    let mut floor_options = options.clone();
    floor_options.exhaustive = false;
    let Some(plans) = plan_terminal_merge_route(&stream.blocks, 0, &floor_options, expired) else {
        return Ok(None);
    };
    let source = CandidateInput {
        compressed: &candidate.data,
        blocks: &stream.blocks,
        meaningful_bits: candidate.bits,
        decoded_limit,
        identity,
    };
    build_candidate_from_plans(
        source,
        plans,
        &floor_options,
        2,
        ReplayPlanner::Full,
        expired,
    )
    .map(Some)
}

/// Build one complete-stream candidate and follow strict improvements through
/// a small number of reparse/replan rounds.
fn build_candidate<F>(
    source: CandidateInput<'_>,
    options: &Options,
    replay_limit: usize,
    expired: &mut F,
) -> Result<Candidate>
where
    F: FnMut() -> bool,
{
    let Some(plans) = plan_stream(source.blocks, 0, options, &mut *expired) else {
        return source_candidate(source, options);
    };
    build_candidate_from_plans(
        source,
        plans,
        options,
        replay_limit,
        ReplayPlanner::Full,
        expired,
    )
}

#[derive(Clone, Copy)]
enum ReplayPlanner {
    Full,
    Fragmented,
}

/// Emit an explicitly selected structural seed and stabilize strict replays.
///
/// Most callers obtain their first plans from [`plan_stream`]. Alternate
/// collection strategies intentionally need to preserve a locally dearer seed,
/// so accepting the initial plan list here prevents the ordinary planner from
/// replacing it before its new block boundaries have been reparsed.
fn build_candidate_from_plans<F>(
    source: CandidateInput<'_>,
    mut plans: Vec<PlannedBlock>,
    options: &Options,
    replay_limit: usize,
    replay_planner: ReplayPlanner,
    expired: &mut F,
) -> Result<Candidate>
where
    F: FnMut() -> bool,
{
    let (mut data, mut bits) = emit_plans(source.compressed, &plans)?;
    let initial_loses = !is_strictly_better(
        data.len(),
        bits,
        source.compressed.len(),
        source.meaningful_bits,
    );

    // A merged stream has token and block boundaries which did not exist in
    // its input. Re-parsing makes those boundaries available to the next
    // planning round. Each accepted round must improve the complete stream,
    // so this cannot oscillate even before the explicit cap is reached.
    for _ in 0..replay_limit {
        if (initial_loses && !options.min_distance_codes) || expired() {
            break;
        }

        let replayed = parse_stream(&data, source.decoded_limit)?;
        validate_replayed_stream(&replayed, data.len(), source.identity)?;

        let replay_plans = match replay_planner {
            ReplayPlanner::Full => plan_stream(&replayed.blocks, 0, options, &mut *expired),
            ReplayPlanner::Fragmented => plan_fragmented_replay(&replayed.blocks, 0, options),
        };
        let Some(replay_plans) = replay_plans else {
            break;
        };
        let (replay_data, replay_bits) = emit_plans(&data, &replay_plans)?;
        if !is_strictly_better(replay_data.len(), replay_bits, data.len(), bits) {
            break;
        }
        data = replay_data;
        bits = replay_bits;
        plans = replay_plans;
    }

    // The last accepted replay is not necessarily followed by another loop
    // iteration: the replay cap or deadline may stop immediately after it.
    // Validate that final selectable stream as well. A known-losing generated
    // stream is skipped because the caller will retain the already-validated
    // source bytes instead.
    if !initial_loses || options.min_distance_codes {
        let completed = parse_stream(&data, source.decoded_limit)?;
        validate_replayed_stream(&completed, data.len(), source.identity)?;
    }

    Ok(Candidate { data, bits, plans })
}

/// Retain a parsed source when the mandatory plan list cannot be allocated.
/// Compatibility mode must rewrite distance alphabets, so it reports the
/// allocation failure rather than silently ignoring the requested transform.
fn source_candidate(source: CandidateInput<'_>, options: &Options) -> Result<Candidate> {
    if options.min_distance_codes {
        return Err(Error::new("could not allocate Deflate plan"));
    }
    let mut data = Vec::new();
    data.try_reserve_exact(source.compressed.len())
        .map_err(|_| Error::new("could not allocate Deflate output"))?;
    data.extend_from_slice(source.compressed);
    Ok(Candidate {
        data,
        bits: source.meaningful_bits,
        plans: Vec::new(),
    })
}

/// Verify every generated stream before using it as input to another round.
///
/// These checks used to be debug assertions. Keeping them in release builds is
/// inexpensive—the parser has already computed every value—and makes an
/// emitter or planner bug fail closed instead of allowing changed decoded data
/// to become the next optimization seed.
fn validate_replayed_stream(
    stream: &ParsedStream,
    expected_bytes: usize,
    identity: StreamIdentity,
) -> Result<()> {
    if stream.consumed != expected_bytes
        || stream.decoded_size != identity.decoded_size
        || stream.crc32 != identity.crc32
        || stream.adler32 != identity.adler32
    {
        return Err(Error::new(
            "internal error: rewritten Deflate stream changed decoded data",
        ));
    }
    Ok(())
}

fn emit_plans(input: &[u8], plans: &[PlannedBlock]) -> Result<(Vec<u8>, u64)> {
    let mut writer = BitWriter::default();
    for (index, plan) in plans.iter().enumerate() {
        emit_block(&mut writer, input, plan, index + 1 == plans.len())?;
    }
    let bits = writer.bit_position();
    Ok((writer.into_bytes(), bits))
}

fn is_strictly_better(
    candidate_bytes: usize,
    candidate_bits: u64,
    reference_bytes: usize,
    reference_bits: u64,
) -> bool {
    candidate_bytes < reference_bytes
        || (candidate_bytes == reference_bytes && candidate_bits < reference_bits)
}

fn inspect_plans(plans: &[PlannedBlock]) {
    let mut alignment = 0_u8;
    for (index, plan) in plans.iter().enumerate() {
        inspect_plan(index, alignment, index + 1 == plans.len(), plan);
        alignment = ((u64::from(alignment) + plan.bits) & 7) as u8;
    }
}

fn inspect_plan(index: usize, alignment: u8, final_block: bool, plan: &PlannedBlock) {
    let output = match &plan.representation {
        Representation::Original(_) => "original",
        Representation::Stored => "stored",
        Representation::Fixed => "fixed",
        Representation::Dynamic(_) => "dynamic",
    };
    let input = match plan.source_type {
        SourceBlockType::Stored => "stored",
        SourceBlockType::Fixed => "fixed",
        SourceBlockType::Dynamic => "dynamic",
    };
    eprintln!(
        "inspect: block={index} final={} align={alignment} input={input} plain={} tokens={} output={output} output_bits={}",
        usize::from(final_block),
        plan.plain.len(),
        plan.tokens.len(),
        plan.bits,
    );
}

const DEADLINE_TIMED_OUT: u8 = 1;
const DEADLINE_ROUTES_CANCELLED: u8 = 2;

struct Deadline {
    started: Instant,
    duration: std::time::Duration,
    // Timeout and route-cancellation flags share one atomic so a hot search
    // probe performs only one synchronization load. The flags carry no data,
    // so relaxed ordering is sufficient.
    state: AtomicU8,
}

impl Deadline {
    fn expired(&self) -> bool {
        if self.state.load(Ordering::Relaxed) & DEADLINE_TIMED_OUT != 0
            || self.started.elapsed() >= self.duration
        {
            self.state.fetch_or(DEADLINE_TIMED_OUT, Ordering::Relaxed);
            true
        } else {
            false
        }
    }

    fn route_should_stop(&self) -> bool {
        if self.state.load(Ordering::Relaxed) != 0 {
            return true;
        }
        if self.started.elapsed() >= self.duration {
            self.state.fetch_or(DEADLINE_TIMED_OUT, Ordering::Relaxed);
            return true;
        }
        false
    }

    fn cancel_routes(&self) {
        // `fetch_or` cannot erase a timeout racing with this route failure.
        self.state
            .fetch_or(DEADLINE_ROUTES_CANCELLED, Ordering::Relaxed);
    }

    fn was_triggered(&self) -> bool {
        self.state.load(Ordering::Relaxed) & DEADLINE_TIMED_OUT != 0
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;
    use crate::deflate::bitstream::BitWriter;
    use crate::deflate::huffman::fixed_trees;
    use crate::deflate::model::Token;

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
    fn bounded_max_floor_still_returns_a_complete_stream() {
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
            DefaultFloor::Bounded,
        )
        .unwrap();
        assert!(optimized.timed_out);
        assert_eq!(optimized.consumed, input.len());

        let reparsed = parse_stream(&optimized.data, 1).unwrap();
        assert_eq!(reparsed.consumed, optimized.data.len());
        assert_eq!(reparsed.decoded_size, 0);
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
            DefaultFloor::Bounded,
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
    fn route_errors_cancel_siblings_without_marking_a_timeout() {
        let deadline = Deadline {
            started: Instant::now(),
            duration: Duration::MAX,
            state: AtomicU8::new(0),
        };
        let failed: Result<()> =
            run_route_with_cancellation(&deadline, || Err(Error::new("synthetic route failure")));

        assert!(failed.is_err());
        assert!(deadline.route_should_stop());
        assert!(!deadline.was_triggered());

        let successful = Deadline {
            started: Instant::now(),
            duration: Duration::MAX,
            state: AtomicU8::new(0),
        };
        let completed: Result<()> = run_route_with_cancellation(&successful, || Ok(()));
        assert!(completed.is_ok());
        assert!(!successful.route_should_stop());
    }

    #[test]
    fn weak_deft4j_threshold_is_strictly_below_two_percent() {
        assert!(deft4j_gain_is_below(10_000, 9_801, 200));
        assert!(!deft4j_gain_is_below(10_000, 9_800, 200));
        assert!(!deft4j_gain_is_below(10_000, 9_000, 200));
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
}
