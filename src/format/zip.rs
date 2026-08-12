// SPDX-License-Identifier: MIT

use crate::checksum::crc32_update;
use crate::deflate::{optimize_raw_prefix_with_floor, DefaultFloor};
use crate::{Error, Optimization, Options, Result};

use std::thread;
use std::time::{Duration, Instant};

use super::{
    scale_duration, try_append_bytes, try_copy_bytes, try_vec_with_capacity, SearchDeadline,
};

const LOCAL_FILE_HEADER: u32 = 0x0403_4b50;
const CENTRAL_DIRECTORY_HEADER: u32 = 0x0201_4b50;
const END_OF_CENTRAL_DIRECTORY: u32 = 0x0605_4b50;
const DATA_DESCRIPTOR: u32 = 0x0807_4b50;
const ZIP64_END_OF_CENTRAL_DIRECTORY: u32 = 0x0606_4b50;

const FLAG_ENCRYPTED: u16 = 0x0001;
const FLAG_DATA_DESCRIPTOR: u16 = 0x0008;
const OUTPUT_ALLOCATION_ERROR: &str = "could not allocate ZIP output";
const MODEL_ALLOCATION_ERROR: &str = "could not allocate ZIP entry model";
const LOCAL_ALLOCATION_ERROR: &str = "could not allocate ZIP local entry";

// Max keeps an exact Default archive as a quality floor while exploring a
// separate direct-Max archive. The direct branch treats its validated source
// stream as an established floor instead of rebuilding ordinary work already
// owned by the Default branch. Bound both retained input and decoded models
// because the two complete branches still use separate archive and Deflate
// arenas.
const PARALLEL_MAX_ARCHIVE_INPUT: usize = 8 * 1_024 * 1_024;
const PARALLEL_MAX_ARCHIVE_DECODED: u64 = 64 * 1_024 * 1_024;
const PARALLEL_MEMBER_ARCHIVE_DECODED: u64 = 64 * 1_024 * 1_024;
const PARALLEL_ARCHIVE_MEMBER_WORKERS: usize = 8;

#[derive(Clone, Debug)]
struct Entry {
    local: Vec<u8>,
    local_size_before: usize,
    central_offset: usize,
    central_size: usize,
    local_offset_before: usize,
    local_offset_after: usize,
    crc32: u32,
    compressed_size_before: u32,
    compressed_size_after: u32,
    uncompressed_size: u32,
    method: u16,
    flags: u16,
    skip: bool,
    strip_extra: bool,
    source_deflate_bits: u64,
    output_deflate_bits: u64,
}

/// Recognize both ordinary archives and ZIP-family inputs that should receive
/// a specific malformed/unsupported ZIP diagnostic.
///
/// A leading signature retains the old behavior for truncated inputs. Looking
/// for the terminal record additionally recognizes valid archives with a
/// self-extracting or other prepended stub.
pub(super) fn has_recognizable_structure(input: &[u8]) -> bool {
    matches!(
        read_le32(input, 0),
        Some(LOCAL_FILE_HEADER)
            | Some(CENTRAL_DIRECTORY_HEADER)
            | Some(END_OF_CENTRAL_DIRECTORY)
            | Some(DATA_DESCRIPTOR)
            | Some(ZIP64_END_OF_CENTRAL_DIRECTORY)
    ) || find_end_of_central_directory(input).is_some()
}

pub(super) fn deflate_stream_count(input: &[u8]) -> Result<usize> {
    let eocd_offset = find_end_of_central_directory(input)
        .ok_or_else(|| Error::new("ZIP end of central directory not found"))?;
    let eocd = input
        .get(eocd_offset..eocd_offset + 22)
        .ok_or_else(|| Error::new("truncated ZIP end of central directory"))?;
    let disk_number = le16(eocd, 4);
    let central_disk = le16(eocd, 6);
    let entries_on_disk = le16(eocd, 8);
    let entry_count = le16(eocd, 10);
    let central_size = le32(eocd, 12);
    let central_offset_u32 = le32(eocd, 16);
    if disk_number != 0 || central_disk != 0 || entries_on_disk != entry_count {
        return Err(Error::new("spanned ZIP archives are not supported"));
    }
    if entry_count == u16::MAX || central_size == u32::MAX || central_offset_u32 == u32::MAX {
        return Err(Error::new("ZIP64 archives are not supported"));
    }
    let central_offset = central_offset_u32 as usize;
    if central_offset
        .checked_add(central_size as usize)
        .filter(|&end| end == eocd_offset)
        .is_none()
    {
        return Err(Error::new("unsupported ZIP layout"));
    }
    let entries = parse_central_entries(input, central_offset, eocd_offset, entry_count, false)?;
    Ok(entries
        .iter()
        .filter(|entry| entry_is_optimizable(entry))
        .count())
}

pub(super) fn optimize(input: &[u8], options: &Options) -> Result<Optimization> {
    if !options.exhaustive {
        return optimize_once(input, options, DefaultFloor::Complete);
    }

    if !options.visual && parallel_max_archive_is_bounded(input) {
        return optimize_max_parallel(input, options);
    }

    optimize_max_sequential(input, options)
}

/// Build Default-seeded and direct-Max archives inside the same wall-clock
/// window, then retain the byte-first winner.
///
/// `Established` makes original-source Max routes available immediately while
/// the caller owns all ordinary Default work. Refining the completed Default
/// archive remains useful rather than duplicate work: its rewritten block and
/// token choices expose a distinct search basin that the source branch cannot
/// necessarily reach.
fn optimize_max_parallel(input: &[u8], options: &Options) -> Result<Optimization> {
    thread::scope(|scope| {
        let started = Instant::now();
        let max_worker = thread::Builder::new()
            .name("columbo-zip-direct-max".into())
            .spawn_scoped(scope, || {
                optimize_once(input, options, DefaultFloor::Established)
            });

        let mut floor_options = options.clone();
        floor_options.exhaustive = false;
        let floor = optimize_once(input, &floor_options, DefaultFloor::Complete);

        let max_worker = match max_worker {
            Ok(worker) => worker,
            // Thread creation failure is not an optimization failure. Finish
            // the established sequential route using the Default result and
            // whatever part of the original allowance remains.
            Err(_) => return refine_complete_floor(input, options, started, floor?),
        };
        let refined_floor =
            floor.and_then(|floor| refine_complete_floor(input, options, started, floor));
        let direct = match max_worker.join() {
            Ok(result) => result?,
            Err(payload) => std::panic::resume_unwind(payload),
        };
        Ok(best_complete_optimization(refined_floor?, direct))
    })
}

fn optimize_max_sequential(input: &[u8], options: &Options) -> Result<Optimization> {
    let started = Instant::now();
    let mut floor_options = options.clone();
    floor_options.exhaustive = false;
    let floor = optimize_once(input, &floor_options, DefaultFloor::Complete)?;
    refine_complete_floor(input, options, started, floor)
}

fn refine_complete_floor(
    input: &[u8],
    options: &Options,
    started: Instant,
    mut floor: Optimization,
) -> Result<Optimization> {
    // Max benchmarks and interactive runs allocate one file-wide allowance,
    // normally measured default time plus an extra search budget. Preserve
    // that contract explicitly: establish the complete default archive once,
    // then use only the actual remainder to refine its already-smaller Deflate
    // members. The second phase starts from the finished floor and therefore
    // uses `Shared` raw floors instead of rebuilding default work per member.
    let remaining = options.timeout.saturating_sub(started.elapsed());
    if remaining.is_zero() {
        floor.timed_out = true;
        return Ok(floor);
    }

    let mut max_options = options.clone();
    max_options.timeout = remaining;
    let mut refined = optimize_once(&floor.data, &max_options, DefaultFloor::Shared)?;
    let refined_wins = refined.data.len() < floor.data.len()
        || (refined.data.len() == floor.data.len() && refined.bits_saved != 0);
    if !refined_wins {
        floor.timed_out |= refined.timed_out;
        return Ok(floor);
    }

    refined.bits_saved = combined_savings(input.len(), &floor, &refined);
    refined.timed_out |= floor.timed_out;
    Ok(refined)
}

fn best_complete_optimization(mut floor: Optimization, mut direct: Optimization) -> Optimization {
    let timed_out = floor.timed_out || direct.timed_out;
    let direct_wins = direct.data.len() < floor.data.len()
        || (direct.data.len() == floor.data.len() && direct.bits_saved > floor.bits_saved);
    if direct_wins {
        direct.timed_out = timed_out;
        direct
    } else {
        floor.timed_out = timed_out;
        floor
    }
}

fn parallel_max_archive_is_bounded(input: &[u8]) -> bool {
    if input.len() > PARALLEL_MAX_ARCHIVE_INPUT {
        return false;
    }
    let Some(eocd_offset) = find_end_of_central_directory(input) else {
        return false;
    };
    let Some(eocd) = input.get(eocd_offset..eocd_offset.saturating_add(22)) else {
        return false;
    };
    let central_offset = le32(eocd, 16) as usize;
    let central_size = le32(eocd, 12) as usize;
    if central_offset
        .checked_add(central_size)
        .filter(|&end| end == eocd_offset)
        .is_none()
    {
        return false;
    }
    let Ok(entries) =
        parse_central_entries(input, central_offset, eocd_offset, le16(eocd, 10), false)
    else {
        return false;
    };
    parallel_max_entry_work_is_bounded(&entries)
}

fn parallel_max_entry_work_is_bounded(entries: &[Entry]) -> bool {
    let mut total_decoded = 0_u64;
    let mut has_optimizable_entry = false;
    for entry in entries.iter().filter(|entry| entry_is_optimizable(entry)) {
        has_optimizable_entry = true;
        total_decoded = match total_decoded.checked_add(u64::from(entry.uncompressed_size)) {
            Some(total) if total <= PARALLEL_MAX_ARCHIVE_DECODED => total,
            Some(_) | None => return false,
        };
    }

    has_optimizable_entry && !uniformly_distributed_member_work(entries)
}

fn uniformly_distributed_member_work(entries: &[Entry]) -> bool {
    let mut member_count = 0_usize;
    let mut total_compressed = 0_u64;
    let mut largest_compressed = 0_u64;
    for entry in entries.iter().filter(|entry| entry_is_optimizable(entry)) {
        member_count += 1;
        let compressed = u64::from(entry.compressed_size_before);
        let Some(total) = total_compressed.checked_add(compressed) else {
            return false;
        };
        total_compressed = total;
        largest_compressed = largest_compressed.max(compressed);
    }

    // Two complete archive branches duplicate parser and floor setup for every
    // member. That overlap pays when a few substantial members own most work,
    // because their independent basins can run together. On a broad, uniform
    // set no member owns even one eighth of the compressed payload; completing
    // Default once and refining it gives more members useful search time.
    // This is a structural work-distribution rule, not a filename or elapsed-
    // time gate, and sufficient Max time still evaluates the same lineages.
    member_count >= 8 && largest_compressed.saturating_mul(8) <= total_compressed
}

fn combined_savings(original_bytes: usize, floor: &Optimization, refined: &Optimization) -> u64 {
    match original_bytes.cmp(&refined.data.len()) {
        std::cmp::Ordering::Greater => u64::try_from(original_bytes - refined.data.len())
            .map_or(u64::MAX, |bytes| bytes.saturating_mul(8)),
        // When both phases retain the original byte length, each public
        // metric is a meaningful Deflate-bit improvement over its own input.
        std::cmp::Ordering::Equal if floor.data.len() == original_bytes => {
            floor.bits_saved.saturating_add(refined.bits_saved)
        }
        std::cmp::Ordering::Equal | std::cmp::Ordering::Less => 0,
    }
}

fn optimize_once(
    input: &[u8],
    options: &Options,
    raw_default_floor: DefaultFloor,
) -> Result<Optimization> {
    let deadline = SearchDeadline::new(options);
    let eocd_offset = find_end_of_central_directory(input)
        .ok_or_else(|| Error::new("ZIP end of central directory not found"))?;
    let eocd = input
        .get(eocd_offset..eocd_offset + 22)
        .ok_or_else(|| Error::new("truncated ZIP end of central directory"))?;

    let disk_number = le16(eocd, 4);
    let central_disk = le16(eocd, 6);
    let entries_on_disk = le16(eocd, 8);
    let entry_count = le16(eocd, 10);
    let central_size = le32(eocd, 12);
    let central_offset_u32 = le32(eocd, 16);

    if disk_number != 0 || central_disk != 0 || entries_on_disk != entry_count {
        return Err(Error::new("spanned ZIP archives are not supported"));
    }
    if entry_count == u16::MAX || central_size == u32::MAX || central_offset_u32 == u32::MAX {
        return Err(Error::new("ZIP64 archives are not supported"));
    }

    let central_offset = central_offset_u32 as usize;
    if central_offset
        .checked_add(central_size as usize)
        .filter(|&end| end == eocd_offset)
        .is_none()
    {
        return Err(Error::new("unsupported ZIP layout"));
    }

    let mut entries = parse_central_entries(
        input,
        central_offset,
        eocd_offset,
        entry_count,
        options.strip_metadata,
    )?;
    let mut next_stream_id = 1_usize;
    let stream_ids: Vec<Option<usize>> = entries
        .iter()
        .map(|entry| {
            if entry_is_optimizable(entry) {
                let id = next_stream_id;
                next_stream_id = next_stream_id.saturating_add(1);
                Some(id)
            } else {
                None
            }
        })
        .collect();
    // Limit the sum before decoding any member. This avoids doing expensive
    // work on an archive whose declared expansion is already unsafe.
    let mut expanded_size = 0_u64;
    for entry in &entries {
        if matches!(entry.method, 0 | 8) && entry.flags & FLAG_ENCRYPTED == 0 {
            expanded_size = expanded_size
                .checked_add(u64::from(entry.uncompressed_size))
                .filter(|&size| size <= options.max_decoded_bytes)
                .ok_or_else(|| Error::new("ZIP expanded data exceeds configured safety limit"))?;
        }
    }
    // Resolve and validate every physical local range before copying or
    // optimizing any payload. A hostile central directory may otherwise point
    // many entries at the same large local record, multiplying work and owned
    // buffers before the reconstruction loop finally notices the overlap.
    let physical_order = preflight_local_entries(input, central_offset, &mut entries)?;

    // Search order is independent of archive layout. In normal mode the
    // original Columbo C implementation gives the largest Deflate member first
    // use of the shared deadline; max mode runs small members first and leaves
    // the actual remainder for the final largest member. Local records are
    // still emitted later in physical-offset order, and central records retain
    // source order.
    let build_order = optimization_order(&entries, options.exhaustive)?;
    let schedule = options.exhaustive.then(|| ZipSchedule::new(&entries));
    let parallel_uniform_members = !options.visual
        && input.len() <= PARALLEL_MAX_ARCHIVE_INPUT
        && expanded_size <= PARALLEL_MEMBER_ARCHIVE_DECODED
        && uniformly_distributed_member_work(&entries);
    let timed_out_entries = if parallel_uniform_members {
        build_uniform_entries_parallel(
            input,
            central_offset,
            &mut entries,
            &stream_ids,
            &build_order,
            options,
            raw_default_floor,
            &deadline,
            schedule,
        )?
    } else {
        build_entries_serial(
            input,
            central_offset,
            &mut entries,
            &stream_ids,
            &build_order,
            options,
            raw_default_floor,
            &deadline,
            schedule,
        )?
    };
    reclaim_timed_out_entries(
        input,
        central_offset,
        &mut entries,
        &stream_ids,
        timed_out_entries,
        options,
        raw_default_floor,
        &deadline,
        schedule,
    )?;
    let timed_out = deadline.is_expired();
    let source_deflate_bits = entries.iter().try_fold(0_u64, |total, entry| {
        total.checked_add(entry.source_deflate_bits)
    });
    let source_deflate_bits =
        source_deflate_bits.ok_or_else(|| Error::new("ZIP Deflate bit count is too large"))?;
    let output_deflate_bits = entries.iter().try_fold(0_u64, |total, entry| {
        total.checked_add(entry.output_deflate_bits)
    });
    let mut output_deflate_bits =
        output_deflate_bits.ok_or_else(|| Error::new("ZIP Deflate bit count is too large"))?;

    // Local records need not appear in central-directory order. Rewrite them
    // by physical offset so self-extracting prefixes and inter-record padding
    // remain byte-for-byte intact.
    let mut output = try_vec_with_capacity(input.len(), OUTPUT_ALLOCATION_ERROR)?;
    let mut cursor = 0_usize;
    for index in physical_order {
        let entry = &mut entries[index];
        if entry.local_offset_before < cursor {
            return Err(Error::new("overlapping ZIP local entries"));
        }
        try_append_bytes(
            &mut output,
            &input[cursor..entry.local_offset_before],
            OUTPUT_ALLOCATION_ERROR,
        )?;
        entry.local_offset_after = output.len();
        if !entry.skip {
            try_append_bytes(&mut output, &entry.local, OUTPUT_ALLOCATION_ERROR)?;
        }
        cursor = entry
            .local_offset_before
            .checked_add(entry.local_size_before)
            .ok_or_else(|| Error::new("ZIP local entry too large"))?;
    }
    if cursor > central_offset {
        return Err(Error::new(
            "ZIP local entries extend into central directory",
        ));
    }
    try_append_bytes(
        &mut output,
        &input[cursor..central_offset],
        OUTPUT_ALLOCATION_ERROR,
    )?;

    let new_central_offset = output.len();
    let mut written_entries = 0_u16;
    for entry in &entries {
        if entry.skip {
            continue;
        }
        let local_offset = u32::try_from(entry.local_offset_after)
            .map_err(|_| Error::new("ZIP central directory points to unknown entry"))?;
        let source = &input[entry.central_offset..entry.central_offset + entry.central_size];
        let mut record = try_copy_bytes(source, OUTPUT_ALLOCATION_ERROR)?;
        put_le16(&mut record, 8, output_flags(entry));
        put_le32(&mut record, 20, entry.compressed_size_after);
        put_le32(&mut record, 42, local_offset);

        let record_size = if options.strip_metadata {
            let name_length = le16(&record, 28) as usize;
            let extra_length = le16(&record, 30) as usize;
            if entry.strip_extra {
                put_le16(&mut record, 30, 0);
            }
            // Central per-file comments are metadata even when an unsupported
            // entry needs to retain its extra field.
            put_le16(&mut record, 32, 0);
            46 + name_length + if entry.strip_extra { 0 } else { extra_length }
        } else {
            record.len()
        };
        try_append_bytes(&mut output, &record[..record_size], OUTPUT_ALLOCATION_ERROR)?;
        written_entries = written_entries
            .checked_add(1)
            .ok_or_else(|| Error::new("optimized ZIP requires ZIP64"))?;
    }

    let new_central_size = output.len() - new_central_offset;
    let new_central_offset = u32::try_from(new_central_offset)
        .map_err(|_| Error::new("optimized ZIP requires ZIP64"))?;
    let new_central_size =
        u32::try_from(new_central_size).map_err(|_| Error::new("optimized ZIP requires ZIP64"))?;

    let comment_length = le16(eocd, 20) as usize;
    let eocd_length = if options.strip_metadata {
        22
    } else {
        22 + comment_length
    };
    let mut new_eocd = try_copy_bytes(
        &input[eocd_offset..eocd_offset + eocd_length],
        OUTPUT_ALLOCATION_ERROR,
    )?;
    put_le16(&mut new_eocd, 8, written_entries);
    put_le16(&mut new_eocd, 10, written_entries);
    put_le32(&mut new_eocd, 12, new_central_size);
    put_le32(&mut new_eocd, 16, new_central_offset);
    if options.strip_metadata {
        put_le16(&mut new_eocd, 20, 0);
    }
    try_append_bytes(&mut output, &new_eocd, OUTPUT_ALLOCATION_ERROR)?;

    if output.len() > input.len() && !options.strict {
        output.clear();
        try_append_bytes(&mut output, input, OUTPUT_ALLOCATION_ERROR)?;
        output_deflate_bits = source_deflate_bits;
    }

    Ok(Optimization::from_metrics(
        input.len(),
        output,
        source_deflate_bits,
        output_deflate_bits,
        timed_out,
    ))
}

#[derive(Clone, Copy)]
struct LocalEntryLayout {
    prefix_size: usize,
    data_offset: usize,
    data_end: usize,
    total_size: usize,
}

/// Validate local headers and physical ranges before expensive member work.
fn preflight_local_entries(
    input: &[u8],
    central_offset: usize,
    entries: &mut [Entry],
) -> Result<Vec<usize>> {
    for entry in entries.iter_mut() {
        entry.local_size_before = local_entry_layout(input, central_offset, entry)?.total_size;
    }

    let mut physical_order = try_vec_with_capacity(entries.len(), MODEL_ALLOCATION_ERROR)?;
    physical_order.extend(0..entries.len());
    physical_order.sort_unstable_by_key(|&index| entries[index].local_offset_before);

    let mut cursor = 0_usize;
    for &index in &physical_order {
        let entry = &entries[index];
        if entry.local_offset_before < cursor {
            return Err(Error::new("overlapping ZIP local entries"));
        }
        cursor = entry
            .local_offset_before
            .checked_add(entry.local_size_before)
            .filter(|&end| end <= central_offset)
            .ok_or_else(|| Error::new("ZIP local entries extend into central directory"))?;
    }
    Ok(physical_order)
}

#[allow(clippy::too_many_arguments)]
fn build_entries_serial(
    input: &[u8],
    central_offset: usize,
    entries: &mut [Entry],
    stream_ids: &[Option<usize>],
    build_order: &[usize],
    options: &Options,
    raw_default_floor: DefaultFloor,
    deadline: &SearchDeadline,
    schedule: Option<ZipSchedule>,
) -> Result<Vec<usize>> {
    let mut timed_out = Vec::new();
    timed_out
        .try_reserve_exact(build_order.len())
        .map_err(|_| Error::new("could not allocate ZIP member schedule"))?;
    for &index in build_order {
        if build_scheduled_entry(
            input,
            central_offset,
            &mut entries[index],
            stream_ids[index],
            options,
            raw_default_floor,
            deadline,
            schedule,
        )? {
            timed_out.push(index);
        }
    }
    Ok(timed_out)
}

/// Optimize a broad set of similarly sized independent members concurrently.
///
/// This path is selected only after the archive-wide input and decoded-memory
/// bounds have passed. Each worker owns cloned entry metadata and its output;
/// immutable archive bytes are shared, and results rejoin before reconstruction.
#[allow(clippy::too_many_arguments)]
fn build_uniform_entries_parallel(
    input: &[u8],
    central_offset: usize,
    entries: &mut [Entry],
    stream_ids: &[Option<usize>],
    build_order: &[usize],
    options: &Options,
    raw_default_floor: DefaultFloor,
    deadline: &SearchDeadline,
    schedule: Option<ZipSchedule>,
) -> Result<Vec<usize>> {
    let optimizable_count = build_order
        .iter()
        .take_while(|&&index| entry_is_optimizable(&entries[index]))
        .count();
    let (optimizable, remaining) = build_order.split_at(optimizable_count);
    // The structural caller currently guarantees at least eight jobs. Keep a
    // serial fallback here as well so a future scheduling change cannot turn
    // an empty worker set into a division by zero.
    if optimizable.is_empty() {
        return build_entries_serial(
            input,
            central_offset,
            entries,
            stream_ids,
            build_order,
            options,
            raw_default_floor,
            deadline,
            schedule,
        );
    }
    let worker_count = optimizable.len().min(PARALLEL_ARCHIVE_MEMBER_WORKERS);
    let jobs_per_worker = optimizable.len() / worker_count;
    let workers_with_extra_job = optimizable.len() % worker_count;

    let completed = thread::scope(|scope| -> Result<_> {
        let mut workers = Vec::new();
        workers
            .try_reserve_exact(worker_count)
            .map_err(|_| Error::new("could not allocate ZIP member workers"))?;
        let mut completed = Vec::new();
        completed
            .try_reserve_exact(optimizable.len())
            .map_err(|_| Error::new("could not allocate ZIP member results"))?;

        let mut start = 0_usize;
        for worker_index in 0..worker_count {
            let count = jobs_per_worker + usize::from(worker_index < workers_with_extra_job);
            let jobs = &optimizable[start..start + count];
            start += count;
            let worker = thread::Builder::new()
                .name(format!("columbo-zip-members-{worker_index}"))
                .spawn_scoped(scope, || {
                    build_entry_batch(
                        input,
                        central_offset,
                        entries,
                        stream_ids,
                        jobs,
                        options,
                        raw_default_floor,
                        deadline,
                        schedule,
                    )
                });
            match worker {
                Ok(worker) => workers.push(worker),
                Err(_) => completed.extend(build_entry_batch(
                    input,
                    central_offset,
                    entries,
                    stream_ids,
                    jobs,
                    options,
                    raw_default_floor,
                    deadline,
                    schedule,
                )?),
            }
        }

        for worker in workers {
            let results = match worker.join() {
                Ok(results) => results?,
                Err(payload) => std::panic::resume_unwind(payload),
            };
            completed.extend(results);
        }
        Ok(completed)
    })?;

    let mut timed_out = Vec::new();
    timed_out
        .try_reserve_exact(build_order.len())
        .map_err(|_| Error::new("could not allocate ZIP member schedule"))?;
    for (index, entry, entry_timed_out) in completed {
        entries[index] = entry;
        if entry_timed_out {
            timed_out.push(index);
        }
    }
    for &index in remaining {
        if build_scheduled_entry(
            input,
            central_offset,
            &mut entries[index],
            stream_ids[index],
            options,
            raw_default_floor,
            deadline,
            schedule,
        )? {
            timed_out.push(index);
        }
    }
    Ok(timed_out)
}

#[allow(clippy::too_many_arguments)]
fn build_entry_batch(
    input: &[u8],
    central_offset: usize,
    entries: &[Entry],
    stream_ids: &[Option<usize>],
    jobs: &[usize],
    options: &Options,
    raw_default_floor: DefaultFloor,
    deadline: &SearchDeadline,
    schedule: Option<ZipSchedule>,
) -> Result<Vec<(usize, Entry, bool)>> {
    // These jobs run serially on one worker while sibling slices overlap in
    // wall time. Divide the worker's allowance among only this slice; using
    // the whole archive denominator would underfund every parallel member.
    let schedule = schedule.map(|_| ZipSchedule::new_for_indices(entries, jobs));
    let mut completed = Vec::new();
    completed
        .try_reserve_exact(jobs.len())
        .map_err(|_| Error::new("could not allocate ZIP member results"))?;
    for &index in jobs {
        let mut entry = entries[index].clone();
        let timed_out = build_scheduled_entry(
            input,
            central_offset,
            &mut entry,
            stream_ids[index],
            options,
            raw_default_floor,
            deadline,
            schedule,
        )?;
        completed.push((index, entry, timed_out));
    }
    Ok(completed)
}

#[allow(clippy::too_many_arguments)]
fn build_scheduled_entry(
    input: &[u8],
    central_offset: usize,
    entry: &mut Entry,
    stream_id: Option<usize>,
    options: &Options,
    raw_default_floor: DefaultFloor,
    deadline: &SearchDeadline,
    schedule: Option<ZipSchedule>,
) -> Result<bool> {
    let file_remaining = deadline.remaining();
    let mut call_options = options.clone();
    call_options.timeout = call_options.timeout.min(file_remaining);
    if let Some(schedule) = schedule {
        call_options.timeout = schedule.timeout_for(options.timeout, file_remaining, entry);
    }
    let mut build_entry = || {
        build_local_entry(
            input,
            central_offset,
            entry,
            &call_options,
            raw_default_floor,
        )
    };
    if let Some(stream_id) = stream_id {
        if call_options.timeout < file_remaining {
            crate::progress::with_stream_slice(stream_id, &[], None, build_entry)
        } else {
            crate::progress::with_stream_group(stream_id, &[], build_entry)
        }
    } else {
        build_entry()
    }
}

#[allow(clippy::too_many_arguments)]
fn reclaim_timed_out_entries(
    input: &[u8],
    central_offset: usize,
    entries: &mut [Entry],
    stream_ids: &[Option<usize>],
    mut pending: Vec<usize>,
    options: &Options,
    raw_default_floor: DefaultFloor,
    deadline: &SearchDeadline,
    schedule: Option<ZipSchedule>,
) -> Result<()> {
    pending.retain(|&index| entry_is_optimizable(&entries[index]));
    while !pending.is_empty() && !deadline.is_expired() {
        let use_effective_weights = schedule.is_some_and(|schedule| schedule.use_effective_weights);
        let mut remaining_weight = pending.iter().fold(0.0_f64, |total, &index| {
            total + zip_stream_weight(&entries[index], use_effective_weights)
        });
        if remaining_weight <= 0.0 {
            break;
        }
        let mut still_pending = Vec::new();
        still_pending
            .try_reserve_exact(pending.len())
            .map_err(|_| Error::new("could not allocate ZIP member schedule"))?;

        for (position, &index) in pending.iter().enumerate() {
            let file_remaining = deadline.remaining();
            if file_remaining.is_zero() {
                break;
            }
            let weight = zip_stream_weight(&entries[index], use_effective_weights);
            let last = position + 1 == pending.len();
            let timeout = if last {
                file_remaining
            } else {
                scale_duration(file_remaining, weight / remaining_weight * 0.95)
            };
            remaining_weight = (remaining_weight - weight).max(0.0);
            if timeout.is_zero() {
                continue;
            }

            let mut candidate = entries[index].clone();
            candidate.local.clear();
            candidate.compressed_size_after = candidate.compressed_size_before;
            candidate.output_deflate_bits = candidate.source_deflate_bits;
            let mut retry_options = options.clone();
            retry_options.timeout = timeout;
            let mut retry = || {
                build_local_entry(
                    input,
                    central_offset,
                    &mut candidate,
                    &retry_options,
                    raw_default_floor,
                )
            };
            let retry_timed_out = if let Some(stream_id) = stream_ids[index] {
                crate::progress::with_stream_reclaim(stream_id, &[], !last, retry)?
            } else {
                retry()?
            };
            if candidate.compressed_size_after < entries[index].compressed_size_after
                || (candidate.compressed_size_after == entries[index].compressed_size_after
                    && candidate.output_deflate_bits < entries[index].output_deflate_bits)
            {
                entries[index] = candidate;
            }
            if retry_timed_out && !deadline.is_expired() {
                still_pending.push(index);
            }
        }
        pending = still_pending;
    }
    Ok(())
}

fn optimization_order(entries: &[Entry], exhaustive: bool) -> Result<Vec<usize>> {
    let mut order = Vec::new();
    order
        .try_reserve_exact(entries.len())
        .map_err(|_| Error::new("could not allocate ZIP optimization order"))?;
    order.extend(0..entries.len());
    order.sort_unstable_by(|&left_index, &right_index| {
        let left = &entries[left_index];
        let right = &entries[right_index];
        let left_optimizable = entry_is_optimizable(left);
        let right_optimizable = entry_is_optimizable(right);

        right_optimizable
            .cmp(&left_optimizable)
            .then_with(|| {
                if !left_optimizable || left.compressed_size_before == right.compressed_size_before
                {
                    std::cmp::Ordering::Equal
                } else if exhaustive {
                    left.compressed_size_before
                        .cmp(&right.compressed_size_before)
                } else {
                    right
                        .compressed_size_before
                        .cmp(&left.compressed_size_before)
                }
            })
            .then_with(|| left.local_offset_before.cmp(&right.local_offset_before))
            .then_with(|| left_index.cmp(&right_index))
    });
    Ok(order)
}

fn entry_is_optimizable(entry: &Entry) -> bool {
    entry.method == 8 && entry.flags & FLAG_ENCRYPTED == 0 && entry.compressed_size_before != 0
}

/// Divide max mode's file-wide deadline between independent ZIP members.
///
/// Small members run first and receive a proportional slice. The final largest
/// member inherits the actual initial remainder, then every member that yielded
/// its slice is eligible for a weighted reclaim pass using actual time left.
/// Thus unused time is not lost even when several members tie for largest.
/// Nearly incompressible
/// archives use their small amount of compression slack as the weight;
/// otherwise huge stored-like members would crowd useful work on small,
/// compressible entries out of the schedule. The proportional denominator
/// counts every member once. Counting the reserved largest member twice can
/// truncate another member's ordinary comparison floor, making max worse than
/// normal even though the largest route later has unused search time.
#[derive(Clone, Copy)]
struct ZipSchedule {
    largest_size: u32,
    reserved_largest_offset: usize,
    total_weight: f64,
    use_effective_weights: bool,
}

impl ZipSchedule {
    fn new(entries: &[Entry]) -> Self {
        Self::from_entries(entries.iter())
    }

    fn new_for_indices(entries: &[Entry], indices: &[usize]) -> Self {
        Self::from_entries(indices.iter().map(|&index| &entries[index]))
    }

    fn from_entries<'a, I>(entries: I) -> Self
    where
        I: Iterator<Item = &'a Entry> + Clone,
    {
        let largest_size = entries
            .clone()
            .filter(|entry| entry_is_optimizable(entry))
            .map(|entry| entry.compressed_size_before)
            .max()
            .unwrap_or(0);
        let reserved_largest_offset = entries
            .clone()
            .filter(|entry| {
                entry_is_optimizable(entry) && entry.compressed_size_before == largest_size
            })
            .map(|entry| entry.local_offset_before)
            .max()
            .unwrap_or(0);
        let use_effective_weights = entries.clone().any(|entry| {
            entry_is_optimizable(entry)
                && entry.compressed_size_before == largest_size
                && u64::from(entry.compressed_size_before) * 100
                    >= u64::from(entry.uncompressed_size) * 98
        });
        let mut total_weight = 0.0_f64;
        for entry in entries.filter(|entry| entry_is_optimizable(entry)) {
            let weight = zip_stream_weight(entry, use_effective_weights);
            total_weight += weight;
        }
        Self {
            largest_size,
            reserved_largest_offset,
            total_weight,
            use_effective_weights,
        }
    }

    fn timeout_for(self, configured: Duration, remaining: Duration, entry: &Entry) -> Duration {
        if !entry_is_optimizable(entry) || remaining.is_zero() {
            return remaining;
        }
        let headroom = scale_duration(remaining, 0.98);
        if entry.compressed_size_before == self.largest_size
            && entry.local_offset_before == self.reserved_largest_offset
        {
            return headroom;
        }
        let weight = zip_stream_weight(entry, self.use_effective_weights);
        if weight <= 0.0 || self.total_weight <= 0.0 {
            return Duration::ZERO;
        }
        scale_duration(configured, weight / self.total_weight).min(headroom)
    }
}

fn zip_stream_weight(entry: &Entry, effective: bool) -> f64 {
    if !effective {
        // Deflate planning repeatedly scans decoded bytes and their token
        // model. Compressed size can badly underweight an efficient member
        // whose optimization work is comparable to a larger neighbour.
        return f64::from(entry.uncompressed_size.max(1));
    }
    let slack = entry
        .uncompressed_size
        .saturating_sub(entry.compressed_size_before);
    (f64::from(slack) + f64::from(entry.compressed_size_before) * 0.05).max(1.0)
}

fn find_end_of_central_directory(input: &[u8]) -> Option<usize> {
    let maximum_search = input.len().min(65_557); // EOCD + maximum 65,535-byte comment.
    for distance_from_end in 22..=maximum_search {
        let offset = input.len() - distance_from_end;
        if read_le32(input, offset) == Some(END_OF_CENTRAL_DIRECTORY) {
            let comment_length = read_le16(input, offset + 20)? as usize;
            if offset.checked_add(22 + comment_length) == Some(input.len()) {
                return Some(offset);
            }
        }
    }
    None
}

fn parse_central_entries(
    input: &[u8],
    central_offset: usize,
    eocd_offset: usize,
    count: u16,
    strip_metadata: bool,
) -> Result<Vec<Entry>> {
    let mut entries = try_vec_with_capacity(count as usize, MODEL_ALLOCATION_ERROR)?;
    let mut position = central_offset;

    for _ in 0..count {
        let header = input
            .get(position..position.saturating_add(46))
            .filter(|_| read_le32(input, position) == Some(CENTRAL_DIRECTORY_HEADER))
            .ok_or_else(|| Error::new("bad ZIP central directory"))?;
        let name_length = le16(header, 28) as usize;
        let extra_length = le16(header, 30) as usize;
        let comment_length = le16(header, 32) as usize;
        let record_size = 46_usize
            .checked_add(name_length)
            .and_then(|size| size.checked_add(extra_length))
            .and_then(|size| size.checked_add(comment_length))
            .filter(|&size| size <= eocd_offset.saturating_sub(position))
            .ok_or_else(|| Error::new("truncated ZIP central directory"))?;

        let compressed_size = le32(header, 20);
        let uncompressed_size = le32(header, 24);
        let local_offset = le32(header, 42);
        if le16(header, 34) != 0 {
            return Err(Error::new("spanned ZIP archives are not supported"));
        }
        if compressed_size == u32::MAX || uncompressed_size == u32::MAX || local_offset == u32::MAX
        {
            return Err(Error::new("ZIP64 entries are not supported"));
        }

        let flags = le16(header, 8);
        let method = le16(header, 10);
        let name = &input[position + 46..position + 46 + name_length];
        let understood = flags & FLAG_ENCRYPTED == 0 && matches!(method, 0 | 8);
        entries.push(Entry {
            local: Vec::new(),
            local_size_before: 0,
            central_offset: position,
            central_size: record_size,
            local_offset_before: local_offset as usize,
            local_offset_after: 0,
            crc32: le32(header, 16),
            compressed_size_before: compressed_size,
            compressed_size_after: compressed_size,
            uncompressed_size,
            method,
            flags,
            skip: strip_metadata
                && name.last() == Some(&b'/')
                && compressed_size == 0
                && uncompressed_size == 0,
            strip_extra: strip_metadata && understood,
            source_deflate_bits: 0,
            output_deflate_bits: 0,
        });
        position += record_size;
    }

    if position != eocd_offset {
        return Err(Error::new("bad ZIP central directory"));
    }
    Ok(entries)
}

/// Build one complete local record. The returned boolean is the raw
/// optimizer's timeout flag.
fn build_local_entry(
    input: &[u8],
    central_offset: usize,
    entry: &mut Entry,
    options: &Options,
    default_floor: DefaultFloor,
) -> Result<bool> {
    let position = entry.local_offset_before;
    let layout = local_entry_layout(input, central_offset, entry)?;
    if layout.total_size != entry.local_size_before {
        return Err(Error::new("ZIP local entry changed after preflight"));
    }
    let prefix_size = layout.prefix_size;
    let data_offset = layout.data_offset;
    let data_end = layout.data_end;
    let old_local_size = layout.total_size;

    // ZipCrypto uses a different password-check byte when bit 3 is set. We
    // cannot clear the data-descriptor flag without decrypting and rewriting
    // its encryption header, so preserve encrypted local records exactly.
    // Other entries can safely move the descriptor values into the header.
    if entry.flags & FLAG_ENCRYPTED != 0 {
        entry.local = try_copy_bytes(
            &input[position..position + old_local_size],
            LOCAL_ALLOCATION_ERROR,
        )?;
        return Ok(false);
    }

    let source_payload = &input[data_offset..data_end];
    let mut payload = try_copy_bytes(source_payload, LOCAL_ALLOCATION_ERROR)?;
    let mut timed_out = false;

    if entry.method == 0 {
        if crc32_update(0, source_payload) != entry.crc32
            || source_payload.len() != entry.uncompressed_size as usize
        {
            return Err(Error::new("ZIP stored member CRC or size mismatch"));
        }
    } else if entry.method == 8 {
        let raw = optimize_raw_prefix_with_floor(
            source_payload,
            options,
            u64::from(entry.uncompressed_size),
            default_floor,
        )
        .map_err(|error| {
            if error.message().contains("internal memory safety") {
                error
            } else if error.message().contains("limit") || error.message().contains("safety") {
                Error::new("ZIP member expands beyond its declared size")
            } else {
                Error::new("invalid ZIP deflate member")
            }
        })?;
        if raw.consumed != source_payload.len()
            || raw.info.crc32 != entry.crc32
            || raw.info.size as u32 != entry.uncompressed_size
        {
            return Err(Error::new("ZIP deflate member CRC or size mismatch"));
        }
        timed_out = raw.timed_out;
        entry.source_deflate_bits = raw.info.source_deflate_bits;
        entry.output_deflate_bits = raw.info.source_deflate_bits;
        if raw.data.len() <= source_payload.len() || options.strict {
            entry.compressed_size_after = u32::try_from(raw.data.len())
                .map_err(|_| Error::new("ZIP local entry too large"))?;
            entry.output_deflate_bits = raw.info.deflate_bits;
            payload = raw.data;
        }
    }

    let name_length = le16(&input[position..position + 30], 26) as usize;
    let new_prefix_size = if entry.strip_extra {
        30 + name_length
    } else {
        prefix_size
    };
    let local_size = new_prefix_size
        .checked_add(payload.len())
        .ok_or_else(|| Error::new("ZIP local entry too large"))?;
    let mut local = try_vec_with_capacity(local_size, LOCAL_ALLOCATION_ERROR)?;
    local.extend_from_slice(&input[position..position + new_prefix_size]);
    put_le16(&mut local, 6, entry.flags & !FLAG_DATA_DESCRIPTOR);
    if entry.strip_extra {
        put_le16(&mut local, 28, 0);
    }
    put_le32(&mut local, 14, entry.crc32);
    put_le32(&mut local, 18, entry.compressed_size_after);
    put_le32(&mut local, 22, entry.uncompressed_size);
    local.extend_from_slice(&payload);

    entry.local = local;
    Ok(timed_out)
}

fn local_entry_layout(
    input: &[u8],
    central_offset: usize,
    entry: &Entry,
) -> Result<LocalEntryLayout> {
    let position = entry.local_offset_before;
    let header = input
        .get(position..position.saturating_add(30))
        .filter(|_| read_le32(input, position) == Some(LOCAL_FILE_HEADER))
        .ok_or_else(|| Error::new("bad ZIP local header"))?;
    let name_length = le16(header, 26) as usize;
    let extra_length = le16(header, 28) as usize;
    let prefix_size = 30_usize
        .checked_add(name_length)
        .and_then(|size| size.checked_add(extra_length))
        .ok_or_else(|| Error::new("truncated ZIP local entry"))?;
    let data_offset = position
        .checked_add(prefix_size)
        .filter(|&offset| offset <= central_offset)
        .ok_or_else(|| Error::new("truncated ZIP local entry"))?;

    let central_header = input
        .get(entry.central_offset..entry.central_offset.saturating_add(46))
        .ok_or_else(|| Error::new("bad ZIP central directory"))?;
    let central_name_length = le16(central_header, 28) as usize;
    let local_name_start = position
        .checked_add(30)
        .ok_or_else(|| Error::new("truncated ZIP local entry"))?;
    let local_name = input
        .get(local_name_start..local_name_start.saturating_add(name_length))
        .ok_or_else(|| Error::new("truncated ZIP local entry"))?;
    let central_name_start = entry
        .central_offset
        .checked_add(46)
        .ok_or_else(|| Error::new("bad ZIP central directory"))?;
    let central_name = input
        .get(central_name_start..central_name_start.saturating_add(central_name_length))
        .ok_or_else(|| Error::new("bad ZIP central directory"))?;
    if le16(header, 6) != entry.flags
        || le16(header, 8) != entry.method
        || local_name != central_name
        || (entry.flags & FLAG_DATA_DESCRIPTOR == 0
            && (le32(header, 14) != entry.crc32
                || le32(header, 18) != entry.compressed_size_before
                || le32(header, 22) != entry.uncompressed_size))
    {
        return Err(Error::new("mismatched ZIP local and central headers"));
    }
    let data_end = data_offset
        .checked_add(entry.compressed_size_before as usize)
        .filter(|&offset| offset <= central_offset)
        .ok_or_else(|| Error::new("truncated ZIP local entry"))?;

    let descriptor_length = if entry.flags & FLAG_DATA_DESCRIPTOR != 0 {
        descriptor_length(input, data_end, central_offset, entry)?
    } else {
        0
    };

    let total_size = prefix_size
        .checked_add(entry.compressed_size_before as usize)
        .and_then(|size| size.checked_add(descriptor_length))
        .ok_or_else(|| Error::new("ZIP local entry too large"))?;
    position
        .checked_add(total_size)
        .filter(|&end| end <= central_offset)
        .ok_or_else(|| Error::new("ZIP local entries extend into central directory"))?;

    Ok(LocalEntryLayout {
        prefix_size,
        data_offset,
        data_end,
        total_size,
    })
}

fn output_flags(entry: &Entry) -> u16 {
    if entry.flags & FLAG_ENCRYPTED != 0 {
        entry.flags
    } else {
        entry.flags & !FLAG_DATA_DESCRIPTOR
    }
}

fn descriptor_length(input: &[u8], position: usize, limit: usize, entry: &Entry) -> Result<usize> {
    let has_signature = read_le32(input, position) == Some(DATA_DESCRIPTOR);
    if has_signature && descriptor_fields_match(input, position + 4, limit, entry) {
        return Ok(16);
    }
    // The signature is optional. A signatureless descriptor is ambiguous when
    // its CRC-32 happens to equal 0x08074b50, so fall back to interpreting that
    // first word as the CRC if the signed layout did not match.
    if descriptor_fields_match(input, position, limit, entry) {
        return Ok(12);
    }
    Err(Error::new("invalid ZIP data descriptor"))
}

fn descriptor_fields_match(input: &[u8], fields: usize, limit: usize, entry: &Entry) -> bool {
    fields.checked_add(12).is_some_and(|end| end <= limit)
        && read_le32(input, fields) == Some(entry.crc32)
        && read_le32(input, fields + 4) == Some(entry.compressed_size_before)
        && read_le32(input, fields + 8) == Some(entry.uncompressed_size)
}

fn read_le16(input: &[u8], offset: usize) -> Option<u16> {
    Some(u16::from_le_bytes(
        input.get(offset..offset + 2)?.try_into().ok()?,
    ))
}

fn read_le32(input: &[u8], offset: usize) -> Option<u32> {
    Some(u32::from_le_bytes(
        input.get(offset..offset + 4)?.try_into().ok()?,
    ))
}

fn le16(input: &[u8], offset: usize) -> u16 {
    u16::from_le_bytes(input[offset..offset + 2].try_into().unwrap())
}

fn le32(input: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes(input[offset..offset + 4].try_into().unwrap())
}

fn put_le16(output: &mut [u8], offset: usize, value: u16) {
    output[offset..offset + 2].copy_from_slice(&value.to_le_bytes());
}

fn put_le32(output: &mut [u8], offset: usize, value: u32) {
    output[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    fn ordering_entry(method: u16, compressed_size: u32, offset: usize) -> Entry {
        Entry {
            local: Vec::new(),
            local_size_before: 0,
            central_offset: 0,
            central_size: 0,
            local_offset_before: offset,
            local_offset_after: 0,
            crc32: 0,
            compressed_size_before: compressed_size,
            compressed_size_after: compressed_size,
            uncompressed_size: 0,
            method,
            flags: 0,
            skip: false,
            strip_extra: false,
            source_deflate_bits: 0,
            output_deflate_bits: 0,
        }
    }

    fn single_entry_archive(
        method: u16,
        payload: &[u8],
        crc: u32,
        uncompressed_size: u32,
        descriptor: bool,
        metadata: bool,
    ) -> Vec<u8> {
        let name = b"a";
        let extra: &[u8] = if metadata { &[0xca, 0xfe] } else { &[] };
        let file_comment: &[u8] = if metadata { b"note" } else { b"" };
        let archive_comment: &[u8] = if metadata { b"archive" } else { b"" };
        let flags = if descriptor { FLAG_DATA_DESCRIPTOR } else { 0 };
        let compressed_size = payload.len() as u32;

        let mut output = Vec::new();
        output.extend_from_slice(&LOCAL_FILE_HEADER.to_le_bytes());
        output.extend_from_slice(&20_u16.to_le_bytes()); // version needed
        output.extend_from_slice(&flags.to_le_bytes());
        output.extend_from_slice(&method.to_le_bytes());
        output.extend_from_slice(&[0; 4]); // time and date
        output.extend_from_slice(&(if descriptor { 0 } else { crc }).to_le_bytes());
        output.extend_from_slice(&(if descriptor { 0 } else { compressed_size }).to_le_bytes());
        output.extend_from_slice(&(if descriptor { 0 } else { uncompressed_size }).to_le_bytes());
        output.extend_from_slice(&(name.len() as u16).to_le_bytes());
        output.extend_from_slice(&(extra.len() as u16).to_le_bytes());
        output.extend_from_slice(name);
        output.extend_from_slice(extra);
        output.extend_from_slice(payload);
        if descriptor {
            output.extend_from_slice(&DATA_DESCRIPTOR.to_le_bytes());
            output.extend_from_slice(&crc.to_le_bytes());
            output.extend_from_slice(&compressed_size.to_le_bytes());
            output.extend_from_slice(&uncompressed_size.to_le_bytes());
        }

        let central_offset = output.len() as u32;
        output.extend_from_slice(&CENTRAL_DIRECTORY_HEADER.to_le_bytes());
        output.extend_from_slice(&20_u16.to_le_bytes()); // version made by
        output.extend_from_slice(&20_u16.to_le_bytes()); // version needed
        output.extend_from_slice(&flags.to_le_bytes());
        output.extend_from_slice(&method.to_le_bytes());
        output.extend_from_slice(&[0; 4]);
        output.extend_from_slice(&crc.to_le_bytes());
        output.extend_from_slice(&compressed_size.to_le_bytes());
        output.extend_from_slice(&uncompressed_size.to_le_bytes());
        output.extend_from_slice(&(name.len() as u16).to_le_bytes());
        output.extend_from_slice(&(extra.len() as u16).to_le_bytes());
        output.extend_from_slice(&(file_comment.len() as u16).to_le_bytes());
        output.extend_from_slice(&[0; 8]); // disk, attributes
        output.extend_from_slice(&0_u32.to_le_bytes()); // local offset
        output.extend_from_slice(name);
        output.extend_from_slice(extra);
        output.extend_from_slice(file_comment);
        let central_size = output.len() as u32 - central_offset;

        output.extend_from_slice(&END_OF_CENTRAL_DIRECTORY.to_le_bytes());
        output.extend_from_slice(&[0; 4]);
        output.extend_from_slice(&1_u16.to_le_bytes());
        output.extend_from_slice(&1_u16.to_le_bytes());
        output.extend_from_slice(&central_size.to_le_bytes());
        output.extend_from_slice(&central_offset.to_le_bytes());
        output.extend_from_slice(&(archive_comment.len() as u16).to_le_bytes());
        output.extend_from_slice(archive_comment);
        output
    }

    #[test]
    fn accepts_an_empty_classic_archive() {
        let input = [
            0x50, 0x4b, 0x05, 0x06, // EOCD
            0, 0, 0, 0, // disk numbers
            0, 0, 0, 0, // entry counts
            0, 0, 0, 0, // central size
            0, 0, 0, 0, // central offset
            0, 0, // comment length
        ];
        let result = optimize(&input, &Options::default()).unwrap();
        assert_eq!(result.data, input);
    }

    #[test]
    fn optimization_order_is_separate_from_archive_order() {
        let entries = [
            ordering_entry(8, 100, 0),
            ordering_entry(0, 500, 1),
            ordering_entry(8, 1_000, 2),
        ];

        assert_eq!(optimization_order(&entries, false).unwrap(), [2, 0, 1]);
        assert_eq!(optimization_order(&entries, true).unwrap(), [0, 2, 1]);
    }

    #[test]
    fn max_schedule_reserves_the_remainder_for_one_largest_member() {
        let mut small = ordering_entry(8, 100, 0);
        small.uncompressed_size = 300;
        let mut largest = ordering_entry(8, 1_000, 1);
        largest.uncompressed_size = 2_000;
        let entries = [small, largest];
        let schedule = ZipSchedule::new(&entries);

        assert_eq!(
            schedule.timeout_for(Duration::from_secs(10), Duration::from_secs(8), &entries[0]),
            Duration::from_secs_f64(10.0 * 300.0 / 2_300.0)
        );
        assert_eq!(
            schedule.timeout_for(Duration::from_secs(10), Duration::from_secs(8), &entries[1]),
            Duration::from_millis(7_840)
        );
    }

    #[test]
    fn max_schedule_uses_the_final_member_when_largest_sizes_tie() {
        let mut first = ordering_entry(8, 1_000, 0);
        first.uncompressed_size = 2_000;
        let mut final_tie = ordering_entry(8, 1_000, 1);
        final_tie.uncompressed_size = 2_000;
        let entries = [first, final_tie];
        let schedule = ZipSchedule::new(&entries);

        assert_eq!(
            schedule.timeout_for(
                Duration::from_secs(10),
                Duration::from_secs(10),
                &entries[0]
            ),
            Duration::from_secs(5)
        );
        assert_eq!(
            schedule.timeout_for(Duration::from_secs(10), Duration::from_secs(6), &entries[1]),
            Duration::from_millis(5_880)
        );
    }

    #[test]
    fn rejects_eocd_with_a_truncated_comment() {
        let mut input = vec![0; 22];
        input[..4].copy_from_slice(&END_OF_CENTRAL_DIRECTORY.to_le_bytes());
        input[20..22].copy_from_slice(&1_u16.to_le_bytes());
        let error = optimize(&input, &Options::default()).unwrap_err();
        assert_eq!(error.message(), "ZIP end of central directory not found");
    }

    #[test]
    fn rejects_a_central_entry_on_another_disk() {
        let mut input = single_entry_archive(0, b"x", crc32_update(0, b"x"), 1, false, false);
        let central = input
            .windows(4)
            .position(|bytes| bytes == CENTRAL_DIRECTORY_HEADER.to_le_bytes())
            .unwrap();
        put_le16(&mut input, central + 34, 1);

        let error = optimize(&input, &Options::default()).unwrap_err();
        assert_eq!(error.message(), "spanned ZIP archives are not supported");
    }

    #[test]
    fn rejects_mismatched_local_and_central_methods() {
        let deflate = [0x01, 0x01, 0x00, 0xfe, 0xff, b'x'];
        let mut input = single_entry_archive(8, &deflate, crc32_update(0, b"x"), 1, false, false);
        put_le16(&mut input, 8, 0); // Only the local method is changed.

        let error = optimize(&input, &Options::default()).unwrap_err();
        assert_eq!(error.message(), "mismatched ZIP local and central headers");
    }

    #[test]
    fn rejects_a_zero_byte_deflate_member() {
        let input = single_entry_archive(8, &[], 0, 0, false, false);
        let error = optimize(&input, &Options::default()).unwrap_err();
        assert_eq!(error.message(), "invalid ZIP deflate member");
    }

    #[test]
    fn rejects_mismatched_local_crc_without_a_data_descriptor() {
        let mut input = single_entry_archive(0, b"x", crc32_update(0, b"x"), 1, false, false);
        put_le32(&mut input, 14, 0);

        let error = optimize(&input, &Options::default()).unwrap_err();
        assert_eq!(error.message(), "mismatched ZIP local and central headers");
    }

    #[test]
    fn rejects_duplicate_local_ranges_before_member_work() {
        let input = single_entry_archive(0, b"x", crc32_update(0, b"x"), 1, false, false);
        let eocd_offset = find_end_of_central_directory(&input).unwrap();
        let central_offset = le32(&input[eocd_offset..], 16) as usize;
        let central_record = input[central_offset..eocd_offset].to_vec();
        let mut eocd = input[eocd_offset..].to_vec();
        put_le16(&mut eocd, 8, 2);
        put_le16(&mut eocd, 10, 2);
        put_le32(&mut eocd, 12, (central_record.len() * 2) as u32);

        let mut duplicated = input[..eocd_offset].to_vec();
        duplicated.extend_from_slice(&central_record);
        duplicated.extend_from_slice(&eocd);

        let error = optimize(&duplicated, &Options::default()).unwrap_err();
        assert_eq!(error.message(), "overlapping ZIP local entries");
    }

    #[test]
    fn accepts_signatureless_descriptor_whose_crc_looks_like_a_signature() {
        let entry = Entry {
            local: Vec::new(),
            local_size_before: 0,
            central_offset: 0,
            central_size: 0,
            local_offset_before: 0,
            local_offset_after: 0,
            crc32: DATA_DESCRIPTOR,
            compressed_size_before: 9,
            compressed_size_after: 9,
            uncompressed_size: 12,
            method: 8,
            flags: FLAG_DATA_DESCRIPTOR,
            skip: false,
            strip_extra: false,
            source_deflate_bits: 0,
            output_deflate_bits: 0,
        };
        let mut descriptor = Vec::new();
        descriptor.extend_from_slice(&DATA_DESCRIPTOR.to_le_bytes());
        descriptor.extend_from_slice(&9_u32.to_le_bytes());
        descriptor.extend_from_slice(&12_u32.to_le_bytes());

        assert_eq!(
            descriptor_length(&descriptor, 0, descriptor.len(), &entry).unwrap(),
            12
        );
    }

    #[test]
    fn removes_a_classic_data_descriptor_and_preserves_the_member() {
        let crc = crc32_update(0, b"x");
        let input = single_entry_archive(0, b"x", crc, 1, true, false);
        let result = optimize(&input, &Options::default()).unwrap();

        assert!(result.data.len() < input.len());
        assert_eq!(le16(&result.data, 6) & FLAG_DATA_DESCRIPTOR, 0);
        assert_eq!(le32(&result.data, 14), crc);
        assert_eq!(le32(&result.data, 18), 1);
        // The rewritten directory offsets and sizes are self-consistent.
        let second = optimize(&result.data, &Options::default()).unwrap();
        assert_eq!(second.data, result.data);
    }

    #[test]
    fn preserves_encrypted_entries_that_use_data_descriptors() {
        // The bytes are deliberately opaque: encrypted entries must not be
        // parsed or normalized without a password. Bit 3 also selects the
        // meaning of ZipCrypto's password-check byte, so it is part of the
        // encrypted representation rather than disposable bookkeeping.
        let payload = [0x5a; 24];
        let mut input = single_entry_archive(8, &payload, 0x1234_5678, 100, true, false);
        put_le16(&mut input, 6, FLAG_ENCRYPTED | FLAG_DATA_DESCRIPTOR);
        let central = input
            .windows(4)
            .position(|bytes| bytes == CENTRAL_DIRECTORY_HEADER.to_le_bytes())
            .unwrap();
        put_le16(
            &mut input,
            central + 8,
            FLAG_ENCRYPTED | FLAG_DATA_DESCRIPTOR,
        );

        let result = optimize(&input, &Options::default()).unwrap();
        assert_eq!(result.data, input);
        assert_eq!(deflate_stream_count(&input).unwrap(), 0);
    }

    #[test]
    fn optimizes_a_deflated_member_without_recompressing_it() {
        // One-byte stored Deflate block containing "x".
        let deflate = [0x01, 0x01, 0x00, 0xfe, 0xff, b'x'];
        let input = single_entry_archive(8, &deflate, crc32_update(0, b"x"), 1, false, false);
        assert_eq!(deflate_stream_count(&input).unwrap(), 1);
        let result = optimize(&input, &Options::default()).unwrap();

        assert!(result.data.len() <= input.len());
        optimize(&result.data, &Options::default()).unwrap();
    }

    #[test]
    fn deflated_member_reports_same_byte_bit_savings() {
        let decoded = [b'A'; 168];
        let input = single_entry_archive(
            8,
            super::super::SAME_BYTE_BIT_WIN_RAW,
            crc32_update(0, &decoded),
            decoded.len() as u32,
            false,
            false,
        );
        let optimized = optimize(&input, &Options::default()).unwrap();

        assert_eq!(optimized.data.len(), input.len());
        assert_eq!(optimized.bits_saved, 1);
    }

    #[test]
    fn bounded_max_deadline_still_returns_a_valid_zip() {
        // Two one-byte stored blocks can be joined without Huffman or token
        // search, even when the file-wide optional-search deadline is zero.
        let deflate = [
            0x00, 0x01, 0x00, 0xfe, 0xff, b'x', 0x01, 0x01, 0x00, 0xfe, 0xff, b'y',
        ];
        let input = single_entry_archive(8, &deflate, crc32_update(0, b"xy"), 2, false, false);
        let options = Options {
            exhaustive: true,
            timeout: Duration::ZERO,
            ..Options::default()
        };

        let result = optimize(&input, &options).unwrap();
        assert!(result.timed_out);
        assert!(result.data.len() < input.len());
        optimize(&result.data, &Options::default()).unwrap();
    }

    #[test]
    fn local_member_slice_is_reclaimed_without_a_file_timeout() {
        let deflate = [
            0x00, 0x01, 0x00, 0xfe, 0xff, b'x', 0x01, 0x01, 0x00, 0xfe, 0xff, b'y',
        ];
        let input = single_entry_archive(8, &deflate, crc32_update(0, b"xy"), 2, false, false);
        let eocd_offset = find_end_of_central_directory(&input).unwrap();
        let eocd = &input[eocd_offset..eocd_offset + 22];
        let central_offset = le32(eocd, 16) as usize;
        let mut entries = parse_central_entries(&input, central_offset, eocd_offset, 1, false)
            .expect("synthetic archive should parse");
        preflight_local_entries(&input, central_offset, &mut entries).unwrap();

        let options = Options {
            exhaustive: true,
            timeout: Duration::from_secs(1),
            ..Options::default()
        };
        let deadline = SearchDeadline::new(&options);
        let deliberately_tiny_slice = ZipSchedule {
            largest_size: u32::MAX,
            reserved_largest_offset: usize::MAX,
            total_weight: f64::MAX,
            use_effective_weights: false,
        };
        let local_timed_out = build_scheduled_entry(
            &input,
            central_offset,
            &mut entries[0],
            None,
            &options,
            DefaultFloor::Shared,
            &deadline,
            Some(deliberately_tiny_slice),
        )
        .unwrap();
        assert!(local_timed_out);
        assert!(!deadline.is_expired());

        reclaim_timed_out_entries(
            &input,
            central_offset,
            &mut entries,
            &[None],
            vec![0],
            &options,
            DefaultFloor::Shared,
            &deadline,
            Some(deliberately_tiny_slice),
        )
        .unwrap();
        assert!(!deadline.is_expired());
    }

    #[test]
    fn max_retains_the_complete_default_archive() {
        let deflate = [0x01, 0x02, 0x00, 0xfd, 0xff, b'x', b'y'];
        let input = single_entry_archive(8, &deflate, crc32_update(0, b"xy"), 2, false, false);
        let default = optimize(&input, &Options::default()).unwrap();
        let max = optimize(
            &input,
            &Options {
                exhaustive: true,
                timeout: Duration::from_secs(1),
                ..Options::default()
            },
        )
        .unwrap();

        assert!(max.data.len() <= default.data.len());
        if max.data.len() == default.data.len() {
            assert!(max.bits_saved >= default.bits_saved);
        }
    }

    #[test]
    fn parallel_max_selection_is_byte_first_then_bit_first() {
        let floor = Optimization {
            data: vec![0; 10],
            bits_saved: 3,
            timed_out: false,
        };
        let bit_winner = Optimization {
            data: vec![1; 10],
            bits_saved: 4,
            timed_out: true,
        };
        let selected = best_complete_optimization(floor.clone(), bit_winner);
        assert_eq!(selected.data, vec![1; 10]);
        assert!(selected.timed_out);

        let byte_winner = Optimization {
            data: vec![2; 9],
            bits_saved: 8,
            timed_out: false,
        };
        let selected = best_complete_optimization(floor, byte_winner);
        assert_eq!(selected.data, vec![2; 9]);
    }

    #[test]
    fn parallel_max_archive_requires_bounded_work() {
        let deflate = [0x01, 0x01, 0x00, 0xfe, 0xff, b'x'];
        let input = single_entry_archive(8, &deflate, crc32_update(0, b"x"), 1, false, false);
        assert!(parallel_max_archive_is_bounded(&input));

        let oversized = single_entry_archive(
            8,
            &deflate,
            crc32_update(0, b"x"),
            u32::try_from(PARALLEL_MAX_ARCHIVE_DECODED + 1).unwrap(),
            false,
            false,
        );
        assert!(!parallel_max_archive_is_bounded(&oversized));
    }

    #[test]
    fn parallel_max_archive_avoids_duplicate_work_on_uniform_member_sets() {
        let stored_only: Vec<_> = (0..8).map(|index| ordering_entry(0, 100, index)).collect();
        assert!(!parallel_max_entry_work_is_bounded(&stored_only));

        let uniform: Vec<_> = (0..8)
            .map(|index| {
                let mut entry = ordering_entry(8, 100, index);
                entry.uncompressed_size = 200;
                entry
            })
            .collect();
        assert!(!parallel_max_entry_work_is_bounded(&uniform));

        // A dominant member gives the independent archive branches useful,
        // different work even when the archive also contains many tiny files.
        let mut skewed = uniform;
        skewed[0].compressed_size_before = 101;
        assert!(parallel_max_entry_work_is_bounded(&skewed));
    }

    #[test]
    fn two_phase_savings_are_relative_to_the_original_archive() {
        let floor = Optimization {
            data: vec![0; 9],
            bits_saved: 8,
            timed_out: false,
        };
        let refined = Optimization {
            data: vec![0; 8],
            bits_saved: 8,
            timed_out: false,
        };
        assert_eq!(combined_savings(10, &floor, &refined), 16);

        let equal_floor = Optimization {
            data: vec![0; 10],
            bits_saved: 3,
            timed_out: false,
        };
        let equal_refined = Optimization {
            data: vec![0; 10],
            bits_saved: 5,
            timed_out: false,
        };
        assert_eq!(combined_savings(10, &equal_floor, &equal_refined), 8);
    }

    #[test]
    fn strip_removes_classic_zip_comments_and_supported_extras() {
        let input = single_entry_archive(0, b"x", crc32_update(0, b"x"), 1, false, true);
        let options = Options {
            strip_metadata: true,
            ..Options::default()
        };
        let result = optimize(&input, &options).unwrap();

        assert!(result.data.len() < input.len());
        assert_eq!(le16(&result.data, 28), 0); // local extra length
        let eocd = find_end_of_central_directory(&result.data).unwrap();
        assert_eq!(le16(&result.data[eocd..], 20), 0);
        optimize(&result.data, &Options::default()).unwrap();
    }

    #[test]
    fn enforces_the_archive_wide_declared_expansion_limit() {
        let deflate = [0x01, 0x01, 0x00, 0xfe, 0xff, b'x'];
        let input = single_entry_archive(8, &deflate, crc32_update(0, b"x"), 1, false, false);
        let options = Options {
            max_decoded_bytes: 0,
            ..Options::default()
        };
        let error = optimize(&input, &options).unwrap_err();
        assert_eq!(
            error.message(),
            "ZIP expanded data exceeds configured safety limit"
        );
    }

    #[test]
    fn stored_members_count_toward_the_archive_expansion_limit() {
        let input = single_entry_archive(0, b"x", crc32_update(0, b"x"), 1, false, false);
        let options = Options {
            max_decoded_bytes: 0,
            ..Options::default()
        };

        let error = optimize(&input, &options).unwrap_err();
        assert_eq!(
            error.message(),
            "ZIP expanded data exceeds configured safety limit"
        );
    }
}
