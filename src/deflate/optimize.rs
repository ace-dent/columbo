// SPDX-License-Identifier: MIT

use std::time::Instant;

use crate::{Error, Options, Result};

use super::bitstream::BitWriter;
use super::block::emit_block;
use super::model::{ParsedBlock, ParsedStream, PlannedBlock, Representation, SourceBlockType};
use super::parse::parse_stream;
use super::stream::{fragmented_collect_seed, plan_fragmented_replay, plan_stream};

// Long source-block chains can need one pass to establish profitable adjacent
// groups, then two inexpensive passes over that much simpler block layout to
// settle their boundaries and tables. Every round below must strictly improve
// the complete stream, so the extra slot cannot oscillate or grow the output.
const DEFAULT_RAW_REPLAY_LIMIT: usize = 3;
const MAX_RAW_REPLAY_LIMIT: usize = 8;

/// Facts collected while decoding the source stream.  Container handlers use
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
/// an optimization found by normal mode. Containers such as PNG and ZIP give
/// each embedded stream a slice of the file-wide deadline; those local calls
/// use [`DefaultFloor::Bounded`] so one member cannot overrun every later one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DefaultFloor {
    Complete,
    Bounded,
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
    let mut deadline = Deadline {
        started,
        duration: options.timeout,
        triggered: false,
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

    let mut candidate = if options.exhaustive {
        // Max mode starts from the genuine normal-mode result. Standalone
        // streams finish that floor so --max cannot lose to normal mode;
        // locally scheduled container streams keep it within their allotted
        // time. Either way, elapsed floor time advances the same deadline.
        let mut floor_options = options.clone();
        floor_options.exhaustive = false;
        let source = CandidateInput {
            compressed: original,
            blocks: &blocks,
            meaningful_bits: parsed.meaningful_bits,
            decoded_limit,
            identity,
        };
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
            DefaultFloor::Bounded => {
                let mut floor_expired = || deadline.expired();
                build_candidate(
                    source,
                    &floor_options,
                    DEFAULT_RAW_REPLAY_LIMIT,
                    &mut floor_expired,
                )?
            }
        }
    } else {
        let source = CandidateInput {
            compressed: original,
            blocks: &blocks,
            meaningful_bits: parsed.meaningful_bits,
            decoded_limit,
            identity,
        };
        build_candidate(source, options, DEFAULT_RAW_REPLAY_LIMIT, &mut || {
            deadline.expired()
        })?
    };

    // A 4,096-token collection can start slightly larger but converge to a
    // better fragmented-stream layout after strict replays. It is additive to
    // the normal-mode comparison floor above: a short container slice must not
    // spend its whole budget here before securing that floor. The collection
    // seed itself remains deadline-independent and can still win when there is
    // no time left for its optional replay rounds.
    let mut fragmented_candidate = if options.exhaustive {
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

        let floor_selected = options.min_distance_codes
            || is_strictly_better(
                candidate.data.len(),
                candidate.bits,
                original.len(),
                parsed.meaningful_bits,
            );
        if floor_selected && !deadline.expired() {
            // Several max transformations become visible only after the
            // default pass has merged blocks or rewritten match choices. Run
            // that finished stream as an additive seed before restarting max
            // from the original source. The owned floor remains available if
            // this route times out or fails to improve it.
            let floor_stream = parse_stream(&candidate.data, decoded_limit)?;
            validate_replayed_stream(&floor_stream, candidate.data.len(), identity)?;
            let source = CandidateInput {
                compressed: &candidate.data,
                blocks: &floor_stream.blocks,
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

        if !deadline.expired() {
            let source = CandidateInput {
                compressed: original,
                blocks: &blocks,
                meaningful_bits: parsed.meaningful_bits,
                decoded_limit,
                identity,
            };
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
        timed_out: deadline.triggered,
    })
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

struct Deadline {
    started: Instant,
    duration: std::time::Duration,
    triggered: bool,
}

impl Deadline {
    fn expired(&mut self) -> bool {
        if self.triggered || self.started.elapsed() >= self.duration {
            self.triggered = true;
        }
        self.triggered
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;
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
