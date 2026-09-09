// SPDX-License-Identifier: MIT

use super::*;
use crate::deflate::bitstream::BitWriter;

#[test]
fn bulk_match_expansion_matches_bytewise_overlap_semantics() {
    let distances = (1..=WINDOW_SIZE)
        .step_by(127)
        .chain([1, 2, 3, 255, 256, 257, 32_767, 32_768]);
    for decoded_position in [32_768_u64, 32_769, 65_535, 65_536, 100_003] {
        let mut window = [0_u8; WINDOW_SIZE];
        for absolute in decoded_position - WINDOW_SIZE as u64..decoded_position {
            window[(absolute & WINDOW_MASK) as usize] = (absolute as u8)
                .wrapping_mul(157)
                .wrapping_add((absolute >> 11) as u8);
        }

        for distance in distances.clone() {
            for length in 3..=MAX_MATCH_LENGTH {
                let mut expected = Vec::with_capacity(length);
                for offset in 0..length {
                    let byte = if offset >= distance {
                        expected[offset - distance]
                    } else {
                        let absolute = decoded_position - distance as u64 + offset as u64;
                        window[(absolute & WINDOW_MASK) as usize]
                    };
                    expected.push(byte);
                }

                let mut scratch = [0_u8; MAX_MATCH_LENGTH];
                assert_eq!(
                    expand_match(
                        &window,
                        decoded_position,
                        distance as u16,
                        length as u16,
                        &mut scratch,
                    ),
                    expected,
                    "position={decoded_position} distance={distance} length={length}",
                );
            }
        }
    }
}

#[test]
fn bulk_history_update_feeds_a_wrapped_cross_block_match() {
    let stored: Vec<u8> = (0..65_535)
        .map(|index| (index as u8).wrapping_mul(193).wrapping_add(17))
        .collect();
    let mut writer = BitWriter::default();
    writer.write(0, 1).unwrap();
    writer.write(0, 2).unwrap();
    writer.align_to_byte().unwrap();
    writer.write(65_535, 16).unwrap();
    writer.write(0, 16).unwrap();
    writer.write_aligned_bytes(&stored).unwrap();

    writer.write(1, 1).unwrap();
    writer.write(1, 2).unwrap();
    let (literal, distance) = crate::deflate::huffman::fixed_trees();
    let length = literal.code(285).unwrap();
    writer.write(u32::from(length.code), length.length).unwrap();
    let offset = distance.code(29).unwrap();
    writer.write(u32::from(offset.code), offset.length).unwrap();
    writer.write(8_191, 13).unwrap();
    let end = literal.code(256).unwrap();
    writer.write(u32::from(end.code), end.length).unwrap();

    let parsed = parse_stream(&writer.into_bytes(), 65_535 + 258).unwrap();
    assert_eq!(parsed.blocks.len(), 2);
    assert_eq!(
        parsed.blocks[1].plain.as_slice(),
        &stored[65_535 - 32_768..65_535 - 32_768 + 258]
    );
}

fn write_dynamic_prefix(
    writer: &mut BitWriter,
    literal_count: usize,
    distance_count: usize,
    code_length_lengths: &[u8; 19],
) {
    assert!((257..=286).contains(&literal_count));
    assert!((1..=RFC_DISTANCE_CODE_COUNT).contains(&distance_count));
    let hclen = CODE_LENGTH_ORDER
        .iter()
        .rposition(|&symbol| code_length_lengths[symbol] != 0)
        .map_or(4, |position| (position + 1).max(4));

    writer.write(1, 1).unwrap(); // Final block.
    writer.write(2, 2).unwrap(); // Dynamic Huffman block.
    writer.write((literal_count - 257) as u32, 5).unwrap();
    writer.write((distance_count - 1) as u32, 5).unwrap();
    writer.write((hclen - 4) as u32, 4).unwrap();
    for &symbol in &CODE_LENGTH_ORDER[..hclen] {
        writer
            .write(u32::from(code_length_lengths[symbol]), 3)
            .unwrap();
    }
}

fn explicit_dynamic_stream(
    literal_lengths: &[u8],
    distance_lengths: &[u8],
    code_length_lengths: [u8; 19],
) -> Vec<u8> {
    let mut writer = BitWriter::default();
    write_dynamic_prefix(
        &mut writer,
        literal_lengths.len(),
        distance_lengths.len(),
        &code_length_lengths,
    );

    let code_length_tree = Huffman::build(&code_length_lengths).unwrap();
    for &length in literal_lengths.iter().chain(distance_lengths) {
        let code = code_length_tree.code(usize::from(length)).unwrap();
        writer.write(u32::from(code.code), code.length).unwrap();
    }

    // Invalid tree-shape tests are rejected before reaching this payload.
    // Valid empty-block fixtures consume the ordinary end code.
    if let Some(end) = Huffman::build(literal_lengths).and_then(|tree| tree.code(256)) {
        writer.write(u32::from(end.code), end.length).unwrap();
    }
    writer.into_bytes()
}

#[test]
fn parses_empty_fixed_stream() {
    // BFINAL=1, BTYPE=fixed, EOB=256 (seven zero wire bits).
    let parsed = parse_stream(&[0x03, 0x00], 1024).unwrap();
    assert_eq!(parsed.consumed, 2);
    assert_eq!(parsed.meaningful_bits, 10);
    assert_eq!(parsed.decoded_size, 0);
    assert_eq!(parsed.source_block_count, 1);
}

#[test]
fn rejects_stored_length_mismatch() {
    let error = parse_stream(&[0x01, 0x01, 0x00, 0x00, 0x00, b'x'], 1024).unwrap_err();
    assert!(error.message().contains("stored block length"));
}

#[test]
fn rejects_incomplete_code_length_tree() {
    let mut writer = BitWriter::default();
    let mut code_length_lengths = [0_u8; 19];
    code_length_lengths[0] = 1; // One one-bit code is still incomplete here.
    write_dynamic_prefix(&mut writer, 257, 1, &code_length_lengths);

    let error = parse_stream(&writer.into_bytes(), 0).unwrap_err();
    assert_eq!(error.message(), "invalid code-length Huffman tree");
}

#[test]
fn rejects_repeat_sixteen_without_a_previous_length() {
    let mut writer = BitWriter::default();
    let mut code_length_lengths = [0_u8; 19];
    code_length_lengths[16] = 1;
    code_length_lengths[18] = 1;
    write_dynamic_prefix(&mut writer, 257, 1, &code_length_lengths);

    let code_length_tree = Huffman::build(&code_length_lengths).unwrap();
    let repeat = code_length_tree.code(16).unwrap();
    writer.write(u32::from(repeat.code), repeat.length).unwrap();
    writer.write(0, 2).unwrap();

    let error = parse_stream(&writer.into_bytes(), 0).unwrap_err();
    assert_eq!(
        error.message(),
        "dynamic length repeat has no previous length"
    );
}

#[test]
fn rejects_incomplete_multi_symbol_payload_trees() {
    let mut code_length_lengths = [0_u8; 19];
    code_length_lengths[0] = 1;
    code_length_lengths[2] = 1;
    let mut literal_lengths = vec![0_u8; 257];
    literal_lengths[0] = 2;
    literal_lengths[256] = 2;
    let input = explicit_dynamic_stream(&literal_lengths, &[0], code_length_lengths);
    let error = parse_stream(&input, 0).unwrap_err();
    assert_eq!(error.message(), "invalid literal/length Huffman tree");

    let mut code_length_lengths = [0_u8; 19];
    code_length_lengths[0] = 1;
    code_length_lengths[1] = 2;
    code_length_lengths[2] = 2;
    let mut literal_lengths = vec![0_u8; 257];
    literal_lengths[256] = 1;
    let input = explicit_dynamic_stream(&literal_lengths, &[2, 2], code_length_lengths);
    let error = parse_stream(&input, 0).unwrap_err();
    assert_eq!(error.message(), "invalid distance Huffman tree");
}

#[test]
fn accepts_minimal_payload_tree_exceptions() {
    let mut code_length_lengths = [0_u8; 19];
    code_length_lengths[0] = 1;
    code_length_lengths[1] = 1;
    let mut literal_lengths = vec![0_u8; 257];
    literal_lengths[256] = 1;

    for distance_length in [0, 1] {
        let input =
            explicit_dynamic_stream(&literal_lengths, &[distance_length], code_length_lengths);
        let parsed = parse_stream(&input, 0).unwrap();
        assert_eq!(parsed.decoded_size, 0);
    }
}

#[test]
fn accepts_unused_reserved_distance_codes_in_dynamic_headers() {
    // RFC 1951 permits HDIST to advertise all 32 distance-code lengths.
    // Symbols 30 and 31 may participate in the tree, but cannot be used
    // by the compressed payload.
    let mut code_length_lengths = [0_u8; 19];
    code_length_lengths[0] = 1;
    code_length_lengths[1] = 1;
    let mut literal_lengths = vec![0_u8; 257];
    literal_lengths[256] = 1;

    for reserved_symbol in 30..=31 {
        let mut distance_lengths = vec![0_u8; reserved_symbol + 1];
        distance_lengths[reserved_symbol] = 1;
        let input =
            explicit_dynamic_stream(&literal_lengths, &distance_lengths, code_length_lengths);

        let parsed = parse_stream(&input, 0).unwrap();
        let dynamic = parsed.blocks[0].original_dynamic.as_ref().unwrap();
        assert_eq!(dynamic.hdist, reserved_symbol + 1);
        assert_eq!(dynamic.distance_lengths[reserved_symbol], 1);
        assert_eq!(parsed.decoded_size, 0);
    }

    // Also retain the audit's compact HDIST=32/all-zero-distance vector.
    let all_zero = [
        0x05, 0xdf, 0x81, 0x00, 0x00, 0x00, 0x00, 0x00, 0x90, 0xff, 0x6b, 0x2b, 0x00,
    ];
    let parsed = parse_stream(&all_zero, 0).unwrap();
    assert_eq!(
        parsed.blocks[0].original_dynamic.as_ref().unwrap().hdist,
        32
    );
}

#[test]
fn rejects_a_reserved_distance_code_when_the_payload_uses_it() {
    let mut code_length_lengths = [0_u8; 19];
    code_length_lengths[0] = 1;
    code_length_lengths[1] = 1;
    let mut literal_lengths = vec![0_u8; 258];
    literal_lengths[256] = 1;
    literal_lengths[257] = 1;
    let mut distance_lengths = vec![0_u8; 31];
    distance_lengths[30] = 1;

    let mut writer = BitWriter::default();
    write_dynamic_prefix(
        &mut writer,
        literal_lengths.len(),
        distance_lengths.len(),
        &code_length_lengths,
    );
    let code_length_tree = Huffman::build(&code_length_lengths).unwrap();
    for &length in literal_lengths.iter().chain(&distance_lengths) {
        let code = code_length_tree.code(usize::from(length)).unwrap();
        writer.write(u32::from(code.code), code.length).unwrap();
    }
    let literal_tree = Huffman::build(&literal_lengths).unwrap();
    let distance_tree = Huffman::build(&distance_lengths).unwrap();
    let match_code = literal_tree.code(257).unwrap();
    writer
        .write(u32::from(match_code.code), match_code.length)
        .unwrap();
    let reserved = distance_tree.code(30).unwrap();
    writer
        .write(u32::from(reserved.code), reserved.length)
        .unwrap();

    let error = parse_stream(&writer.into_bytes(), 3).unwrap_err();
    assert_eq!(error.message(), "invalid distance code");
}

#[test]
fn accepts_the_defluff_258_alias_as_an_input_compatibility_extension() {
    // Defluff emits symbol 284 with extra value 31 for length 258. That
    // spelling is outside RFC 1951, but accepting existing streams lets
    // default mode normalize them when doing so is no larger.
    let input = [
        0xe5, 0xc0, 0x81, 0x00, 0x00, 0x00, 0x00, 0x80, 0x20, 0xb6, 0xfd, 0xa5, 0x06, 0xa9, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0xbe, 0x01,
    ];
    let parsed = parse_stream(&input, 4_096).unwrap();

    assert!(parsed.blocks.iter().any(|block| {
        block.tokens.iter().any(|token| {
            matches!(
                token,
                Token::Match {
                    length: 258,
                    length_symbol: 284,
                    length_extra: 31,
                    length_extra_bits: 5,
                    ..
                }
            )
        })
    }));
}

#[test]
fn collapses_long_runs_of_empty_blocks_while_counting_them() {
    const BLOCKS: usize = 10_000;

    let mut writer = BitWriter::default();
    for index in 0..BLOCKS {
        writer.write(u32::from(index + 1 == BLOCKS), 1).unwrap();
        writer.write(1, 2).unwrap(); // Fixed Huffman block.
        writer.write(0, 7).unwrap(); // Fixed end-of-block symbol 256.
    }

    let parsed = parse_stream(&writer.into_bytes(), 0).unwrap();
    assert_eq!(parsed.source_block_count, BLOCKS);
    assert_eq!(parsed.source_empty_block_count, BLOCKS);
    assert_eq!(parsed.source_trailing_empty_block_count, BLOCKS);
    assert_eq!(parsed.blocks.len(), 1);
    assert_eq!(parsed.decoded_size, 0);
}

#[test]
fn counts_only_consecutive_empty_blocks_at_the_source_tail() {
    let mut writer = BitWriter::default();

    writer.write(0, 1).unwrap(); // Non-final empty fixed block.
    writer.write(1, 2).unwrap();
    writer.write(0, 7).unwrap(); // Fixed end-of-block symbol 256.

    writer.write(0, 1).unwrap(); // Non-final stored content block.
    writer.write(0, 2).unwrap();
    writer.align_to_byte().unwrap();
    writer.write(1, 16).unwrap();
    writer.write(0xfffe, 16).unwrap();
    writer.write_aligned_bytes(b"x").unwrap();

    for index in 0..2 {
        writer.write(u32::from(index == 1), 1).unwrap();
        writer.write(1, 2).unwrap();
        writer.write(0, 7).unwrap();
    }

    let parsed = parse_stream(&writer.into_bytes(), 1).unwrap();
    assert_eq!(parsed.source_block_count, 4);
    assert_eq!(parsed.source_empty_block_count, 3);
    assert_eq!(parsed.source_trailing_empty_block_count, 2);
    assert_eq!(parsed.blocks.len(), 1);
    assert_eq!(parsed.decoded_size, 1);
}

#[test]
fn rejects_excessive_discarded_empty_blocks() {
    let mut writer = BitWriter::default();
    for _ in 0..=MAX_SOURCE_BLOCKS {
        writer.write(0, 1).unwrap(); // Keep every encoded block non-final.
        writer.write(1, 2).unwrap(); // Fixed Huffman block.
        writer.write(0, 7).unwrap(); // Fixed end-of-block symbol 256.
    }

    let error = parse_stream(&writer.into_bytes(), 0).unwrap_err();
    assert_eq!(
        error.message(),
        "Deflate stream exceeds the source-block safety limit"
    );
}

#[test]
fn rejects_a_stored_block_before_its_token_model_exceeds_budget() {
    let input = [0x01, 0x01, 0x00, 0xfe, 0xff, b'x'];
    let error = parse_stream_with_model_limit(&input, 1024, PARSED_BLOCK_MODEL_BYTES).unwrap_err();
    assert!(error.message().contains("memory safety limit"));
}

#[test]
fn incremental_model_accounting_preserves_the_exact_limit() {
    let (literal, _) = crate::deflate::huffman::fixed_trees();
    let byte = literal.code(usize::from(b'x')).unwrap();
    let end = literal.code(256).unwrap();
    let mut writer = BitWriter::default();
    writer.write(1, 1).unwrap();
    writer.write(1, 2).unwrap();
    writer.write(u32::from(byte.code), byte.length).unwrap();
    writer.write(u32::from(end.code), end.length).unwrap();
    let input = writer.into_bytes();

    let exact_limit = parsed_model_bytes(1, 1, 1).unwrap();
    let parsed = parse_stream_with_model_limit(&input, 1, exact_limit).unwrap();
    assert_eq!(parsed.decoded_size, 1);
    let error = parse_stream_with_model_limit(&input, 1, exact_limit - 1).unwrap_err();
    assert!(error.message().contains("memory safety limit"));
}

#[test]
fn bounds_many_nonempty_source_blocks() {
    let (literal, _) = crate::deflate::huffman::fixed_trees();
    let byte = literal.code(usize::from(b'x')).unwrap();
    let end = literal.code(256).unwrap();
    let mut writer = BitWriter::default();
    for index in 0..8 {
        writer.write(u32::from(index == 7), 1).unwrap();
        writer.write(1, 2).unwrap();
        writer.write(u32::from(byte.code), byte.length).unwrap();
        writer.write(u32::from(end.code), end.length).unwrap();
    }

    let error =
        parse_stream_with_model_limit(&writer.into_bytes(), 1024, PARSED_BLOCK_MODEL_BYTES * 3)
            .unwrap_err();
    assert!(error.message().contains("memory safety limit"));
}
