// SPDX-License-Identifier: MIT

use std::sync::OnceLock;

use super::bitstream::BitReader;
use crate::{Error, Result};

const MAX_CODE_BITS: usize = 15;
const MAX_C_CODES: usize = 320;
const MAX_TRACKED_DEPTH: usize = 63;

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
/// Deflate alphabets contain at most 288 symbols. A linear code list keeps the
/// construction and malformed-stream checks easy to audit, and exactly matches
/// the original Columbo C implementation's decoder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Huffman {
    codes: Vec<HuffCode>,
    max_bits: u8,
}

impl Huffman {
    /// Build canonical codes from code lengths.
    ///
    /// `None` means that a length exceeds Deflate's 15-bit limit, the tree is
    /// oversubscribed, or the alphabet exceeds the original Columbo C decoder's
    /// 320-code table limit. Incomplete trees are valid: Deflate uses them for
    /// one-symbol trees.
    pub(crate) fn build(lengths: &[u8]) -> Option<Self> {
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

        let mut next_code = [0_u32; MAX_CODE_BITS + 1];
        let mut code = 0_u32;
        for bits in 1..=MAX_CODE_BITS {
            code = (code + count_by_length[bits - 1]) << 1;
            if code + count_by_length[bits] > (1_u32 << bits) {
                return None;
            }
            next_code[bits] = code;
        }

        let mut codes = Vec::with_capacity(populated);
        for (symbol, &length) in lengths.iter().enumerate() {
            if length == 0 {
                continue;
            }
            let symbol = u16::try_from(symbol).ok()?;
            let index = usize::from(length);
            codes.push(HuffCode {
                symbol,
                length,
                code: reverse_bits(next_code[index] as u16, length),
            });
            next_code[index] += 1;
        }

        Some(Self { codes, max_bits })
    }

    /// Decode one symbol, consuming no more than the tree's maximum length.
    pub(crate) fn decode(&self, reader: &mut BitReader<'_>) -> Result<u16> {
        let mut code = 0_u16;
        for length in 1..=self.max_bits {
            code |= (reader.read(1)? as u16) << (length - 1);
            if let Some(entry) = self
                .codes
                .iter()
                .find(|entry| entry.length == length && entry.code == code)
            {
                return Ok(entry.symbol);
            }
        }
        Err(Error::new("invalid Huffman code in Deflate stream"))
    }

    /// Look up a symbol for emission.
    pub(crate) fn code(&self, symbol: usize) -> Option<HuffCode> {
        let symbol = u16::try_from(symbol).ok()?;
        self.codes
            .iter()
            .copied()
            .find(|entry| entry.symbol == symbol)
    }

    #[cfg(test)]
    pub(crate) fn max_bits(&self) -> u8 {
        self.max_bits
    }
}

fn reverse_bits(mut code: u16, length: u8) -> u16 {
    let mut reversed = 0_u16;
    for _ in 0..length {
        reversed = (reversed << 1) | (code & 1);
        code >>= 1;
    }
    reversed
}

/// Return the RFC 1951 fixed literal/length and distance trees.
///
/// `OnceLock` avoids rebuilding and reallocating these tables for every fixed
/// block while still leaving their construction visible and testable.
pub(crate) fn fixed_trees() -> (&'static Huffman, &'static Huffman) {
    static FIXED: OnceLock<(Huffman, Huffman)> = OnceLock::new();
    let (literal_length, distance) = FIXED.get_or_init(|| {
        let mut literal_lengths = [0_u8; 288];
        literal_lengths[..144].fill(8);
        literal_lengths[144..256].fill(9);
        literal_lengths[256..280].fill(7);
        literal_lengths[280..].fill(8);

        let distance_lengths = [5_u8; 32];
        (
            Huffman::build(&literal_lengths).expect("fixed literal tree is valid"),
            Huffman::build(&distance_lengths).expect("fixed distance tree is valid"),
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
        0 => return,
        1 => {
            lengths[nodes[active[0]].symbol.expect("leaf has a symbol")] = 1;
            return;
        }
        _ => {}
    }

    while active.len() > 1 {
        let first_position = (1..active.len()).fold(0, |best, position| {
            if node_less(&nodes, active[position], active[best], variant) {
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
            if node_less(&nodes, active[position], active[best], second_variant) {
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

    let max_depth = assign_depths(&nodes, active[0], 0, lengths);
    if max_depth <= usize::from(max_bits) {
        return;
    }
    limit_generic_lengths(frequencies, lengths, max_bits);
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

pub(crate) fn tree_exceeds_limit(frequencies: &[u32], max_bits: u8) -> bool {
    unconstrained_huffman_max_depth(frequencies) > usize::from(max_bits)
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
    make_lengths_into(frequencies, lengths, max_bits, 0);
    if !tree_exceeds_limit(frequencies, max_bits) {
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
    fn canonical_builder_rejects_malformed_trees() {
        assert!(Huffman::build(&[]).is_none());
        assert!(Huffman::build(&[16]).is_none());
        assert!(Huffman::build(&[1, 1, 1]).is_none());
        assert!(Huffman::build(&[0, 0]).is_some());
    }

    #[test]
    fn fixed_trees_have_rfc_1951_lengths() {
        let (literal, distance) = fixed_trees();
        assert_eq!(literal.max_bits(), 9);
        assert_eq!(distance.max_bits(), 5);
        assert_eq!(literal.code(0).unwrap().length, 8);
        assert_eq!(literal.code(143).unwrap().length, 8);
        assert_eq!(literal.code(144).unwrap().length, 9);
        assert_eq!(literal.code(256).unwrap().length, 7);
        assert_eq!(literal.code(280).unwrap().length, 8);
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
    fn deft4j_adds_a_dummy_leaf_for_a_single_symbol() {
        let frequencies = [0, 7, 0, 0];
        assert_eq!(make_lengths(&frequencies, 3, 0), [0, 1, 0, 0]);
        assert_eq!(make_lengths_deft4j_java_heap(&frequencies, 3), [1, 1, 0, 0]);
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
