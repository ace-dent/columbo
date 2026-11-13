// SPDX-License-Identifier: MIT
// SPDX-FileCopyrightText: 2026 Andrew C.E. Dent <https://github.com/ace-dent>

use super::*;

#[test]
fn padding_normalization_includes_discarded_empty_stored_blocks() {
    let mut input = [0xf8, 0, 0, 0xff, 0xff, 0x03, 0xfc];
    normalize_padding(&mut input, 0).unwrap();
    assert_eq!(input, [0, 0, 0, 0xff, 0xff, 0x03, 0]);
    let parsed = parse_stream(&input, 0).unwrap();
    assert_eq!(parsed.decoded_size, 0);
    assert_eq!(parsed.consumed, input.len());
}
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

    // Fixtures with an invalid tree fail before this point; valid ones end with
    // symbol 256.
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
    // HDIST may give all 32 distance code lengths; symbols 30 and 31 may have
    // codes but never occur in compressed data.
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

    // Compact vector with HDIST=32 and all distance code lengths zero.
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
    // Defluff emits symbol 284 with extra value 31 for length 258. Accepting
    // such streams lets default mode normalize them when that is no larger.
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

/// Parse under one model limit, whatever the stream's compressed size.
fn parse_stream_with_model_limit(
    input: &[u8],
    decoded_limit: u64,
    model_limit: usize,
) -> Result<ParsedStream> {
    let limits = ModelLimits {
        base: model_limit,
        scaled: model_limit,
    };
    parse_stream_with_model_limits(input, decoded_limit, limits)
}

/// `count` fixed blocks of one literal each: 18 bits for a whole block record.
fn one_literal_blocks(count: usize) -> Vec<u8> {
    let (literal, _) = crate::deflate::huffman::fixed_trees();
    let byte = literal.code(usize::from(b'x')).unwrap();
    let end = literal.code(256).unwrap();
    let mut writer = BitWriter::default();
    for index in 0..count {
        writer.write(u32::from(index + 1 == count), 1).unwrap();
        writer.write(1, 2).unwrap();
        writer.write(u32::from(byte.code), byte.length).unwrap();
        writer.write(u32::from(end.code), end.length).unwrap();
    }
    writer.into_bytes()
}

/// `count` stored blocks of `size` bytes each.
fn stored_blocks(count: usize, size: u16) -> Vec<u8> {
    let payload = vec![b'x'; usize::from(size)];
    let mut input = Vec::with_capacity(count * (payload.len() + 5));
    for index in 0..count {
        input.push(u8::from(index + 1 == count));
        input.extend_from_slice(&size.to_le_bytes());
        input.extend_from_slice(&(!size).to_le_bytes());
        input.extend_from_slice(&payload);
    }
    input
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

#[test]
fn parser_limit_check_matches_the_model_and_block_limits() {
    let token = std::mem::size_of::<Token>();
    // Retained blocks reach the model limit long before the source-block
    // limit, which also counts empty blocks.
    let blocks = BASE_PARSED_MODEL_BYTES / PARSED_BLOCK_MODEL_BYTES;
    assert!(blocks < MAX_SOURCE_BLOCKS);
    assert!(fits_parser_limits(0, 0, blocks));
    assert!(!fits_parser_limits(0, 0, blocks + 1));
    assert!(!fits_parser_limits(0, 0, MAX_SOURCE_BLOCKS + 1));

    let block_bytes = PARSED_BLOCK_MODEL_BYTES as u64;
    let decoded = BASE_PARSED_MODEL_BYTES as u64 - block_bytes - token as u64;
    assert!(fits_parser_limits(decoded, 1, 1));
    assert!(!fits_parser_limits(decoded + 1, 1, 1));
    assert!(!fits_parser_limits(decoded, 2, 1));
    assert!(!fits_parser_limits(u64::MAX, 0, 1));
}

const _: () = assert!(MAX_PARSED_MODEL_BYTES >= BASE_PARSED_MODEL_BYTES);
// A block of 12 compressed bytes pays for its record, as documented.
const _: () = assert!(PARSED_BLOCK_MODEL_BYTES <= 12 * BLOCK_RECORD_EXPANSION_RATIO);

#[test]
fn scaled_model_limit_covers_the_decoded_and_source_block_limits() {
    if usize::BITS > 32 {
        assert_eq!(MAX_PARSED_MODEL_BYTES as u64, 4 << 30);
        // A stream at the decoded limit fits while its tokens average five
        // decoded bytes, and so does every record the source-block limit allows.
        let decoded = crate::options::MAX_DECODED_BYTES as usize;
        assert!(parsed_model_bytes(decoded, decoded / 5, 1).unwrap() <= MAX_PARSED_MODEL_BYTES);
        assert!(parsed_model_bytes(0, 0, MAX_SOURCE_BLOCKS).unwrap() <= MAX_PARSED_MODEL_BYTES);
    }
    // Token counts stay below the `u32` symbol counts.
    let token = std::mem::size_of::<Token>();
    assert!(MAX_PARSED_MODEL_BYTES / (token + 1) < u32::MAX as usize);
}

#[test]
fn one_literal_blocks_never_reach_the_scaled_model_limit() {
    // Each record is charged for 18 bits of input, far beyond the allowed
    // expansion, so only the base limit applies however large the scaled one.
    let limits = ModelLimits {
        base: parsed_model_bytes(4, 4, 4).unwrap(),
        scaled: usize::MAX,
    };
    let parsed = parse_stream_with_model_limits(&one_literal_blocks(4), 1024, limits).unwrap();
    assert_eq!(parsed.blocks.len(), 4);
    let error = parse_stream_with_model_limits(&one_literal_blocks(5), 1024, limits).unwrap_err();
    assert!(error.message().contains("memory safety limit"));
}

#[test]
fn one_literal_blocks_stop_at_the_base_model_limit() {
    let blocks = BASE_PARSED_MODEL_BYTES / PARSED_BLOCK_MODEL_BYTES + 1;
    assert!(blocks < MAX_SOURCE_BLOCKS);
    let input = one_literal_blocks(blocks);
    assert!(input.len() < 128 * 1024);
    let error = parse_stream(&input, blocks as u64).unwrap_err();
    assert!(error.message().contains("memory safety limit"));
}

#[test]
fn proportionate_block_records_use_the_scaled_model_limit() {
    const BLOCKS: usize = 64;
    const SIZE: u16 = 32;
    // A stored block of 37 bytes pays for its record at the allowed expansion.
    assert!(PARSED_BLOCK_MODEL_BYTES <= (usize::from(SIZE) + 5) * BLOCK_RECORD_EXPANSION_RATIO / 2);
    let input = stored_blocks(BLOCKS, SIZE);
    let decoded = BLOCKS * usize::from(SIZE);
    let model = parsed_model_bytes(decoded, decoded, BLOCKS).unwrap();
    let base = parsed_model_bytes(2 * usize::from(SIZE), 2 * usize::from(SIZE), 2).unwrap();
    assert!(base < model);

    let limits = ModelLimits {
        base,
        scaled: model,
    };
    let parsed = parse_stream_with_model_limits(&input, decoded as u64, limits).unwrap();
    assert_eq!(parsed.blocks.len(), BLOCKS);
    assert_eq!(parsed.decoded_size, decoded as u64);

    // The scaled limit is exact, and without it the base limit applies.
    let limits = ModelLimits {
        base,
        scaled: model - 1,
    };
    let error = parse_stream_with_model_limits(&input, decoded as u64, limits).unwrap_err();
    assert!(error.message().contains("memory safety limit"));
    let error = parse_stream_with_model_limit(&input, decoded as u64, base).unwrap_err();
    assert!(error.message().contains("memory safety limit"));
}

#[test]
fn block_records_must_stay_proportionate_to_reach_the_scaled_model_limit() {
    // Ordinary blocks earn the scaled limit; one-literal blocks appended
    // after them use up that allowance and fall back to the base limit.
    const SIZE: u16 = 32;
    let mut input = stored_blocks(8, SIZE);
    *input.last_mut().unwrap() = b'x';
    let last_header = input.len() - usize::from(SIZE) - 5;
    input[last_header] = 0; // No longer the final block.
    let compressed = input.len();
    let allowed = compressed * BLOCK_RECORD_EXPANSION_RATIO / PARSED_BLOCK_MODEL_BYTES;
    assert!(allowed > 8);
    input.extend_from_slice(&one_literal_blocks(2 * allowed));

    let limits = ModelLimits {
        base: parsed_model_bytes(2 * usize::from(SIZE), 2 * usize::from(SIZE), 2).unwrap(),
        scaled: usize::MAX,
    };
    let error = parse_stream_with_model_limits(&input, 1 << 20, limits).unwrap_err();
    assert!(error.message().contains("memory safety limit"));
}

#[test]
fn parses_a_stream_whose_model_exceeds_the_base_limit() {
    if usize::BITS <= 32 {
        return;
    }
    // 21 MiB of stored bytes: every byte is a literal token, so the model is
    // above the 256 MiB base limit that used to reject such streams.
    const BLOCKS: usize = 21 * 1024 * 1024 / 65_535 + 1;
    let input = stored_blocks(BLOCKS, u16::MAX);
    let decoded = BLOCKS * 65_535;
    assert!(parsed_model_bytes(decoded, decoded, BLOCKS).unwrap() > BASE_PARSED_MODEL_BYTES);
    assert!(!fits_parser_limits(decoded as u64, decoded, BLOCKS));

    let parsed = parse_stream(&input, decoded as u64).unwrap();
    assert_eq!(parsed.decoded_size, decoded as u64);
    assert_eq!(parsed.blocks.len(), BLOCKS);
    assert_eq!(parsed.consumed, input.len());
}

#[test]
fn a_long_block_earns_the_scaled_model_limit_from_its_own_bytes() {
    // One fixed block of 64 literals, alone or after a one-byte stored block.
    // The base limit runs out halfway through the large block, before which
    // too few bytes were read to pay for its record; its own 8-bit literals
    // must pay for it.
    const LITERALS: usize = 64;
    const _: () =
        assert!(2 * PARSED_BLOCK_MODEL_BYTES <= LITERALS / 2 * BLOCK_RECORD_EXPANSION_RATIO);
    let (literal, _) = crate::deflate::huffman::fixed_trees();
    let byte = literal.code(usize::from(b'x')).unwrap();
    let end = literal.code(256).unwrap();
    for leading_block in [false, true] {
        let mut writer = BitWriter::default();
        if leading_block {
            writer.write(0, 3).unwrap();
            writer.align_to_byte().unwrap();
            writer.write(1, 16).unwrap();
            writer.write(0xfffe, 16).unwrap();
            writer.write_aligned_bytes(b"x").unwrap();
        }
        writer.write(1, 1).unwrap();
        writer.write(1, 2).unwrap();
        for _ in 0..LITERALS {
            writer.write(u32::from(byte.code), byte.length).unwrap();
        }
        writer.write(u32::from(end.code), end.length).unwrap();
        let input = writer.into_bytes();

        let blocks = 1 + usize::from(leading_block);
        let decoded = LITERALS + blocks - 1;
        let limits = ModelLimits {
            base: parsed_model_bytes(LITERALS / 2, LITERALS / 2, blocks).unwrap(),
            scaled: parsed_model_bytes(decoded, decoded, blocks).unwrap(),
        };
        let parsed = parse_stream_with_model_limits(&input, 1024, limits).unwrap();
        assert_eq!(parsed.decoded_size, decoded as u64);
        let limits = ModelLimits {
            scaled: limits.scaled - 1,
            ..limits
        };
        let error = parse_stream_with_model_limits(&input, 1024, limits).unwrap_err();
        assert!(error.message().contains("memory safety limit"));
    }
}

/// A plan holding its own copy of `block`'s buffers.
fn plan_copying(block: &ParsedBlock) -> PlannedBlock {
    PlannedBlock {
        tokens: Arc::new(block.tokens.to_vec()),
        plain: Arc::new(block.plain.to_vec()),
        representation: crate::deflate::model::Representation::Fixed,
        bits: 0,
        source_type: block.source_type,
    }
}

fn assert_same_parse(left: &ParsedStream, right: &ParsedStream) {
    assert_eq!(left.blocks.len(), right.blocks.len());
    for (left, right) in left.blocks.iter().zip(&right.blocks) {
        assert_eq!(left.tokens[..], right.tokens[..]);
        assert_eq!(left.plain[..], right.plain[..]);
        assert_eq!(left.literal_frequencies, right.literal_frequencies);
        assert_eq!(left.distance_frequencies, right.distance_frequencies);
        assert_eq!(left.original, right.original);
        assert_eq!(
            left.original_literal_lengths,
            right.original_literal_lengths
        );
        assert_eq!(
            left.original_distance_lengths,
            right.original_distance_lengths
        );
        assert_eq!(left.source_type, right.source_type);
    }
    let totals = |stream: &ParsedStream| {
        (
            stream.source_block_count,
            stream.consumed,
            stream.meaningful_bits,
            stream.crc32,
            stream.adler32,
            stream.decoded_size,
            stream.max_distance,
        )
    };
    assert_eq!(totals(left), totals(right));
}

fn shares(block: &ParsedBlock, plan: &PlannedBlock) -> bool {
    Arc::ptr_eq(&block.tokens, &plan.tokens) && Arc::ptr_eq(&block.plain, &plan.plain)
}

#[test]
fn a_parse_shares_the_buffers_of_the_plans_it_reproduces() {
    // A stored block, then a dynamic block whose matches reach into it.
    let input = crate::deflate::header::test_support::coupled_swap_test_stream();
    let parsed = parse_stream(&input, 1 << 14).unwrap();
    assert_eq!(parsed.blocks.len(), 2);
    assert_eq!(parsed.blocks[0].source_type, SourceBlockType::Stored);
    assert!(parsed.blocks[1]
        .tokens
        .iter()
        .any(|token| matches!(token, Token::Match { .. })));

    let plans: Vec<_> = parsed.blocks.iter().map(plan_copying).collect();
    let shared = parse_stream_sharing(&input, 1 << 14, &plans).unwrap();
    assert_same_parse(&parsed, &shared);
    assert!(shared
        .blocks
        .iter()
        .zip(&plans)
        .all(|(block, plan)| shares(block, plan)));
}

#[test]
fn a_parse_keeps_its_own_buffers_where_a_plan_differs() {
    let input = crate::deflate::header::test_support::coupled_swap_test_stream();
    let parsed = parse_stream(&input, 1 << 14).unwrap();
    let exact: Vec<_> = parsed.blocks.iter().map(plan_copying).collect();
    let edited = |block: usize, edit: &dyn Fn(&mut Vec<Token>, &mut Vec<u8>)| {
        let mut plans: Vec<_> = parsed.blocks.iter().map(plan_copying).collect();
        let mut tokens = plans[block].tokens.to_vec();
        let mut plain = plans[block].plain.to_vec();
        edit(&mut tokens, &mut plain);
        plans[block].tokens = Arc::new(tokens);
        plans[block].plain = Arc::new(plain);
        plans
    };
    let different_literal = |token: &mut Token| {
        *token = match *token {
            Token::Literal(byte) => Token::Literal(byte ^ 1),
            _ => Token::Literal(0),
        };
    };
    // (plans, whether each parsed block may share its plan's buffers)
    let cases: Vec<(Vec<PlannedBlock>, [bool; 2])> = vec![
        // The last token differs, after every earlier one matched.
        (
            edited(1, &|tokens, _| {
                different_literal(tokens.last_mut().unwrap())
            }),
            [true, false],
        ),
        // The first token differs.
        (
            edited(1, &|tokens, _| different_literal(&mut tokens[0])),
            [true, false],
        ),
        // The plan is one token short, or one token long.
        (
            edited(1, &|tokens, plain| {
                let removed = tokens.pop().unwrap().decoded_len();
                plain.truncate(plain.len() - removed);
            }),
            [true, false],
        ),
        (
            edited(1, &|tokens, plain| {
                tokens.push(Token::Literal(b'x'));
                plain.push(b'x');
            }),
            [true, false],
        ),
        // A stored byte differs.
        (edited(0, &|_, plain| plain[0] ^= 1), [false, true]),
        // One plan covers both blocks, after an empty plan.
        (
            {
                let mut merged = plan_copying(&parsed.blocks[0]);
                let mut tokens = exact[0].tokens.to_vec();
                tokens.extend_from_slice(&exact[1].tokens);
                let mut plain = exact[0].plain.to_vec();
                plain.extend_from_slice(&exact[1].plain);
                merged.tokens = Arc::new(tokens);
                merged.plain = Arc::new(plain);
                let mut empty = plan_copying(&parsed.blocks[0]);
                empty.tokens = Arc::new(Vec::new());
                empty.plain = Arc::new(Vec::new());
                vec![empty, merged]
            },
            [false, false],
        ),
        // The first plan is missing, so the second starts at the wrong offset.
        (vec![plan_copying(&parsed.blocks[1])], [false, false]),
    ];
    for (plans, expected) in cases {
        let shared = parse_stream_sharing(&input, 1 << 14, &plans).unwrap();
        assert_same_parse(&parsed, &shared);
        for (index, block) in shared.blocks.iter().enumerate() {
            let shared_with_plan = plans.iter().any(|plan| {
                Arc::ptr_eq(&block.tokens, &plan.tokens) || Arc::ptr_eq(&block.plain, &plan.plain)
            });
            assert_eq!(shared_with_plan, expected[index], "block {index}");
        }
    }
}
