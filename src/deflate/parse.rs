// SPDX-License-Identifier: MIT
// SPDX-FileCopyrightText: 2026 Andrew C.E. Dent <https://github.com/ace-dent>

//! Validating Deflate parser used by every optimization route.
//!
//! Parsing finishes before any optional search checks the deadline, so a
//! successful prefix result covers one whole stream and `timed_out` never
//! describes a partly decoded member. The only accepted extension to RFC 1951
//! is Defluff's length-258 alias (symbol 284 with extra value 31).

use std::sync::{Arc, OnceLock};

use crate::checksum::{adler32_update, crc32_update};
use crate::{Error, Result};

use super::bitstream::BitReader;
use super::huffman::{
    payload_tree_shape_is_valid, CodeLengthDecoder, Huffman, DISTANCE_DECODE_ROOT_BITS,
    FIXED_DISTANCE_CODE_LENGTHS, FIXED_LITERAL_CODE_LENGTHS, LITERAL_LENGTH_DECODE_ROOT_BITS,
};
use super::model::{
    DynamicPlan, OriginalBits, ParsedBlock, ParsedStream, PlannedBlock, RleToken, SourceBlockType,
    Token, CODE_LENGTH_ORDER, DISTANCE_BASE, DISTANCE_EXTRA_BITS, LENGTH_BASE, LENGTH_EXTRA_BITS,
    RFC_DISTANCE_CODE_COUNT, TOKEN_CHARGE_BYTES, USABLE_DISTANCE_CODE_COUNT,
};

struct BlockPayload {
    tokens: Arc<Vec<Token>>,
    plain: Arc<Vec<u8>>,
    literal_frequencies: [u32; 286],
    distance_frequencies: [u32; 30],
    dynamic: Option<DynamicPlan>,
}

/// Parsed-model memory any stream may use, however small its input. It is also
/// the limit for one transformed-token candidate.
pub(crate) const BASE_PARSED_MODEL_BYTES: usize = 256 * 1024 * 1024;
/// Parsed-model memory a stream may use while its block records stay within
/// [`BLOCK_RECORD_EXPANSION_RATIO`]: 4 GiB, enough for a stream at the 1 GiB
/// decoded limit whose tokens average five decoded bytes. A 32-bit target
/// keeps the base limit.
pub(crate) const MAX_PARSED_MODEL_BYTES: usize = if usize::BITS > 32 {
    BASE_PARSED_MODEL_BYTES << 4
} else {
    BASE_PARSED_MODEL_BYTES
};
/// Block-record bytes allowed per compressed byte read, beyond the base
/// limit. Records are the one part of the model that a small stream can
/// inflate: decoded bytes have their own limit and a token costs at least one
/// bit, but a one-literal fixed block charges a whole record for 18 bits. The
/// decoded-data expansion ratio (`MAX_EXPANSION_RATIO`) is reused, so records
/// expand no further than decoded data may; blocks that average 12 or more
/// compressed bytes always qualify.
const BLOCK_RECORD_EXPANSION_RATIO: usize = crate::options::MAX_EXPANSION_RATIO as usize;
/// Empty blocks (10 bits when fixed) are dropped from the model, so this bounds
/// parser work on a small hostile stream.
const MAX_SOURCE_BLOCKS: usize = 262_144;
const PARSED_BLOCK_MODEL_BYTES: usize = std::mem::size_of::<ParsedBlock>() + 4 * 1024;
const WINDOW_SIZE: usize = 32_768;
const WINDOW_MASK: u64 = (WINDOW_SIZE - 1) as u64;
const MAX_MATCH_LENGTH: usize = 258;

/// The two parsed-model limits of one parse.
#[derive(Clone, Copy)]
struct ModelLimits {
    /// Applies to every stream.
    base: usize,
    /// Applies while block records are proportionate to the compressed bytes.
    scaled: usize,
}

impl ModelLimits {
    const STREAM: Self = Self {
        base: BASE_PARSED_MODEL_BYTES,
        scaled: MAX_PARSED_MODEL_BYTES,
    };

    /// Model bytes left for tokens and decoded data once `block_count`
    /// records are charged against the first `compressed_bytes` of the
    /// stream. `None` when the records alone exceed the limit.
    fn payload_limit(self, block_count: usize, compressed_bytes: u64) -> Option<usize> {
        let record_bytes = block_count.checked_mul(PARSED_BLOCK_MODEL_BYTES)?;
        let proportionate = usize::try_from(compressed_bytes)
            .ok()
            .and_then(|bytes| bytes.checked_mul(BLOCK_RECORD_EXPANSION_RATIO))
            .map_or(true, |allowed| record_bytes <= allowed);
        let limit = if proportionate {
            self.scaled.max(self.base)
        } else {
            self.base
        };
        limit.checked_sub(record_bytes)
    }
}

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

/// Model size charged against the parsed-model limits; search candidates use
/// the same formula against [`BASE_PARSED_MODEL_BYTES`].
pub(crate) fn parsed_model_bytes(
    decoded_bytes: usize,
    token_count: usize,
    block_count: usize,
) -> Option<usize> {
    token_count
        .checked_mul(TOKEN_CHARGE_BYTES)
        .and_then(|bytes| bytes.checked_add(decoded_bytes))
        .and_then(|bytes| {
            block_count
                .checked_mul(PARSED_BLOCK_MODEL_BYTES)
                .and_then(|block_bytes| bytes.checked_add(block_bytes))
        })
}

/// Whether one stream of non-empty blocks with these totals stays within
/// [`parse_stream`]'s source-block limit and its base model limit, which
/// holds whatever the stream's compressed size turns out to be.
pub(crate) fn fits_parser_limits(
    decoded_bytes: u64,
    token_count: usize,
    block_count: usize,
) -> bool {
    block_count <= MAX_SOURCE_BLOCKS
        && usize::try_from(decoded_bytes)
            .ok()
            .and_then(|decoded| parsed_model_bytes(decoded, token_count, block_count))
            .is_some_and(|bytes| bytes <= BASE_PARSED_MODEL_BYTES)
}

pub(crate) fn parse_stream(input: &[u8], decoded_limit: u64) -> Result<ParsedStream> {
    parse_stream_with_model_limits(input, decoded_limit, ModelLimits::STREAM)
}

/// [`parse_stream`] for bytes emitted from `plans`: a block that reproduces
/// the tokens and decoded bytes of the plan starting at its decoded offset
/// shares that plan's buffers instead of holding a copy. Validation and limits
/// are unchanged; only where the payload is stored differs.
pub(crate) fn parse_stream_sharing(
    input: &[u8],
    decoded_limit: u64,
    plans: &[PlannedBlock],
) -> Result<ParsedStream> {
    parse_stream_with_padding(input, decoded_limit, ModelLimits::STREAM, plans, |_, _| {
        Ok(())
    })
}

fn parse_stream_with_model_limits(
    input: &[u8],
    decoded_limit: u64,
    model_limits: ModelLimits,
) -> Result<ParsedStream> {
    parse_stream_with_padding(input, decoded_limit, model_limits, &[], |_, _| Ok(()))
}

/// Parse and validate every block, calling `padding(start, end)` for each bit
/// range a decoder ignores: stored-block alignment and final-byte padding.
fn parse_stream_with_padding<'a>(
    input: &'a [u8],
    decoded_limit: u64,
    model_limits: ModelLimits,
    expected_plans: &'a [PlannedBlock],
    mut padding: impl FnMut(u64, u64) -> Result<()>,
) -> Result<ParsedStream> {
    let mut parser = Parser {
        reader: BitReader::new(input),
        expected_plans,
        expected_index: 0,
        expected_start: 0,
        window: [0; WINDOW_SIZE],
        decoded_position: 0,
        decoded_limit,
        model_limits,
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
        ModelLimits::STREAM,
        &[],
        |start, end| {
            if start != end {
                let offset = (start / 8) as usize;
                let mask = (0xff_u16 << (start % 8)) as u8;
                if input[offset] & mask != 0 {
                    masks
                        .try_reserve(1)
                        .map_err(|_| Error::allocation("padding masks"))?;
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
    /// Plans whose buffers matching blocks share, with the index and decoded
    /// offset of the first plan not yet passed.
    expected_plans: &'a [PlannedBlock],
    expected_index: usize,
    expected_start: u64,
    window: [u8; WINDOW_SIZE],
    decoded_position: u64,
    decoded_limit: u64,
    model_limits: ModelLimits,
    model_payload_bytes: usize,
    current_model_payload_limit: Option<usize>,
    retained_blocks: usize,
    crc32: u32,
    adler32: u32,
    max_distance: u16,
}

impl<'a> Parser<'a> {
    fn parse_block(&mut self) -> Result<(ParsedBlock, bool)> {
        let start = self.reader.bit_position();
        self.refresh_model_payload_limit();
        let expected = self.expected_block();
        let final_block = self.reader.read(1)? != 0;
        let block_type = match self.reader.read(2)? {
            0 => SourceBlockType::Stored,
            1 => SourceBlockType::Fixed,
            2 => SourceBlockType::Dynamic,
            _ => return Err(Error::new("invalid Deflate block type")),
        };

        let payload = match block_type {
            SourceBlockType::Stored => self.parse_stored_block(expected)?,
            SourceBlockType::Fixed => {
                let (literal, distance) = fixed_payload_trees();
                self.parse_huffman_payload(literal, distance, expected)?
            }
            SourceBlockType::Dynamic => {
                let (literal, distance, plan) = self.parse_dynamic_header()?;
                let mut payload = self.parse_huffman_payload(&literal, &distance, expected)?;
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
                tokens,
                plain,
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

    fn parse_stored_block(&mut self, expected: Option<&PlannedBlock>) -> Result<BlockPayload> {
        self.reader.align_to_byte()?;
        let length = self.reader.read(16)? as u16;
        let complement = self.reader.read(16)? as u16;
        if length ^ complement != u16::MAX {
            return Err(Error::new("bad stored block length"));
        }

        // The payload is borrowed, so it counts as read before it is charged.
        let source = self.reader.read_aligned_bytes(usize::from(length))?;
        self.reserve_payload(u64::from(length), usize::from(length))?;
        let mut literal_frequencies = [0_u32; 286];
        for &byte in source {
            literal_frequencies[usize::from(byte)] += 1;
        }
        literal_frequencies[256] = 1;
        let (tokens, plain) = match expected {
            Some(expected)
                if expected.plain.as_slice() == source
                    && expected.tokens.len() == source.len()
                    && expected
                        .tokens
                        .iter()
                        .zip(source)
                        .all(|(&token, &byte)| token == Token::Literal(byte)) =>
            {
                (Arc::clone(&expected.tokens), Arc::clone(&expected.plain))
            }
            _ => {
                let mut plain = Vec::new();
                plain
                    .try_reserve_exact(source.len())
                    .map_err(|_| model_limit_error())?;
                plain.extend_from_slice(source);
                let mut tokens = Vec::new();
                tokens
                    .try_reserve_exact(source.len())
                    .map_err(|_| model_limit_error())?;
                tokens.extend(source.iter().map(|&byte| Token::Literal(byte)));
                (Arc::new(tokens), Arc::new(plain))
            }
        };
        self.append_history(source);
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
        expected: Option<&PlannedBlock>,
    ) -> Result<BlockPayload> {
        let mut buffers = PayloadBuffers::new(expected);
        let mut literal_frequencies = [0_u32; 286];
        let mut distance_frequencies = [0_u32; 30];
        // `expand_match` writes every byte it returns, so one buffer serves
        // every match.
        let mut scratch = [0_u8; MAX_MATCH_LENGTH];

        loop {
            let decoded = literal_tree.decode_value_inline(&mut self.reader)?;
            let symbol = decoded.symbol;
            if symbol > 285 {
                return Err(Error::new("invalid literal/length code"));
            }
            // The model limits keep every count far below `u32::MAX`.
            let count = &mut literal_frequencies[usize::from(symbol)];
            *count = count.wrapping_add(1);
            match symbol {
                0..=255 => {
                    self.reserve_payload(1, 1)?;
                    let byte = symbol as u8;
                    buffers.push_literal(byte)?;
                    self.push_history(byte);
                }
                256 => break,
                257..=285 => {
                    let length = decoded.value;

                    let decoded_distance = distance_tree.decode_value_inline(&mut self.reader)?;
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
                    let count = &mut distance_frequencies[distance_index];
                    *count = count.wrapping_add(1);
                    let token = Token::Match {
                        length,
                        distance,
                        length_symbol: symbol,
                        distance_symbol: distance_symbol as u8,
                    };

                    let expanded = expand_match(
                        &self.window,
                        self.decoded_position,
                        distance,
                        length,
                        &mut scratch,
                    );
                    buffers.push_match(token, expanded)?;
                    self.append_history(expanded);
                }
                _ => unreachable!(),
            }
        }

        let (tokens, plain) = buffers.finish()?;
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
                    .checked_mul(TOKEN_CHARGE_BYTES)
                    .and_then(|tokens| decoded.checked_add(tokens))
            })
            .ok_or_else(model_limit_error)?;
        let model_payload_bytes = self
            .model_payload_bytes
            .checked_add(added_model_bytes)
            .ok_or_else(model_limit_error)?;
        let exceeds =
            |limit: Option<usize>| limit.map_or(true, |limit| model_payload_bytes > limit);
        if exceeds(self.current_model_payload_limit) {
            // The current block's own bytes may now pay for its record.
            self.refresh_model_payload_limit();
            if exceeds(self.current_model_payload_limit) {
                return Err(model_limit_error());
            }
        }
        self.model_payload_bytes = model_payload_bytes;
        Ok(())
    }

    /// Set the payload limit for the current block, whose record is charged
    /// against every compressed byte read so far, its own included. Records
    /// stay fixed within a block while bytes read only grow, so the limit can
    /// only rise until the next block.
    fn refresh_model_payload_limit(&mut self) {
        self.current_model_payload_limit = self.retained_blocks.checked_add(1).and_then(|blocks| {
            self.model_limits
                .payload_limit(blocks, self.reader.bit_position() / 8)
        });
    }

    /// The expected plan that starts at the current decoded offset, if any.
    /// Plans are walked once, in decoded order; empty plans and plans that
    /// start before this offset are passed.
    fn expected_block(&mut self) -> Option<&'a PlannedBlock> {
        let plans = self.expected_plans;
        while let Some(plan) = plans.get(self.expected_index) {
            if self.expected_start >= self.decoded_position && !plan.plain.is_empty() {
                break;
            }
            self.expected_start += plan.plain.len() as u64;
            self.expected_index += 1;
        }
        let plan = plans.get(self.expected_index)?;
        (self.expected_start == self.decoded_position).then_some(plan)
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

/// A block's token and decoded-byte buffers.
type SharedPayload = (Arc<Vec<Token>>, Arc<Vec<u8>>);

/// A Huffman block's tokens and decoded bytes. While they reproduce an
/// expected plan's buffers they are compared rather than stored, and the block
/// then shares those buffers. At the first difference the reproduced prefix is
/// copied and the block continues in its own buffers.
struct PayloadBuffers<'a> {
    tokens: Vec<Token>,
    plain: Vec<u8>,
    expected: Option<&'a PlannedBlock>,
    /// Tokens and decoded bytes of `expected` reproduced so far.
    matched_tokens: usize,
    matched_plain: usize,
}

impl<'a> PayloadBuffers<'a> {
    fn new(expected: Option<&'a PlannedBlock>) -> Self {
        Self {
            tokens: Vec::new(),
            plain: Vec::new(),
            expected,
            matched_tokens: 0,
            matched_plain: 0,
        }
    }

    #[inline(always)]
    fn push_literal(&mut self, byte: u8) -> Result<()> {
        if let Some(expected) = self.expected {
            if expected.tokens.get(self.matched_tokens) == Some(&Token::Literal(byte))
                && expected.plain.get(self.matched_plain) == Some(&byte)
            {
                self.matched_tokens += 1;
                self.matched_plain += 1;
                return Ok(());
            }
            self.copy_reproduced()?;
        }
        self.tokens
            .try_reserve(1)
            .map_err(|_| model_limit_error())?;
        self.plain.try_reserve(1).map_err(|_| model_limit_error())?;
        self.tokens.push(Token::Literal(byte));
        self.plain.push(byte);
        Ok(())
    }

    #[inline(always)]
    fn push_match(&mut self, token: Token, expanded: &[u8]) -> Result<()> {
        if let Some(expected) = self.expected {
            let plain_end = self.matched_plain + expanded.len();
            if expected.tokens.get(self.matched_tokens) == Some(&token)
                && expected.plain.get(self.matched_plain..plain_end) == Some(expanded)
            {
                self.matched_tokens += 1;
                self.matched_plain = plain_end;
                return Ok(());
            }
            self.copy_reproduced()?;
        }
        self.tokens
            .try_reserve(1)
            .map_err(|_| model_limit_error())?;
        self.plain
            .try_reserve(expanded.len())
            .map_err(|_| model_limit_error())?;
        self.tokens.push(token);
        self.plain.extend_from_slice(expanded);
        Ok(())
    }

    /// Stop comparing and copy the reproduced prefix into own buffers.
    #[cold]
    #[inline(never)]
    fn copy_reproduced(&mut self) -> Result<()> {
        let Some(expected) = self.expected.take() else {
            return Ok(());
        };
        let tokens = &expected.tokens[..self.matched_tokens];
        let plain = &expected.plain[..self.matched_plain];
        self.tokens
            .try_reserve(tokens.len())
            .map_err(|_| model_limit_error())?;
        self.plain
            .try_reserve(plain.len())
            .map_err(|_| model_limit_error())?;
        self.tokens.extend_from_slice(tokens);
        self.plain.extend_from_slice(plain);
        Ok(())
    }

    /// The expected plan's buffers when the block reproduced all of them,
    /// otherwise the block's own.
    fn finish(mut self) -> Result<SharedPayload> {
        if let Some(expected) = self.expected {
            if self.matched_tokens == expected.tokens.len()
                && self.matched_plain == expected.plain.len()
            {
                return Ok((Arc::clone(&expected.tokens), Arc::clone(&expected.plain)));
            }
            self.copy_reproduced()?;
        }
        Ok((Arc::new(self.tokens), Arc::new(self.plain)))
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
        .map_err(|_| Error::allocation("stored Deflate payload"))?;
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
