// SPDX-License-Identifier: MIT

use std::time::Duration;

use super::*;
use crate::format::test_support::SAME_BYTE_BIT_WIN_RAW;

fn optimize(input: &[u8], options: &Options) -> Result<Optimization> {
    let members = preflight(input, options.max_decoded_bytes)?;
    optimize_preflight(input, options, members)
}

fn same_byte_bit_win_member() -> Vec<u8> {
    let decoded = [b'A'; 168];
    let mut member = vec![0x1f, 0x8b, 8, 0, 0, 0, 0, 0, 0, 255];
    member.extend_from_slice(SAME_BYTE_BIT_WIN_RAW);
    member.extend_from_slice(&crc32_update(0, &decoded).to_le_bytes());
    member.extend_from_slice(&(decoded.len() as u32).to_le_bytes());
    member
}

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

fn feedback_member() -> Vec<u8> {
    let raw = [
        0x25, 0xc0, 0x01, 0x01, 0xc0, 0x30, 0x0c, 0xc3, 0x30, 0x6c, 0xb5, 0x9b, 0xf0, 0x87, 0xf4,
        0x7d, 0xd3, 0xcc, 0xcc, 0xcc, 0xcc, 0x01, 0x00, 0x00, 0xc0, 0x71, 0x5d, 0xaa, 0xaa, 0xaa,
        0xfe, 0x76, 0x77, 0x93, 0x24, 0x49, 0x9e, 0xa7, 0x6d, 0xdb, 0xf6, 0x03,
    ];
    let (_, info) = inspect_raw_prefix(&raw, 86).unwrap();
    let mut member = vec![0x1f, 0x8b, 8, 0, 0, 0, 0, 0, 0, 255];
    member.extend_from_slice(&raw);
    member.extend_from_slice(&info.crc32.to_le_bytes());
    member.extend_from_slice(&(info.size as u32).to_le_bytes());
    member
}

fn aggregate_deflate_bits(input: &[u8]) -> u64 {
    parse_members(input, u64::MAX)
        .unwrap()
        .into_iter()
        .map(|member| {
            inspect_raw_prefix(&input[member.payload_start..], member.decoded_size)
                .unwrap()
                .1
                .source_deflate_bits
        })
        .sum()
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
    assert_eq!(deflate_stream_count(&input, 1).unwrap(), 2);
    let result = optimize(&input, &Options::default()).unwrap();
    assert_eq!(result.data, input);
    assert!(!result.timed_out);
}

#[test]
fn max_members_use_an_exact_shared_default_floor() {
    assert_eq!(member_default_floor(false), DefaultFloor::Shared);
    assert_eq!(member_default_floor(true), DefaultFloor::SharedExact);
}

#[test]
fn zero_budget_gzip_max_retains_default_in_bytes_and_bits() {
    let input = feedback_member();
    for strict in [true, false] {
        let default_options = Options {
            strict,
            timeout: Duration::from_secs(1),
            ..Options::default()
        };
        let default = optimize(&input, &default_options).unwrap();
        let maximum = optimize(
            &input,
            &Options {
                exhaustive: true,
                timeout: Duration::ZERO,
                ..default_options
            },
        )
        .unwrap();

        assert!(maximum.data.len() <= default.data.len(), "strict={strict}");
        assert!(
            aggregate_deflate_bits(&maximum.data) <= aggregate_deflate_bits(&default.data),
            "strict={strict}"
        );
    }
}

#[test]
fn concatenated_members_aggregate_same_byte_bit_savings() {
    let member = same_byte_bit_win_member();
    let mut input = member.clone();
    input.extend_from_slice(&member);
    let optimized = optimize(&input, &Options::default()).unwrap();

    assert_eq!(optimized.data.len(), input.len());
    assert_eq!(optimized.bits_saved, 2);
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
fn concatenated_member_slice_is_reclaimed_without_a_file_timeout() {
    let mut input = empty_member(0);
    input.extend(empty_member(0));
    let members = parse_members(&input, 1).unwrap();
    let options = Options {
        timeout: Duration::from_secs(1),
        ..Options::default()
    };
    let deadline = SearchDeadline::new(&options);
    let mut slice_options = options.clone();
    slice_options.timeout = Duration::ZERO;
    let initial = optimize_member(&input, members[0], &slice_options).unwrap();
    assert!(initial.timed_out);

    let mut results = Vec::new();
    results.resize_with(members.len(), || None);
    results[0] = Some(initial);
    reclaim_timed_out_members(&input, &members, &mut results, vec![0], &options, &deadline)
        .unwrap();

    assert!(!results[0].as_ref().unwrap().timed_out);
    assert!(!deadline.is_expired());
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
