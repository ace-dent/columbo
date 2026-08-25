// SPDX-License-Identifier: MIT

use std::collections::VecDeque;
use std::sync::OnceLock;

use super::bitstream::BitReader;
use crate::{Error, Result};

const MAX_CODE_BITS: usize = 15;
const MAX_C_CODES: usize = 320;
const MAX_TRACKED_DEPTH: usize = 63;
const COMPLETE_HUFFMAN_CODE_SPACE: u32 = 1 << MAX_CODE_BITS;
const DEFAULT_DECODE_ROOT_BITS: u8 = 9;
// Payload alphabets use independently measured lookup/build/cache tradeoffs.
// Code-length trees use the default but naturally cap at their seven-bit max.
pub(crate) const LITERAL_LENGTH_DECODE_ROOT_BITS: u8 = 10;
pub(crate) const DISTANCE_DECODE_ROOT_BITS: u8 = 8;

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

/// RFC 1951 fixed literal/length code lengths, including reserved symbols
/// 286 and 287 because they are part of the predefined Huffman alphabet.
pub(crate) const FIXED_LITERAL_CODE_LENGTHS: [u8; 288] = make_fixed_literal_code_lengths();
/// RFC 1951 fixed distance code lengths, including reserved symbols 30 and 31.
pub(crate) const FIXED_DISTANCE_CODE_LENGTHS: [u8; 32] = [5; 32];

/// Whether code lengths occupy the complete canonical Huffman code space.
pub(crate) fn huffman_tree_shape_is_complete(lengths: &[u8]) -> bool {
    let mut occupied = 0_u32;
    for &length in lengths {
        if length == 0 {
            continue;
        }
        let Some(shift) = MAX_CODE_BITS.checked_sub(usize::from(length)) else {
            return false;
        };
        let Some(updated) = occupied.checked_add(1_u32 << shift) else {
            return false;
        };
        occupied = updated;
    }
    occupied == COMPLETE_HUFFMAN_CODE_SPACE
}

/// Code-length alphabets have no empty or one-symbol exceptions.
pub(crate) fn code_length_tree_shape_is_valid(lengths: &[u8]) -> bool {
    huffman_tree_shape_is_complete(lengths)
}

/// Validate the tree shapes accepted for Deflate payload alphabets.
///
/// A populated literal/length or distance tree is normally complete. RFC 1951
/// explicitly permits a one-bit singleton distance tree; the zlib-compatible
/// profile also accepts a one-bit singleton literal/end tree. `allow_empty`
/// admits the literal-only distance-alphabet case.
pub(crate) fn payload_tree_shape_is_valid(lengths: &[u8], allow_empty: bool) -> bool {
    let populated = lengths.iter().filter(|&&length| length != 0).count();
    match populated {
        0 => allow_empty,
        1 => lengths.contains(&1),
        _ => huffman_tree_shape_is_complete(lengths),
    }
}

/// Validate that lengths can form a canonical Deflate Huffman table without
/// allocating the emission and decode structures built by `Huffman::build`.
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

/// One canonical Huffman code in the bit order used on the Deflate wire.
///
/// Deflate writes bits least-significant bit first, so `code` is the reverse
/// of the conventional, most-significant-bit-first canonical representation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct HuffCode {
    pub(crate) symbol: u16,
    pub(crate) length: u8,
    pub(crate) code: u16,
}

/// A small canonical Huffman table.
///
/// Deflate alphabets contain at most 288 payload symbols. Emission retains a
/// direct symbol-indexed code table, while decoding uses one canonical range
/// per possible bit length so malformed input cannot trigger alphabet-wide
/// scans.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Huffman {
    /// Emission entries are indexed directly by symbol; zero length is unused.
    codes: Vec<HuffCode>,
    /// Canonical-code ranges allow decoding in at most fifteen comparisons,
    /// independent of the alphabet size.
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

#[derive(Debug, Clone, PartialEq, Eq)]
struct DecodeTable {
    entries: Vec<DecodeEntry>,
    root_bits: u8,
}

impl Huffman {
    /// Build canonical codes from code lengths.
    ///
    /// `None` means that a length exceeds Deflate's 15-bit limit, the tree is
    /// oversubscribed, or the alphabet exceeds the original Columbo C decoder's
    /// 320-code table limit. Construction can represent an incomplete tree;
    /// callers apply the alphabet-specific complete, empty, or singleton rule.
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

    /// Build the same canonical representation plus the parser's two-level
    /// decode table. Planner and emitter trees use `build()` so they do not pay
    /// this allocation cost.
    pub(crate) fn build_decoder(lengths: &[u8]) -> Option<Self> {
        Self::build_decoder_with_root_bits(lengths, DEFAULT_DECODE_ROOT_BITS)
    }

    pub(crate) fn build_decoder_with_root_bits(lengths: &[u8], root_bits: u8) -> Option<Self> {
        let mut tree = Self::build(lengths)?;
        tree.decode_table = Some(build_decode_table(&tree, root_bits)?);
        Some(tree)
    }

    /// Build a payload decoder whose table entries also carry the base value
    /// and extra-bit width associated with each symbol. This lets parsing
    /// consume a codeword and its extra field as one logical operation.
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

    /// Decode one symbol, consuming no more than the tree's maximum length.
    pub(crate) fn decode(&self, reader: &mut BitReader<'_>) -> Result<u16> {
        if let Some(table) = &self.decode_table {
            return self.decode_table(reader, table);
        }
        self.decode_canonical(reader)
    }

    /// Decode a profiled symbol together with its following extra-bit field.
    pub(crate) fn decode_value(&self, reader: &mut BitReader<'_>) -> Result<DecodedValue> {
        let profile = self
            .decode_profile
            .ok_or_else(|| Error::new("internal Huffman decoder has no value profile"))?;
        if let Some(table) = &self.decode_table {
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

        // Near the end of a stream there may be enough input for the actual
        // codeword but not for the table's wider root lookup. Preserve the
        // canonical fallback and consume its extra field separately.
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

    fn decode_table(&self, reader: &mut BitReader<'_>, table: &DecodeTable) -> Result<u16> {
        if let Some((entry, bits)) = self.peek_decode_table(reader, table)? {
            reader.drop_bits(bits)?;
            return Ok(entry.symbol());
        }
        self.decode_canonical(reader)
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
            // Huffman bits arrive in canonical most-significant-bit order,
            // even though fixed-width Deflate fields are least-significant
            // bit first. Building the canonical value incrementally avoids a
            // bit reversal and an alphabet-wide scan at every depth.
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

    /// Look up a symbol for emission.
    pub(crate) fn code(&self, symbol: usize) -> Option<HuffCode> {
        let code = *self.codes.get(symbol)?;
        (code.length != 0).then_some(code)
    }

    #[cfg(test)]
    pub(crate) fn max_bits(&self) -> u8 {
        self.max_bits
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

    // Grow the direct root table one address bit at a time. Duplicating the
    // already populated prefix keeps the inherited shorter codes contiguous
    // in memory; codes introduced at this width then replace their one exact
    // slot. The resulting bit-reversed lookup layout is identical to filling
    // every symbol's widely spaced suffixes independently.
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

    // Install subtable pointers only after the root has been expanded so the
    // contiguous copies above cannot replicate a pointer into another prefix.
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

/// Return the RFC 1951 fixed literal/length and distance trees.
///
/// `OnceLock` avoids rebuilding the decode tables for every fixed block. The
/// underlying code lengths are shared with every planner that prices a fixed
/// representation, so the RFC table has a single definition.
pub(crate) fn fixed_trees() -> (&'static Huffman, &'static Huffman) {
    static FIXED: OnceLock<(Huffman, Huffman)> = OnceLock::new();
    let (literal_length, distance) = FIXED.get_or_init(|| {
        (
            Huffman::build_decoder(&FIXED_LITERAL_CODE_LENGTHS)
                .expect("fixed literal tree is valid"),
            Huffman::build_decoder(&FIXED_DISTANCE_CODE_LENGTHS)
                .expect("fixed distance tree is valid"),
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

fn generic_heap_sift_down(nodes: &[Node], heap: &mut [usize], mut root: usize, variant: u32) {
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

fn generic_heap_pop(nodes: &[Node], heap: &mut Vec<usize>, variant: u32) -> Option<usize> {
    let result = *heap.first()?;
    let last = heap.pop().expect("the heap is non-empty");
    if !heap.is_empty() {
        heap[0] = last;
        generic_heap_sift_down(nodes, heap, 0, variant);
    }
    Some(result)
}

fn generic_heap_push(nodes: &[Node], heap: &mut Vec<usize>, node: usize, variant: u32) {
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

#[cfg(test)]
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

fn merge_generic_nodes_heap(nodes: &mut Vec<Node>, active: &mut Vec<usize>, variant: u32) {
    for start in (0..active.len() / 2).rev() {
        generic_heap_sift_down(nodes, active, start, variant);
    }
    while active.len() > 1 {
        let left = generic_heap_pop(nodes, active, variant).expect("two nodes remain");
        let right = generic_heap_pop(nodes, active, variant).expect("one node remains");
        let index = nodes.len();
        let frequency = nodes[left].frequency.wrapping_add(nodes[right].frequency);
        nodes.push(Node::branch(frequency, left, right, index));
        generic_heap_push(nodes, active, index, variant);
    }
}

fn generic_heap_pop_active(
    nodes: &[Node],
    heap: &mut Vec<usize>,
    active: &[bool],
    variant: u32,
) -> Option<usize> {
    loop {
        let node = generic_heap_pop(nodes, heap, variant)?;
        if active[node] {
            return Some(node);
        }
    }
}

/// Merge variants whose first and second child reverse their equal-frequency
/// preference. Each node enters both total-order heaps; removing it only marks
/// the shared active bit, and the other heap discards that stale entry when it
/// reaches its front. This preserves the two distinct choices without an
/// active-set scan or deletion from the middle of a heap.
fn merge_generic_nodes_dual_heap(nodes: &mut Vec<Node>, active: &mut Vec<usize>, variant: u32) {
    debug_assert_ne!(variant & 2, 0);
    let mut increasing_order = active.clone();
    let mut decreasing_order = active.clone();
    for start in (0..increasing_order.len() / 2).rev() {
        generic_heap_sift_down(nodes, &mut increasing_order, start, 0);
        generic_heap_sift_down(nodes, &mut decreasing_order, start, 1);
    }

    let leaf_count = active.len();
    let mut is_active = Vec::with_capacity(leaf_count.saturating_mul(2));
    is_active.resize(nodes.len(), true);
    let mut remaining = leaf_count;
    while remaining > 1 {
        let reverse_first = variant & 1 != 0;
        let left = if reverse_first {
            generic_heap_pop_active(nodes, &mut decreasing_order, &is_active, 1)
        } else {
            generic_heap_pop_active(nodes, &mut increasing_order, &is_active, 0)
        }
        .expect("two nodes remain");
        is_active[left] = false;

        let right = if reverse_first {
            generic_heap_pop_active(nodes, &mut increasing_order, &is_active, 0)
        } else {
            generic_heap_pop_active(nodes, &mut decreasing_order, &is_active, 1)
        }
        .expect("one node remains");
        is_active[right] = false;

        let index = nodes.len();
        let frequency = nodes[left].frequency.wrapping_add(nodes[right].frequency);
        nodes.push(Node::branch(frequency, left, right, index));
        is_active.push(true);
        generic_heap_push(nodes, &mut increasing_order, index, 0);
        generic_heap_push(nodes, &mut decreasing_order, index, 1);
        remaining -= 1;
    }

    let root = generic_heap_pop_active(nodes, &mut increasing_order, &is_active, 0)
        .expect("one root remains");
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

fn merge_generic_nodes_two_front(nodes: &mut Vec<Node>, active: &mut Vec<usize>, variant: u32) {
    active.sort_unstable_by(|&left, &right| {
        if node_less(nodes, left, right, variant) {
            std::cmp::Ordering::Less
        } else if node_less(nodes, right, left, variant) {
            std::cmp::Ordering::Greater
        } else {
            std::cmp::Ordering::Equal
        }
    });

    // The sorted leaves occupy the original prefix. Newly formed branches
    // have nondecreasing frequencies and increasing creation order, so variant
    // zero can consume each front exactly once without maintaining a heap.
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

/// Variant one prefers the newest node within an equal-frequency group. The
/// leaves are sorted once; generated branch frequencies are monotonic, so one
/// linked stack per frequency run exposes that newest branch in constant time.
fn merge_generic_nodes_reverse_front(nodes: &mut Vec<Node>, active: &mut Vec<usize>) {
    active.sort_unstable_by(|&left, &right| {
        if node_less(nodes, left, right, 1) {
            std::cmp::Ordering::Less
        } else if node_less(nodes, right, left, 1) {
            std::cmp::Ordering::Greater
        } else {
            std::cmp::Ordering::Equal
        }
    });

    let leaves = active.clone();
    let mut leaf = 0;
    let mut runs = VecDeque::with_capacity(leaves.len());
    let mut previous = vec![None; nodes.len()];
    let mut remaining = leaves.len();
    while remaining > 1 {
        let left = take_reverse_front(nodes, &leaves, &mut leaf, &mut runs, &previous);
        let right = take_reverse_front(nodes, &leaves, &mut leaf, &mut runs, &previous);
        let index = nodes.len();
        let frequency = nodes[left].frequency.wrapping_add(nodes[right].frequency);
        nodes.push(Node::branch(frequency, left, right, index));
        push_reverse_branch(&mut runs, &mut previous, index, frequency);
        remaining -= 1;
    }

    let root = take_reverse_front(nodes, &leaves, &mut leaf, &mut runs, &previous);
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

/// Build the generic Columbo Huffman-length candidate.
pub(crate) fn make_lengths(frequencies: &[u32], max_bits: u8, variant: u32) -> Vec<u8> {
    let mut lengths = vec![0; frequencies.len()];
    make_lengths_into(frequencies, &mut lengths, max_bits, variant);
    lengths
}

/// Fill a caller-owned buffer with the generic Columbo length candidate.
pub(crate) fn make_lengths_into(
    frequencies: &[u32],
    lengths: &mut [u8],
    max_bits: u8,
    variant: u32,
) {
    make_lengths_inner(frequencies, lengths, max_bits, variant);
}

/// Return whether the unconstrained tree exceeded `max_bits` before its
/// generic repair was applied.
fn make_lengths_inner(frequencies: &[u32], lengths: &mut [u8], max_bits: u8, variant: u32) -> bool {
    assert_eq!(frequencies.len(), lengths.len());
    lengths.fill(0);

    let mut nodes = Vec::with_capacity(frequencies.len().saturating_mul(2));
    let mut active = Vec::with_capacity(frequencies.len());
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

    // Variant zero's increasing tie order makes both its leaf and newly formed
    // branch fronts monotonic. Use ordered fronts for variants zero and one
    // when no wrapped frequency sum can invalidate that property. Variants two
    // and three use one heap for each of their opposing child tie orders.
    let total_frequency: u64 = frequencies
        .iter()
        .map(|&frequency| u64::from(frequency))
        .sum();
    if variant <= 1 && total_frequency <= u64::from(u32::MAX) {
        if variant == 0 {
            merge_generic_nodes_two_front(&mut nodes, &mut active, variant);
        } else {
            merge_generic_nodes_reverse_front(&mut nodes, &mut active);
        }
    } else if variant & 2 == 0 {
        merge_generic_nodes_heap(&mut nodes, &mut active, variant);
    } else {
        merge_generic_nodes_dual_heap(&mut nodes, &mut active, variant);
    }

    let max_depth = assign_depths(&nodes, active[0], 0, lengths);
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

    // This is zlib's familiar overflow repair: split a shorter code into two
    // children, then reclaim one slot at the maximum permitted depth.
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
        // The original Columbo C counter is unsigned. An odd overflow wraps
        // after its last pair and keeps repairing until no shorter code
        // remains. Preserve that observable, occasionally incomplete-tree
        // behavior.
        overflow = overflow.wrapping_sub(2);
    }

    let mut leaves: Vec<(u32, usize)> = frequencies
        .iter()
        .copied()
        .enumerate()
        .filter_map(|(symbol, frequency)| (frequency != 0).then_some((frequency, symbol)))
        .collect();
    // Longest repaired lengths go to the least frequent symbols. Symbol is a
    // deterministic ascending tie-break, matching Columbo's original C
    // `qsort` comparator.
    leaves.sort_unstable_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));

    lengths.fill(0);
    let mut leaf = 0;
    for bits in (1..=limit).rev() {
        for _ in 0..count_by_length[bits] {
            if leaf == leaves.len() {
                return;
            }
            lengths[leaves[leaf].1] = bits as u8;
            leaf += 1;
        }
    }
}

#[cfg(test)]
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

#[cfg(test)]
pub(crate) fn tree_exceeds_limit(frequencies: &[u32], max_bits: u8) -> bool {
    unconstrained_huffman_max_depth(frequencies) > usize::from(max_bits)
}

fn columbo_rle_frequency_bucket(frequency: u32) -> u32 {
    if frequency == 0 {
        return 0;
    }

    // Retain roughly two leading significant bits. A u64 calculation makes
    // rounding safe even for a count at u32::MAX.
    let highest_bit = u32::BITS - 1 - frequency.leading_zeros();
    let quantum = 1_u64 << highest_bit.saturating_sub(1);
    let rounded = ((u64::from(frequency) + quantum / 2) / quantum) * quantum;
    rounded.min(u64::from(u32::MAX)) as u32
}

/// Construct Columbo's adjacency-aware Huffman pseudofrequencies.
///
/// Zopfli's `OptimizeHuffmanForRle`, which Turtledeflate embeds with Zopfli's
/// attribution, motivates trying an alternate tree from RLE-friendly weights.
/// This quantizer is Columbo-original: it rounds counts onto a logarithmic grid
/// and changes a nonzero count only when an immediate neighbour shares its
/// bucket. It does not implement Zopfli's run marking or stride averaging.
/// Callers must score the resulting tree against the original frequencies.
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

/// Construct Zopfli-compatible RLE-friendly Huffman pseudofrequencies.
///
/// This independently implements the published behavior of Zopfli's
/// `OptimizeHuffmanForRle`: preserve already-useful equal-count runs, then
/// replace sufficiently long nearby-count strides by their rounded mean.
/// Trailing zero symbols remain untouched. Callers must build an alternate
/// tree from these weights and score that tree against the original counts.
///
/// Algorithm source and authorship: Google Zopfli, Copyright 2011 Google Inc.,
/// Lode Vandevenne and Jyrki Alakuijala, Apache License 2.0.
pub(crate) fn make_zopfli_rle_pseudofrequencies<const N: usize>(frequencies: &mut [u32; N]) {
    let Some(last_nonzero) = frequencies.iter().rposition(|&frequency| frequency != 0) else {
        return;
    };
    let length = last_nonzero + 1;
    let mut good_for_rle = [false; N];

    let mut run_start = 0;
    while run_start < length {
        let count = frequencies[run_start];
        let mut run_end = run_start + 1;
        while run_end < length && frequencies[run_end] == count {
            run_end += 1;
        }
        let run_length = run_end - run_start;
        if (count == 0 && run_length >= 5) || (count != 0 && run_length >= 7) {
            good_for_rle[run_start..run_end].fill(true);
        }
        run_start = run_end;
    }

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

/// Build Columbo's generic tree with Defluff's package-list depth limiter.
///
/// This is a Columbo hybrid, not Defluff's complete tree builder: it keeps
/// Columbo's ordinary tree whenever that tree already satisfies `max_bits`.
pub(crate) fn make_lengths_columbo_defluff_limited(
    frequencies: &[u32],
    max_bits: u8,
    variant: u32,
) -> Vec<u8> {
    let mut lengths = vec![0; frequencies.len()];
    make_lengths_columbo_defluff_limited_into(frequencies, &mut lengths, max_bits, variant);
    lengths
}

pub(crate) fn make_lengths_columbo_defluff_limited_into(
    frequencies: &[u32],
    lengths: &mut [u8],
    max_bits: u8,
    _variant: u32,
) {
    if !make_lengths_inner(frequencies, lengths, max_bits, 0) {
        return;
    }
    apply_defluff_package_merge(frequencies, lengths, max_bits);
}

/// Build the exact Defluff tree, including its leaf-before-branch tie rule.
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

    // Defluff enters its allocating package-list path only for an over-depth
    // ordinary tree. Keep a valid bounded tree if reconstruction cannot win.
    make_lengths_into(frequencies, lengths, max_bits, 0);
    apply_defluff_package_merge(frequencies, lengths, max_bits);
}

fn make_lengths_defluff_unconstrained(frequencies: &[u32], lengths: &mut [u8]) -> usize {
    lengths.fill(0);
    let mut nodes: Vec<Node> = frequencies
        .iter()
        .copied()
        .enumerate()
        .filter_map(|(symbol, frequency)| {
            (frequency != 0).then_some(Node::leaf(frequency, symbol, symbol))
        })
        .collect();

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

    // Merge a sorted leaf queue with the queue of completed branches. Defluff
    // chooses a leaf on equal weight (`<=`), unlike several other candidates.
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
    symbol: Option<usize>,
    left: Option<usize>,
    right: Option<usize>,
}

fn apply_defluff_package_merge(frequencies: &[u32], lengths: &mut [u8], max_bits: u8) {
    let leaf_count = frequencies
        .iter()
        .filter(|&&frequency| frequency != 0)
        .count();
    if leaf_count < 2 || max_bits == 0 {
        return;
    }

    let target = 2 * leaf_count - 2;
    let mut nodes = Vec::new();
    let mut leaves = Vec::with_capacity(leaf_count);

    // Stable insertion by weight preserves ascending symbol order for ties.
    for (symbol, &frequency) in frequencies.iter().enumerate() {
        if frequency == 0 {
            continue;
        }
        let index = nodes.len();
        nodes.push(PackageNode {
            weight: u64::from(frequency),
            symbol: Some(symbol),
            left: None,
            right: None,
        });
        let mut position = leaves.len();
        leaves.push(index);
        while position > 0 && nodes[leaves[position - 1]].weight > nodes[index].weight {
            leaves[position] = leaves[position - 1];
            position -= 1;
        }
        leaves[position] = index;
    }

    let mut previous = leaves.clone();
    for _level in 1..max_bits {
        let package_count = previous.len() / 2;
        let package_start = nodes.len();
        for pair in previous.chunks_exact(2) {
            nodes.push(PackageNode {
                weight: nodes[pair[0]].weight + nodes[pair[1]].weight,
                symbol: None,
                left: Some(pair[0]),
                right: Some(pair[1]),
            });
        }

        let mut current = Vec::with_capacity(target);
        let mut leaf_position = 0;
        let mut package_position = 0;
        while current.len() < target
            && (leaf_position < leaf_count || package_position < package_count)
        {
            let take_leaf = package_position >= package_count
                || (leaf_position < leaf_count
                    && nodes[leaves[leaf_position]].weight
                        <= nodes[package_start + package_position].weight);
            if take_leaf {
                current.push(leaves[leaf_position]);
                leaf_position += 1;
            } else {
                current.push(package_start + package_position);
                package_position += 1;
            }
        }
        previous = current;
    }

    if previous.len() < target {
        return;
    }
    let mut candidate = vec![0_u8; frequencies.len()];
    for &node in previous.iter().take(target) {
        accumulate_package_lengths(&nodes, node, &mut candidate);
    }
    if package_lengths_are_valid(frequencies, &candidate, max_bits) {
        lengths.copy_from_slice(&candidate);
    }
}

fn accumulate_package_lengths(nodes: &[PackageNode], node_index: usize, lengths: &mut [u8]) {
    let node = nodes[node_index];
    if let Some(symbol) = node.symbol {
        lengths[symbol] += 1;
        return;
    }
    accumulate_package_lengths(nodes, node.left.expect("package has a left child"), lengths);
    accumulate_package_lengths(
        nodes,
        node.right.expect("package has a right child"),
        lengths,
    );
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

#[derive(Debug, Clone, Copy)]
enum HeapTie {
    FrequencyOnly,
    Height,
    Order,
}

fn heap_node_less(nodes: &[Node], a: usize, b: usize, tie: HeapTie) -> bool {
    if nodes[a].frequency != nodes[b].frequency {
        return nodes[a].frequency < nodes[b].frequency;
    }
    match tie {
        HeapTie::FrequencyOnly => false,
        HeapTie::Height => nodes[a].height < nodes[b].height,
        HeapTie::Order => nodes[a].order < nodes[b].order,
    }
}

fn heap_sift_down(nodes: &[Node], heap: &mut [usize], start: usize, tie: HeapTie) {
    let mut root = start;
    loop {
        let mut child = root * 2 + 1;
        if child >= heap.len() {
            return;
        }
        if child + 1 < heap.len() && heap_node_less(nodes, heap[child + 1], heap[child], tie) {
            child += 1;
        }
        if !heap_node_less(nodes, heap[child], heap[root], tie) {
            return;
        }
        heap.swap(root, child);
        root = child;
    }
}

fn heapify_frequency_only(nodes: &[Node], heap: &mut [usize]) {
    for start in (0..heap.len() / 2).rev() {
        heap_sift_down(nodes, heap, start, HeapTie::FrequencyOnly);
    }
}

/// Build the candidate produced by DeflOpt's frequency/height heap.
pub(crate) fn make_lengths_deflopt_heap(
    frequencies: &[u32],
    max_bits: u8,
    variant: u32,
) -> Vec<u8> {
    let mut lengths = vec![0; frequencies.len()];
    make_lengths_deflopt_heap_into(frequencies, &mut lengths, max_bits, variant);
    lengths
}

pub(crate) fn make_lengths_deflopt_heap_into(
    frequencies: &[u32],
    lengths: &mut [u8],
    max_bits: u8,
    variant: u32,
) {
    make_lengths_variant_heap_inner(frequencies, lengths, max_bits, variant, false);
}

/// Build Columbo's legacy order-key heap extension.
///
/// This intentionally is not labelled as DeflOpt parity: DeflOpt 2.07 breaks
/// frequency ties by subtree height, whereas the original Columbo C
/// implementation retained this earlier order-key interpretation as an
/// additive candidate.
pub(crate) fn make_lengths_order_heap(frequencies: &[u32], max_bits: u8, variant: u32) -> Vec<u8> {
    let mut lengths = vec![0; frequencies.len()];
    make_lengths_order_heap_into(frequencies, &mut lengths, max_bits, variant);
    lengths
}

pub(crate) fn make_lengths_order_heap_into(
    frequencies: &[u32],
    lengths: &mut [u8],
    max_bits: u8,
    variant: u32,
) {
    make_lengths_variant_heap_inner(frequencies, lengths, max_bits, variant, true);
}

fn make_lengths_variant_heap_inner(
    frequencies: &[u32],
    lengths: &mut [u8],
    max_bits: u8,
    variant: u32,
    use_order_tie: bool,
) {
    assert_eq!(frequencies.len(), lengths.len());
    lengths.fill(0);

    let mut nodes = Vec::with_capacity(frequencies.len().saturating_mul(2));
    let mut heap = Vec::with_capacity(frequencies.len());
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

    let mut leaf_order = vec![0_usize; leaf_count];
    let mut leaf_order_position = leaf_count;
    heapify_frequency_only(&nodes, &mut heap);

    while heap.len() > 1 {
        let left = heap[0];
        if nodes[left].symbol.is_some() {
            leaf_order_position -= 1;
            leaf_order[leaf_order_position] = left;
        }
        let last = heap.pop().expect("heap is non-empty");
        heap[0] = last;

        // Variant bit 1 controls repair after removing the first child.
        let first_tie = if variant & 2 != 0 {
            HeapTie::FrequencyOnly
        } else if use_order_tie {
            HeapTie::Order
        } else {
            HeapTie::Height
        };
        heap_sift_down(&nodes, &mut heap, 0, first_tie);

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

        // Variant bit 0 independently controls repair after parent insertion.
        let second_tie = if variant & 1 != 0 {
            HeapTie::FrequencyOnly
        } else if use_order_tie {
            HeapTie::Order
        } else {
            HeapTie::Height
        };
        heap_sift_down(&nodes, &mut heap, 0, second_tie);
    }

    debug_assert_eq!(leaf_order_position, 0);
    let max_depth = assign_depths(&nodes, heap[0], 0, lengths);
    if max_depth <= usize::from(max_bits) {
        return;
    }
    repair_deflopt_overflow(&nodes, &leaf_order, lengths, max_bits, max_depth);
}

fn repair_deflopt_overflow(
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
    // Split one shorter code into two and replace two deepest codes by their
    // parent until every count fits. This mirrors DeflOpt's 0x407a80 loop.
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

    // DeflOpt records leaves backwards as the heap first consumes them. After
    // repair, shorter lengths are assigned by walking that exact saved order.
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

fn java_heap_offer(nodes: &[Node], heap: &mut Vec<usize>, node: usize) {
    let mut position = heap.len();
    heap.push(node);
    while position > 0 {
        let parent = (position - 1) >> 1;
        // `java.util.PriorityQueue.siftUpComparable` stops on compare >= 0.
        // deft4j compares only frequency, so an equal node retains its current
        // heap relationship.
        if nodes[node].frequency >= nodes[heap[parent]].frequency {
            break;
        }
        heap[position] = heap[parent];
        position = parent;
    }
    heap[position] = node;
}

fn java_heap_sift_down(nodes: &[Node], heap: &mut [usize], mut position: usize, node: usize) {
    let half = heap.len() >> 1;
    while position < half {
        let mut child = position * 2 + 1;
        let mut candidate = heap[child];
        let right = child + 1;
        // `PriorityQueue` sift-down chooses the right child only when it is
        // strictly smaller.
        if right < heap.len() && nodes[candidate].frequency > nodes[heap[right]].frequency {
            child = right;
            candidate = heap[child];
        }
        if nodes[node].frequency <= nodes[candidate].frequency {
            break;
        }
        heap[position] = candidate;
        position = child;
    }
    heap[position] = node;
}

fn java_heap_poll(nodes: &[Node], heap: &mut Vec<usize>) -> Option<usize> {
    let result = *heap.first()?;
    let last = heap.pop().expect("heap is non-empty");
    if !heap.is_empty() {
        java_heap_sift_down(nodes, heap, 0, last);
    }
    Some(result)
}

/// Build the tree produced by deft4j's `java.util.PriorityQueue` path.
///
/// Columbo defensively falls back to DeflOpt's variant-zero tree if an
/// internally inconsistent heap or over-depth repair cannot be reconstructed.
pub(crate) fn make_lengths_deft4j_java_heap(frequencies: &[u32], max_bits: u8) -> Vec<u8> {
    let mut lengths = vec![0; frequencies.len()];
    make_lengths_deft4j_java_heap_into(frequencies, &mut lengths, max_bits);
    lengths
}

pub(crate) fn make_lengths_deft4j_java_heap_into(
    frequencies: &[u32],
    lengths: &mut [u8],
    max_bits: u8,
) {
    assert_eq!(frequencies.len(), lengths.len());
    lengths.fill(0);

    let mut nodes = Vec::with_capacity(1024);
    let mut heap = Vec::with_capacity(frequencies.len());
    for (symbol, &frequency) in frequencies.iter().enumerate() {
        if frequency == 0 {
            continue;
        }
        let index = nodes.len();
        nodes.push(Node::leaf(frequency, symbol, index));
        java_heap_offer(&nodes, &mut heap, index);
    }

    // `HuffmanTree` forces at least two leaves when the alphabet has spare
    // symbols. deft4j's payload-distance caller separately bypasses this raw
    // builder for zero- and one-symbol distance alphabets.
    let mut symbol = 0;
    while heap.len() < 2 && symbol < frequencies.len() {
        if frequencies[symbol] == 0 {
            let index = nodes.len();
            nodes.push(Node::leaf(1, symbol, index));
            java_heap_offer(&nodes, &mut heap, index);
        }
        symbol += 1;
    }

    match heap.len() {
        0 => return,
        1 => {
            lengths[nodes[heap[0]].symbol.expect("leaf has a symbol")] = 1;
            return;
        }
        _ => {}
    }

    let initial_count = heap.len();
    for _ in 0..initial_count - 1 {
        let Some(left) = java_heap_poll(&nodes, &mut heap) else {
            make_lengths_deflopt_heap_into(frequencies, lengths, max_bits, 0);
            return;
        };
        let Some(right) = java_heap_poll(&nodes, &mut heap) else {
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
        java_heap_offer(&nodes, &mut heap, index);
    }

    let root = heap[0];
    let max_depth = assign_depths(&nodes, root, 0, lengths);
    if max_depth <= usize::from(max_bits) {
        return;
    }

    // deft4j reshapes the explicit tree instead of merely redistributing its
    // length counts. Use the DeflOpt candidate only if that reshape fails.
    if !deft4j_repair_overlong_tree(&mut nodes, root, lengths, max_bits) {
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
mod tests {
    use super::*;
    use crate::deflate::bitstream::BitWriter;

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
        // Fibonacci-like weights force an over-depth ordinary tree at 4 bits.
        let frequencies = [1, 1, 2, 3, 5, 8, 13, 0];
        assert!(tree_exceeds_limit(&frequencies, 4));

        let candidates = [
            make_lengths(&frequencies, 4, 0),
            make_lengths_columbo_defluff_limited(&frequencies, 4, 0),
            make_lengths_defluff_exact(&frequencies, 4, 0),
            make_lengths_deflopt_heap(&frequencies, 4, 0),
            make_lengths_order_heap(&frequencies, 4, 0),
            make_lengths_deft4j_java_heap(&frequencies, 4),
        ];
        for lengths in candidates {
            assert_bounded_lengths(&frequencies, &lengths, 4);
        }
    }

    #[test]
    fn length_families_match_original_columbo_c_on_overflow() {
        let frequencies = [1, 1, 2, 3, 5, 8, 13, 0];

        // These vectors come directly from the original Columbo `huffman.c`.
        // In particular, the generic builder intentionally preserves its
        // odd-overflow result even though a complete bounded tree is available.
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
    fn caller_owned_output_matches_allocating_helpers() {
        let frequencies = [4, 1, 9, 2, 2, 0];
        let mut output = [99_u8; 6];
        make_lengths_deflopt_heap_into(&frequencies, &mut output, 4, 2);
        assert_eq!(output, make_lengths_deflopt_heap(&frequencies, 4, 2)[..]);

        make_lengths_defluff_exact_into(&frequencies, &mut output, 4, 0);
        assert_eq!(output, make_lengths_defluff_exact(&frequencies, 4, 0)[..]);

        make_lengths_columbo_defluff_limited_into(&frequencies, &mut output, 4, 0);
        assert_eq!(
            output,
            make_lengths_columbo_defluff_limited(&frequencies, 4, 0)[..]
        );
    }
}
