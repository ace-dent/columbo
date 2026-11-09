// SPDX-License-Identifier: MIT
// SPDX-FileCopyrightText: 2026 Andrew C.E. Dent <https://github.com/ace-dent>

//! Concatenated GZIP member validation, scheduling, and wrapper reconstruction.

use crate::checksum::crc32_update;
use crate::deflate::{
    inspect_raw_prefix, optimize_raw_prefix_with_floor, DefaultFloor, JoinPart, RawOptimization,
    StreamJoiner,
};
use crate::progress::MemberMergeProgress;
use crate::sort::sort_indices_unstable;
use crate::{Error, Optimization, Options, Result};

use super::{scale_duration, try_append_bytes, try_vec_with_capacity, SearchDeadline};

const FTEXT: u8 = 0x01;
const FHCRC: u8 = 0x02;
const FEXTRA: u8 = 0x04;
const FNAME: u8 = 0x08;
const FCOMMENT: u8 = 0x10;
const RESERVED_FLAGS: u8 = 0xe0;
const OUTPUT_ALLOCATION: &str = "GZIP output";
/// Bounds per-member parser and checksum work: RFC 1952 allows any number of
/// members, and an empty member is only 20 bytes.
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

pub(super) fn preflight(input: &[u8], max_decoded_bytes: u64) -> Result<Vec<Member>> {
    parse_members(input, max_decoded_bytes)
}

/// One member's stream as it is written: Max's or Default's result.
#[derive(Clone, Copy)]
struct MemberStream<'a> {
    data: &'a [u8],
    deflate_bits: u64,
}

impl<'a> MemberStream<'a> {
    fn optimized(raw: &'a RawOptimization) -> Self {
        Self {
            data: &raw.data,
            deflate_bits: raw.info.deflate_bits,
        }
    }

    /// Default's result for the member: the endpoint Max kept where it
    /// differs, otherwise the member's own stream.
    fn default_endpoint(raw: &'a RawOptimization) -> Self {
        raw.default_endpoint.as_ref().map_or_else(
            || Self::optimized(raw),
            |endpoint| Self {
                data: &endpoint.data,
                deflate_bits: endpoint.deflate_bits,
            },
        )
    }
}

pub(super) fn optimize_preflight(
    input: &[u8],
    options: &Options,
    members: Vec<Member>,
) -> Result<Optimization> {
    let members = members.as_slice();
    let deadline = SearchDeadline::new(options);
    // Max merges against Default's members as well, below.
    let keep_default_endpoints = options.exhaustive && options.strip_metadata && members.len() > 1;
    let mut order = Vec::new();
    order
        .try_reserve_exact(members.len())
        .map_err(|_| Error::allocation("GZIP member schedule"))?;
    order.extend(0..members.len());
    let key = |index: usize| {
        let member = members[index];
        (member.trailer_start - member.payload_start, index)
    };
    sort_indices_unstable(&mut order, &mut |left, right| key(left).cmp(&key(right)));
    let total_weight = members.iter().try_fold(0_u64, |total, member| {
        total.checked_add(member.decoded_size.max(1))
    });
    let total_weight =
        total_weight.ok_or_else(|| Error::resource_limit("GZIP decoded size is too large"))?;
    let mut results = Vec::new();
    results
        .try_reserve_exact(members.len())
        .map_err(|_| Error::allocation("GZIP member results"))?;
    results.resize_with(members.len(), || None);
    let mut pending = Vec::new();
    pending
        .try_reserve_exact(members.len())
        .map_err(|_| Error::allocation("GZIP member schedule"))?;

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
        let run = || optimize_member(input, members[index], &call_options, keep_default_endpoints);
        let raw = if call_options.timeout < file_remaining {
            crate::progress::with_stream_slice(index + 1, &[], None, run)?
        } else {
            crate::progress::with_stream_group(index + 1, &[], run)?
        };
        if raw.timed_out {
            pending.push(index);
        } else if options.verbose {
            crate::progress::complete_stream_group(index + 1, &[]);
        }
        results[index] = Some(raw);
    }

    reclaim_timed_out_members(
        input,
        members,
        &mut results,
        pending,
        options,
        &deadline,
        keep_default_endpoints,
    )?;
    if options.verbose {
        for index in 0..members.len() {
            crate::progress::complete_stream_group(index + 1, &[]);
        }
    }

    let results: Vec<RawOptimization> = results
        .into_iter()
        .map(|raw| raw.expect("every GZIP member has one optimization result"))
        .collect();
    let mut source_deflate_bits = 0_u64;
    for raw in &results {
        source_deflate_bits = source_deflate_bits
            .checked_add(raw.info.source_deflate_bits)
            .ok_or_else(|| Error::new("GZIP Deflate bit count is too large"))?;
    }
    let mut output = try_vec_with_capacity(input.len(), OUTPUT_ALLOCATION)?;
    let mut output_deflate_bits = if options.strip_metadata && members.len() > 1 {
        let (bits, merge) =
            write_merged_file(input, members, &results, options, &deadline, &mut output)?;
        crate::progress::record_member_merge(options, merge);
        bits
    } else {
        let mut bits = 0_u64;
        for (member, raw) in members.iter().zip(&results) {
            bits = bits
                .checked_add(raw.info.deflate_bits)
                .ok_or_else(|| Error::new("GZIP Deflate bit count is too large"))?;
            write_member(input, member, &raw.data, options, &mut output)?;
        }
        bits
    };

    let stream_end = members
        .last()
        .expect("preflight requires a GZIP member")
        .end;
    if !options.strip_metadata {
        try_append_bytes(&mut output, &input[stream_end..], OUTPUT_ALLOCATION)?;
    }
    if output.len() > input.len() && !options.strict && !options.strip_metadata {
        output.clear();
        try_append_bytes(&mut output, input, OUTPUT_ALLOCATION)?;
        output_deflate_bits = source_deflate_bits;
    }

    Ok(Optimization::from_metrics(
        input.len(),
        output,
        source_deflate_bits,
        output_deflate_bits,
        deadline.is_expired(),
    )
    .with_removed_data(if options.strip_metadata {
        input.len() - stream_end
    } else {
        0
    }))
}

/// Write one member with its optimized Deflate stream; `--strip` drops its
/// optional header fields.
fn write_member(
    input: &[u8],
    member: &Member,
    payload: &[u8],
    options: &Options,
    output: &mut Vec<u8>,
) -> Result<()> {
    let has_optional_metadata = member.flags & (FEXTRA | FNAME | FCOMMENT | FHCRC) != 0;
    if options.strip_metadata && has_optional_metadata {
        let mut header = [0_u8; 10];
        header.copy_from_slice(&input[member.start..member.start + 10]);
        // Keep FTEXT, a content hint; clear only the flags of dropped fields.
        header[3] = member.flags & !(FEXTRA | FNAME | FCOMMENT | FHCRC);
        try_append_bytes(output, &header, OUTPUT_ALLOCATION)?;
    } else {
        try_append_bytes(
            output,
            &input[member.start..member.payload_start],
            OUTPUT_ALLOCATION,
        )?;
    }
    try_append_bytes(output, payload, OUTPUT_ALLOCATION)?;
    try_append_bytes(
        output,
        &input[member.trailer_start..member.end],
        OUTPUT_ALLOCATION,
    )
}

/// Under `--strip`, write the members merged, as [`write_merged_members`]
/// does. Each Max member is no worse than its Default endpoint, but tokens
/// chosen for one member's codes can price worse once its boundaries are
/// re-planned in a merged block. So Max also merges the members' Default
/// endpoints, which every Max member keeps as mandatory work, and keeps that
/// file unless its own is no worse in bytes and Deflate bits. Once Default
/// finishes its members and boundary merges, it writes the same file.
fn write_merged_file(
    input: &[u8],
    members: &[Member],
    results: &[RawOptimization],
    options: &Options,
    deadline: &SearchDeadline,
    output: &mut Vec<u8>,
) -> Result<(u64, MemberMergeProgress)> {
    let start = output.len();
    let streams = member_streams(results, MemberStream::optimized)?;
    let merged = write_merged_members(input, members, &streams, options, deadline, output)?;
    if results.iter().all(|raw| raw.default_endpoint.is_none()) {
        return Ok(merged);
    }
    let streams = member_streams(results, MemberStream::default_endpoint)?;
    let mut floor = try_vec_with_capacity(output.len() - start, OUTPUT_ALLOCATION)?;
    let floor_merged =
        write_merged_members(input, members, &streams, options, deadline, &mut floor)?;
    if output.len() - start <= floor.len() && merged.0 <= floor_merged.0 {
        return Ok(merged);
    }
    output.truncate(start);
    try_append_bytes(output, &floor, OUTPUT_ALLOCATION)?;
    Ok(floor_merged)
}

/// Write runs of consecutive members as one member each. A
/// member's boundary carries no data once its optional fields are dropped,
/// and the joined Deflate stream decodes to the members' output in order and
/// is never longer than their streams, so each removed member saves at least
/// its 10-byte header and 8-byte trailer. A run grows while its joined stream
/// stays within the parser's limits, so that Columbo can check the stream and
/// parse it again; greedy runs leave the fewest members. Returns the output's
/// meaningful Deflate bits and the merge's report.
fn write_merged_members(
    input: &[u8],
    members: &[Member],
    results: &[MemberStream<'_>],
    options: &Options,
    deadline: &SearchDeadline,
    output: &mut Vec<u8>,
) -> Result<(u64, MemberMergeProgress)> {
    let start = output.len();
    let mut bits = 0_u64;
    let mut merged_members = 0_usize;
    let mut run_start = 0_usize;
    let mut joiner = StreamJoiner::default();
    for (index, (member, raw)) in members.iter().zip(results).enumerate() {
        let part = JoinPart::parse(raw.data, member.decoded_size)?;
        if !joiner.is_empty() && !joiner.fits(&part) {
            let run = run_start..index;
            let joined = std::mem::take(&mut joiner);
            bits = add_bits(
                bits,
                write_member_run(
                    input,
                    &members[run.clone()],
                    &results[run],
                    joined,
                    options,
                    deadline,
                    output,
                )?,
            )?;
            merged_members += 1;
            run_start = index;
        }
        joiner.push(part)?;
    }
    let run = run_start..members.len();
    bits = add_bits(
        bits,
        write_member_run(
            input,
            &members[run.clone()],
            &results[run],
            joiner,
            options,
            deadline,
            output,
        )?,
    )?;
    merged_members += 1;

    let separate_bytes = results.iter().try_fold(0_u64, |total, raw| {
        total.checked_add(raw.data.len() as u64 + 18)
    });
    let merged_bytes = (output.len() - start) as u64;
    let merge = MemberMergeProgress {
        members: members.len(),
        merged_members,
        saved_bytes: separate_bytes.map_or(0, |bytes| bytes.saturating_sub(merged_bytes)),
    };
    Ok((bits, merge))
}

/// Write one run of members as one member: the first member's header and
/// the joined stream with its CRC-32 and size. A run of one keeps its own
/// stream.
fn write_member_run(
    input: &[u8],
    members: &[Member],
    results: &[MemberStream<'_>],
    joiner: StreamJoiner<'_>,
    options: &Options,
    deadline: &SearchDeadline,
    output: &mut Vec<u8>,
) -> Result<u64> {
    if let ([member], [raw]) = (members, results) {
        write_member(input, member, raw.data, options, output)?;
        return Ok(raw.deflate_bits);
    }
    // Max finishes the boundary merges after the deadline too, as it does
    // APNG frame reuse, so that it keeps every saving Default finds; Default
    // stops at the deadline.
    let mut expired = || !options.exhaustive && deadline.is_expired();
    let joined = joiner.finish(options, &mut expired)?;
    let first = members[0];
    let mut header = [0_u8; 10];
    header.copy_from_slice(&input[first.start..first.start + 10]);
    // FTEXT says the decoded data is probably text, so the merged member keeps
    // it only when every member set it. MTIME, XFL and OS are the first
    // member's, which is the header gzip itself reads for the whole file.
    header[3] = if members.iter().all(|member| member.flags & FTEXT != 0) {
        FTEXT
    } else {
        0
    };
    try_append_bytes(output, &header, OUTPUT_ALLOCATION)?;
    try_append_bytes(output, &joined.data, OUTPUT_ALLOCATION)?;
    try_append_bytes(output, &joined.crc32.to_le_bytes(), OUTPUT_ALLOCATION)?;
    // ISIZE is the decoded size modulo 2^32.
    try_append_bytes(
        output,
        &(joined.decoded_size as u32).to_le_bytes(),
        OUTPUT_ALLOCATION,
    )?;
    Ok(joined.meaningful_bits)
}

fn member_streams<'a>(
    results: &'a [RawOptimization],
    stream: fn(&'a RawOptimization) -> MemberStream<'a>,
) -> Result<Vec<MemberStream<'a>>> {
    let mut streams = Vec::new();
    streams
        .try_reserve_exact(results.len())
        .map_err(|_| Error::allocation("GZIP member streams"))?;
    streams.extend(results.iter().map(stream));
    Ok(streams)
}

fn add_bits(total: u64, bits: u64) -> Result<u64> {
    total
        .checked_add(bits)
        .ok_or_else(|| Error::new("GZIP Deflate bit count is too large"))
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
        let remaining = &input[member_start..];
        if !members.is_empty() && !has_signature(remaining) {
            // Trailing data containing a member signature (or ending with its
            // first byte) may hide a damaged or partial member. Reject it
            // rather than resynchronizing or removing it with --strip.
            if remaining.windows(2).any(|bytes| bytes == [0x1f, 0x8b])
                || remaining.last() == Some(&0x1f)
            {
                return Err(Error::new("ambiguous GZIP suffix or inter-member data"));
            }
            break;
        }
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
            .map_err(|_| Error::allocation("GZIP member model"))?;
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

/// Optimize one member. Max keeps the member's Default endpoint only when
/// `keep_default_endpoint` asks for it and it differs from Max's stream.
fn optimize_member(
    input: &[u8],
    member: Member,
    options: &Options,
    keep_default_endpoint: bool,
) -> Result<RawOptimization> {
    let default_floor = member_default_floor(options.exhaustive);
    let mut raw = optimize_raw_prefix_with_floor(
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
    settle_default_endpoint(&mut raw, keep_default_endpoint);
    Ok(raw)
}

/// Keep a member's Default endpoint only when `keep` asks for it and it
/// differs from the member's own stream, which then stands in for it.
fn settle_default_endpoint(raw: &mut RawOptimization, keep: bool) {
    if !keep
        || raw
            .default_endpoint
            .as_ref()
            .is_some_and(|endpoint| endpoint.data == raw.data)
    {
        raw.default_endpoint = None;
    }
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
    keep_default_endpoints: bool,
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
            .map_err(|_| Error::allocation("GZIP member schedule"))?;
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
            let run = || {
                optimize_member(
                    input,
                    members[index],
                    &retry_options,
                    keep_default_endpoints,
                )
            };
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
