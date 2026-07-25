// SPDX-License-Identifier: MIT

//! Internal, lossless representation of a parsed Deflate stream.
//!
//! Columbo is a structural optimizer: it keeps the compressor's LZ77 parse
//! and searches for cheaper ways to serialize it. The model therefore keeps
//! both the decoded match values and the exact symbols/extras used on input.

use std::sync::Arc;

use super::huffman::huffman_tree_shape_is_complete;

/// RFC 1951 advertises distance-code lengths for symbols 0 through 31.
///
/// Symbols 30 and 31 participate in Huffman-tree construction but are
/// reserved and must never occur in a compressed payload.
pub(crate) const RFC_DISTANCE_CODE_COUNT: usize = 32;
pub(crate) const USABLE_DISTANCE_CODE_COUNT: usize = 30;
pub(crate) const MAX_DYNAMIC_CODE_LENGTH_COUNT: usize = 286 + RFC_DISTANCE_CODE_COUNT;

pub(crate) const LENGTH_BASE: [u16; 29] = [
    3, 4, 5, 6, 7, 8, 9, 10, 11, 13, 15, 17, 19, 23, 27, 31, 35, 43, 51, 59, 67, 83, 99, 115, 131,
    163, 195, 227, 258,
];

pub(crate) const LENGTH_EXTRA_BITS: [u8; 29] = [
    0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 2, 2, 2, 2, 3, 3, 3, 3, 4, 4, 4, 4, 5, 5, 5, 5, 0,
];

pub(crate) const DISTANCE_BASE: [u16; 30] = [
    1, 2, 3, 4, 5, 7, 9, 13, 17, 25, 33, 49, 65, 97, 129, 193, 257, 385, 513, 769, 1025, 1537,
    2049, 3073, 4097, 6145, 8193, 12289, 16385, 24577,
];

pub(crate) const DISTANCE_EXTRA_BITS: [u8; 30] = [
    0, 0, 0, 0, 1, 1, 2, 2, 3, 3, 4, 4, 5, 5, 6, 6, 7, 7, 8, 8, 9, 9, 10, 10, 11, 11, 12, 12, 13,
    13,
];

pub(crate) const CODE_LENGTH_ORDER: [usize; 19] = [
    16, 17, 18, 0, 8, 7, 9, 6, 10, 5, 11, 4, 12, 3, 13, 2, 14, 1, 15,
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum Token {
    Literal(u8),
    Match {
        length: u16,
        distance: u16,
        length_symbol: u16,
        distance_symbol: u8,
        length_extra: u16,
        distance_extra: u16,
        length_extra_bits: u8,
        distance_extra_bits: u8,
    },
}

impl Token {
    pub(crate) fn decoded_len(self) -> usize {
        match self {
            Self::Literal(_) => 1,
            Self::Match { length, .. } => usize::from(length),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct RleToken {
    pub(crate) symbol: u8,
    pub(crate) extra: u8,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DynamicPlan {
    pub(crate) literal_lengths: Vec<u8>,
    pub(crate) distance_lengths: Vec<u8>,
    pub(crate) code_length_lengths: [u8; 19],
    pub(crate) rle: Vec<RleToken>,
    pub(crate) hlit: usize,
    pub(crate) hdist: usize,
    pub(crate) hclen: usize,
    /// Complete block cost, including the three-bit block header.
    pub(crate) bits: u64,
}

impl DynamicPlan {
    /// Whether the payload advertises at least two usable distance symbols.
    ///
    /// Dynamic headers may also carry lengths for reserved symbols 30 and 31;
    /// those do not satisfy Columbo's strict decoder-compatibility policy and
    /// are deliberately excluded from this count.
    pub(crate) fn has_two_usable_distance_codes(&self) -> bool {
        self.distance_lengths
            .iter()
            .take(USABLE_DISTANCE_CODE_COUNT)
            .filter(|&&length| length != 0)
            .count()
            >= 2
    }

    /// Whether all three dynamic alphabets follow conservative decoder
    /// practice rather than RFC 1951's empty/singleton exceptions.
    ///
    /// This mirrors the compatibility fix made for libdeflate issue #323:
    /// literal/length, distance, and code-length trees must all occupy their
    /// complete Kraft code space. Distance symbols 30 and 31 remain reserved,
    /// so at least two usable distance symbols are required as well.
    pub(crate) fn has_strictly_compatible_huffman_codes(&self) -> bool {
        huffman_tree_shape_is_complete(&self.literal_lengths)
            && huffman_tree_shape_is_complete(&self.distance_lengths)
            && huffman_tree_shape_is_complete(&self.code_length_lengths)
            && self.has_two_usable_distance_codes()
    }

    /// Clone the small, owned Huffman-table vectors without relying on an
    /// infallible allocator. Optional searches can abandon a candidate when
    /// memory is tight while retaining the already-valid source plan.
    pub(crate) fn try_clone(&self) -> Option<Self> {
        Some(Self {
            literal_lengths: try_clone_slice(&self.literal_lengths)?,
            distance_lengths: try_clone_slice(&self.distance_lengths)?,
            code_length_lengths: self.code_length_lengths,
            rle: try_clone_slice(&self.rle)?,
            hlit: self.hlit,
            hdist: self.hdist,
            hclen: self.hclen,
            bits: self.bits,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SourceBlockType {
    Stored,
    Fixed,
    Dynamic,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct OriginalBits {
    pub(crate) start: u64,
    pub(crate) len: u64,
    pub(crate) alignment: u8,
    pub(crate) block_type: SourceBlockType,
}

#[derive(Debug, Clone)]
pub(crate) struct ParsedBlock {
    // Immutable payload buffers are shared by parsed blocks and plans. Most
    // planning choices only change representation metadata; sharing avoids
    // retaining several full copies of a near-limit decoded stream.
    pub(crate) tokens: Arc<Vec<Token>>,
    pub(crate) plain: Arc<Vec<u8>>,
    pub(crate) literal_frequencies: [u32; 286],
    pub(crate) distance_frequencies: [u32; 30],
    pub(crate) original_literal_lengths: Option<[u8; 286]>,
    pub(crate) original_distance_lengths: Option<[u8; 30]>,
    pub(crate) original_dynamic: Option<DynamicPlan>,
    pub(crate) original: Option<OriginalBits>,
    /// Boundaries inherited when adjacent source blocks are combined.
    pub(crate) source_splits: Vec<usize>,
    pub(crate) source_type: SourceBlockType,
}

impl ParsedBlock {
    pub(crate) fn recount_frequencies(&mut self) {
        let (literal, distance) = count_frequencies(&self.tokens);
        self.literal_frequencies = literal;
        self.distance_frequencies = distance;
    }

    /// Copy mutable block metadata while sharing the immutable token/plain
    /// payloads. This is used only by optional preparation and grouping paths.
    pub(crate) fn try_clone_shared(&self) -> Option<Self> {
        Some(Self {
            tokens: Arc::clone(&self.tokens),
            plain: Arc::clone(&self.plain),
            literal_frequencies: self.literal_frequencies,
            distance_frequencies: self.distance_frequencies,
            original_literal_lengths: self.original_literal_lengths,
            original_distance_lengths: self.original_distance_lengths,
            original_dynamic: match &self.original_dynamic {
                Some(dynamic) => Some(dynamic.try_clone()?),
                None => None,
            },
            original: self.original,
            source_splits: try_clone_slice(&self.source_splits)?,
            source_type: self.source_type,
        })
    }
}

#[derive(Debug, Clone)]
pub(crate) enum Representation {
    Original(OriginalBits),
    Stored,
    Fixed,
    Dynamic(DynamicPlan),
}

impl Representation {
    pub(crate) fn try_clone(&self) -> Option<Self> {
        Some(match self {
            Self::Original(original) => Self::Original(*original),
            Self::Stored => Self::Stored,
            Self::Fixed => Self::Fixed,
            Self::Dynamic(dynamic) => Self::Dynamic(dynamic.try_clone()?),
        })
    }
}

#[derive(Debug, Clone)]
pub(crate) struct PlannedBlock {
    pub(crate) tokens: Arc<Vec<Token>>,
    pub(crate) plain: Arc<Vec<u8>>,
    pub(crate) representation: Representation,
    pub(crate) bits: u64,
    pub(crate) source_type: SourceBlockType,
}

#[derive(Debug)]
pub(crate) struct ParsedStream {
    pub(crate) blocks: Vec<ParsedBlock>,
    pub(crate) consumed: usize,
    pub(crate) meaningful_bits: u64,
    pub(crate) crc32: u32,
    pub(crate) adler32: u32,
    pub(crate) decoded_size: u64,
    /// Largest backward distance used by the source stream. A zlib wrapper
    /// may advertise a window smaller than raw Deflate's 32 KiB maximum.
    pub(crate) max_distance: u16,
    pub(crate) source_block_count: usize,
    pub(crate) source_empty_block_count: usize,
    pub(crate) source_trailing_empty_block_count: usize,
}

pub(crate) fn count_frequencies(tokens: &[Token]) -> ([u32; 286], [u32; 30]) {
    let mut literal = [0_u32; 286];
    let mut distance = [0_u32; 30];
    for token in tokens {
        match *token {
            Token::Literal(value) => literal[usize::from(value)] += 1,
            Token::Match {
                length_symbol,
                distance_symbol,
                ..
            } => {
                literal[usize::from(length_symbol)] += 1;
                distance[usize::from(distance_symbol)] += 1;
            }
        }
    }
    literal[256] += 1;
    (literal, distance)
}

pub(crate) fn token_extra_bits(tokens: &[Token]) -> u64 {
    tokens
        .iter()
        .map(|token| match token {
            Token::Literal(_) => 0,
            Token::Match {
                length_extra_bits,
                distance_extra_bits,
                ..
            } => u64::from(*length_extra_bits) + u64::from(*distance_extra_bits),
        })
        .sum()
}

/// Fallibly copy a slice for optional planning routes.
pub(crate) fn try_clone_slice<T: Copy>(source: &[T]) -> Option<Vec<T>> {
    let mut output = Vec::new();
    output.try_reserve_exact(source.len()).ok()?;
    output.extend_from_slice(source);
    Some(output)
}
