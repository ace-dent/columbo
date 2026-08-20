// SPDX-License-Identifier: MIT

use std::thread;
use std::time::{Duration, Instant};

use crate::checksum::crc32_update;
use crate::deflate::{
    decoded_bytes_for_comparison, raw_source_benefits_from_early_max_lineage,
    raw_stream_decodes_to, DefaultFloor, RawInfo,
};
use crate::{Error, ErrorKind, Optimization, Options, Result};

use super::{scale_duration, zlib, SearchDeadline};

const SIGNATURE: &[u8; 8] = b"\x89PNG\r\n\x1a\n";
/// Exact cross-frame reuse is optional. Bounding the retained comparison bytes
/// keeps an APNG with many very large frames from doubling its memory use.
const MAX_EXACT_REUSE_BYTES: usize = 32 * 1024 * 1024;
const MAX_EXACT_REUSE_WORK_BYTES: u64 = 64 * 1024 * 1024;
const MAX_METADATA_PROBE_WORK_BYTES: u64 = 64 * 1024 * 1024;
// Max must not discard affordable Default savings in compressed metadata just
// because an image route spends the shared file deadline first. Precompute the
// complete non-Max metadata floors when their aggregate compressed size fits
// the same bounded work class as speculative metadata probes, then reuse those
// results during reconstruction. Larger metadata sets retain the streaming
// schedule so caching cannot duplicate an attacker-sized portion of the input.
const MAX_CACHED_METADATA_FLOOR_BYTES: u64 = MAX_METADATA_PROBE_WORK_BYTES;
// A single-image Max run can retain exact Default and direct-Max raw models
// together. Keep that combined working set within the same broad bounds used
// for independent raw-route arenas.
const PARALLEL_MAX_IMAGE_COMPRESSED: usize = 8 * 1024 * 1024;
const PARALLEL_MAX_IMAGE_DECODED: u64 = 64 * 1024 * 1024;
const PARALLEL_MAX_IMAGE_WORKERS: usize = 8;
// A fifth is enough to finish the inexpensive parent on representative image
// streams while reserving most of Max for the descendants that need it. This
// is the same follow-up reservation used by Deflate's bounded route schedule.
const QUICK_FLOOR_SEARCH_FRACTION: f64 = 0.20;
/// Every APNG frame owns and validates an independent zlib stream. Bound that
/// invocation count separately from the generic chunk count because an empty
/// frame consumes almost no decoded-byte budget.
const MAX_APNG_FRAMES: usize = 16_384;
/// Compressed ancillary chunks are each decoded and checksum-validated even
/// when the optional search deadline is exhausted. Keep zero-length metadata
/// streams from multiplying that mandatory parser setup indefinitely.
const MAX_COMPRESSED_METADATA_STREAMS: usize = 4_096;
/// Twelve-byte empty chunks otherwise amplify into several independent Rust
/// model records. This remains far beyond practical PNG/APNG use while
/// keeping parser bookkeeping and mandatory CRC work comfortably bounded.
const MAX_PNG_CHUNKS: usize = 65_536;

#[derive(Clone, Copy)]
struct Chunk<'a> {
    kind: [u8; 4],
    data: &'a [u8],
    encoded: &'a [u8],
    discard_on_output: bool,
}

pub(super) struct ParsedPng<'a> {
    chunks: Vec<Chunk<'a>>,
    datastream_len: usize,
    idat: Vec<u8>,
    idat_decoded_size: u64,
    fdat_frames: Vec<Vec<u8>>,
    fdat_decoded_sizes: Vec<u64>,
    has_rewrite_sensitive_ancillary: bool,
    has_vestigial_rgba_trns: bool,
}

#[derive(Default)]
struct ParseState {
    saw_ihdr: bool,
    width: u32,
    height: u32,
    bit_depth: u8,
    color_type: u8,
    interlace_method: u8,
    saw_plte: bool,
    palette_entries: u32,
    saw_idat: bool,
    saw_iend: bool,
    after_idat_run: bool,

    saw_actl: bool,
    animation_frames: u32,
    fctl_count: u32,
    sequence_expected: u32,
    frame_open: bool,
    frame_has_data: bool,
    current_fdat_decoded_size: Option<u64>,

    saw_chrm: bool,
    saw_gama: bool,
    saw_iccp: bool,
    saw_sbit: bool,
    saw_srgb: bool,
    saw_cicp: bool,
    saw_mdcv: bool,
    saw_clli: bool,
    saw_trns: bool,
    saw_bkgd: bool,
    saw_hist: bool,
    saw_phys: bool,
    saw_scal: bool,
    saw_exif: bool,
    saw_time: bool,
    saw_cabx: bool,
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

fn empty_chunk_replacements(chunk_count: usize) -> Result<Vec<Option<CompressedBodyOptimization>>> {
    let mut replacements = Vec::new();
    replacements
        .try_reserve_exact(chunk_count)
        .map_err(|_| Error::new("could not allocate PNG chunk model"))?;
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

#[cfg(test)]
pub(super) fn optimize(input: &[u8], options: &Options) -> Result<Optimization> {
    let parsed = preflight(input, options.strip_metadata)?;
    optimize_preflight(input, options, parsed)
}

pub(super) fn optimize_preflight(
    input: &[u8],
    options: &Options,
    parsed: ParsedPng<'_>,
) -> Result<Optimization> {
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
            .ok_or_else(|| Error::new("could not allocate PNG output"))?;
        return Ok(Optimization::from_metrics(
            input.len(),
            data,
            source_deflate_bits,
            source_deflate_bits,
            budget.deadline.is_expired(),
        ));
    }

    let mut output = Vec::new();
    output
        .try_reserve_exact(datastream_len)
        .map_err(|_| Error::new("could not allocate PNG output"))?;
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
                    .ok_or_else(|| Error::new("could not allocate APNG control chunk"))?;
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
                        .map_err(|_| Error::new("could not allocate APNG frame"))?;
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
                        optimize_compressed_body(
                            chunk.kind,
                            chunk.data,
                            options,
                            DefaultFloor::Shared,
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
                    .map_err(|_| Error::new("could not allocate PNG output"))?;
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
                        .map_err(|_| Error::new("could not allocate PNG output"))?;
                    output.extend_from_slice(chunk.encoded);
                }
            }
        } else {
            output.extend_from_slice(&input[..datastream_len]);
        }
        output_deflate_bits = source_deflate_bits;
    }

    Ok(Optimization::from_metrics(
        input.len(),
        output,
        source_deflate_bits,
        output_deflate_bits,
        budget.deadline.is_expired(),
    ))
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
                DefaultFloor::Complete,
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
            .ok_or_else(|| Error::new("could not allocate PNG metadata result"));
    };
    if !options.exhaustive || budget.deadline.remaining().is_zero() {
        return try_clone_compressed_body(floor)
            .ok_or_else(|| Error::new("could not allocate PNG metadata result"));
    }

    let floor_data = floor.replacement.as_deref().unwrap_or(source_data);
    let zlib_offset = compressed_zlib_offset(kind, floor_data)
        .ok_or_else(|| Error::new("invalid compressed PNG metadata"))?;
    let call_options = budget.deadline.options_for_call(options);
    let refined = run_png_zlib(
        &floor_data[zlib_offset..],
        &call_options,
        decoded_size,
        true,
        DefaultFloor::Established,
    )?;
    let info = refined
        .info
        .as_ref()
        .ok_or_else(|| Error::new("invalid compressed PNG metadata"))?;
    if info.size != decoded_size {
        return Err(Error::new("invalid compressed PNG metadata"));
    }

    let body_len = zlib_offset
        .checked_add(refined.data.len())
        .ok_or_else(|| Error::new("PNG compressed metadata too large"))?;
    let refined_wins = body_len < floor_data.len()
        || (body_len == floor_data.len() && info.deflate_bits < floor.output_deflate_bits);
    if !refined_wins {
        return try_clone_compressed_body(floor)
            .ok_or_else(|| Error::new("could not allocate PNG metadata result"));
    }

    let mut body = Vec::new();
    body.try_reserve_exact(body_len)
        .map_err(|_| Error::new("could not allocate PNG compressed metadata"))?;
    body.extend_from_slice(&floor_data[..zlib_offset]);
    body.extend_from_slice(&refined.data);
    Ok(CompressedBodyOptimization {
        replacement: Some(body),
        source_deflate_bits: floor.source_deflate_bits,
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
        .map_err(|_| Error::new("could not allocate PNG stream identifiers"))?;
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

fn parse(input: &[u8], strip_metadata: bool) -> Result<ParsedPng<'_>> {
    if !input.starts_with(SIGNATURE) {
        return Err(Error::new("invalid PNG signature"));
    }

    let mut chunks = Vec::new();
    let mut idat = Vec::new();
    let mut fdat = Vec::new();
    let mut fdat_frames = Vec::new();
    let mut fdat_decoded_sizes = Vec::new();
    let mut state = ParseState::default();
    let mut has_rewrite_sensitive_ancillary = false;
    let mut has_vestigial_rgba_trns = false;
    let mut compressed_metadata_streams = 0_usize;
    let mut position = SIGNATURE.len();

    while input.len().saturating_sub(position) >= 12 {
        if chunks.len() >= MAX_PNG_CHUNKS {
            return Err(Error::resource_limit("PNG contains too many chunks"));
        }
        let length = be32(input, position)?;
        if length > 0x7fff_ffff {
            return Err(Error::new("invalid PNG chunk length"));
        }
        let length = length as usize;
        if length > input.len() - position - 12 {
            return Err(Error::new("truncated PNG chunk"));
        }

        let kind: [u8; 4] = input[position + 4..position + 8].try_into().unwrap();
        let data = &input[position + 8..position + 8 + length];
        let after = position + 12 + length;
        let stored_crc =
            u32::from_be_bytes(input[position + 8 + length..after].try_into().unwrap());

        if !valid_chunk_type(kind) {
            return Err(Error::new("invalid PNG chunk type"));
        }
        let calculated_crc = crc32_update(crc32_update(0, &kind), data);
        if calculated_crc != stored_crc {
            return Err(Error::integrity_mismatch("bad PNG chunk CRC"));
        }

        if position == SIGNATURE.len() {
            validate_ihdr(kind, data, &mut state)?;
        } else if kind == *b"IHDR" {
            return Err(Error::new("invalid PNG IHDR"));
        }
        if kind == *b"IEND" && !data.is_empty() {
            return Err(Error::new("invalid PNG IEND"));
        }
        if kind[0] & 0x20 == 0 && !is_known_critical(kind) {
            return Err(Error::new("unknown PNG critical chunk"));
        }
        let strip_chunk = should_strip_kind(kind, strip_metadata);
        if is_rewrite_sensitive_ancillary(kind) {
            has_rewrite_sensitive_ancillary = true;
        }

        validate_palette(kind, data, &mut state)?;
        // Metadata whose semantics or placement are invalid is useful to no
        // output that explicitly strips it. Still validate chunk boundaries,
        // type bytes, and CRC above: --strip is not a general PNG repair mode.
        let discard_on_output = if strip_chunk {
            false
        } else {
            validate_ancillary(kind, data, &mut state)?
        };
        has_vestigial_rgba_trns |= discard_on_output;
        // fcTL begins the next fdAT zlib stream; IEND closes the final one.
        if matches!(&kind, b"fcTL" | b"IEND") && !fdat.is_empty() {
            fdat_frames
                .try_reserve(1)
                .map_err(|_| Error::new("could not allocate PNG frame model"))?;
            fdat_decoded_sizes
                .try_reserve(1)
                .map_err(|_| Error::new("could not allocate PNG frame model"))?;
            fdat_frames.push(std::mem::take(&mut fdat));
            fdat_decoded_sizes.push(
                state
                    .current_fdat_decoded_size
                    .take()
                    .ok_or_else(|| Error::new("invalid APNG frame data"))?,
            );
        }

        validate_animation_control(kind, data, &mut state)?;
        if !strip_chunk && compressed_zlib_offset(kind, data).is_some() {
            compressed_metadata_streams += 1;
            if compressed_metadata_streams > MAX_COMPRESSED_METADATA_STREAMS {
                return Err(Error::resource_limit(
                    "PNG contains too many compressed metadata streams",
                ));
            }
        }

        if kind == *b"IDAT" {
            if state.after_idat_run {
                return Err(Error::new("non-consecutive IDAT chunk"));
            }
            if !state.saw_idat && state.color_type == 3 && !state.saw_plte {
                return Err(Error::new("missing PNG PLTE"));
            }
            state.saw_idat = true;
            idat.try_reserve(data.len())
                .map_err(|_| Error::new("could not allocate PNG image stream"))?;
            idat.extend_from_slice(data);
        } else if kind == *b"fdAT" {
            if !state.saw_actl
                || !state.saw_idat
                || !state.frame_open
                || data.len() < 4
                || u32::from_be_bytes(data[..4].try_into().unwrap()) != state.sequence_expected
            {
                return Err(Error::new("bad APNG fdAT chunk"));
            }
            state.sequence_expected += 1;
            // A sequence-number-only chunk may occur between real fdAT
            // packets, but it cannot by itself satisfy the frame-data
            // requirement.
            state.frame_has_data |= data.len() > 4;
            fdat.try_reserve(data.len() - 4)
                .map_err(|_| Error::new("could not allocate PNG frame stream"))?;
            fdat.extend_from_slice(&data[4..]);
        } else if state.saw_idat && !strip_chunk {
            state.after_idat_run = true;
        }

        chunks
            .try_reserve(1)
            .map_err(|_| Error::new("could not allocate PNG chunk model"))?;
        chunks.push(Chunk {
            kind,
            data,
            encoded: &input[position..after],
            discard_on_output,
        });
        position = after;
        if kind == *b"IEND" {
            state.saw_iend = true;
            // IEND terminates the PNG datastream. Tolerate an enclosing file's
            // suffix on input, but leave it outside the chunk model so every
            // reconstructed output discards it.
            break;
        }
    }

    if state.saw_actl
        && (state.fctl_count != state.animation_frames
            || (state.frame_open && !state.frame_has_data))
    {
        return Err(Error::new("invalid APNG frame count"));
    }
    if !state.saw_ihdr || !state.saw_iend {
        return Err(Error::new("invalid PNG trailer"));
    }
    if !state.saw_idat {
        return Err(Error::new("no IDAT chunk found"));
    }
    if has_vestigial_rgba_trns && has_rewrite_sensitive_ancillary && !strip_metadata {
        return Err(Error::new(
            "cannot remove invalid PNG tRNS while preserving rewrite-sensitive metadata",
        ));
    }
    if idat.len() < 6 {
        return Err(Error::new("IDAT zlib stream too small"));
    }
    if (idat[0] & 0x0f) != 8 || (idat[0] >> 4) > 7 || idat[1] & 0x20 != 0 {
        return Err(Error::unsupported_feature("unsupported PNG zlib header"));
    }
    if ((u16::from(idat[0]) << 8) | u16::from(idat[1])) % 31 != 0 {
        return Err(Error::new("invalid PNG zlib header check"));
    }

    let idat_decoded_size = png_image_decoded_size(&state)?;
    Ok(ParsedPng {
        chunks,
        datastream_len: position,
        idat,
        idat_decoded_size,
        fdat_frames,
        fdat_decoded_sizes,
        has_rewrite_sensitive_ancillary,
        has_vestigial_rgba_trns,
    })
}

fn validate_ihdr(kind: [u8; 4], data: &[u8], state: &mut ParseState) -> Result<()> {
    if kind != *b"IHDR" || data.len() != 13 {
        return Err(Error::new("invalid PNG IHDR"));
    }
    state.width = u32::from_be_bytes(data[..4].try_into().unwrap());
    state.height = u32::from_be_bytes(data[4..8].try_into().unwrap());
    state.bit_depth = data[8];
    state.color_type = data[9];
    state.interlace_method = data[12];
    if state.width == 0
        || state.height == 0
        || !valid_bit_depth(state.color_type, state.bit_depth)
        || data[10] != 0
        || data[11] != 0
        || data[12] > 1
    {
        return Err(Error::new("invalid PNG IHDR"));
    }
    state.saw_ihdr = true;
    Ok(())
}

/// Return the exact number of filtered scanline bytes carried by IDAT.
///
/// This is also a security boundary for parallel Max: each branch receives
/// this value as its decode ceiling, so a small malicious zlib stream cannot
/// make two workers retain unexpectedly large payload models.
fn png_image_decoded_size(state: &ParseState) -> Result<u64> {
    png_decoded_size(
        state.width,
        state.height,
        state.bit_depth,
        state.color_type,
        state.interlace_method,
    )
}

fn png_decoded_size(
    width: u32,
    height: u32,
    bit_depth: u8,
    color_type: u8,
    interlace_method: u8,
) -> Result<u64> {
    let samples_per_pixel = match color_type {
        0 | 3 => 1_u64,
        2 => 3,
        4 => 2,
        6 => 4,
        _ => return Err(Error::new("invalid PNG IHDR")),
    };
    let bits_per_pixel = samples_per_pixel * u64::from(bit_depth);
    let pass_size = |width: u64, height: u64| -> Option<u64> {
        if width == 0 || height == 0 {
            return Some(0);
        }
        let row_bits = width.checked_mul(bits_per_pixel)?;
        let row_bytes = row_bits.checked_add(7)? / 8;
        height.checked_mul(row_bytes.checked_add(1)?)
    };

    let width = u64::from(width);
    let height = u64::from(height);
    if interlace_method == 0 {
        return pass_size(width, height)
            .ok_or_else(|| Error::resource_limit("PNG image dimensions are too large"));
    }

    // Adam7 pass geometry: (starting x, starting y, x step, y step).
    const ADAM7: [(u64, u64, u64, u64); 7] = [
        (0, 0, 8, 8),
        (4, 0, 8, 8),
        (0, 4, 4, 8),
        (2, 0, 4, 4),
        (0, 2, 2, 4),
        (1, 0, 2, 2),
        (0, 1, 1, 2),
    ];
    ADAM7
        .iter()
        .try_fold(0_u64, |total, &(start_x, start_y, step_x, step_y)| {
            let pass_width = width
                .checked_sub(start_x)
                .map_or(0, |remaining| remaining.div_ceil(step_x));
            let pass_height = height
                .checked_sub(start_y)
                .map_or(0, |remaining| remaining.div_ceil(step_y));
            total.checked_add(pass_size(pass_width, pass_height)?)
        })
        .ok_or_else(|| Error::resource_limit("PNG image dimensions are too large"))
}

fn validate_palette(kind: [u8; 4], data: &[u8], state: &mut ParseState) -> Result<()> {
    if kind != *b"PLTE" {
        return Ok(());
    }
    if state.saw_plte
        || state.saw_idat
        || data.is_empty()
        || data.len() > 768
        || data.len() % 3 != 0
        || matches!(state.color_type, 0 | 4)
    {
        return Err(Error::new("invalid PNG PLTE"));
    }
    state.palette_entries = (data.len() / 3) as u32;
    if state.color_type == 3 && state.palette_entries > (1_u32 << state.bit_depth) {
        return Err(Error::new("invalid PNG PLTE"));
    }
    state.saw_plte = true;
    Ok(())
}

/// Validate ancillary semantics and report whether a known-invalid chunk must
/// be omitted from every successful output.
///
/// Some exporters leave an indexed palette's tRNS behind after converting the
/// image data and IHDR to RGBA. PLTE itself is a valid suggested palette for
/// color type 6, but tRNS is forbidden because RGBA already carries alpha.
/// Tolerate only that narrow, structurally consistent signature. The chunk is
/// never allowed to influence pixel interpretation and is never preserved.
fn validate_ancillary(kind: [u8; 4], data: &[u8], state: &mut ParseState) -> Result<bool> {
    let mut discard_on_output = false;
    macro_rules! once_before_image {
        ($kind:literal, $seen:ident, $length:expr, $message:literal) => {
            if kind == *$kind {
                if state.$seen || state.saw_plte || state.saw_idat || data.len() != $length {
                    return Err(Error::new($message));
                }
                state.$seen = true;
            }
        };
    }

    once_before_image!(b"cHRM", saw_chrm, 32, "invalid PNG cHRM");
    once_before_image!(b"gAMA", saw_gama, 4, "invalid PNG gAMA");
    once_before_image!(b"cICP", saw_cicp, 4, "invalid PNG cICP");
    once_before_image!(b"mDCV", saw_mdcv, 24, "invalid PNG mDCV");
    once_before_image!(b"cLLI", saw_clli, 8, "invalid PNG cLLI");

    if kind == *b"iCCP" {
        let name_end = find_nul(data, 0);
        if state.saw_iccp
            || state.saw_plte
            || state.saw_idat
            || name_end.is_none()
            || name_end == Some(0)
            || name_end.is_some_and(|end| end > 79)
            || name_end.map_or(true, |end| end + 2 > data.len() || data[end + 1] != 0)
        {
            return Err(Error::new("invalid PNG iCCP"));
        }
        state.saw_iccp = true;
    }

    if kind == *b"sBIT" {
        let expected = match state.color_type {
            0 => 1,
            2 | 3 => 3,
            4 => 2,
            _ => 4,
        };
        if state.saw_sbit || state.saw_plte || state.saw_idat || data.len() != expected {
            return Err(Error::new("invalid PNG sBIT"));
        }
        state.saw_sbit = true;
    }
    if kind == *b"sRGB" {
        if state.saw_srgb || state.saw_plte || state.saw_idat || data.len() != 1 || data[0] > 3 {
            return Err(Error::new("invalid PNG sRGB"));
        }
        state.saw_srgb = true;
    }
    if kind == *b"tRNS" {
        let valid_for_color_type = match state.color_type {
            0 => data.len() == 2,
            2 => data.len() == 6,
            3 => state.saw_plte && data.len() <= state.palette_entries as usize,
            6 => {
                let vestigial_palette_alpha = state.saw_plte
                    && !data.is_empty()
                    && data.len() <= state.palette_entries as usize;
                discard_on_output = vestigial_palette_alpha;
                vestigial_palette_alpha
            }
            _ => false,
        };
        if state.saw_trns || state.saw_idat || !valid_for_color_type {
            return Err(Error::new("invalid PNG tRNS"));
        }
        state.saw_trns = true;
    }
    if kind == *b"bKGD" {
        let expected = match state.color_type {
            0 | 4 => 2,
            3 => 1,
            _ => 6,
        };
        if state.saw_bkgd
            || state.saw_idat
            || (state.color_type == 3 && !state.saw_plte)
            || data.len() != expected
        {
            return Err(Error::new("invalid PNG bKGD"));
        }
        state.saw_bkgd = true;
    }
    if kind == *b"hIST" {
        if state.saw_hist
            || state.saw_idat
            || !state.saw_plte
            || data.len() != state.palette_entries as usize * 2
        {
            return Err(Error::new("invalid PNG hIST"));
        }
        state.saw_hist = true;
    }
    if kind == *b"pHYs" {
        if state.saw_phys || state.saw_idat || data.len() != 9 || data[8] > 1 {
            return Err(Error::new("invalid PNG pHYs"));
        }
        state.saw_phys = true;
    }
    if kind == *b"sCAL" {
        let separator = find_nul(data, 1);
        if state.saw_scal
            || state.saw_idat
            || data.len() < 4
            || !matches!(data[0], 1 | 2)
            || separator.is_none()
            || separator.is_some_and(|offset| {
                offset == 1
                    || offset + 1 == data.len()
                    || !valid_positive_png_float(&data[1..offset])
                    || !valid_positive_png_float(&data[offset + 1..])
            })
        {
            return Err(Error::new("invalid PNG sCAL"));
        }
        state.saw_scal = true;
    }
    if kind == *b"eXIf" {
        if state.saw_exif || state.saw_idat {
            return Err(Error::new("invalid PNG eXIf"));
        }
        state.saw_exif = true;
    }
    if kind == *b"tIME" {
        let year = if data.len() == 7 {
            u16::from_be_bytes(data[..2].try_into().unwrap())
        } else {
            0
        };
        if state.saw_time
            || data.len() != 7
            || year == 0
            || data[2] == 0
            || data[2] > 12
            || data[3] == 0
            || data[3] > 31
            || data[4] > 23
            || data[5] > 59
            || data[6] > 60
        {
            return Err(Error::new("invalid PNG tIME"));
        }
        state.saw_time = true;
    }
    if kind == *b"caBX" {
        // Content Credentials are bound to the PNG datastream and cannot be
        // updated by a Deflate optimizer. PNG additionally permits only one
        // caBX and requires it to precede IDAT.
        if state.saw_cabx || state.saw_idat {
            return Err(Error::new("invalid PNG caBX"));
        }
        state.saw_cabx = true;
    }
    validate_compressed_metadata(kind, data)?;

    if kind == *b"sPLT" {
        if state.saw_idat || data.len() < 3 {
            return Err(Error::new("invalid PNG sPLT"));
        }
        let name_end = find_nul(data, 0);
        if name_end.is_none()
            || name_end == Some(0)
            || name_end.map_or(true, |end| {
                end + 2 > data.len() || !matches!(data[end + 1], 8 | 16)
            })
        {
            return Err(Error::new("invalid PNG sPLT"));
        }
        let name_end = name_end.unwrap();
        let entry_size = if data[name_end + 1] == 8 { 6 } else { 10 };
        if (data.len() - name_end - 2) % entry_size != 0 {
            return Err(Error::new("invalid PNG sPLT"));
        }
    }
    Ok(discard_on_output)
}

/// Validate the decimal notation registered for PNG extension chunks.
///
/// `sCAL` requires a value greater than zero, but converting untrusted text to
/// `f64` would incorrectly reject valid extreme exponents through overflow or
/// underflow. The sign and nonzero decimal digits establish positivity without
/// imposing an artificial numeric range.
fn valid_positive_png_float(value: &[u8]) -> bool {
    if value.is_empty() {
        return false;
    }

    let mut index = 0;
    match value[0] {
        b'+' => index += 1,
        b'-' => return false,
        _ => {}
    }

    let mut integer_digits = 0;
    let mut nonzero_mantissa = false;
    while index < value.len() && value[index].is_ascii_digit() {
        nonzero_mantissa |= value[index] != b'0';
        integer_digits += 1;
        index += 1;
    }

    let mut fraction_digits = 0;
    if value.get(index) == Some(&b'.') {
        index += 1;
        while index < value.len() && value[index].is_ascii_digit() {
            nonzero_mantissa |= value[index] != b'0';
            fraction_digits += 1;
            index += 1;
        }
    }
    if integer_digits == 0 && fraction_digits == 0 {
        return false;
    }

    if matches!(value.get(index).copied(), Some(b'e' | b'E')) {
        index += 1;
        if matches!(value.get(index).copied(), Some(b'+' | b'-')) {
            index += 1;
        }
        let exponent_start = index;
        while index < value.len() && value[index].is_ascii_digit() {
            index += 1;
        }
        if index == exponent_start {
            return false;
        }
    }

    index == value.len() && nonzero_mantissa
}

fn validate_compressed_metadata(kind: [u8; 4], data: &[u8]) -> Result<()> {
    if kind == *b"zTXt" {
        let keyword_end = find_nul(data, 0);
        if keyword_end.is_none()
            || keyword_end == Some(0)
            || keyword_end.is_some_and(|end| end > 79)
            || keyword_end.map_or(true, |end| end + 2 > data.len() || data[end + 1] != 0)
        {
            return Err(Error::new("invalid PNG zTXt"));
        }
    }
    if kind == *b"iTXt" {
        let keyword_end = find_nul(data, 0);
        let Some(keyword_end) = keyword_end else {
            return Err(Error::new("invalid PNG iTXt"));
        };
        if keyword_end == 0
            || keyword_end > 79
            || keyword_end + 3 > data.len()
            || data[keyword_end + 1] > 1
            || (data[keyword_end + 1] == 1 && data[keyword_end + 2] != 0)
        {
            return Err(Error::new("invalid PNG iTXt"));
        }
        let Some(language_end) = find_nul(data, keyword_end + 3) else {
            return Err(Error::new("invalid PNG iTXt"));
        };
        if find_nul(data, language_end + 1).is_none() {
            return Err(Error::new("invalid PNG iTXt"));
        }
    }
    Ok(())
}

fn validate_animation_control(kind: [u8; 4], data: &[u8], state: &mut ParseState) -> Result<()> {
    if kind == *b"acTL" {
        if data.len() != 8 || state.saw_actl || state.saw_idat {
            return Err(Error::new("invalid APNG acTL chunk"));
        }
        state.animation_frames = u32::from_be_bytes(data[..4].try_into().unwrap());
        if state.animation_frames == 0 || u64::from(state.animation_frames) > MAX_APNG_FRAMES as u64
        {
            return Err(Error::new("invalid APNG acTL chunk"));
        }
        state.saw_actl = true;
    }

    if kind == *b"fcTL" {
        if !state.saw_actl
            || data.len() != 26
            || u32::from_be_bytes(data[..4].try_into().unwrap()) != state.sequence_expected
        {
            return Err(Error::new("invalid APNG fcTL chunk"));
        }
        state.sequence_expected += 1;
        state.fctl_count += 1;
        let frame_width = u32::from_be_bytes(data[4..8].try_into().unwrap());
        let frame_height = u32::from_be_bytes(data[8..12].try_into().unwrap());
        let x_offset = u32::from_be_bytes(data[12..16].try_into().unwrap());
        let y_offset = u32::from_be_bytes(data[16..20].try_into().unwrap());
        if frame_width == 0
            || frame_height == 0
            || frame_width > state.width
            || frame_height > state.height
            || x_offset > state.width - frame_width
            || y_offset > state.height - frame_height
            || data[24] > 2
            || data[25] > 1
        {
            return Err(Error::new("invalid APNG fcTL chunk"));
        }

        if !state.saw_idat {
            if state.frame_open
                || x_offset != 0
                || y_offset != 0
                || frame_width != state.width
                || frame_height != state.height
            {
                return Err(Error::new("invalid APNG fcTL chunk"));
            }
            // The default image uses IDAT, validated separately below.
            state.frame_has_data = true;
            state.current_fdat_decoded_size = None;
        } else {
            if state.frame_open && !state.frame_has_data {
                return Err(Error::new("missing APNG frame data"));
            }
            state.frame_has_data = false;
            state.current_fdat_decoded_size = Some(png_decoded_size(
                frame_width,
                frame_height,
                state.bit_depth,
                state.color_type,
                state.interlace_method,
            )?);
        }
        state.frame_open = true;
    }
    Ok(())
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

// Parsing, checksum validation, and PNG reconstruction also consume wall time,
// but only the raw Deflate searches receive the proportional slices below.
// Reserve ten percent outside the non-largest slices (twenty percent for a
// container with more than 32 unique image streams). The final largest stream
// receives the remaining initial search time. Streams that exhaust those fair
// shares are queued for weighted reclaim passes using real time left after all
// initial work. The raw optimizer's ten-percent-plus-one-second grace applies
// within each admitted slice; only the container deadline is a file timeout.
const NON_LARGEST_IMAGE_SEARCH_FRACTION: f64 = 0.90;
const MANY_IMAGE_SEARCH_FRACTION: f64 = 0.80;
const MANY_IMAGE_JOB_THRESHOLD: usize = 32;

fn total_image_decoded_bytes(idat: u64, frames: &[u64]) -> Result<u64> {
    frames
        .iter()
        .try_fold(idat, |total, &size| total.checked_add(size))
        .ok_or_else(|| Error::resource_limit("decoded PNG data exceeds configured safety limit"))
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
        .map_err(|_| Error::new("could not allocate PNG frame model"))?;
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
    .map_err(|_| Error::new("could not allocate PNG frame jobs"))?;
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
        .map_err(|_| Error::new("could not allocate PNG frame results"))?;
    optimized.resize_with(frames.len(), || None);
    let image_floor = if single_image {
        // Establish the genuine normal result before spending the remaining
        // file budget on PNG's bounded max routes. This keeps max output no
        // larger than default without allowing a slow normal floor to starve
        // every max-only route.
        DefaultFloor::CompleteThenBounded
    } else {
        DefaultFloor::Shared
    };
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
            .map_err(|_| Error::new("could not allocate PNG frame results"))?;
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
            optimized[index] = Some(
                try_clone_frame(frame)
                    .ok_or_else(|| Error::new("could not allocate duplicate PNG frame result"))?,
            );
        }
    }
    let mut complete = Vec::new();
    complete
        .try_reserve_exact(optimized.len())
        .map_err(|_| Error::new("could not allocate PNG frame results"))?;
    for frame in optimized {
        complete.push(frame.expect("every APNG frame has a representative result"));
    }

    let _ = reuse_best_exact_frames(&mut complete, &mut || budget.deadline.remaining().is_zero());
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
            .map_err(|_| Error::new("could not allocate PNG frame schedule"))?;

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
            .map_err(|_| Error::new("could not allocate PNG image workers"))?;
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
        .map_err(|_| Error::new("could not allocate PNG frame results"))?;
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
                DefaultFloor::Shared,
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

/// Find the earliest exact-compressed representative in O(n log n) compares.
/// Sorting slices directly avoids adversarial hash-collision buckets.
fn frame_representatives(frames: &[Vec<u8>], decoded_sizes: &[u64]) -> Result<Vec<usize>> {
    if frames.len() != decoded_sizes.len() {
        return Err(Error::new("invalid APNG frame model"));
    }
    let mut order = Vec::new();
    order
        .try_reserve_exact(frames.len())
        .map_err(|_| Error::new("could not allocate PNG frame model"))?;
    order.extend(0..frames.len());
    order.sort_unstable_by(|&left, &right| {
        (frames[left].as_slice(), decoded_sizes[left])
            .cmp(&(frames[right].as_slice(), decoded_sizes[right]))
            .then_with(|| left.cmp(&right))
    });

    let mut representatives = Vec::new();
    representatives
        .try_reserve_exact(frames.len())
        .map_err(|_| Error::new("could not allocate PNG frame model"))?;
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
        .map_err(|_| Error::new("could not allocate PNG compressed metadata"))?;
    body.extend_from_slice(&data[..zlib_offset]);
    body.extend_from_slice(&optimized.data);
    Ok(Some(CompressedBodyOptimization {
        replacement: Some(body),
        source_deflate_bits,
        output_deflate_bits,
        decoded_size,
    }))
}

fn compressed_zlib_offset(kind: [u8; 4], data: &[u8]) -> Option<usize> {
    if kind == *b"zTXt" {
        let keyword_end = find_nul(data, 0)?;
        return (keyword_end + 2 <= data.len() && data[keyword_end + 1] == 0)
            .then_some(keyword_end + 2);
    }
    if kind == *b"iTXt" {
        let keyword_end = find_nul(data, 0)?;
        if keyword_end + 3 > data.len() || data[keyword_end + 1] != 1 || data[keyword_end + 2] != 0
        {
            return None;
        }
        let language_end = find_nul(data, keyword_end + 3)?;
        let translated_end = find_nul(data, language_end + 1)?;
        return Some(translated_end + 1);
    }
    if kind == *b"iCCP" {
        let name_end = find_nul(data, 0)?;
        return (name_end + 2 <= data.len() && data[name_end + 1] == 0).then_some(name_end + 2);
    }
    None
}

fn find_nul(data: &[u8], start: usize) -> Option<usize> {
    data.get(start..)?
        .iter()
        .position(|&byte| byte == 0)
        .map(|offset| start + offset)
}

fn append_chunk(output: &mut Vec<u8>, kind: [u8; 4], data: &[u8]) -> Result<()> {
    let length = u32::try_from(data.len()).map_err(|_| Error::new("PNG chunk too large"))?;
    let encoded_len = data
        .len()
        .checked_add(12)
        .ok_or_else(|| Error::new("PNG chunk too large"))?;
    output
        .try_reserve(encoded_len)
        .map_err(|_| Error::new("could not allocate PNG output"))?;
    output.extend_from_slice(&length.to_be_bytes());
    output.extend_from_slice(&kind);
    output.extend_from_slice(data);
    let crc = crc32_update(crc32_update(0, &kind), data);
    output.extend_from_slice(&crc.to_be_bytes());
    Ok(())
}

fn should_strip(kind: [u8; 4], options: &Options) -> bool {
    should_strip_kind(kind, options.strip_metadata)
}

fn should_strip_kind(kind: [u8; 4], strip_metadata: bool) -> bool {
    strip_metadata && (is_strippable_metadata(kind) || is_unknown_unsafe_ancillary(kind))
}

fn is_strippable_metadata(kind: [u8; 4]) -> bool {
    matches!(
        &kind,
        b"bKGD"
            | b"caBX"
            | b"cHRM"
            | b"cICP"
            | b"cLLI"
            | b"eXIf"
            | b"gAMA"
            | b"hIST"
            | b"iCCP"
            | b"iDOT"
            | b"iTXt"
            | b"mDCV"
            | b"pHYs"
            | b"sCAL"
            | b"sBIT"
            | b"sPLT"
            | b"sRGB"
            | b"sTER"
            | b"dSIG"
            | b"tEXt"
            | b"tIME"
            | b"zTXt"
    )
}

fn is_known_critical(kind: [u8; 4]) -> bool {
    matches!(&kind, b"IHDR" | b"PLTE" | b"IDAT" | b"IEND")
}

fn is_known_ancillary(kind: [u8; 4]) -> bool {
    is_strippable_metadata(kind) || matches!(&kind, b"tRNS" | b"acTL" | b"fcTL" | b"fdAT")
}

fn is_unknown_unsafe_ancillary(kind: [u8; 4]) -> bool {
    kind[0] & 0x20 != 0 && !is_known_ancillary(kind) && kind[3] & 0x20 == 0
}

fn is_rewrite_sensitive_ancillary(kind: [u8; 4]) -> bool {
    // caBX and dSIG authenticate original datastream bytes; iDOT describes
    // Apple's original IDAT layout. Columbo cannot rebuild any of them after
    // changing critical chunks, so default mode takes the same conservative
    // path used for an unrecognized unsafe-to-copy ancillary chunk.
    matches!(&kind, b"caBX" | b"dSIG" | b"iDOT") || is_unknown_unsafe_ancillary(kind)
}

fn valid_chunk_type(kind: [u8; 4]) -> bool {
    kind.iter().all(u8::is_ascii_alphabetic) && kind[2] & 0x20 == 0
}

fn valid_bit_depth(color_type: u8, bit_depth: u8) -> bool {
    match color_type {
        0 => matches!(bit_depth, 1 | 2 | 4 | 8 | 16),
        2 | 4 | 6 => matches!(bit_depth, 8 | 16),
        3 => matches!(bit_depth, 1 | 2 | 4 | 8),
        _ => false,
    }
}

fn be32(input: &[u8], offset: usize) -> Result<u32> {
    input
        .get(offset..offset + 4)
        .and_then(|bytes| bytes.try_into().ok())
        .map(u32::from_be_bytes)
        .ok_or_else(|| Error::new("truncated PNG chunk"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::checksum::adler32;

    fn chunk(kind: [u8; 4], data: &[u8]) -> Vec<u8> {
        let mut bytes = Vec::new();
        append_chunk(&mut bytes, kind, data).unwrap();
        bytes
    }

    fn ihdr() -> [u8; 13] {
        let mut data = [0_u8; 13];
        data[3] = 1; // width
        data[7] = 1; // height
        data[8] = 8; // grayscale, eight bits per sample
        data
    }

    fn frame_control(sequence: u32) -> Vec<u8> {
        let mut body = Vec::new();
        body.extend_from_slice(&sequence.to_be_bytes());
        body.extend_from_slice(&1_u32.to_be_bytes()); // width
        body.extend_from_slice(&1_u32.to_be_bytes()); // height
        body.extend_from_slice(&0_u32.to_be_bytes()); // x offset
        body.extend_from_slice(&0_u32.to_be_bytes()); // y offset
        body.extend_from_slice(&1_u16.to_be_bytes()); // delay numerator
        body.extend_from_slice(&10_u16.to_be_bytes()); // delay denominator
        body.extend_from_slice(&[0, 0]); // dispose and blend operations
        body
    }

    fn black_scanline_zlib() -> Vec<u8> {
        vec![
            0x78, 0x01, // zlib header
            0x01, 0x02, 0x00, 0xfd, 0xff, 0x00, 0x00, // stored Deflate block
            0x00, 0x02, 0x00, 0x01, // Adler-32([filter=0, pixel=0])
        ]
    }

    fn stored_zlib(decoded: &[u8]) -> Vec<u8> {
        let length = u16::try_from(decoded.len()).unwrap();
        let mut stream = vec![0x78, 0x01, 0x01];
        stream.extend_from_slice(&length.to_le_bytes());
        stream.extend_from_slice(&(!length).to_le_bytes());
        stream.extend_from_slice(decoded);
        stream.extend_from_slice(&adler32(decoded).to_be_bytes());
        stream
    }

    fn assert_maximum_flevel(stream: &[u8]) {
        assert!(zlib::has_rfc1950_header(stream));
        assert_eq!(stream[1] >> 6, 3);
    }

    #[test]
    fn validates_crc_before_decoding_idat() {
        let mut input = SIGNATURE.to_vec();
        let mut ihdr = [0_u8; 13];
        ihdr[3] = 1;
        ihdr[7] = 1;
        ihdr[8] = 8;
        ihdr[9] = 0;
        input.extend(chunk(*b"IHDR", &ihdr));
        let mut bad_idat = chunk(*b"IDAT", &[0x78, 0x01, 1, 0, 0, 0]);
        *bad_idat.last_mut().unwrap() ^= 1;
        input.extend(bad_idat);
        input.extend(chunk(*b"IEND", &[]));

        let error = optimize(&input, &Options::default()).unwrap_err();
        assert_eq!(error.message(), "bad PNG chunk CRC");
    }

    #[test]
    fn strip_does_not_turn_bad_metadata_crc_into_a_repair() {
        let mut input = SIGNATURE.to_vec();
        input.extend(chunk(*b"IHDR", &ihdr()));
        let mut bad_cabx = chunk(*b"caBX", b"credential");
        *bad_cabx.last_mut().unwrap() ^= 1;
        input.extend(bad_cabx);
        input.extend(chunk(*b"IDAT", &black_scanline_zlib()));
        input.extend(chunk(*b"IEND", &[]));

        for strip_metadata in [false, true] {
            let error = optimize(
                &input,
                &Options {
                    strip_metadata,
                    ..Options::default()
                },
            )
            .unwrap_err();
            assert_eq!(error.message(), "bad PNG chunk CRC");
        }
    }

    #[test]
    fn rejects_nonconsecutive_idat_chunks() {
        let mut input = SIGNATURE.to_vec();
        let mut ihdr = [0_u8; 13];
        ihdr[3] = 1;
        ihdr[7] = 1;
        ihdr[8] = 8;
        input.extend(chunk(*b"IHDR", &ihdr));
        input.extend(chunk(*b"IDAT", &[0x78, 0x01, 1]));
        input.extend(chunk(*b"tEXt", b"x"));
        input.extend(chunk(*b"IDAT", &[0, 0, 0]));
        input.extend(chunk(*b"IEND", &[]));

        let error = optimize(&input, &Options::default()).unwrap_err();
        assert_eq!(error.message(), "non-consecutive IDAT chunk");
    }

    #[test]
    fn optimizes_and_revalidates_a_minimal_png() {
        let mut input = SIGNATURE.to_vec();
        input.extend(chunk(*b"IHDR", &ihdr()));
        input.extend(chunk(*b"IDAT", &black_scanline_zlib()));
        input.extend(chunk(*b"IEND", &[]));

        let result = optimize(&input, &Options::default()).unwrap();
        assert!(result.data.len() <= input.len());
        let parsed = parse(&result.data, false).unwrap();
        assert_maximum_flevel(&parsed.idat);
    }

    #[test]
    fn strips_everything_after_iend_in_every_mode() {
        let mut input = SIGNATURE.to_vec();
        input.extend(chunk(*b"IHDR", &ihdr()));
        input.extend(chunk(*b"IDAT", &black_scanline_zlib()));
        input.extend(chunk(*b"IEND", &[]));
        input.extend_from_slice(b"opaque payload after the PNG datastream");

        let modes = [
            Options::default(),
            Options {
                strict: false,
                ..Options::default()
            },
            Options {
                strip_metadata: true,
                ..Options::default()
            },
            Options {
                exhaustive: true,
                timeout: Duration::ZERO,
                ..Options::default()
            },
        ];
        let iend = chunk(*b"IEND", &[]);
        for options in modes {
            let result = optimize(&input, &options).unwrap();
            assert!(result.data.ends_with(&iend));
            let saved_bytes = u64::try_from(input.len() - result.data.len()).unwrap();
            assert_eq!(result.bits_saved, saved_bytes * 8);
            let parsed = parse(&result.data, false).unwrap();
            assert_eq!(parsed.datastream_len, result.data.len());
        }
    }

    #[test]
    fn strips_everything_after_an_apng_iend() {
        let zlib = black_scanline_zlib();
        let mut input = SIGNATURE.to_vec();
        input.extend(chunk(*b"IHDR", &ihdr()));
        let mut actl = Vec::new();
        actl.extend_from_slice(&2_u32.to_be_bytes());
        actl.extend_from_slice(&0_u32.to_be_bytes());
        input.extend(chunk(*b"acTL", &actl));
        input.extend(chunk(*b"fcTL", &frame_control(0)));
        input.extend(chunk(*b"IDAT", &zlib));
        input.extend(chunk(*b"fcTL", &frame_control(1)));
        let mut frame_data = 2_u32.to_be_bytes().to_vec();
        frame_data.extend_from_slice(&zlib);
        input.extend(chunk(*b"fdAT", &frame_data));
        input.extend(chunk(*b"IEND", &[]));
        input.extend_from_slice(b"payload after the APNG datastream");

        let result = optimize(&input, &Options::default()).unwrap();
        assert!(result.data.ends_with(&chunk(*b"IEND", &[])));
        let saved_bytes = u64::try_from(input.len() - result.data.len()).unwrap();
        assert_eq!(result.bits_saved, saved_bytes * 8);
        let parsed = parse(&result.data, false).unwrap();
        assert_eq!(parsed.datastream_len, result.data.len());
        assert_eq!(parsed.fdat_frames.len(), 1);
    }

    #[test]
    fn accepts_and_removes_vestigial_rgba_trns_in_every_mode() {
        let mut header = ihdr();
        header[9] = 6;
        let decoded = [0, 10, 20, 30, 40];
        let mut input = SIGNATURE.to_vec();
        input.extend(chunk(*b"IHDR", &header));
        input.extend(chunk(*b"PLTE", &[0, 0, 0, 255, 255, 255]));
        input.extend(chunk(*b"tRNS", &[0, 255]));
        input.extend(chunk(*b"IDAT", &stored_zlib(&decoded)));
        input.extend(chunk(*b"IEND", &[]));

        let modes = [
            Options::default(),
            Options {
                strict: false,
                ..Options::default()
            },
            Options {
                strip_metadata: true,
                ..Options::default()
            },
            Options {
                exhaustive: true,
                timeout: Duration::ZERO,
                ..Options::default()
            },
        ];
        for options in modes {
            let result = optimize(&input, &options).unwrap();
            let parsed = parse(&result.data, false).unwrap();
            assert_eq!(parsed.chunks[0].data[9], 6);
            assert!(parsed.chunks.iter().any(|chunk| chunk.kind == *b"PLTE"));
            assert!(parsed.chunks.iter().all(|chunk| chunk.kind != *b"tRNS"));
            assert!(zlib_decodes_to(&parsed.idat, &decoded));
            let saved_bytes = u64::try_from(input.len() - result.data.len()).unwrap();
            assert_eq!(result.bits_saved, saved_bytes * 8);
        }
    }

    #[test]
    fn removes_vestigial_rgba_trns_and_trailing_bytes_from_apng() {
        let mut header = ihdr();
        header[9] = 6;
        let decoded = [0, 10, 20, 30, 40];
        let zlib = stored_zlib(&decoded);
        let mut input = SIGNATURE.to_vec();
        input.extend(chunk(*b"IHDR", &header));
        let mut actl = Vec::new();
        actl.extend_from_slice(&2_u32.to_be_bytes());
        actl.extend_from_slice(&0_u32.to_be_bytes());
        input.extend(chunk(*b"acTL", &actl));
        input.extend(chunk(*b"PLTE", &[0, 0, 0]));
        input.extend(chunk(*b"tRNS", &[0]));
        input.extend(chunk(*b"fcTL", &frame_control(0)));
        input.extend(chunk(*b"IDAT", &zlib));
        input.extend(chunk(*b"fcTL", &frame_control(1)));
        let mut frame_data = 2_u32.to_be_bytes().to_vec();
        frame_data.extend_from_slice(&zlib);
        input.extend(chunk(*b"fdAT", &frame_data));
        input.extend(chunk(*b"IEND", &[]));
        input.extend_from_slice(b"payload after IEND");

        let result = optimize(&input, &Options::default()).unwrap();
        let parsed = parse(&result.data, false).unwrap();
        assert_eq!(parsed.datastream_len, result.data.len());
        assert!(parsed.chunks.iter().all(|chunk| chunk.kind != *b"tRNS"));
        assert!(zlib_decodes_to(&parsed.idat, &decoded));
        assert_eq!(parsed.fdat_frames.len(), 1);
        assert!(zlib_decodes_to(&parsed.fdat_frames[0], &decoded));
        let saved_bytes = u64::try_from(input.len() - result.data.len()).unwrap();
        assert_eq!(result.bits_saved, saved_bytes * 8);
    }

    #[test]
    fn valid_indexed_trns_remains_pixel_semantics_even_with_strip() {
        let mut header = ihdr();
        header[9] = 3;
        let mut input = SIGNATURE.to_vec();
        input.extend(chunk(*b"IHDR", &header));
        input.extend(chunk(*b"PLTE", &[0, 0, 0]));
        input.extend(chunk(*b"tRNS", &[0]));
        input.extend(chunk(*b"IDAT", &black_scanline_zlib()));
        input.extend(chunk(*b"IEND", &[]));

        for strip_metadata in [false, true] {
            let result = optimize(
                &input,
                &Options {
                    strip_metadata,
                    ..Options::default()
                },
            )
            .unwrap();
            let parsed = parse(&result.data, false).unwrap();
            assert!(parsed.chunks.iter().any(|chunk| chunk.kind == *b"tRNS"));
        }
    }

    #[test]
    fn rejects_rgba_trns_outside_the_vestigial_palette_signature() {
        let mut header = ihdr();
        header[9] = 6;
        let cases: [(&[u8], Option<&[u8]>); 3] = [
            (&[0], None),
            (&[], Some(&[0, 0, 0])),
            (&[0, 255], Some(&[0, 0, 0])),
        ];

        for (transparency, palette) in cases {
            let mut input = SIGNATURE.to_vec();
            input.extend(chunk(*b"IHDR", &header));
            if let Some(palette) = palette {
                input.extend(chunk(*b"PLTE", palette));
            }
            input.extend(chunk(*b"tRNS", transparency));
            input.extend(chunk(*b"IDAT", &stored_zlib(&[0, 0, 0, 0, 0])));
            input.extend(chunk(*b"IEND", &[]));

            for strip_metadata in [false, true] {
                let error = optimize(
                    &input,
                    &Options {
                        strip_metadata,
                        ..Options::default()
                    },
                )
                .unwrap_err();
                assert_eq!(error.message(), "invalid PNG tRNS");
            }
        }
    }

    #[test]
    fn rejects_vestigial_rgba_trns_after_idat_or_when_duplicated() {
        let mut header = ihdr();
        header[9] = 6;
        let idat = chunk(*b"IDAT", &stored_zlib(&[0, 0, 0, 0, 0]));
        let trns = chunk(*b"tRNS", &[0]);

        let mut after_idat = SIGNATURE.to_vec();
        after_idat.extend(chunk(*b"IHDR", &header));
        after_idat.extend(chunk(*b"PLTE", &[0, 0, 0]));
        after_idat.extend(&idat);
        after_idat.extend(&trns);
        after_idat.extend(chunk(*b"IEND", &[]));

        let mut duplicated = SIGNATURE.to_vec();
        duplicated.extend(chunk(*b"IHDR", &header));
        duplicated.extend(chunk(*b"PLTE", &[0, 0, 0]));
        duplicated.extend(&trns);
        duplicated.extend(&trns);
        duplicated.extend(&idat);
        duplicated.extend(chunk(*b"IEND", &[]));

        for input in [&after_idat, &duplicated] {
            let error = optimize(input, &Options::default()).unwrap_err();
            assert_eq!(error.message(), "invalid PNG tRNS");
        }
    }

    #[test]
    fn vestigial_rgba_trns_does_not_hide_a_wrong_color_mode() {
        let mut header = ihdr();
        header[9] = 6;
        let mut input = SIGNATURE.to_vec();
        input.extend(chunk(*b"IHDR", &header));
        input.extend(chunk(*b"PLTE", &[0, 0, 0]));
        input.extend(chunk(*b"tRNS", &[0]));
        // This is a two-byte grayscale scanline, not the five bytes required
        // for a one-pixel, eight-bit RGBA scanline.
        input.extend(chunk(*b"IDAT", &black_scanline_zlib()));
        input.extend(chunk(*b"IEND", &[]));

        let error = optimize(&input, &Options::default()).unwrap_err();
        assert_eq!(error.message(), "PNG image data size does not match IHDR");
    }

    #[test]
    fn still_rejects_trns_for_grayscale_alpha() {
        let mut header = ihdr();
        header[9] = 4;
        let mut input = SIGNATURE.to_vec();
        input.extend(chunk(*b"IHDR", &header));
        input.extend(chunk(*b"tRNS", &[0]));
        input.extend(chunk(*b"IDAT", &stored_zlib(&[0, 0, 0])));
        input.extend(chunk(*b"IEND", &[]));

        let error = optimize(&input, &Options::default()).unwrap_err();
        assert_eq!(error.message(), "invalid PNG tRNS");
    }

    #[test]
    fn static_png_has_one_physical_deflate_stream() {
        let mut input = SIGNATURE.to_vec();
        input.extend(chunk(*b"IHDR", &ihdr()));
        input.extend(chunk(*b"IDAT", &black_scanline_zlib()));
        input.extend(chunk(*b"IEND", &[]));

        assert_eq!(deflate_stream_count(&input, false).unwrap(), 1);
    }

    #[test]
    fn compressed_metadata_counts_after_image_streams() {
        let mut input = SIGNATURE.to_vec();
        input.extend(chunk(*b"IHDR", &ihdr()));
        input.extend(chunk(*b"IDAT", &black_scanline_zlib()));
        let mut text = b"Comment\0\0".to_vec();
        text.extend(black_scanline_zlib());
        input.extend(chunk(*b"zTXt", &text));
        input.extend(chunk(*b"IEND", &[]));

        assert_eq!(deflate_stream_count(&input, false).unwrap(), 2);
        let parsed = parse(&input, false).unwrap();
        assert_eq!(
            metadata_stream_ids(&parsed)
                .unwrap()
                .into_iter()
                .flatten()
                .collect::<Vec<_>>(),
            vec![2]
        );
    }

    #[test]
    fn duplicate_apng_frames_share_a_named_visual_group() {
        assert_eq!(
            image_job_stream_group(ImageJob::Idat, &[0, 0, 2]),
            (1, vec![])
        );
        assert_eq!(
            image_job_stream_group(ImageJob::Frame(0), &[0, 0, 2]),
            (2, vec![3])
        );
        assert_eq!(
            image_job_stream_group(ImageJob::Frame(2), &[0, 0, 2]),
            (4, vec![])
        );
    }

    #[test]
    fn idat_reports_same_byte_bit_savings() {
        let mut header = ihdr();
        // The synthetic raw stream expands to 168 bytes. Model that as one
        // filter byte followed by 167 grayscale samples.
        header[..4].copy_from_slice(&167_u32.to_be_bytes());
        let mut input = SIGNATURE.to_vec();
        input.extend(chunk(*b"IHDR", &header));
        input.extend(chunk(*b"IDAT", &super::super::same_byte_bit_win_zlib()));
        input.extend(chunk(*b"IEND", &[]));

        let optimized = optimize(&input, &Options::default()).unwrap();

        assert_eq!(optimized.data.len(), input.len());
        assert_eq!(optimized.bits_saved, 1);
    }

    #[test]
    fn bounded_max_deadline_still_returns_a_valid_png() {
        let mut input = SIGNATURE.to_vec();
        input.extend(chunk(*b"IHDR", &ihdr()));
        input.extend(chunk(*b"IDAT", &black_scanline_zlib()));
        input.extend(chunk(*b"IEND", &[]));
        let options = Options {
            exhaustive: true,
            timeout: Duration::ZERO,
            ..Options::default()
        };

        let result = optimize(&input, &options).unwrap();
        assert!(result.timed_out);
        assert!(result.data.len() <= input.len());
        parse(&result.data, false).unwrap();
    }

    #[test]
    fn image_decoded_size_matches_scanline_and_adam7_geometry() {
        let indexed = ParseState {
            width: 13,
            height: 7,
            bit_depth: 1,
            color_type: 3,
            ..ParseState::default()
        };
        // Each row contains one filter byte and ceil(13 / 8) data bytes.
        assert_eq!(png_image_decoded_size(&indexed).unwrap(), 21);

        let interlaced = ParseState {
            width: 8,
            height: 8,
            bit_depth: 8,
            color_type: 0,
            interlace_method: 1,
            ..ParseState::default()
        };
        // The seven filtered Adam7 passes contain 2+2+3+6+10+20+36 bytes.
        assert_eq!(png_image_decoded_size(&interlaced).unwrap(), 79);
    }

    #[test]
    fn rejects_idat_size_mismatch_in_default_and_max() {
        let mut input = SIGNATURE.to_vec();
        input.extend(chunk(*b"IHDR", &ihdr()));
        // A 1x1 eight-bit grayscale image requires exactly two bytes: one
        // filter byte and one sample. The checksum is valid for three bytes.
        input.extend(chunk(*b"IDAT", &stored_zlib(&[0, 0, 0])));
        input.extend(chunk(*b"IEND", &[]));

        for exhaustive in [false, true] {
            let options = Options {
                exhaustive,
                timeout: Duration::ZERO,
                ..Options::default()
            };
            let error = optimize(&input, &options).unwrap_err();
            assert_eq!(error.message(), "PNG image data size does not match IHDR");
        }
    }

    #[test]
    fn parallel_max_image_requires_bounded_input_and_decoded_work() {
        assert!(parallel_max_image_is_bounded(
            PARALLEL_MAX_IMAGE_COMPRESSED,
            PARALLEL_MAX_IMAGE_DECODED,
        ));
        assert!(!parallel_max_image_is_bounded(
            PARALLEL_MAX_IMAGE_COMPRESSED + 1,
            PARALLEL_MAX_IMAGE_DECODED,
        ));
        assert!(!parallel_max_image_is_bounded(
            PARALLEL_MAX_IMAGE_COMPRESSED,
            PARALLEL_MAX_IMAGE_DECODED + 1,
        ));
        assert!(parallel_multi_image_is_bounded(
            PARALLEL_MAX_IMAGE_COMPRESSED,
            PARALLEL_MAX_IMAGE_DECODED,
        ));
        assert!(!parallel_multi_image_is_bounded(
            PARALLEL_MAX_IMAGE_COMPRESSED + 1,
            PARALLEL_MAX_IMAGE_DECODED,
        ));
    }

    #[test]
    fn parallel_image_timeout_accounts_for_each_child_grace() {
        let timeout = parallel_image_job_timeout(Duration::from_secs(10), 2, 1, 2);
        assert!(timeout >= Duration::from_millis(4_540));
        assert!(timeout <= Duration::from_millis(4_550));
        assert_eq!(
            parallel_image_job_timeout(Duration::from_secs(10), 1, 1, 1),
            Duration::from_secs(10)
        );
    }

    #[test]
    fn parallel_max_selection_is_byte_first_then_bit_first() {
        let stream = |length, bits, timed_out| zlib::StreamOptimization {
            data: vec![0; length],
            info: Some(RawInfo {
                deflate_bits: bits,
                ..RawInfo::default()
            }),
            timed_out,
        };

        let selected = best_zlib_optimization(stream(10, 100, false), stream(10, 99, true));
        assert_eq!(selected.info.unwrap().deflate_bits, 99);
        assert!(selected.timed_out);

        let selected = best_zlib_optimization(stream(10, 90, true), stream(9, 100, false));
        assert_eq!(selected.data.len(), 9);
        assert!(selected.timed_out);
    }

    #[test]
    fn validates_scal_floats_without_imposing_a_machine_numeric_range() {
        let valid: &[&[u8]] = &[
            b"1",
            b"+1.",
            b".5",
            b"0.0001",
            b"5e-324",
            b"1E+999999999999999999999999999999999999999",
        ];
        for value in valid {
            assert!(valid_positive_png_float(value), "{value:?}");
        }

        let invalid: &[&[u8]] = &[
            b"",
            b"0",
            b"+0.0e999",
            b"-1",
            b".",
            b"+.",
            b"1e",
            b"1e+",
            b"1 0",
            b"1_0",
            b"NaN",
            b"inf",
        ];
        for value in invalid {
            assert!(!valid_positive_png_float(value), "{value:?}");
        }
    }

    #[test]
    fn rejects_malformed_duplicate_and_misordered_scal_chunks() {
        let valid = [1, b'+', b'1', b'.', b'0', b'e', b'-', b'9', 0, b'.', b'5'];
        let mut state = ParseState::default();
        validate_ancillary(*b"sCAL", &valid, &mut state).unwrap();

        let duplicate = validate_ancillary(*b"sCAL", &valid, &mut state).unwrap_err();
        assert_eq!(duplicate.message(), "invalid PNG sCAL");

        let invalid: &[&[u8]] = &[
            &[0, b'1', 0, b'1'],
            &[3, b'1', 0, b'1'],
            &[1, b'1'],
            &[1, 0, b'1'],
            &[1, b'1', 0],
            &[1, b'1', 0, b'1', 0],
            &[1, b'0', 0, b'1'],
            &[1, b'1', 0, b'-', b'1'],
        ];
        for data in invalid {
            let error = validate_ancillary(*b"sCAL", data, &mut ParseState::default()).unwrap_err();
            assert_eq!(error.message(), "invalid PNG sCAL", "{data:?}");
        }

        let mut after_idat = ParseState {
            saw_idat: true,
            ..ParseState::default()
        };
        let misordered = validate_ancillary(*b"sCAL", &valid, &mut after_idat).unwrap_err();
        assert_eq!(misordered.message(), "invalid PNG sCAL");
    }

    #[test]
    fn coalesces_ten_idat_chunks_and_preserves_registered_scal() {
        // This is the already-minimal 1x1 zlib stream used by the 24-chunk
        // corpus case. Only removing nine redundant IDAT wrappers can shrink
        // it, for an exact saving of 9 * 12 bytes.
        let zlib = [0x78, 0x01, 0x63, 0xf8, 0x0f, 0x00, 0x01, 0x01, 0x01, 0x00];
        let scal = [1, b'1', b'.', b'0', 0, b'1', b'.', b'0'];
        let mut input = SIGNATURE.to_vec();
        input.extend(chunk(*b"IHDR", &ihdr()));
        input.extend(chunk(*b"sCAL", &scal));
        for byte in zlib {
            input.extend(chunk(*b"IDAT", &[byte]));
        }
        input.extend(chunk(*b"IEND", &[]));

        let result = optimize(&input, &Options::default()).unwrap();
        assert_eq!(input.len() - result.data.len(), 108);

        let parsed = parse(&result.data, false).unwrap();
        assert_eq!(
            parsed
                .chunks
                .iter()
                .filter(|chunk| chunk.kind == *b"IDAT")
                .count(),
            1
        );
        let preserved = parsed
            .chunks
            .iter()
            .find(|chunk| chunk.kind == *b"sCAL")
            .expect("registered sCAL metadata should be preserved");
        assert_eq!(preserved.data, scal);
    }

    #[test]
    fn zero_search_budget_still_validates_every_frame_stream() {
        let invalid_frame = vec![
            0x78, 0x01, 0x03, 0x00, // valid empty Deflate stream
            0x00, 0x00, 0x00, 0x02, // wrong Adler-32 for empty data
        ];
        let options = Options {
            exhaustive: true,
            timeout: Duration::ZERO,
            ..Options::default()
        };
        let mut budget = DecodeBudget {
            remaining: options.max_decoded_bytes,
            deadline: SearchDeadline::new(&options),
        };

        let error = optimize_image_streams(
            &black_scanline_zlib(),
            2,
            &[invalid_frame],
            &[0],
            false,
            &options,
            &mut budget,
        )
        .unwrap_err();
        assert_eq!(error.message(), "zlib Adler-32 mismatch");
    }

    #[test]
    fn checksum_tuple_is_only_a_filter_for_cross_frame_reuse() {
        let first_data = stored_zlib(&[0, 0]);
        let second_data = stored_zlib(&[0, 1]);
        assert_eq!(first_data.len(), second_data.len());

        // Deliberately forge equal summary fields. The second representation
        // appears cheaper by bit count, but its exact decoded bytes differ.
        let summary = RawInfo {
            size: 2,
            crc32: 7,
            adler32: 11,
            deflate_bits: 80,
            ..RawInfo::default()
        };
        let mut frames = vec![
            FrameOptimization {
                data: first_data.clone(),
                info: Some(summary.clone()),
            },
            FrameOptimization {
                data: second_data.clone(),
                info: Some(RawInfo {
                    deflate_bits: 79,
                    ..summary
                }),
            },
        ];

        reuse_best_exact_frames(&mut frames, &mut || false);
        assert_eq!(frames[0].data, first_data);
        assert_eq!(frames[1].data, second_data);
    }

    #[test]
    fn cross_frame_reuse_preserves_each_members_source_bit_count() {
        let source = stored_zlib(&[0, 0]);
        let optimized = zlib::optimize_embedded(
            &source,
            &Options::default(),
            2,
            false,
            DefaultFloor::Complete,
        )
        .unwrap();
        assert!(optimized.data.len() < source.len());
        let best_info = optimized.info.unwrap();
        let mut source_info = best_info.clone();
        source_info.source_deflate_bits = 101;
        source_info.deflate_bits = 101;
        let mut donor_info = best_info;
        donor_info.source_deflate_bits = 202;
        let donor_bits = donor_info.deflate_bits;
        let mut frames = vec![
            FrameOptimization {
                data: source,
                info: Some(source_info),
            },
            FrameOptimization {
                data: optimized.data,
                info: Some(donor_info),
            },
        ];

        reuse_best_exact_frames(&mut frames, &mut || false);

        let replaced = frames[0].info.as_ref().unwrap();
        assert_eq!(replaced.source_deflate_bits, 101);
        assert_eq!(replaced.deflate_bits, donor_bits);
    }

    #[test]
    fn exact_frame_reuse_stops_when_its_deadline_is_spent() {
        let summary = RawInfo {
            size: 2,
            crc32: 7,
            adler32: 11,
            ..RawInfo::default()
        };
        let mut frames = vec![
            FrameOptimization {
                data: stored_zlib(&[0, 0]),
                info: Some(RawInfo {
                    deflate_bits: 80,
                    ..summary.clone()
                }),
            },
            FrameOptimization {
                data: stored_zlib(&[0, 1]),
                info: Some(RawInfo {
                    deflate_bits: 79,
                    ..summary
                }),
            },
        ];
        let before: Vec<Vec<u8>> = frames.iter().map(|frame| frame.data.clone()).collect();

        assert!(reuse_best_exact_frames(&mut frames, &mut || true));
        assert_eq!(
            frames
                .iter()
                .map(|frame| frame.data.clone())
                .collect::<Vec<_>>(),
            before
        );
    }

    #[test]
    fn identical_frame_grouping_uses_the_earliest_exact_source() {
        let frames = vec![
            b"beta".to_vec(),
            b"alpha".to_vec(),
            b"beta".to_vec(),
            b"alpha".to_vec(),
            b"gamma".to_vec(),
        ];
        assert_eq!(
            frame_representatives(&frames, &[1; 5]).unwrap(),
            [0, 1, 0, 1, 4]
        );
        assert_eq!(
            frame_representatives(&frames, &[1, 1, 2, 1, 1]).unwrap(),
            [0, 1, 2, 1, 4]
        );
    }

    #[test]
    fn rejects_reserved_png_zlib_window_exponent() {
        let mut input = SIGNATURE.to_vec();
        input.extend(chunk(*b"IHDR", &ihdr()));
        input.extend(chunk(
            *b"IDAT",
            &[0x88, 0x1c, 0x03, 0x00, 0x00, 0x00, 0x00, 0x01],
        ));
        input.extend(chunk(*b"IEND", &[]));

        let error = optimize(&input, &Options::default()).unwrap_err();
        assert_eq!(error.message(), "unsupported PNG zlib header");
    }

    #[test]
    fn rejects_pathological_apng_frame_counts_early() {
        let mut state = ParseState::default();
        let mut control = [0_u8; 8];
        control[..4].copy_from_slice(&((MAX_APNG_FRAMES as u32) + 1).to_be_bytes());

        let error = validate_animation_control(*b"acTL", &control, &mut state).unwrap_err();
        assert_eq!(error.message(), "invalid APNG acTL chunk");
    }

    #[test]
    fn rejects_pathological_compressed_metadata_counts_early() {
        let mut input = SIGNATURE.to_vec();
        input.extend(chunk(*b"IHDR", &ihdr()));
        for _ in 0..=MAX_COMPRESSED_METADATA_STREAMS {
            input.extend(chunk(*b"zTXt", b"k\0\0"));
        }

        let error = match parse(&input, false) {
            Err(error) => error,
            Ok(_) => panic!("excess compressed metadata should fail"),
        };
        assert_eq!(
            error.message(),
            "PNG contains too many compressed metadata streams"
        );
    }

    #[test]
    fn rejects_pathological_chunk_counts_early() {
        let mut input = SIGNATURE.to_vec();
        input.extend(chunk(*b"IHDR", &ihdr()));
        for _ in 0..MAX_PNG_CHUNKS {
            input.extend(chunk(*b"aaAa", &[]));
        }

        let error = match parse(&input, false) {
            Err(error) => error,
            Ok(_) => panic!("excess PNG chunks should fail"),
        };
        assert_eq!(error.message(), "PNG contains too many chunks");
    }

    #[test]
    fn image_timeout_is_proportional_and_gives_largest_the_remainder() {
        let configured = Duration::from_secs(10);
        let remaining = Duration::from_secs(8);

        assert_eq!(
            image_stream_timeout(
                configured,
                remaining,
                2,
                10,
                NON_LARGEST_IMAGE_SEARCH_FRACTION,
                false,
            ),
            Duration::from_millis(1_800)
        );
        assert_eq!(
            image_stream_timeout(
                configured,
                remaining,
                2,
                10,
                MANY_IMAGE_SEARCH_FRACTION,
                false,
            ),
            Duration::from_millis(1_600)
        );
        assert_eq!(
            image_stream_timeout(
                configured,
                remaining,
                2,
                10,
                NON_LARGEST_IMAGE_SEARCH_FRACTION,
                true,
            ),
            Duration::from_millis(7_840)
        );
        assert_eq!(
            image_stream_timeout(
                configured,
                Duration::ZERO,
                2,
                10,
                NON_LARGEST_IMAGE_SEARCH_FRACTION,
                true,
            ),
            Duration::ZERO
        );
    }

    #[test]
    fn image_scheduler_obeys_a_tiny_wall_budget_in_both_modes() {
        let source_idat = black_scanline_zlib();
        // Distinct payloads prevent representative folding, so this exercises
        // the actual multi-job schedule rather than one cloned job.
        let frames: Vec<_> = (0_u8..12).map(|value| stored_zlib(&[value, 0])).collect();
        let source_lengths: Vec<_> = frames.iter().map(Vec::len).collect();
        for exhaustive in [false, true] {
            let options = Options {
                exhaustive,
                timeout: Duration::from_millis(20),
                ..Options::default()
            };
            let mut budget = DecodeBudget {
                remaining: options.max_decoded_bytes,
                deadline: SearchDeadline::new(&options),
            };

            let started = std::time::Instant::now();
            let (optimized_idat, optimized_frames) = optimize_image_streams(
                &source_idat,
                2,
                &frames,
                &[2; 12],
                false,
                &options,
                &mut budget,
            )
            .unwrap();

            assert!(started.elapsed() < Duration::from_secs(2));
            assert_eq!(optimized_frames.len(), frames.len());
            assert!(optimized_idat.data.len() <= source_idat.len());
            assert!(optimized_frames
                .iter()
                .zip(&source_lengths)
                .all(|(frame, &source_len)| frame.data.len() <= source_len));
        }
    }

    #[test]
    fn image_scheduler_reclaims_a_local_slice_without_a_file_timeout() {
        let idat = black_scanline_zlib();
        let slice_options = Options {
            timeout: Duration::ZERO,
            ..Options::default()
        };
        let initial = run_png_image_zlib(&idat, &slice_options, 2, DefaultFloor::Shared).unwrap();
        assert!(initial.timed_out);

        let options = Options {
            timeout: Duration::from_secs(1),
            ..Options::default()
        };
        let deadline = SearchDeadline::new(&options);
        let mut results = vec![(ImageJob::Idat, initial)];
        reclaim_timed_out_image_jobs(
            &mut results,
            &idat,
            2,
            &[],
            &[],
            &[],
            &[],
            DefaultFloor::Shared,
            false,
            &options,
            &deadline,
        )
        .unwrap();

        assert!(!results[0].1.timed_out);
        assert!(!deadline.is_expired());
    }

    #[test]
    fn duplicate_frames_share_search_but_each_consume_decode_budget() {
        let idat = stored_zlib(b"");
        let frame = black_scanline_zlib(); // Two decoded scanline bytes.
        let frames = vec![frame.clone(), frame];

        let options = Options {
            timeout: Duration::ZERO,
            max_decoded_bytes: 3,
            ..Options::default()
        };
        let mut budget = DecodeBudget {
            remaining: options.max_decoded_bytes,
            deadline: SearchDeadline::new(&options),
        };
        let error =
            optimize_image_streams(&idat, 0, &frames, &[2, 2], false, &options, &mut budget)
                .unwrap_err();
        assert_eq!(
            error.message(),
            "decoded PNG data exceeds configured safety limit"
        );

        let options = Options {
            max_decoded_bytes: 4,
            ..options
        };
        let mut budget = DecodeBudget {
            remaining: options.max_decoded_bytes,
            deadline: SearchDeadline::new(&options),
        };
        optimize_image_streams(&idat, 0, &frames, &[2, 2], false, &options, &mut budget).unwrap();
        assert_eq!(budget.remaining, 0);
    }

    #[test]
    fn coalesces_idat_chunks_and_strips_text_metadata() {
        let zlib = black_scanline_zlib();
        let mut input = SIGNATURE.to_vec();
        input.extend(chunk(*b"IHDR", &ihdr()));
        input.extend(chunk(*b"tEXt", b"Comment\0fixture"));
        input.extend(chunk(*b"IDAT", &zlib[..5]));
        input.extend(chunk(*b"IDAT", &zlib[5..]));
        input.extend(chunk(*b"IEND", &[]));

        let options = Options {
            strip_metadata: true,
            ..Options::default()
        };
        let result = optimize(&input, &options).unwrap();
        let parsed = parse(&result.data, false).unwrap();
        assert_eq!(
            parsed
                .chunks
                .iter()
                .filter(|chunk| chunk.kind == *b"IDAT")
                .count(),
            1
        );
        assert!(!parsed.chunks.iter().any(|chunk| chunk.kind == *b"tEXt"));
        assert!(result.data.len() < input.len());
    }

    #[test]
    fn uncompressed_itxt_is_preserved_without_entering_zlib_optimization() {
        let metadata = b"Comment\0\0\0\0\0plain UTF-8 text";
        let mut input = SIGNATURE.to_vec();
        input.extend(chunk(*b"IHDR", &ihdr()));
        input.extend(chunk(*b"iTXt", metadata));
        input.extend(chunk(*b"IDAT", &black_scanline_zlib()));
        input.extend(chunk(*b"IEND", &[]));

        let result = optimize(&input, &Options::default()).unwrap();
        let parsed = parse(&result.data, false).unwrap();
        let preserved = parsed
            .chunks
            .iter()
            .find(|chunk| chunk.kind == *b"iTXt")
            .expect("uncompressed iTXt should be preserved");
        assert_eq!(preserved.data, metadata);
    }

    #[test]
    fn preserves_png_datastream_with_unknown_unsafe_ancillary_chunk() {
        let zlib = black_scanline_zlib();
        let mut input = SIGNATURE.to_vec();
        input.extend(chunk(*b"IHDR", &ihdr()));
        // Ancillary (lowercase first byte), unknown, and unsafe to copy after
        // critical-data changes (uppercase fourth byte).
        input.extend(chunk(*b"vpAG", b"private contract"));
        input.extend(chunk(*b"IDAT", &zlib[..5]));
        input.extend(chunk(*b"IDAT", &zlib[5..]));
        input.extend(chunk(*b"IEND", &[]));
        let datastream = input.clone();
        let trailing = b"payload after IEND";
        input.extend_from_slice(trailing);

        let result = optimize(&input, &Options::default()).unwrap();
        assert_eq!(result.data, datastream);
        assert_eq!(result.bits_saved, trailing.len() as u64 * 8);
    }

    #[test]
    fn preserves_or_explicitly_strips_rewrite_sensitive_metadata() {
        for (kind, data) in [
            (*b"caBX", b"credential".as_slice()),
            (*b"dSIG", b"signature".as_slice()),
            (*b"iDOT", &[0_u8; 28][..]),
        ] {
            let zlib = black_scanline_zlib();
            let mut input = SIGNATURE.to_vec();
            input.extend(chunk(*b"IHDR", &ihdr()));
            input.extend(chunk(kind, data));
            input.extend(chunk(*b"IDAT", &zlib[..5]));
            input.extend(chunk(*b"IDAT", &zlib[5..]));
            input.extend(chunk(*b"IEND", &[]));

            let preserved = optimize(&input, &Options::default()).unwrap();
            assert_eq!(preserved.data, input, "{}", String::from_utf8_lossy(&kind));

            let stripped = optimize(
                &input,
                &Options {
                    strip_metadata: true,
                    ..Options::default()
                },
            )
            .unwrap();
            let parsed = parse(&stripped.data, false).unwrap();
            assert!(parsed.chunks.iter().all(|chunk| chunk.kind != kind));
            assert_eq!(
                parsed
                    .chunks
                    .iter()
                    .filter(|chunk| chunk.kind == *b"IDAT")
                    .count(),
                1
            );
        }
    }

    #[test]
    fn vestigial_rgba_trns_requires_strip_with_rewrite_sensitive_metadata() {
        let mut header = ihdr();
        header[9] = 6;
        let mut input = SIGNATURE.to_vec();
        input.extend(chunk(*b"IHDR", &header));
        input.extend(chunk(*b"vpAG", b"private contract"));
        input.extend(chunk(*b"PLTE", &[0, 0, 0]));
        input.extend(chunk(*b"tRNS", &[0]));
        input.extend(chunk(*b"IDAT", &stored_zlib(&[0, 0, 0, 0, 0])));
        input.extend(chunk(*b"IEND", &[]));

        let error = optimize(&input, &Options::default()).unwrap_err();
        assert_eq!(
            error.message(),
            "cannot remove invalid PNG tRNS while preserving rewrite-sensitive metadata"
        );

        let result = optimize(
            &input,
            &Options {
                strip_metadata: true,
                ..Options::default()
            },
        )
        .unwrap();
        let parsed = parse(&result.data, false).unwrap();
        assert!(parsed
            .chunks
            .iter()
            .all(|chunk| !matches!(&chunk.kind, b"tRNS" | b"vpAG")));
    }

    #[test]
    fn invalid_cabx_is_removed_only_in_strip_mode() {
        for layout in 0..3 {
            let zlib = black_scanline_zlib();
            let mut input = SIGNATURE.to_vec();
            input.extend(chunk(*b"IHDR", &ihdr()));
            if layout == 0 {
                input.extend(chunk(*b"caBX", b"first"));
                input.extend(chunk(*b"caBX", b"second"));
            }
            if layout == 2 {
                input.extend(chunk(*b"IDAT", &zlib[..5]));
                input.extend(chunk(*b"caBX", b"interrupted IDAT"));
                input.extend(chunk(*b"IDAT", &zlib[5..]));
            } else {
                input.extend(chunk(*b"IDAT", &zlib));
            }
            if layout == 1 {
                input.extend(chunk(*b"caBX", b"misordered"));
            }
            input.extend(chunk(*b"IEND", &[]));

            let error = optimize(&input, &Options::default()).unwrap_err();
            assert_eq!(error.message(), "invalid PNG caBX");
            assert!(deflate_stream_count(&input, false).is_err());

            let options = Options {
                strip_metadata: true,
                ..Options::default()
            };
            assert_eq!(deflate_stream_count(&input, true).unwrap(), 1);
            let stripped = optimize(&input, &options).unwrap();
            let parsed = parse(&stripped.data, false).unwrap();
            assert!(!parsed.chunks.iter().any(|chunk| chunk.kind == *b"caBX"));
        }
    }

    #[test]
    fn malformed_supported_metadata_is_removed_only_in_strip_mode() {
        let mut input = SIGNATURE.to_vec();
        input.extend(chunk(*b"IHDR", &ihdr()));
        input.extend(chunk(*b"sCAL", b"\x01width\0"));
        input.extend(chunk(*b"IDAT", &black_scanline_zlib()));
        input.extend(chunk(*b"IEND", &[]));

        let error = optimize(&input, &Options::default()).unwrap_err();
        assert_eq!(error.message(), "invalid PNG sCAL");

        let stripped = optimize(
            &input,
            &Options {
                strip_metadata: true,
                ..Options::default()
            },
        )
        .unwrap();
        let parsed = parse(&stripped.data, false).unwrap();
        assert!(!parsed.chunks.iter().any(|chunk| chunk.kind == *b"sCAL"));
    }

    #[test]
    fn rebuilds_apng_frame_streams_and_sequence_numbers() {
        let zlib = black_scanline_zlib();
        let mut input = SIGNATURE.to_vec();
        input.extend(chunk(*b"IHDR", &ihdr()));
        let mut actl = Vec::new();
        actl.extend_from_slice(&2_u32.to_be_bytes());
        actl.extend_from_slice(&0_u32.to_be_bytes());
        input.extend(chunk(*b"acTL", &actl));

        input.extend(chunk(*b"fcTL", &frame_control(0)));
        input.extend(chunk(*b"IDAT", &zlib));
        input.extend(chunk(*b"fcTL", &frame_control(1)));
        let mut frame_data = 2_u32.to_be_bytes().to_vec();
        frame_data.extend_from_slice(&zlib);
        input.extend(chunk(*b"fdAT", &frame_data));
        input.extend(chunk(*b"IEND", &[]));

        let result = optimize(&input, &Options::default()).unwrap();
        assert!(result.data.len() <= input.len());
        let parsed = parse(&result.data, false).unwrap();
        assert_eq!(parsed.fdat_frames.len(), 1);
        assert_eq!(parsed.fdat_decoded_sizes, [2]);
        assert_maximum_flevel(&parsed.idat);
        assert_maximum_flevel(&parsed.fdat_frames[0]);
    }

    #[test]
    fn sequence_number_only_fdat_does_not_satisfy_frame_data() {
        let mut input = SIGNATURE.to_vec();
        input.extend(chunk(*b"IHDR", &ihdr()));
        let mut actl = Vec::new();
        actl.extend_from_slice(&2_u32.to_be_bytes());
        actl.extend_from_slice(&0_u32.to_be_bytes());
        input.extend(chunk(*b"acTL", &actl));
        input.extend(chunk(*b"fcTL", &frame_control(0)));
        input.extend(chunk(*b"IDAT", &black_scanline_zlib()));
        input.extend(chunk(*b"fcTL", &frame_control(1)));
        input.extend(chunk(*b"fdAT", &2_u32.to_be_bytes()));
        input.extend(chunk(*b"IEND", &[]));

        let error = match parse(&input, false) {
            Err(error) => error,
            Ok(_) => panic!("sequence-number-only fdAT should be rejected"),
        };
        assert_eq!(error.message(), "invalid APNG frame count");
    }

    #[test]
    fn relaxed_compressed_metadata_retains_flevel_only_improvement() {
        let empty_zlib = [0x78, 0x01, 0x03, 0x00, 0x00, 0x00, 0x00, 0x01];
        let mut metadata = b"Comment\0\0".to_vec();
        metadata.extend_from_slice(&empty_zlib);
        let mut input = SIGNATURE.to_vec();
        input.extend(chunk(*b"IHDR", &ihdr()));
        input.extend(chunk(*b"zTXt", &metadata));
        input.extend(chunk(*b"IDAT", &black_scanline_zlib()));
        input.extend(chunk(*b"IEND", &[]));

        let result = optimize(
            &input,
            &Options {
                strict: false,
                ..Options::default()
            },
        )
        .unwrap();
        let parsed = parse(&result.data, false).unwrap();
        let metadata = parsed
            .chunks
            .iter()
            .find(|chunk| chunk.kind == *b"zTXt")
            .expect("compressed metadata should be retained");
        let offset = compressed_zlib_offset(metadata.kind, metadata.data).unwrap();
        assert_maximum_flevel(&metadata.data[offset..]);
    }

    #[test]
    fn metadata_probe_counts_decoded_bytes_only_once() {
        // This valid zTXt stream expands to one byte and is already too small
        // for the quick pass to shrink. Together with the two-byte image row,
        // it exactly fills the deliberately tiny decoded-data budget.
        let metadata_zlib = [0x78, 0x9c, 0xab, 0x00, 0x00, 0x00, 0x79, 0x00, 0x79];
        let mut metadata = b"Comment\0\0".to_vec();
        metadata.extend_from_slice(&metadata_zlib);

        let mut input = SIGNATURE.to_vec();
        input.extend(chunk(*b"IHDR", &ihdr()));
        input.extend(chunk(*b"zTXt", &metadata));
        input.extend(chunk(*b"IDAT", &black_scanline_zlib()));
        input.extend(chunk(*b"IEND", &[]));

        let options = Options {
            max_decoded_bytes: 3,
            ..Options::default()
        };
        let result = optimize(&input, &options).unwrap();
        assert!(!result.timed_out);
        parse(&result.data, false).unwrap();
    }

    #[test]
    fn metadata_probe_policy_is_identical_in_detailed_modes() {
        let metadata_zlib = [0x78, 0x9c, 0xab, 0x00, 0x00, 0x00, 0x79, 0x00, 0x79];
        let mut metadata = b"Comment\0\0".to_vec();
        metadata.extend_from_slice(&metadata_zlib);

        let mut input = SIGNATURE.to_vec();
        input.extend(chunk(*b"IHDR", &ihdr()));
        input.extend(chunk(*b"zTXt", &metadata));
        input.extend(chunk(*b"IDAT", &black_scanline_zlib()));
        input.extend(chunk(*b"IEND", &[]));

        let quiet_options = Options {
            max_decoded_bytes: 3,
            ..Options::default()
        };
        let quiet = optimize(&input, &quiet_options).unwrap();

        let mut verbose_options = quiet_options.clone();
        verbose_options.verbose = true;
        let verbose = optimize(&input, &verbose_options).unwrap();

        let mut visual_options = quiet_options;
        visual_options.visual = true;
        let visual = optimize(&input, &visual_options).unwrap();

        assert_eq!(verbose.data, quiet.data);
        assert_eq!(verbose.bits_saved, quiet.bits_saved);
        assert_eq!(visual.data, quiet.data);
        assert_eq!(visual.bits_saved, quiet.bits_saved);
    }

    #[test]
    fn reporting_modes_do_not_change_the_mandatory_metadata_reserve() {
        for (exhaustive, metadata_bytes, expected) in
            [(false, 0, false), (false, 1, true), (true, 1, false)]
        {
            let quiet = Options {
                exhaustive,
                ..Options::default()
            };
            let mut verbose = quiet.clone();
            verbose.verbose = true;
            let mut visual = quiet.clone();
            visual.visual = true;

            assert_eq!(
                image_work_needs_metadata_reserve(&quiet, metadata_bytes),
                expected
            );
            assert_eq!(
                image_work_needs_metadata_reserve(&verbose, metadata_bytes),
                expected
            );
            assert_eq!(
                image_work_needs_metadata_reserve(&visual, metadata_bytes),
                expected
            );
        }
    }

    #[test]
    fn parallel_max_metadata_floor_preserves_the_combined_decode_budget() {
        let metadata_zlib = [0x78, 0x9c, 0xab, 0x00, 0x00, 0x00, 0x79, 0x00, 0x79];
        let mut metadata = b"Comment\0\0".to_vec();
        metadata.extend_from_slice(&metadata_zlib);

        let mut input = SIGNATURE.to_vec();
        input.extend(chunk(*b"IHDR", &ihdr()));
        input.extend(chunk(*b"zTXt", &metadata));
        input.extend(chunk(*b"IDAT", &black_scanline_zlib()));
        input.extend(chunk(*b"IEND", &[]));

        let options = Options {
            exhaustive: true,
            timeout: Duration::from_millis(20),
            max_decoded_bytes: 3,
            ..Options::default()
        };
        let result = optimize(&input, &options).unwrap();
        parse(&result.data, false).unwrap();

        let error = optimize(
            &input,
            &Options {
                max_decoded_bytes: 2,
                ..options
            },
        )
        .unwrap_err();
        assert_eq!(
            error.message(),
            "decoded PNG data exceeds configured safety limit"
        );
    }

    #[test]
    fn bounded_max_precomputes_complete_metadata_floors() {
        let mut metadata = b"Comment\0\0".to_vec();
        metadata.extend_from_slice(&super::super::same_byte_bit_win_zlib());
        let mut input = SIGNATURE.to_vec();
        input.extend(chunk(*b"IHDR", &ihdr()));
        input.extend(chunk(*b"zTXt", &metadata));
        input.extend(chunk(*b"IDAT", &black_scanline_zlib()));
        input.extend(chunk(*b"IEND", &[]));

        let options = Options {
            exhaustive: true,
            timeout: Duration::from_secs(1),
            ..Options::default()
        };
        let parsed = parse(&input, false).unwrap();
        let stream_ids = metadata_stream_ids(&parsed).unwrap();
        let mut cached = Vec::new();
        cached.resize_with(parsed.chunks.len(), || None);
        let mut budget = DecodeBudget {
            remaining: options.max_decoded_bytes,
            deadline: SearchDeadline::new(&options),
        };

        precompute_max_metadata_floors(&parsed, &stream_ids, &options, &mut budget, &mut cached)
            .unwrap();

        let metadata_index = parsed
            .chunks
            .iter()
            .position(|chunk| chunk.kind == *b"zTXt")
            .unwrap();
        let floor = cached[metadata_index]
            .as_ref()
            .expect("Max should retain a bounded complete metadata floor");
        assert!(floor.replacement.is_some());
        assert!(floor.output_deflate_bits < floor.source_deflate_bits);
        assert_eq!(floor.decoded_size, Some(168));
        assert_eq!(
            options.max_decoded_bytes - budget.remaining,
            168,
            "the cached floor should charge decoded metadata exactly once"
        );

        let remaining_after_floor = budget.remaining;
        let metadata_chunk = &parsed.chunks[metadata_index];
        let refined = refine_cached_compressed_body(
            metadata_chunk.kind,
            metadata_chunk.data,
            floor,
            &options,
            &mut budget,
        )
        .unwrap();
        assert_eq!(
            budget.remaining, remaining_after_floor,
            "a Max descendant must not charge the same decoded stream twice"
        );
        let floor_len = floor
            .replacement
            .as_ref()
            .map_or(metadata_chunk.data.len(), Vec::len);
        let refined_len = refined
            .replacement
            .as_ref()
            .map_or(metadata_chunk.data.len(), Vec::len);
        assert!(
            (refined_len, refined.output_deflate_bits) <= (floor_len, floor.output_deflate_bits),
            "Max must retain the complete metadata floor"
        );
    }

    #[test]
    fn compressed_metadata_contributes_same_byte_bit_savings_in_both_policies() {
        let mut metadata = b"Comment\0\0".to_vec();
        // This source is one bit behind strict output and three bits behind
        // relaxed output, without changing the byte length.
        metadata.extend_from_slice(&super::super::same_byte_bit_win_zlib());
        let image = zlib::optimize_embedded(
            &black_scanline_zlib(),
            &Options::default(),
            2,
            false,
            DefaultFloor::Complete,
        )
        .unwrap()
        .data;
        let mut input = SIGNATURE.to_vec();
        input.extend(chunk(*b"IHDR", &ihdr()));
        input.extend(chunk(*b"zTXt", &metadata));
        input.extend(chunk(*b"IDAT", &image));
        input.extend(chunk(*b"IEND", &[]));

        for strict in [true, false] {
            let optimized = optimize(
                &input,
                &Options {
                    strict,
                    ..Options::default()
                },
            )
            .unwrap();

            assert_eq!(optimized.data.len(), input.len());
            assert_eq!(optimized.bits_saved, if strict { 1 } else { 3 });
        }
    }

    #[test]
    fn retained_lenient_metadata_still_consumes_decode_budget() {
        let mut lookalike = stored_zlib(b"x");
        lookalike.extend_from_slice(b"trailing");
        let options = Options {
            max_decoded_bytes: 1,
            ..Options::default()
        };
        let mut budget = DecodeBudget {
            remaining: options.max_decoded_bytes,
            deadline: SearchDeadline::new(&options),
        };

        let retained = optimize_png_zlib(
            &lookalike,
            &options,
            true,
            DefaultFloor::Shared,
            &mut budget,
        )
        .unwrap();
        assert_eq!(retained.data, lookalike);
        assert_eq!(budget.remaining, 0);

        let error = optimize_png_zlib(
            &lookalike,
            &options,
            true,
            DefaultFloor::Shared,
            &mut budget,
        )
        .unwrap_err();
        assert_eq!(
            error.message(),
            "decoded PNG data exceeds configured safety limit"
        );
    }
}
