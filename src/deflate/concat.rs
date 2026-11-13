// SPDX-License-Identifier: MIT
// SPDX-FileCopyrightText: 2026 Andrew C.E. Dent <https://github.com/ace-dent>

//! Joining complete raw Deflate streams into one stream.
//!
//! The joined stream decodes to the parts' output in order. Each non-empty
//! block keeps its bits except BFINAL, which only the last block sets; a
//! stored block is written again at its new position, and empty blocks are
//! dropped. A match in a later part reaches back only into that part's own
//! output, which the earlier output in front of it does not change.
//!
//! The result is never longer than the parts written separately, each padded
//! to a byte. Huffman-coded bits do not depend on their position, so a block
//! that starts earlier also ends earlier, and a stored block that starts
//! earlier ends on the same or an earlier byte boundary.
//!
//! Joining also makes blocks of different parts neighbors. Each part's own
//! terminal merges (R16) priced every other pair of neighbors, so only the
//! pairs at the boundaries between parts get the R16 pass, and a merge is kept
//! only when it is strictly smaller.

use std::sync::Arc;

use crate::checksum::crc32_update;
use crate::{Error, Result};

use crate::Options;

use super::bitstream::BitWriter;
use super::block::emit_block;
use super::model::{ParsedStream, PlannedBlock, Representation, SourceBlockType};
use super::parse::{fits_parser_limits, parse_stream};
use super::stop::SearchStop;
use super::stream::plan_boundary_merges;

/// A complete raw stream parsed for [`StreamJoiner`].
pub(crate) struct JoinPart<'a> {
    input: &'a [u8],
    parsed: ParsedStream,
}

impl<'a> JoinPart<'a> {
    /// Parse a complete raw stream that decodes to exactly `decoded_size`
    /// bytes, such as a validated optimization result.
    pub(crate) fn parse(input: &'a [u8], decoded_size: u64) -> Result<Self> {
        let parsed = parse_stream(input, decoded_size)?;
        if parsed.consumed != input.len() || parsed.decoded_size != decoded_size {
            return Err(Error::internal(
                "a Deflate stream to join does not match its decoded size",
            ));
        }
        Ok(Self { input, parsed })
    }

    /// Tokens and blocks the part adds to a joined stream.
    fn counts(&self) -> (usize, usize) {
        self.parsed
            .blocks
            .iter()
            .filter(|block| !block.plain.is_empty())
            .fold((0, 0), |(tokens, blocks), block| {
                (tokens + block.tokens.len(), blocks + 1)
            })
    }
}

/// A raw stream joined from complete parts.
pub(crate) struct JoinedStream {
    pub(crate) data: Vec<u8>,
    pub(crate) meaningful_bits: u64,
    pub(crate) crc32: u32,
    pub(crate) decoded_size: u64,
}

/// Joins complete raw streams, in the order pushed, into one raw stream.
#[derive(Default)]
pub(crate) struct StreamJoiner<'a> {
    writer: BitWriter,
    /// The latest block, written once it is known whether it is the last.
    pending: Option<(&'a [u8], PlannedBlock)>,
    parts: usize,
    decoded_size: u64,
    token_count: usize,
    block_count: usize,
    crc32: u32,
    /// Indices of the blocks that start a part after an earlier block.
    boundaries: Vec<usize>,
}

impl<'a> StreamJoiner<'a> {
    pub(crate) fn is_empty(&self) -> bool {
        self.parts == 0
    }

    /// Whether `part` can join while the joined stream stays within the
    /// parser's base model limit and block limit, so that Columbo can check it
    /// and parse it again whatever its compressed size. A part that exceeds
    /// the base limit was admitted under the scaled limit; it forms a run of
    /// one and keeps its own stream.
    pub(crate) fn fits(&self, part: &JoinPart<'_>) -> bool {
        let (tokens, blocks) = part.counts();
        match (
            self.decoded_size.checked_add(part.parsed.decoded_size),
            self.token_count.checked_add(tokens),
            self.block_count.checked_add(blocks),
        ) {
            (Some(decoded), Some(tokens), Some(blocks)) => {
                fits_parser_limits(decoded, tokens, blocks)
            }
            _ => false,
        }
    }

    pub(crate) fn push(&mut self, part: JoinPart<'a>) -> Result<()> {
        let JoinPart { input, parsed } = part;
        if self.block_count != 0 && parsed.blocks.iter().any(|block| !block.plain.is_empty()) {
            self.boundaries
                .try_reserve(1)
                .map_err(|_| Error::allocation("joined Deflate boundaries"))?;
            self.boundaries.push(self.block_count);
        }
        self.decoded_size = self
            .decoded_size
            .checked_add(parsed.decoded_size)
            .ok_or_else(|| Error::resource_limit("joined Deflate stream is too large"))?;
        for block in parsed.blocks {
            if block.plain.is_empty() {
                continue;
            }
            let original = block
                .original
                .ok_or_else(|| Error::internal("a Deflate block to join has no source bits"))?;
            self.crc32 = crc32_update(self.crc32, &block.plain);
            self.token_count += block.tokens.len();
            self.block_count += 1;
            // A stored block's padding depends on where it starts; every other
            // block is copied bit for bit.
            let representation = if original.block_type == SourceBlockType::Stored {
                Representation::Stored
            } else {
                Representation::Original(original)
            };
            let planned = PlannedBlock {
                tokens: block.tokens,
                plain: block.plain,
                representation,
                bits: original.len,
                source_type: block.source_type,
            };
            if let Some((previous_input, previous)) = self.pending.replace((input, planned)) {
                emit_block(&mut self.writer, previous_input, &previous, false)?;
            }
        }
        self.parts += 1;
        Ok(())
    }

    /// Write the last block as final, check that the joined stream decodes to
    /// the parts' output, then run R16 at the boundaries between parts until
    /// `expired` reports true, within R16's work budget.
    pub(crate) fn finish(
        mut self,
        options: &Options,
        expired: &mut dyn FnMut() -> bool,
    ) -> Result<JoinedStream> {
        match self.pending.take() {
            Some((input, last)) => emit_block(&mut self.writer, input, &last, true)?,
            None => {
                // Every part decodes to nothing: one empty fixed block.
                let empty = PlannedBlock {
                    tokens: Arc::new(Vec::new()),
                    plain: Arc::new(Vec::new()),
                    representation: Representation::Fixed,
                    bits: 10,
                    source_type: SourceBlockType::Fixed,
                };
                emit_block(&mut self.writer, &[], &empty, true)?;
            }
        }
        let meaningful_bits = self.writer.bit_position();
        let data = std::mem::take(&mut self.writer).into_bytes();
        let parsed = self.check(&data, meaningful_bits)?;
        let mut joined = JoinedStream {
            data,
            meaningful_bits,
            crc32: self.crc32,
            decoded_size: self.decoded_size,
        };
        if self.boundaries.is_empty() {
            return Ok(joined);
        }

        let mut stop = SearchStop::callback(expired);
        let Some(plans) =
            plan_boundary_merges(&parsed.blocks, &self.boundaries, options, &mut stop)
        else {
            return Ok(joined);
        };
        drop(parsed);
        let mut writer = BitWriter::default();
        for (index, plan) in plans.iter().enumerate() {
            emit_block(&mut writer, &joined.data, plan, index + 1 == plans.len())?;
        }
        let merged_bits = writer.bit_position();
        if merged_bits < joined.meaningful_bits {
            let data = writer.into_bytes();
            self.check(&data, merged_bits)?;
            joined.data = data;
            joined.meaningful_bits = merged_bits;
        }
        Ok(joined)
    }

    /// Parse a joined stream and check that it decodes to the parts' output.
    fn check(&self, data: &[u8], meaningful_bits: u64) -> Result<ParsedStream> {
        let parsed = parse_stream(data, self.decoded_size)?;
        if parsed.consumed != data.len()
            || parsed.decoded_size != self.decoded_size
            || parsed.crc32 != self.crc32
            || parsed.meaningful_bits != meaningful_bits
        {
            return Err(Error::internal(
                "joined Deflate stream does not decode to its parts",
            ));
        }
        Ok(parsed)
    }
}

#[cfg(test)]
mod tests;
