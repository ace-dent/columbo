// SPDX-License-Identifier: MIT

use crate::checksum::crc32_update;
use crate::deflate::{optimize_raw_prefix_with_floor, DefaultFloor};
use crate::{Error, Optimization, Options, Result};

use super::{try_append_bytes, try_vec_with_capacity, SearchDeadline};

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

pub(super) fn optimize(input: &[u8], options: &Options) -> Result<Optimization> {
    if input.is_empty() {
        return Err(Error::new("invalid GZIP signature"));
    }

    let deadline = SearchDeadline::new(options);
    let mut output = try_vec_with_capacity(input.len(), OUTPUT_ALLOCATION_ERROR)?;
    let mut member_start = 0;
    let mut member_count = 0_usize;
    let mut decoded_remaining = options.max_decoded_bytes;
    let mut timed_out = false;

    // RFC 1952 explicitly permits concatenated members. Each has an
    // independently checksummed Deflate stream, so parse and optimize them in
    // order while sharing one file-wide expansion budget.
    while member_start < input.len() {
        if member_count >= MAX_GZIP_MEMBERS {
            return Err(Error::new("GZIP contains too many members"));
        }
        member_count += 1;
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

        let mut position = member_start + 10;
        let mut has_optional_metadata = false;

        if flags & FEXTRA != 0 {
            has_optional_metadata = true;
            let length = read_u16(input, position, "truncated GZIP extra field")? as usize;
            position += 2;
            position = position
                .checked_add(length)
                .filter(|&end| end <= input.len())
                .ok_or_else(|| Error::new("truncated GZIP extra field"))?;
        }
        if flags & FNAME != 0 {
            has_optional_metadata = true;
            position = skip_zero_terminated(input, position, "truncated GZIP filename")?;
        }
        if flags & FCOMMENT != 0 {
            has_optional_metadata = true;
            position = skip_zero_terminated(input, position, "truncated GZIP comment")?;
        }
        if flags & FHCRC != 0 {
            has_optional_metadata = true;
            let stored = read_u16(input, position, "truncated GZIP header CRC")?;
            let calculated = crc32_update(0, &input[member_start..position]) as u16;
            if stored != calculated {
                return Err(Error::new("GZIP header CRC mismatch"));
            }
            position += 2;
        }

        let payload_start = position;
        let call_options = deadline.options_for_call(options);
        let raw = optimize_raw_prefix_with_floor(
            &input[payload_start..],
            &call_options,
            decoded_remaining,
            DefaultFloor::Shared,
        )?;
        if raw.info.size > decoded_remaining {
            return Err(Error::new(
                "decoded GZIP data exceeds configured safety limit",
            ));
        }
        decoded_remaining -= raw.info.size;

        let trailer_start = payload_start
            .checked_add(raw.consumed)
            .filter(|&start| raw.consumed != 0 && input.len().saturating_sub(start) >= 8)
            .ok_or_else(|| Error::new("missing GZIP trailer"))?;
        let stored_crc =
            u32::from_le_bytes(input[trailer_start..trailer_start + 4].try_into().unwrap());
        let stored_size = u32::from_le_bytes(
            input[trailer_start + 4..trailer_start + 8]
                .try_into()
                .unwrap(),
        );
        if stored_crc != raw.info.crc32 || stored_size != raw.info.size as u32 {
            return Err(Error::new("GZIP trailer CRC or size mismatch"));
        }

        if options.strip_metadata && has_optional_metadata {
            let mut header = input[member_start..member_start + 10].to_vec();
            // FTEXT is a content hint rather than an optional variable-length
            // field. Keep it, and remove only fields whose bytes were dropped.
            header[3] = flags & !(FEXTRA | FNAME | FCOMMENT | FHCRC);
            try_append_bytes(&mut output, &header, OUTPUT_ALLOCATION_ERROR)?;
        } else {
            try_append_bytes(
                &mut output,
                &input[member_start..payload_start],
                OUTPUT_ALLOCATION_ERROR,
            )?;
        }
        try_append_bytes(&mut output, &raw.data, OUTPUT_ALLOCATION_ERROR)?;
        try_append_bytes(
            &mut output,
            &input[trailer_start..trailer_start + 8],
            OUTPUT_ALLOCATION_ERROR,
        )?;

        timed_out |= raw.timed_out;
        member_start = trailer_start + 8;
    }

    if output.len() > input.len() && !options.strict {
        output.clear();
        try_append_bytes(&mut output, input, OUTPUT_ALLOCATION_ERROR)?;
    }

    Ok(Optimization {
        data: output,
        timed_out,
    })
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
mod tests {
    use std::time::Duration;

    use super::*;

    fn empty_member(flags: u8) -> Vec<u8> {
        let mut member = vec![0x1f, 0x8b, 8, flags, 0, 0, 0, 0, 0, 255];
        if flags & FEXTRA != 0 {
            member.extend_from_slice(&2_u16.to_le_bytes());
            member.extend_from_slice(&[0xde, 0xad]);
        }
        if flags & FNAME != 0 {
            member.extend_from_slice(b"empty.txt\0");
        }
        if flags & FCOMMENT != 0 {
            member.extend_from_slice(b"fixture\0");
        }
        if flags & FHCRC != 0 {
            let crc = crc32_update(0, &member) as u16;
            member.extend_from_slice(&crc.to_le_bytes());
        }
        member.extend_from_slice(&[0x03, 0x00]); // Empty fixed Deflate stream.
        member.extend_from_slice(&0_u32.to_le_bytes()); // CRC-32("").
        member.extend_from_slice(&0_u32.to_le_bytes()); // ISIZE.
        member
    }

    #[test]
    fn rejects_reserved_flags_before_decoding() {
        let mut input = vec![0x1f, 0x8b, 8, 0x01 | 0x20];
        input.resize(18, 0);
        let error = optimize(&input, &Options::default()).unwrap_err();
        assert_eq!(error.message(), "reserved GZIP flags are set");
    }

    #[test]
    fn rejects_an_empty_gzip_file() {
        let error = optimize(&[], &Options::default()).unwrap_err();
        assert_eq!(error.message(), "invalid GZIP signature");
    }

    #[test]
    fn reports_truncated_optional_filename() {
        let mut input = vec![0x1f, 0x8b, 8, FNAME];
        input.resize(18, b'a');
        let error = optimize(&input, &Options::default()).unwrap_err();
        assert_eq!(error.message(), "truncated GZIP filename");
    }

    #[test]
    fn optimizes_concatenated_members() {
        let mut input = empty_member(0);
        input.extend(empty_member(0));
        let result = optimize(&input, &Options::default()).unwrap();
        assert_eq!(result.data, input);
    }

    #[test]
    fn concatenated_max_members_respect_the_shared_deadline() {
        let mut input = empty_member(0);
        input.extend(empty_member(0));
        let options = Options {
            exhaustive: true,
            timeout: Duration::ZERO,
            ..Options::default()
        };

        let result = optimize(&input, &options).unwrap();
        assert!(result.timed_out);
        assert_eq!(result.data, input);
    }

    #[test]
    fn rejects_pathological_empty_member_counts() {
        let member = empty_member(0);
        let input = member.repeat(MAX_GZIP_MEMBERS + 1);
        let options = Options {
            timeout: Duration::ZERO,
            ..Options::default()
        };

        let error = optimize(&input, &options).unwrap_err();
        assert_eq!(error.message(), "GZIP contains too many members");
    }

    #[test]
    fn strip_removes_optional_header_fields_but_keeps_ftext() {
        let input = empty_member(0x01 | FEXTRA | FNAME | FCOMMENT | FHCRC);
        let options = Options {
            strip_metadata: true,
            ..Options::default()
        };
        let result = optimize(&input, &options).unwrap();

        assert_eq!(result.data.len(), 20);
        assert_eq!(result.data[3], 0x01);
        // The stripped stream remains a valid member on a second pass.
        optimize(&result.data, &Options::default()).unwrap();
    }
}
