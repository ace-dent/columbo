// SPDX-License-Identifier: MIT

//! Validating Deflate parser used by every optimization route.
//!
//! Parsing is deliberately completed before any optional search observes the
//! deadline. A successful prefix result therefore always consumes one whole
//! stream; `timed_out` can never describe a partially decoded member. The one
//! deliberate compatibility extension is Defluff's non-RFC length-258 alias.

use crate::checksum::{adler32_update, crc32_update};
use crate::{Error, Result};
use std::sync::OnceLock;

use super::bitstream::BitReader;
use super::huffman::{
    code_length_tree_shape_is_valid, payload_tree_shape_is_valid, Huffman,
    DISTANCE_DECODE_ROOT_BITS, FIXED_DISTANCE_CODE_LENGTHS, FIXED_LITERAL_CODE_LENGTHS,
    LITERAL_LENGTH_DECODE_ROOT_BITS,
};
use super::model::{
    DynamicPlan, OriginalBits, ParsedBlock, ParsedStream, RleToken, SourceBlockType, Token,
    CODE_LENGTH_ORDER, DISTANCE_BASE, DISTANCE_EXTRA_BITS, LENGTH_BASE, LENGTH_EXTRA_BITS,
    RFC_DISTANCE_CODE_COUNT, USABLE_DISTANCE_CODE_COUNT,
};

/// Data shared by every block representation after its payload is decoded.
///
/// Naming these fields keeps the stored, fixed, and dynamic parser routes
/// visibly aligned without relying on the position of values in a long tuple.
struct BlockPayload {
    tokens: Vec<Token>,
    plain: Vec<u8>,
    literal_frequencies: [u32; 286],
    distance_frequencies: [u32; 30],
    dynamic: Option<DynamicPlan>,
}

/// Parsed blocks deliberately retain decoded bytes, tokens, frequency tables,
/// and source metadata for later structural searches. A byte-only input limit
/// cannot bound that richer representation: a tiny stream may contain hundreds
/// of thousands of one-literal blocks. Keep the persistent model bounded so a
/// hostile but valid stream returns an error instead of exhausting the process.
/// Shared ceiling for persistent parsing and optional transformed-token
/// candidates. Search imports this value so a match cannot expand into a
/// token vector larger than the model the parser itself is willing to keep.
pub(crate) const MAX_PARSED_MODEL_BYTES: usize = 256 * 1024 * 1024;
/// Empty fixed blocks occupy only ten bits and are discarded from the retained
/// model. Without a separate count limit, a relatively small hostile stream
/// could therefore force millions of parser iterations while using almost no
/// decoded-byte or model budget.
const MAX_SOURCE_BLOCKS: usize = 262_144;
const PARSED_BLOCK_MODEL_BYTES: usize = std::mem::size_of::<ParsedBlock>() + 4 * 1024;
const WINDOW_SIZE: usize = 32_768;
const WINDOW_MASK: u64 = (WINDOW_SIZE - 1) as u64;
const MAX_MATCH_LENGTH: usize = 258;

/// Materialize one already-validated match without repeatedly updating the
/// parser's ring buffer. The first period comes from prior history; doubling
/// that period in the scratch buffer exactly reproduces Deflate's overlapping
/// copy semantics for every distance, including distance one.
fn expand_match<'a>(
    window: &[u8; WINDOW_SIZE],
    decoded_position: u64,
    distance: u16,
    length: u16,
    scratch: &'a mut [u8; MAX_MATCH_LENGTH],
) -> &'a [u8] {
    let distance = usize::from(distance);
    let length = usize::from(length);
    debug_assert!((1..=WINDOW_SIZE).contains(&distance));
    debug_assert!((3..=MAX_MATCH_LENGTH).contains(&length));
    debug_assert!(decoded_position >= distance as u64);

    let period = distance.min(length);
    let source = ((decoded_position - distance as u64) & WINDOW_MASK) as usize;
    let first = period.min(WINDOW_SIZE - source);
    scratch[..first].copy_from_slice(&window[source..source + first]);
    if first != period {
        scratch[first..period].copy_from_slice(&window[..period - first]);
    }

    let mut filled = period;
    while filled < length {
        let copied = filled.min(length - filled);
        scratch.copy_within(..copied, filled);
        filled += copied;
    }
    &scratch[..length]
}

fn fixed_payload_trees() -> (&'static Huffman, &'static Huffman) {
    static FIXED: OnceLock<(Huffman, Huffman)> = OnceLock::new();
    let (literal, distance) = FIXED.get_or_init(|| {
        (
            Huffman::build_value_decoder_with_root_bits(
                &FIXED_LITERAL_CODE_LENGTHS,
                257,
                &LENGTH_BASE,
                &LENGTH_EXTRA_BITS,
                LITERAL_LENGTH_DECODE_ROOT_BITS,
            )
            .expect("fixed literal tree is valid"),
            Huffman::build_value_decoder_with_root_bits(
                &FIXED_DISTANCE_CODE_LENGTHS,
                0,
                &DISTANCE_BASE,
                &DISTANCE_EXTRA_BITS,
                DISTANCE_DECODE_ROOT_BITS,
            )
            .expect("fixed distance tree is valid"),
        )
    });
    (literal, distance)
}

/// Estimate the owned model bytes accounted by the parser. Optional search
/// candidates use the same formula so their expanded tokens plus decoded
/// bytes cannot exceed the parser's established ceiling.
pub(crate) fn parsed_model_bytes(
    decoded_bytes: usize,
    token_count: usize,
    block_count: usize,
) -> Option<usize> {
    token_count
        .checked_mul(std::mem::size_of::<Token>())
        .and_then(|bytes| bytes.checked_add(decoded_bytes))
        .and_then(|bytes| {
            block_count
                .checked_mul(PARSED_BLOCK_MODEL_BYTES)
                .and_then(|block_bytes| bytes.checked_add(block_bytes))
        })
}

pub(crate) fn parse_stream(input: &[u8], decoded_limit: u64) -> Result<ParsedStream> {
    parse_stream_with_model_limit(input, decoded_limit, MAX_PARSED_MODEL_BYTES)
}

fn parse_stream_with_model_limit(
    input: &[u8],
    decoded_limit: u64,
    model_limit: usize,
) -> Result<ParsedStream> {
    let mut parser = Parser {
        reader: BitReader::new(input),
        window: [0; WINDOW_SIZE],
        decoded_position: 0,
        decoded_limit,
        model_limit,
        model_tokens: 0,
        retained_blocks: 0,
        crc32: 0,
        adler32: 1,
        max_distance: 0,
    };

    let mut blocks = Vec::new();
    let mut source_blocks = 0;
    let mut empty_blocks = 0;
    #[cfg(test)]
    let mut trailing_empty_blocks = 0;
    let mut saw_content = false;
    loop {
        if source_blocks >= MAX_SOURCE_BLOCKS {
            return Err(Error::complexity_limit(
                "Deflate stream exceeds the source-block safety limit",
            ));
        }
        parser.retained_blocks = blocks.len();
        let (block, final_block) = parser.parse_block()?;
        source_blocks += 1;
        if block.plain.is_empty() {
            empty_blocks += 1;
            #[cfg(test)]
            {
                trailing_empty_blocks += 1;
            }

            // Empty blocks have no effect on decoded bytes or history. Keep
            // one only while the stream might prove entirely empty; once a
            // content block exists, every empty block can be discarded as it
            // is parsed. This mirrors the original Columbo C implementation's
            // pending-block loop and prevents a compact run of empty blocks
            // from amplifying into large memory use through per-block frequency
            // tables.
            if !saw_content && blocks.is_empty() {
                blocks.try_reserve(1).map_err(|_| model_limit_error())?;
                blocks.push(block);
            }
        } else {
            #[cfg(test)]
            {
                trailing_empty_blocks = 0;
            }
            if !saw_content {
                blocks.clear(); // Drop the provisional all-empty block.
                saw_content = true;
            }
            blocks.try_reserve(1).map_err(|_| model_limit_error())?;
            blocks.push(block);
        }
        if final_block {
            break;
        }
    }

    let meaningful_bits = parser.reader.bit_position();
    let consumed = usize::try_from(meaningful_bits.div_ceil(8))
        .map_err(|_| Error::new("Deflate stream is too large"))?;
    debug_assert!(consumed <= input.len());

    Ok(ParsedStream {
        source_block_count: source_blocks,
        source_empty_block_count: empty_blocks,
        #[cfg(test)]
        source_trailing_empty_block_count: trailing_empty_blocks,
        blocks,
        consumed,
        meaningful_bits,
        crc32: parser.crc32,
        adler32: parser.adler32,
        decoded_size: parser.decoded_position,
        max_distance: parser.max_distance,
    })
}

struct Parser<'a> {
    reader: BitReader<'a>,
    window: [u8; WINDOW_SIZE],
    decoded_position: u64,
    decoded_limit: u64,
    model_limit: usize,
    model_tokens: usize,
    retained_blocks: usize,
    crc32: u32,
    adler32: u32,
    max_distance: u16,
}

impl Parser<'_> {
    fn parse_block(&mut self) -> Result<(ParsedBlock, bool)> {
        let start = self.reader.bit_position();
        let final_block = self.reader.read(1)? != 0;
        let block_type = match self.reader.read(2)? {
            0 => SourceBlockType::Stored,
            1 => SourceBlockType::Fixed,
            2 => SourceBlockType::Dynamic,
            _ => return Err(Error::new("invalid Deflate block type")),
        };

        let payload = match block_type {
            SourceBlockType::Stored => self.parse_stored_block()?,
            SourceBlockType::Fixed => {
                let (literal, distance) = fixed_payload_trees();
                self.parse_huffman_payload(literal, distance)?
            }
            SourceBlockType::Dynamic => {
                let (literal, distance, plan) = self.parse_dynamic_header()?;
                let mut payload = self.parse_huffman_payload(&literal, &distance)?;
                payload.dynamic = Some(plan);
                payload
            }
        };
        let BlockPayload {
            tokens,
            plain,
            literal_frequencies,
            distance_frequencies,
            dynamic,
        } = payload;

        self.update_checksums(&plain);
        let end = self.reader.bit_position();
        let original = OriginalBits {
            start,
            len: end - start,
            alignment: (start & 7) as u8,
            block_type,
        };

        let mut original_literal_lengths = None;
        let mut original_distance_lengths = None;
        if let Some(plan) = &dynamic {
            let mut literal = [0_u8; 286];
            let mut distance = [0_u8; 30];
            literal[..plan.literal_lengths.len()].copy_from_slice(&plan.literal_lengths);
            let usable_distance_codes = plan.distance_lengths.len().min(USABLE_DISTANCE_CODE_COUNT);
            distance[..usable_distance_codes]
                .copy_from_slice(&plan.distance_lengths[..usable_distance_codes]);
            original_literal_lengths = Some(literal);
            original_distance_lengths = Some(distance);
        }

        Ok((
            ParsedBlock {
                tokens: tokens.into(),
                plain: plain.into(),
                literal_frequencies,
                distance_frequencies,
                original_literal_lengths,
                original_distance_lengths,
                original_dynamic: dynamic,
                original: Some(original),
                source_splits: Vec::new(),
                source_type: block_type,
            },
            final_block,
        ))
    }

    fn parse_stored_block(&mut self) -> Result<BlockPayload> {
        self.reader.align_to_byte()?;
        let length = self.reader.read(16)? as u16;
        let complement = self.reader.read(16)? as u16;
        if length ^ complement != u16::MAX {
            return Err(Error::new("bad stored block length"));
        }

        self.reserve_decoded(u64::from(length))?;
        self.reserve_model(u64::from(length), usize::from(length))?;
        let source = self.reader.read_aligned_bytes(usize::from(length))?;
        let mut plain = Vec::new();
        plain
            .try_reserve_exact(source.len())
            .map_err(|_| model_limit_error())?;
        plain.extend_from_slice(source);
        let mut tokens = Vec::new();
        tokens
            .try_reserve_exact(plain.len())
            .map_err(|_| model_limit_error())?;
        let mut literal_frequencies = [0_u32; 286];
        for &byte in &plain {
            tokens.push(Token::Literal(byte));
            literal_frequencies[usize::from(byte)] += 1;
        }
        literal_frequencies[256] = 1;
        self.append_history(&plain);
        Ok(BlockPayload {
            tokens,
            plain,
            literal_frequencies,
            distance_frequencies: [0; 30],
            dynamic: None,
        })
    }

    fn parse_dynamic_header(&mut self) -> Result<(Huffman, Huffman, DynamicPlan)> {
        let hlit = self.reader.read(5)? as usize + 257;
        let hdist = self.reader.read(5)? as usize + 1;
        let hclen = self.reader.read(4)? as usize + 4;
        if hlit > 286 || hdist > RFC_DISTANCE_CODE_COUNT {
            return Err(Error::new("invalid dynamic Huffman header"));
        }

        let mut code_length_lengths = [0_u8; 19];
        for &symbol in &CODE_LENGTH_ORDER[..hclen] {
            code_length_lengths[symbol] = self.reader.read(3)? as u8;
        }
        let code_length_tree = Huffman::build_decoder(&code_length_lengths)
            .ok_or_else(|| Error::new("invalid code-length Huffman tree"))?;
        // The code-length alphabet has none of the one-symbol exceptions used
        // by payload trees. zlib-compatible decoders require it to be complete.
        if !code_length_tree_shape_is_valid(&code_length_lengths) {
            return Err(Error::new("invalid code-length Huffman tree"));
        }

        let target = hlit + hdist;
        let mut lengths = Vec::with_capacity(target);
        let mut rle = Vec::new();
        let mut previous = 0_u8;
        while lengths.len() < target {
            let symbol = code_length_tree.decode(&mut self.reader)?;
            match symbol {
                0..=15 => {
                    previous = symbol as u8;
                    lengths.push(previous);
                    rle.push(RleToken {
                        symbol: previous,
                        extra: 0,
                    });
                }
                16 => {
                    // Symbol 16 copies an already decoded length; unlike 17 and
                    // 18, it does not provide an implicit initial zero.
                    if lengths.is_empty() {
                        return Err(Error::new("dynamic length repeat has no previous length"));
                    }
                    let extra = self.reader.read(2)? as u8;
                    let count = usize::from(extra) + 3;
                    if lengths.len() + count > target {
                        return Err(Error::new("dynamic length repeat overflows header"));
                    }
                    lengths.extend(std::iter::repeat(previous).take(count));
                    rle.push(RleToken { symbol: 16, extra });
                }
                17 => {
                    let extra = self.reader.read(3)? as u8;
                    let count = usize::from(extra) + 3;
                    if lengths.len() + count > target {
                        return Err(Error::new("dynamic length repeat overflows header"));
                    }
                    lengths.extend(std::iter::repeat(0).take(count));
                    previous = 0;
                    rle.push(RleToken { symbol: 17, extra });
                }
                18 => {
                    let extra = self.reader.read(7)? as u8;
                    let count = usize::from(extra) + 11;
                    if lengths.len() + count > target {
                        return Err(Error::new("dynamic length repeat overflows header"));
                    }
                    lengths.extend(std::iter::repeat(0).take(count));
                    previous = 0;
                    rle.push(RleToken { symbol: 18, extra });
                }
                _ => return Err(Error::new("invalid dynamic length symbol")),
            }
        }

        let literal_lengths = lengths[..hlit].to_vec();
        let distance_lengths = lengths[hlit..].to_vec();
        let literal = Huffman::build_value_decoder_with_root_bits(
            &literal_lengths,
            257,
            &LENGTH_BASE,
            &LENGTH_EXTRA_BITS,
            LITERAL_LENGTH_DECODE_ROOT_BITS,
        )
        .ok_or_else(|| Error::new("invalid literal/length Huffman tree"))?;
        if literal.code(256).is_none() {
            return Err(Error::new("dynamic Huffman tree has no end code"));
        }
        if !payload_tree_shape_is_valid(&literal_lengths, false) {
            return Err(Error::new("invalid literal/length Huffman tree"));
        }
        let distance = Huffman::build_value_decoder_with_root_bits(
            &distance_lengths,
            0,
            &DISTANCE_BASE,
            &DISTANCE_EXTRA_BITS,
            DISTANCE_DECODE_ROOT_BITS,
        )
        .ok_or_else(|| Error::new("invalid distance Huffman tree"))?;
        if !payload_tree_shape_is_valid(&distance_lengths, true) {
            return Err(Error::new("invalid distance Huffman tree"));
        }

        Ok((
            literal,
            distance,
            DynamicPlan {
                literal_lengths,
                distance_lengths,
                code_length_lengths,
                rle,
                hlit,
                hdist,
                hclen,
                bits: 0,
            },
        ))
    }

    fn parse_huffman_payload(
        &mut self,
        literal_tree: &Huffman,
        distance_tree: &Huffman,
    ) -> Result<BlockPayload> {
        let mut tokens = Vec::new();
        let mut plain = Vec::new();
        let mut literal_frequencies = [0_u32; 286];
        let mut distance_frequencies = [0_u32; 30];

        loop {
            let decoded = literal_tree.decode_value(&mut self.reader)?;
            let symbol = decoded.symbol;
            if symbol > 285 {
                return Err(Error::new("invalid literal/length code"));
            }
            literal_frequencies[usize::from(symbol)] += 1;
            match symbol {
                0..=255 => {
                    self.reserve_decoded(1)?;
                    self.reserve_model(1, 1)?;
                    tokens.try_reserve(1).map_err(|_| model_limit_error())?;
                    plain.try_reserve(1).map_err(|_| model_limit_error())?;
                    let byte = symbol as u8;
                    tokens.push(Token::Literal(byte));
                    plain.push(byte);
                    self.push_history(byte);
                }
                256 => break,
                257..=285 => {
                    let length = decoded.value;

                    let decoded_distance = distance_tree.decode_value(&mut self.reader)?;
                    let distance_symbol = decoded_distance.symbol;
                    if distance_symbol > 29 {
                        return Err(Error::new("invalid distance code"));
                    }
                    let distance_index = usize::from(distance_symbol);
                    let distance = decoded_distance.value;
                    if distance == 0
                        || distance > 32_768
                        || u64::from(distance) > self.decoded_position
                    {
                        return Err(Error::new("distance points before beginning of stream"));
                    }
                    self.max_distance = self.max_distance.max(distance);
                    self.reserve_decoded(u64::from(length))?;
                    self.reserve_model(u64::from(length), 1)?;
                    tokens.try_reserve(1).map_err(|_| model_limit_error())?;
                    plain
                        .try_reserve(usize::from(length))
                        .map_err(|_| model_limit_error())?;
                    distance_frequencies[distance_index] += 1;
                    tokens.push(Token::Match {
                        length,
                        distance,
                        length_symbol: symbol,
                        distance_symbol: distance_symbol as u8,
                        length_extra: decoded.extra,
                        distance_extra: decoded_distance.extra,
                        length_extra_bits: decoded.extra_bits,
                        distance_extra_bits: decoded_distance.extra_bits,
                    });

                    let mut scratch = [0_u8; MAX_MATCH_LENGTH];
                    let expanded = expand_match(
                        &self.window,
                        self.decoded_position,
                        distance,
                        length,
                        &mut scratch,
                    );
                    plain.extend_from_slice(expanded);
                    self.append_history(expanded);
                }
                _ => unreachable!(),
            }
        }

        Ok(BlockPayload {
            tokens,
            plain,
            literal_frequencies,
            distance_frequencies,
            dynamic: None,
        })
    }

    fn reserve_decoded(&self, count: u64) -> Result<()> {
        if count > self.decoded_limit.saturating_sub(self.decoded_position) {
            return Err(Error::resource_limit("decoded data exceeds safety limit"));
        }
        Ok(())
    }

    fn reserve_model(&mut self, decoded_count: u64, token_count: usize) -> Result<()> {
        let decoded = self
            .decoded_position
            .checked_add(decoded_count)
            .and_then(|bytes| usize::try_from(bytes).ok())
            .ok_or_else(model_limit_error)?;
        let tokens = self
            .model_tokens
            .checked_add(token_count)
            .ok_or_else(model_limit_error)?;
        let blocks = self
            .retained_blocks
            .checked_add(1)
            .ok_or_else(model_limit_error)?;
        let estimated =
            parsed_model_bytes(decoded, tokens, blocks).ok_or_else(model_limit_error)?;
        if estimated > self.model_limit {
            return Err(model_limit_error());
        }
        self.model_tokens = tokens;
        Ok(())
    }

    fn push_history(&mut self, byte: u8) {
        self.window[(self.decoded_position & WINDOW_MASK) as usize] = byte;
        self.decoded_position += 1;
    }

    fn append_history(&mut self, bytes: &[u8]) {
        if bytes.is_empty() {
            return;
        }

        let next_position = self.decoded_position + bytes.len() as u64;
        // When the input is larger than the ring, only its final window can
        // affect a later match. Locate that suffix at its absolute position so
        // the circular layout remains identical to byte-at-a-time insertion.
        let retained = if bytes.len() > WINDOW_SIZE {
            &bytes[bytes.len() - WINDOW_SIZE..]
        } else {
            bytes
        };
        let retained_position = next_position - retained.len() as u64;
        let destination = (retained_position & WINDOW_MASK) as usize;
        let first = retained.len().min(WINDOW_SIZE - destination);
        self.window[destination..destination + first].copy_from_slice(&retained[..first]);
        if first != retained.len() {
            self.window[..retained.len() - first].copy_from_slice(&retained[first..]);
        }
        self.decoded_position = next_position;
    }

    fn update_checksums(&mut self, bytes: &[u8]) {
        self.crc32 = crc32_update(self.crc32, bytes);
        self.adler32 = adler32_update(self.adler32, bytes);
    }
}

/// Decode one already-validated raw stream into a bounded comparison buffer.
///
/// This is deliberately not a public decompression API. PNG uses it only to
/// prove that two APNG frame streams contain exactly the same decoded bytes
/// before reusing the smaller compressed representation. Returning `None`
/// merely disables that optional optimization.
pub(crate) fn decoded_bytes_for_comparison(
    input: &[u8],
    decoded_limit: u64,
    comparison_limit: usize,
) -> Option<Vec<u8>> {
    if decoded_limit > comparison_limit as u64 {
        return None;
    }
    let parsed = parse_stream(input, decoded_limit).ok()?;
    let decoded_size = usize::try_from(parsed.decoded_size).ok()?;
    if parsed.consumed != input.len() || decoded_size > comparison_limit {
        return None;
    }

    let mut decoded = Vec::new();
    decoded.try_reserve_exact(decoded_size).ok()?;
    for block in parsed.blocks {
        decoded.extend_from_slice(&block.plain);
    }
    (decoded.len() == decoded_size).then_some(decoded)
}

/// Compare a raw stream with exact decoded bytes without allocating a second
/// complete decoded buffer.
pub(crate) fn raw_stream_decodes_to(input: &[u8], decoded_limit: u64, expected: &[u8]) -> bool {
    let Ok(parsed) = parse_stream(input, decoded_limit) else {
        return false;
    };
    if parsed.consumed != input.len() || parsed.decoded_size != expected.len() as u64 {
        return false;
    }

    let mut offset = 0_usize;
    for block in parsed.blocks {
        let Some(end) = offset.checked_add(block.plain.len()) else {
            return false;
        };
        if expected.get(offset..end) != Some(block.plain.as_slice()) {
            return false;
        }
        offset = end;
    }
    offset == expected.len()
}

fn model_limit_error() -> Error {
    Error::complexity_limit("Deflate structure exceeds internal memory safety limit")
}

#[cfg(test)]
mod tests {
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
            0xe5, 0xc0, 0x81, 0x00, 0x00, 0x00, 0x00, 0x80, 0x20, 0xb6, 0xfd, 0xa5, 0x06, 0xa9,
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0xbe, 0x01,
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
        let error =
            parse_stream_with_model_limit(&input, 1024, PARSED_BLOCK_MODEL_BYTES).unwrap_err();
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
}
