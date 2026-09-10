// SPDX-License-Identifier: MIT

use std::time::Duration;

use super::*;
use crate::format::test_support::SAME_BYTE_BIT_WIN_RAW;

fn optimize(input: &[u8], options: &Options) -> Result<Optimization> {
    let parsed = preflight(input, options.strip_metadata, options.max_decoded_bytes)?;
    optimize_preflight(input, options, &parsed)
}

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
        uncompressed_size: if method == 8 {
            compressed_size.max(ALWAYS_STORE_MAX_BYTES + 1)
        } else {
            0
        },
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

fn archive_with_reverse_central_order(decoded_entries: &[&[u8]]) -> Vec<u8> {
    assert!(decoded_entries.len() <= u16::MAX as usize);
    let mut output = Vec::new();
    let mut names = Vec::new();
    let mut local_offsets = Vec::new();
    let mut crcs = Vec::new();
    let mut compressed_sizes = Vec::new();

    for (index, &decoded) in decoded_entries.iter().enumerate() {
        let decoded_size = u16::try_from(decoded.len()).unwrap();
        let mut payload = vec![0x01]; // Final stored Deflate block.
        payload.extend_from_slice(&decoded_size.to_le_bytes());
        payload.extend_from_slice(&(!decoded_size).to_le_bytes());
        payload.extend_from_slice(decoded);
        let crc = crc32_update(0, decoded);
        let name = format!("member-{index}").into_bytes();
        local_offsets.push(output.len() as u32);
        output.extend_from_slice(&LOCAL_FILE_HEADER.to_le_bytes());
        output.extend_from_slice(&20_u16.to_le_bytes()); // version needed
        output.extend_from_slice(&0_u16.to_le_bytes()); // flags
        output.extend_from_slice(&8_u16.to_le_bytes()); // Deflate
        output.extend_from_slice(&[0; 4]); // time and date
        output.extend_from_slice(&crc.to_le_bytes());
        output.extend_from_slice(&(payload.len() as u32).to_le_bytes());
        output.extend_from_slice(&(decoded.len() as u32).to_le_bytes());
        output.extend_from_slice(&(name.len() as u16).to_le_bytes());
        output.extend_from_slice(&0_u16.to_le_bytes()); // extra length
        output.extend_from_slice(&name);
        output.extend_from_slice(payload.as_slice());
        names.push(name);
        crcs.push(crc);
        compressed_sizes.push(payload.len() as u32);
    }

    let central_offset = output.len() as u32;
    for index in (0..decoded_entries.len()).rev() {
        let name = &names[index];
        output.extend_from_slice(&CENTRAL_DIRECTORY_HEADER.to_le_bytes());
        output.extend_from_slice(&20_u16.to_le_bytes()); // version made by
        output.extend_from_slice(&20_u16.to_le_bytes()); // version needed
        output.extend_from_slice(&0_u16.to_le_bytes()); // flags
        output.extend_from_slice(&8_u16.to_le_bytes()); // Deflate
        output.extend_from_slice(&[0; 4]); // time and date
        output.extend_from_slice(&crcs[index].to_le_bytes());
        output.extend_from_slice(&compressed_sizes[index].to_le_bytes());
        output.extend_from_slice(&(decoded_entries[index].len() as u32).to_le_bytes());
        output.extend_from_slice(&(name.len() as u16).to_le_bytes());
        output.extend_from_slice(&0_u16.to_le_bytes()); // extra length
        output.extend_from_slice(&0_u16.to_le_bytes()); // comment length
        output.extend_from_slice(&[0; 8]); // disk and attributes
        output.extend_from_slice(&local_offsets[index].to_le_bytes());
        output.extend_from_slice(name);
    }
    let central_size = output.len() as u32 - central_offset;

    output.extend_from_slice(&END_OF_CENTRAL_DIRECTORY.to_le_bytes());
    output.extend_from_slice(&[0; 4]); // disk numbers
    output.extend_from_slice(&(decoded_entries.len() as u16).to_le_bytes());
    output.extend_from_slice(&(decoded_entries.len() as u16).to_le_bytes());
    output.extend_from_slice(&central_size.to_le_bytes());
    output.extend_from_slice(&central_offset.to_le_bytes());
    output.extend_from_slice(&0_u16.to_le_bytes()); // comment length
    output
}

fn uniform_archive_with_reverse_central_order(entry_count: usize) -> Vec<u8> {
    archive_with_reverse_central_order(&vec![b"xxxxxx".as_slice(); entry_count])
}

fn archive_entry_names(input: &[u8]) -> (Vec<Vec<u8>>, Vec<Vec<u8>>) {
    let eocd_offset = find_end_of_central_directory(input).unwrap();
    let eocd = &input[eocd_offset..];
    let central_offset = le32(eocd, 16) as usize;
    let mut entries =
        parse_central_entries(input, central_offset, eocd_offset, le16(eocd, 10), false).unwrap();
    let physical_order = preflight_local_entries(input, central_offset, &mut entries).unwrap();

    let physical = physical_order
        .into_iter()
        .map(|index| {
            let offset = entries[index].local_offset_before;
            let name_length = le16(&input[offset..], 26) as usize;
            input[offset + 30..offset + 30 + name_length].to_vec()
        })
        .collect();
    let central = entries
        .iter()
        .map(|entry| {
            let offset = entry.central_offset;
            let name_length = le16(&input[offset..], 28) as usize;
            input[offset + 46..offset + 46 + name_length].to_vec()
        })
        .collect();
    (physical, central)
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
fn tiny_deflate_streams_are_counted_but_not_optimization_jobs() {
    for decoded_size in 0..=ALWAYS_STORE_MAX_BYTES {
        let mut entry = ordering_entry(8, 8, decoded_size as usize);
        entry.uncompressed_size = decoded_size;
        assert!(entry_is_deflate_stream(&entry));
        assert!(!entry_is_optimizable(&entry));
    }

    let mut entry = ordering_entry(8, 8, 5);
    entry.uncompressed_size = ALWAYS_STORE_MAX_BYTES + 1;
    assert!(entry_is_optimizable(&entry));
}

#[test]
fn parallel_optimization_preserves_local_and_central_order() {
    let input = uniform_archive_with_reverse_central_order(8);
    let source_order = archive_entry_names(&input);
    assert_ne!(source_order.0, source_order.1);

    let result = optimize(
        &input,
        &Options {
            exhaustive: true,
            timeout: Duration::from_millis(200),
            ..Options::default()
        },
    )
    .unwrap();

    assert_eq!(archive_entry_names(&result.data), source_order);
}

#[test]
fn detailed_reporting_keeps_parallel_member_optimization_enabled() {
    let input = uniform_archive_with_reverse_central_order(8);
    let quiet_options = Options {
        exhaustive: true,
        timeout: Duration::from_millis(200),
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
fn detailed_reporting_keeps_bounded_default_floor_parallelism_enabled() {
    let input = archive_with_reverse_central_order(&[b"several", b"a longer independent member"]);
    let parsed = preflight(&input, false, u64::MAX).unwrap();
    assert!(parallel_max_archive_is_bounded(&input, &parsed));

    let quiet_options = Options {
        exhaustive: true,
        timeout: Duration::from_millis(200),
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
fn default_always_stores_a_tiny_deflate_member() {
    // One-byte stored Deflate block containing "x".
    let deflate = [0x01, 0x01, 0x00, 0xfe, 0xff, b'x'];
    let input = single_entry_archive(8, &deflate, crc32_update(0, b"x"), 1, false, false);
    assert_eq!(deflate_stream_count(&input).unwrap(), 1);
    let result = optimize(&input, &Options::default()).unwrap();

    assert_eq!(result.data.len(), input.len() - 5);
    let parsed = preflight(&result.data, false, u64::MAX).unwrap();
    assert_eq!(parsed.entries[0].method, 0);
    assert_eq!(parsed.entries[0].compressed_size_before, 1);
    assert!(result.should_replace());
}

#[test]
fn default_preserves_larger_deflate_members() {
    let decoded = b"abcde";
    let deflate = [0x01, 0x05, 0x00, 0xfa, 0xff, b'a', b'b', b'c', b'd', b'e'];
    let input = single_entry_archive(
        8,
        &deflate,
        crc32_update(0, decoded),
        decoded.len() as u32,
        false,
        false,
    );
    let result = optimize(&input, &Options::default()).unwrap();
    let parsed = preflight(&result.data, false, u64::MAX).unwrap();

    assert_eq!(parsed.entries[0].method, 8);
}

#[test]
fn zero_through_four_byte_members_always_use_store_in_both_modes() {
    for decoded_size in 0..=ALWAYS_STORE_MAX_BYTES as usize {
        let decoded = &b"abcd"[..decoded_size];
        let length = decoded_size as u16;
        let mut deflate = vec![0x01];
        deflate.extend_from_slice(&length.to_le_bytes());
        deflate.extend_from_slice(&(!length).to_le_bytes());
        deflate.extend_from_slice(decoded);
        let input = single_entry_archive(
            8,
            &deflate,
            crc32_update(0, decoded),
            decoded_size as u32,
            false,
            false,
        );

        for exhaustive in [false, true] {
            let result = optimize(
                &input,
                &Options {
                    exhaustive,
                    timeout: Duration::ZERO,
                    ..Options::default()
                },
            )
            .unwrap();
            let parsed = preflight(&result.data, false, u64::MAX).unwrap();
            assert_eq!(parsed.entries[0].method, 0, "size={decoded_size}");
            assert_eq!(
                parsed.entries[0].compressed_size_before, decoded_size as u32,
                "size={decoded_size}"
            );
        }
    }
}

#[test]
fn equal_sized_four_byte_store_normalization_is_recommended() {
    // One literal plus an overlapping three-byte match encodes "AAAA" in
    // 30 meaningful bits and four physical Deflate bytes. Store ties the
    // physical payload size, so the wrapper rule—not a claimed saving—
    // makes this rewrite mandatory.
    let deflate = [0x73, 0x04, 0x02, 0x00];
    let decoded = b"AAAA";
    let input = single_entry_archive(
        8,
        &deflate,
        crc32_update(0, decoded),
        decoded.len() as u32,
        false,
        false,
    );
    let result = optimize(&input, &Options::default()).unwrap();
    let parsed = preflight(&result.data, false, u64::MAX).unwrap();

    assert_eq!(result.data.len(), input.len());
    assert_eq!(result.bits_saved, 0);
    assert!(result.should_replace());
    assert_eq!(parsed.entries[0].method, 0);
}

#[test]
fn terminal_store_keeps_unchanged_records_in_physical_order() {
    let input = archive_with_reverse_central_order(&[b"x", b"unchanged member", b"yz"]);
    let parsed = preflight(&input, false, u64::MAX).unwrap();
    let retained = &parsed.entries[1];
    let retained_local = input
        [retained.local_offset_before..retained.local_offset_before + retained.local_size_before]
        .to_vec();
    let source_bits = parsed
        .entries
        .iter()
        .map(|entry| {
            let layout = local_entry_layout(&input, parsed.central_offset, entry).unwrap();
            decoded_bytes_for_storage(
                &input[layout.data_offset..layout.data_end],
                u64::from(entry.uncompressed_size),
            )
            .unwrap()
            .1
        })
        .sum();
    let optimized = finalize_store_fallback(
        ZipOptimization {
            data: input.clone(),
            source_deflate_bits: source_bits,
            output_deflate_bits: source_bits,
            store_changes: Vec::new(),
            forced_store_change: false,
            timed_out: false,
        },
        &Options::default(),
    )
    .unwrap();
    let rebuilt = preflight(&optimized.data, false, u64::MAX).unwrap();
    assert_eq!(
        archive_entry_names(&optimized.data),
        archive_entry_names(&input)
    );
    assert_eq!(
        rebuilt
            .entries
            .iter()
            .map(|entry| entry.method)
            .collect::<Vec<_>>(),
        [0, 8, 0]
    );
    let retained = &rebuilt.entries[1];
    assert_eq!(
        &optimized.data[retained.local_offset_before
            ..retained.local_offset_before + retained.local_size_before],
        retained_local,
    );

    // The second pass has no eligible Deflate members and returns the
    // existing archive buffer, including its completed Store results.
    let pointer = optimized.data.as_ptr();
    let again = finalize_store_fallback(optimized, &Options::default()).unwrap();
    assert_eq!(again.data.as_ptr(), pointer);
}

#[test]
fn terminal_store_runs_after_a_zero_budget_max_floor() {
    let deflate = [0x01, 0x01, 0x00, 0xfe, 0xff, b'x'];
    let input = single_entry_archive(8, &deflate, crc32_update(0, b"x"), 1, true, false);
    let result = optimize(
        &input,
        &Options {
            exhaustive: true,
            timeout: Duration::ZERO,
            ..Options::default()
        },
    )
    .unwrap();

    assert!(result.timed_out);
    let parsed = preflight(&result.data, false, u64::MAX).unwrap();
    let entry = &parsed.entries[0];
    assert_eq!(entry.method, 0);
    assert_eq!(entry.flags & FLAG_DATA_DESCRIPTOR, 0);
    assert_eq!(entry.compressed_size_before, 1);
    let layout = local_entry_layout(&result.data, parsed.central_offset, entry).unwrap();
    assert_eq!(&result.data[layout.data_offset..layout.data_end], b"x");
}

#[test]
fn terminal_store_does_not_replace_a_smaller_deflate_payload() {
    // Raw Deflate for one hundred "A" bytes. Its match remains much
    // smaller than the one-hundred-byte Store representation.
    let deflate = [0x73, 0x74, 0xa4, 0x3d, 0x00, 0x00];
    let decoded = [b'A'; 100];
    let input = single_entry_archive(
        8,
        &deflate,
        crc32_update(0, &decoded),
        decoded.len() as u32,
        false,
        false,
    );
    let result = optimize(
        &input,
        &Options {
            exhaustive: true,
            timeout: Duration::ZERO,
            ..Options::default()
        },
    )
    .unwrap();
    let parsed = preflight(&result.data, false, u64::MAX).unwrap();

    assert_eq!(parsed.entries[0].method, 8);
    assert!(parsed.entries[0].compressed_size_before < decoded.len() as u32);
}

#[test]
fn store_requires_at_least_ten_percent_of_payload_bytes_saved() {
    assert!(store_saves_at_least_ten_percent(90, 100));
    assert!(store_saves_at_least_ten_percent(9, 10));
    assert!(!store_saves_at_least_ten_percent(91, 100));
    assert!(!store_saves_at_least_ten_percent(10, 10));
    assert!(!store_saves_at_least_ten_percent(10, 9));
}

#[test]
fn max_preserves_deflate_below_the_store_threshold() {
    let decoded: Vec<u8> = (0..46).collect();
    let length = decoded.len() as u16;
    let mut deflate = vec![0x01];
    deflate.extend_from_slice(&length.to_le_bytes());
    deflate.extend_from_slice(&(!length).to_le_bytes());
    deflate.extend_from_slice(&decoded);
    let input = single_entry_archive(
        8,
        &deflate,
        crc32_update(0, &decoded),
        decoded.len() as u32,
        false,
        false,
    );
    let result = optimize(
        &input,
        &Options {
            exhaustive: true,
            timeout: Duration::ZERO,
            ..Options::default()
        },
    )
    .unwrap();
    let parsed = preflight(&result.data, false, u64::MAX).unwrap();

    assert_eq!(parsed.entries[0].method, 8);
}

#[test]
fn deflated_member_reports_same_byte_bit_savings() {
    let decoded = [b'A'; 168];
    let input = single_entry_archive(
        8,
        SAME_BYTE_BIT_WIN_RAW,
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
        0x00, 0x03, 0x00, 0xfc, 0xff, b'a', b'b', b'c', 0x01, 0x03, 0x00, 0xfc, 0xff, b'd', b'e',
        b'f',
    ];
    let input = single_entry_archive(8, &deflate, crc32_update(0, b"abcdef"), 6, false, false);
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
        ZIP_NORMAL_PRODUCER,
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
        ZIP_NORMAL_PRODUCER,
    )
    .unwrap();
    assert!(!deadline.is_expired());
}

#[test]
fn max_retains_the_complete_default_archive() {
    let deflate = [0x01, 0x02, 0x00, 0xfd, 0xff, b'x', b'y'];
    let input = single_entry_archive(8, &deflate, crc32_update(0, b"xy"), 2, false, false);
    let parsed = preflight(&input, false, u64::MAX).unwrap();
    let default = optimize_once(
        &input,
        &Options {
            timeout: Duration::from_secs(1),
            ..Options::default()
        },
        DefaultFloor::Complete,
        ParallelMemberPolicy::UniformWorkOnly,
        ZIP_NORMAL_PRODUCER,
        &parsed,
    )
    .unwrap();
    let max = optimize_max_parallel(
        &input,
        &Options {
            exhaustive: true,
            timeout: Duration::ZERO,
            ..Options::default()
        },
        &parsed,
    )
    .unwrap();

    assert!(max.data.len() <= default.data.len());
    assert!(max.output_deflate_bits <= default.output_deflate_bits);
}

#[test]
fn parallel_max_selection_requires_byte_and_bit_dominance() {
    let floor = ZipOptimization {
        data: vec![0; 10],
        source_deflate_bits: 120,
        output_deflate_bits: 100,
        store_changes: Vec::new(),
        forced_store_change: false,
        timed_out: false,
    };
    let bit_winner = ZipOptimization {
        data: vec![1; 10],
        source_deflate_bits: 120,
        output_deflate_bits: 99,
        store_changes: vec![ZipStoreProgress {
            stream_id: 1,
            deflate_bytes: 10,
            stored_bytes: 8,
        }],
        forced_store_change: false,
        timed_out: true,
    };
    let selected = best_complete_optimization(
        ZipOptimization {
            data: floor.data.clone(),
            source_deflate_bits: floor.source_deflate_bits,
            output_deflate_bits: floor.output_deflate_bits,
            store_changes: floor.store_changes.clone(),
            forced_store_change: floor.forced_store_change,
            timed_out: floor.timed_out,
        },
        bit_winner,
    );
    assert_eq!(selected.data, vec![1; 10]);
    assert_eq!(selected.store_changes.len(), 1);
    assert!(selected.timed_out);

    let byte_only_winner = ZipOptimization {
        data: vec![2; 9],
        source_deflate_bits: 120,
        output_deflate_bits: 101,
        store_changes: Vec::new(),
        forced_store_change: false,
        timed_out: false,
    };
    let selected = best_complete_optimization(floor, byte_only_winner);
    assert_eq!(selected.data, vec![0; 10]);

    let floor = ZipOptimization {
        data: vec![0; 10],
        source_deflate_bits: 120,
        output_deflate_bits: 100,
        store_changes: Vec::new(),
        forced_store_change: false,
        timed_out: false,
    };
    let dominating = ZipOptimization {
        data: vec![2; 9],
        source_deflate_bits: 120,
        output_deflate_bits: 90,
        store_changes: Vec::new(),
        forced_store_change: false,
        timed_out: false,
    };
    let selected = best_complete_optimization(floor, dominating);
    assert_eq!(selected.data, vec![2; 9]);
}

#[test]
fn parallel_max_archive_requires_bounded_work() {
    let deflate = [0x01, 0x05, 0x00, 0xfa, 0xff, b'a', b'b', b'c', b'd', b'e'];
    let input = single_entry_archive(8, &deflate, crc32_update(0, b"abcde"), 5, false, false);
    let parsed = preflight(&input, false, u64::MAX).unwrap();
    assert!(parallel_max_archive_is_bounded(&input, &parsed));

    let oversized = single_entry_archive(
        8,
        &deflate,
        crc32_update(0, b"x"),
        u32::try_from(PARALLEL_MAX_ARCHIVE_DECODED + 1).unwrap(),
        false,
        false,
    );
    let parsed = preflight(&oversized, false, u64::MAX).unwrap();
    assert!(!parallel_max_archive_is_bounded(&oversized, &parsed));
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
fn bounded_default_floor_parallelizes_independent_members() {
    let entries = [ordering_entry(8, 100, 0), ordering_entry(8, 1_000, 1)];
    let expanded_size = entries
        .iter()
        .map(|entry| u64::from(entry.uncompressed_size))
        .sum();

    assert!(!should_parallelize_members(
        2_000,
        expanded_size,
        &entries,
        entries.len(),
        ParallelMemberPolicy::UniformWorkOnly,
    ));
    assert!(should_parallelize_members(
        2_000,
        expanded_size,
        &entries,
        entries.len(),
        ParallelMemberPolicy::AnyIndependentWork,
    ));

    // The broader floor policy never bypasses the retained-input or
    // aggregate-decoded-memory bounds.
    assert!(!should_parallelize_members(
        PARALLEL_MAX_ARCHIVE_INPUT + 1,
        expanded_size,
        &entries,
        entries.len(),
        ParallelMemberPolicy::AnyIndependentWork,
    ));
    assert!(!should_parallelize_members(
        2_000,
        PARALLEL_MEMBER_ARCHIVE_DECODED + 1,
        &entries,
        entries.len(),
        ParallelMemberPolicy::AnyIndependentWork,
    ));
}

#[test]
fn internal_zip_metrics_convert_relative_to_the_original_archive() {
    let shorter = ZipOptimization {
        data: vec![0; 8],
        source_deflate_bits: 100,
        output_deflate_bits: 90,
        store_changes: Vec::new(),
        forced_store_change: false,
        timed_out: false,
    };
    assert_eq!(shorter.into_public(10).bits_saved, 16);

    let equal = ZipOptimization {
        data: vec![0; 10],
        source_deflate_bits: 100,
        output_deflate_bits: 92,
        store_changes: Vec::new(),
        forced_store_change: false,
        timed_out: false,
    };
    assert_eq!(equal.into_public(10).bits_saved, 8);
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
