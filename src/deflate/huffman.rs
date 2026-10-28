// SPDX-License-Identifier: MIT
// SPDX-FileCopyrightText: 2026 Andrew C.E. Dent <https://github.com/ace-dent>

//! Canonical Huffman codes, decoding tables and length-limited code
//! construction.

use std::collections::VecDeque;
use std::sync::OnceLock;

use crate::{Error, Result};

use super::bitstream::BitReader;

const MAX_CODE_BITS: usize = 15;
const MAX_C_CODES: usize = 320;
const MAX_TRACKED_DEPTH: usize = 63;
const COMPLETE_HUFFMAN_CODE_SPACE: u32 = 1 << MAX_CODE_BITS;
const CODE_LENGTH_DECODE_BITS: u8 = 7;
const CODE_LENGTH_DECODE_SIZE: usize = 1 << CODE_LENGTH_DECODE_BITS;
/// Root table widths of the two-level decoders: one bit more than each alphabet
/// needs, chosen from measured build and cache costs.
pub(crate) const LITERAL_LENGTH_DECODE_ROOT_BITS: u8 = 10;
pub(crate) const DISTANCE_DECODE_ROOT_BITS: u8 = 6;

const fn make_fixed_literal_code_lengths() -> [u8; 288] {
    let mut lengths = [0_u8; 288];
    let mut symbol = 0;
    while symbol < lengths.len() {
        lengths[symbol] = if symbol < 144 {
            8
        } else if symbol < 256 {
            9
        } else if symbol < 280 {
            7
        } else {
            8
        };
        symbol += 1;
    }
    lengths
}

pub(crate) const FIXED_LITERAL_CODE_LENGTHS: [u8; 288] = make_fixed_literal_code_lengths();
pub(crate) const FIXED_DISTANCE_CODE_LENGTHS: [u8; 32] = [5; 32];

/// Whether the code lengths form a complete prefix code (Kraft sum exactly 1).
// Out of line: each call scans the lengths.
#[inline(never)]
pub(crate) fn huffman_tree_shape_is_complete(lengths: &[u8]) -> bool {
    // Branch-free, so the loop vectorizes: a zero length adds nothing, and a
    // length over the limit fails the final test whatever it adds. A `u32`
    // sum of 2^16 lengths of at most 2^15 each cannot overflow, and wrapping
    // additions keep release overflow checks out of the loop.
    let mut occupied = 0_u64;
    let mut longest = 0_u8;
    for chunk in lengths.chunks(1 << 16) {
        let mut chunk_occupied = 0_u32;
        for &length in chunk {
            longest = longest.max(length);
            chunk_occupied = chunk_occupied.wrapping_add(
                (u32::from(length != 0) << MAX_CODE_BITS) >> length.min(MAX_CODE_BITS as u8),
            );
        }
        occupied = occupied.wrapping_add(u64::from(chunk_occupied));
    }
    usize::from(longest) <= MAX_CODE_BITS && occupied == u64::from(COMPLETE_HUFFMAN_CODE_SPACE)
}

/// The code-length code must be complete; there is no empty or single-code
/// exception.
pub(crate) fn code_length_tree_shape_is_valid(lengths: &[u8]) -> bool {
    huffman_tree_shape_is_complete(lengths)
}

/// Accept a complete code, a single one-bit code (RFC 1951 allows this for
/// distances; zlib also accepts it for literal/length codes), or an empty code
/// if `allow_empty` (the distance code of a literal-only block).
// Out of line: each call scans the lengths.
#[inline(never)]
pub(crate) fn payload_tree_shape_is_valid(lengths: &[u8], allow_empty: bool) -> bool {
    let populated = lengths.iter().filter(|&&length| length != 0).count();
    match populated {
        0 => allow_empty,
        1 => lengths.contains(&1),
        _ => huffman_tree_shape_is_complete(lengths),
    }
}

/// Whether [`Huffman::build`] would accept `lengths`, without allocating its
/// tables.
pub(crate) fn huffman_code_lengths_are_valid(lengths: &[u8]) -> bool {
    analyze_code_lengths(lengths).is_some()
}

fn analyze_code_lengths(lengths: &[u8]) -> Option<([u32; MAX_CODE_BITS + 1], u8, usize)> {
    if lengths.is_empty() {
        return None;
    }

    let mut count_by_length = [0_u32; MAX_CODE_BITS + 1];
    let mut max_bits = 0_u8;
    let mut populated = 0_usize;
    for &length in lengths {
        if usize::from(length) > MAX_CODE_BITS {
            return None;
        }
        if length != 0 {
            count_by_length[usize::from(length)] += 1;
            max_bits = max_bits.max(length);
            populated += 1;
        }
    }
    if populated > MAX_C_CODES {
        return None;
    }

    let mut code = 0_u32;
    for bits in 1..=MAX_CODE_BITS {
        code = (code + count_by_length[bits - 1]) << 1;
        if code + count_by_length[bits] > (1_u32 << bits) {
            return None;
        }
    }
    Some((count_by_length, max_bits, populated))
}

/// `code` is bit-reversed: RFC 1951 packs Huffman codes most significant bit
/// first into an LSB-first stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct HuffCode {
    pub(crate) symbol: u16,
    pub(crate) length: u8,
    pub(crate) code: u16,
}

/// Canonical Huffman code for one alphabet, with encoding and decoding tables.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Huffman {
    /// Indexed by symbol; length 0 means unused.
    codes: Vec<HuffCode>,
    /// Per-length canonical code ranges, so decoding takes at most 15
    /// comparisons.
    first_code: [u16; MAX_CODE_BITS + 1],
    first_symbol: [u16; MAX_CODE_BITS + 1],
    code_count: [u16; MAX_CODE_BITS + 1],
    decode_symbols: Vec<u16>,
    max_bits: u8,
    decode_table: Option<DecodeTable>,
    decode_profile: Option<DecodeProfile>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
struct DecodeEntry(u64);

impl DecodeEntry {
    const SYMBOL_SHIFT: u32 = 0;
    const VALUE_SHIFT: u32 = 9;
    const BITS_SHIFT: u32 = 25;
    const EXTRA_BITS_SHIFT: u32 = 29;
    const SUBTABLE_BITS_SHIFT: u32 = 33;
    const SUBTABLE_START_SHIFT: u32 = 37;

    fn direct(symbol: u16, value_base: u16, bits: u8) -> Self {
        debug_assert!(symbol < 1 << 9);
        debug_assert!(bits < 1 << 4);
        Self(
            (u64::from(symbol) << Self::SYMBOL_SHIFT)
                | (u64::from(value_base) << Self::VALUE_SHIFT)
                | (u64::from(bits) << Self::BITS_SHIFT),
        )
    }

    fn subtable(bits: u8, start: u16) -> Self {
        debug_assert!(bits < 1 << 4);
        Self(
            (u64::from(bits) << Self::SUBTABLE_BITS_SHIFT)
                | (u64::from(start) << Self::SUBTABLE_START_SHIFT),
        )
    }

    fn with_metadata(mut self, value_base: u16, extra_bits: u8) -> Self {
        debug_assert!(extra_bits < 1 << 4);
        const VALUE_MASK: u64 = u16::MAX as u64;
        const WIDTH_MASK: u64 = 0xf;
        self.0 &= !((VALUE_MASK << Self::VALUE_SHIFT) | (WIDTH_MASK << Self::EXTRA_BITS_SHIFT));
        self.0 |= (u64::from(value_base) << Self::VALUE_SHIFT)
            | (u64::from(extra_bits) << Self::EXTRA_BITS_SHIFT);
        self
    }

    fn symbol(self) -> u16 {
        ((self.0 >> Self::SYMBOL_SHIFT) & 0x1ff) as u16
    }

    fn value_base(self) -> u16 {
        ((self.0 >> Self::VALUE_SHIFT) & u64::from(u16::MAX)) as u16
    }

    fn bits(self) -> u8 {
        ((self.0 >> Self::BITS_SHIFT) & 0xf) as u8
    }

    fn extra_bits(self) -> u8 {
        ((self.0 >> Self::EXTRA_BITS_SHIFT) & 0xf) as u8
    }

    fn subtable_bits(self) -> u8 {
        ((self.0 >> Self::SUBTABLE_BITS_SHIFT) & 0xf) as u8
    }

    fn subtable_start(self) -> u16 {
        ((self.0 >> Self::SUBTABLE_START_SHIFT) & u64::from(u16::MAX)) as u16
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct DecodeProfile {
    first_value_symbol: u16,
    bases: &'static [u16],
    extra_bits: &'static [u8],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct DecodedValue {
    pub(crate) symbol: u16,
    pub(crate) value: u16,
    pub(crate) extra: u16,
    pub(crate) extra_bits: u8,
}

/// Single-level decoder for the code-length alphabet: its codes are at most 7
/// bits (lengths are sent in 3-bit fields), so one 128-entry table holds them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CodeLengthDecoder {
    entries: [u8; CODE_LENGTH_DECODE_SIZE],
}

impl CodeLengthDecoder {
    pub(crate) fn build(lengths: &[u8; 19]) -> Option<Self> {
        if lengths
            .iter()
            .any(|&length| length > CODE_LENGTH_DECODE_BITS)
            || !code_length_tree_shape_is_valid(lengths)
        {
            return None;
        }

        let mut count_by_length = [0_u16; CODE_LENGTH_DECODE_BITS as usize + 1];
        for &length in lengths {
            if length != 0 {
                count_by_length[usize::from(length)] += 1;
            }
        }
        let mut next_code = [0_u16; CODE_LENGTH_DECODE_BITS as usize + 1];
        let mut code = 0_u16;
        for bits in 1..=usize::from(CODE_LENGTH_DECODE_BITS) {
            code = (code + count_by_length[bits - 1]) << 1;
            next_code[bits] = code;
        }

        let mut entries = [u8::MAX; CODE_LENGTH_DECODE_SIZE];
        for (symbol, &length) in lengths.iter().enumerate() {
            if length == 0 {
                continue;
            }
            let index = usize::from(length);
            let wire_code = usize::from(reverse_bits(next_code[index], length));
            let stride = 1_usize << length;
            let packed = ((symbol as u8) << 3) | length;
            for slot in (wire_code..CODE_LENGTH_DECODE_SIZE).step_by(stride) {
                entries[slot] = packed;
            }
            next_code[index] += 1;
        }
        entries
            .iter()
            .all(|&entry| entry != u8::MAX)
            .then_some(Self { entries })
    }

    pub(crate) fn decode(&self, reader: &mut BitReader<'_>) -> Result<u16> {
        if let Ok(value) = reader.peek(CODE_LENGTH_DECODE_BITS) {
            let packed = self.entries[value as usize];
            reader.drop_bits(packed & 7)?;
            return Ok(u16::from(packed >> 3));
        }

        // Near the end of input, fewer than seven bits may remain. Try each
        // length so decoding never reads past the input.
        for bits in 1..=CODE_LENGTH_DECODE_BITS {
            let value = reader.peek(bits)?;
            let packed = self.entries[value as usize];
            if packed & 7 == bits {
                reader.drop_bits(bits)?;
                return Ok(u16::from(packed >> 3));
            }
        }
        Err(Error::new("invalid Huffman code in Deflate stream"))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct DecodeTable {
    entries: Vec<DecodeEntry>,
    root_bits: u8,
}

impl DecodeTable {
    /// The entry the low bits of `window` select, and its code length;
    /// `window` must hold at least the longest code.
    #[inline]
    fn entry(&self, window: u32) -> Result<(DecodeEntry, u8)> {
        if self.root_bits == 0 {
            return Err(Error::new("invalid Huffman code in Deflate stream"));
        }
        let root_entry = self.entries[(window & ((1_u32 << self.root_bits) - 1)) as usize];
        let subtable_bits = root_entry.subtable_bits();
        let (entry, bits) = if subtable_bits == 0 {
            (root_entry, root_entry.bits())
        } else {
            let subtable_mask = (1_u32 << subtable_bits) - 1;
            let index = usize::from(root_entry.subtable_start())
                + ((window >> self.root_bits) & subtable_mask) as usize;
            let entry = self.entries[index];
            (entry, self.root_bits + entry.bits())
        };
        if entry.bits() == 0 {
            return Err(Error::new("invalid Huffman code in Deflate stream"));
        }
        Ok((entry, bits))
    }
}

impl Huffman {
    /// Build canonical codes from code lengths. Incomplete codes are accepted;
    /// callers apply each alphabet's rule. At most `MAX_C_CODES` symbols may
    /// have codes, the table limit of Columbo's C decoder.
    pub(crate) fn build(lengths: &[u8]) -> Option<Self> {
        let (count_by_length, max_bits, populated) = analyze_code_lengths(lengths)?;

        let mut next_code = [0_u32; MAX_CODE_BITS + 1];
        let mut code = 0_u32;
        for bits in 1..=MAX_CODE_BITS {
            code = (code + count_by_length[bits - 1]) << 1;
            next_code[bits] = code;
        }

        let mut first_code = [0_u16; MAX_CODE_BITS + 1];
        let mut first_symbol = [0_u16; MAX_CODE_BITS + 1];
        let mut code_count = [0_u16; MAX_CODE_BITS + 1];
        let mut symbol_offset = 0_u16;
        for bits in 1..=MAX_CODE_BITS {
            first_code[bits] = next_code[bits] as u16;
            first_symbol[bits] = symbol_offset;
            code_count[bits] = count_by_length[bits] as u16;
            symbol_offset += code_count[bits];
        }

        let mut decode_symbols = vec![0_u16; populated];
        let mut next_symbol = first_symbol;
        let mut codes = Vec::new();
        codes.try_reserve_exact(lengths.len()).ok()?;
        codes.resize(
            lengths.len(),
            HuffCode {
                symbol: 0,
                length: 0,
                code: 0,
            },
        );
        for (symbol, &length) in lengths.iter().enumerate() {
            if length == 0 {
                continue;
            }
            let symbol = u16::try_from(symbol).ok()?;
            let index = usize::from(length);
            let decode_index = usize::from(next_symbol[index]);
            decode_symbols[decode_index] = symbol;
            next_symbol[index] += 1;
            codes[usize::from(symbol)] = HuffCode {
                symbol,
                length,
                code: reverse_bits(next_code[index] as u16, length),
            };
            next_code[index] += 1;
        }

        Some(Self {
            codes,
            first_code,
            first_symbol,
            code_count,
            decode_symbols,
            max_bits,
            decode_table: None,
            decode_profile: None,
        })
    }

    /// Build a two-level decoder whose entries also hold each symbol's base
    /// value and extra-bit count, so a code and its extra bits are read in one
    /// step.
    pub(crate) fn build_value_decoder_with_root_bits(
        lengths: &[u8],
        first_value_symbol: u16,
        bases: &'static [u16],
        extra_bits: &'static [u8],
        root_bits: u8,
    ) -> Option<Self> {
        if bases.len() != extra_bits.len() {
            return None;
        }
        let profile = DecodeProfile {
            first_value_symbol,
            bases,
            extra_bits,
        };
        let mut tree = Self::build(lengths)?;
        let mut table = build_decode_table(&tree, root_bits)?;
        for entry in &mut table.entries {
            if entry.bits() == 0 {
                continue;
            }
            let (base, width) = decode_metadata(profile, entry.symbol());
            *entry = entry.with_metadata(base, width);
        }
        tree.decode_table = Some(table);
        tree.decode_profile = Some(profile);
        Some(tree)
    }

    pub(crate) fn decode_value(&self, reader: &mut BitReader<'_>) -> Result<DecodedValue> {
        let profile = self
            .decode_profile
            .ok_or_else(|| Error::internal("internal Huffman decoder has no value profile"))?;
        if let Some(table) = &self.decode_table {
            // With 32 bits in hand, the longest code and its extra bits are
            // both there, so one look decodes the value.
            if let Some(window) = reader.peek_word() {
                let (entry, code_bits) = table.entry(window)?;
                let extra_bits = entry.extra_bits();
                let extra = ((window >> code_bits) & ((1_u32 << extra_bits) - 1)) as u16;
                reader.drop_bits(code_bits + extra_bits)?;
                return Ok(DecodedValue {
                    symbol: entry.symbol(),
                    value: entry.value_base() + extra,
                    extra,
                    extra_bits,
                });
            }
            if let Some((entry, code_bits)) = self.peek_decode_table(reader, table)? {
                let extra_bits = entry.extra_bits();
                let total_bits = code_bits + extra_bits;
                let packed = reader.peek(total_bits)?;
                let extra = if extra_bits == 0 {
                    0
                } else {
                    let mask = (1_u32 << extra_bits) - 1;
                    ((packed >> code_bits) & mask) as u16
                };
                reader.drop_bits(total_bits)?;
                return Ok(DecodedValue {
                    symbol: entry.symbol(),
                    value: entry.value_base() + extra,
                    extra,
                    extra_bits,
                });
            }
        }

        // Near the end of input the wider table lookup may not fit; decode
        // bit by bit instead.
        let symbol = self.decode_canonical(reader)?;
        let (base, extra_bits) = decode_metadata(profile, symbol);
        let extra = if extra_bits == 0 {
            0
        } else {
            reader.read(extra_bits)? as u16
        };
        Ok(DecodedValue {
            symbol,
            value: base + extra,
            extra,
            extra_bits,
        })
    }

    fn peek_decode_table(
        &self,
        reader: &mut BitReader<'_>,
        table: &DecodeTable,
    ) -> Result<Option<(DecodeEntry, u8)>> {
        if table.root_bits == 0 {
            return Err(Error::new("invalid Huffman code in Deflate stream"));
        }
        let root_value = match reader.peek(table.root_bits) {
            Ok(value) => value,
            Err(_) => return Ok(None),
        };
        let root_entry = table.entries[root_value as usize];
        let subtable_bits = root_entry.subtable_bits();
        if subtable_bits == 0 {
            let bits = root_entry.bits();
            if bits == 0 {
                return Err(Error::new("invalid Huffman code in Deflate stream"));
            }
            return Ok(Some((root_entry, bits)));
        }

        let lookup_bits = table.root_bits + subtable_bits;
        let value = match reader.peek(lookup_bits) {
            Ok(value) => value,
            Err(_) => return Ok(None),
        };
        let subtable_mask = (1_u32 << subtable_bits) - 1;
        let subtable_index = usize::from(root_entry.subtable_start())
            + ((value >> table.root_bits) & subtable_mask) as usize;
        let entry = table.entries[subtable_index];
        let bits = entry.bits();
        if bits == 0 {
            return Err(Error::new("invalid Huffman code in Deflate stream"));
        }
        Ok(Some((entry, table.root_bits + bits)))
    }

    fn decode_canonical(&self, reader: &mut BitReader<'_>) -> Result<u16> {
        let mut code = 0_u16;
        for length in 1..=self.max_bits {
            // Huffman codes are packed most significant bit first, so
            // building the code one bit at a time needs no bit reversal.
            code = (code << 1) | reader.read(1)? as u16;
            let index = usize::from(length);
            let offset = code.wrapping_sub(self.first_code[index]);
            if offset < self.code_count[index] {
                let symbol_index = usize::from(self.first_symbol[index]) + usize::from(offset);
                return Ok(self.decode_symbols[symbol_index]);
            }
        }
        Err(Error::new("invalid Huffman code in Deflate stream"))
    }

    pub(crate) fn code(&self, symbol: usize) -> Option<HuffCode> {
        let code = *self.codes.get(symbol)?;
        (code.length != 0).then_some(code)
    }
}

fn decode_metadata(profile: DecodeProfile, symbol: u16) -> (u16, u8) {
    let Some(index) = symbol.checked_sub(profile.first_value_symbol) else {
        return (symbol, 0);
    };
    let index = usize::from(index);
    match (profile.bases.get(index), profile.extra_bits.get(index)) {
        (Some(&base), Some(&extra_bits)) => (base, extra_bits),
        _ => (symbol, 0),
    }
}

fn build_decode_table(tree: &Huffman, root_bits: u8) -> Option<DecodeTable> {
    if tree.max_bits == 0 {
        return Some(DecodeTable {
            entries: Vec::new(),
            root_bits: 0,
        });
    }
    let root_bits = tree.max_bits.min(root_bits);
    let root_size = 1_usize << root_bits;
    let root_mask = (root_size - 1) as u16;

    let mut subtable_widths = Vec::new();
    subtable_widths.try_reserve_exact(root_size).ok()?;
    subtable_widths.resize(root_size, 0_u8);
    for code in tree.codes.iter().filter(|code| code.length > root_bits) {
        let prefix = usize::from(code.code & root_mask);
        subtable_widths[prefix] = subtable_widths[prefix].max(code.length - root_bits);
    }

    let mut entries = Vec::new();
    entries.try_reserve_exact(root_size).ok()?;
    entries.resize(root_size, DecodeEntry::default());

    // Build the root table by doubling: for each code length, copy the filled
    // lower half into the upper half, then write each code of that length
    // into its one slot.
    for length in 1..=root_bits {
        if length > 1 {
            let previous_size = 1_usize << (length - 1);
            entries.copy_within(..previous_size, previous_size);
        }
        let length_index = usize::from(length);
        let first = usize::from(tree.first_symbol[length_index]);
        let end = first + usize::from(tree.code_count[length_index]);
        for &symbol in &tree.decode_symbols[first..end] {
            let code = tree.codes[usize::from(symbol)];
            entries[usize::from(code.code)] =
                DecodeEntry::direct(code.symbol, code.symbol, code.length);
        }
    }

    // Add subtable pointers after the root table is complete, so the doubling
    // copies above cannot duplicate them into other slots.
    for (prefix, &subtable_bits) in subtable_widths.iter().enumerate() {
        if subtable_bits != 0 {
            let subtable_start = u16::try_from(entries.len()).ok()?;
            let subtable_size = 1_usize << subtable_bits;
            entries.try_reserve(subtable_size).ok()?;
            entries.resize(entries.len() + subtable_size, DecodeEntry::default());
            entries[prefix] = DecodeEntry::subtable(subtable_bits, subtable_start);
        }
    }

    for code in tree.codes.iter().filter(|code| code.length > root_bits) {
        let prefix = usize::from(code.code & root_mask);
        let root_entry = entries[prefix];
        let remaining_bits = code.length - root_bits;
        let suffix_count = 1_usize << (root_entry.subtable_bits() - remaining_bits);
        let code_suffix = usize::from(code.code >> root_bits);
        for suffix in 0..suffix_count {
            let index =
                usize::from(root_entry.subtable_start()) + code_suffix + (suffix << remaining_bits);
            entries[index] = DecodeEntry::direct(code.symbol, code.symbol, remaining_bits);
        }
    }

    Some(DecodeTable { entries, root_bits })
}

fn reverse_bits(code: u16, length: u8) -> u16 {
    code.reverse_bits() >> (u16::BITS as u8 - length)
}

pub(crate) fn fixed_trees() -> (&'static Huffman, &'static Huffman) {
    static FIXED: OnceLock<(Huffman, Huffman)> = OnceLock::new();
    let (literal_length, distance) = FIXED.get_or_init(|| {
        (
            Huffman::build(&FIXED_LITERAL_CODE_LENGTHS).expect("fixed literal tree is valid"),
            Huffman::build(&FIXED_DISTANCE_CODE_LENGTHS).expect("fixed distance tree is valid"),
        )
    });
    (literal_length, distance)
}

#[derive(Debug, Clone, Copy)]
struct Node {
    frequency: u32,
    symbol: Option<usize>,
    left: Option<usize>,
    right: Option<usize>,
    parent: Option<usize>,
    side: u8,
    order: usize,
    height: usize,
}

impl Node {
    fn leaf(frequency: u32, symbol: usize, order: usize) -> Self {
        Self {
            frequency,
            symbol: Some(symbol),
            left: None,
            right: None,
            parent: None,
            side: 0,
            order,
            height: 0,
        }
    }

    fn branch(frequency: u32, left: usize, right: usize, order: usize) -> Self {
        Self {
            frequency,
            symbol: None,
            left: Some(left),
            right: Some(right),
            parent: None,
            side: 0,
            order,
            height: 0,
        }
    }
}

/// [`node_less`] as one integer: the frequency, then the order, ascending for
/// even variants and descending for odd ones. Orders are symbol or node
/// indices, so they fit 32 bits.
fn node_key(node: &Node, variant: u32) -> u64 {
    debug_assert!(u32::try_from(node.order).is_ok());
    let order = node.order as u32;
    let order = if variant & 1 != 0 { !order } else { order };
    u64::from(node.frequency) << 32 | u64::from(order)
}

fn node_less(nodes: &[Node], a: usize, b: usize, variant: u32) -> bool {
    if nodes[a].frequency != nodes[b].frequency {
        return nodes[a].frequency < nodes[b].frequency;
    }
    if variant & 1 != 0 {
        nodes[a].order > nodes[b].order
    } else {
        nodes[a].order < nodes[b].order
    }
}

/// A heap entry in one word: the node's frequency and a 16-bit order, ordered
/// as [`node_key`] orders them, in the top 48 bits, and the node in the low 16,
/// so comparisons need not read the node. Equal keys compare as [`node_less`]
/// does, so the heap moves exactly as one of node indices would.
type KeyedNode = u64;

fn keyed_node(nodes: &[Node], node: usize, variant: u32) -> KeyedNode {
    debug_assert!(node < 1 << 16 && nodes[node].order < 1 << 16);
    let order = nodes[node].order as u16;
    let order = if variant & 1 != 0 { !order } else { order };
    u64::from(nodes[node].frequency) << 32 | u64::from(order) << 16 | node as u64
}

/// A [`KeyedNode`]'s key, without the node.
fn keyed_rank(entry: KeyedNode) -> u64 {
    entry >> 16
}

fn keyed_index(entry: KeyedNode) -> usize {
    (entry & 0xffff) as usize
}

/// Reusable buffers for the generic builder's heap merges.
#[derive(Default)]
pub(crate) struct KeyedHeaps {
    increasing: Vec<KeyedNode>,
    decreasing: Vec<KeyedNode>,
    is_active: Vec<bool>,
}

/// Fill `heap` with `members` keyed for `variant`, in heap order.
fn fill_keyed_heap(heap: &mut Vec<KeyedNode>, nodes: &[Node], members: &[usize], variant: u32) {
    heap.clear();
    heap.extend(members.iter().map(|&node| keyed_node(nodes, node, variant)));
    for start in (0..heap.len() / 2).rev() {
        keyed_heap_sift_down(heap, start);
    }
}

fn keyed_heap_sift_down(heap: &mut [KeyedNode], mut root: usize) {
    loop {
        let mut child = root * 2 + 1;
        if child >= heap.len() {
            return;
        }
        if child + 1 < heap.len() && keyed_rank(heap[child + 1]) < keyed_rank(heap[child]) {
            child += 1;
        }
        if keyed_rank(heap[child]) >= keyed_rank(heap[root]) {
            return;
        }
        heap.swap(root, child);
        root = child;
    }
}

fn keyed_heap_pop(heap: &mut Vec<KeyedNode>) -> Option<usize> {
    let result = keyed_index(*heap.first()?);
    let last = heap.pop().expect("the heap is non-empty");
    if !heap.is_empty() {
        heap[0] = last;
        keyed_heap_sift_down(heap, 0);
    }
    Some(result)
}

fn keyed_heap_push(heap: &mut Vec<KeyedNode>, entry: KeyedNode) {
    let mut position = heap.len();
    heap.push(entry);
    while position != 0 {
        let parent = (position - 1) / 2;
        if keyed_rank(heap[position]) >= keyed_rank(heap[parent]) {
            break;
        }
        heap.swap(position, parent);
        position = parent;
    }
}

fn merge_generic_nodes_heap(
    nodes: &mut Vec<Node>,
    active: &mut Vec<usize>,
    variant: u32,
    heap: &mut Vec<KeyedNode>,
) {
    fill_keyed_heap(heap, nodes, active, variant);
    while heap.len() > 1 {
        let left = keyed_heap_pop(heap).expect("two nodes remain");
        let right = keyed_heap_pop(heap).expect("one node remains");
        let index = nodes.len();
        let frequency = nodes[left].frequency.wrapping_add(nodes[right].frequency);
        nodes.push(Node::branch(frequency, left, right, index));
        keyed_heap_push(heap, keyed_node(nodes, index, variant));
    }
    active.clear();
    active.extend(heap.first().map(|&root| keyed_index(root)));
}

fn keyed_heap_pop_active(heap: &mut Vec<KeyedNode>, active: &[bool]) -> Option<usize> {
    loop {
        let node = keyed_heap_pop(heap)?;
        if active[node] {
            return Some(node);
        }
    }
}

/// Merge for variants 2 and 3, whose first and second child break ties in
/// opposite directions: one heap per tie order, and a node taken from one is
/// skipped by the other.
fn merge_generic_nodes_dual_heap(
    nodes: &mut Vec<Node>,
    active: &mut Vec<usize>,
    variant: u32,
    heaps: &mut KeyedHeaps,
) {
    debug_assert_ne!(variant & 2, 0);
    let KeyedHeaps {
        increasing: increasing_order,
        decreasing: decreasing_order,
        is_active,
    } = heaps;
    fill_keyed_heap(increasing_order, nodes, active, 0);
    fill_keyed_heap(decreasing_order, nodes, active, 1);

    let leaf_count = active.len();
    is_active.clear();
    is_active.resize(nodes.len(), true);
    let mut remaining = leaf_count;
    while remaining > 1 {
        let reverse_first = variant & 1 != 0;
        let left = if reverse_first {
            keyed_heap_pop_active(decreasing_order, is_active)
        } else {
            keyed_heap_pop_active(increasing_order, is_active)
        }
        .expect("two nodes remain");
        is_active[left] = false;

        let right = if reverse_first {
            keyed_heap_pop_active(increasing_order, is_active)
        } else {
            keyed_heap_pop_active(decreasing_order, is_active)
        }
        .expect("one node remains");
        is_active[right] = false;

        let index = nodes.len();
        let frequency = nodes[left].frequency.wrapping_add(nodes[right].frequency);
        nodes.push(Node::branch(frequency, left, right, index));
        is_active.push(true);
        keyed_heap_push(increasing_order, keyed_node(nodes, index, 0));
        keyed_heap_push(decreasing_order, keyed_node(nodes, index, 1));
        remaining -= 1;
    }

    let root = keyed_heap_pop_active(increasing_order, is_active).expect("one root remains");
    active.clear();
    active.push(root);
}

fn take_generic_front(
    nodes: &[Node],
    active: &[usize],
    leaf_end: usize,
    leaf: &mut usize,
    branch: &mut usize,
    variant: u32,
) -> usize {
    let take_leaf = *leaf < leaf_end
        && (*branch >= active.len() || node_less(nodes, active[*leaf], active[*branch], variant));
    if take_leaf {
        let result = active[*leaf];
        *leaf += 1;
        result
    } else {
        let result = active[*branch];
        *branch += 1;
        result
    }
}

/// Pack a leaf's sort key: `frequency`, then `tie`. Every caller's ties are
/// distinct, so the keys are too, and their order does not depend on the sort
/// algorithm.
fn leaf_key(frequency: u32, tie: u32) -> u64 {
    u64::from(frequency) << 32 | u64::from(tie)
}

/// The tie of a key from [`leaf_key`].
fn leaf_key_tie(key: u64) -> u32 {
    key as u32
}

/// The overflow repair and package-merge builders sort their leaves through
/// this one instance. The two-queue merges and Defluff's builder keep their own
/// sorts: they build the most trees, and a key buffer for each cost more than
/// it saved.
#[inline(never)]
fn sort_leaf_keys(keys: &mut [u64]) {
    keys.sort_unstable();
}

/// Order nodes by [`node_less`]. Both two-queue merges share this one sort.
fn sort_generic_nodes(nodes: &[Node], active: &mut [usize], variant: u32) {
    active.sort_unstable_by_key(|&node| node_key(&nodes[node], variant));
}

fn merge_generic_nodes_two_front(nodes: &mut Vec<Node>, active: &mut Vec<usize>, variant: u32) {
    sort_generic_nodes(nodes, active, variant);

    // Two-queue construction: branch frequencies never decrease and creation
    // order increases, so for variant 0 both queues stay sorted.
    let leaf_end = active.len();
    active.reserve(leaf_end.saturating_sub(1));
    let mut leaf = 0;
    let mut branch = leaf_end;
    let mut remaining = leaf_end;

    while remaining > 1 {
        let left = take_generic_front(nodes, active, leaf_end, &mut leaf, &mut branch, variant);
        let right = take_generic_front(nodes, active, leaf_end, &mut leaf, &mut branch, variant);
        let index = nodes.len();
        let frequency = nodes[left].frequency.wrapping_add(nodes[right].frequency);
        nodes.push(Node::branch(frequency, left, right, index));
        active.push(index);
        remaining -= 1;
    }

    let root = take_generic_front(nodes, active, leaf_end, &mut leaf, &mut branch, variant);
    active.clear();
    active.push(root);
}

#[derive(Clone, Copy)]
struct ReverseBranchRun {
    frequency: u32,
    newest: usize,
}

fn push_reverse_branch(
    runs: &mut VecDeque<ReverseBranchRun>,
    previous: &mut Vec<Option<usize>>,
    node: usize,
    frequency: u32,
) {
    debug_assert_eq!(previous.len(), node);
    let predecessor = runs
        .back()
        .filter(|run| run.frequency == frequency)
        .map(|run| run.newest);
    previous.push(predecessor);
    if let Some(run) = runs.back_mut().filter(|run| run.frequency == frequency) {
        run.newest = node;
    } else {
        debug_assert!(runs.back().map_or(true, |run| run.frequency < frequency));
        runs.push_back(ReverseBranchRun {
            frequency,
            newest: node,
        });
    }
}

fn pop_reverse_branch(
    runs: &mut VecDeque<ReverseBranchRun>,
    previous: &[Option<usize>],
) -> Option<usize> {
    let run = runs.front_mut()?;
    let node = run.newest;
    if let Some(predecessor) = previous[node] {
        run.newest = predecessor;
    } else {
        runs.pop_front();
    }
    Some(node)
}

fn take_reverse_front(
    nodes: &[Node],
    leaves: &[usize],
    leaf: &mut usize,
    runs: &mut VecDeque<ReverseBranchRun>,
    previous: &[Option<usize>],
) -> usize {
    let branch = runs.front().map(|run| run.newest);
    let take_leaf = *leaf < leaves.len()
        && branch.map_or(true, |branch| node_less(nodes, leaves[*leaf], branch, 1));
    if take_leaf {
        let node = leaves[*leaf];
        *leaf += 1;
        node
    } else {
        pop_reverse_branch(runs, previous).expect("one branch remains")
    }
}

/// Reusable buffers for the generic builder's reverse two-queue merge.
#[derive(Default)]
pub(crate) struct ReverseFront {
    leaves: Vec<usize>,
    runs: VecDeque<ReverseBranchRun>,
    previous: Vec<Option<usize>>,
}

/// Two-queue merge for variant 1, which prefers the newest node on equal
/// frequency; a linked stack per run of equal frequencies gives the newest
/// branch in constant time.
fn merge_generic_nodes_reverse_front(
    nodes: &mut Vec<Node>,
    active: &mut Vec<usize>,
    buffers: &mut ReverseFront,
) {
    sort_generic_nodes(nodes, active, 1);

    let ReverseFront {
        leaves,
        runs,
        previous,
    } = buffers;
    leaves.clear();
    leaves.extend_from_slice(active);
    let mut leaf = 0;
    runs.clear();
    runs.reserve(leaves.len());
    // One entry per node, branches included.
    previous.clear();
    previous.reserve(nodes.len().saturating_mul(2));
    previous.resize(nodes.len(), None);
    let mut remaining = leaves.len();
    while remaining > 1 {
        let left = take_reverse_front(nodes, leaves, &mut leaf, runs, previous);
        let right = take_reverse_front(nodes, leaves, &mut leaf, runs, previous);
        let index = nodes.len();
        let frequency = nodes[left].frequency.wrapping_add(nodes[right].frequency);
        nodes.push(Node::branch(frequency, left, right, index));
        push_reverse_branch(runs, previous, index, frequency);
        remaining -= 1;
    }

    let root = take_reverse_front(nodes, leaves, &mut leaf, runs, previous);
    active.clear();
    active.push(root);
}

fn assign_depths(nodes: &[Node], node_index: usize, depth: usize, lengths: &mut [u8]) -> usize {
    let node = nodes[node_index];
    if let Some(symbol) = node.symbol {
        lengths[symbol] = depth.min(usize::from(u8::MAX)) as u8;
        return depth;
    }

    let left_depth = assign_depths(
        nodes,
        node.left.expect("branch has a left child"),
        depth + 1,
        lengths,
    );
    let right_depth = assign_depths(
        nodes,
        node.right.expect("branch has a right child"),
        depth + 1,
        lengths,
    );
    left_depth.max(right_depth)
}

/// [`assign_depths`] from the root, for a tree built by merging: every branch
/// follows its children and the root is the last node, so one pass from the
/// end gives each node its depth before its children need it.
fn assign_depths_from_last(nodes: &[Node], depths: &mut Vec<u32>, lengths: &mut [u8]) -> usize {
    depths.clear();
    depths.resize(nodes.len(), 0);
    let mut max_depth = 0;
    for (index, node) in nodes.iter().enumerate().rev() {
        let depth = depths[index];
        if let Some(symbol) = node.symbol {
            lengths[symbol] = depth.min(u32::from(u8::MAX)) as u8;
            max_depth = max_depth.max(depth as usize);
        } else {
            for child in [node.left, node.right] {
                depths[child.expect("branch has two children")] = depth + 1;
            }
        }
    }
    max_depth
}

fn first_leaf_at_depth(
    nodes: &[Node],
    node_index: usize,
    depth: usize,
    target: usize,
) -> Option<usize> {
    let node = nodes[node_index];
    if node.symbol.is_some() {
        return (depth == target).then_some(node_index);
    }
    first_leaf_at_depth(
        nodes,
        node.left.expect("branch has a left child"),
        depth + 1,
        target,
    )
    .or_else(|| {
        first_leaf_at_depth(
            nodes,
            node.right.expect("branch has a right child"),
            depth + 1,
            target,
        )
    })
}

/// Build Columbo's generic Huffman code lengths, limited to `max_bits`.
///
/// `variant` (0-3) sets the equal-frequency tie rule: bit 0 prefers newer
/// nodes, and bit 1 makes the second child use the opposite order to the
/// first. Over-long codes are shortened with zlib's overflow repair.
pub(crate) fn make_lengths(frequencies: &[u32], max_bits: u8, variant: u32) -> Vec<u8> {
    let mut lengths = vec![0; frequencies.len()];
    make_lengths_into(
        frequencies,
        &mut lengths,
        max_bits,
        variant,
        &mut TreeBuildScratch::default(),
    );
    lengths
}

/// Return whether the unlimited tree was deeper than `max_bits` and needed
/// repair.
pub(crate) fn make_lengths_into(
    frequencies: &[u32],
    lengths: &mut [u8],
    max_bits: u8,
    variant: u32,
    scratch: &mut TreeBuildScratch,
) -> bool {
    make_lengths_inner(frequencies, lengths, max_bits, variant, scratch)
}

/// Return whether the unlimited tree was deeper than `max_bits` and needed
/// repair.
fn make_lengths_inner(
    frequencies: &[u32],
    lengths: &mut [u8],
    max_bits: u8,
    variant: u32,
    scratch: &mut TreeBuildScratch,
) -> bool {
    assert_eq!(frequencies.len(), lengths.len());
    // Every node index and order fits a keyed heap entry's 16 bits.
    assert!(
        frequencies.len() <= MAX_PACKED_HEAP_LEAVES,
        "alphabet fits the heap"
    );
    lengths.fill(0);

    let TreeBuildScratch {
        nodes,
        node_indices: active,
        depths,
        keyed_heaps,
        reverse_front,
        ..
    } = scratch;
    nodes.clear();
    active.clear();
    nodes.reserve(frequencies.len().saturating_mul(2));
    // The two-queue merge appends each branch to the leaves.
    active.reserve(frequencies.len().saturating_mul(2));
    for (symbol, &frequency) in frequencies.iter().enumerate() {
        if frequency == 0 {
            continue;
        }
        active.push(nodes.len());
        nodes.push(Node::leaf(frequency, symbol, symbol));
    }

    match active.len() {
        0 => return false,
        1 => {
            lengths[nodes[active[0]].symbol.expect("leaf has a symbol")] = 1;
            return false;
        }
        _ => {}
    }

    // Variants 0 and 1 use a two-queue merge, which needs nondecreasing
    // branch frequencies; use a heap instead if a frequency sum could wrap.
    let total_frequency: u64 = frequencies
        .iter()
        .map(|&frequency| u64::from(frequency))
        .sum();
    if variant <= 1 && total_frequency <= u64::from(u32::MAX) {
        if variant == 0 {
            merge_generic_nodes_two_front(nodes, active, variant);
        } else {
            merge_generic_nodes_reverse_front(nodes, active, reverse_front);
        }
    } else if variant & 2 == 0 {
        merge_generic_nodes_heap(nodes, active, variant, &mut keyed_heaps.increasing);
    } else {
        merge_generic_nodes_dual_heap(nodes, active, variant, keyed_heaps);
    }

    debug_assert_eq!(active[0], nodes.len() - 1);
    let max_depth = assign_depths_from_last(nodes, depths, lengths);
    if max_depth <= usize::from(max_bits) {
        return false;
    }
    limit_generic_lengths(frequencies, lengths, max_bits);
    true
}

fn limit_generic_lengths(frequencies: &[u32], lengths: &mut [u8], max_bits: u8) {
    let limit = usize::from(max_bits);
    if limit == 0 || limit > MAX_TRACKED_DEPTH {
        lengths.fill(0);
        return;
    }

    let mut count_by_length = [0_usize; MAX_TRACKED_DEPTH + 1];
    let mut overflow = 0_usize;
    for &length in lengths.iter() {
        if length == 0 {
            continue;
        }
        let mut length = usize::from(length).min(MAX_TRACKED_DEPTH);
        if length > limit {
            length = limit;
            overflow += 1;
        }
        count_by_length[length] += 1;
    }

    // zlib's overflow repair (`gen_bitlen`): move a leaf from the deepest
    // level below `limit` down one level and give it an overflowed leaf as
    // its sibling.
    while overflow != 0 {
        let mut bits = limit - 1;
        while bits > 0 && count_by_length[bits] == 0 {
            bits -= 1;
        }
        if bits == 0 || count_by_length[limit] == 0 {
            break;
        }
        count_by_length[bits] -= 1;
        count_by_length[bits + 1] += 2;
        count_by_length[limit] -= 1;
        // Matches Columbo's C implementation, whose counter is unsigned: an
        // odd overflow wraps after its last pair and repair continues until
        // no shorter code remains, which can leave an incomplete code.
        overflow = overflow.wrapping_sub(2);
    }

    // Assign the shortest lengths to the most frequent symbols. Equal
    // frequencies are ordered by symbol, matching the `qsort` comparator of
    // Columbo's C implementation; inverting the frequency sorts it descending.
    let mut leaves: Vec<u64> = frequencies
        .iter()
        .enumerate()
        .filter(|&(_, &frequency)| frequency != 0)
        .map(|(symbol, &frequency)| leaf_key(!frequency, symbol as u32))
        .collect();
    sort_leaf_keys(&mut leaves);

    lengths.fill(0);
    let mut leaf = 0;
    for (bits, &count) in count_by_length.iter().enumerate().take(limit + 1).skip(1) {
        for _ in 0..count {
            if leaf == leaves.len() {
                return;
            }
            lengths[leaf_key_tie(leaves[leaf]) as usize] = bits as u8;
            leaf += 1;
        }
    }
}

fn columbo_rle_frequency_bucket(frequency: u32) -> u32 {
    if frequency == 0 {
        return 0;
    }

    // Round to about two significant bits, in u64 so rounding cannot overflow.
    let highest_bit = u32::BITS - 1 - frequency.leading_zeros();
    let quantum = 1_u64 << highest_bit.saturating_sub(1);
    let rounded = ((u64::from(frequency) + quantum / 2) / quantum) * quantum;
    rounded.min(u64::from(u32::MAX)) as u32
}

/// Build Columbo's adjacency-based RLE-friendly Huffman pseudofrequencies.
///
/// The idea comes from Zopfli's `OptimizeHuffmanForRle` (via Turtledeflate);
/// this quantizer is Columbo's own. Callers of all three smoothers must price
/// the resulting code against the true frequencies.
pub(crate) fn make_columbo_rle_pseudofrequencies<const N: usize>(frequencies: &mut [u32; N]) {
    let mut buckets = [0_u32; N];
    for (bucket, &frequency) in buckets.iter_mut().zip(frequencies.iter()) {
        *bucket = columbo_rle_frequency_bucket(frequency);
    }

    for symbol in 0..N {
        if frequencies[symbol] == 0 {
            continue;
        }
        let bucket = buckets[symbol];
        let matches_left = symbol != 0 && buckets[symbol - 1] == bucket;
        let matches_right = symbol + 1 < N && buckets[symbol + 1] == bucket;
        if matches_left || matches_right {
            frequencies[symbol] = bucket;
        }
    }
}

/// Mark runs of equal counts that the Zopfli and Brotli smoothers keep. The
/// caller excludes trailing zeros and passes a cleared buffer.
fn mark_rle_runs(frequencies: &[u32], protected: &mut [bool]) {
    let mut run_start = 0;
    while run_start < frequencies.len() {
        let count = frequencies[run_start];
        let mut run_end = run_start + 1;
        while run_end < frequencies.len() && frequencies[run_end] == count {
            run_end += 1;
        }
        let run_length = run_end - run_start;
        if (count == 0 && run_length >= 5) || (count != 0 && run_length >= 7) {
            protected[run_start..run_end].fill(true);
        }
        run_start = run_end;
    }
}

/// Independent implementation of Zopfli's `OptimizeHuffmanForRle`: keep useful
/// runs of equal counts, then replace each long enough stretch of similar
/// counts with its rounded mean.
///
/// Algorithm: Google Zopfli, Copyright 2011 Google Inc., Lode Vandevenne and
/// Jyrki Alakuijala, Apache License 2.0.
///
/// One copy serves both alphabets; neither has more than 286 symbols.
pub(crate) fn make_zopfli_rle_pseudofrequencies(frequencies: &mut [u32]) {
    let Some(last_nonzero) = frequencies.iter().rposition(|&frequency| frequency != 0) else {
        return;
    };
    let length = last_nonzero + 1;
    let mut good_for_rle = [false; 286];
    mark_rle_runs(&frequencies[..length], &mut good_for_rle[..length]);

    let mut stride = 0_usize;
    let mut limit = frequencies[0];
    let mut sum = 0_u64;
    for symbol in 0..=length {
        let ends_stride =
            symbol == length || good_for_rle[symbol] || frequencies[symbol].abs_diff(limit) >= 4;
        if ends_stride {
            if stride >= 4 || (stride >= 3 && sum == 0) {
                let average = if sum == 0 {
                    0
                } else {
                    ((sum + stride as u64 / 2) / stride as u64).max(1)
                };
                frequencies[symbol - stride..symbol].fill(average as u32);
            }

            stride = 0;
            sum = 0;
            limit = if symbol + 3 < length {
                let lookahead = frequencies[symbol..symbol + 4]
                    .iter()
                    .map(|&frequency| u64::from(frequency))
                    .sum::<u64>();
                ((lookahead + 2) / 4) as u32
            } else if symbol < length {
                frequencies[symbol]
            } else {
                0
            };
        }

        stride += 1;
        if symbol != length {
            sum += u64::from(frequencies[symbol]);
        }
    }
}

/// Independent implementation of Brotli's fixed-point smoother: keep useful
/// runs, and replace long stretches whose counts stay close to a moving mean
/// (24.8 fixed point) by that mean. Trailing zero symbols are never introduced.
///
/// Algorithm: Google Brotli, Copyright 2010 Google Inc., MIT License; ECT
/// applies it to Deflate codes.
pub(crate) fn make_brotli_rle_pseudofrequencies<const N: usize>(frequencies: &mut [u32; N]) {
    let Some(last_nonzero) = frequencies.iter().rposition(|&frequency| frequency != 0) else {
        return;
    };
    let length = last_nonzero + 1;
    let mut protected = [false; N];
    mark_rle_runs(&frequencies[..length], &mut protected[..length]);

    const STREAK_LIMIT: u64 = 1_240;
    let initial_count = frequencies[..length.min(3)]
        .iter()
        .map(|&frequency| u64::from(frequency))
        .sum::<u64>();
    let mut limit = 256 * initial_count / length.min(3) as u64 + 420;
    let mut stride = 0_usize;
    let mut sum = 0_u64;

    for symbol in 0..=length {
        let scaled = if symbol == length {
            0
        } else {
            256 * u64::from(frequencies[symbol])
        };
        let ends_stride = symbol == length
            || protected[symbol]
            || (symbol != 0 && protected[symbol - 1])
            || scaled.abs_diff(limit) >= STREAK_LIMIT;
        if ends_stride {
            if stride >= 4 {
                let average = if sum == 0 {
                    0
                } else {
                    ((sum + stride as u64 / 2) / stride as u64).max(1)
                };
                frequencies[symbol - stride..symbol].fill(average as u32);
            }

            stride = 0;
            sum = 0;
            limit = if symbol + 2 < length {
                256 * frequencies[symbol..symbol + 3]
                    .iter()
                    .map(|&frequency| u64::from(frequency))
                    .sum::<u64>()
                    / 3
                    + 420
            } else if symbol < length {
                256 * u64::from(frequencies[symbol])
            } else {
                0
            };
        }

        stride += 1;
        if symbol != length {
            sum += u64::from(frequencies[symbol]);
            if stride >= 4 {
                limit = (256 * sum + stride as u64 / 2) / stride as u64;
            }
            if stride == 4 {
                limit += 120;
            }
        }
    }
}

/// Build [`make_lengths`] code lengths (variant 0), applying Defluff's
/// package-merge only when they exceed `max_bits`.
pub(crate) fn make_lengths_columbo_defluff_limited(
    frequencies: &[u32],
    max_bits: u8,
    variant: u32,
) -> Vec<u8> {
    let mut lengths = vec![0; frequencies.len()];
    make_lengths_columbo_defluff_limited_into(
        frequencies,
        &mut lengths,
        max_bits,
        variant,
        &mut TreeBuildScratch::default(),
    );
    lengths
}

/// Return whether the unlimited tree was deeper than `max_bits` and needed
/// package-merge.
pub(crate) fn make_lengths_columbo_defluff_limited_into(
    frequencies: &[u32],
    lengths: &mut [u8],
    max_bits: u8,
    _variant: u32,
    scratch: &mut TreeBuildScratch,
) -> bool {
    if !make_lengths_inner(frequencies, lengths, max_bits, 0, scratch) {
        return false;
    }
    apply_package_merge_with_scratch(
        frequencies,
        lengths,
        max_bits,
        PackageMergeTie::LeafFirst,
        &mut scratch.package_merge,
    );
    true
}

/// Build Defluff's code lengths exactly, including its leaf-before-branch tie
/// rule.
pub(crate) fn make_lengths_defluff_exact(
    frequencies: &[u32],
    max_bits: u8,
    variant: u32,
) -> Vec<u8> {
    let mut lengths = vec![0; frequencies.len()];
    make_lengths_defluff_exact_into(frequencies, &mut lengths, max_bits, variant);
    lengths
}

pub(crate) fn make_lengths_defluff_exact_into(
    frequencies: &[u32],
    lengths: &mut [u8],
    max_bits: u8,
    _variant: u32,
) {
    assert_eq!(frequencies.len(), lengths.len());
    let max_depth = make_lengths_defluff_unconstrained(frequencies, lengths);
    if max_depth <= usize::from(max_bits) {
        return;
    }

    // Start package-merge from the `make_lengths` result, kept if it fails.
    make_lengths_into(
        frequencies,
        lengths,
        max_bits,
        0,
        &mut TreeBuildScratch::default(),
    );
    apply_package_merge(frequencies, lengths, max_bits, PackageMergeTie::LeafFirst);
}

/// Run package-merge with Zopfli's package-first tie rule (Defluff takes the
/// leaf on equal weight), keeping `fallback` if package-merge fails. Only the
/// tie rule follows Zopfli, not its implementation.
pub(crate) fn make_lengths_zopfli_package_from(
    frequencies: &[u32],
    fallback: &[u8],
    max_bits: u8,
) -> Vec<u8> {
    assert_eq!(frequencies.len(), fallback.len());
    let mut lengths = fallback.to_vec();
    apply_package_merge(
        frequencies,
        &mut lengths,
        max_bits,
        PackageMergeTie::PackageFirst,
    );
    lengths
}

fn make_lengths_defluff_unconstrained(frequencies: &[u32], lengths: &mut [u8]) -> usize {
    lengths.fill(0);
    let mut nodes = Vec::with_capacity(frequencies.len().saturating_mul(2));
    nodes.extend(
        frequencies
            .iter()
            .copied()
            .enumerate()
            .filter_map(|(symbol, frequency)| {
                (frequency != 0).then_some(Node::leaf(frequency, symbol, symbol))
            }),
    );

    match nodes.len() {
        0 => return 0,
        1 => {
            lengths[nodes[0].symbol.expect("leaf has a symbol")] = 1;
            return 1;
        }
        _ => {}
    }

    nodes.sort_unstable_by(|a, b| {
        a.frequency
            .cmp(&b.frequency)
            .then_with(|| a.symbol.cmp(&b.symbol))
    });
    let leaf_count = nodes.len();
    let final_count = 2 * leaf_count - 1;
    let mut leaf_position = 0;
    let mut branch_position = leaf_count;

    // Two-queue merge of the sorted leaves and the new branches. Defluff
    // takes the leaf on equal weight (`<=`).
    while nodes.len() < final_count {
        let mut children = [0_usize; 2];
        for child in &mut children {
            let take_leaf = leaf_position < leaf_count
                && (branch_position >= nodes.len()
                    || nodes[leaf_position].frequency <= nodes[branch_position].frequency);
            if take_leaf {
                *child = leaf_position;
                leaf_position += 1;
            } else {
                *child = branch_position;
                branch_position += 1;
            }
        }
        let frequency = nodes[children[0]]
            .frequency
            .wrapping_add(nodes[children[1]].frequency);
        let index = nodes.len();
        nodes.push(Node::branch(frequency, children[0], children[1], index));
    }

    assign_depths(&nodes, nodes.len() - 1, 0, lengths)
}

#[derive(Debug, Clone, Copy)]
struct PackageNode {
    weight: u64,
    kind: PackageKind,
}

#[derive(Debug, Clone, Copy)]
enum PackageKind {
    Leaf(usize),
    Pair(usize, usize),
}

#[derive(Debug, Clone, Copy)]
enum PackageMergeTie {
    LeafFirst,
    PackageFirst,
}

fn apply_package_merge(
    frequencies: &[u32],
    lengths: &mut [u8],
    max_bits: u8,
    tie: PackageMergeTie,
) {
    apply_package_merge_with_scratch(
        frequencies,
        lengths,
        max_bits,
        tie,
        &mut PackageMergeScratch::default(),
    );
}

/// Package-merge's buffers, kept between calls; no state carries over.
#[derive(Default)]
struct PackageMergeScratch {
    nodes: Vec<PackageNode>,
    keys: Vec<u64>,
    leaves: Vec<usize>,
    previous: Vec<usize>,
    current: Vec<usize>,
    occurrences: Vec<u8>,
    candidate: Vec<u8>,
}

fn apply_package_merge_with_scratch(
    frequencies: &[u32],
    lengths: &mut [u8],
    max_bits: u8,
    tie: PackageMergeTie,
    scratch: &mut PackageMergeScratch,
) {
    let leaf_count = frequencies
        .iter()
        .filter(|&&frequency| frequency != 0)
        .count();
    if leaf_count < 2 || max_bits == 0 {
        return;
    }

    let PackageMergeScratch {
        nodes,
        keys,
        leaves,
        previous,
        current,
        occurrences,
        candidate,
    } = scratch;
    let target = 2 * leaf_count - 2;
    // The leaves, then fewer than `leaf_count` packages at each further level.
    nodes.clear();
    nodes.reserve(leaf_count.saturating_mul(usize::from(max_bits)));
    keys.clear();

    // Leaf node indices increase with symbol (zero counts are skipped), so
    // sorting once by (weight, index) breaks ties by symbol.
    for (symbol, &frequency) in frequencies.iter().enumerate() {
        if frequency == 0 {
            continue;
        }
        keys.push(leaf_key(frequency, nodes.len() as u32));
        nodes.push(PackageNode {
            weight: u64::from(frequency),
            kind: PackageKind::Leaf(symbol),
        });
    }
    sort_leaf_keys(keys);
    leaves.clear();
    leaves.extend(keys.iter().map(|&key| leaf_key_tie(key) as usize));

    previous.clear();
    previous.reserve(target);
    previous.extend_from_slice(leaves);
    for _level in 1..max_bits {
        let package_count = previous.len() / 2;
        let package_start = nodes.len();
        for pair in previous.chunks_exact(2) {
            nodes.push(PackageNode {
                weight: nodes[pair[0]].weight + nodes[pair[1]].weight,
                kind: PackageKind::Pair(pair[0], pair[1]),
            });
        }

        current.clear();
        current.reserve(target);
        let mut leaf_position = 0;
        let mut package_position = 0;
        while current.len() < target
            && (leaf_position < leaf_count || package_position < package_count)
        {
            let take_leaf = package_position >= package_count
                || (leaf_position < leaf_count
                    && match tie {
                        PackageMergeTie::LeafFirst => {
                            nodes[leaves[leaf_position]].weight
                                <= nodes[package_start + package_position].weight
                        }
                        PackageMergeTie::PackageFirst => {
                            nodes[leaves[leaf_position]].weight
                                < nodes[package_start + package_position].weight
                        }
                    });
            if take_leaf {
                current.push(leaves[leaf_position]);
                leaf_position += 1;
            } else {
                current.push(package_start + package_position);
                package_position += 1;
            }
        }
        std::mem::swap(previous, current);
    }

    if previous.len() < target {
        return;
    }
    // A leaf's length is how often it occurs in the selected nodes. Every
    // package follows the nodes it pairs, so one pass from the end hands each
    // node's count to its children before they are read.
    occurrences.clear();
    occurrences.resize(nodes.len(), 0);
    for &node in previous.iter().take(target) {
        occurrences[node] += 1;
    }
    candidate.clear();
    candidate.resize(frequencies.len(), 0);
    for (index, node) in nodes.iter().enumerate().rev() {
        let count = occurrences[index];
        if count == 0 {
            continue;
        }
        match node.kind {
            PackageKind::Leaf(symbol) => candidate[symbol] += count,
            PackageKind::Pair(left, right) => {
                occurrences[left] += count;
                occurrences[right] += count;
            }
        }
    }
    if package_lengths_are_valid(frequencies, candidate, max_bits) {
        lengths.copy_from_slice(candidate);
    }
}

fn package_lengths_are_valid(frequencies: &[u32], lengths: &[u8], max_bits: u8) -> bool {
    if max_bits == 0 || max_bits >= 63 {
        return false;
    }
    let slots = 1_u64 << max_bits;
    let mut used = 0_u64;
    for (&frequency, &length) in frequencies.iter().zip(lengths) {
        if frequency == 0 {
            if length != 0 {
                return false;
            }
            continue;
        }
        if length == 0 || length > max_bits {
            return false;
        }
        used += 1_u64 << (max_bits - length);
        if used > slots {
            return false;
        }
    }
    used == slots
}

/// Leaves a [`HeapEntry`] heap can hold: with their branches, at most
/// 65,535 nodes.
const MAX_PACKED_HEAP_LEAVES: usize = 1 << 15;

/// One live subtree in the DeflOpt-style heap, in one word. Leaves take
/// indices `0..leaf_count` in symbol order and each branch the next index, so
/// the index is also the node's creation order. The top 32 bits hold the
/// frequency, the next 16 the tie field (the height for the DeflOpt heap, the
/// index for the order-key heap) and the low 16 the index. Comparing the top
/// 48 bits orders entries by frequency, then by the tie field; the top 32
/// ignore ties.
#[derive(Debug, Clone, Copy)]
struct HeapEntry(u64);

impl HeapEntry {
    fn new(frequency: u32, tie: u32, index: u32) -> Self {
        debug_assert!(tie <= u32::from(u16::MAX) && index <= u32::from(u16::MAX));
        Self(u64::from(frequency) << 32 | u64::from(tie) << 16 | u64::from(index))
    }

    fn frequency(self) -> u32 {
        (self.0 >> 32) as u32
    }

    fn tie(self) -> u32 {
        u32::from((self.0 >> 16) as u16)
    }

    fn index(self) -> u32 {
        u32::from(self.0 as u16)
    }
}

/// Reusable buffers for the DeflOpt-style heap builder, deft4j's Java heap,
/// the generic node builder and package-merge; no state carries over between
/// builds.
#[derive(Default)]
pub(crate) struct TreeBuildScratch {
    heap: Vec<HeapEntry>,
    /// The symbol of each leaf, by leaf index.
    symbols: Vec<u32>,
    /// The two children of each branch, by branch index minus the leaf count.
    children: Vec<[u32; 2]>,
    depths: Vec<u32>,
    /// Leaf symbols in reverse heap-removal order, for overflow repair.
    leaf_order: Vec<u32>,
    nodes: Vec<Node>,
    /// The generic builder's live nodes.
    node_indices: Vec<usize>,
    java_heap: Vec<u64>,
    keyed_heaps: KeyedHeaps,
    reverse_front: ReverseFront,
    package_merge: PackageMergeScratch,
}

/// Sift-down comparisons whose outcome a tie rule could change. Every
/// comparison asks whether one entry is strictly less than another, so it
/// turns on the tie field only when the frequencies are equal and the entry
/// that would move up has the smaller field: then a whole-key comparison
/// moves it and a frequency-only one does not. Without `TRACK`, nothing is
/// noted.
#[derive(Clone, Copy, Default)]
struct TieTurns<const TRACK: bool> {
    /// Some comparison turned on the heap's own tie field: the height in the
    /// DeflOpt heap, the creation order in the order-key heap.
    tie: bool,
    /// Some comparison turned on the creation order.
    order: bool,
}

impl<const TRACK: bool> TieTurns<TRACK> {
    /// Note a comparison in which `rising` moves above `staying` only if it is
    /// strictly less.
    #[inline(always)]
    fn note(&mut self, rising: HeapEntry, staying: HeapEntry) {
        if TRACK {
            let tied = rising.frequency() == staying.frequency();
            self.tie |= tied & (rising.tie() < staying.tie());
            self.order |= tied & (rising.index() < staying.index());
        }
    }
}

/// Restore the heap order below `start`, comparing whole keys, or only
/// frequencies if `FREQUENCY_ONLY`, and note in `turns` each comparison
/// another tie rule could decide differently.
#[inline(always)]
fn heap_sift_down<const FREQUENCY_ONLY: bool, const TRACK: bool>(
    heap: &mut [HeapEntry],
    start: usize,
    turns: &mut TieTurns<TRACK>,
) {
    let rank = |entry: HeapEntry| {
        if FREQUENCY_ONLY {
            entry.0 >> 32
        } else {
            entry.0 >> 16
        }
    };
    let entry = heap[start];
    let entry_rank = rank(entry);
    let mut root = start;
    loop {
        let mut child = root * 2 + 1;
        if child >= heap.len() {
            break;
        }
        let mut child_rank = rank(heap[child]);
        if child + 1 < heap.len() {
            turns.note(heap[child + 1], heap[child]);
            let right_rank = rank(heap[child + 1]);
            if right_rank < child_rank {
                child += 1;
                child_rank = right_rank;
            }
        }
        turns.note(heap[child], entry);
        if child_rank >= entry_rank {
            break;
        }
        heap[root] = heap[child];
        root = child;
    }
    heap[root] = entry;
}

/// What a DeflOpt-style or order-key heap build learned besides its lengths.
#[derive(Clone, Copy, Debug)]
pub(crate) struct HeapBuild {
    /// The Huffman tree was deeper than `max_bits` and needed repair.
    pub(crate) repaired: bool,
    /// Every variant of the DeflOpt heap builds the same lengths. Only a build
    /// that tracks ties can tell.
    pub(crate) all_deflopt_variants: bool,
    /// Every variant of the order-key heap builds the same lengths. Only a
    /// build that tracks ties can tell.
    pub(crate) all_order_variants: bool,
}

pub(crate) fn make_lengths_deflopt_heap_into(
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
        false,
        false,
        &mut TreeBuildScratch::default(),
    );
}

/// `track_ties` asks which other variants build the same lengths, at some
/// cost to this build.
pub(crate) fn make_lengths_deflopt_heap_into_with_scratch(
    frequencies: &[u32],
    lengths: &mut [u8],
    max_bits: u8,
    variant: u32,
    track_ties: bool,
    scratch: &mut TreeBuildScratch,
) -> HeapBuild {
    make_lengths_variant_heap_inner(
        frequencies,
        lengths,
        max_bits,
        variant,
        false,
        track_ties,
        scratch,
    )
}

/// `track_ties` as for [`make_lengths_deflopt_heap_into_with_scratch`].
pub(crate) fn make_lengths_order_heap_into_with_scratch(
    frequencies: &[u32],
    lengths: &mut [u8],
    max_bits: u8,
    variant: u32,
    track_ties: bool,
    scratch: &mut TreeBuildScratch,
) -> HeapBuild {
    make_lengths_variant_heap_inner(
        frequencies,
        lengths,
        max_bits,
        variant,
        true,
        track_ties,
        scratch,
    )
}

/// Build one heap variant's lengths. All variants of both heaps start from the
/// same frequency-ordered heap, and each merge sift-down differs only in its
/// tie rule. When no comparison of this build turned on a rule, every variant
/// whose rules agree with it everywhere takes the same steps, removes the
/// entries in the same order and so builds, and repairs, the same tree.
fn make_lengths_variant_heap_inner(
    frequencies: &[u32],
    lengths: &mut [u8],
    max_bits: u8,
    variant: u32,
    use_order_tie: bool,
    track_ties: bool,
    scratch: &mut TreeBuildScratch,
) -> HeapBuild {
    assert_eq!(frequencies.len(), lengths.len());
    // Every node index and height fits a packed entry's 16 bits.
    assert!(
        frequencies.len() <= MAX_PACKED_HEAP_LEAVES,
        "alphabet fits the heap"
    );
    lengths.fill(0);
    // Fewer than two leaves take the same early return in every variant.
    let mut build = HeapBuild {
        repaired: false,
        all_deflopt_variants: true,
        all_order_variants: true,
    };

    let TreeBuildScratch {
        heap,
        symbols,
        children,
        depths,
        leaf_order,
        ..
    } = scratch;
    heap.clear();
    symbols.clear();
    children.clear();
    // Grow fresh buffers once rather than by doubling.
    heap.reserve(frequencies.len());
    symbols.reserve(frequencies.len());
    children.reserve(frequencies.len());
    for (symbol, &frequency) in frequencies.iter().enumerate() {
        if frequency != 0 {
            let index = u32::try_from(symbols.len()).expect("alphabet fits u32");
            // Leaves have height 0.
            let tie = if use_order_tie { index } else { 0 };
            heap.push(HeapEntry::new(frequency, tie, index));
            symbols.push(u32::try_from(symbol).expect("alphabet fits u32"));
        }
    }
    let leaf_count = symbols.len();
    match leaf_count {
        0 => return build,
        1 => {
            lengths[symbols[0] as usize] = 1;
            return build;
        }
        _ => {}
    }

    // Every variant orders the initial heap by frequency alone.
    for start in (0..heap.len() / 2).rev() {
        heap_sift_down::<true, false>(heap, start, &mut TieTurns::default());
    }

    // Variant bit 1 sets the tie rule of the sift-down after removing the
    // first child, and bit 0 the one after inserting the parent; a set bit
    // compares frequencies only.
    let leaves = leaf_count as u32;
    // Only variant 0, which every tracked caller asks for first, has a
    // tracking merge; the others report every comparison as turned.
    let mut untracked = TieTurns::<false>::default();
    let (tie, order) = match (variant & 3, track_ties) {
        (0, true) => {
            let mut turns = TieTurns::<true>::default();
            merge_heap::<false, false, true>(heap, children, leaves, use_order_tie, &mut turns);
            (turns.tie, turns.order)
        }
        (0, false) => {
            merge_heap::<false, false, false>(
                heap,
                children,
                leaves,
                use_order_tie,
                &mut untracked,
            );
            (true, true)
        }
        (1, _) => {
            merge_heap::<false, true, false>(heap, children, leaves, use_order_tie, &mut untracked);
            (true, true)
        }
        (2, _) => {
            merge_heap::<true, false, false>(heap, children, leaves, use_order_tie, &mut untracked);
            (true, true)
        }
        _ => {
            merge_heap::<true, true, false>(heap, children, leaves, use_order_tie, &mut untracked);
            (true, true)
        }
    };
    // In the order-key heap the tie field is the creation order. Without
    // turns on either field, both heaps follow the frequency-only steps.
    build.all_deflopt_variants = !use_order_tie && !tie;
    build.all_order_variants = !tie && !order;

    // Every branch is created after both children, so one pass from the root,
    // the last branch, down to the first assigns every depth.
    depths.clear();
    depths.resize(leaf_count + children.len(), 0);
    for (branch, &[left, right]) in children.iter().enumerate().rev() {
        let depth = depths[leaf_count + branch] + 1;
        depths[left as usize] = depth;
        depths[right as usize] = depth;
    }
    let mut max_depth = 0;
    for (&symbol, &depth) in symbols.iter().zip(depths.iter()) {
        lengths[symbol as usize] = depth.min(u32::from(u8::MAX)) as u8;
        max_depth = max_depth.max(depth as usize);
    }
    if max_depth <= usize::from(max_bits) {
        return build;
    }
    // Each branch's children are the two entries removed before creating
    // it, so the branches in reverse, right child first, give the leaves in
    // reverse removal order.
    leaf_order.clear();
    for &[left, right] in children.iter().rev() {
        for child in [right, left] {
            if child < leaves {
                leaf_order.push(symbols[child as usize]);
            }
        }
    }
    repair_deflopt_overflow(leaf_order, lengths, max_bits, max_depth);
    build.repaired = true;
    build
}

/// Merge the two least entries until one remains, recording each branch's
/// children. `FIRST_FREQUENCY_ONLY` and `SECOND_FREQUENCY_ONLY` choose the tie
/// rule of the sift-downs after removing the first child and after inserting
/// the parent.
fn merge_heap<
    const FIRST_FREQUENCY_ONLY: bool,
    const SECOND_FREQUENCY_ONLY: bool,
    const TRACK: bool,
>(
    heap: &mut Vec<HeapEntry>,
    children: &mut Vec<[u32; 2]>,
    leaves: u32,
    use_order_tie: bool,
    turns: &mut TieTurns<TRACK>,
) {
    while heap.len() > 1 {
        let left = heap[0];
        let last = heap.pop().expect("heap is non-empty");
        heap[0] = last;
        heap_sift_down::<FIRST_FREQUENCY_ONLY, TRACK>(heap, 0, turns);

        let right = heap[0];
        let index = leaves + u32::try_from(children.len()).expect("alphabet fits u32");
        children.push([left.index(), right.index()]);
        let frequency = left.frequency().wrapping_add(right.frequency());
        let tie = if use_order_tie {
            index
        } else {
            left.tie().max(right.tie()) + 1
        };
        heap[0] = HeapEntry::new(frequency, tie, index);
        heap_sift_down::<SECOND_FREQUENCY_ONLY, TRACK>(heap, 0, turns);
    }
}

fn repair_deflopt_overflow(leaf_order: &[u32], lengths: &mut [u8], max_bits: u8, max_depth: usize) {
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
            lengths[leaf_order[leaf] as usize] = bits as u8;
            leaf += 1;
        }
    }
}

/// A heap entry: the node's frequency in the high half and its index in the
/// low half, so comparisons, which deft4j makes on frequency alone, need not
/// read the node.
fn java_heap_entry(nodes: &[Node], node: usize) -> u64 {
    debug_assert!(u32::try_from(node).is_ok());
    u64::from(nodes[node].frequency) << 32 | node as u64
}

fn java_heap_frequency(entry: u64) -> u64 {
    entry >> 32
}

fn java_heap_node(entry: u64) -> usize {
    entry as u32 as usize
}

fn java_heap_offer(nodes: &[Node], heap: &mut Vec<u64>, node: usize) {
    let entry = java_heap_entry(nodes, node);
    let mut position = heap.len();
    heap.push(entry);
    while position > 0 {
        let parent = (position - 1) >> 1;
        // `java.util.PriorityQueue.siftUpComparable` stops on compare >= 0,
        // and deft4j compares only frequency, so an equal node stays put.
        if java_heap_frequency(entry) >= java_heap_frequency(heap[parent]) {
            break;
        }
        heap[position] = heap[parent];
        position = parent;
    }
    heap[position] = entry;
}

fn java_heap_sift_down(heap: &mut [u64], mut position: usize, entry: u64) {
    let half = heap.len() >> 1;
    while position < half {
        let mut child = position * 2 + 1;
        let mut candidate = heap[child];
        let right = child + 1;
        // `PriorityQueue` sift-down chooses the right child only when it is
        // strictly smaller.
        if right < heap.len() && java_heap_frequency(candidate) > java_heap_frequency(heap[right]) {
            child = right;
            candidate = heap[child];
        }
        if java_heap_frequency(entry) <= java_heap_frequency(candidate) {
            break;
        }
        heap[position] = candidate;
        position = child;
    }
    heap[position] = entry;
}

fn java_heap_poll(heap: &mut Vec<u64>) -> Option<usize> {
    let result = *heap.first()?;
    let last = heap.pop().expect("heap is non-empty");
    if !heap.is_empty() {
        java_heap_sift_down(heap, 0, last);
    }
    Some(java_heap_node(result))
}

/// Build the code lengths deft4j's `java.util.PriorityQueue` builder produces,
/// falling back to the DeflOpt heap builder (variant 0) if that fails.
pub(crate) fn make_lengths_deft4j_java_heap(frequencies: &[u32], max_bits: u8) -> Vec<u8> {
    let mut lengths = vec![0; frequencies.len()];
    make_lengths_deft4j_java_heap_into(
        frequencies,
        &mut lengths,
        max_bits,
        &mut TreeBuildScratch::default(),
    );
    lengths
}

pub(crate) fn make_lengths_deft4j_java_heap_into(
    frequencies: &[u32],
    lengths: &mut [u8],
    max_bits: u8,
    scratch: &mut TreeBuildScratch,
) {
    assert_eq!(frequencies.len(), lengths.len());
    lengths.fill(0);

    let TreeBuildScratch {
        nodes,
        java_heap: heap,
        depths,
        ..
    } = scratch;
    nodes.clear();
    heap.clear();
    // A tree has fewer than two nodes per leaf, and deft4j pads to two
    // leaves; overflow repair may add nodes.
    let leaves = frequencies.len().max(2);
    nodes.reserve(leaves * 2);
    heap.reserve(leaves);
    for (symbol, &frequency) in frequencies.iter().enumerate() {
        if frequency == 0 {
            continue;
        }
        let index = nodes.len();
        nodes.push(Node::leaf(frequency, symbol, index));
        java_heap_offer(nodes, heap, index);
    }

    // Like deft4j's `HuffmanTree`, add unused symbols with weight 1 until
    // there are two leaves.
    let mut symbol = 0;
    while heap.len() < 2 && symbol < frequencies.len() {
        if frequencies[symbol] == 0 {
            let index = nodes.len();
            nodes.push(Node::leaf(1, symbol, index));
            java_heap_offer(nodes, heap, index);
        }
        symbol += 1;
    }

    match heap.len() {
        0 => return,
        1 => {
            lengths[nodes[java_heap_node(heap[0])]
                .symbol
                .expect("leaf has a symbol")] = 1;
            return;
        }
        _ => {}
    }

    let initial_count = heap.len();
    for _ in 0..initial_count - 1 {
        let Some(left) = java_heap_poll(heap) else {
            make_lengths_deflopt_heap_into(frequencies, lengths, max_bits, 0);
            return;
        };
        let Some(right) = java_heap_poll(heap) else {
            make_lengths_deflopt_heap_into(frequencies, lengths, max_bits, 0);
            return;
        };

        let index = nodes.len();
        let mut parent = Node::branch(
            nodes[left].frequency.wrapping_add(nodes[right].frequency),
            left,
            right,
            index,
        );
        parent.height = nodes[left].height.max(nodes[right].height) + 1;
        nodes[left].parent = Some(index);
        nodes[left].side = 0;
        nodes[right].parent = Some(index);
        nodes[right].side = 1;
        nodes.push(parent);
        java_heap_offer(nodes, heap, index);
    }

    let root = java_heap_node(heap[0]);
    debug_assert_eq!(root, nodes.len() - 1);
    let max_depth = assign_depths_from_last(nodes, depths, lengths);
    if max_depth <= usize::from(max_bits) {
        return;
    }

    // deft4j reshapes the tree rather than redistributing length counts.
    if !deft4j_repair_overlong_tree(nodes, root, lengths, max_bits) {
        make_lengths_deflopt_heap_into(frequencies, lengths, max_bits, 0);
    }
}

fn deft4j_repair_overlong_tree(
    nodes: &mut Vec<Node>,
    root: usize,
    lengths: &mut [u8],
    max_bits: u8,
) -> bool {
    lengths.fill(0);
    let mut max_depth = assign_depths(nodes, root, 0, lengths);

    while max_depth > usize::from(max_bits) {
        let Some(leaf_a) = first_leaf_at_depth(nodes, root, 0, max_depth) else {
            return false;
        };
        let Some(parent_one) = nodes[leaf_a].parent else {
            return false;
        };
        let leaf_b = if nodes[leaf_a].side == 0 {
            nodes[parent_one].right
        } else {
            nodes[parent_one].left
        };
        let Some(leaf_b) = leaf_b else {
            return false;
        };
        if nodes[leaf_b].symbol.is_none() {
            return false;
        }

        let Some(parent_two) = nodes[parent_one].parent else {
            return false;
        };
        if nodes[parent_one].side == 0 {
            nodes[parent_two].left = Some(leaf_b);
            nodes[leaf_b].side = 0;
        } else {
            nodes[parent_two].right = Some(leaf_b);
            nodes[leaf_b].side = 1;
        }
        nodes[leaf_b].parent = Some(parent_two);

        let mut moved = false;
        if max_depth >= 3 {
            for depth in (1..=max_depth - 2).rev() {
                let Some(leaf_c) = first_leaf_at_depth(nodes, root, 0, depth) else {
                    continue;
                };
                let Some(parent_three) = nodes[leaf_c].parent else {
                    return false;
                };
                if nodes.len() >= 1024 {
                    return false;
                }

                let new_side = nodes[leaf_c].side;
                let index = nodes.len();
                let mut branch = Node::branch(
                    nodes[leaf_a]
                        .frequency
                        .wrapping_add(nodes[leaf_c].frequency),
                    leaf_a,
                    leaf_c,
                    index,
                );
                branch.parent = Some(parent_three);
                branch.side = new_side;
                branch.height = nodes[leaf_a].height.max(nodes[leaf_c].height) + 1;

                if new_side == 0 {
                    nodes[parent_three].left = Some(index);
                } else {
                    nodes[parent_three].right = Some(index);
                }
                nodes[leaf_a].parent = Some(index);
                nodes[leaf_a].side = 0;
                nodes[leaf_c].parent = Some(index);
                nodes[leaf_c].side = 1;
                nodes.push(branch);
                moved = true;
                break;
            }
        }
        if !moved {
            return false;
        }

        lengths.fill(0);
        max_depth = assign_depths(nodes, root, 0, lengths);
    }
    true
}

#[cfg(test)]
pub(crate) mod test_support;
#[cfg(test)]
mod tests;
