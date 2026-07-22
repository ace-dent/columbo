// SPDX-License-Identifier: MIT

//! Strict Deflate parser used by every optimization route.
//!
//! Parsing is deliberately completed before any optional search observes the
//! deadline.  A successful prefix result therefore always consumes one whole
//! stream; `timed_out` can never describe a partially decoded member.

use crate::checksum::crc32_update;
use crate::{Error, Result};

use super::bitstream::BitReader;
use super::huffman::{fixed_trees, Huffman};
use super::model::{
    DynamicPlan, OriginalBits, ParsedBlock, ParsedStream, RleToken, SourceBlockType, Token,
    CODE_LENGTH_ORDER, DISTANCE_BASE, DISTANCE_EXTRA_BITS, LENGTH_BASE, LENGTH_EXTRA_BITS,
};

type HuffmanPayload = (Vec<Token>, Vec<u8>, [u32; 286], [u32; 30]);
type StoredPayload = (
    Vec<Token>,
    Vec<u8>,
    [u32; 286],
    [u32; 30],
    Option<DynamicPlan>,
);

// Parsed blocks deliberately retain decoded bytes, tokens, frequency tables,
// and source metadata for later structural searches. A byte-only input limit
// cannot bound that richer representation: a tiny stream may contain hundreds
// of thousands of one-literal blocks. Keep the persistent model bounded so a
// hostile but valid stream returns an error instead of exhausting the process.
/// Shared ceiling for persistent parsing and optional transformed-token
/// candidates. Search imports this value so a match cannot expand into a
/// token vector larger than the model the parser itself is willing to keep.
pub(crate) const MAX_PARSED_MODEL_BYTES: usize = 256 * 1024 * 1024;
// Empty fixed blocks occupy only ten bits and are discarded from the retained
// model. Without a separate count limit, a relatively small hostile stream
// could therefore force millions of parser iterations while using almost no
// decoded-byte or model budget.
const MAX_SOURCE_BLOCKS: usize = 1_000_000;
const PARSED_BLOCK_MODEL_BYTES: usize = std::mem::size_of::<ParsedBlock>() + 4 * 1024;

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
        window: [0; 32_768],
        decoded_position: 0,
        decoded_limit,
        model_limit,
        model_tokens: 0,
        retained_blocks: 0,
        crc32: 0,
        adler_low: 1,
        adler_high: 0,
        max_distance: 0,
    };

    let mut blocks = Vec::new();
    let mut source_blocks = 0;
    let mut empty_blocks = 0;
    let mut saw_content = false;
    loop {
        if source_blocks >= MAX_SOURCE_BLOCKS {
            return Err(Error::new(
                "Deflate stream exceeds the source-block safety limit",
            ));
        }
        parser.retained_blocks = blocks.len();
        let (block, final_block) = parser.parse_block()?;
        source_blocks += 1;
        if block.plain.is_empty() {
            empty_blocks += 1;

            // Empty blocks have no effect on decoded bytes or history. Keep
            // one only while the stream might prove entirely empty; once a
            // content block exists, every empty block can be discarded as it
            // is parsed. This mirrors the C pending-block loop and prevents a
            // compact run of empty blocks from amplifying into large memory
            // use through per-block frequency tables.
            if !saw_content && blocks.is_empty() {
                blocks.try_reserve(1).map_err(|_| model_limit_error())?;
                blocks.push(block);
            }
        } else {
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
        blocks,
        consumed,
        meaningful_bits,
        crc32: parser.crc32,
        adler32: (parser.adler_high << 16) | parser.adler_low,
        decoded_size: parser.decoded_position,
        max_distance: parser.max_distance,
    })
}

struct Parser<'a> {
    reader: BitReader<'a>,
    window: [u8; 32_768],
    decoded_position: u64,
    decoded_limit: u64,
    model_limit: usize,
    model_tokens: usize,
    retained_blocks: usize,
    crc32: u32,
    adler_low: u32,
    adler_high: u32,
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

        let (tokens, plain, literal_frequencies, distance_frequencies, dynamic) = match block_type {
            SourceBlockType::Stored => self.parse_stored_block()?,
            SourceBlockType::Fixed => {
                let (literal, distance) = fixed_trees();
                let (tokens, plain, literal_frequencies, distance_frequencies) =
                    self.parse_huffman_payload(literal, distance)?;
                (
                    tokens,
                    plain,
                    literal_frequencies,
                    distance_frequencies,
                    None,
                )
            }
            SourceBlockType::Dynamic => {
                let (literal, distance, plan) = self.parse_dynamic_header()?;
                let (tokens, plain, literal_frequencies, distance_frequencies) =
                    self.parse_huffman_payload(&literal, &distance)?;
                (
                    tokens,
                    plain,
                    literal_frequencies,
                    distance_frequencies,
                    Some(plan),
                )
            }
        };

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
            distance[..plan.distance_lengths.len()].copy_from_slice(&plan.distance_lengths);
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

    fn parse_stored_block(&mut self) -> Result<StoredPayload> {
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
        Ok((tokens, plain, literal_frequencies, [0; 30], None))
    }

    fn parse_dynamic_header(&mut self) -> Result<(Huffman, Huffman, DynamicPlan)> {
        let hlit = self.reader.read(5)? as usize + 257;
        let hdist = self.reader.read(5)? as usize + 1;
        let hclen = self.reader.read(4)? as usize + 4;
        if hlit > 286 || hdist > 30 {
            return Err(Error::new("invalid dynamic Huffman header"));
        }

        let mut code_length_lengths = [0_u8; 19];
        for &symbol in &CODE_LENGTH_ORDER[..hclen] {
            code_length_lengths[symbol] = self.reader.read(3)? as u8;
        }
        let code_length_tree = Huffman::build(&code_length_lengths)
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
        let literal = Huffman::build(&literal_lengths)
            .ok_or_else(|| Error::new("invalid literal/length Huffman tree"))?;
        let distance = Huffman::build(&distance_lengths)
            .ok_or_else(|| Error::new("invalid distance Huffman tree"))?;
        if literal.code(256).is_none() {
            return Err(Error::new("dynamic Huffman tree has no end code"));
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
    ) -> Result<HuffmanPayload> {
        let mut tokens = Vec::new();
        let mut plain = Vec::new();
        let mut literal_frequencies = [0_u32; 286];
        let mut distance_frequencies = [0_u32; 30];

        loop {
            let symbol = literal_tree.decode(&mut self.reader)?;
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
                    let length_index = usize::from(symbol - 257);
                    let length_extra_bits = LENGTH_EXTRA_BITS[length_index];
                    let length_extra = self.reader.read(length_extra_bits)? as u16;
                    let length = LENGTH_BASE[length_index] + length_extra;

                    let distance_symbol = distance_tree.decode(&mut self.reader)?;
                    if distance_symbol > 29 {
                        return Err(Error::new("invalid distance code"));
                    }
                    let distance_index = usize::from(distance_symbol);
                    let distance_extra_bits = DISTANCE_EXTRA_BITS[distance_index];
                    let distance_extra = self.reader.read(distance_extra_bits)? as u16;
                    let distance = DISTANCE_BASE[distance_index] + distance_extra;
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
                        length_extra,
                        distance_extra,
                        length_extra_bits,
                        distance_extra_bits,
                    });

                    // Deflate copies overlap like `memmove`: each newly
                    // produced byte is immediately visible to the next one.
                    for _ in 0..length {
                        let source = (self.decoded_position - u64::from(distance)) & 32_767;
                        let byte = self.window[source as usize];
                        plain.push(byte);
                        self.push_history(byte);
                    }
                }
                _ => unreachable!(),
            }
        }

        Ok((tokens, plain, literal_frequencies, distance_frequencies))
    }

    fn reserve_decoded(&self, count: u64) -> Result<()> {
        if count > self.decoded_limit.saturating_sub(self.decoded_position) {
            return Err(Error::new("decoded data exceeds safety limit"));
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
        self.window[(self.decoded_position & 32_767) as usize] = byte;
        self.decoded_position += 1;
    }

    fn append_history(&mut self, bytes: &[u8]) {
        for &byte in bytes {
            self.push_history(byte);
        }
    }

    fn update_checksums(&mut self, bytes: &[u8]) {
        const MOD_ADLER: u32 = 65_521;
        self.crc32 = crc32_update(self.crc32, bytes);
        for &byte in bytes {
            self.adler_low += u32::from(byte);
            if self.adler_low >= MOD_ADLER {
                self.adler_low -= MOD_ADLER;
            }
            self.adler_high += self.adler_low;
            if self.adler_high >= MOD_ADLER {
                self.adler_high %= MOD_ADLER;
            }
        }
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
    Error::new("Deflate structure exceeds internal memory safety limit")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::deflate::bitstream::BitWriter;

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
        assert_eq!(parsed.blocks.len(), 1);
        assert_eq!(parsed.decoded_size, 0);
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
        let (literal, _) = fixed_trees();
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
