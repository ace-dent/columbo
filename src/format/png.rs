// SPDX-License-Identifier: MIT

//! PNG/APNG stream scheduling and metadata-preserving rebuilding.

mod chunks;

use std::thread;
use std::time::{Duration, Instant};

use crate::deflate::{
    decoded_bytes_for_comparison, raw_source_benefits_from_early_max_lineage,
    raw_stream_decodes_to, DefaultFloor, RawInfo,
};
use crate::{Error, ErrorKind, Optimization, Options, Result};

use super::{scale_duration, zlib, SearchDeadline};
pub(super) use chunks::ParsedPng;
use chunks::{append_chunk, compressed_zlib_offset, parse, should_strip_kind, Chunk, SIGNATURE};

/// Exact cross-frame reuse is optional. Bounding the retained comparison bytes
/// keeps an APNG with many very large frames from doubling its memory use.
const MAX_EXACT_REUSE_BYTES: usize = 32 * 1024 * 1024;
const MAX_EXACT_REUSE_WORK_BYTES: u64 = 64 * 1024 * 1024;
const MAX_METADATA_PROBE_WORK_BYTES: u64 = 64 * 1024 * 1024;
/// Max must not discard affordable Default savings in compressed metadata just
/// because an image route spends the shared file deadline first. Precompute the
/// complete non-Max metadata floors when their aggregate compressed size fits
/// the same bounded work class as speculative metadata probes, then reuse those
/// results during reconstruction. Larger metadata sets retain the streaming
/// schedule so caching cannot duplicate an attacker-sized portion of the input.
const MAX_CACHED_METADATA_FLOOR_BYTES: u64 = MAX_METADATA_PROBE_WORK_BYTES;
/// A single-image Max run can retain exact Default and direct-Max raw models
/// together. Keep that combined working set within the same broad bounds used
/// for independent raw-route arenas.
const PARALLEL_MAX_IMAGE_COMPRESSED: usize = 8 * 1024 * 1024;
const PARALLEL_MAX_IMAGE_DECODED: u64 = 64 * 1024 * 1024;
const PARALLEL_MAX_IMAGE_WORKERS: usize = 8;
/// A fifth is enough to finish the inexpensive parent on representative image
/// streams while reserving most of Max for the descendants that need it. This
/// is the same follow-up reservation used by Deflate's bounded route schedule.
const QUICK_FLOOR_SEARCH_FRACTION: f64 = 0.20;

struct PngOptimization {
    data: Vec<u8>,
    source_deflate_bits: u64,
    output_deflate_bits: u64,
    timed_out: bool,
}

impl PngOptimization {
    fn dominates(&self, floor: &Self) -> bool {
        self.data.len() <= floor.data.len() && self.output_deflate_bits <= floor.output_deflate_bits
    }

    fn into_public(self, source_bytes: usize) -> Optimization {
        Optimization::from_metrics(
            source_bytes,
            self.data,
            self.source_deflate_bits,
            self.output_deflate_bits,
            self.timed_out,
        )
    }
}

fn retain_dominating_maximum(
    floor: PngOptimization,
    maximum: PngOptimization,
    overall_timed_out: bool,
) -> PngOptimization {
    let timed_out = floor.timed_out || maximum.timed_out || overall_timed_out;
    let mut selected = if maximum.dominates(&floor) {
        maximum
    } else {
        floor
    };
    selected.timed_out = timed_out;
    selected
}

struct DecodeBudget {
    remaining: u64,
    deadline: SearchDeadline,
}

#[derive(Clone, Debug)]
struct FrameOptimization {
    data: Vec<u8>,
    info: Option<RawInfo>,
}

#[derive(Clone, Debug)]
struct CompressedBodyOptimization {
    replacement: Option<Vec<u8>>,
    source_deflate_bits: u64,
    output_deflate_bits: u64,
    decoded_size: Option<u64>,
}

fn compressed_body_len(candidate: &CompressedBodyOptimization, source_len: usize) -> usize {
    candidate.replacement.as_ref().map_or(source_len, Vec::len)
}

fn compressed_body_strictly_dominates(
    candidate: &CompressedBodyOptimization,
    floor: &CompressedBodyOptimization,
    source_len: usize,
) -> bool {
    let candidate_len = compressed_body_len(candidate, source_len);
    let floor_len = compressed_body_len(floor, source_len);
    candidate_len <= floor_len
        && candidate.output_deflate_bits <= floor.output_deflate_bits
        && (candidate_len < floor_len || candidate.output_deflate_bits < floor.output_deflate_bits)
}

fn empty_chunk_replacements(chunk_count: usize) -> Result<Vec<Option<CompressedBodyOptimization>>> {
    let mut replacements = Vec::new();
    replacements
        .try_reserve_exact(chunk_count)
        .map_err(|_| Error::internal("could not allocate PNG chunk model"))?;
    replacements.resize_with(chunk_count, || None);
    Ok(replacements)
}

pub(super) fn deflate_stream_count(input: &[u8], strip_metadata: bool) -> Result<usize> {
    let parsed = preflight(input, strip_metadata)?;
    stream_count(&parsed)
}

pub(super) fn preflight(input: &[u8], strip_metadata: bool) -> Result<ParsedPng<'_>> {
    parse(input, strip_metadata)
}

pub(super) fn stream_count(parsed: &ParsedPng<'_>) -> Result<usize> {
    let compressed_metadata = parsed
        .chunks
        .iter()
        .filter(|chunk| supported_compressed_metadata(chunk))
        .count();
    1_usize
        .checked_add(parsed.fdat_frames.len())
        .and_then(|count| count.checked_add(compressed_metadata))
        .ok_or_else(|| Error::resource_limit("too many PNG Deflate streams"))
}

pub(super) fn optimize_preflight(
    input: &[u8],
    options: &Options,
    parsed: ParsedPng<'_>,
) -> Result<Optimization> {
    // A multi-image APNG splits Max's wall budget across independent image
    // streams. Per-stream bounded floors cannot guarantee the same fixed point
    // as the complete Default file pipeline: a small but complex frame may
    // exhaust its proportional slice even when Default finishes the whole file
    // before Max's deadline. Race that complete file result against an
    // original-source Max pass with the full allowance, then retain Max only
    // when the complete file is no worse in both bytes and aggregate
    // meaningful Deflate bits. This keeps the quality floor without deducting
    // Default's elapsed time from Max's search budget.
    if options.exhaustive
        && !parsed.fdat_frames.is_empty()
        && !options.timeout.is_zero()
        && (options.strip_metadata || !parsed.has_rewrite_sensitive_ancillary)
        && parallel_apng_file_floor_is_bounded(&parsed)?
    {
        let started = Instant::now();
        let floor_parsed = parse(input, options.strip_metadata)?;
        let mut floor_options = options.clone();
        floor_options.exhaustive = false;
        floor_options.verbose = false;
        floor_options.visual = false;

        return thread::scope(|scope| {
            let floor_worker = thread::Builder::new()
                .name("columbo-apng-default-floor".into())
                .spawn_scoped(scope, move || {
                    optimize_preflight_once(input, &floor_options, floor_parsed)
                });

            let maximum = optimize_preflight_once(input, options, parsed);
            let floor = match floor_worker {
                Ok(worker) => match worker.join() {
                    Ok(result) => result,
                    Err(payload) => std::panic::resume_unwind(payload),
                },
                // Thread creation failure is not an optimization failure.
                // Preserve both quality contracts serially in this exceptional
                // path; Max still receives its complete configured allowance.
                Err(_) => {
                    let floor_parsed = parse(input, options.strip_metadata)?;
                    let mut floor_options = options.clone();
                    floor_options.exhaustive = false;
                    floor_options.verbose = false;
                    floor_options.visual = false;
                    optimize_preflight_once(input, &floor_options, floor_parsed)
                }
            }?;
            let maximum = maximum?;
            let selected =
                retain_dominating_maximum(floor, maximum, started.elapsed() >= options.timeout);
            Ok(selected.into_public(input.len()))
        });
    }

    optimize_preflight_once(input, options, parsed).map(|result| result.into_public(input.len()))
}

/// Whether a second complete APNG model may safely overlap Max.
///
/// Reuse the image-worker input and decoded-work bounds because the sibling
/// owns another parsed compressed model and another complete output. A
/// single-core process keeps the historical Max route instead of forcing two
/// deadline-sensitive whole-file searches to time-slice one CPU.
fn parallel_apng_file_floor_is_bounded(parsed: &ParsedPng<'_>) -> Result<bool> {
    if thread::available_parallelism().map_or(true, |parallelism| parallelism.get() < 2) {
        return Ok(false);
    }
    let total_compressed = parsed
        .fdat_frames
        .iter()
        .try_fold(parsed.idat.len(), |total, frame| {
            total.checked_add(frame.len())
        })
        .ok_or_else(|| Error::resource_limit("PNG frame data is too large"))?;
    let total_decoded =
        total_image_decoded_bytes(parsed.idat_decoded_size, &parsed.fdat_decoded_sizes)?;
    Ok(parallel_multi_image_is_bounded(
        total_compressed,
        total_decoded,
    ))
}

fn optimize_preflight_once(
    input: &[u8],
    options: &Options,
    parsed: ParsedPng<'_>,
) -> Result<PngOptimization> {
    let datastream_len = parsed.datastream_len;
    let mut budget = DecodeBudget {
        remaining: options.max_decoded_bytes,
        deadline: SearchDeadline::new(options),
    };
    let metadata_stream_ids = metadata_stream_ids(&parsed)?;

    // Small compressed metadata gets a short first pass in the original
    // Columbo C implementation so a profile or text comment cannot consume the
    // image stream's search time. If that pass finds no reduction,
    // reconstruction gives it one normal pass.
    let mut quick_replacements = empty_chunk_replacements(parsed.chunks.len())?;
    let mut probe_work_remaining = MAX_METADATA_PROBE_WORK_BYTES;
    for (index, chunk) in parsed.chunks.iter().enumerate() {
        if options.exhaustive {
            continue;
        }
        if should_strip(chunk.kind, options) {
            continue;
        }
        let Some(offset) = compressed_zlib_offset(chunk.kind, chunk.data) else {
            continue;
        };
        if chunk.data.len() - offset > 4_096 {
            continue;
        }

        // Quick probes are optional and may be retried. Give them a separate,
        // monotonically decreasing compressed+decoded work allowance so a
        // long list of non-winning metadata streams cannot repeatedly consume
        // the complete file expansion budget.
        let compressed_work = (chunk.data.len() - offset) as u64;
        if compressed_work > probe_work_remaining {
            continue;
        }
        probe_work_remaining -= compressed_work;

        let mut quick_options = options.clone();
        quick_options.timeout = quick_options.timeout.min(Duration::from_millis(100));
        let remaining_before_probe = budget.remaining;
        let probe_allowance = remaining_before_probe.min(probe_work_remaining);
        budget.remaining = probe_allowance;
        let mut run_probe = || {
            optimize_compressed_body(
                chunk.kind,
                chunk.data,
                &quick_options,
                DefaultFloor::Shared,
                &mut budget,
            )
        };
        let probe = if options.verbose || options.visual {
            let stream_id = metadata_stream_ids[index]
                .expect("every supported compressed metadata chunk has a stream identifier");
            crate::progress::with_stream_slice(stream_id, &[], Some("metadata probe"), run_probe)
        } else {
            run_probe()
        };
        let decoded_work = probe_allowance.saturating_sub(budget.remaining);
        probe_work_remaining = probe_work_remaining.saturating_sub(decoded_work);
        let replacement = match probe {
            Ok(replacement) => replacement,
            Err(error) if error.kind() == ErrorKind::ResourceLimit => {
                // The lower-level decoder cannot report partial decoded bytes
                // on this error. Conservatively spend the rest of the optional
                // probe allowance so a second tiny high-expansion stream cannot
                // repeat the same near-limit decode.
                probe_work_remaining = 0;
                budget.remaining = remaining_before_probe;
                continue;
            }
            Err(error) => return Err(error),
        };
        quick_replacements[index] =
            replacement.filter(|replacement| replacement.replacement.is_some());

        // A non-winning quick probe is retried later with the stream's normal
        // search allowance. Charge its decoded bytes only on that definitive
        // pass; otherwise the same metadata stream would consume the global
        // expansion budget twice. The 100 ms probe also has a local deadline,
        // so it must not report a file-wide timeout by itself.
        if quick_replacements[index].is_none() {
            budget.remaining = remaining_before_probe;
        } else {
            budget.remaining = remaining_before_probe.saturating_sub(decoded_work);
            if options.verbose || options.visual {
                let stream_id = metadata_stream_ids[index]
                    .expect("every supported compressed metadata chunk has a stream identifier");
                crate::progress::complete_stream_group(stream_id, &[]);
            }
        }
    }

    let metadata_compressed_bytes = compressed_metadata_bytes(&parsed, options);
    let parallel_metadata_floor = options.exhaustive
        // APNG already parallelizes bounded independent image streams. Avoid
        // nesting another worker layer, which would compete with those image
        // routes and make deadline-sensitive quality less predictable.
        && parsed.fdat_frames.is_empty()
        && metadata_compressed_bytes
            .is_some_and(|bytes| bytes != 0 && bytes <= MAX_CACHED_METADATA_FLOOR_BYTES);

    let (optimized_idat, optimized_frames) = if parallel_metadata_floor {
        // Image data and compressed metadata are independent Deflate streams.
        // Building their mandatory Max-mode Default floors serially lets a
        // tiny profile consume a material part of a short image allowance.
        // Run the image work beside the bounded metadata-floor pass instead.
        // Reserve the images' exact decoded size from the metadata worker's
        // budget up front, so concurrency cannot weaken the file-wide safety
        // limit or charge a physical stream twice.
        let image_decoded_bytes =
            total_image_decoded_bytes(parsed.idat_decoded_size, &parsed.fdat_decoded_sizes)?;
        if image_decoded_bytes > budget.remaining {
            return Err(Error::resource_limit(
                "decoded PNG data exceeds configured safety limit",
            ));
        }
        let image_budget_bytes = budget.remaining;
        // Copy the container's clock, not merely its configured duration. A
        // fresh deadline here would give the worker back time already spent
        // parsing and probing metadata, weakening the file-wide timeout.
        let image_deadline = budget.deadline;

        thread::scope(|scope| -> Result<_> {
            // Keep the dominant image search on the caller thread. Besides
            // avoiding a hand-off for the largest stream, this preserves warm
            // parser and route state while the smaller ancillary floor moves
            // to the worker.
            let metadata_budget_bytes = image_budget_bytes - image_decoded_bytes;
            let parsed_ref = &parsed;
            let metadata_stream_ids_ref = &metadata_stream_ids;
            let metadata_worker = match thread::Builder::new()
                .name("columbo-png-metadata-floor".into())
                .spawn_scoped(scope, move || -> Result<_> {
                    let mut metadata_budget = DecodeBudget {
                        remaining: metadata_budget_bytes,
                        deadline: image_deadline,
                    };
                    let mut metadata_floors = empty_chunk_replacements(parsed_ref.chunks.len())?;
                    precompute_max_metadata_floors(
                        parsed_ref,
                        metadata_stream_ids_ref,
                        options,
                        &mut metadata_budget,
                        &mut metadata_floors,
                    )?;
                    Ok((metadata_budget, metadata_floors))
                }) {
                Ok(worker) => worker,
                Err(_) => {
                    precompute_max_metadata_floors(
                        &parsed,
                        &metadata_stream_ids,
                        options,
                        &mut budget,
                        &mut quick_replacements,
                    )?;
                    return optimize_image_streams(
                        &parsed.idat,
                        parsed.idat_decoded_size,
                        &parsed.fdat_frames,
                        &parsed.fdat_decoded_sizes,
                        image_work_needs_metadata_reserve(
                            options,
                            metadata_compressed_bytes.unwrap_or(0),
                        ),
                        options,
                        &mut budget,
                    );
                }
            };

            let mut image_budget = DecodeBudget {
                remaining: image_budget_bytes,
                deadline: image_deadline,
            };
            let image_result = optimize_image_streams(
                &parsed.idat,
                parsed.idat_decoded_size,
                &parsed.fdat_frames,
                &parsed.fdat_decoded_sizes,
                image_work_needs_metadata_reserve(options, metadata_compressed_bytes.unwrap_or(0)),
                options,
                &mut image_budget,
            );
            let metadata_result = match metadata_worker.join() {
                Ok(result) => result,
                Err(payload) => std::panic::resume_unwind(payload),
            };
            let (metadata_budget, metadata_floors) = metadata_result?;
            budget = metadata_budget;
            quick_replacements = metadata_floors;
            image_result
        })?
    } else {
        precompute_max_metadata_floors(
            &parsed,
            &metadata_stream_ids,
            options,
            &mut budget,
            &mut quick_replacements,
        )?;
        optimize_image_streams(
            &parsed.idat,
            parsed.idat_decoded_size,
            &parsed.fdat_frames,
            &parsed.fdat_decoded_sizes,
            image_work_needs_metadata_reserve(options, metadata_compressed_bytes.unwrap_or(0)),
            options,
            &mut budget,
        )?
    };
    let mut source_deflate_bits = frame_source_bits(&optimized_idat)?;
    let mut output_deflate_bits = frame_output_bits(&optimized_idat)?;
    for frame in &optimized_frames {
        source_deflate_bits = source_deflate_bits
            .checked_add(frame_source_bits(frame)?)
            .ok_or_else(|| Error::new("PNG Deflate bit count is too large"))?;
        output_deflate_bits = output_deflate_bits
            .checked_add(frame_output_bits(frame)?)
            .ok_or_else(|| Error::new("PNG Deflate bit count is too large"))?;
    }

    // A signature, iDOT, or unknown unsafe-to-copy ancillary chunk may depend
    // on the exact critical image representation. Columbo cannot update its
    // contract, so after validating every image stream preserve the complete
    // PNG datastream unless --strip explicitly removes that chunk. Bytes after
    // IEND are outside that datastream and are never retained.
    if !options.strip_metadata && parsed.has_rewrite_sensitive_ancillary {
        let data = try_clone_bytes(&input[..datastream_len])
            .ok_or_else(|| Error::internal("could not allocate PNG output"))?;
        return Ok(PngOptimization {
            data,
            source_deflate_bits,
            output_deflate_bits: source_deflate_bits,
            timed_out: budget.deadline.is_expired(),
        });
    }

    let mut output = Vec::new();
    output
        .try_reserve_exact(datastream_len)
        .map_err(|_| Error::internal("could not allocate PNG output"))?;
    output.extend_from_slice(SIGNATURE);
    let mut idat_written = false;
    let mut frame_index = 0_usize;
    let mut frame_written = false;
    let mut animation_sequence = 0_u32;

    for (index, chunk) in parsed.chunks.iter().enumerate() {
        if chunk.discard_on_output || should_strip(chunk.kind, options) {
            continue;
        }

        match &chunk.kind {
            b"IDAT" => {
                if !idat_written {
                    // IDAT boundaries are only packetization. Coalescing them
                    // saves twelve bytes for every redundant chunk.
                    append_chunk(&mut output, *b"IDAT", &optimized_idat.data)?;
                    idat_written = true;
                }
            }
            b"fcTL" => {
                frame_written = false;
                let mut body = try_clone_bytes(chunk.data)
                    .ok_or_else(|| Error::internal("could not allocate APNG control chunk"))?;
                body[..4].copy_from_slice(&animation_sequence.to_be_bytes());
                animation_sequence += 1;
                append_chunk(&mut output, *b"fcTL", &body)?;
            }
            b"fdAT" => {
                if !frame_written {
                    let frame = optimized_frames
                        .get(frame_index)
                        .ok_or_else(|| Error::new("could not rebuild APNG frame"))?;
                    let body_len = frame
                        .data
                        .len()
                        .checked_add(4)
                        .ok_or_else(|| Error::new("APNG frame too large"))?;
                    let mut body = Vec::new();
                    body.try_reserve_exact(body_len)
                        .map_err(|_| Error::internal("could not allocate APNG frame"))?;
                    body.extend_from_slice(&animation_sequence.to_be_bytes());
                    body.extend_from_slice(&frame.data);
                    animation_sequence += 1;
                    append_chunk(&mut output, *b"fdAT", &body)?;
                    frame_index += 1;
                    frame_written = true;
                }
            }
            b"zTXt" | b"iTXt" | b"iCCP"
                if compressed_zlib_offset(chunk.kind, chunk.data).is_some() =>
            {
                let replacement = if let Some(replacement) = &quick_replacements[index] {
                    let mut refine_metadata = || {
                        refine_cached_compressed_body(
                            chunk.kind,
                            chunk.data,
                            replacement,
                            options,
                            &mut budget,
                        )
                    };
                    if let Some(stream_id) = metadata_stream_ids[index] {
                        crate::progress::with_stream_group(stream_id, &[], refine_metadata)?
                    } else {
                        refine_metadata()?
                    }
                } else {
                    let mut optimize_metadata = || {
                        let default_floor = metadata_default_floor(options.exhaustive);
                        optimize_compressed_body(
                            chunk.kind,
                            chunk.data,
                            options,
                            default_floor,
                            &mut budget,
                        )
                    };
                    if let Some(stream_id) = metadata_stream_ids[index] {
                        crate::progress::with_stream_group(stream_id, &[], optimize_metadata)?
                    } else {
                        optimize_metadata()?
                    }
                    .ok_or_else(|| Error::new("invalid compressed PNG metadata"))?
                };
                if options.verbose || options.visual {
                    let stream_id = metadata_stream_ids[index].expect(
                        "every supported compressed metadata chunk has a stream identifier",
                    );
                    crate::progress::complete_stream_group(stream_id, &[]);
                }
                source_deflate_bits = source_deflate_bits
                    .checked_add(replacement.source_deflate_bits)
                    .ok_or_else(|| Error::new("PNG Deflate bit count is too large"))?;
                output_deflate_bits = output_deflate_bits
                    .checked_add(replacement.output_deflate_bits)
                    .ok_or_else(|| Error::new("PNG Deflate bit count is too large"))?;
                append_chunk(
                    &mut output,
                    chunk.kind,
                    replacement.replacement.as_deref().unwrap_or(chunk.data),
                )?;
            }
            _ => {
                output
                    .try_reserve(chunk.encoded.len())
                    .map_err(|_| Error::internal("could not allocate PNG output"))?;
                output.extend_from_slice(chunk.encoded);
            }
        }
    }

    if output.len() > datastream_len && !options.strict {
        output.clear();
        if parsed.has_vestigial_rgba_trns {
            output.extend_from_slice(SIGNATURE);
            for chunk in &parsed.chunks {
                if !chunk.discard_on_output {
                    output
                        .try_reserve(chunk.encoded.len())
                        .map_err(|_| Error::internal("could not allocate PNG output"))?;
                    output.extend_from_slice(chunk.encoded);
                }
            }
        } else {
            output.extend_from_slice(&input[..datastream_len]);
        }
        output_deflate_bits = source_deflate_bits;
    }

    Ok(PngOptimization {
        data: output,
        source_deflate_bits,
        output_deflate_bits,
        timed_out: budget.deadline.is_expired(),
    })
}

/// Whether image work must leave time for a mandatory metadata pass.
///
/// Max has already cached a complete Default floor for every supported
/// compressed metadata stream before reaching image scheduling. Its later
/// metadata refinement is optional and may consume only the actual remainder;
/// it must not shorten the dominant image search. Default has no cached floor,
/// so it still reserves time whenever compressed metadata follows.
fn image_work_needs_metadata_reserve(options: &Options, metadata_bytes: u64) -> bool {
    !options.exhaustive && metadata_bytes != 0
}

fn metadata_default_floor(exhaustive: bool) -> DefaultFloor {
    if exhaustive {
        DefaultFloor::SharedExact
    } else {
        DefaultFloor::Shared
    }
}

/// Cache the complete Default result for a bounded set of metadata streams.
///
/// PNG uses one file-wide search deadline. Without this phase, a single image
/// Max route can spend that deadline before reconstruction reaches zTXt, iTXt,
/// or iCCP, causing Max to return a worse whole file than Default. Building a
/// complete metadata floor is broadly valid: Max retains every completed
/// Default candidate and later optional work only adds alternatives. Bounded
/// static-PNG Max runs may overlap this pass with independent image work in
/// every reporting mode.
fn compressed_metadata_bytes(parsed: &ParsedPng<'_>, options: &Options) -> Option<u64> {
    parsed
        .chunks
        .iter()
        .filter(|chunk| !should_strip(chunk.kind, options))
        .filter_map(|chunk| {
            compressed_zlib_offset(chunk.kind, chunk.data)
                .map(|offset| (chunk.data.len() - offset) as u64)
        })
        .try_fold(0_u64, u64::checked_add)
}

fn precompute_max_metadata_floors(
    parsed: &ParsedPng<'_>,
    metadata_stream_ids: &[Option<usize>],
    options: &Options,
    budget: &mut DecodeBudget,
    cached: &mut [Option<CompressedBodyOptimization>],
) -> Result<()> {
    if !options.exhaustive {
        return Ok(());
    }

    if match compressed_metadata_bytes(parsed, options) {
        Some(bytes) => bytes > MAX_CACHED_METADATA_FLOOR_BYTES,
        None => true,
    } {
        return Ok(());
    }

    let mut floor_options = options.clone();
    floor_options.exhaustive = false;
    for (index, chunk) in parsed.chunks.iter().enumerate() {
        if should_strip(chunk.kind, options)
            || compressed_zlib_offset(chunk.kind, chunk.data).is_none()
        {
            continue;
        }

        let mut optimize_metadata = || {
            optimize_compressed_body(
                chunk.kind,
                chunk.data,
                &floor_options,
                DefaultFloor::MandatoryComplete,
                budget,
            )
        };
        let optimized = if let Some(stream_id) = metadata_stream_ids[index] {
            crate::progress::with_stream_group(stream_id, &[], optimize_metadata)?
        } else {
            optimize_metadata()?
        }
        .ok_or_else(|| Error::new("invalid compressed PNG metadata"))?;
        cached[index] = Some(optimized);
    }
    Ok(())
}

/// Continue a cached metadata floor through Max without rebuilding Default.
///
/// The definitive floor already charged this physical stream to the file's
/// decoded-data budget. This replay decodes the same bytes for validation, so
/// it uses the known exact size as a local ceiling and does not charge the
/// container budget twice. If the image routes spent the shared deadline, the
/// completed floor is returned immediately.
fn refine_cached_compressed_body(
    kind: [u8; 4],
    source_data: &[u8],
    floor: &CompressedBodyOptimization,
    options: &Options,
    budget: &mut DecodeBudget,
) -> Result<CompressedBodyOptimization> {
    let Some(decoded_size) = floor.decoded_size else {
        return try_clone_compressed_body(floor)
            .ok_or_else(|| Error::internal("could not allocate PNG metadata result"));
    };
    if !options.exhaustive || budget.deadline.remaining().is_zero() {
        return try_clone_compressed_body(floor)
            .ok_or_else(|| Error::internal("could not allocate PNG metadata result"));
    }

    let floor_data = floor.replacement.as_deref().unwrap_or(source_data);
    let remaining = budget.deadline.remaining();
    let direct_allowance = remaining / 2;
    let mut best = try_clone_compressed_body(floor)
        .ok_or_else(|| Error::internal("could not allocate PNG metadata result"))?;

    // The cached Default floor may have changed tokens before Max reaches
    // metadata reconstruction. Preserve deft4j's original-source basin with a
    // bounded independent slice, then use the actual remainder on the
    // floor-seeded basin. Both are optional descendants of the retained floor;
    // a local slice ending is not a file timeout.
    if !direct_allowance.is_zero() {
        let mut direct_options = budget.deadline.options_for_call(options);
        direct_options.timeout = direct_options.timeout.min(direct_allowance);
        let mut direct =
            optimize_cached_compressed_parent(kind, source_data, decoded_size, &direct_options)?;
        direct.source_deflate_bits = floor.source_deflate_bits;
        if compressed_body_strictly_dominates(&direct, &best, source_data.len()) {
            best = direct;
        }
    }

    if !budget.deadline.remaining().is_zero() {
        let call_options = budget.deadline.options_for_call(options);
        let mut refined =
            optimize_cached_compressed_parent(kind, floor_data, decoded_size, &call_options)?;
        refined.source_deflate_bits = floor.source_deflate_bits;
        if compressed_body_strictly_dominates(&refined, &best, source_data.len()) {
            best = refined;
        }
    }

    Ok(best)
}

fn optimize_cached_compressed_parent(
    kind: [u8; 4],
    parent_data: &[u8],
    decoded_size: u64,
    options: &Options,
) -> Result<CompressedBodyOptimization> {
    let zlib_offset = compressed_zlib_offset(kind, parent_data)
        .ok_or_else(|| Error::new("invalid compressed PNG metadata"))?;
    let optimized = run_png_zlib(
        &parent_data[zlib_offset..],
        options,
        decoded_size,
        true,
        DefaultFloor::Established,
    )?;
    let info = optimized
        .info
        .as_ref()
        .ok_or_else(|| Error::new("invalid compressed PNG metadata"))?;
    if info.size != decoded_size {
        return Err(Error::new("invalid compressed PNG metadata"));
    }

    let body_len = zlib_offset
        .checked_add(optimized.data.len())
        .ok_or_else(|| Error::new("PNG compressed metadata too large"))?;

    let mut body = Vec::new();
    body.try_reserve_exact(body_len)
        .map_err(|_| Error::internal("could not allocate PNG compressed metadata"))?;
    body.extend_from_slice(&parent_data[..zlib_offset]);
    body.extend_from_slice(&optimized.data);
    Ok(CompressedBodyOptimization {
        replacement: Some(body),
        source_deflate_bits: info.source_deflate_bits,
        output_deflate_bits: info.deflate_bits,
        decoded_size: Some(decoded_size),
    })
}

fn supported_compressed_metadata(chunk: &Chunk<'_>) -> bool {
    compressed_zlib_offset(chunk.kind, chunk.data)
        .and_then(|offset| chunk.data.get(offset..))
        .is_some_and(|zlib| {
            zlib.len() >= 6 && zlib::has_rfc1950_header(zlib) && zlib[1] & 0x20 == 0
        })
}

fn metadata_stream_ids(parsed: &ParsedPng<'_>) -> Result<Vec<Option<usize>>> {
    let mut next_id = parsed
        .fdat_frames
        .len()
        .checked_add(2)
        .ok_or_else(|| Error::resource_limit("too many PNG Deflate streams"))?;
    let mut ids = Vec::new();
    ids.try_reserve_exact(parsed.chunks.len())
        .map_err(|_| Error::internal("could not allocate PNG stream identifiers"))?;
    for chunk in &parsed.chunks {
        if supported_compressed_metadata(chunk) {
            ids.push(Some(next_id));
            next_id = next_id
                .checked_add(1)
                .ok_or_else(|| Error::resource_limit("too many PNG Deflate streams"))?;
        } else {
            ids.push(None);
        }
    }
    Ok(ids)
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ImageJob {
    Idat,
    Frame(usize),
}

fn image_job_stream_group(job: ImageJob, representatives: &[usize]) -> (usize, Vec<usize>) {
    match job {
        ImageJob::Idat => (1, Vec::new()),
        ImageJob::Frame(representative) => {
            let duplicates = representatives
                .iter()
                .enumerate()
                .filter_map(|(index, &owner)| {
                    (owner == representative && index != representative).then_some(index + 2)
                })
                .collect();
            (representative + 2, duplicates)
        }
    }
}

/// Parsing, checksum validation, and PNG reconstruction also consume wall time,
/// but only the raw Deflate searches receive the proportional slices below.
/// Reserve ten percent outside the non-largest slices (twenty percent for a
/// container with more than 32 unique image streams). The final largest stream
/// receives the remaining initial search time. Streams that exhaust those fair
/// shares are queued for weighted reclaim passes using real time left after all
/// initial work. The raw optimizer's ten-percent-plus-one-second grace applies
/// within each admitted slice; only the container deadline is a file timeout.
const NON_LARGEST_IMAGE_SEARCH_FRACTION: f64 = 0.90;
const MANY_IMAGE_SEARCH_FRACTION: f64 = 0.80;
const MANY_IMAGE_JOB_THRESHOLD: usize = 32;

fn total_image_decoded_bytes(idat: u64, frames: &[u64]) -> Result<u64> {
    frames
        .iter()
        .try_fold(idat, |total, &size| total.checked_add(size))
        .ok_or_else(|| Error::resource_limit("decoded PNG data exceeds configured safety limit"))
}

fn image_default_floor(single_image: bool, exhaustive: bool) -> DefaultFloor {
    if single_image {
        // Establish the genuine normal result before spending the remaining
        // file budget on PNG's bounded max routes. This keeps max output no
        // larger than default without allowing a slow normal floor to starve
        // every max-only route.
        DefaultFloor::CompleteThenBounded
    } else if exhaustive {
        DefaultFloor::ApngMax
    } else {
        DefaultFloor::ApngDefault
    }
}

/// Optimize the IDAT stream and each unique APNG frame under one file budget.
///
/// Small streams run first so a large IDAT cannot consume the whole deadline.
/// Duplicate frames contribute their full byte weight to their representative
/// because improving that one stream saves the same bytes at every occurrence.
fn optimize_image_streams(
    idat: &[u8],
    idat_decoded_size: u64,
    frames: &[Vec<u8>],
    frame_decoded_sizes: &[u64],
    has_later_streams: bool,
    options: &Options,
    budget: &mut DecodeBudget,
) -> Result<(FrameOptimization, Vec<FrameOptimization>)> {
    if frames.len() != frame_decoded_sizes.len() {
        return Err(Error::new("invalid APNG frame model"));
    }
    let total_decoded_size = total_image_decoded_bytes(idat_decoded_size, frame_decoded_sizes)?;
    if total_decoded_size > budget.remaining {
        return Err(Error::resource_limit(
            "decoded PNG data exceeds configured safety limit",
        ));
    }
    let representatives = frame_representatives(frames, frame_decoded_sizes)?;
    let mut representative_weights = Vec::new();
    representative_weights
        .try_reserve_exact(frames.len())
        .map_err(|_| Error::internal("could not allocate PNG frame model"))?;
    representative_weights.resize(frames.len(), 0_usize);
    for (index, &representative) in representatives.iter().enumerate() {
        representative_weights[representative] = representative_weights[representative]
            .checked_add(frames[index].len())
            .ok_or_else(|| Error::new("PNG frame data too large"))?;
    }

    let mut jobs = Vec::new();
    jobs.try_reserve_exact(
        frames
            .len()
            .checked_add(1)
            .ok_or_else(|| Error::resource_limit("too many PNG frames"))?,
    )
    .map_err(|_| Error::internal("could not allocate PNG frame jobs"))?;
    jobs.push(ImageJob::Idat);
    jobs.extend(
        representatives
            .iter()
            .enumerate()
            .filter_map(|(index, &representative)| {
                (index == representative).then_some(ImageJob::Frame(index))
            }),
    );

    let total_weight = frames
        .iter()
        .try_fold(idat.len(), |total, frame| total.checked_add(frame.len()))
        .ok_or_else(|| Error::new("PNG frame data too large"))?;
    // Include source order explicitly so the in-place unstable sort retains
    // IDAT-before-fdAT ordering on ties without allocating a merge buffer.
    // Normal and max mode share one file deadline, so both need small-first
    // scheduling; otherwise the first large frame can starve every later one.
    jobs.sort_unstable_by_key(|job| {
        let source_order = match job {
            ImageJob::Idat => 0,
            ImageJob::Frame(index) => index.saturating_add(1),
        };
        (image_job_size(*job, idat, frames), source_order)
    });
    // Only the final job receives the reserved remainder. Distinct streams can
    // have the same largest byte length; treating every tie as the reserve sink
    // would let an earlier tie consume the time intended for the final job.
    let reserved_largest = *jobs.last().expect("the IDAT job is always present");
    let single_image = jobs.len() == 1;
    let non_largest_fraction = if jobs.len() > MANY_IMAGE_JOB_THRESHOLD {
        MANY_IMAGE_SEARCH_FRACTION
    } else {
        NON_LARGEST_IMAGE_SEARCH_FRACTION
    };
    let mut optimized_idat = None;
    let mut optimized = Vec::<Option<FrameOptimization>>::new();
    optimized
        .try_reserve_exact(frames.len())
        .map_err(|_| Error::internal("could not allocate PNG frame results"))?;
    optimized.resize_with(frames.len(), || None);
    let image_floor = image_default_floor(single_image, options.exhaustive);
    let parallel_multi_image = options.exhaustive
        && !single_image
        && !budget.deadline.remaining().is_zero()
        && parallel_multi_image_is_bounded(total_weight, total_decoded_size);
    let mut results = if parallel_multi_image {
        let phase_fraction = if has_later_streams { 0.86 } else { 0.96 };
        let results = optimize_image_jobs_parallel(
            &jobs,
            idat,
            idat_decoded_size,
            frames,
            frame_decoded_sizes,
            &representative_weights,
            &representatives,
            options,
            // Reserve a small container margin outside child raw-route grace
            // for per-stream parsing, worker joins, and PNG reconstruction.
            scale_duration(budget.deadline.remaining(), phase_fraction),
        )?;
        // Every unique stream was validated against its exact IHDR/fcTL size.
        // Charge duplicate frames as well, but only once after all workers have
        // rejoined so no thread mutates the shared file budget.
        budget.remaining -= total_decoded_size;
        results
    } else {
        let mut results = Vec::new();
        results
            .try_reserve_exact(jobs.len())
            .map_err(|_| Error::internal("could not allocate PNG frame results"))?;
        for job in jobs {
            let weight = match job {
                ImageJob::Idat => idat.len(),
                ImageJob::Frame(representative) => representative_weights[representative],
            };
            let mut call_options = options.clone();
            let file_remaining = budget.deadline.remaining();
            let image_remaining = if has_later_streams {
                scale_duration(file_remaining, NON_LARGEST_IMAGE_SEARCH_FRACTION)
            } else {
                file_remaining
            };
            call_options.timeout = image_stream_timeout(
                options.timeout,
                image_remaining,
                weight,
                total_weight,
                non_largest_fraction,
                job == reserved_largest,
            );

            // A spent search budget disables optional searches, not validation.
            // Every IDAT/fdAT stream must still be fully decoded, checksum-
            // checked, and charged to the file-wide expansion limit.
            let (stream, expected_decoded_size) =
                image_job_source(job, idat, idat_decoded_size, frames, frame_decoded_sizes);
            let mut optimize_job = || {
                if single_image
                    && options.exhaustive
                    && parallel_max_image_is_bounded(idat.len(), idat_decoded_size)
                {
                    optimize_single_image_max_parallel(
                        stream,
                        expected_decoded_size,
                        &call_options,
                        budget,
                    )
                } else {
                    optimize_scheduled_png_image_zlib(
                        stream,
                        expected_decoded_size,
                        &call_options,
                        image_floor,
                        budget,
                    )
                }
            };
            let result = if options.visual || options.verbose {
                let (stream_id, duplicates) = image_job_stream_group(job, &representatives);
                if call_options.timeout < file_remaining {
                    crate::progress::with_stream_slice(stream_id, &duplicates, None, optimize_job)?
                } else {
                    crate::progress::with_stream_group(stream_id, &duplicates, optimize_job)?
                }
            } else {
                optimize_job()?
            };
            if !result.timed_out && (options.verbose || options.visual) {
                let (stream_id, duplicates) = image_job_stream_group(job, &representatives);
                crate::progress::complete_stream_group(stream_id, &duplicates);
            }
            results.push((job, result));
        }
        results
    };

    reclaim_timed_out_image_jobs(
        &mut results,
        idat,
        idat_decoded_size,
        frames,
        frame_decoded_sizes,
        &representative_weights,
        &representatives,
        image_floor,
        has_later_streams,
        options,
        &budget.deadline,
    )?;
    if options.verbose || options.visual {
        for (job, _) in &results {
            let (stream_id, duplicates) = image_job_stream_group(*job, &representatives);
            crate::progress::complete_stream_group(stream_id, &duplicates);
        }
    }
    for (job, result) in results {
        store_image_job_result(job, result, &mut optimized_idat, &mut optimized);
    }

    for (index, &representative) in representatives.iter().enumerate() {
        if index != representative {
            let frame = optimized[representative]
                .as_ref()
                .expect("every duplicate frame has an optimized representative");
            // Exact compressed duplicates share optimization work, not decode
            // budget. Each fdAT stream is an independent decoded payload in
            // the container and must count toward the file-wide safety limit.
            if !parallel_multi_image {
                let decoded_size = frame
                    .info
                    .as_ref()
                    .ok_or_else(|| Error::new("invalid PNG frame zlib stream"))?
                    .size;
                budget.remaining -= decoded_size;
            }
            optimized[index] =
                Some(try_clone_frame(frame).ok_or_else(|| {
                    Error::internal("could not allocate duplicate PNG frame result")
                })?);
        }
    }
    let mut complete = Vec::new();
    complete
        .try_reserve_exact(optimized.len())
        .map_err(|_| Error::internal("could not allocate PNG frame results"))?;
    for frame in optimized {
        complete.push(frame.expect("every APNG frame has a representative result"));
    }

    // Max must also retain every file-level saving available to Default.
    // Exact-frame reuse has an explicit work cap, so finish that bounded pass
    // even when Max-exclusive search has consumed the wall-clock allowance.
    let mandatory_default_comparison = options.exhaustive;
    let _ = reuse_best_exact_frames(&mut complete, &mut || {
        !mandatory_default_comparison && budget.deadline.remaining().is_zero()
    });
    Ok((
        optimized_idat.expect("the IDAT job is always present"),
        complete,
    ))
}

fn store_image_job_result(
    job: ImageJob,
    result: zlib::StreamOptimization,
    optimized_idat: &mut Option<FrameOptimization>,
    optimized_frames: &mut [Option<FrameOptimization>],
) {
    let result = FrameOptimization {
        data: result.data,
        info: result.info,
    };
    match job {
        ImageJob::Idat => *optimized_idat = Some(result),
        ImageJob::Frame(index) => optimized_frames[index] = Some(result),
    }
}

#[allow(clippy::too_many_arguments)]
fn reclaim_timed_out_image_jobs(
    results: &mut [(ImageJob, zlib::StreamOptimization)],
    idat: &[u8],
    idat_decoded_size: u64,
    frames: &[Vec<u8>],
    frame_decoded_sizes: &[u64],
    representative_weights: &[usize],
    representatives: &[usize],
    default_floor: DefaultFloor,
    has_later_streams: bool,
    options: &Options,
    deadline: &SearchDeadline,
) -> Result<()> {
    let mut pending = results
        .iter()
        .enumerate()
        .filter_map(|(index, (_, result))| result.timed_out.then_some(index))
        .collect::<Vec<_>>();
    if pending.is_empty() || deadline.is_expired() {
        return Ok(());
    }

    // Leave a final share for compressed PNG metadata, which is reconstructed
    // after the image streams. Files containing only image streams can reclaim
    // the complete actual remainder.
    let reclaim_started = Instant::now();
    let reclaim_allowance = if has_later_streams {
        scale_duration(deadline.remaining(), NON_LARGEST_IMAGE_SEARCH_FRACTION)
    } else {
        deadline.remaining()
    };

    while !pending.is_empty() && !deadline.is_expired() {
        let remaining = reclaim_allowance
            .saturating_sub(reclaim_started.elapsed())
            .min(deadline.remaining());
        if remaining.is_zero() {
            break;
        }
        let total_weight = pending.iter().try_fold(0_usize, |total, &index| {
            total.checked_add(image_job_weight(
                results[index].0,
                idat,
                representative_weights,
            ))
        });
        let Some(mut remaining_weight) = total_weight.filter(|&weight| weight != 0) else {
            break;
        };
        let mut still_pending = Vec::new();
        still_pending
            .try_reserve_exact(pending.len())
            .map_err(|_| Error::internal("could not allocate PNG frame schedule"))?;

        for (position, &result_index) in pending.iter().enumerate() {
            let file_remaining = reclaim_allowance
                .saturating_sub(reclaim_started.elapsed())
                .min(deadline.remaining());
            if file_remaining.is_zero() {
                break;
            }
            let job = results[result_index].0;
            let weight = image_job_weight(job, idat, representative_weights);
            let last = position + 1 == pending.len();
            let timeout = if last {
                file_remaining
            } else {
                scale_duration(
                    file_remaining,
                    weight as f64 / remaining_weight as f64 * 0.95,
                )
            };
            remaining_weight = remaining_weight.saturating_sub(weight);
            if timeout.is_zero() {
                continue;
            }

            let mut retry_options = options.clone();
            retry_options.timeout = timeout;
            let (stream, expected_decoded_size) =
                image_job_source(job, idat, idat_decoded_size, frames, frame_decoded_sizes);
            let retry_job =
                || run_png_image_zlib(stream, &retry_options, expected_decoded_size, default_floor);
            let retry = if options.visual || options.verbose {
                let (stream_id, duplicates) = image_job_stream_group(job, representatives);
                crate::progress::with_stream_reclaim(stream_id, &duplicates, !last, retry_job)?
            } else {
                retry_job()?
            };
            let retry_timed_out = retry.timed_out;
            if zlib_optimization_is_better(&retry, &results[result_index].1) {
                results[result_index].1 = retry;
            } else {
                // The same search completed under the larger allowance. Even
                // when its bytes tie the incumbent, this stream no longer
                // needs another reclamation pass.
                results[result_index].1.timed_out = retry_timed_out;
            }
            if retry_timed_out && !deadline.is_expired() {
                still_pending.push(result_index);
            }
        }
        pending = still_pending;
    }
    Ok(())
}

fn image_job_source<'a>(
    job: ImageJob,
    idat: &'a [u8],
    idat_decoded_size: u64,
    frames: &'a [Vec<u8>],
    frame_decoded_sizes: &[u64],
) -> (&'a [u8], u64) {
    match job {
        ImageJob::Idat => (idat, idat_decoded_size),
        ImageJob::Frame(index) => (frames[index].as_slice(), frame_decoded_sizes[index]),
    }
}

fn image_job_weight(job: ImageJob, idat: &[u8], representative_weights: &[usize]) -> usize {
    match job {
        ImageJob::Idat => idat.len(),
        ImageJob::Frame(index) => representative_weights[index],
    }
}

fn parallel_multi_image_is_bounded(total_compressed: usize, total_decoded: u64) -> bool {
    total_compressed <= PARALLEL_MAX_IMAGE_COMPRESSED && total_decoded <= PARALLEL_MAX_IMAGE_DECODED
}

/// Optimize independent APNG streams on a fixed number of CPU lanes.
///
/// Jobs are already sorted small-to-large. Contiguous balanced slices leave
/// the largest stream in the shortest final slice when the division is uneven,
/// preserving the serial scheduler's preference without a work-stealing queue
/// or shared mutable state. Slice-limited results return to the caller for a
/// work-conserving serial reclaim pass under the one file deadline.
#[allow(clippy::too_many_arguments)]
fn optimize_image_jobs_parallel(
    jobs: &[ImageJob],
    idat: &[u8],
    idat_decoded_size: u64,
    frames: &[Vec<u8>],
    frame_decoded_sizes: &[u64],
    representative_weights: &[usize],
    representatives: &[usize],
    options: &Options,
    phase_timeout: Duration,
) -> Result<Vec<(ImageJob, zlib::StreamOptimization)>> {
    let worker_count = jobs.len().min(PARALLEL_MAX_IMAGE_WORKERS);
    let jobs_per_worker = jobs.len() / worker_count;
    let workers_with_extra_job = jobs.len() % worker_count;

    thread::scope(|scope| {
        let mut workers = Vec::new();
        workers
            .try_reserve_exact(worker_count)
            .map_err(|_| Error::internal("could not allocate PNG image workers"))?;
        let mut fallback_results = Vec::new();
        let mut start = 0_usize;
        for worker_index in 0..worker_count {
            let count = jobs_per_worker + usize::from(worker_index < workers_with_extra_job);
            let worker_jobs = &jobs[start..start + count];
            start += count;
            let worker = thread::Builder::new()
                .name(format!("columbo-png-images-{worker_index}"))
                .spawn_scoped(scope, move || {
                    optimize_image_job_slice(
                        worker_jobs,
                        idat,
                        idat_decoded_size,
                        frames,
                        frame_decoded_sizes,
                        representative_weights,
                        representatives,
                        options,
                        phase_timeout,
                    )
                });
            match worker {
                Ok(worker) => workers.push(worker),
                // Thread exhaustion is not a format error. Run this disjoint
                // slice on the caller while successfully spawned workers
                // continue their own slices.
                Err(_) => {
                    let mut results = optimize_image_job_slice(
                        worker_jobs,
                        idat,
                        idat_decoded_size,
                        frames,
                        frame_decoded_sizes,
                        representative_weights,
                        representatives,
                        options,
                        phase_timeout,
                    )?;
                    fallback_results.append(&mut results);
                }
            }
        }

        for worker in workers {
            let mut results = match worker.join() {
                Ok(result) => result?,
                Err(payload) => std::panic::resume_unwind(payload),
            };
            fallback_results.append(&mut results);
        }
        Ok(fallback_results)
    })
}

#[allow(clippy::too_many_arguments)]
fn optimize_image_job_slice(
    jobs: &[ImageJob],
    idat: &[u8],
    idat_decoded_size: u64,
    frames: &[Vec<u8>],
    frame_decoded_sizes: &[u64],
    representative_weights: &[usize],
    representatives: &[usize],
    options: &Options,
    phase_timeout: Duration,
) -> Result<Vec<(ImageJob, zlib::StreamOptimization)>> {
    let total_weight = jobs.iter().try_fold(0_usize, |total, &job| {
        total.checked_add(image_job_weight(job, idat, representative_weights))
    });
    let total_weight = total_weight.ok_or_else(|| Error::new("PNG frame data too large"))?;
    let mut results = Vec::new();
    results
        .try_reserve_exact(jobs.len())
        .map_err(|_| Error::internal("could not allocate PNG frame results"))?;
    for &job in jobs {
        let weight = image_job_weight(job, idat, representative_weights);
        let mut call_options = options.clone();
        call_options.timeout =
            parallel_image_job_timeout(phase_timeout, jobs.len(), weight, total_weight);
        let (stream, expected_decoded_size) =
            image_job_source(job, idat, idat_decoded_size, frames, frame_decoded_sizes);
        let optimize_job = || {
            run_png_image_zlib(
                stream,
                &call_options,
                expected_decoded_size,
                DefaultFloor::ApngMax,
            )
        };
        let (stream_id, duplicates) = image_job_stream_group(job, representatives);
        let optimized =
            crate::progress::with_stream_slice(stream_id, &duplicates, None, optimize_job)?;
        if !optimized.timed_out && (options.verbose || options.visual) {
            crate::progress::complete_stream_group(stream_id, &duplicates);
        }
        results.push((job, optimized));
    }
    Ok(results)
}

/// Divide one worker's wall allowance while accounting for every child raw
/// route's ten-percent-plus-one-second active-work grace.
fn parallel_image_job_timeout(
    phase_timeout: Duration,
    job_count: usize,
    weight: usize,
    total_weight: usize,
) -> Duration {
    if phase_timeout.is_zero() || job_count == 0 || weight == 0 || total_weight == 0 {
        return Duration::ZERO;
    }
    let hard_budget = phase_timeout
        .saturating_add(scale_duration(phase_timeout, 0.10))
        .saturating_add(Duration::from_secs(1));
    let fixed_graces = Duration::from_secs(job_count as u64);
    let soft_budget = scale_duration(hard_budget.saturating_sub(fixed_graces), 1.0 / 1.10);
    scale_duration(soft_budget, weight as f64 / total_weight as f64)
}

/// Keep the standard unstable sorter from specializing its relatively large
/// implementation to the APNG frame comparator.
#[inline(never)]
fn sort_frame_indices(
    indices: &mut [usize],
    compare: &mut dyn FnMut(usize, usize) -> std::cmp::Ordering,
) {
    indices.sort_unstable_by(|&left, &right| compare(left, right));
}

/// Find the earliest exact-compressed representative in O(n log n) compares.
/// Sorting slices directly avoids adversarial hash-collision buckets.
fn frame_representatives(frames: &[Vec<u8>], decoded_sizes: &[u64]) -> Result<Vec<usize>> {
    if frames.len() != decoded_sizes.len() {
        return Err(Error::new("invalid APNG frame model"));
    }
    let mut order = Vec::new();
    order
        .try_reserve_exact(frames.len())
        .map_err(|_| Error::internal("could not allocate PNG frame model"))?;
    order.extend(0..frames.len());
    sort_frame_indices(&mut order, &mut |left, right| {
        (frames[left].as_slice(), decoded_sizes[left])
            .cmp(&(frames[right].as_slice(), decoded_sizes[right]))
            .then_with(|| left.cmp(&right))
    });

    let mut representatives = Vec::new();
    representatives
        .try_reserve_exact(frames.len())
        .map_err(|_| Error::internal("could not allocate PNG frame model"))?;
    representatives.extend(0..frames.len());
    let mut group_start = 0;
    while group_start < order.len() {
        let first = order[group_start];
        let group_len = order[group_start..].partition_point(|&index| {
            frames[index] == frames[first] && decoded_sizes[index] == decoded_sizes[first]
        });
        for &index in &order[group_start..group_start + group_len] {
            representatives[index] = first;
        }
        group_start += group_len;
    }
    Ok(representatives)
}

fn try_clone_frame(frame: &FrameOptimization) -> Option<FrameOptimization> {
    Some(FrameOptimization {
        data: try_clone_bytes(&frame.data)?,
        info: frame.info.clone(),
    })
}

fn frame_source_bits(frame: &FrameOptimization) -> Result<u64> {
    frame
        .info
        .as_ref()
        .map(|info| info.source_deflate_bits)
        .ok_or_else(|| Error::new("valid PNG image stream has no Deflate information"))
}

fn frame_output_bits(frame: &FrameOptimization) -> Result<u64> {
    frame
        .info
        .as_ref()
        .map(|info| info.deflate_bits)
        .ok_or_else(|| Error::new("valid PNG image stream has no Deflate information"))
}

fn try_clone_compressed_body(
    optimized: &CompressedBodyOptimization,
) -> Option<CompressedBodyOptimization> {
    Some(CompressedBodyOptimization {
        replacement: match optimized.replacement.as_deref() {
            Some(data) => Some(try_clone_bytes(data)?),
            None => None,
        },
        source_deflate_bits: optimized.source_deflate_bits,
        output_deflate_bits: optimized.output_deflate_bits,
        decoded_size: optimized.decoded_size,
    })
}

fn image_job_size(job: ImageJob, idat: &[u8], frames: &[Vec<u8>]) -> usize {
    match job {
        ImageJob::Idat => idat.len(),
        ImageJob::Frame(index) => frames[index].len(),
    }
}

fn image_stream_timeout(
    configured: Duration,
    remaining: Duration,
    weight: usize,
    total_weight: usize,
    non_largest_fraction: f64,
    is_largest: bool,
) -> Duration {
    if weight == 0 || total_weight == 0 || remaining.is_zero() {
        return Duration::ZERO;
    }
    let headroom = scale_duration(remaining, 0.98);
    if is_largest {
        return headroom;
    }
    let proportional = scale_duration(
        configured,
        weight as f64 / total_weight as f64 * non_largest_fraction,
    );
    proportional.min(headroom)
}

/// Reuse a smaller frame representation only after exact decoded comparison.
///
/// CRC-32 and Adler-32 are useful filters, but together they are not an
/// identity proof: deliberately different byte strings can collide. We decode
/// one bounded reference for each checksum group and compare every candidate
/// byte-for-byte before substituting its compressed bytes.
fn reuse_best_exact_frames<F>(frames: &mut [FrameOptimization], expired: &mut F) -> bool
where
    F: FnMut() -> bool,
{
    // Build and sort checksum summaries first. Singleton summaries cannot
    // participate in reuse and therefore cost no extra decode. A fallible flat
    // vector keeps this optional route deterministic without many tiny map
    // allocations on an APNG containing thousands of frames.
    let mut summaries = Vec::<((u64, u32, u32), usize)>::new();
    if summaries.try_reserve_exact(frames.len()).is_err() {
        return false;
    }
    for (index, frame) in frames.iter().enumerate() {
        if let Some(info) = &frame.info {
            summaries.push(((info.size, info.crc32, info.adler32), index));
        }
    }
    summaries.sort_unstable();

    let mut grouped = Vec::new();
    if grouped.try_reserve_exact(frames.len()).is_err() {
        return false;
    }
    grouped.resize(frames.len(), false);
    let mut work_remaining = MAX_EXACT_REUSE_WORK_BYTES;
    let mut timed_out = false;
    let mut group_start = 0;
    'groups: while group_start < summaries.len() {
        let summary = summaries[group_start].0;
        let group_len =
            summaries[group_start..].partition_point(|&(candidate, _)| candidate == summary);
        let group_end = group_start + group_len;
        if group_len == 1 {
            group_start = group_end;
            continue;
        }

        // Equal scores cannot improve one another, regardless of content.
        let first_score = frame_score(&frames[summaries[group_start].1]);
        if summaries[group_start..group_end]
            .iter()
            .all(|&(_, index)| frame_score(&frames[index]) == first_score)
        {
            group_start = group_end;
            continue;
        }

        for position in group_start..group_end {
            let index = summaries[position].1;
            if grouped[index] {
                continue;
            }
            if expired() {
                timed_out = true;
                break 'groups;
            }
            let decoded_size = frames[index]
                .info
                .as_ref()
                .expect("a summary member has decode information")
                .size;
            let reference_work = exact_comparison_work(&frames[index], decoded_size);
            if reference_work > work_remaining {
                break 'groups;
            }
            work_remaining -= reference_work;
            let Some(reference_decoded) =
                decoded_zlib_for_comparison(&frames[index].data, decoded_size)
            else {
                grouped[index] = true;
                continue;
            };

            let mut members = Vec::new();
            if members.try_reserve_exact(group_end - position).is_err() {
                return timed_out;
            }
            members.push(index);
            for &(_, candidate) in &summaries[position + 1..group_end] {
                if grouped[candidate] {
                    continue;
                }
                let equal = if frames[candidate].data == frames[index].data {
                    true
                } else {
                    if expired() {
                        timed_out = true;
                        break 'groups;
                    }
                    let candidate_work = exact_comparison_work(&frames[candidate], decoded_size);
                    if candidate_work > work_remaining {
                        break 'groups;
                    }
                    work_remaining -= candidate_work;
                    zlib_decodes_to(&frames[candidate].data, &reference_decoded)
                };
                if equal {
                    members.push(candidate);
                }
            }

            let mut best = index;
            for &candidate in &members[1..] {
                if frame_is_better(&frames[candidate], &frames[best]) {
                    best = candidate;
                }
            }
            let Some(best_data) = try_clone_bytes(&frames[best].data) else {
                return timed_out;
            };
            let best_info = frames[best]
                .info
                .clone()
                .expect("an exact-reuse member has decoded stream information");
            for member in members {
                grouped[member] = true;
                if frame_is_better(&frames[best], &frames[member]) {
                    let Some(replacement) = try_clone_bytes(&best_data) else {
                        continue;
                    };
                    let source_deflate_bits = frames[member]
                        .info
                        .as_ref()
                        .expect("an exact-reuse member has decoded stream information")
                        .source_deflate_bits;
                    let mut replacement_info = best_info.clone();
                    replacement_info.source_deflate_bits = source_deflate_bits;
                    frames[member].data = replacement;
                    frames[member].info = Some(replacement_info);
                }
            }
        }
        group_start = group_end;
    }
    timed_out
}

fn exact_comparison_work(frame: &FrameOptimization, decoded_size: u64) -> u64 {
    decoded_size.saturating_add(frame.data.len() as u64).max(1)
}

fn try_clone_bytes(source: &[u8]) -> Option<Vec<u8>> {
    let mut copy = Vec::new();
    copy.try_reserve_exact(source.len()).ok()?;
    copy.extend_from_slice(source);
    Some(copy)
}

fn frame_score(frame: &FrameOptimization) -> (usize, u64) {
    (
        frame.data.len(),
        frame
            .info
            .as_ref()
            .map_or(u64::MAX, |info| info.deflate_bits),
    )
}

fn frame_is_better(candidate: &FrameOptimization, reference: &FrameOptimization) -> bool {
    candidate.data.len() < reference.data.len()
        || (candidate.data.len() == reference.data.len()
            && candidate
                .info
                .as_ref()
                .zip(reference.info.as_ref())
                .is_some_and(|(candidate, reference)| {
                    candidate.deflate_bits < reference.deflate_bits
                }))
}

fn decoded_zlib_for_comparison(input: &[u8], decoded_size: u64) -> Option<Vec<u8>> {
    let raw = zlib_raw_payload(input)?;
    decoded_bytes_for_comparison(raw, decoded_size, MAX_EXACT_REUSE_BYTES)
}

fn zlib_decodes_to(input: &[u8], expected: &[u8]) -> bool {
    let Some(raw) = zlib_raw_payload(input) else {
        return false;
    };
    raw_stream_decodes_to(raw, expected.len() as u64, expected)
}

fn zlib_raw_payload(input: &[u8]) -> Option<&[u8]> {
    (input.len() >= 6).then(|| &input[2..input.len() - 4])
}

fn parallel_max_image_is_bounded(compressed_size: usize, decoded_size: u64) -> bool {
    compressed_size <= PARALLEL_MAX_IMAGE_COMPRESSED && decoded_size <= PARALLEL_MAX_IMAGE_DECODED
}

/// Race the independent lineages useful to a single-image Max run.
///
/// The main lineage retains the established CompleteThenBounded schedule: it
/// finishes exact Default as the non-regression floor, then explores original-
/// source Max states that an ordinary rewrite can remove. The worker finishes
/// a cheaper transformed parent early, then spends the remainder in that
/// distinct basin. We deliberately do not run Max again from the late exact-
/// Default result: that lost broadly to the early parent while duplicating
/// descendant work. Both lineages use the exact PNG scanline size as their
/// decode ceiling; the file-wide safety budget is charged once after rejoin.
fn optimize_single_image_max_parallel(
    input: &[u8],
    expected_decoded_size: u64,
    options: &Options,
    budget: &mut DecodeBudget,
) -> Result<zlib::StreamOptimization> {
    if expected_decoded_size > budget.remaining {
        return Err(Error::resource_limit(
            "decoded PNG data exceeds configured safety limit",
        ));
    }
    let decoded_limit = expected_decoded_size;
    let raw = zlib_raw_payload(input).ok_or_else(|| Error::new("invalid PNG image zlib stream"))?;
    // Start the transformed lineage only when its search basin is distinct or
    // exact Default would otherwise serialize all work in a short allowance.
    // Other sources keep the CPU for the already-concurrent direct routes.
    let run_early_lineage = raw_source_benefits_from_early_max_lineage(raw, decoded_limit)
        .map_err(map_png_zlib_error)
        .map_err(map_png_image_zlib_error)?;
    let selected = thread::scope(|scope| {
        let early_worker = run_early_lineage
            .then(|| {
                // A child thread has no physical-stream presentation context,
                // so suppress only its duplicate UI. These flags do not gate
                // routes, deadlines, candidate selection, or memory policy;
                // every optimization field remains identical to the caller.
                let mut quiet_options = options.clone();
                quiet_options.verbose = false;
                quiet_options.visual = false;
                thread::Builder::new()
                    .name("columbo-png-early-max".into())
                    .spawn_scoped(scope, move || {
                        optimize_quick_image_floor_lineage(input, &quiet_options, decoded_limit)
                    })
                    .ok()
            })
            .flatten();

        let mut selected = run_png_image_zlib(
            input,
            options,
            decoded_limit,
            DefaultFloor::CompleteThenBounded,
        )?;
        if let Some(early_worker) = early_worker {
            let early = match early_worker.join() {
                Ok(result) => result?,
                Err(payload) => std::panic::resume_unwind(payload),
            };
            selected = best_zlib_optimization(selected, early);
        };
        Ok(selected)
    })?;

    let info = selected
        .info
        .as_ref()
        .ok_or_else(|| Error::new("invalid PNG image zlib stream"))?;
    if info.size != expected_decoded_size {
        return Err(Error::new("PNG image data size does not match IHDR"));
    }
    if info.size > budget.remaining {
        return Err(Error::resource_limit(
            "decoded PNG data exceeds configured safety limit",
        ));
    }
    budget.remaining -= info.size;
    Ok(selected)
}

/// Establish an early complete transformed parent, then reserve most of this
/// branch's allowance for Max descendants. The exact Default worker remains
/// independent, so an interrupted quick floor cannot weaken the final result.
fn optimize_quick_image_floor_lineage(
    input: &[u8],
    options: &Options,
    decoded_limit: u64,
) -> Result<zlib::StreamOptimization> {
    let started = Instant::now();
    let mut floor_options = options.clone();
    floor_options.exhaustive = false;
    floor_options.timeout = scale_duration(options.timeout, QUICK_FLOOR_SEARCH_FRACTION);
    let floor = run_png_image_zlib(input, &floor_options, decoded_limit, DefaultFloor::Complete)?;
    refine_single_image_floor(floor, options, started, decoded_limit)
}

fn refine_single_image_floor(
    mut floor: zlib::StreamOptimization,
    options: &Options,
    started: Instant,
    decoded_limit: u64,
) -> Result<zlib::StreamOptimization> {
    let remaining = options.timeout.saturating_sub(started.elapsed());
    if remaining.is_zero() {
        floor.timed_out = true;
        return Ok(floor);
    }

    let mut refine_options = options.clone();
    refine_options.timeout = remaining;
    let mut refined = run_png_image_zlib(
        &floor.data,
        &refine_options,
        decoded_limit,
        DefaultFloor::Established,
    )?;
    if !zlib_optimization_is_better(&refined, &floor) {
        floor.timed_out |= refined.timed_out;
        return Ok(floor);
    }
    if let (Some(source), Some(output)) = (floor.info.as_ref(), refined.info.as_mut()) {
        output.source_deflate_bits = source.source_deflate_bits;
    }
    refined.timed_out |= floor.timed_out;
    Ok(refined)
}

fn best_zlib_optimization(
    mut floor: zlib::StreamOptimization,
    mut direct: zlib::StreamOptimization,
) -> zlib::StreamOptimization {
    let timed_out = floor.timed_out || direct.timed_out;
    if zlib_optimization_is_better(&direct, &floor) {
        direct.timed_out = timed_out;
        direct
    } else {
        floor.timed_out = timed_out;
        floor
    }
}

fn zlib_optimization_is_better(
    candidate: &zlib::StreamOptimization,
    incumbent: &zlib::StreamOptimization,
) -> bool {
    candidate.data.len() < incumbent.data.len()
        || (candidate.data.len() == incumbent.data.len()
            && candidate.info.as_ref().map(|info| info.deflate_bits)
                < incumbent.info.as_ref().map(|info| info.deflate_bits))
}

fn run_png_zlib(
    input: &[u8],
    options: &Options,
    decoded_limit: u64,
    lenient_header: bool,
    default_floor: DefaultFloor,
) -> Result<zlib::StreamOptimization> {
    zlib::optimize_embedded(input, options, decoded_limit, lenient_header, default_floor)
        .map_err(map_png_zlib_error)
}

fn run_png_image_zlib(
    input: &[u8],
    options: &Options,
    expected_decoded_size: u64,
    default_floor: DefaultFloor,
) -> Result<zlib::StreamOptimization> {
    run_png_zlib(input, options, expected_decoded_size, false, default_floor)
        .map_err(map_png_image_zlib_error)
}

fn map_png_image_zlib_error(error: Error) -> Error {
    if error.kind() == ErrorKind::ResourceLimit {
        Error::new("PNG image data size does not match IHDR")
    } else {
        error
    }
}

fn map_png_zlib_error(error: Error) -> Error {
    match error.kind() {
        ErrorKind::ResourceLimit => {
            Error::resource_limit("decoded PNG data exceeds configured safety limit")
        }
        ErrorKind::ComplexityLimit | ErrorKind::Internal => error,
        _ => error,
    }
}

fn optimize_png_zlib(
    input: &[u8],
    options: &Options,
    lenient_header: bool,
    default_floor: DefaultFloor,
    budget: &mut DecodeBudget,
) -> Result<zlib::StreamOptimization> {
    let call_options = budget.deadline.options_for_call(options);
    optimize_png_zlib_with_options(input, &call_options, lenient_header, default_floor, budget)
}

fn optimize_scheduled_png_image_zlib(
    input: &[u8],
    expected_decoded_size: u64,
    options: &Options,
    default_floor: DefaultFloor,
    budget: &mut DecodeBudget,
) -> Result<zlib::StreamOptimization> {
    if expected_decoded_size > budget.remaining {
        return Err(Error::resource_limit(
            "decoded PNG data exceeds configured safety limit",
        ));
    }
    let result = run_png_image_zlib(input, options, expected_decoded_size, default_floor)?;
    let info = result
        .info
        .as_ref()
        .ok_or_else(|| Error::new("invalid PNG image zlib stream"))?;
    if info.size != expected_decoded_size {
        return Err(Error::new("PNG image data size does not match IHDR"));
    }
    budget.remaining -= info.size;
    Ok(result)
}

fn optimize_png_zlib_with_options(
    input: &[u8],
    call_options: &Options,
    lenient_header: bool,
    default_floor: DefaultFloor,
    budget: &mut DecodeBudget,
) -> Result<zlib::StreamOptimization> {
    let result = run_png_zlib(
        input,
        call_options,
        budget.remaining,
        lenient_header,
        default_floor,
    )?;
    if let Some(info) = &result.info {
        if info.size > budget.remaining {
            return Err(Error::resource_limit(
                "decoded PNG data exceeds configured safety limit",
            ));
        }
        budget.remaining -= info.size;
    }
    Ok(result)
}

fn optimize_compressed_body(
    kind: [u8; 4],
    data: &[u8],
    options: &Options,
    default_floor: DefaultFloor,
    budget: &mut DecodeBudget,
) -> Result<Option<CompressedBodyOptimization>> {
    let Some(zlib_offset) = compressed_zlib_offset(kind, data) else {
        return Ok(None);
    };
    let optimized = optimize_png_zlib(&data[zlib_offset..], options, true, default_floor, budget)?;
    let source_deflate_bits = optimized
        .info
        .as_ref()
        .map_or(0, |info| info.source_deflate_bits);
    let output_deflate_bits = optimized.info.as_ref().map_or(0, |info| info.deflate_bits);
    let decoded_size = optimized.info.as_ref().map(|info| info.size);
    let body_len = zlib_offset
        .checked_add(optimized.data.len())
        .ok_or_else(|| Error::new("PNG compressed metadata too large"))?;
    let source_header = data.get(zlib_offset..).and_then(|zlib| zlib.get(..2));
    let header_improved = optimized.data.get(..2) != source_header;
    if !options.strict
        && (body_len > data.len()
            || (body_len == data.len()
                && output_deflate_bits >= source_deflate_bits
                && !header_improved))
    {
        return Ok(Some(CompressedBodyOptimization {
            replacement: None,
            source_deflate_bits,
            output_deflate_bits: source_deflate_bits,
            decoded_size,
        }));
    }
    let mut body = Vec::new();
    body.try_reserve_exact(body_len)
        .map_err(|_| Error::internal("could not allocate PNG compressed metadata"))?;
    body.extend_from_slice(&data[..zlib_offset]);
    body.extend_from_slice(&optimized.data);
    Ok(Some(CompressedBodyOptimization {
        replacement: Some(body),
        source_deflate_bits,
        output_deflate_bits,
        decoded_size,
    }))
}

fn should_strip(kind: [u8; 4], options: &Options) -> bool {
    should_strip_kind(kind, options.strip_metadata)
}

#[cfg(test)]
mod test_support;
#[cfg(test)]
mod tests;
