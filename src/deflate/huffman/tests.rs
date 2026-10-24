// SPDX-License-Identifier: MIT
// SPDX-FileCopyrightText: 2026 Andrew C.E. Dent <https://github.com/ace-dent>

use super::test_support::make_lengths_deflopt_heap;
use super::*;
use crate::deflate::bitstream::BitWriter;

const DEFAULT_DECODE_ROOT_BITS: u8 = 9;

impl Huffman {
    /// Build the code with a two-level decode table but no value metadata.
    fn build_decoder(lengths: &[u8]) -> Option<Self> {
        Self::build_decoder_with_root_bits(lengths, DEFAULT_DECODE_ROOT_BITS)
    }

    fn build_decoder_with_root_bits(lengths: &[u8], root_bits: u8) -> Option<Self> {
        let mut tree = Self::build(lengths)?;
        tree.decode_table = Some(build_decode_table(&tree, root_bits)?);
        Some(tree)
    }

    /// Decode one symbol, consuming no more than the tree's maximum length.
    fn decode(&self, reader: &mut BitReader<'_>) -> Result<u16> {
        if let Some(table) = &self.decode_table {
            return self.decode_table(reader, table);
        }
        self.decode_canonical(reader)
    }

    fn decode_table(&self, reader: &mut BitReader<'_>, table: &DecodeTable) -> Result<u16> {
        if let Some((entry, bits)) = self.peek_decode_table(reader, table)? {
            reader.drop_bits(bits)?;
            return Ok(entry.symbol());
        }
        self.decode_canonical(reader)
    }

    fn max_bits(&self) -> u8 {
        self.max_bits
    }
}

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

/// Build Columbo's heap variant that breaks frequency ties by creation order,
/// not by subtree height as DeflOpt 2.07 does.
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
        &mut TreeBuildScratch::default(),
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

fn swapping_heap_sift_down(heap: &mut [HeapEntry], start: usize, frequency_only: bool) {
    let less = |a: HeapEntry, b: HeapEntry| {
        if a.frequency() != b.frequency() {
            return a.frequency() < b.frequency();
        }
        !frequency_only && a.tie() < b.tie()
    };
    let mut root = start;
    loop {
        let mut child = root * 2 + 1;
        if child >= heap.len() {
            return;
        }
        if child + 1 < heap.len() && less(heap[child + 1], heap[child]) {
            child += 1;
        }
        if !less(heap[child], heap[root]) {
            return;
        }
        heap.swap(root, child);
        root = child;
    }
}

#[test]
fn hole_heap_sift_matches_the_swapping_reference() {
    // The tie field stands for a height or a creation order alike.
    let entries: Vec<HeapEntry> = (0..64_u32)
        .map(|index| HeapEntry {
            key: heap_key((index * 17 + 5) % 13, (index * 7 + 3) % 11),
            index,
        })
        .collect();

    for len in 1..=entries.len() {
        for seed in 0..4_u64 {
            let mut heap = entries[..len].to_vec();
            let mut random = seed + len as u64;
            for index in (1..len).rev() {
                random ^= random << 13;
                random ^= random >> 7;
                random ^= random << 17;
                heap.swap(index, random as usize % (index + 1));
            }
            for start in 0..len {
                for frequency_only in [false, true] {
                    let mut expected = heap.clone();
                    let mut actual = heap.clone();
                    swapping_heap_sift_down(&mut expected, start, frequency_only);
                    if frequency_only {
                        heap_sift_down::<true>(&mut actual, start);
                    } else {
                        heap_sift_down::<false>(&mut actual, start);
                    }
                    let index =
                        |heap: &[HeapEntry]| heap.iter().map(|e| e.index).collect::<Vec<_>>();
                    assert_eq!(index(&actual), index(&expected));
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
    // Fibonacci-like weights make the unlimited tree deeper than 4 bits.
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

    // Expected vectors come from Columbo's C `huffman.c`. The generic
    // builder keeps its odd-overflow result even though a complete code
    // within the limit exists.
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
fn generic_overflow_repair_gives_short_codes_to_frequent_symbols() {
    // The unlimited code is [4, 4, 3, 2, 1]; repair at 3 bits leaves one
    // 1-bit and four 3-bit codes. The 1-bit code belongs to the symbol with
    // count 8: 32 payload bits, against 46 with the order reversed.
    for variant in 0..4 {
        assert_eq!(make_lengths(&[1, 1, 2, 4, 8], 3, variant), [3, 3, 3, 3, 1]);
    }
}

#[test]
fn generic_overflow_repair_keeps_lengths_ordered_by_frequency() {
    let mut state = 0x2545_f491_u32;
    let mut repaired = 0;
    for _ in 0..2_000 {
        let mut frequencies = [0_u32; 24];
        for frequency in &mut frequencies {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            // Skewed counts make deep unlimited trees common.
            *frequency = (1_u32 << (state % 14)) >> (state >> 28);
        }
        let max_bits = 5 + (state % 3) as u8;
        let mut lengths = vec![0; frequencies.len()];
        if !make_lengths_inner(
            &frequencies,
            &mut lengths,
            max_bits,
            0,
            &mut TreeBuildScratch::default(),
        ) {
            continue;
        }
        repaired += 1;
        assert_bounded_lengths(&frequencies, &lengths, max_bits);
        for (a, &frequency_a) in frequencies.iter().enumerate() {
            for (b, &frequency_b) in frequencies.iter().enumerate() {
                if frequency_a > frequency_b && frequency_b != 0 {
                    assert!(lengths[a] <= lengths[b], "{frequencies:?} -> {lengths:?}");
                }
            }
        }
    }
    assert!(repaired > 100, "only {repaired} inputs needed repair");
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

/// The node-index heap the keyed heaps replaced.
fn reference_generic_heap_sift_down(
    nodes: &[Node],
    heap: &mut [usize],
    mut root: usize,
    variant: u32,
) {
    loop {
        let mut child = root * 2 + 1;
        if child >= heap.len() {
            return;
        }
        if child + 1 < heap.len() && node_less(nodes, heap[child + 1], heap[child], variant) {
            child += 1;
        }
        if !node_less(nodes, heap[child], heap[root], variant) {
            return;
        }
        heap.swap(root, child);
        root = child;
    }
}

fn reference_generic_heap_pop(
    nodes: &[Node],
    heap: &mut Vec<usize>,
    variant: u32,
) -> Option<usize> {
    let result = *heap.first()?;
    let last = heap.pop().expect("the heap is non-empty");
    if !heap.is_empty() {
        heap[0] = last;
        reference_generic_heap_sift_down(nodes, heap, 0, variant);
    }
    Some(result)
}

fn reference_generic_heap_push(nodes: &[Node], heap: &mut Vec<usize>, node: usize, variant: u32) {
    let mut position = heap.len();
    heap.push(node);
    while position != 0 {
        let parent = (position - 1) / 2;
        if !node_less(nodes, heap[position], heap[parent], variant) {
            break;
        }
        heap.swap(position, parent);
        position = parent;
    }
}

fn reference_merge_generic_nodes_heap(
    nodes: &mut Vec<Node>,
    active: &mut Vec<usize>,
    variant: u32,
) {
    for start in (0..active.len() / 2).rev() {
        reference_generic_heap_sift_down(nodes, active, start, variant);
    }
    while active.len() > 1 {
        let left = reference_generic_heap_pop(nodes, active, variant).expect("two nodes remain");
        let right = reference_generic_heap_pop(nodes, active, variant).expect("one node remains");
        let index = nodes.len();
        let frequency = nodes[left].frequency.wrapping_add(nodes[right].frequency);
        nodes.push(Node::branch(frequency, left, right, index));
        reference_generic_heap_push(nodes, active, index, variant);
    }
}

fn reference_generic_heap_pop_active(
    nodes: &[Node],
    heap: &mut Vec<usize>,
    active: &[bool],
    variant: u32,
) -> Option<usize> {
    loop {
        let node = reference_generic_heap_pop(nodes, heap, variant)?;
        if active[node] {
            return Some(node);
        }
    }
}

fn reference_merge_generic_nodes_dual_heap(
    nodes: &mut Vec<Node>,
    active: &mut Vec<usize>,
    variant: u32,
) {
    let mut increasing_order = active.clone();
    let mut decreasing_order = active.clone();
    for start in (0..increasing_order.len() / 2).rev() {
        reference_generic_heap_sift_down(nodes, &mut increasing_order, start, 0);
        reference_generic_heap_sift_down(nodes, &mut decreasing_order, start, 1);
    }
    let mut is_active = vec![true; nodes.len()];
    let mut remaining = active.len();
    while remaining > 1 {
        let (first, second) = if variant & 1 != 0 { (1, 0) } else { (0, 1) };
        let mut pop = |order: u32, is_active: &[bool]| {
            let heap = if order == 0 {
                &mut increasing_order
            } else {
                &mut decreasing_order
            };
            reference_generic_heap_pop_active(nodes, heap, is_active, order)
        };
        let left = pop(first, &is_active).expect("two nodes remain");
        is_active[left] = false;
        let right = pop(second, &is_active).expect("one node remains");
        is_active[right] = false;
        let index = nodes.len();
        let frequency = nodes[left].frequency.wrapping_add(nodes[right].frequency);
        nodes.push(Node::branch(frequency, left, right, index));
        is_active.push(true);
        reference_generic_heap_push(nodes, &mut increasing_order, index, 0);
        reference_generic_heap_push(nodes, &mut decreasing_order, index, 1);
        remaining -= 1;
    }
    let root = reference_generic_heap_pop_active(nodes, &mut increasing_order, &is_active, 0)
        .expect("one root remains");
    active.clear();
    active.push(root);
}

#[test]
fn keyed_heap_merges_match_the_node_heap_reference() {
    let topology = |nodes: &[Node]| -> Vec<_> {
        nodes
            .iter()
            .map(|node| {
                (
                    node.frequency,
                    node.symbol,
                    node.left,
                    node.right,
                    node.order,
                )
            })
            .collect()
    };
    let mut state = 0x2545_f491_u32;
    for alphabet_len in [19, 30, 286] {
        for sample in 0..256 {
            // Small frequencies give many ties, and a leaf's order (its
            // symbol) can equal a branch's (its index).
            let mut nodes = Vec::new();
            let mut active = Vec::new();
            for symbol in 0..alphabet_len {
                state ^= state << 13;
                state ^= state >> 17;
                state ^= state << 5;
                let frequency = (state ^ sample) % 5;
                if frequency != 0 {
                    active.push(nodes.len());
                    nodes.push(Node::leaf(frequency, symbol, symbol));
                }
            }
            if active.len() < 2 {
                continue;
            }
            for variant in 0..4 {
                let (mut keyed_nodes, mut keyed_active) = (nodes.clone(), active.clone());
                let (mut reference_nodes, mut reference_active) = (nodes.clone(), active.clone());
                if variant & 2 == 0 {
                    merge_generic_nodes_heap(&mut keyed_nodes, &mut keyed_active, variant);
                    reference_merge_generic_nodes_heap(
                        &mut reference_nodes,
                        &mut reference_active,
                        variant,
                    );
                } else {
                    merge_generic_nodes_dual_heap(&mut keyed_nodes, &mut keyed_active, variant);
                    reference_merge_generic_nodes_dual_heap(
                        &mut reference_nodes,
                        &mut reference_active,
                        variant,
                    );
                }
                assert_eq!(
                    topology(&keyed_nodes),
                    topology(&reference_nodes),
                    "alphabet={alphabet_len} sample={sample} variant={variant}",
                );
                assert_eq!(keyed_active, reference_active);
            }
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

    make_lengths_columbo_defluff_limited_into(
        &frequencies,
        &mut output,
        4,
        0,
        &mut TreeBuildScratch::default(),
    );
    assert_eq!(
        output,
        make_lengths_columbo_defluff_limited(&frequencies, 4, 0)[..]
    );
}

#[test]
fn shared_tree_scratch_matches_fresh_builds() {
    // One scratch serves every builder in turn, as in a header search, so
    // each must clear what the previous build left.
    let mut state = 0x3c6e_f372_fe94_f82b_u64;
    let mut next = move || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state
    };
    let mut scratch = TreeBuildScratch::default();
    for case in 0..6_000 {
        let size = [19, 30, 286, 2][case % 4];
        let range = [2, 6, 50, 1 << 16][case % 3];
        let mut frequencies: Vec<u32> = (0..size)
            .map(|_| {
                if next() % 3 == 0 {
                    0
                } else {
                    (next() % range) as u32
                }
            })
            .collect();
        if case % 5 == 0 {
            // Fibonacci-like counts force overflow repair.
            let (mut a, mut b) = (1_u32, 1_u32);
            for frequency in frequencies.iter_mut().take(24) {
                *frequency = a;
                (a, b) = (b, a.saturating_add(b));
            }
        }
        let max_bits = [7_u8, 15][case % 2];
        let mut lengths = vec![0; size];

        make_lengths_deft4j_java_heap_into(&frequencies, &mut lengths, max_bits, &mut scratch);
        assert_eq!(
            lengths,
            make_lengths_deft4j_java_heap(&frequencies, max_bits),
            "case {case}"
        );
        for variant in 0..4 {
            make_lengths_into(&frequencies, &mut lengths, max_bits, variant, &mut scratch);
            assert_eq!(
                lengths,
                make_lengths(&frequencies, max_bits, variant),
                "case {case}, variant {variant}"
            );
            make_lengths_deflopt_heap_into_with_scratch(
                &frequencies,
                &mut lengths,
                max_bits,
                variant,
                &mut scratch,
            );
            assert_eq!(
                lengths,
                make_lengths_deflopt_heap(&frequencies, max_bits, variant),
                "case {case}, variant {variant}"
            );
        }
        make_lengths_columbo_defluff_limited_into(
            &frequencies,
            &mut lengths,
            max_bits,
            0,
            &mut scratch,
        );
        assert_eq!(
            lengths,
            make_lengths_columbo_defluff_limited(&frequencies, max_bits, 0),
            "case {case}"
        );
    }
}

#[test]
fn reused_deflopt_heap_scratch_matches_standalone_builds() {
    let fixtures: &[&[u32]] = &[
        &[4, 1, 9, 2, 2, 0],
        &[10, 10, 10, 10, 3, 3, 1, 0, 0],
        &[1, 1, 2, 3, 5, 8, 13, 21, 34, 55, 0, 0, 0],
    ];
    let mut scratch = TreeBuildScratch::default();

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

// The DeflOpt-style heap builder before its compact rewrite, kept to prove
// that the rewrite returns identical lengths.
#[derive(Debug, Clone, Copy)]
enum ReferenceHeapTie {
    FrequencyOnly,
    Height,
    Order,
}

#[derive(Default)]
struct ReferenceHeapScratch {
    nodes: Vec<Node>,
    heap: Vec<usize>,
    leaf_order: Vec<usize>,
}

#[inline]
fn reference_heap_node_less(nodes: &[Node], a: usize, b: usize, tie: ReferenceHeapTie) -> bool {
    if nodes[a].frequency != nodes[b].frequency {
        return nodes[a].frequency < nodes[b].frequency;
    }
    match tie {
        ReferenceHeapTie::FrequencyOnly => false,
        ReferenceHeapTie::Height => nodes[a].height < nodes[b].height,
        ReferenceHeapTie::Order => nodes[a].order < nodes[b].order,
    }
}

fn reference_heap_sift_down(
    nodes: &[Node],
    heap: &mut [usize],
    start: usize,
    tie: ReferenceHeapTie,
) {
    let node = heap[start];
    let mut root = start;
    loop {
        let mut child = root * 2 + 1;
        if child >= heap.len() {
            break;
        }
        if child + 1 < heap.len()
            && reference_heap_node_less(nodes, heap[child + 1], heap[child], tie)
        {
            child += 1;
        }
        if !reference_heap_node_less(nodes, heap[child], node, tie) {
            break;
        }
        heap[root] = heap[child];
        root = child;
    }
    heap[root] = node;
}

fn reference_heapify_frequency_only(nodes: &[Node], heap: &mut [usize]) {
    for start in (0..heap.len() / 2).rev() {
        reference_heap_sift_down(nodes, heap, start, ReferenceHeapTie::FrequencyOnly);
    }
}

fn reference_variant_heap(
    frequencies: &[u32],
    lengths: &mut [u8],
    max_bits: u8,
    variant: u32,
    use_order_tie: bool,
    scratch: &mut ReferenceHeapScratch,
) {
    assert_eq!(frequencies.len(), lengths.len());
    lengths.fill(0);

    let ReferenceHeapScratch {
        nodes,
        heap,
        leaf_order,
    } = scratch;
    nodes.clear();
    heap.clear();
    leaf_order.clear();
    nodes.reserve(frequencies.len().saturating_mul(2));
    heap.reserve(frequencies.len());
    for (symbol, &frequency) in frequencies.iter().enumerate() {
        if frequency != 0 {
            heap.push(nodes.len());
            nodes.push(Node::leaf(frequency, symbol, nodes.len()));
        }
    }
    let leaf_count = heap.len();
    match leaf_count {
        0 => return,
        1 => {
            lengths[nodes[heap[0]].symbol.expect("leaf has a symbol")] = 1;
            return;
        }
        _ => {}
    }

    leaf_order.resize(leaf_count, 0);
    let mut leaf_order_position = leaf_count;
    reference_heapify_frequency_only(nodes, heap);

    // Variant bit 1 sets the tie rule of the sift-down after removing the
    // first child, and bit 0 the one after inserting the parent.
    let first_tie = if variant & 2 != 0 {
        ReferenceHeapTie::FrequencyOnly
    } else if use_order_tie {
        ReferenceHeapTie::Order
    } else {
        ReferenceHeapTie::Height
    };
    let second_tie = if variant & 1 != 0 {
        ReferenceHeapTie::FrequencyOnly
    } else if use_order_tie {
        ReferenceHeapTie::Order
    } else {
        ReferenceHeapTie::Height
    };

    while heap.len() > 1 {
        let left = heap[0];
        if nodes[left].symbol.is_some() {
            leaf_order_position -= 1;
            leaf_order[leaf_order_position] = left;
        }
        let last = heap.pop().expect("heap is non-empty");
        heap[0] = last;

        reference_heap_sift_down(nodes, heap, 0, first_tie);

        let right = heap[0];
        if nodes[right].symbol.is_some() {
            leaf_order_position -= 1;
            leaf_order[leaf_order_position] = right;
        }
        let index = nodes.len();
        let mut parent = Node::branch(
            nodes[left].frequency.wrapping_add(nodes[right].frequency),
            left,
            right,
            index,
        );
        parent.height = nodes[left].height.max(nodes[right].height) + 1;
        nodes.push(parent);
        heap[0] = index;

        reference_heap_sift_down(nodes, heap, 0, second_tie);
    }

    debug_assert_eq!(leaf_order_position, 0);
    let max_depth = assign_depths(nodes, heap[0], 0, lengths);
    if max_depth <= usize::from(max_bits) {
        return;
    }
    reference_repair_deflopt_overflow(nodes, leaf_order, lengths, max_bits, max_depth);
}

fn reference_repair_deflopt_overflow(
    nodes: &[Node],
    leaf_order: &[usize],
    lengths: &mut [u8],
    max_bits: u8,
    max_depth: usize,
) {
    let limit = usize::from(max_bits);
    if limit == 0 || limit > MAX_TRACKED_DEPTH {
        lengths.fill(0);
        return;
    }

    let mut count_by_length = [0_usize; MAX_TRACKED_DEPTH + 1];
    for &length in lengths.iter() {
        if length != 0 {
            count_by_length[usize::from(length).min(MAX_TRACKED_DEPTH)] += 1;
        }
    }

    let mut deepest = max_depth.min(MAX_TRACKED_DEPTH);
    // Until no code exceeds `limit`, split one shorter code into two and
    // merge two of the deepest codes into their parent, as DeflOpt's loop at
    // 0x407a80 does.
    while deepest > limit {
        let mut bits = limit - 1;
        while bits > 0 && count_by_length[bits] == 0 {
            bits -= 1;
        }
        if bits == 0 {
            break;
        }
        count_by_length[bits] -= 1;
        count_by_length[bits + 1] += 2;
        if count_by_length[deepest] >= 2 {
            count_by_length[deepest] -= 2;
            count_by_length[deepest - 1] += 1;
        }
        while deepest > limit && count_by_length[deepest] == 0 {
            deepest -= 1;
        }
    }
    count_by_length[limit + 1..].fill(0);

    // Like DeflOpt, assign the shortest lengths in reverse heap-removal order.
    lengths.fill(0);
    let mut leaf = 0;
    for (bits, &count) in count_by_length.iter().enumerate().take(limit + 1).skip(1) {
        for _ in 0..count {
            if leaf == leaf_order.len() {
                return;
            }
            let symbol = nodes[leaf_order[leaf]]
                .symbol
                .expect("leaf order contains only leaves");
            lengths[symbol] = bits as u8;
            leaf += 1;
        }
    }
}

#[test]
fn compact_variant_heap_matches_the_reference_builder() {
    let mut state = 0x9e37_79b9_7f4a_7c15_u64;
    let mut next = move || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state
    };
    let mut scratch = TreeBuildScratch::default();
    let mut reference_scratch = ReferenceHeapScratch::default();
    for case in 0..40_000 {
        let size = match case % 4 {
            0 => 19,
            1 => 30,
            2 => 286,
            _ => 1 + (next() % 320) as usize,
        };
        // Small ranges make equal counts and deep, overflowing trees common;
        // large ones exercise wrapping parent sums.
        let range = match case % 5 {
            0 => 2,
            1 => 5,
            2 => 40,
            3 => 1 << 20,
            _ => u64::from(u32::MAX),
        };
        let mut frequencies: Vec<u32> = (0..size)
            .map(|_| {
                if next() % 3 == 0 {
                    0
                } else {
                    (next() % range) as u32
                }
            })
            .collect();
        if case % 7 == 0 {
            // Fibonacci-like counts force overflow repair.
            let (mut a, mut b) = (1_u32, 1_u32);
            for frequency in frequencies.iter_mut().take(30) {
                *frequency = a;
                (a, b) = (b, a.saturating_add(b));
            }
        }
        for max_bits in [7_u8, 15] {
            for variant in 0..4 {
                for use_order_tie in [false, true] {
                    let mut lengths = vec![0; size];
                    let mut expected = vec![0; size];
                    make_lengths_variant_heap_inner(
                        &frequencies,
                        &mut lengths,
                        max_bits,
                        variant,
                        use_order_tie,
                        &mut scratch,
                    );
                    reference_variant_heap(
                        &frequencies,
                        &mut expected,
                        max_bits,
                        variant,
                        use_order_tie,
                        &mut reference_scratch,
                    );
                    assert_eq!(
                        lengths, expected,
                        "case {case}, variant {variant}, order tie {use_order_tie}, limit {max_bits}"
                    );
                }
            }
        }
    }
}

#[test]
fn frequency_only_heap_variant_ignores_the_tie_key() {
    // Header and tree candidate lists skip one of these two builds.
    let mut state = 0x6a09_e667_f3bc_c908_u64;
    let mut next = move || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state
    };
    let mut scratch = TreeBuildScratch::default();
    for case in 0..20_000 {
        let size = [19, 30, 286][case % 3];
        let range = [2, 6, 50, 1 << 16][case % 4];
        let frequencies: Vec<u32> = (0..size)
            .map(|_| {
                if next() % 3 == 0 {
                    0
                } else {
                    (next() % range) as u32
                }
            })
            .collect();
        for max_bits in [7_u8, 15] {
            let mut deflopt = vec![0; size];
            let mut order = vec![0; size];
            make_lengths_deflopt_heap_into_with_scratch(
                &frequencies,
                &mut deflopt,
                max_bits,
                3,
                &mut scratch,
            );
            make_lengths_order_heap_into_with_scratch(
                &frequencies,
                &mut order,
                max_bits,
                3,
                &mut scratch,
            );
            assert_eq!(deflopt, order, "case {case}, limit {max_bits}");
        }
    }
}

#[test]
fn leaf_key_sorts_match_the_comparator_sorts() {
    let mut random = 0x9e37_79b9_7f4a_7c15_u64;
    let mut next = || {
        random ^= random << 13;
        random ^= random >> 7;
        random ^= random << 17;
        random
    };
    for round in 0..2_000 {
        let len = 2 + next() as u32 % 290;
        // Few distinct frequencies give many ties; the last range reaches
        // the frequency field's limit.
        let range = [2, 3, 50, u64::from(u32::MAX)][round % 4];
        let leaves: Vec<(u32, u32)> = (0..len)
            .map(|symbol| ((next() % range) as u32, symbol))
            .filter(|&(frequency, _)| frequency != 0)
            .collect();
        for descending in [false, true] {
            let mut expected = leaves.clone();
            if descending {
                expected.sort_unstable_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
            } else {
                expected.sort_unstable();
            }
            let mut keys: Vec<u64> = leaves
                .iter()
                .map(|&(frequency, symbol)| {
                    leaf_key(if descending { !frequency } else { frequency }, symbol)
                })
                .collect();
            sort_leaf_keys(&mut keys);
            let actual: Vec<u32> = keys.iter().map(|&key| leaf_key_tie(key)).collect();
            let expected: Vec<u32> = expected.iter().map(|&(_, symbol)| symbol).collect();
            assert_eq!(actual, expected, "round {round}, descending {descending}");
        }
    }
}
