// SPDX-License-Identifier: MIT
// SPDX-FileCopyrightText: 2026 Andrew C.E. Dent <https://github.com/ace-dent>

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

#[test]
fn suffix_policy_keeps_members_and_strip_merges_them() {
    let mut input = empty_member(0);
    input.extend(same_byte_bit_win_member());
    input.extend_from_slice(b"private suffix");
    for strip_metadata in [false, true] {
        for exhaustive in [false, true] {
            let result = optimize(
                &input,
                &Options {
                    strip_metadata,
                    exhaustive,
                    timeout: Duration::ZERO,
                    ..Options::default()
                },
            )
            .unwrap();
            let members = preflight(&result.data, 168).unwrap();
            assert_eq!(members.len(), if strip_metadata { 1 } else { 2 });
            assert_eq!(members.last().unwrap().decoded_size, 168);
            assert_eq!(
                &result.data[members.last().unwrap().end..],
                if strip_metadata {
                    b"".as_slice()
                } else {
                    b"private suffix"
                }
            );
            assert_eq!(
                result.removed_data_bytes,
                if strip_metadata { 14 } else { 0 }
            );
        }
    }
}

#[test]
fn gzip_member_order_survives_size_based_scheduling_and_merging() {
    // GZIP schedules shorter compressed members first; physical order must
    // still be large, small, medium when the archive is reconstructed, with
    // separate members or, under --strip, in one merged member.
    let expected = [
        b"the largest member payload".as_slice(),
        b"x",
        b"middle member",
    ];
    let mut input = Vec::new();
    for decoded in expected {
        input.extend_from_slice(&[0x1f, 0x8b, 8, 0, 0, 0, 0, 0, 0, 255]);
        input.push(1); // Final stored Deflate block.
        let length = decoded.len() as u16;
        input.extend_from_slice(&length.to_le_bytes());
        input.extend_from_slice(&(!length).to_le_bytes());
        input.extend_from_slice(decoded);
        input.extend_from_slice(&crc32_update(0, decoded).to_le_bytes());
        input.extend_from_slice(&(decoded.len() as u32).to_le_bytes());
    }
    input.extend_from_slice(b"archive suffix");
    for exhaustive in [false, true] {
        let options = Options {
            exhaustive,
            timeout: Duration::from_secs(1),
            ..Options::default()
        };
        let result = optimize(&input, &options).unwrap();
        let members = preflight(&result.data, 1024).unwrap();
        assert_eq!(members.len(), expected.len());
        for (member, decoded) in members.iter().zip(expected) {
            assert!(crate::deflate::raw_stream_decodes_to(
                &result.data[member.payload_start..member.trailer_start],
                decoded.len() as u64,
                decoded,
            ));
        }

        let stripped = optimize(
            &input,
            &Options {
                strip_metadata: true,
                ..options
            },
        )
        .unwrap();
        let members = preflight(&stripped.data, 1024).unwrap();
        assert_eq!(members.len(), 1);
        let merged = expected.concat();
        assert!(crate::deflate::raw_stream_decodes_to(
            &stripped.data[members[0].payload_start..members[0].trailer_start],
            merged.len() as u64,
            &merged,
        ));
        assert_eq!(members[0].end, stripped.data.len());
    }
}

#[test]
fn stripping_does_not_hide_damaged_or_separated_gzip_members() {
    let first = empty_member(0);
    let mut damaged = empty_member(0);
    damaged[12] ^= 1; // Trailer CRC.
    let mut separated = b"gap".to_vec();
    separated.extend(empty_member(0));
    for tail in [vec![0x1f], vec![0x1f, 0x8b], damaged, separated] {
        let mut input = first.clone();
        input.extend(tail);
        for strip_metadata in [false, true] {
            assert!(optimize(
                &input,
                &Options {
                    strip_metadata,
                    ..Options::default()
                }
            )
            .is_err());
        }
    }
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
    let initial = optimize_member(&input, members[0], &slice_options, false).unwrap();
    assert!(initial.timed_out);

    let mut results = Vec::new();
    results.resize_with(members.len(), || None);
    results[0] = Some(initial);
    reclaim_timed_out_members(
        &input,
        &members,
        &mut results,
        vec![0],
        &options,
        &deadline,
        false,
    )
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

fn member_with_header(header: [u8; 10], raw: &[u8], decoded: &[u8]) -> Vec<u8> {
    let mut member = header.to_vec();
    member.extend_from_slice(raw);
    member.extend_from_slice(&crc32_update(0, decoded).to_le_bytes());
    member.extend_from_slice(&(decoded.len() as u32).to_le_bytes());
    member
}

fn stored_raw(decoded: &[u8]) -> Vec<u8> {
    let length = decoded.len() as u16;
    let mut raw = vec![1];
    raw.extend_from_slice(&length.to_le_bytes());
    raw.extend_from_slice(&(!length).to_le_bytes());
    raw.extend_from_slice(decoded);
    raw
}

fn merged_payload(data: &[u8]) -> (Member, Vec<u8>) {
    let members = preflight(data, 1 << 20).unwrap();
    assert_eq!(members.len(), 1);
    let member = members[0];
    let parsed = crate::deflate::inspect_raw_prefix(
        &data[member.payload_start..member.trailer_start],
        member.decoded_size,
    )
    .unwrap();
    assert_eq!(parsed.1.size, member.decoded_size);
    (member, data[member.start..member.start + 10].to_vec())
}

#[test]
fn strip_merges_members_under_the_first_header() {
    let parts: [(&[u8], u8, u8); 3] = [
        (b"first member, marked as text", FTEXT, 3),
        (b"second", FTEXT | FNAME, 11),
        (b"third member", FTEXT, 7),
    ];
    let mut input = Vec::new();
    let mut separate = Vec::new();
    let mut decoded = Vec::new();
    for (index, (text, flags, os)) in parts.iter().enumerate() {
        let mut header = [0x1f, 0x8b, 8, *flags, index as u8 + 1, 0, 0, 0, 2, *os];
        let mut member = header.to_vec();
        if flags & FNAME != 0 {
            member.extend_from_slice(b"name.txt\0");
        }
        member.extend(member_with_header(header, &stored_raw(text), text)[10..].iter());
        input.extend(member);
        header[3] &= FTEXT;
        separate.extend(member_with_header(header, &stored_raw(text), text));
        decoded.extend_from_slice(text);
    }

    for exhaustive in [false, true] {
        for strict in [true, false] {
            let options = Options {
                exhaustive,
                strict,
                timeout: Duration::from_secs(1),
                ..Options::default()
            };
            let unmerged = optimize(&separate, &options).unwrap();
            assert_eq!(preflight(&unmerged.data, 1024).unwrap().len(), 3);
            let merged = optimize(
                &input,
                &Options {
                    strip_metadata: true,
                    ..options
                },
            )
            .unwrap();
            let (member, header) = merged_payload(&merged.data);
            // FTEXT survives because every member set it; MTIME, XFL and OS
            // are the first member's.
            assert_eq!(header, [0x1f, 0x8b, 8, FTEXT, 1, 0, 0, 0, 2, 3]);
            assert_eq!(member.decoded_size, decoded.len() as u64);
            assert!(crate::deflate::raw_stream_decodes_to(
                &merged.data[member.payload_start..member.trailer_start],
                decoded.len() as u64,
                &decoded,
            ));
            assert!(merged.data.len() + 2 * 18 <= unmerged.data.len());
            assert_eq!(merged.removed_data_bytes, 0);
        }
    }

    // One member without FTEXT clears it from the merged header.
    let mut mixed = member_with_header(
        [0x1f, 0x8b, 8, FTEXT, 0, 0, 0, 0, 0, 255],
        &stored_raw(b"a"),
        b"a",
    );
    mixed.extend(member_with_header(
        [0x1f, 0x8b, 8, 0, 0, 0, 0, 0, 0, 255],
        &stored_raw(b"b"),
        b"b",
    ));
    let merged = optimize(
        &mixed,
        &Options {
            strip_metadata: true,
            ..Options::default()
        },
    )
    .unwrap();
    assert_eq!(merged_payload(&merged.data).1[3], 0);
}

#[test]
fn strip_merges_empty_members_into_one_empty_member() {
    let input = empty_member(FNAME).repeat(100);
    let result = optimize(
        &input,
        &Options {
            strip_metadata: true,
            ..Options::default()
        },
    )
    .unwrap();
    let (member, _) = merged_payload(&result.data);
    assert_eq!(member.decoded_size, 0);
    assert_eq!(result.data.len(), 20);
}

/// A raw stream of `count` fixed blocks, each holding the literal 'a'.
fn one_literal_blocks(count: usize) -> Vec<u8> {
    // BFINAL, BTYPE 01, the 8-bit fixed code 10010001 for 'a', then the
    // 7-bit end-of-block code 0000000.
    let mut bits = Vec::with_capacity(count * 18);
    for index in 0..count {
        bits.push(u8::from(index + 1 == count));
        bits.extend_from_slice(&[1, 0, 1, 0, 0, 1, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0]);
    }
    bits.chunks(8)
        .map(|byte| {
            byte.iter()
                .enumerate()
                .fold(0, |value, (bit, &set)| value | (set << bit))
        })
        .collect()
}

#[test]
fn strip_splits_a_merge_where_the_joined_stream_would_exceed_the_parser_limits() {
    // A zero timeout keeps each relaxed member's source stream of 16,000
    // blocks. Together the five members exceed the parser's model limit, so
    // they merge into several members, each a stream Columbo can parse again.
    const BLOCKS: usize = 16_000;
    let decoded = vec![b'a'; BLOCKS];
    let raw = one_literal_blocks(BLOCKS);
    let input =
        member_with_header([0x1f, 0x8b, 8, 0, 0, 0, 0, 0, 0, 255], &raw, &decoded).repeat(5);
    let options = Options {
        strip_metadata: true,
        strict: false,
        timeout: Duration::ZERO,
        ..Options::default()
    };
    let result = optimize(&input, &options).unwrap();
    let members = preflight(&result.data, 1 << 20).unwrap();
    assert!(
        members.len() > 1 && members.len() < 5,
        "{} members",
        members.len()
    );
    assert_eq!(
        members
            .iter()
            .map(|member| member.decoded_size)
            .sum::<u64>(),
        5 * BLOCKS as u64
    );
    assert!(result.data.len() < input.len());
}

#[test]
fn strip_max_is_never_above_the_default_file() {
    // Short appended members, as a rotated log holds: Max's per-member tokens
    // can price worse once the members' blocks merge, so Max also merges the
    // members' Default endpoints and is no larger.
    let lines = [
        b"2026-11-01 GET /index.html 200 user=alice\n".as_slice(),
        b"2026-11-01 POST /api/v1/items 201 user=bob\n",
        b"2026-11-02 GET /api/v1/items 304 user=alice\n",
        b"2026-11-02 GET /index.html 404 user=carol\n",
        b"2026-11-03 PUT /api/v1/items 500 user=bob\n",
        b"2026-11-03 GET /index.html 200 user=alice\n",
    ];
    let mut input = Vec::new();
    for line in lines.iter().cycle().take(18) {
        input.extend(member_with_header(
            [0x1f, 0x8b, 8, 0, 0, 0, 0, 0, 0, 255],
            &stored_raw(line),
            line,
        ));
    }
    let options = Options {
        strip_metadata: true,
        timeout: Duration::from_secs(1),
        ..Options::default()
    };
    let default = optimize(&input, &options).unwrap();
    let max = optimize(
        &input,
        &Options {
            exhaustive: true,
            ..options
        },
    )
    .unwrap();
    assert_eq!(preflight(&max.data, 1 << 20).unwrap().len(), 1);
    assert!(max.data.len() <= default.data.len());
    assert!(max.bits_saved >= default.bits_saved);
}

fn stored_result(decoded: &[u8], default_endpoint: Option<&[u8]>) -> RawOptimization {
    let data = stored_raw(decoded);
    let (consumed, info) = crate::deflate::inspect_raw_prefix(&data, decoded.len() as u64).unwrap();
    let default_endpoint = default_endpoint.map(|endpoint| crate::deflate::DefaultEndpoint {
        data: endpoint.to_vec(),
        deflate_bits: crate::deflate::inspect_raw_prefix(endpoint, decoded.len() as u64)
            .unwrap()
            .1
            .deflate_bits,
    });
    RawOptimization {
        data,
        consumed,
        info,
        output_max_distance: 0,
        timed_out: false,
        default_endpoint,
    }
}

#[test]
fn strip_max_keeps_the_merged_default_endpoints_when_they_are_smaller() {
    let decoded = b"abcabcabcabcabcabc";
    let mut input = Vec::new();
    for _ in 0..2 {
        input.extend(member_with_header(
            [0x1f, 0x8b, 8, 0, 0, 0, 0, 0, 0, 255],
            &stored_raw(decoded),
            decoded,
        ));
    }
    let members = parse_members(&input, 1 << 20).unwrap();
    let options = Options {
        exhaustive: true,
        strip_metadata: true,
        timeout: Duration::from_secs(1),
        ..Options::default()
    };
    let deadline = SearchDeadline::new(&options);
    // Default's endpoint for the member: one fixed block with a match.
    let fixed = optimize_member(
        &input,
        members[0],
        &Options {
            exhaustive: false,
            ..options.clone()
        },
        false,
    )
    .unwrap()
    .data;
    assert!(fixed.len() < stored_raw(decoded).len());

    let merge = |results: &[RawOptimization]| {
        let mut output = Vec::new();
        let (bits, merge) =
            write_merged_file(&input, &members, results, &options, &deadline, &mut output).unwrap();
        assert_eq!(merge.merged_members, 1);
        (output, bits)
    };
    let stored_only = merge(&[stored_result(decoded, None), stored_result(decoded, None)]);
    // One member's endpoint differs, so the endpoints are merged too, and
    // their smaller file is kept.
    let with_endpoint = merge(&[
        stored_result(decoded, Some(&fixed)),
        stored_result(decoded, None),
    ]);
    assert!(with_endpoint.0.len() < stored_only.0.len());
    assert!(with_endpoint.1 < stored_only.1);
    let (member, _) = merged_payload(&with_endpoint.0);
    assert_eq!(member.decoded_size, 2 * decoded.len() as u64);
    // A larger endpoint leaves Max's file as it is.
    let mut larger = stored_raw(decoded);
    larger[0] = 0;
    larger.extend(stored_raw(b""));
    assert_eq!(
        merge(&[
            stored_result(decoded, Some(&larger)),
            stored_result(decoded, None),
        ]),
        stored_only
    );
}

#[test]
fn members_keep_only_a_requested_default_endpoint_that_differs() {
    let decoded = b"abcabcabcabc";
    // The same data as a non-final stored block and an empty final one.
    let mut other = stored_raw(decoded);
    other[0] = 0;
    other.extend(stored_raw(b""));
    for (endpoint, keep, kept) in [
        (other.clone(), true, true),
        (other, false, false),
        (stored_raw(decoded), true, false),
    ] {
        let mut raw = stored_result(decoded, Some(&endpoint));
        settle_default_endpoint(&mut raw, keep);
        assert_eq!(raw.default_endpoint.is_some(), kept);
    }

    // An empty member's Max stream is its Default endpoint.
    let input = empty_member(0);
    let members = parse_members(&input, 1).unwrap();
    let options = Options {
        exhaustive: true,
        timeout: Duration::from_secs(1),
        ..Options::default()
    };
    let raw = optimize_member(&input, members[0], &options, true).unwrap();
    assert!(raw.default_endpoint.is_none());
}
