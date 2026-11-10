// SPDX-License-Identifier: MIT
// SPDX-FileCopyrightText: 2026 Andrew C.E. Dent <https://github.com/ace-dent>

//! Code-length codes for a header's RLE histogram: the builders of the
//! 19-symbol code, at most 7 bits, and each thread's cache of the trees they
//! build. Each builder is a pure function of the histogram, so a cached tree,
//! kept with what a header search asks of it, serves later headers with the
//! same histogram.

use std::cell::RefCell;

use super::{
    code_lengths_follow_frequencies, rle_frequency_cost, trim_code_lengths,
    FREQUENCY_ONLY_HEAP_VARIANT, INF,
};
use crate::deflate::huffman::{
    code_length_tree_shape_is_valid, make_code_length_lengths_variant_heap,
    make_lengths_columbo_defluff_limited_into, make_lengths_defluff_exact_into,
    make_lengths_deft4j_java_heap_into, make_lengths_into, HeapBuild, TreeBuildScratch,
};

/// A builder of code-length codes: 19 symbols, at most 7 bits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum CodeLengthBuilder {
    DefloptHeap(u32),
    OrderHeap(u32),
    Deft4jJavaHeap,
    DefluffLimited,
    Columbo(u32),
    DefluffExact,
}

impl CodeLengthBuilder {
    /// Trees in each [`CodeLengthTreeEntry`].
    const COUNT: usize = 14;

    /// This builder's tree in a [`CodeLengthTreeEntry`], or `None` for a
    /// variant no header search asks for. Variant 3 of both heap builders
    /// breaks every tie by frequency alone, so the two build the same tree.
    fn index(self) -> Option<usize> {
        let index = match self {
            Self::DefloptHeap(FREQUENCY_ONLY_HEAP_VARIANT)
            | Self::OrderHeap(FREQUENCY_ONLY_HEAP_VARIANT) => 3,
            Self::DefloptHeap(variant @ 0..=2) => variant,
            Self::OrderHeap(variant @ 0..=2) => 4 + variant,
            Self::Deft4jJavaHeap => 7,
            Self::DefluffLimited => 8,
            Self::Columbo(variant @ 0..=3) => 9 + variant,
            Self::DefluffExact => 13,
            Self::DefloptHeap(_) | Self::OrderHeap(_) | Self::Columbo(_) => return None,
        };
        Some(index as usize)
    }

    /// Build this builder's tree. `track_ties` asks a heap builder which
    /// other heap variants build the same tree.
    fn build(
        self,
        frequencies: &[u32; 19],
        track_ties: bool,
        heap_scratch: &mut TreeBuildScratch,
    ) -> BuiltCodeLengthTree {
        let mut lengths = [0_u8; 19];
        let mut shared = 0;
        let mut huffman = false;
        let mut heap_build = |build: HeapBuild| {
            huffman = !build.repaired;
            // DeflOpt variant `v` has index `v`, order-key variants 0 to 2
            // follow at 4, and both heaps' variant 3 is index 3.
            let order = u16::from(build.order_variants);
            shared |= u16::from(build.deflopt_variants) | (order & 0b111) << 4 | (order & 0b1000);
        };
        match self {
            Self::DefloptHeap(variant) => heap_build(make_code_length_lengths_variant_heap(
                frequencies,
                &mut lengths,
                variant,
                false,
                track_ties,
            )),
            Self::OrderHeap(variant) => heap_build(make_code_length_lengths_variant_heap(
                frequencies,
                &mut lengths,
                variant,
                true,
                track_ties,
            )),
            Self::Deft4jJavaHeap => {
                make_lengths_deft4j_java_heap_into(frequencies, &mut lengths, 7, heap_scratch)
            }
            Self::DefluffLimited => {
                if !make_lengths_columbo_defluff_limited_into(
                    frequencies,
                    &mut lengths,
                    7,
                    0,
                    heap_scratch,
                ) {
                    shared = UNLIMITED_TREES;
                }
            }
            Self::Columbo(variant) => {
                let repaired =
                    make_lengths_into(frequencies, &mut lengths, 7, variant, heap_scratch);
                if variant == 0 && !repaired {
                    shared = UNLIMITED_TREES;
                }
            }
            Self::DefluffExact => {
                make_lengths_defluff_exact_into(frequencies, &mut lengths, 7, 0, heap_scratch)
            }
        }
        BuiltCodeLengthTree {
            lengths,
            shared,
            huffman,
        }
    }
}

/// `DefluffLimited` and `Columbo(0)` start from the same variant-0 Huffman
/// tree and differ only in how they limit one deeper than 7 bits.
const UNLIMITED_TREES: u16 = 1 << 8 | 1 << 9;

/// A code-length code a builder gave one RLE histogram.
struct BuiltCodeLengthTree {
    lengths: [u8; 19],
    /// The [`CodeLengthBuilder::index`] bits of other builders known to build
    /// the same tree for this histogram.
    shared: u16,
    /// Whether a heap builder's Huffman tree fit 7 bits unrepaired, so no
    /// prefix code spells the histogram's symbols in fewer bits.
    huffman: bool,
}

/// A code-length code for one RLE histogram, with what a header search asks
/// of it. Each is fixed by the lengths and the histogram, so a cached tree
/// keeps them and later searches need not work them out again.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct CodeLengthTree {
    pub(super) lengths: [u8; 19],
    /// HCLEN, from [`trim_code_lengths`].
    pub(super) hclen: u8,
    /// Whether a heap builder's Huffman tree fit 7 bits unrepaired, as
    /// [`BuiltCodeLengthTree`] describes.
    pub(super) huffman: bool,
    /// [`code_length_tree_shape_is_valid`].
    pub(super) valid: bool,
    /// [`code_lengths_follow_frequencies`] for the histogram.
    pub(super) follows: bool,
    /// [`rle_frequency_cost`] of the histogram, or `u32::MAX` for [`INF`];
    /// a finite cost is at most 316 tokens of 14 bits.
    rle_bits: u32,
}

const EMPTY_CODE_LENGTH_TREE: CodeLengthTree = CodeLengthTree {
    lengths: [0; 19],
    hclen: 0,
    huffman: false,
    valid: false,
    follows: false,
    rle_bits: 0,
};

impl CodeLengthTree {
    fn new(lengths: [u8; 19], huffman: bool, frequencies: &[u32; 19]) -> Self {
        let rle_bits = rle_frequency_cost(frequencies, &lengths);
        Self {
            lengths,
            hclen: trim_code_lengths(&lengths) as u8,
            huffman,
            valid: code_length_tree_shape_is_valid(&lengths),
            follows: code_lengths_follow_frequencies(&lengths, frequencies),
            rle_bits: u32::try_from(rle_bits).unwrap_or(u32::MAX),
        }
    }

    /// [`rle_frequency_cost`] of the histogram.
    pub(super) fn rle_bits(&self) -> u64 {
        if self.rle_bits == u32::MAX {
            INF
        } else {
            u64::from(self.rle_bits)
        }
    }
}

/// One RLE histogram's code-length trees, by [`CodeLengthBuilder::index`].
#[derive(Clone, Copy)]
pub(super) struct CodeLengthTreeEntry {
    frequencies: [u32; 19],
    /// Bit `index` is set once `trees[index]` holds that builder's tree.
    built: u16,
    trees: [CodeLengthTree; CodeLengthBuilder::COUNT],
}

const EMPTY_CODE_LENGTH_TREE_ENTRY: CodeLengthTreeEntry = CodeLengthTreeEntry {
    frequencies: [0; 19],
    built: 0,
    trees: [EMPTY_CODE_LENGTH_TREE; CodeLengthBuilder::COUNT],
};

/// Entries in each thread's code-length tree cache, 472 bytes each, about
/// 1.8 MiB. Header plans for different code lengths often share an RLE
/// histogram, and repeats come close together. Storing each histogram once,
/// with all its builders' trees, holds more trees than one slot per builder
/// and histogram: in a completing Max run, this many entries built 10% fewer
/// trees than 16,384 such slots, and twice the entries 5% fewer again.
const CODE_LENGTH_TREE_CACHE_BITS: u32 = 12;

thread_local! {
    /// Code-length trees built on this thread, direct-mapped by RLE histogram.
    /// Each builder is a pure function of the histogram, so a hit, which
    /// compares the complete histogram, returns the tree it would build.
    pub(super) static CODE_LENGTH_TREES: RefCell<Option<Box<[CodeLengthTreeEntry]>>> =
        const { RefCell::new(None) };
}

/// Whether two RLE histograms are equal, without a call to the C library's
/// comparison: the differences are combined, so the comparison vectorizes.
fn same_histogram(left: &[u32; 19], right: &[u32; 19]) -> bool {
    left.iter()
        .zip(right)
        .fold(0, |differences, (left, right)| differences | (left ^ right))
        == 0
}

fn code_length_tree_slot(frequencies: &[u32; 19]) -> usize {
    const MULTIPLIER: u64 = 0x9e37_79b9_7f4a_7c15;
    // Four lanes, so the multiplications overlap instead of each waiting for
    // the last; a hit still compares the whole histogram.
    let mut lanes = [0_u64, 1, 2, 3];
    for (index, &frequency) in frequencies.iter().enumerate() {
        let lane = &mut lanes[index % lanes.len()];
        *lane = (*lane ^ u64::from(frequency)).wrapping_mul(MULTIPLIER);
    }
    let hash = lanes
        .iter()
        .fold(0_u64, |hash, &lane| (hash ^ lane).wrapping_mul(MULTIPLIER));
    (hash >> (u64::BITS - CODE_LENGTH_TREE_CACHE_BITS)) as usize
}

/// Build the code-length code `builder` gives `frequencies`, or return the one
/// this thread last built for the same builder and histogram.
pub(super) fn code_length_tree(
    builder: CodeLengthBuilder,
    frequencies: &[u32; 19],
    heap_scratch: &mut TreeBuildScratch,
) -> [u8; 19] {
    CodeLengthTrees::new(frequencies).get(builder, heap_scratch)
}

/// One RLE histogram's slot in this thread's code-length tree cache, found
/// once for every builder a header search asks for.
pub(super) struct CodeLengthTrees<'a> {
    frequencies: &'a [u32; 19],
    slot: usize,
}

impl<'a> CodeLengthTrees<'a> {
    pub(super) fn new(frequencies: &'a [u32; 19]) -> Self {
        Self {
            frequencies,
            slot: code_length_tree_slot(frequencies),
        }
    }

    /// [`code_length_tree`] for this histogram.
    fn get(&self, builder: CodeLengthBuilder, heap_scratch: &mut TreeBuildScratch) -> [u8; 19] {
        self.get_tree(builder, heap_scratch).lengths
    }

    /// [`Self::get`], and whether the tree is an unrepaired Huffman tree.
    #[cfg(test)]
    fn get_with_huffman(
        &self,
        builder: CodeLengthBuilder,
        heap_scratch: &mut TreeBuildScratch,
    ) -> ([u8; 19], bool) {
        let tree = self.get_tree(builder, heap_scratch);
        (tree.lengths, tree.huffman)
    }

    /// The tree [`Self::get`] returns, with what a header search asks of it.
    pub(super) fn get_tree(
        &self,
        builder: CodeLengthBuilder,
        heap_scratch: &mut TreeBuildScratch,
    ) -> CodeLengthTree {
        let Self { frequencies, slot } = *self;
        let Some(index) = builder.index() else {
            let built = builder.build(frequencies, false, heap_scratch);
            return CodeLengthTree::new(built.lengths, built.huffman, frequencies);
        };
        let cached = CODE_LENGTH_TREES
            .try_with(|cache| {
                let cache = cache.try_borrow().ok()?;
                let entry = cache.as_ref()?.get(slot)?;
                if !same_histogram(&entry.frequencies, frequencies) {
                    return None;
                }
                (entry.built & (1 << index) != 0).then_some(entry.trees[index])
            })
            .ok()
            .flatten();
        if let Some(cached) = cached {
            return cached;
        }

        // Every heap build tracks which variants share its tree. Noting ties
        // adds work to each comparison, but a histogram's later variants
        // mostly follow an earlier build's steps, and the builds this skips
        // cost more.
        let BuiltCodeLengthTree {
            lengths,
            shared,
            huffman,
        } = builder.build(frequencies, true, heap_scratch);
        let tree = CodeLengthTree::new(lengths, huffman, frequencies);
        let _ = CODE_LENGTH_TREES.try_with(|cache| {
            let Ok(mut cache) = cache.try_borrow_mut() else {
                return;
            };
            if cache.is_none() {
                let mut entries = Vec::new();
                if entries
                    .try_reserve_exact(1 << CODE_LENGTH_TREE_CACHE_BITS)
                    .is_err()
                {
                    return;
                }
                entries.resize(
                    1 << CODE_LENGTH_TREE_CACHE_BITS,
                    EMPTY_CODE_LENGTH_TREE_ENTRY,
                );
                *cache = Some(entries.into_boxed_slice());
            }
            let Some(entry) = cache.as_mut().and_then(|entries| entries.get_mut(slot)) else {
                return;
            };
            if !same_histogram(&entry.frequencies, frequencies) {
                entry.frequencies = *frequencies;
                entry.built = 0;
            }
            let mut indices = shared | 1 << index;
            while indices != 0 {
                let index = indices.trailing_zeros() as usize;
                indices &= indices - 1;
                entry.trees[index] = tree;
                entry.built |= 1 << index;
            }
        });
        tree
    }
}

#[cfg(test)]
mod tests;
