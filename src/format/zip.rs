// SPDX-License-Identifier: MIT

use crate::checksum::crc32_update;
use crate::deflate::{optimize_raw_prefix_with_floor, DefaultFloor};
use crate::{Error, Optimization, Options, Result};

use std::time::Duration;

use super::{
    scale_duration, try_append_bytes, try_copy_bytes, try_vec_with_capacity, SearchDeadline,
};

const LOCAL_FILE_HEADER: u32 = 0x0403_4b50;
const CENTRAL_DIRECTORY_HEADER: u32 = 0x0201_4b50;
const END_OF_CENTRAL_DIRECTORY: u32 = 0x0605_4b50;
const DATA_DESCRIPTOR: u32 = 0x0807_4b50;

const FLAG_ENCRYPTED: u16 = 0x0001;
const FLAG_DATA_DESCRIPTOR: u16 = 0x0008;
const OUTPUT_ALLOCATION_ERROR: &str = "could not allocate ZIP output";
const MODEL_ALLOCATION_ERROR: &str = "could not allocate ZIP entry model";
const LOCAL_ALLOCATION_ERROR: &str = "could not allocate ZIP local entry";

#[derive(Debug)]
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
}

pub(super) fn optimize(input: &[u8], options: &Options) -> Result<Optimization> {
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
    let mut timed_out = false;
    for index in build_order {
        let mut call_options = deadline.options_for_call(options);
        if let Some(schedule) = schedule {
            call_options.timeout =
                schedule.timeout_for(options.timeout, deadline.remaining(), &entries[index]);
        }
        timed_out |= build_local_entry(input, central_offset, &mut entries[index], &call_options)?;
    }

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

    if output.len() > input.len() && !options.min_distance_codes {
        output.clear();
        try_append_bytes(&mut output, input, OUTPUT_ALLOCATION_ERROR)?;
    }

    Ok(Optimization {
        data: output,
        timed_out,
    })
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
/// member inherits the actual remainder, so time unused by the earlier slices
/// is not lost even when several members tie for largest. Nearly incompressible
/// archives use their small amount of compression slack as the weight;
/// otherwise four huge stored-like members would crowd useful work on small,
/// compressible entries out of the schedule.
#[derive(Clone, Copy)]
struct ZipSchedule {
    largest_size: u32,
    reserved_largest_offset: usize,
    largest_weight: f64,
    total_weight: f64,
    use_effective_weights: bool,
}

impl ZipSchedule {
    fn new(entries: &[Entry]) -> Self {
        let largest_size = entries
            .iter()
            .filter(|entry| entry_is_optimizable(entry))
            .map(|entry| entry.compressed_size_before)
            .max()
            .unwrap_or(0);
        let reserved_largest_offset = entries
            .iter()
            .filter(|entry| {
                entry_is_optimizable(entry) && entry.compressed_size_before == largest_size
            })
            .map(|entry| entry.local_offset_before)
            .max()
            .unwrap_or(0);
        let use_effective_weights = entries.iter().any(|entry| {
            entry_is_optimizable(entry)
                && entry.compressed_size_before == largest_size
                && u64::from(entry.compressed_size_before) * 100
                    >= u64::from(entry.uncompressed_size) * 98
        });
        let mut total_weight = 0.0_f64;
        let mut largest_weight = 0.0_f64;
        for entry in entries.iter().filter(|entry| entry_is_optimizable(entry)) {
            let weight = zip_stream_weight(entry, use_effective_weights);
            total_weight += weight;
            if entry.compressed_size_before == largest_size {
                largest_weight = largest_weight.max(weight);
            }
        }
        Self {
            largest_size,
            reserved_largest_offset,
            largest_weight,
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
        let denominator = self.total_weight + self.largest_weight;
        if weight <= 0.0 || denominator <= 0.0 {
            return Duration::ZERO;
        }
        scale_duration(configured, weight / denominator).min(headroom)
    }
}

fn zip_stream_weight(entry: &Entry, effective: bool) -> f64 {
    if !effective {
        return f64::from(entry.compressed_size_before);
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
            DefaultFloor::Shared,
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
        if raw.data.len() <= source_payload.len() || options.min_distance_codes {
            entry.compressed_size_after = u32::try_from(raw.data.len())
                .map_err(|_| Error::new("ZIP local entry too large"))?;
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
        small.uncompressed_size = 200;
        let mut largest = ordering_entry(8, 1_000, 1);
        largest.uncompressed_size = 2_000;
        let entries = [small, largest];
        let schedule = ZipSchedule::new(&entries);

        assert_eq!(
            schedule.timeout_for(Duration::from_secs(10), Duration::from_secs(8), &entries[0]),
            Duration::from_secs_f64(10.0 * 100.0 / 2_100.0)
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
            Duration::from_secs_f64(10.0 / 3.0)
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
    }

    #[test]
    fn optimizes_a_deflated_member_without_recompressing_it() {
        // One-byte stored Deflate block containing "x".
        let deflate = [0x01, 0x01, 0x00, 0xfe, 0xff, b'x'];
        let input = single_entry_archive(8, &deflate, crc32_update(0, b"x"), 1, false, false);
        let result = optimize(&input, &Options::default()).unwrap();

        assert!(result.data.len() <= input.len());
        optimize(&result.data, &Options::default()).unwrap();
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
