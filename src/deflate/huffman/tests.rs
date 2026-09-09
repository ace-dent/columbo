// SPDX-License-Identifier: MIT

use super::*;
use crate::deflate::bitstream::BitWriter;

fn merge_generic_nodes_scanning(nodes: &mut Vec<Node>, active: &mut Vec<usize>, variant: u32) {
    while active.len() > 1 {
        let first_position = (1..active.len()).fold(0, |best, position| {
            if node_less(nodes, active[position], active[best], variant) {
                position
            } else {
                best
            }
        });
        let left = active.swap_remove(first_position);

        // Variant bit 1 reverses only the second equal-frequency choice.
        let second_variant = if variant & 2 != 0 {
            variant ^ 1
        } else {
            variant
        };
        let second_position = (1..active.len()).fold(0, |best, position| {
            if node_less(nodes, active[position], active[best], second_variant) {
                position
            } else {
                best
            }
        });
        let right = active.swap_remove(second_position);

        let index = nodes.len();
        let frequency = nodes[left].frequency.wrapping_add(nodes[right].frequency);
        nodes.push(Node::branch(frequency, left, right, index));
        active.push(index);
    }
}

fn unconstrained_huffman_max_depth(frequencies: &[u32]) -> usize {
    let mut scratch = vec![0; frequencies.len()];
    let mut nodes = Vec::with_capacity(frequencies.len().saturating_mul(2));
    let mut active = Vec::with_capacity(frequencies.len());

    for (symbol, &frequency) in frequencies.iter().enumerate() {
        if frequency != 0 {
            active.push(nodes.len());
            nodes.push(Node::leaf(frequency, symbol, symbol));
        }
    }
    if active.len() < 2 {
        return active.len();
    }

    while active.len() > 1 {
        let first_position = (1..active.len()).fold(0, |best, position| {
            if node_less(&nodes, active[position], active[best], 0) {
                position
            } else {
                best
            }
        });
        let left = active.swap_remove(first_position);
        let second_position = (1..active.len()).fold(0, |best, position| {
            if node_less(&nodes, active[position], active[best], 0) {
                position
            } else {
                best
            }
        });
        let right = active.swap_remove(second_position);
        let index = nodes.len();
        nodes.push(Node::branch(
            nodes[left].frequency.wrapping_add(nodes[right].frequency),
            left,
            right,
            index,
        ));
        active.push(index);
    }
    assign_depths(&nodes, active[0], 0, &mut scratch)
}

fn tree_exceeds_limit(frequencies: &[u32], max_bits: u8) -> bool {
    unconstrained_huffman_max_depth(frequencies) > usize::from(max_bits)
}

/// Build Columbo's legacy order-key heap extension.
///
/// This intentionally is not labelled as DeflOpt parity: DeflOpt 2.07 breaks
/// frequency ties by subtree height, whereas the original Columbo C
/// implementation retained this earlier order-key interpretation as an
/// additive candidate.
fn make_lengths_order_heap(frequencies: &[u32], max_bits: u8, variant: u32) -> Vec<u8> {
    let mut lengths = vec![0; frequencies.len()];
    make_lengths_order_heap_into(frequencies, &mut lengths, max_bits, variant);
    lengths
}

fn make_lengths_order_heap_into(
    frequencies: &[u32],
    lengths: &mut [u8],
    max_bits: u8,
    variant: u32,
) {
    make_lengths_variant_heap_inner(
        frequencies,
        lengths,
        max_bits,
        variant,
        true,
        &mut DefloptHeapScratch::default(),
    );
}

fn make_lengths_scanning_reference(frequencies: &[u32], max_bits: u8, variant: u32) -> Vec<u8> {
    let mut lengths = vec![0; frequencies.len()];
    let mut nodes = Vec::with_capacity(frequencies.len().saturating_mul(2));
    let mut active = Vec::with_capacity(frequencies.len());
    for (symbol, &frequency) in frequencies.iter().enumerate() {
        if frequency != 0 {
            active.push(nodes.len());
            nodes.push(Node::leaf(frequency, symbol, symbol));
        }
    }
    match active.len() {
        0 => return lengths,
        1 => {
            lengths[nodes[active[0]].symbol.expect("leaf has a symbol")] = 1;
            return lengths;
        }
        _ => {}
    }

    merge_generic_nodes_scanning(&mut nodes, &mut active, variant);
    let max_depth = assign_depths(&nodes, active[0], 0, &mut lengths);
    if max_depth > usize::from(max_bits) {
        limit_generic_lengths(frequencies, &mut lengths, max_bits);
    }
    lengths
}

fn assert_bounded_lengths(frequencies: &[u32], lengths: &[u8], max_bits: u8) {
    assert_eq!(frequencies.len(), lengths.len());
    assert!(lengths.iter().all(|&length| length <= max_bits));
    for (&frequency, &length) in frequencies.iter().zip(lengths) {
        if frequency == 0 {
            assert_eq!(length, 0);
        } else {
            assert_ne!(length, 0);
        }
    }
}

fn swapping_heap_sift_down(nodes: &[Node], heap: &mut [usize], start: usize, tie: HeapTie) {
    let mut root = start;
    loop {
        let mut child = root * 2 + 1;
        if child >= heap.len() {
            return;
        }
        if child + 1 < heap.len() && heap_node_less(nodes, heap[child + 1], heap[child], tie) {
            child += 1;
        }
        if !heap_node_less(nodes, heap[child], heap[root], tie) {
            return;
        }
        heap.swap(root, child);
        root = child;
    }
}

#[test]
fn hole_heap_sift_matches_the_swapping_reference() {
    let mut nodes = Vec::new();
    for index in 0..64 {
        let mut node = Node::leaf(((index * 17 + 5) % 13) as u32, index, 63 - index);
        node.height = (index * 7 + 3) % 11;
        nodes.push(node);
    }

    for len in 1..=nodes.len() {
        for seed in 0..4_u64 {
            let mut heap = (0..len).collect::<Vec<_>>();
            let mut random = seed + len as u64;
            for index in (1..len).rev() {
                random ^= random << 13;
                random ^= random >> 7;
                random ^= random << 17;
                heap.swap(index, random as usize % (index + 1));
            }
            for start in 0..len {
                for tie in [HeapTie::FrequencyOnly, HeapTie::Height, HeapTie::Order] {
                    let mut expected = heap.clone();
                    let mut actual = heap.clone();
                    swapping_heap_sift_down(&nodes, &mut expected, start, tie);
                    heap_sift_down(&nodes, &mut actual, start, tie);
                    assert_eq!(actual, expected);
                }
            }
        }
    }
}

#[test]
fn canonical_codes_round_trip_in_deflate_bit_order() {
    let tree = Huffman::build(&[1, 2, 2]).unwrap();
    assert_eq!(
        tree.code(0),
        Some(HuffCode {
            symbol: 0,
            length: 1,
            code: 0
        })
    );
    assert_eq!(tree.code(1).unwrap().code, 0b01);
    assert_eq!(tree.code(2).unwrap().code, 0b11);

    let mut writer = BitWriter::default();
    for symbol in [2, 0, 1] {
        let entry = tree.code(symbol).unwrap();
        writer.write(u32::from(entry.code), entry.length).unwrap();
    }
    let encoded = writer.into_bytes();
    let mut reader = BitReader::new(&encoded);
    assert_eq!(tree.decode(&mut reader).unwrap(), 2);
    assert_eq!(tree.decode(&mut reader).unwrap(), 0);
    assert_eq!(tree.decode(&mut reader).unwrap(), 1);
}

#[test]
fn table_decoder_matches_canonical_ranges_and_subtables() {
    let deep_lengths = [1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 15];
    for lengths in [&FIXED_LITERAL_CODE_LENGTHS[..], &deep_lengths[..]] {
        let canonical = Huffman::build(lengths).unwrap();
        let table = Huffman::build_decoder(lengths).unwrap();
        let symbols: Vec<_> = lengths
            .iter()
            .enumerate()
            .filter_map(|(symbol, &length)| (length != 0).then_some(symbol))
            .cycle()
            .take(lengths.len() * 3)
            .collect();
        let mut writer = BitWriter::default();
        for &symbol in &symbols {
            let code = canonical.code(symbol).unwrap();
            writer.write(u32::from(code.code), code.length).unwrap();
        }
        let encoded = writer.into_bytes();
        let mut canonical_reader = BitReader::new(&encoded);
        let mut table_reader = BitReader::new(&encoded);
        for symbol in symbols {
            assert_eq!(
                canonical.decode(&mut canonical_reader).unwrap(),
                symbol as u16
            );
            assert_eq!(table.decode(&mut table_reader).unwrap(), symbol as u16);
            assert_eq!(canonical_reader.bit_position(), table_reader.bit_position());
        }
    }
}

#[test]
fn compact_code_length_decoder_matches_canonical_table() {
    let mut lengths = [0_u8; 19];
    lengths[..8].copy_from_slice(&[2, 3, 3, 3, 3, 3, 4, 4]);
    let canonical = Huffman::build(&lengths).unwrap();
    let compact = CodeLengthDecoder::build(&lengths).unwrap();
    let symbols = [7, 0, 3, 6, 5, 2, 1, 4, 0, 7];
    let mut writer = BitWriter::default();
    for symbol in symbols {
        let code = canonical.code(symbol).unwrap();
        writer.write(u32::from(code.code), code.length).unwrap();
    }
    let encoded = writer.into_bytes();
    let mut canonical_reader = BitReader::new(&encoded);
    let mut compact_reader = BitReader::new(&encoded);
    for symbol in symbols {
        assert_eq!(
            canonical.decode(&mut canonical_reader).unwrap(),
            symbol as u16
        );
        assert_eq!(compact.decode(&mut compact_reader).unwrap(), symbol as u16);
        assert_eq!(
            canonical_reader.bit_position(),
            compact_reader.bit_position()
        );
    }
}

#[test]
fn compact_code_length_decoder_rejects_invalid_shapes() {
    assert!(CodeLengthDecoder::build(&[0; 19]).is_none());
    let mut incomplete = [0_u8; 19];
    incomplete[0] = 1;
    assert!(CodeLengthDecoder::build(&incomplete).is_none());
    let mut overlong = [0_u8; 19];
    overlong[0] = 8;
    assert!(CodeLengthDecoder::build(&overlong).is_none());
}

#[test]
fn compact_code_length_decoder_accepts_a_short_code_at_physical_end() {
    let mut lengths = [0_u8; 19];
    lengths[0] = 1;
    lengths[1] = 1;
    let decoder = CodeLengthDecoder::build(&lengths).unwrap();
    let mut reader = BitReader::new(&[0]);
    reader.read(7).unwrap();
    assert_eq!(decoder.decode(&mut reader).unwrap(), 0);
    assert_eq!(reader.bit_position(), 8);
}

#[test]
fn table_widths_match_canonical_decoding_on_generated_trees() {
    let mut state = 0x34c7_91ed_u32;
    for sample in 0..64 {
        let mut frequencies = [0_u32; 286];
        for frequency in &mut frequencies {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            *frequency = if state % 9 == 0 {
                0
            } else {
                state % 65_521 + 1
            };
        }
        let lengths = make_lengths(&frequencies, 15, sample % 4);
        let canonical = Huffman::build(&lengths).unwrap();
        let symbols: Vec<_> = lengths
            .iter()
            .enumerate()
            .filter_map(|(symbol, &length)| (length != 0).then_some(symbol))
            .cycle()
            .take(1_024)
            .collect();
        let mut writer = BitWriter::default();
        for &symbol in &symbols {
            let code = canonical.code(symbol).unwrap();
            writer.write(u32::from(code.code), code.length).unwrap();
        }
        let encoded = writer.into_bytes();

        for root_bits in 7..=11 {
            let decoder = Huffman::build_decoder_with_root_bits(&lengths, root_bits).unwrap();
            let mut reader = BitReader::new(&encoded);
            for &symbol in &symbols {
                assert_eq!(decoder.decode(&mut reader).unwrap(), symbol as u16);
            }
        }
    }
}

#[test]
fn profiled_decoder_combines_codewords_with_extra_fields() {
    static BASES: [u16; 2] = [10, 20];
    static EXTRA_BITS: [u8; 2] = [1, 2];

    let lengths = [2, 2, 2, 2];
    let encoder = Huffman::build(&lengths).unwrap();
    let decoder = Huffman::build_value_decoder_with_root_bits(
        &lengths,
        2,
        &BASES,
        &EXTRA_BITS,
        DEFAULT_DECODE_ROOT_BITS,
    )
    .unwrap();
    let mut writer = BitWriter::default();
    for (symbol, extra, width) in [(0, 0, 0), (2, 1, 1), (3, 3, 2)] {
        let code = encoder.code(symbol).unwrap();
        writer.write(u32::from(code.code), code.length).unwrap();
        writer.write(extra, width).unwrap();
    }

    let encoded = writer.into_bytes();
    let mut reader = BitReader::new(&encoded);
    assert_eq!(
        decoder.decode_value(&mut reader).unwrap(),
        DecodedValue {
            symbol: 0,
            value: 0,
            extra: 0,
            extra_bits: 0,
        }
    );
    assert_eq!(
        decoder.decode_value(&mut reader).unwrap(),
        DecodedValue {
            symbol: 2,
            value: 11,
            extra: 1,
            extra_bits: 1,
        }
    );
    assert_eq!(
        decoder.decode_value(&mut reader).unwrap(),
        DecodedValue {
            symbol: 3,
            value: 23,
            extra: 3,
            extra_bits: 2,
        }
    );
    assert_eq!(reader.bit_position(), 9);
}

#[test]
fn decode_entry_fits_one_u64() {
    assert_eq!(std::mem::size_of::<DecodeEntry>(), 8);
}

#[test]
fn canonical_builder_rejects_malformed_trees() {
    assert!(Huffman::build(&[]).is_none());
    assert!(Huffman::build(&[16]).is_none());
    assert!(Huffman::build(&[1, 1, 1]).is_none());
    assert!(Huffman::build(&[0, 0]).is_some());
}

#[test]
fn allocation_free_validator_matches_canonical_builder() {
    let mut state = 0x9e37_79b9_u32;
    for length in 0..=321 {
        let mut lengths = Vec::with_capacity(length);
        for _ in 0..length {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            lengths.push((state % 18) as u8);
        }
        assert_eq!(
            huffman_code_lengths_are_valid(&lengths),
            Huffman::build(&lengths).is_some()
        );
    }
    for lengths in [
        &FIXED_LITERAL_CODE_LENGTHS[..],
        &FIXED_DISTANCE_CODE_LENGTHS[..],
        &[1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 15][..],
    ] {
        assert!(huffman_code_lengths_are_valid(lengths));
        assert!(Huffman::build(lengths).is_some());
    }
}

#[test]
fn fixed_trees_have_rfc_1951_lengths() {
    // Keep the RFC ranges explicit: comparing a tree only with the table
    // that built it would not detect an incorrectly defined table.
    assert!(FIXED_LITERAL_CODE_LENGTHS[..144]
        .iter()
        .all(|&length| length == 8));
    assert!(FIXED_LITERAL_CODE_LENGTHS[144..256]
        .iter()
        .all(|&length| length == 9));
    assert!(FIXED_LITERAL_CODE_LENGTHS[256..280]
        .iter()
        .all(|&length| length == 7));
    assert!(FIXED_LITERAL_CODE_LENGTHS[280..]
        .iter()
        .all(|&length| length == 8));
    assert!(FIXED_DISTANCE_CODE_LENGTHS
        .iter()
        .all(|&length| length == 5));

    let (literal, distance) = fixed_trees();
    assert_eq!(literal.max_bits(), 9);
    assert_eq!(distance.max_bits(), 5);
    for (symbol, &length) in FIXED_LITERAL_CODE_LENGTHS.iter().enumerate() {
        assert_eq!(literal.code(symbol).unwrap().length, length);
    }
    for (symbol, &length) in FIXED_DISTANCE_CODE_LENGTHS.iter().enumerate() {
        assert_eq!(distance.code(symbol).unwrap().length, length);
    }
}

#[test]
fn builders_preserve_inactive_symbols_and_depth_limit() {
    // Fibonacci-like weights force an over-depth ordinary tree at 4 bits.
    let frequencies = [1, 1, 2, 3, 5, 8, 13, 0];
    assert!(tree_exceeds_limit(&frequencies, 4));

    let defluff = make_lengths_defluff_exact(&frequencies, 4, 0);
    let candidates = [
        make_lengths(&frequencies, 4, 0),
        make_lengths_columbo_defluff_limited(&frequencies, 4, 0),
        defluff.clone(),
        make_lengths_deflopt_heap(&frequencies, 4, 0),
        make_lengths_order_heap(&frequencies, 4, 0),
        make_lengths_deft4j_java_heap(&frequencies, 4),
        make_lengths_zopfli_package_from(&frequencies, &defluff, 4),
    ];
    for lengths in candidates {
        assert_bounded_lengths(&frequencies, &lengths, 4);
    }
}

#[test]
fn zopfli_package_tie_is_distinct_without_changing_payload_cost() {
    let mut state = 0x6d2b_79f5_u32;
    for _ in 0..4096 {
        let mut frequencies = [0_u32; 19];
        for frequency in &mut frequencies {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            *frequency = state % 17;
        }
        if frequencies
            .iter()
            .filter(|&&frequency| frequency != 0)
            .count()
            < 2
        {
            continue;
        }

        let defluff = make_lengths_defluff_exact(&frequencies, 7, 0);
        let zopfli = make_lengths_zopfli_package_from(&frequencies, &defluff, 7);
        assert_bounded_lengths(&frequencies, &zopfli, 7);
        if defluff == zopfli {
            continue;
        }

        let payload_cost = |lengths: &[u8]| {
            frequencies
                .iter()
                .zip(lengths)
                .map(|(&frequency, &length)| u64::from(frequency) * u64::from(length))
                .sum::<u64>()
        };
        assert_eq!(payload_cost(&zopfli), payload_cost(&defluff));
        return;
    }
    panic!("deterministic tie corpus did not expose a distinct package tree");
}

#[test]
fn length_families_match_original_columbo_c_on_overflow() {
    let frequencies = [1, 1, 2, 3, 5, 8, 13, 0];

    // These vectors come directly from the original Columbo `huffman.c`.
    // In particular, the generic builder intentionally preserves its
    // odd-overflow result even though a complete bounded tree is available.
    assert_eq!(make_lengths(&frequencies, 4, 0), [4, 4, 4, 4, 4, 4, 4, 0]);
    assert_eq!(
        make_lengths_columbo_defluff_limited(&frequencies, 4, 0),
        [4, 4, 3, 3, 3, 2, 2, 0]
    );
    assert_eq!(
        make_lengths_defluff_exact(&frequencies, 4, 0),
        [4, 4, 3, 3, 3, 2, 2, 0]
    );
    assert_eq!(
        make_lengths_deflopt_heap(&frequencies, 4, 0),
        [4, 4, 4, 4, 3, 3, 1, 0]
    );
    assert_eq!(
        make_lengths_order_heap(&frequencies, 4, 0),
        [4, 4, 4, 4, 3, 3, 1, 0]
    );
    assert_eq!(
        make_lengths_deft4j_java_heap(&frequencies, 4),
        [4, 4, 3, 4, 4, 3, 1, 0]
    );
}

#[test]
fn variants_match_original_columbo_c_equal_frequency_ties() {
    let frequencies = [4, 1, 9, 2, 2, 0];
    assert_eq!(make_lengths(&frequencies, 4, 0), [2, 4, 1, 4, 3, 0]);
    assert_eq!(make_lengths(&frequencies, 4, 1), [2, 4, 1, 3, 4, 0]);
    assert_eq!(
        make_lengths_order_heap(&frequencies, 4, 0),
        [2, 4, 1, 4, 3, 0]
    );
    assert_eq!(
        make_lengths_order_heap(&frequencies, 4, 2),
        [2, 4, 1, 3, 4, 0]
    );
}

#[test]
fn optimized_generic_variants_match_the_scanning_topology() {
    let mut state = 0x7f4a_7c15_u32;
    for alphabet_len in [19, 30, 286] {
        for sample in 0..128 {
            let mut frequencies = Vec::with_capacity(alphabet_len);
            for symbol in 0..alphabet_len {
                state ^= state << 13;
                state ^= state >> 17;
                state ^= state << 5;
                let frequency = if (state ^ symbol as u32 ^ sample) % 7 == 0 {
                    0
                } else {
                    state % 65_521 + 1
                };
                frequencies.push(frequency);
            }
            for variant in 0..4 {
                let max_bits = if alphabet_len == 19 { 7 } else { 15 };
                assert_eq!(
                    make_lengths(&frequencies, max_bits, variant),
                    make_lengths_scanning_reference(&frequencies, max_bits, variant),
                    "alphabet={alphabet_len} sample={sample} variant={variant}",
                );
            }
        }
    }

    for frequencies in [
        vec![u32::MAX, u32::MAX - 1, 7, 5, 3, 1],
        vec![u32::MAX / 2 + 1; 19],
    ] {
        for variant in 0..4 {
            assert_eq!(
                make_lengths(&frequencies, 15, variant),
                make_lengths_scanning_reference(&frequencies, 15, variant),
                "wrapped-frequency fallback variant={variant}",
            );
        }
    }
}

#[test]
fn deft4j_adds_a_dummy_leaf_for_a_single_symbol() {
    let frequencies = [0, 7, 0, 0];
    assert_eq!(make_lengths(&frequencies, 3, 0), [0, 1, 0, 0]);
    assert_eq!(make_lengths_deft4j_java_heap(&frequencies, 3), [1, 1, 0, 0]);
}

#[test]
fn columbo_rle_pseudofrequencies_join_adjacent_buckets() {
    let mut frequencies = [9, 10, 11, 12, 21, 0, 0];

    make_columbo_rle_pseudofrequencies(&mut frequencies);

    assert_eq!(frequencies, [9, 12, 12, 12, 21, 0, 0]);
}

#[test]
fn columbo_rle_pseudofrequencies_do_not_bridge_zero_symbols() {
    let mut frequencies = [10, 0, 11, 7, 8, 0, 0];
    let original = frequencies;

    make_columbo_rle_pseudofrequencies(&mut frequencies);

    assert_eq!(frequencies, [10, 0, 11, 8, 8, 0, 0]);
    assert!(original
        .iter()
        .zip(frequencies)
        .all(|(&before, after)| before == 0 || after != 0));
}

#[test]
fn columbo_rle_frequency_buckets_handle_maximum_counts() {
    let mut frequencies = [u32::MAX - 1, u32::MAX, 0];

    make_columbo_rle_pseudofrequencies(&mut frequencies);

    assert_eq!(frequencies, [u32::MAX, u32::MAX, 0]);
}

#[test]
fn zopfli_rle_pseudofrequencies_collapse_nearby_counts() {
    let mut frequencies = [10, 11, 12, 13, 50, 0, 0];

    make_zopfli_rle_pseudofrequencies(&mut frequencies);

    assert_eq!(frequencies, [12, 12, 12, 12, 50, 0, 0]);
}

#[test]
fn zopfli_rle_pseudofrequencies_preserve_good_runs_and_trailing_zeros() {
    let mut frequencies = [7, 7, 7, 7, 7, 7, 7, 50, 2, 1, 0, 0, 0, 0, 0, 50, 0, 0];
    let trailing = frequencies[16..].to_vec();

    make_zopfli_rle_pseudofrequencies(&mut frequencies);

    assert_eq!(&frequencies[..7], &[7; 7]);
    assert_eq!(&frequencies[10..15], &[0; 5]);
    assert_eq!(&frequencies[16..], trailing);
}

#[test]
fn zopfli_rle_pseudofrequencies_handle_empty_and_maximum_counts() {
    let mut empty = [0_u32; 8];
    make_zopfli_rle_pseudofrequencies(&mut empty);
    assert_eq!(empty, [0; 8]);

    let mut maximum = [u32::MAX, u32::MAX - 1, u32::MAX - 2, u32::MAX - 3, 0];
    make_zopfli_rle_pseudofrequencies(&mut maximum);
    assert_eq!(
        maximum,
        [u32::MAX - 1, u32::MAX - 1, u32::MAX - 1, u32::MAX - 1, 0,]
    );
}

#[test]
fn brotli_rle_pseudofrequencies_follow_a_moving_fixed_point_mean() {
    let mut frequencies = [10, 14, 13, 12, 11, 50, 0, 0];

    make_brotli_rle_pseudofrequencies(&mut frequencies);

    assert_eq!(frequencies, [12, 12, 12, 12, 12, 50, 0, 0]);
}

#[test]
fn brotli_rle_pseudofrequencies_preserve_runs_and_extreme_counts() {
    let mut frequencies = [
        u32::MAX,
        u32::MAX - 1,
        u32::MAX - 2,
        u32::MAX - 3,
        7,
        7,
        7,
        7,
        7,
        7,
        7,
        0,
        0,
    ];

    make_brotli_rle_pseudofrequencies(&mut frequencies);

    assert_eq!(&frequencies[..4], &[u32::MAX - 1; 4]);
    assert_eq!(&frequencies[4..11], &[7; 7]);
    assert_eq!(&frequencies[11..], &[0; 2]);
}

#[test]
fn caller_owned_output_matches_allocating_helpers() {
    let frequencies = [4, 1, 9, 2, 2, 0];
    let mut output = [99_u8; 6];
    make_lengths_deflopt_heap_into(&frequencies, &mut output, 4, 2);
    assert_eq!(output, make_lengths_deflopt_heap(&frequencies, 4, 2)[..]);

    make_lengths_defluff_exact_into(&frequencies, &mut output, 4, 0);
    assert_eq!(output, make_lengths_defluff_exact(&frequencies, 4, 0)[..]);

    make_lengths_columbo_defluff_limited_into(&frequencies, &mut output, 4, 0);
    assert_eq!(
        output,
        make_lengths_columbo_defluff_limited(&frequencies, 4, 0)[..]
    );
}

#[test]
fn reused_deflopt_heap_scratch_matches_standalone_builds() {
    let fixtures: &[&[u32]] = &[
        &[4, 1, 9, 2, 2, 0],
        &[10, 10, 10, 10, 3, 3, 1, 0, 0],
        &[1, 1, 2, 3, 5, 8, 13, 21, 34, 55, 0, 0, 0],
    ];
    let mut scratch = DefloptHeapScratch::default();

    for &frequencies in fixtures {
        for variant in 0..4 {
            let expected = make_lengths_deflopt_heap(frequencies, 7, variant);
            let mut actual = vec![0; frequencies.len()];
            make_lengths_deflopt_heap_into_with_scratch(
                frequencies,
                &mut actual,
                7,
                variant,
                &mut scratch,
            );
            assert_eq!(actual, expected);

            let expected = make_lengths_order_heap(frequencies, 7, variant);
            make_lengths_order_heap_into_with_scratch(
                frequencies,
                &mut actual,
                7,
                variant,
                &mut scratch,
            );
            assert_eq!(actual, expected);
        }
    }
}
