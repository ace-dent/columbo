// SPDX-License-Identifier: MIT
// SPDX-FileCopyrightText: 2026 Andrew C.E. Dent <https://github.com/ace-dent>

//! Validating Deflate parser used by every optimization route.
//!
//! Parsing finishes before any optional search checks the deadline, so a
//! successful prefix result covers one whole stream and `timed_out` never
//! describes a partly decoded member. The only accepted extension to RFC 1951
//! is Defluff's length-258 alias (symbol 284 with extra value 31).

use std::sync::OnceLock;

use crate::checksum::{adler32_update, crc32_update};
use crate::{Error, Result};

use super::bitstream::BitReader;
use super::huffman::{
    payload_tree_shape_is_valid, CodeLengthDecoder, Huffman, DISTANCE_DECODE_ROOT_BITS,
    FIXED_DISTANCE_CODE_LENGTHS, FIXED_LITERAL_CODE_LENGTHS, LITERAL_LENGTH_DECODE_ROOT_BITS,
};
use super::model::{
    DynamicPlan, OriginalBits, ParsedBlock, ParsedStream, RleToken, SourceBlockType, Token,
    CODE_LENGTH_ORDER, DISTANCE_BASE, DISTANCE_EXTRA_BITS, LENGTH_BASE, LENGTH_EXTRA_BITS,
    RFC_DISTANCE_CODE_COUNT, USABLE_DISTANCE_CODE_COUNT,
};

struct BlockPayload {
    tokens: Vec<Token>,
    plain: Vec<u8>,
    literal_frequencies: [u32; 286],
    distance_frequencies: [u32; 30],
    dynamic: Option<DynamicPlan>,
}

/// Memory limit for the parsed model, shared with transformed-token candidates.
/// Input size cannot bound the model: a tiny stream can hold hundreds of
/// thousands of one-literal blocks.
pub(crate) const MAX_PARSED_MODEL_BYTES: usize = 256 * 1024 * 1024;
/// Empty blocks (10 bits when fixed) are dropped from the model, so this bounds
/// parser work on a small hostile stream.
const MAX_SOURCE_BLOCKS: usize = 262_144;
const PARSED_BLOCK_MODEL_BYTES: usize = std::mem::size_of::<ParsedBlock>() + 4 * 1024;
const WINDOW_SIZE: usize = 32_768;
const WINDOW_MASK: u64 = (WINDOW_SIZE - 1) as u64;
const MAX_MATCH_LENGTH: usize = 258;

/// Expand one validated match into `scratch`. Doubling the first
/// min(distance, length) bytes reproduces Deflate's overlapping copy.
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

/// Model size charged against [`MAX_PARSED_MODEL_BYTES`]; search candidates
/// use the same formula.
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
    parse_stream_with_padding(input, decoded_limit, model_limit, |_, _| Ok(()))
}

/// Parse and validate every block, calling `padding(start, end)` for each bit
/// range a decoder ignores: stored-block alignment and final-byte padding.
fn parse_stream_with_padding(
    input: &[u8],
    decoded_limit: u64,
    model_limit: usize,
    mut padding: impl FnMut(u64, u64) -> Result<()>,
) -> Result<ParsedStream> {
    let mut parser = Parser {
        reader: BitReader::new(input),
        window: [0; WINDOW_SIZE],
        decoded_position: 0,
        decoded_limit,
        model_limit,
        model_payload_bytes: 0,
        current_model_payload_limit: None,
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
        if let Some(original) = block.original.as_ref() {
            if original.block_type == SourceBlockType::Stored {
                let start = original.start + 3;
                padding(start, start.div_ceil(8) * 8)?;
            }
        }
        source_blocks += 1;
        if block.plain.is_empty() {
            empty_blocks += 1;
            #[cfg(test)]
            {
                trailing_empty_blocks += 1;
            }

            // Keep one empty block only until content appears, so an empty
            // stream still has a block (as in Columbo's C parser).
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
    padding(meaningful_bits, consumed as u64 * 8)?;

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

/// Zero the stored-block alignment bits and final-byte padding found by the
/// parser. Masks are collected first so parsing sees the unmodified stream.
pub(crate) fn normalize_padding(input: &mut [u8], decoded_limit: u64) -> Result<()> {
    let mut masks = Vec::new();
    let parsed = parse_stream_with_padding(
        input,
        decoded_limit,
        MAX_PARSED_MODEL_BYTES,
        |start, end| {
            if start != end {
                let offset = (start / 8) as usize;
                let mask = (0xff_u16 << (start % 8)) as u8;
                if input[offset] & mask != 0 {
                    masks
                        .try_reserve(1)
                        .map_err(|_| Error::internal("could not allocate padding masks"))?;
                    masks.push((offset, mask));
                }
            }
            Ok(())
        },
    )?;
    drop(parsed);
    for (offset, mask) in masks {
        input[offset] &= !mask;
    }
    Ok(())
}

struct Parser<'a> {
    reader: BitReader<'a>,
    window: [u8; WINDOW_SIZE],
    decoded_position: u64,
    decoded_limit: u64,
    model_limit: usize,
    model_payload_bytes: usize,
    current_model_payload_limit: Option<usize>,
    retained_blocks: usize,
    crc32: u32,
    adler32: u32,
    max_distance: u16,
}

impl Parser<'_> {
    fn parse_block(&mut self) -> Result<(ParsedBlock, bool)> {
        self.current_model_payload_limit = self
            .retained_blocks
            .checked_add(1)
            .and_then(|blocks| blocks.checked_mul(PARSED_BLOCK_MODEL_BYTES))
            .and_then(|block_bytes| self.model_limit.checked_sub(block_bytes));
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

        self.reserve_payload(u64::from(length), usize::from(length))?;
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
        // Unlike the payload codes, the code-length code has no single-code
        // exception; zlib requires it to be complete.
        let code_length_tree = CodeLengthDecoder::build(&code_length_lengths)
            .ok_or_else(|| Error::new("invalid code-length Huffman tree"))?;

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
                    self.reserve_payload(1, 1)?;
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
                    self.reserve_payload(u64::from(length), 1)?;
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

    fn reserve_payload(&mut self, decoded_count: u64, token_count: usize) -> Result<()> {
        if decoded_count > self.decoded_limit.saturating_sub(self.decoded_position) {
            return Err(Error::resource_limit("decoded data exceeds safety limit"));
        }

        let added_model_bytes = usize::try_from(decoded_count)
            .ok()
            .and_then(|decoded| {
                token_count
                    .checked_mul(std::mem::size_of::<Token>())
                    .and_then(|tokens| decoded.checked_add(tokens))
            })
            .ok_or_else(model_limit_error)?;
        let model_payload_bytes = self
            .model_payload_bytes
            .checked_add(added_model_bytes)
            .ok_or_else(model_limit_error)?;
        if self
            .current_model_payload_limit
            .map_or(true, |limit| model_payload_bytes > limit)
        {
            return Err(model_limit_error());
        }
        self.model_payload_bytes = model_payload_bytes;
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
        // Only the last 32 KiB matter. Place them at their absolute position
        // so the ring matches byte-by-byte insertion.
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

/// Decode one already-validated raw stream if it decodes to at most
/// `comparison_limit` bytes; `None` disables APNG exact frame reuse.
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

/// Decode one complete raw stream for a ZIP Store (method 0) alternative,
/// returning the decoded bytes, meaningful bits and CRC-32.
pub(crate) fn decoded_bytes_for_storage(
    input: &[u8],
    decoded_limit: u64,
) -> Result<(Vec<u8>, u64, u32)> {
    let parsed = parse_stream(input, decoded_limit)?;
    if parsed.consumed != input.len() {
        return Err(Error::internal(
            "optimized Deflate storage candidate has trailing data",
        ));
    }
    let decoded_size = usize::try_from(parsed.decoded_size)
        .map_err(|_| Error::resource_limit("decoded Deflate payload is too large"))?;
    let mut decoded = Vec::new();
    decoded
        .try_reserve_exact(decoded_size)
        .map_err(|_| Error::internal("could not allocate stored Deflate payload"))?;
    for block in parsed.blocks {
        decoded.extend_from_slice(&block.plain);
    }
    if decoded.len() != decoded_size {
        return Err(Error::internal(
            "optimized Deflate storage candidate changed decoded size",
        ));
    }
    Ok((decoded, parsed.meaningful_bits, parsed.crc32))
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
mod tests;
