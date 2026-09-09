// SPDX-License-Identifier: MIT

//! Concatenated GZIP member validation, scheduling, and wrapper reconstruction.

use crate::checksum::crc32_update;
use crate::deflate::{
    inspect_raw_prefix, optimize_raw_prefix_with_floor, DefaultFloor, RawOptimization,
};
use crate::{Error, Optimization, Options, Result};

use super::{scale_duration, try_append_bytes, try_vec_with_capacity, SearchDeadline};

const FHCRC: u8 = 0x02;
const FEXTRA: u8 = 0x04;
const FNAME: u8 = 0x08;
const FCOMMENT: u8 = 0x10;
const RESERVED_FLAGS: u8 = 0xe0;
const OUTPUT_ALLOCATION_ERROR: &str = "could not allocate GZIP output";
/// Concatenation is legitimate, but an empty member is only twenty bytes. A
/// dedicated cap prevents a small file from multiplying parser setup and
/// checksum work without consuming the decoded-byte budget.
const MAX_GZIP_MEMBERS: usize = 16_384;

#[derive(Clone, Copy)]
pub(super) struct Member {
    start: usize,
    payload_start: usize,
    trailer_start: usize,
    end: usize,
    flags: u8,
    decoded_size: u64,
}

pub(super) fn deflate_stream_count(input: &[u8], max_decoded_bytes: u64) -> Result<usize> {
    Ok(preflight(input, max_decoded_bytes)?.len())
}

#[cfg(test)]
pub(super) fn optimize(input: &[u8], options: &Options) -> Result<Optimization> {
    let members = preflight(input, options.max_decoded_bytes)?;
    optimize_preflight(input, options, members)
}

pub(super) fn preflight(input: &[u8], max_decoded_bytes: u64) -> Result<Vec<Member>> {
    parse_members(input, max_decoded_bytes)
}

pub(super) fn optimize_preflight(
    input: &[u8],
    options: &Options,
    members: Vec<Member>,
) -> Result<Optimization> {
    let deadline = SearchDeadline::new(options);
    let mut order = Vec::new();
    order
        .try_reserve_exact(members.len())
        .map_err(|_| Error::internal("could not allocate GZIP member schedule"))?;
    order.extend(0..members.len());
    order.sort_unstable_by_key(|&index| {
        let member = members[index];
        (member.trailer_start - member.payload_start, index)
    });
    let total_weight = members.iter().try_fold(0_u64, |total, member| {
        total.checked_add(member.decoded_size.max(1))
    });
    let total_weight =
        total_weight.ok_or_else(|| Error::resource_limit("GZIP decoded size is too large"))?;
    let mut results = Vec::new();
    results
        .try_reserve_exact(members.len())
        .map_err(|_| Error::internal("could not allocate GZIP member results"))?;
    results.resize_with(members.len(), || None);
    let mut pending = Vec::new();
    pending
        .try_reserve_exact(members.len())
        .map_err(|_| Error::internal("could not allocate GZIP member schedule"))?;

    for (position, &index) in order.iter().enumerate() {
        let file_remaining = deadline.remaining();
        let mut call_options = options.clone();
        call_options.timeout = gzip_member_timeout(
            options.timeout,
            file_remaining,
            members[index].decoded_size.max(1),
            total_weight,
            position + 1 == order.len(),
        );
        let run = || optimize_member(input, members[index], &call_options);
        let raw = if call_options.timeout < file_remaining {
            crate::progress::with_stream_slice(index + 1, &[], None, run)?
        } else {
            crate::progress::with_stream_group(index + 1, &[], run)?
        };
        if raw.timed_out {
            pending.push(index);
        } else if options.verbose || options.visual {
            crate::progress::complete_stream_group(index + 1, &[]);
        }
        results[index] = Some(raw);
    }

    reclaim_timed_out_members(input, &members, &mut results, pending, options, &deadline)?;
    if options.verbose || options.visual {
        for index in 0..members.len() {
            crate::progress::complete_stream_group(index + 1, &[]);
        }
    }

    let mut output = try_vec_with_capacity(input.len(), OUTPUT_ALLOCATION_ERROR)?;
    let mut source_deflate_bits = 0_u64;
    let mut output_deflate_bits = 0_u64;
    for (index, member) in members.iter().enumerate() {
        let raw = results[index]
            .take()
            .expect("every GZIP member has one optimization result");
        source_deflate_bits = source_deflate_bits
            .checked_add(raw.info.source_deflate_bits)
            .ok_or_else(|| Error::new("GZIP Deflate bit count is too large"))?;
        output_deflate_bits = output_deflate_bits
            .checked_add(raw.info.deflate_bits)
            .ok_or_else(|| Error::new("GZIP Deflate bit count is too large"))?;
        let has_optional_metadata = member.flags & (FEXTRA | FNAME | FCOMMENT | FHCRC) != 0;
        if options.strip_metadata && has_optional_metadata {
            let mut header = input[member.start..member.start + 10].to_vec();
            // FTEXT is a content hint rather than an optional variable-length
            // field. Keep it, and remove only fields whose bytes were dropped.
            header[3] = member.flags & !(FEXTRA | FNAME | FCOMMENT | FHCRC);
            try_append_bytes(&mut output, &header, OUTPUT_ALLOCATION_ERROR)?;
        } else {
            try_append_bytes(
                &mut output,
                &input[member.start..member.payload_start],
                OUTPUT_ALLOCATION_ERROR,
            )?;
        }
        try_append_bytes(&mut output, &raw.data, OUTPUT_ALLOCATION_ERROR)?;
        try_append_bytes(
            &mut output,
            &input[member.trailer_start..member.end],
            OUTPUT_ALLOCATION_ERROR,
        )?;
    }

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
        deadline.is_expired(),
    ))
}

pub(super) fn has_rfc1952_header(input: &[u8]) -> bool {
    input.len() >= 4
        && input[0] == 0x1f
        && input[1] == 0x8b
        && input[2] == 8
        && input[3] & RESERVED_FLAGS == 0
}

pub(super) fn has_signature(input: &[u8]) -> bool {
    input.starts_with(&[0x1f, 0x8b])
}

fn parse_members(input: &[u8], max_decoded_bytes: u64) -> Result<Vec<Member>> {
    if input.is_empty() {
        return Err(Error::new("invalid GZIP signature"));
    }
    let mut members = Vec::new();
    let mut member_start = 0_usize;
    let mut decoded_remaining = max_decoded_bytes;
    while member_start < input.len() {
        if members.len() >= MAX_GZIP_MEMBERS {
            return Err(Error::resource_limit("GZIP contains too many members"));
        }
        if input.len() - member_start < 18
            || input[member_start] != 0x1f
            || input[member_start + 1] != 0x8b
        {
            return Err(Error::new("invalid GZIP signature"));
        }
        if input[member_start + 2] != 8 {
            return Err(Error::new("invalid GZIP compression method"));
        }
        let flags = input[member_start + 3];
        if flags & RESERVED_FLAGS != 0 {
            return Err(Error::new("reserved GZIP flags are set"));
        }

        let mut payload_start = member_start + 10;
        if flags & FEXTRA != 0 {
            let length = read_u16(input, payload_start, "truncated GZIP extra field")? as usize;
            payload_start += 2;
            payload_start = payload_start
                .checked_add(length)
                .filter(|&end| end <= input.len())
                .ok_or_else(|| Error::new("truncated GZIP extra field"))?;
        }
        if flags & FNAME != 0 {
            payload_start = skip_zero_terminated(input, payload_start, "truncated GZIP filename")?;
        }
        if flags & FCOMMENT != 0 {
            payload_start = skip_zero_terminated(input, payload_start, "truncated GZIP comment")?;
        }
        if flags & FHCRC != 0 {
            let stored = read_u16(input, payload_start, "truncated GZIP header CRC")?;
            let calculated = crc32_update(0, &input[member_start..payload_start]) as u16;
            if stored != calculated {
                return Err(Error::integrity_mismatch("GZIP header CRC mismatch"));
            }
            payload_start += 2;
        }

        let (consumed, info) = inspect_raw_prefix(&input[payload_start..], decoded_remaining)?;
        decoded_remaining = decoded_remaining.checked_sub(info.size).ok_or_else(|| {
            Error::resource_limit("decoded GZIP data exceeds configured safety limit")
        })?;
        let trailer_start = payload_start
            .checked_add(consumed)
            .filter(|&start| consumed != 0 && input.len().saturating_sub(start) >= 8)
            .ok_or_else(|| Error::new("missing GZIP trailer"))?;
        let end = trailer_start + 8;
        let stored_crc =
            u32::from_le_bytes(input[trailer_start..trailer_start + 4].try_into().unwrap());
        let stored_size = u32::from_le_bytes(input[trailer_start + 4..end].try_into().unwrap());
        if stored_crc != info.crc32 || stored_size != info.size as u32 {
            return Err(Error::integrity_mismatch(
                "GZIP trailer CRC or size mismatch",
            ));
        }
        members
            .try_reserve(1)
            .map_err(|_| Error::internal("could not allocate GZIP member model"))?;
        members.push(Member {
            start: member_start,
            payload_start,
            trailer_start,
            end,
            flags,
            decoded_size: info.size,
        });
        member_start = end;
    }
    Ok(members)
}

fn optimize_member(input: &[u8], member: Member, options: &Options) -> Result<RawOptimization> {
    let default_floor = member_default_floor(options.exhaustive);
    let raw = optimize_raw_prefix_with_floor(
        &input[member.payload_start..],
        options,
        member.decoded_size,
        default_floor,
    )?;
    if member.payload_start.checked_add(raw.consumed) != Some(member.trailer_start)
        || raw.info.size != member.decoded_size
    {
        return Err(Error::new("invalid GZIP deflate member"));
    }
    Ok(raw)
}

fn member_default_floor(exhaustive: bool) -> DefaultFloor {
    if exhaustive {
        DefaultFloor::SharedExact
    } else {
        DefaultFloor::Shared
    }
}

fn gzip_member_timeout(
    configured: std::time::Duration,
    remaining: std::time::Duration,
    weight: u64,
    total_weight: u64,
    reserved_largest: bool,
) -> std::time::Duration {
    if remaining.is_zero() || weight == 0 || total_weight == 0 {
        return std::time::Duration::ZERO;
    }
    let headroom = scale_duration(remaining, 0.98);
    if reserved_largest {
        return headroom;
    }
    scale_duration(configured, weight as f64 / total_weight as f64 * 0.90).min(headroom)
}

fn raw_is_better(candidate: &RawOptimization, incumbent: &RawOptimization) -> bool {
    candidate.data.len() < incumbent.data.len()
        || (candidate.data.len() == incumbent.data.len()
            && candidate.info.deflate_bits < incumbent.info.deflate_bits)
}

fn reclaim_timed_out_members(
    input: &[u8],
    members: &[Member],
    results: &mut [Option<RawOptimization>],
    mut pending: Vec<usize>,
    options: &Options,
    deadline: &SearchDeadline,
) -> Result<()> {
    while !pending.is_empty() && !deadline.is_expired() {
        let mut remaining_weight = pending
            .iter()
            .try_fold(0_u64, |total, &index| {
                total.checked_add(members[index].decoded_size.max(1))
            })
            .ok_or_else(|| Error::resource_limit("GZIP decoded size is too large"))?;
        let mut still_pending = Vec::new();
        still_pending
            .try_reserve_exact(pending.len())
            .map_err(|_| Error::internal("could not allocate GZIP member schedule"))?;
        for (position, &index) in pending.iter().enumerate() {
            let file_remaining = deadline.remaining();
            if file_remaining.is_zero() {
                break;
            }
            let weight = members[index].decoded_size.max(1);
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
            let run = || optimize_member(input, members[index], &retry_options);
            let retry = crate::progress::with_stream_reclaim(index + 1, &[], !last, run)?;
            let retry_timed_out = retry.timed_out;
            let incumbent = results[index]
                .as_mut()
                .expect("every scheduled GZIP member has an incumbent");
            if raw_is_better(&retry, incumbent) {
                *incumbent = retry;
            } else {
                incumbent.timed_out = retry_timed_out;
            }
            if retry_timed_out && !deadline.is_expired() {
                still_pending.push(index);
            }
        }
        pending = still_pending;
    }
    Ok(())
}

fn read_u16(input: &[u8], position: usize, message: &'static str) -> Result<u16> {
    let bytes = input
        .get(position..position + 2)
        .ok_or_else(|| Error::new(message))?;
    Ok(u16::from_le_bytes(bytes.try_into().unwrap()))
}

fn skip_zero_terminated(input: &[u8], position: usize, message: &'static str) -> Result<usize> {
    input[position..]
        .iter()
        .position(|&byte| byte == 0)
        .map(|offset| position + offset + 1)
        .ok_or_else(|| Error::new(message))
}

#[cfg(test)]
mod tests;
