// SPDX-License-Identifier: MIT
// SPDX-FileCopyrightText: 2026 Andrew C.E. Dent <https://github.com/ace-dent>

//! Dynamic-header construction and exact bit accounting.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;

use super::huffman::{
    code_length_tree_shape_is_valid, huffman_code_lengths_are_valid,
    make_brotli_rle_pseudofrequencies, make_columbo_rle_pseudofrequencies, make_lengths,
    make_lengths_columbo_defluff_limited, make_lengths_columbo_defluff_limited_into,
    make_lengths_deflopt_heap_into_with_scratch, make_lengths_defluff_exact,
    make_lengths_defluff_exact_into, make_lengths_deft4j_java_heap,
    make_lengths_deft4j_java_heap_into, make_lengths_into,
    make_lengths_order_heap_into_with_scratch, make_lengths_zopfli_package_from,
    make_zopfli_rle_pseudofrequencies, payload_tree_shape_is_valid, TreeBuildScratch,
    FIXED_DISTANCE_CODE_LENGTHS, FIXED_LITERAL_CODE_LENGTHS,
};
use super::model::{
    token_extra_bits, try_clone_slice, DynamicPlan, RleToken, Token, CODE_LENGTH_ORDER,
    MAX_DYNAMIC_CODE_LENGTH_COUNT, RFC_DISTANCE_CODE_COUNT,
};
use super::plan_store::{Charged, PlanStore};
use super::stop::SearchStop;
use crate::sort::SortShared;

mod coupled;
mod response;
mod rotate;
mod tree;
pub(crate) use coupled::{plan_coupled_length_swaps, CoupledSwapBudget};
pub(crate) use response::{plan_header_response, plan_length_exchange, ResponseBudget};
pub(crate) use rotate::{plan_code_length_rotations, RotationBudget};
pub(crate) use tree::{plan_header_tree, HeaderTreeBudget};

const INF: u64 = u64::MAX / 4;
const MAX_HEADER_PLAN_CACHE_ENTRIES: usize = 512;
const REDUCED_PAYLOAD_TREE_DEPTHS: [u8; 2] = [10, 9];
const MAX_RESTRICTED_PAYLOAD_TREE_DEPTH: u8 = 14;

fn minimum_complete_tree_depth(frequencies: &[u32]) -> u8 {
    let populated = frequencies
        .iter()
        .filter(|&&frequency| frequency != 0)
        .count();
    if populated <= 1 {
        1
    } else {
        (usize::BITS - (populated - 1).leading_zeros()) as u8
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct HeaderPlanCacheStats {
    pub(crate) lookups: usize,
    pub(crate) hits: usize,
    pub(crate) misses: usize,
    pub(crate) inserts: usize,
    pub(crate) collision_checks: usize,
    pub(crate) saturated: usize,
}

struct CachedHeaderPlan {
    fingerprint: u64,
    next_same_hash: Option<usize>,
    exhaustive: bool,
    rle_mask: u8,
    /// A complete dynamic plan priced with zero payload bits.
    kernel: DynamicPlan,
}

/// Bounded, route-local cache of dynamic header plans priced without payload;
/// callers add the payload cost. Every hash hit compares the complete code
/// lengths and policy before reuse.
pub(crate) struct HeaderPlanCache {
    first_by_hash: HashMap<u64, usize>,
    entries: Vec<CachedHeaderPlan>,
    stats: HeaderPlanCacheStats,
    max_entries: usize,
}

impl Default for HeaderPlanCache {
    fn default() -> Self {
        Self::new()
    }
}

impl HeaderPlanCache {
    pub(crate) fn new() -> Self {
        Self {
            first_by_hash: HashMap::new(),
            entries: Vec::new(),
            stats: HeaderPlanCacheStats::default(),
            max_entries: MAX_HEADER_PLAN_CACHE_ENTRIES,
        }
    }

    fn price(
        &mut self,
        literal_lengths: &[u8],
        distance_lengths: &[u8],
        data_bits: u64,
        exhaustive: bool,
        rle_mask: u8,
    ) -> Option<DynamicPlan> {
        let fingerprint =
            header_plan_fingerprint(literal_lengths, distance_lengths, exhaustive, rle_mask);
        self.stats.lookups = self.stats.lookups.saturating_add(1);
        let mut candidate = self.first_by_hash.get(&fingerprint).copied();
        while let Some(index) = candidate {
            self.stats.collision_checks = self.stats.collision_checks.saturating_add(1);
            let entry = self.entries.get(index)?;
            let next = entry.next_same_hash;
            let matches = entry.fingerprint == fingerprint
                && entry.exhaustive == exhaustive
                && entry.rle_mask == rle_mask
                && entry.kernel.literal_lengths == literal_lengths
                && entry.kernel.distance_lengths == distance_lengths;
            if matches {
                let mut plan = entry.kernel.try_clone()?;
                plan.bits = plan.bits.checked_add(data_bits)?;
                self.stats.hits = self.stats.hits.saturating_add(1);
                return Some(plan);
            }
            candidate = next;
        }
        self.stats.misses = self.stats.misses.saturating_add(1);

        let mut plan = stored_header_plan_kernel(
            fingerprint,
            literal_lengths,
            distance_lengths,
            exhaustive,
            rle_mask,
        )?;
        let bits = plan.bits.checked_add(data_bits)?;
        self.insert(fingerprint, exhaustive, rle_mask, &plan);
        plan.bits = bits;
        Some(plan)
    }

    fn insert(&mut self, fingerprint: u64, exhaustive: bool, rle_mask: u8, kernel: &DynamicPlan) {
        if self.entries.len() >= self.max_entries {
            self.stats.saturated = self.stats.saturated.saturating_add(1);
            return;
        }
        if self.entries.try_reserve(1).is_err() || self.first_by_hash.try_reserve(1).is_err() {
            self.stats.saturated = self.stats.saturated.saturating_add(1);
            return;
        }
        let Some(kernel) = kernel.try_clone() else {
            return;
        };
        let next_same_hash = self.first_by_hash.get(&fingerprint).copied();
        let index = self.entries.len();
        self.entries.push(CachedHeaderPlan {
            fingerprint,
            next_same_hash,
            exhaustive,
            rle_mask,
            kernel,
        });
        self.first_by_hash.insert(fingerprint, index);
        self.stats.inserts = self.stats.inserts.saturating_add(1);
    }
}

/// Charged bytes of completed header plans each thread keeps across routes.
/// Allocator overhead roughly doubles the resident cost: on the Default
/// benchmark sample, peak memory rose by at most 20 MiB.
const MAX_THREAD_HEADER_PLAN_BYTES: usize = 8 << 20;

/// A dynamic plan packed into one allocation: the code lengths two per byte,
/// then each RLE token as its symbol and extra-bits value.
pub(crate) struct PackedDynamicPlan {
    hlit: u16,
    hdist: u8,
    hclen: u8,
    code_length_lengths: [u8; 19],
    bits: u64,
    packed: Box<[u8]>,
}

impl PackedDynamicPlan {
    pub(crate) fn pack(plan: &DynamicPlan) -> Option<Self> {
        if plan.literal_lengths.len() != plan.hlit || plan.distance_lengths.len() != plan.hdist {
            return None;
        }
        let length_count = plan.hlit + plan.hdist;
        if plan
            .literal_lengths
            .iter()
            .chain(&plan.distance_lengths)
            .any(|&length| length > 15)
        {
            return None;
        }
        let length_bytes = length_count.div_ceil(2);
        let mut packed = Vec::new();
        packed
            .try_reserve_exact(length_bytes + 2 * plan.rle.len())
            .ok()?;
        let pack_pair = |pair: &[u8]| pair[0] | (pair[1] << 4);
        let mut literal_pairs = plan.literal_lengths.chunks_exact(2);
        packed.extend(literal_pairs.by_ref().map(pack_pair));
        // An odd literal length pairs with the first distance length.
        let mut distance_lengths = &plan.distance_lengths[..];
        if let [low] = literal_pairs.remainder() {
            let (high, rest) = distance_lengths
                .split_first()
                .map_or((0, &[][..]), |(&high, rest)| (high, rest));
            packed.push(low | (high << 4));
            distance_lengths = rest;
        }
        let mut distance_pairs = distance_lengths.chunks_exact(2);
        packed.extend(distance_pairs.by_ref().map(pack_pair));
        packed.extend(distance_pairs.remainder());
        packed.resize(length_bytes + 2 * plan.rle.len(), 0);
        for (pair, token) in packed[length_bytes..].chunks_exact_mut(2).zip(&plan.rle) {
            pair[0] = token.symbol;
            pair[1] = token.extra;
        }
        Some(Self {
            hlit: u16::try_from(plan.hlit).ok()?,
            hdist: u8::try_from(plan.hdist).ok()?,
            hclen: u8::try_from(plan.hclen).ok()?,
            code_length_lengths: plan.code_length_lengths,
            bits: plan.bits,
            packed: packed.into_boxed_slice(),
        })
    }

    /// The packed allocation with allocator rounding.
    pub(crate) fn heap_charge(&self) -> usize {
        (self.packed.len() + 16).next_multiple_of(16)
    }

    fn length(&self, index: usize) -> u8 {
        (self.packed[index / 2] >> (4 * (index % 2))) & 15
    }

    fn rle_tokens(&self) -> &[u8] {
        &self.packed[(usize::from(self.hlit) + usize::from(self.hdist)).div_ceil(2)..]
    }

    fn lengths_match(&self, literal_lengths: &[u8], distance_lengths: &[u8]) -> bool {
        usize::from(self.hlit) == literal_lengths.len()
            && usize::from(self.hdist) == distance_lengths.len()
            && literal_lengths
                .iter()
                .chain(distance_lengths)
                .enumerate()
                .all(|(index, &length)| self.length(index) == length)
    }

    /// Whether `plan` is the plan packed here, field for field.
    pub(crate) fn matches(&self, plan: &DynamicPlan) -> bool {
        let rle = self.rle_tokens();
        self.bits == plan.bits
            && usize::from(self.hclen) == plan.hclen
            && self.code_length_lengths == plan.code_length_lengths
            && rle.len() == 2 * plan.rle.len()
            && rle
                .chunks_exact(2)
                .zip(&plan.rle)
                .all(|(pair, token)| pair[0] == token.symbol && pair[1] == token.extra)
            && self.lengths_match(&plan.literal_lengths, &plan.distance_lengths)
    }

    pub(crate) fn unpack(&self) -> Option<DynamicPlan> {
        let hlit = usize::from(self.hlit);
        let hdist = usize::from(self.hdist);
        let mut literal_lengths = Vec::new();
        literal_lengths.try_reserve_exact(hlit).ok()?;
        literal_lengths.extend((0..hlit).map(|index| self.length(index)));
        let mut distance_lengths = Vec::new();
        distance_lengths.try_reserve_exact(hdist).ok()?;
        distance_lengths.extend((hlit..hlit + hdist).map(|index| self.length(index)));
        let tokens = self.rle_tokens();
        let mut rle = Vec::new();
        rle.try_reserve_exact(tokens.len() / 2).ok()?;
        rle.extend(tokens.chunks_exact(2).map(|pair| RleToken {
            symbol: pair[0],
            extra: pair[1],
        }));
        Some(DynamicPlan {
            literal_lengths,
            distance_lengths,
            code_length_lengths: self.code_length_lengths,
            rle,
            hlit,
            hdist,
            hclen: usize::from(self.hclen),
            bits: self.bits,
        })
    }
}

/// A completed header plan priced with zero payload bits, and its policy.
struct StoredHeaderPlan {
    exhaustive: bool,
    rle_mask: u8,
    plan: PackedDynamicPlan,
}

impl StoredHeaderPlan {
    fn pack(exhaustive: bool, rle_mask: u8, kernel: &DynamicPlan) -> Option<Self> {
        Some(Self {
            exhaustive,
            rle_mask,
            plan: PackedDynamicPlan::pack(kernel)?,
        })
    }

    fn matches(
        &self,
        literal_lengths: &[u8],
        distance_lengths: &[u8],
        exhaustive: bool,
        rle_mask: u8,
    ) -> bool {
        self.exhaustive == exhaustive
            && self.rle_mask == rle_mask
            && self.plan.lengths_match(literal_lengths, distance_lengths)
    }
}

impl Charged for StoredHeaderPlan {
    /// A table slot with growth slack, and the packed allocation.
    fn charge(&self) -> usize {
        2 * std::mem::size_of::<(u64, Self)>() + self.plan.heap_charge()
    }
}

/// Completed header plans shared by every route on one thread. Routes replan
/// the same code lengths from different token states, and each route-local
/// [`HeaderPlanCache`] starts empty. Every hash hit compares the complete code
/// lengths and policy before reuse.
type HeaderPlanStore = PlanStore<StoredHeaderPlan>;

impl Default for HeaderPlanStore {
    fn default() -> Self {
        Self::with_budget(MAX_THREAD_HEADER_PLAN_BYTES)
    }
}

thread_local! {
    static HEADER_PLAN_STORE: RefCell<HeaderPlanStore> = RefCell::new(HeaderPlanStore::default());
}

impl HeaderPlanStore {
    fn get(
        &mut self,
        fingerprint: u64,
        literal_lengths: &[u8],
        distance_lengths: &[u8],
        exhaustive: bool,
        rle_mask: u8,
    ) -> Option<DynamicPlan> {
        self.lookup(
            fingerprint,
            |entry| entry.matches(literal_lengths, distance_lengths, exhaustive, rle_mask),
            |entry| entry.plan.unpack(),
        )
    }
}

/// Plan a header for trimmed code lengths with zero payload bits, reusing this
/// thread's completed plan when one matches exactly.
fn stored_header_plan_kernel(
    fingerprint: u64,
    literal_lengths: &[u8],
    distance_lengths: &[u8],
    exhaustive: bool,
    rle_mask: u8,
) -> Option<DynamicPlan> {
    let stored = HEADER_PLAN_STORE
        .try_with(|store| {
            store.try_borrow_mut().ok()?.get(
                fingerprint,
                literal_lengths,
                distance_lengths,
                exhaustive,
                rle_mask,
            )
        })
        .ok()
        .flatten();
    if stored.is_some() {
        return stored;
    }
    let plan = plan_for_trimmed_lengths_uncached(
        literal_lengths,
        distance_lengths,
        0,
        exhaustive,
        rle_mask,
    )?;
    if let Some(entry) = StoredHeaderPlan::pack(exhaustive, rle_mask, &plan) {
        let _ = HEADER_PLAN_STORE.try_with(|store| {
            if let Ok(mut store) = store.try_borrow_mut() {
                store.insert(fingerprint, entry);
            }
        });
    }
    Some(plan)
}

/// Drop this thread's stored header plans, code-length trees and run
/// spellings. A top-level optimization calls this on return, so library callers
/// keep no plans between inputs; worker threads free theirs when they exit.
pub(crate) fn release_thread_header_plans() {
    let _ = HEADER_PLAN_SEARCH.try_with(Cell::take);
    let _ = HEADER_PLAN_STORE.try_with(|store| {
        if let Ok(mut store) = store.try_borrow_mut() {
            *store = HeaderPlanStore::default();
        }
    });
    let _ = CODE_LENGTH_TREES.try_with(|cache| {
        if let Ok(mut cache) = cache.try_borrow_mut() {
            *cache = None;
        }
    });
}

/// A builder of code-length codes: 19 symbols, at most 7 bits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CodeLengthBuilder {
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

    /// Build this builder's tree, and say whether it is also the tree of both
    /// `DefluffLimited` and `Columbo(0)`: they start from the same variant-0
    /// Huffman tree and differ only in how they limit one deeper than 7 bits.
    fn build(
        self,
        frequencies: &[u32; 19],
        heap_scratch: &mut TreeBuildScratch,
    ) -> ([u8; 19], bool) {
        let mut lengths = [0_u8; 19];
        let mut unlimited = false;
        match self {
            Self::DefloptHeap(variant) => make_lengths_deflopt_heap_into_with_scratch(
                frequencies,
                &mut lengths,
                7,
                variant,
                heap_scratch,
            ),
            Self::OrderHeap(variant) => make_lengths_order_heap_into_with_scratch(
                frequencies,
                &mut lengths,
                7,
                variant,
                heap_scratch,
            ),
            Self::Deft4jJavaHeap => {
                make_lengths_deft4j_java_heap_into(frequencies, &mut lengths, 7, heap_scratch)
            }
            Self::DefluffLimited => {
                unlimited = !make_lengths_columbo_defluff_limited_into(
                    frequencies,
                    &mut lengths,
                    7,
                    0,
                    heap_scratch,
                );
            }
            Self::Columbo(variant) => {
                let repaired =
                    make_lengths_into(frequencies, &mut lengths, 7, variant, heap_scratch);
                unlimited = variant == 0 && !repaired;
            }
            Self::DefluffExact => make_lengths_defluff_exact_into(frequencies, &mut lengths, 7, 0),
        }
        (lengths, unlimited)
    }
}

/// One RLE histogram's code-length trees, by [`CodeLengthBuilder::index`].
#[derive(Clone, Copy)]
struct CodeLengthTreeEntry {
    frequencies: [u32; 19],
    /// Bit `index` is set once `lengths[index]` holds that builder's tree.
    built: u16,
    lengths: [[u8; 19]; CodeLengthBuilder::COUNT],
}

const EMPTY_CODE_LENGTH_TREE_ENTRY: CodeLengthTreeEntry = CodeLengthTreeEntry {
    frequencies: [0; 19],
    built: 0,
    lengths: [[0; 19]; CodeLengthBuilder::COUNT],
};

/// Entries in each thread's code-length tree cache, about 1.3 MiB. Header
/// plans for different code lengths often share an RLE histogram, and repeats
/// come close together. Storing each histogram once, with all its builders'
/// trees, holds more trees in less memory than one slot per builder and
/// histogram: in a completing Max run, this size built 10% fewer trees than
/// 16,384 such slots (1.7 MiB), and twice the entries 5% fewer again.
const CODE_LENGTH_TREE_CACHE_BITS: u32 = 12;

thread_local! {
    /// Code-length trees built on this thread, direct-mapped by RLE histogram.
    /// Each builder is a pure function of the histogram, so a hit, which
    /// compares the complete histogram, returns the tree it would build.
    static CODE_LENGTH_TREES: RefCell<Option<Box<[CodeLengthTreeEntry]>>> =
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
fn code_length_tree(
    builder: CodeLengthBuilder,
    frequencies: &[u32; 19],
    heap_scratch: &mut TreeBuildScratch,
) -> [u8; 19] {
    CodeLengthTrees::new(frequencies).get(builder, heap_scratch)
}

/// One RLE histogram's slot in this thread's code-length tree cache, found
/// once for every builder a header search asks for.
struct CodeLengthTrees<'a> {
    frequencies: &'a [u32; 19],
    slot: usize,
}

impl<'a> CodeLengthTrees<'a> {
    fn new(frequencies: &'a [u32; 19]) -> Self {
        Self {
            frequencies,
            slot: code_length_tree_slot(frequencies),
        }
    }

    /// [`code_length_tree`] for this histogram.
    fn get(&self, builder: CodeLengthBuilder, heap_scratch: &mut TreeBuildScratch) -> [u8; 19] {
        let Self { frequencies, slot } = *self;
        let Some(index) = builder.index() else {
            return builder.build(frequencies, heap_scratch).0;
        };
        let cached = CODE_LENGTH_TREES
            .try_with(|cache| {
                let cache = cache.try_borrow().ok()?;
                let entry = cache.as_ref()?.get(slot)?;
                (entry.built & (1 << index) != 0 && same_histogram(&entry.frequencies, frequencies))
                    .then_some(entry.lengths[index])
            })
            .ok()
            .flatten();
        if let Some(lengths) = cached {
            return lengths;
        }

        let (lengths, unlimited) = builder.build(frequencies, heap_scratch);
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
            let shared: &[CodeLengthBuilder] = if unlimited {
                &[
                    CodeLengthBuilder::DefluffLimited,
                    CodeLengthBuilder::Columbo(0),
                ]
            } else {
                &[builder]
            };
            for index in shared.iter().filter_map(|builder| builder.index()) {
                entry.lengths[index] = lengths;
                entry.built |= 1 << index;
            }
        });
        lengths
    }
}

fn header_plan_fingerprint(
    literal_lengths: &[u8],
    distance_lengths: &[u8],
    exhaustive: bool,
    rle_mask: u8,
) -> u64 {
    let mut fingerprint = 0xcbf2_9ce4_8422_2325_u64;
    for lengths in [literal_lengths, distance_lengths] {
        // Eight lengths a step; the count tells a short last step's zero
        // padding from lengths.
        mix_header_plan_fingerprint(&mut fingerprint, lengths.len() as u64);
        let mut chunks = lengths.chunks_exact(8);
        for chunk in &mut chunks {
            let word = u64::from_le_bytes(chunk.try_into().expect("chunks hold eight lengths"));
            mix_header_plan_fingerprint(&mut fingerprint, word);
        }
        if !chunks.remainder().is_empty() {
            let mut word = [0_u8; 8];
            word[..chunks.remainder().len()].copy_from_slice(chunks.remainder());
            mix_header_plan_fingerprint(&mut fingerprint, u64::from_le_bytes(word));
        }
    }
    mix_header_plan_fingerprint(
        &mut fingerprint,
        u64::from(exhaustive) | (u64::from(rle_mask) << 1),
    );
    fingerprint
}

#[inline]
fn mix_header_plan_fingerprint(fingerprint: &mut u64, value: u64) {
    // An odd multiplier with bits throughout carries each input bit upward,
    // and the shift folds the high half back down.
    *fingerprint ^= value;
    *fingerprint = fingerprint.wrapping_mul(0x9e37_79b9_7f4a_7c15);
    *fingerprint ^= *fingerprint >> 32;
}

/// `Complete` is the 56-way header option grid of deft4j's
/// `addOptimisedRecoded`. `DefaultRecode` only decides whether deft4j's
/// repeated individual-prune step reached a smaller fixed point.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Deft4jHeaderPolicy {
    Complete,
    DefaultRecode,
}

/// Switches for deft4j's code-length run packer. `special_repeat` is deft4j's
/// `ohh` flag, which enables the 4+4 (`use_eight`) and 4+3 (`use_seven`)
/// code-16 splits.
#[derive(Debug, Clone, Copy)]
struct Deft4jPackOptions {
    special_repeat: bool,
    use_eight: bool,
    use_seven: bool,
    no_repeat: bool,
    no_zero_repeat: bool,
    no_long_zero_repeat: bool,
    no_repeat_zeros: bool,
}

#[derive(Debug, Clone, Copy)]
struct Deft4jHeaderOptions {
    pack: Deft4jPackOptions,
    prune: bool,
    optimize_header: bool,
}

const DEFT4J_DEFAULT_RECODE_OPTIONS: Deft4jHeaderOptions = Deft4jHeaderOptions {
    pack: Deft4jPackOptions {
        special_repeat: true,
        use_eight: true,
        use_seven: true,
        no_repeat: false,
        no_zero_repeat: false,
        no_long_zero_repeat: false,
        no_repeat_zeros: false,
    },
    prune: false,
    optimize_header: false,
};

pub(crate) fn token_bits(
    tokens: &[Token],
    literal_lengths: &[u8],
    distance_lengths: &[u8],
) -> Option<u64> {
    let mut bits = 0_u64;
    for token in tokens {
        match *token {
            Token::Literal(value) => {
                let literal = *literal_lengths.get(usize::from(value))?;
                if literal == 0 {
                    return None;
                }
                bits = bits.checked_add(u64::from(literal))?;
            }
            Token::Match {
                length_symbol,
                distance_symbol,
                length_extra_bits,
                distance_extra_bits,
                ..
            } => {
                let literal = *literal_lengths.get(usize::from(length_symbol))?;
                let distance = *distance_lengths.get(usize::from(distance_symbol))?;
                if literal == 0 || distance == 0 {
                    return None;
                }
                let token_bits = u64::from(literal)
                    + u64::from(distance)
                    + u64::from(length_extra_bits)
                    + u64::from(distance_extra_bits);
                bits = bits.checked_add(token_bits)?;
            }
        }
    }
    let end = *literal_lengths.get(256)?;
    if end == 0 {
        return None;
    }
    bits.checked_add(u64::from(end))
}

// Out of line: each call scans both alphabets.
#[inline(never)]
pub(crate) fn token_bits_from_frequencies(
    literal_frequencies: &[u32; 286],
    distance_frequencies: &[u32; 30],
    literal_lengths: &[u8],
    distance_lengths: &[u8],
    extra_bits: u64,
) -> Option<u64> {
    let literal_bits = alphabet_payload_bits(literal_frequencies, literal_lengths)?;
    let distance_bits = alphabet_payload_bits(distance_frequencies, distance_lengths)?;
    extra_bits
        .checked_add(literal_bits)?
        .checked_add(distance_bits)
}

fn alphabet_payload_bits<const N: usize>(frequencies: &[u32; N], lengths: &[u8]) -> Option<u64> {
    let mut bits = 0_u64;
    for (symbol, &frequency) in frequencies.iter().enumerate() {
        if frequency == 0 {
            continue;
        }
        let length = *lengths.get(symbol)?;
        if length == 0 {
            return None;
        }
        bits = bits.checked_add(u64::from(frequency) * u64::from(length))?;
    }
    Some(bits)
}

pub(crate) fn score_existing_dynamic(
    tokens: &[Token],
    source: &DynamicPlan,
    strict: bool,
) -> Option<DynamicPlan> {
    if strict && !source.has_strictly_compatible_huffman_codes() {
        return None;
    }
    if source.hlit < 257
        || source.hlit > 286
        || source.hdist == 0
        || source.hdist > RFC_DISTANCE_CODE_COUNT
        || source.hclen < 4
        || source.hclen > 19
        || source.literal_lengths.len() != source.hlit
        || source.distance_lengths.len() != source.hdist
        || !huffman_code_lengths_are_valid(&source.literal_lengths)
        || !huffman_code_lengths_are_valid(&source.distance_lengths)
        || !huffman_code_lengths_are_valid(&source.code_length_lengths)
        || !code_length_tree_shape_is_valid(&source.code_length_lengths)
        || !payload_tree_shape_is_valid(&source.literal_lengths, false)
        || !payload_tree_shape_is_valid(&source.distance_lengths, true)
    {
        return None;
    }
    let mut plan = source.clone();
    let data_bits = token_bits(tokens, &plan.literal_lengths, &plan.distance_lengths)?;
    plan.bits = dynamic_bits(data_bits, &plan)?;
    Some(plan)
}

/// `extra_bits` is the [`token_extra_bits`] total of `tokens`, which the
/// caller has usually summed already.
#[allow(clippy::too_many_arguments)]
pub(crate) fn best_dynamic_plan_cached(
    tokens: &[Token],
    literal_frequencies: &[u32; 286],
    distance_frequencies: &[u32; 30],
    extra_bits: u64,
    original: Option<&DynamicPlan>,
    strict: bool,
    exhaustive: bool,
    stop: &mut SearchStop<'_>,
    header_cache: &mut HeaderPlanCache,
) -> Option<DynamicPlan> {
    debug_assert_eq!(extra_bits, token_extra_bits(tokens));
    let source_exact = original.and_then(|plan| score_existing_dynamic(tokens, plan, strict));
    let source_is_compatible = source_exact.is_some();
    let mut best = source_exact;

    // Also repack the source code lengths (`score_existing_dynamic` keeps the
    // source RLE); DeflOpt tries both.
    if let Some(source) = original {
        if source_is_compatible {
            let data_bits = token_bits_from_frequencies(
                literal_frequencies,
                distance_frequencies,
                &source.literal_lengths,
                &source.distance_lengths,
                extra_bits,
            );
            if let Some(data_bits) = data_bits {
                if let Some(repacked) = plan_for_explicit_lengths_with_cost_cached(
                    &source.literal_lengths,
                    &source.distance_lengths,
                    data_bits,
                    exhaustive,
                    header_cache,
                ) {
                    keep_better(&mut best, repacked);
                }
            }
        }
    }

    let mut build_literal_frequencies = *literal_frequencies;
    ensure_code_symbols(&mut build_literal_frequencies, strict);
    let mut literal_candidates = tree_candidates(&build_literal_frequencies, 15, exhaustive);
    let mut build_distance_frequencies = *distance_frequencies;
    ensure_distance_symbols(&mut build_distance_frequencies, strict);
    let mut distance_candidates = tree_candidates(&build_distance_frequencies, 15, exhaustive);

    literal_candidates.retain(|lengths| {
        lengths.get(256).copied().unwrap_or(0) != 0 && payload_tree_shape_is_valid(lengths, false)
    });
    distance_candidates.retain(|lengths| {
        payload_tree_shape_is_valid(lengths, true)
            && distance_frequencies
                .iter()
                .enumerate()
                .all(|(symbol, &frequency)| frequency == 0 || lengths[symbol] != 0)
    });
    // Keep insertion order, not payload order: a slightly dearer tree can
    // have a much smaller header.
    literal_candidates.truncate(20);
    distance_candidates.truncate(20);

    'outer: for literal in &literal_candidates {
        for distance in &distance_candidates {
            if stop.reached() {
                break 'outer;
            }
            let data_bits = token_bits_from_frequencies(
                literal_frequencies,
                distance_frequencies,
                literal,
                distance,
                extra_bits,
            );
            if let Some(data_bits) = data_bits {
                if let Some(candidate) = plan_for_explicit_lengths_with_cost_cached(
                    literal,
                    distance,
                    data_bits,
                    exhaustive,
                    header_cache,
                ) {
                    keep_better(&mut best, candidate);
                }
            }
        }
    }

    // A depth-limited tree can have a much cheaper header than a
    // payload-optimal tree with very long rare codes (as in ECT).
    if exhaustive {
        for max_bits in REDUCED_PAYLOAD_TREE_DEPTHS {
            if stop.reached() {
                break;
            }
            if let Some(candidate) = exact_tree_candidate(
                literal_frequencies,
                distance_frequencies,
                &build_literal_frequencies,
                &build_distance_frequencies,
                extra_bits,
                max_bits,
                true,
                header_cache,
            ) {
                keep_better(&mut best, candidate);
            }
        }
    }

    if exhaustive && !stop.reached() {
        if let Some(candidate) = columbo_rle_tree_candidate(
            literal_frequencies,
            distance_frequencies,
            extra_bits,
            strict,
            exhaustive,
        ) {
            keep_better(&mut best, candidate);
        }
    }

    if exhaustive && !stop.reached() {
        if let Some(candidate) = zopfli_rle_tree_candidate(
            literal_frequencies,
            distance_frequencies,
            extra_bits,
            strict,
            exhaustive,
        ) {
            keep_better(&mut best, candidate);
        }
    }

    // Equal-frequency symbols can exchange code lengths without changing the
    // payload cost or code validity, but the exchange changes the header.
    if exhaustive && !stop.reached() {
        if let Some(seed) = best.clone() {
            let seed_literal = pad_lengths::<286>(&seed.literal_lengths);
            let seed_distance = pad_lengths::<30>(&seed.distance_lengths);
            for descending in [false, true] {
                // First rearrange the literal/length code lengths alone, with
                // the distance lengths fixed: rearranging both alphabets can
                // hide a literal-side header win.
                let mut literal = seed_literal;
                arrange_equal_frequency_lengths(literal_frequencies, &mut literal, descending);
                if literal != seed_literal && !stop.reached() {
                    if let Some(candidate) =
                        plan_for_explicit_lengths(tokens, &literal, &seed_distance, exhaustive)
                    {
                        keep_better(&mut best, candidate);
                    }
                }

                let mut distance = seed_distance;
                arrange_equal_frequency_lengths(distance_frequencies, &mut distance, descending);
                if (literal != seed_literal || distance != seed_distance) && !stop.reached() {
                    if let Some(candidate) =
                        plan_for_explicit_lengths(tokens, &literal, &distance, exhaustive)
                    {
                        keep_better(&mut best, candidate);
                    }
                }
            }
        }
    }

    // Greedy length swaps help most on small, literal-heavy blocks.
    if exhaustive && tokens.len() <= 700 && !stop.reached() {
        if let Some(seed) = best.clone() {
            improve_by_length_swaps(
                tokens,
                literal_frequencies,
                distance_frequencies,
                &seed,
                exhaustive,
                stop,
                &mut best,
            );
        }
    }
    best
}

fn pad_lengths<const N: usize>(lengths: &[u8]) -> [u8; N] {
    let mut padded = [0_u8; N];
    let count = lengths.len().min(N);
    padded[..count].copy_from_slice(&lengths[..count]);
    padded
}

const BALANCED_TREE_CANDIDATE_CAP: usize = 5;
const BALANCED_TREE_PAIRED_CAP: usize = 4;
const BALANCED_TREE_PAYLOAD_MARGIN_BITS: i64 = 18;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct BalancedTreeOpportunities {
    pub(crate) dynamic_blocks: usize,
    pub(crate) literal_pair_moves: usize,
    pub(crate) literal_quad_moves: usize,
    pub(crate) distance_pair_moves: usize,
    pub(crate) distance_quad_moves: usize,
    pub(crate) paired_prices: usize,
}

impl BalancedTreeOpportunities {
    pub(crate) fn add_assign(&mut self, other: Self) {
        self.dynamic_blocks = self.dynamic_blocks.saturating_add(other.dynamic_blocks);
        self.literal_pair_moves = self
            .literal_pair_moves
            .saturating_add(other.literal_pair_moves);
        self.literal_quad_moves = self
            .literal_quad_moves
            .saturating_add(other.literal_quad_moves);
        self.distance_pair_moves = self
            .distance_pair_moves
            .saturating_add(other.distance_pair_moves);
        self.distance_quad_moves = self
            .distance_quad_moves
            .saturating_add(other.distance_quad_moves);
        self.paired_prices = self.paired_prices.saturating_add(other.paired_prices);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum BalancedTreeFamily {
    Pair,
    Quad,
}

#[derive(Clone)]
struct BalancedTreeMove<const N: usize> {
    lengths: [u8; N],
    payload_delta: i64,
    family: BalancedTreeFamily,
}

// Out of line: each call scans and shifts the candidate list.
#[inline(never)]
fn insert_frequency_candidate(
    candidates: &mut Vec<(usize, u32)>,
    entry: (usize, u32),
    descending: bool,
) {
    let insertion = candidates
        .iter()
        .position(|&(_, frequency)| {
            if descending {
                frequency < entry.1
            } else {
                frequency > entry.1
            }
        })
        .unwrap_or(candidates.len());
    if insertion < BALANCED_TREE_CANDIDATE_CAP {
        if candidates.len() == BALANCED_TREE_CANDIDATE_CAP {
            candidates.pop();
        }
        candidates.insert(insertion, entry);
    }
}

fn collect_pair_moves<const N: usize>(
    seed_lengths: &[u8; N],
    frequencies: &[u32; N],
) -> Option<Vec<BalancedTreeMove<N>>> {
    // At most 13 length bands × 5 shortenings × 10 lengthening pairs = 650
    // moves, within the 700 reserved.
    let mut moves = Vec::new();
    if moves.try_reserve_exact(14 * 5 * 10).is_err() {
        return None;
    }
    let mut shorten = Vec::<(usize, u32)>::new();
    let mut lengthen = Vec::<(usize, u32)>::new();
    if shorten
        .try_reserve_exact(BALANCED_TREE_CANDIDATE_CAP)
        .is_err()
        || lengthen
            .try_reserve_exact(BALANCED_TREE_CANDIDATE_CAP)
            .is_err()
    {
        return None;
    }

    for length in 2_u8..=14 {
        shorten.clear();
        lengthen.clear();
        for (symbol, &candidate) in seed_lengths.iter().enumerate() {
            if candidate != length {
                continue;
            }
            let entry = (symbol, frequencies[symbol]);
            insert_frequency_candidate(&mut shorten, entry, true);
            insert_frequency_candidate(&mut lengthen, entry, false);
        }
        if shorten.is_empty() || lengthen.len() < 2 {
            continue;
        }

        for &(short_symbol, short_frequency) in &shorten {
            for first in 0..lengthen.len() - 1 {
                for second in first + 1..lengthen.len() {
                    let long_symbols = [lengthen[first], lengthen[second]];
                    if long_symbols
                        .iter()
                        .any(|&(symbol, _)| symbol == short_symbol)
                    {
                        continue;
                    }
                    let payload_delta = long_symbols
                        .iter()
                        .fold(-i64::from(short_frequency), |delta, (_, frequency)| {
                            delta + i64::from(*frequency)
                        });
                    if payload_delta > BALANCED_TREE_PAYLOAD_MARGIN_BITS {
                        continue;
                    }

                    let mut lengths = *seed_lengths;
                    lengths[short_symbol] -= 1;
                    for &(symbol, _) in &long_symbols {
                        lengths[symbol] += 1;
                    }
                    moves.push(BalancedTreeMove {
                        lengths,
                        payload_delta,
                        family: BalancedTreeFamily::Pair,
                    });
                }
            }
        }
    }
    Some(moves)
}

fn collect_quad_moves<const N: usize>(
    seed_lengths: &[u8; N],
    frequencies: &[u32; N],
) -> Option<Vec<BalancedTreeMove<N>>> {
    // At most 12 length bands × 5 shortenings × 5 lengthening quads = 300
    // moves.
    let mut moves = Vec::new();
    if moves.try_reserve_exact(12 * 5 * 5).is_err() {
        return None;
    }
    let mut shorten = Vec::<(usize, u32)>::new();
    let mut lengthen = Vec::<(usize, u32)>::new();
    if shorten
        .try_reserve_exact(BALANCED_TREE_CANDIDATE_CAP)
        .is_err()
        || lengthen
            .try_reserve_exact(BALANCED_TREE_CANDIDATE_CAP)
            .is_err()
    {
        return None;
    }

    for length in 2_u8..=13 {
        shorten.clear();
        lengthen.clear();
        for (symbol, &candidate) in seed_lengths.iter().enumerate() {
            if candidate == length {
                insert_frequency_candidate(&mut shorten, (symbol, frequencies[symbol]), true);
            } else if candidate == length + 1 {
                insert_frequency_candidate(&mut lengthen, (symbol, frequencies[symbol]), false);
            }
        }
        if shorten.is_empty() || lengthen.len() < 4 {
            continue;
        }

        for &(short_symbol, short_frequency) in &shorten {
            for first in 0..lengthen.len() - 3 {
                for second in first + 1..lengthen.len() - 2 {
                    for third in second + 1..lengthen.len() - 1 {
                        for fourth in third + 1..lengthen.len() {
                            let long_symbols = [
                                lengthen[first],
                                lengthen[second],
                                lengthen[third],
                                lengthen[fourth],
                            ];
                            let payload_delta = long_symbols
                                .iter()
                                .fold(-i64::from(short_frequency), |delta, (_, frequency)| {
                                    delta + i64::from(*frequency)
                                });
                            if payload_delta > BALANCED_TREE_PAYLOAD_MARGIN_BITS {
                                continue;
                            }

                            let mut lengths = *seed_lengths;
                            lengths[short_symbol] -= 1;
                            for &(symbol, _) in &long_symbols {
                                lengths[symbol] += 1;
                            }
                            moves.push(BalancedTreeMove {
                                lengths,
                                payload_delta,
                                family: BalancedTreeFamily::Quad,
                            });
                        }
                    }
                }
            }
        }
    }
    Some(moves)
}

fn retain_best_balanced_moves<const N: usize>(
    pair: &[BalancedTreeMove<N>],
    quad: &[BalancedTreeMove<N>],
) -> Option<Vec<BalancedTreeMove<N>>> {
    let mut best = Vec::new();
    if best.try_reserve_exact(BALANCED_TREE_PAIRED_CAP).is_err() {
        return None;
    }
    for candidate in pair.iter().chain(quad) {
        let insertion = best
            .iter()
            .position(|current: &BalancedTreeMove<N>| {
                (current.payload_delta, current.family, &current.lengths)
                    > (
                        candidate.payload_delta,
                        candidate.family,
                        &candidate.lengths,
                    )
            })
            .unwrap_or(best.len());
        if insertion < BALANCED_TREE_PAIRED_CAP {
            if best.len() == BALANCED_TREE_PAIRED_CAP {
                best.pop();
            }
            best.insert(insertion, candidate.clone());
        }
    }
    Some(best)
}

pub(crate) fn balanced_tree_opportunities(
    literal_frequencies: &[u32; 286],
    distance_frequencies: &[u32; 30],
    seed: &DynamicPlan,
) -> Option<BalancedTreeOpportunities> {
    if seed.literal_lengths.len() > 286 || seed.distance_lengths.len() > 30 {
        return None;
    }
    let seed_literal = pad_lengths::<286>(&seed.literal_lengths);
    let seed_distance = pad_lengths::<30>(&seed.distance_lengths);
    let literal_pair = collect_pair_moves(&seed_literal, literal_frequencies)?.len();
    let literal_quad = collect_quad_moves(&seed_literal, literal_frequencies)?.len();
    let distance_pair = collect_pair_moves(&seed_distance, distance_frequencies)?.len();
    let distance_quad = collect_quad_moves(&seed_distance, distance_frequencies)?.len();
    let paired_prices = literal_pair
        .saturating_add(literal_quad)
        .min(BALANCED_TREE_PAIRED_CAP)
        .saturating_mul(
            distance_pair
                .saturating_add(distance_quad)
                .min(BALANCED_TREE_PAIRED_CAP),
        );
    Some(BalancedTreeOpportunities {
        dynamic_blocks: 1,
        literal_pair_moves: literal_pair,
        literal_quad_moves: literal_quad,
        distance_pair_moves: distance_pair,
        distance_quad_moves: distance_quad,
        paired_prices,
    })
}

/// Try bounded Kraft-preserving moves on both data alphabets (D3, M4).
///
/// A pair move shortens one code by a bit and lengthens two codes of the same
/// length; a quad move shortens one code and lengthens four codes one bit
/// longer than it. A cross-product of the best moves per alphabet can also
/// exploit the RLE run that crosses the alphabet boundary. The seed plan is
/// not returned; callers keep it.
pub(crate) fn plan_columbo_balanced_tree_candidate(
    tokens: &[Token],
    literal_frequencies: &[u32; 286],
    distance_frequencies: &[u32; 30],
    seed: &DynamicPlan,
    exhaustive_header: bool,
    price_literal_pair: bool,
) -> Option<DynamicPlan> {
    if seed.literal_lengths.len() > 286 || seed.distance_lengths.len() > 30 {
        return None;
    }
    let seed_literal = pad_lengths::<286>(&seed.literal_lengths);
    let seed_distance = pad_lengths::<30>(&seed.distance_lengths);
    let extra_bits = token_extra_bits(tokens);
    let mut best: Option<DynamicPlan> = None;
    let literal_pair = collect_pair_moves(&seed_literal, literal_frequencies)?;
    let literal_quad = collect_quad_moves(&seed_literal, literal_frequencies)?;
    let distance_pair = collect_pair_moves(&seed_distance, distance_frequencies)?;
    let distance_quad = collect_quad_moves(&seed_distance, distance_frequencies)?;

    for candidate in literal_pair
        .iter()
        .filter(|_| price_literal_pair)
        .chain(&literal_quad)
    {
        let Some(data_bits) = token_bits_from_frequencies(
            literal_frequencies,
            distance_frequencies,
            &candidate.lengths,
            &seed_distance,
            extra_bits,
        ) else {
            continue;
        };
        if let Some(candidate) = plan_for_explicit_lengths_with_cost(
            &candidate.lengths,
            &seed_distance,
            data_bits,
            exhaustive_header,
        ) {
            keep_better(&mut best, candidate);
        }
    }

    for candidate in distance_pair.iter().chain(&distance_quad) {
        let Some(data_bits) = token_bits_from_frequencies(
            literal_frequencies,
            distance_frequencies,
            &seed_literal,
            &candidate.lengths,
            extra_bits,
        ) else {
            continue;
        };
        if let Some(candidate) = plan_for_explicit_lengths_with_cost(
            &seed_literal,
            &candidate.lengths,
            data_bits,
            exhaustive_header,
        ) {
            keep_better(&mut best, candidate);
        }
    }

    let literal_best = retain_best_balanced_moves(&literal_pair, &literal_quad)?;
    let distance_best = retain_best_balanced_moves(&distance_pair, &distance_quad)?;
    for literal in &literal_best {
        for distance in &distance_best {
            let combined_delta = literal.payload_delta.saturating_add(distance.payload_delta);
            if combined_delta > BALANCED_TREE_PAYLOAD_MARGIN_BITS
                || (!exhaustive_header && combined_delta > 0)
            {
                continue;
            }
            let Some(data_bits) = token_bits_from_frequencies(
                literal_frequencies,
                distance_frequencies,
                &literal.lengths,
                &distance.lengths,
                extra_bits,
            ) else {
                continue;
            };
            if let Some(candidate) = plan_for_explicit_lengths_with_cost(
                &literal.lengths,
                &distance.lengths,
                data_bits,
                exhaustive_header,
            ) {
                keep_better(&mut best, candidate);
            }
        }
    }

    best
}

/// Sort one tree's code lengths within each group of equal-frequency symbols,
/// which preserves the Kraft sum and payload cost. Ascending and descending
/// order give two cheap header candidates instead of a permutation search.
fn arrange_equal_frequency_lengths<const N: usize>(
    frequencies: &[u32; N],
    lengths: &mut [u8; N],
    descending: bool,
) {
    let mut visited = [false; N];
    for first in 0..N {
        if visited[first] || frequencies[first] == 0 || lengths[first] == 0 {
            continue;
        }

        let frequency = frequencies[first];
        let mut positions = Vec::new();
        let mut assigned_lengths = Vec::new();
        for symbol in first..N {
            if !visited[symbol] && frequencies[symbol] == frequency && lengths[symbol] != 0 {
                visited[symbol] = true;
                positions.push(symbol);
                assigned_lengths.push(lengths[symbol]);
            }
        }
        if positions.len() < 2 {
            continue;
        }

        assigned_lengths.sort_shared_by(Ord::cmp);
        if descending {
            assigned_lengths.reverse();
        }
        for (symbol, length) in positions.into_iter().zip(assigned_lengths) {
            lengths[symbol] = length;
        }
    }
}

// Out of line: each call scans an alphabet.
#[inline(never)]
fn ensure_code_symbols(frequencies: &mut [u32], strict: bool) {
    if !strict {
        return;
    }
    let mut used = frequencies
        .iter()
        .enumerate()
        .filter_map(|(symbol, &frequency)| (frequency != 0).then_some(symbol));
    match (used.next(), used.next()) {
        (None, _) => {
            frequencies[0] = 1;
            frequencies[1] = 1;
        }
        (Some(only), None) => frequencies[usize::from(only == 0)] = 1,
        (Some(_), Some(_)) => {}
    }
}

/// Uniform complete distance codes for a strict block that uses at most one
/// distance symbol.
///
/// Any complete code that covers the used symbol decodes the same payload. A
/// uniform code of depth 1 to 4 is one run of one code-length value; the
/// cheapest depth depends on the rest of the header and on that value's
/// position in the RFC 1951 code-length order.
fn strict_distance_completions(frequencies: &[u32; 30]) -> Vec<Vec<u8>> {
    let mut used = frequencies
        .iter()
        .enumerate()
        .filter_map(|(symbol, &frequency)| (frequency != 0).then_some(symbol));
    let only = match (used.next(), used.next()) {
        (None, _) => None,
        (Some(only), None) => Some(only),
        (Some(_), Some(_)) => return Vec::new(),
    };
    (1..=4_u8)
        .map(|depth| {
            let width = 1_usize << depth;
            let start = only.map_or(0, |symbol| (symbol + 1).saturating_sub(width));
            let mut lengths = vec![0_u8; frequencies.len()];
            lengths[start..start + width].fill(depth);
            lengths
        })
        .collect()
}

fn ensure_distance_symbols(frequencies: &mut [u32; 30], strict: bool) {
    ensure_code_symbols(frequencies, strict);
}

/// The heap variant whose sift-downs both break ties by frequency alone, so
/// its DeflOpt and order-key builders return the same tree.
const FREQUENCY_ONLY_HEAP_VARIANT: u32 = 3;

fn tree_candidates(frequencies: &[u32], max_bits: u8, exhaustive: bool) -> Vec<Vec<u8>> {
    let mut candidates = Vec::with_capacity(if exhaustive { 16 } else { 8 });
    let mut heap_scratch = TreeBuildScratch::default();
    // Family order matters: of two equal complete plans, the earlier wins.
    // Without `exhaustive`, only the DeflOpt and order-key heaps run (at most
    // 64 pairs).
    for variant in 0..4 {
        let mut lengths = vec![0_u8; frequencies.len()];
        make_lengths_deflopt_heap_into_with_scratch(
            frequencies,
            &mut lengths,
            max_bits,
            variant,
            &mut heap_scratch,
        );
        push_unique(&mut candidates, lengths);
    }
    // Variant 3 breaks both ties by frequency alone, so its order-key tree is
    // the DeflOpt tree already listed, which `push_unique` would discard.
    for variant in 0..FREQUENCY_ONLY_HEAP_VARIANT {
        let mut lengths = vec![0_u8; frequencies.len()];
        make_lengths_order_heap_into_with_scratch(
            frequencies,
            &mut lengths,
            max_bits,
            variant,
            &mut heap_scratch,
        );
        push_unique(&mut candidates, lengths);
    }

    if exhaustive {
        for variant in 0..4 {
            push_unique(
                &mut candidates,
                make_lengths(frequencies, max_bits, variant),
            );
        }
        push_unique(
            &mut candidates,
            make_lengths_columbo_defluff_limited(frequencies, max_bits, 0),
        );
        let defluff_exact = make_lengths_defluff_exact(frequencies, max_bits, 0);
        push_unique(&mut candidates, defluff_exact.clone());
        push_unique(
            &mut candidates,
            make_lengths_deft4j_java_heap(frequencies, max_bits),
        );
        push_unique(
            &mut candidates,
            make_lengths_zopfli_package_from(frequencies, &defluff_exact, max_bits),
        );
    }
    candidates.retain(|lengths| {
        lengths.len() == frequencies.len()
            && frequencies
                .iter()
                .zip(lengths)
                .all(|(&frequency, &length)| frequency == 0 || length != 0)
            && huffman_code_lengths_are_valid(lengths)
    });
    candidates
}

#[allow(clippy::too_many_arguments)]
fn exact_tree_candidate(
    literal_frequencies: &[u32; 286],
    distance_frequencies: &[u32; 30],
    build_literal_frequencies: &[u32; 286],
    build_distance_frequencies: &[u32; 30],
    extra_bits: u64,
    max_bits: u8,
    exhaustive: bool,
    header_cache: &mut HeaderPlanCache,
) -> Option<DynamicPlan> {
    let literal_lengths = make_lengths_defluff_exact(build_literal_frequencies, max_bits, 0);
    let distance_lengths = make_lengths_defluff_exact(build_distance_frequencies, max_bits, 0);

    explicit_exact_tree_candidate(
        literal_frequencies,
        distance_frequencies,
        &literal_lengths,
        &distance_lengths,
        extra_bits,
        exhaustive,
        header_cache,
    )
}

#[allow(clippy::too_many_arguments)]
fn explicit_exact_tree_candidate(
    literal_frequencies: &[u32; 286],
    distance_frequencies: &[u32; 30],
    literal_lengths: &[u8],
    distance_lengths: &[u8],
    extra_bits: u64,
    exhaustive: bool,
    header_cache: &mut HeaderPlanCache,
) -> Option<DynamicPlan> {
    let data_bits = token_bits_from_frequencies(
        literal_frequencies,
        distance_frequencies,
        literal_lengths,
        distance_lengths,
        extra_bits,
    )?;
    price_explicit_exact_tree_candidate(
        distance_frequencies,
        literal_lengths,
        distance_lengths,
        data_bits,
        exhaustive,
        header_cache,
    )
}

fn price_explicit_exact_tree_candidate(
    distance_frequencies: &[u32; 30],
    literal_lengths: &[u8],
    distance_lengths: &[u8],
    data_bits: u64,
    exhaustive: bool,
    header_cache: &mut HeaderPlanCache,
) -> Option<DynamicPlan> {
    if literal_lengths.get(256).copied().unwrap_or(0) == 0
        || !payload_tree_shape_is_valid(literal_lengths, false)
        || !payload_tree_shape_is_valid(distance_lengths, true)
        || distance_frequencies
            .iter()
            .enumerate()
            .any(|(symbol, &frequency)| frequency != 0 && distance_lengths[symbol] == 0)
    {
        return None;
    }
    plan_for_explicit_lengths_with_cost_cached(
        literal_lengths,
        distance_lengths,
        data_bits,
        exhaustive,
        header_cache,
    )
}

#[allow(clippy::too_many_arguments)]
fn price_exact_tree_cross_product(
    literal_frequencies: &[u32; 286],
    distance_frequencies: &[u32; 30],
    literal_candidates: &[Vec<u8>],
    distance_candidates: &[Vec<u8>],
    extra_bits: u64,
    exhaustive_header: bool,
    header_cache: &mut HeaderPlanCache,
    best: &mut Option<DynamicPlan>,
    stop: &mut SearchStop<'_>,
) -> bool {
    let literal_payload_bits: Vec<Option<u64>> = literal_candidates
        .iter()
        .map(|lengths| alphabet_payload_bits(literal_frequencies, lengths))
        .collect();
    let distance_payload_bits: Vec<Option<u64>> = distance_candidates
        .iter()
        .map(|lengths| alphabet_payload_bits(distance_frequencies, lengths))
        .collect();
    let pair_capacity = literal_candidates
        .len()
        .checked_mul(distance_candidates.len())
        .unwrap_or(0);
    let mut ordered_pairs = Vec::new();
    if ordered_pairs.try_reserve_exact(pair_capacity).is_ok() {
        for (literal_index, &literal_bits) in literal_payload_bits.iter().enumerate() {
            let Some(literal_bits) = literal_bits else {
                continue;
            };
            for (distance_index, &distance_bits) in distance_payload_bits.iter().enumerate() {
                let Some(data_bits) = distance_bits.and_then(|distance_bits| {
                    extra_bits
                        .checked_add(literal_bits)?
                        .checked_add(distance_bits)
                }) else {
                    continue;
                };
                ordered_pairs.push((data_bits, literal_index, distance_index));
            }
        }
        // Payload bits are a lower bound on the block cost, so the smallest
        // first does the most useful work before a deadline and prunes more.
        ordered_pairs.sort_shared_by(Ord::cmp);
        for (data_bits, literal_index, distance_index) in ordered_pairs {
            if !price_exact_tree_pair(
                distance_frequencies,
                &literal_candidates[literal_index],
                &distance_candidates[distance_index],
                data_bits,
                exhaustive_header,
                header_cache,
                best,
                stop,
            ) {
                return false;
            }
        }
        return true;
    }

    // If the ordering buffer cannot be allocated, price the same pairs in
    // index order.
    for (literal_index, &literal_bits) in literal_payload_bits.iter().enumerate() {
        let Some(literal_bits) = literal_bits else {
            continue;
        };
        for (distance_index, &distance_bits) in distance_payload_bits.iter().enumerate() {
            let Some(data_bits) = distance_bits.and_then(|distance_bits| {
                extra_bits
                    .checked_add(literal_bits)?
                    .checked_add(distance_bits)
            }) else {
                continue;
            };
            if !price_exact_tree_pair(
                distance_frequencies,
                &literal_candidates[literal_index],
                &distance_candidates[distance_index],
                data_bits,
                exhaustive_header,
                header_cache,
                best,
                stop,
            ) {
                return false;
            }
        }
    }
    true
}

#[allow(clippy::too_many_arguments)]
fn price_exact_tree_pair(
    distance_frequencies: &[u32; 30],
    literal_lengths: &[u8],
    distance_lengths: &[u8],
    data_bits: u64,
    exhaustive_header: bool,
    header_cache: &mut HeaderPlanCache,
    best: &mut Option<DynamicPlan>,
    stop: &mut SearchStop<'_>,
) -> bool {
    if stop.reached() {
        return false;
    }
    // A dynamic header costs at least one bit, so a pair whose payload alone
    // does not beat the incumbent cannot win.
    if best
        .as_ref()
        .is_some_and(|current| data_bits >= current.bits)
    {
        return true;
    }
    if let Some(candidate) = price_explicit_exact_tree_candidate(
        distance_frequencies,
        literal_lengths,
        distance_lengths,
        data_bits,
        exhaustive_header,
        header_cache,
    ) {
        keep_better(best, candidate);
    }
    true
}

/// Price data trees at every feasible depth limit below 15, without changing
/// tokens.
///
/// By default both alphabets share each limit, and the leaf-first (Defluff) and
/// package-first (Zopfli) trees form a two-by-two product. With
/// `independent_depths`, it prices the full cross-product of every distinct
/// tree per alphabet; the caller must bound that work.
pub(crate) fn plan_bounded_depth_tree_candidate(
    tokens: &[Token],
    literal_frequencies: &[u32; 286],
    distance_frequencies: &[u32; 30],
    strict: bool,
    exhaustive_header: bool,
    independent_depths: bool,
    stop: &mut SearchStop<'_>,
) -> Option<DynamicPlan> {
    let mut build_literal_frequencies = *literal_frequencies;
    ensure_code_symbols(&mut build_literal_frequencies, strict);
    let mut build_distance_frequencies = *distance_frequencies;
    ensure_distance_symbols(&mut build_distance_frequencies, strict);
    let extra_bits = token_extra_bits(tokens);
    let mut header_cache = HeaderPlanCache::new();
    let mut best = None;

    if independent_depths {
        let mut literal_candidates = Vec::new();
        for max_bits in minimum_complete_tree_depth(&build_literal_frequencies)
            ..=MAX_RESTRICTED_PAYLOAD_TREE_DEPTH
        {
            let defluff = make_lengths_defluff_exact(&build_literal_frequencies, max_bits, 0);
            let package_first =
                make_lengths_zopfli_package_from(&build_literal_frequencies, &defluff, max_bits);
            push_unique(&mut literal_candidates, defluff);
            push_unique(&mut literal_candidates, package_first);
        }
        let mut distance_candidates = Vec::new();
        for max_bits in minimum_complete_tree_depth(&build_distance_frequencies)
            ..=MAX_RESTRICTED_PAYLOAD_TREE_DEPTH
        {
            let defluff = make_lengths_defluff_exact(&build_distance_frequencies, max_bits, 0);
            let package_first =
                make_lengths_zopfli_package_from(&build_distance_frequencies, &defluff, max_bits);
            push_unique(&mut distance_candidates, defluff);
            push_unique(&mut distance_candidates, package_first);
        }
        price_exact_tree_cross_product(
            literal_frequencies,
            distance_frequencies,
            &literal_candidates,
            &distance_candidates,
            extra_bits,
            exhaustive_header,
            &mut header_cache,
            &mut best,
            stop,
        );
    } else {
        let minimum_depth = minimum_complete_tree_depth(&build_literal_frequencies)
            .max(minimum_complete_tree_depth(&build_distance_frequencies));
        let mut literal_candidates = Vec::with_capacity(2);
        let mut distance_candidates = Vec::with_capacity(2);
        for max_bits in minimum_depth..=MAX_RESTRICTED_PAYLOAD_TREE_DEPTH {
            literal_candidates.clear();
            let literal_defluff =
                make_lengths_defluff_exact(&build_literal_frequencies, max_bits, 0);
            let literal_package_first = make_lengths_zopfli_package_from(
                &build_literal_frequencies,
                &literal_defluff,
                max_bits,
            );
            push_unique(&mut literal_candidates, literal_defluff);
            push_unique(&mut literal_candidates, literal_package_first);
            distance_candidates.clear();
            let distance_defluff =
                make_lengths_defluff_exact(&build_distance_frequencies, max_bits, 0);
            let distance_package_first = make_lengths_zopfli_package_from(
                &build_distance_frequencies,
                &distance_defluff,
                max_bits,
            );
            push_unique(&mut distance_candidates, distance_defluff);
            push_unique(&mut distance_candidates, distance_package_first);
            if !price_exact_tree_cross_product(
                literal_frequencies,
                distance_frequencies,
                &literal_candidates,
                &distance_candidates,
                extra_bits,
                exhaustive_header,
                &mut header_cache,
                &mut best,
                stop,
            ) {
                break;
            }
        }
    }
    best
}

/// Price data trees built from Brotli's and Zopfli's RLE-smoothed counts at
/// each depth limit from 15 down to 9. This stays out of the ordinary block
/// planner because a locally cheaper tree can lead later token feedback to a
/// worse fixed point.
pub(crate) fn plan_rle_smoothed_tree_candidate(
    tokens: &[Token],
    literal_frequencies: &[u32; 286],
    distance_frequencies: &[u32; 30],
    strict: bool,
) -> Option<DynamicPlan> {
    let mut families = [
        (*literal_frequencies, *distance_frequencies),
        (*literal_frequencies, *distance_frequencies),
    ];
    make_brotli_rle_pseudofrequencies(&mut families[0].0);
    make_brotli_rle_pseudofrequencies(&mut families[0].1);
    make_zopfli_rle_pseudofrequencies(&mut families[1].0);
    make_zopfli_rle_pseudofrequencies(&mut families[1].1);
    let extra_bits = token_extra_bits(tokens);
    let mut cache = HeaderPlanCache::new();
    let mut best = None;
    for (build_literal_frequencies, build_distance_frequencies) in &mut families {
        if *build_literal_frequencies == *literal_frequencies
            && *build_distance_frequencies == *distance_frequencies
        {
            continue;
        }
        ensure_code_symbols(build_literal_frequencies, strict);
        ensure_distance_symbols(build_distance_frequencies, strict);
        for max_bits in [15, 14, 13, 12, 11, 10, 9] {
            if let Some(candidate) = exact_tree_candidate(
                literal_frequencies,
                distance_frequencies,
                build_literal_frequencies,
                build_distance_frequencies,
                extra_bits,
                max_bits,
                true,
                &mut cache,
            ) {
                keep_better(&mut best, candidate);
            }
        }
    }
    best
}

/// Build a tree from Columbo's RLE-friendly counts, priced with the true
/// frequencies.
fn columbo_rle_tree_candidate(
    literal_frequencies: &[u32; 286],
    distance_frequencies: &[u32; 30],
    extra_bits: u64,
    strict: bool,
    exhaustive: bool,
) -> Option<DynamicPlan> {
    let mut pseudo_literal_frequencies = *literal_frequencies;
    let mut pseudo_distance_frequencies = *distance_frequencies;
    make_columbo_rle_pseudofrequencies(&mut pseudo_literal_frequencies);
    make_columbo_rle_pseudofrequencies(&mut pseudo_distance_frequencies);

    let quantization_changed_tree_weights = pseudo_literal_frequencies != *literal_frequencies
        || pseudo_distance_frequencies != *distance_frequencies;
    if !quantization_changed_tree_weights {
        return None;
    }

    ensure_code_symbols(&mut pseudo_literal_frequencies, strict);
    ensure_distance_symbols(&mut pseudo_distance_frequencies, strict);
    let literal = make_lengths(&pseudo_literal_frequencies, 15, 0);
    let distance = make_lengths(&pseudo_distance_frequencies, 15, 0);
    if literal_frequencies
        .iter()
        .zip(&literal)
        .any(|(&frequency, &length)| frequency != 0 && length == 0)
        || distance_frequencies
            .iter()
            .zip(&distance)
            .any(|(&frequency, &length)| frequency != 0 && length == 0)
    {
        return None;
    }
    huffman_code_lengths_are_valid(&literal).then_some(())?;
    huffman_code_lengths_are_valid(&distance).then_some(())?;
    let data_bits = token_bits_from_frequencies(
        literal_frequencies,
        distance_frequencies,
        &literal,
        &distance,
        extra_bits,
    )?;
    plan_for_explicit_lengths_with_cost(&literal, &distance, data_bits, exhaustive)
}

/// Build a tree from Zopfli's RLE-friendly counts, priced with the true
/// frequencies.
fn zopfli_rle_tree_candidate(
    literal_frequencies: &[u32; 286],
    distance_frequencies: &[u32; 30],
    extra_bits: u64,
    strict: bool,
    exhaustive: bool,
) -> Option<DynamicPlan> {
    let mut pseudo_literal_frequencies = *literal_frequencies;
    let mut pseudo_distance_frequencies = *distance_frequencies;
    make_zopfli_rle_pseudofrequencies(&mut pseudo_literal_frequencies);
    make_zopfli_rle_pseudofrequencies(&mut pseudo_distance_frequencies);

    let smoothing_changed_tree_weights = pseudo_literal_frequencies != *literal_frequencies
        || pseudo_distance_frequencies != *distance_frequencies;
    if !smoothing_changed_tree_weights {
        return None;
    }

    ensure_code_symbols(&mut pseudo_literal_frequencies, strict);
    ensure_distance_symbols(&mut pseudo_distance_frequencies, strict);
    let literal = make_lengths(&pseudo_literal_frequencies, 15, 0);
    let distance = make_lengths(&pseudo_distance_frequencies, 15, 0);
    if literal_frequencies
        .iter()
        .zip(&literal)
        .any(|(&frequency, &length)| frequency != 0 && length == 0)
        || distance_frequencies
            .iter()
            .zip(&distance)
            .any(|(&frequency, &length)| frequency != 0 && length == 0)
    {
        return None;
    }
    huffman_code_lengths_are_valid(&literal).then_some(())?;
    huffman_code_lengths_are_valid(&distance).then_some(())?;
    let data_bits = token_bits_from_frequencies(
        literal_frequencies,
        distance_frequencies,
        &literal,
        &distance,
        extra_bits,
    )?;
    plan_for_explicit_lengths_with_cost(&literal, &distance, data_bits, exhaustive)
}

// Out of line: the scan of existing candidates outweighs a call.
#[inline(never)]
fn push_unique<T: PartialEq>(candidates: &mut Vec<T>, candidate: T) {
    if !candidates.iter().any(|current| current == &candidate) {
        candidates.push(candidate);
    }
}

fn trim_literal(lengths: &[u8]) -> usize {
    trim_to_last_nonzero(lengths, 257)
}

fn trim_distance(lengths: &[u8]) -> usize {
    trim_to_last_nonzero(lengths, 1)
}

// Out of line: every header plan trims both alphabets, from many places.
#[inline(never)]
fn trim_to_last_nonzero(lengths: &[u8], minimum: usize) -> usize {
    lengths
        .iter()
        .enumerate()
        .skip(minimum)
        .rfind(|(_, length)| **length != 0)
        .map_or(minimum, |(index, _)| index + 1)
}

/// Cheap estimate of one block's size, used to rank or prune candidates. It
/// never decides acceptance: selected candidates are replanned with exact
/// pricing.
pub(crate) fn estimate_boundary_block_bits(
    literal_frequencies: &[u32; 286],
    distance_frequencies: &[u32; 30],
    extra_bits: u64,
    strict: bool,
) -> Option<u64> {
    let mut build_literal_frequencies = *literal_frequencies;
    ensure_code_symbols(&mut build_literal_frequencies, strict);
    let mut build_distance_frequencies = *distance_frequencies;
    if build_distance_frequencies
        .iter()
        .all(|&frequency| frequency == 0)
    {
        build_distance_frequencies[0] = 1;
    }
    ensure_distance_symbols(&mut build_distance_frequencies, strict);

    let mut literal_lengths = [0_u8; 286];
    let mut distance_lengths = [0_u8; 30];
    let mut heap_scratch = TreeBuildScratch::default();
    make_lengths_deflopt_heap_into_with_scratch(
        &build_literal_frequencies,
        &mut literal_lengths,
        15,
        0,
        &mut heap_scratch,
    );
    make_lengths_deflopt_heap_into_with_scratch(
        &build_distance_frequencies,
        &mut distance_lengths,
        15,
        0,
        &mut heap_scratch,
    );
    let data_bits = token_bits_from_frequencies(
        literal_frequencies,
        distance_frequencies,
        &literal_lengths,
        &distance_lengths,
        extra_bits,
    )?;
    let dynamic_bits = greedy_dynamic_header_bits(
        &literal_lengths,
        &distance_lengths,
        data_bits,
        &mut heap_scratch,
    )?;

    let fixed_bits = token_bits_from_frequencies(
        literal_frequencies,
        distance_frequencies,
        &FIXED_LITERAL_CODE_LENGTHS,
        &FIXED_DISTANCE_CODE_LENGTHS,
        extra_bits,
    )?
    .checked_add(3)?;

    Some(dynamic_bits.min(fixed_bits))
}

fn greedy_dynamic_header_bits(
    literal_lengths: &[u8],
    distance_lengths: &[u8],
    data_bits: u64,
    heap_scratch: &mut TreeBuildScratch,
) -> Option<u64> {
    let hlit = trim_literal(literal_lengths);
    let hdist = trim_distance(distance_lengths);
    let concatenated_len = hlit.checked_add(hdist)?;
    let mut concatenated = [0_u8; MAX_DYNAMIC_CODE_LENGTH_COUNT];
    concatenated
        .get_mut(..hlit)?
        .copy_from_slice(&literal_lengths[..hlit]);
    concatenated
        .get_mut(hlit..concatenated_len)?
        .copy_from_slice(&distance_lengths[..hdist]);

    let rle = greedy_rle(&concatenated[..concatenated_len], false, false, false);
    let code_length_frequencies = rle_frequencies(&rle);
    let code_length_lengths = code_length_tree(
        CodeLengthBuilder::DefloptHeap(0),
        &code_length_frequencies,
        heap_scratch,
    );
    huffman_code_lengths_are_valid(&code_length_lengths).then_some(())?;
    if !code_length_tree_shape_is_valid(&code_length_lengths) {
        return None;
    }

    let hclen = trim_code_lengths(&code_length_lengths);
    let mut bits = 3_u64
        .checked_add(5)?
        .checked_add(5)?
        .checked_add(4)?
        .checked_add(u64::try_from(hclen).ok()?.checked_mul(3)?)?;
    for token in rle {
        let code_bits = *code_length_lengths.get(usize::from(token.symbol))?;
        if code_bits == 0 {
            return None;
        }
        bits = bits
            .checked_add(u64::from(code_bits))?
            .checked_add(rle_extra_bits(token.symbol))?;
    }
    bits.checked_add(data_bits)
}

/// Plan a dynamic block for given literal/length and distance code lengths,
/// without changing the tokens.
// Out of line: inlined, each caller also took a copy of the masked planner.
#[inline(never)]
pub(crate) fn plan_for_explicit_lengths(
    tokens: &[Token],
    literal_lengths: &[u8],
    distance_lengths: &[u8],
    exhaustive: bool,
) -> Option<DynamicPlan> {
    plan_for_explicit_lengths_masked(tokens, literal_lengths, distance_lengths, exhaustive, 0xff)
}

fn plan_for_explicit_lengths_masked(
    tokens: &[Token],
    literal_lengths: &[u8],
    distance_lengths: &[u8],
    exhaustive: bool,
    rle_mask: u8,
) -> Option<DynamicPlan> {
    let hlit = trim_literal(literal_lengths);
    let hdist = trim_distance(distance_lengths);
    let literal_lengths = &literal_lengths[..hlit];
    let distance_lengths = &distance_lengths[..hdist];
    let data_bits = token_bits(tokens, literal_lengths, distance_lengths)?;

    plan_for_trimmed_lengths(
        literal_lengths,
        distance_lengths,
        data_bits,
        exhaustive,
        rle_mask,
    )
}

// Out of line: each call plans a whole header.
#[inline(never)]
pub(crate) fn plan_for_explicit_lengths_with_cost(
    literal_lengths: &[u8],
    distance_lengths: &[u8],
    data_bits: u64,
    exhaustive: bool,
) -> Option<DynamicPlan> {
    let hlit = trim_literal(literal_lengths);
    let hdist = trim_distance(distance_lengths);
    plan_for_trimmed_lengths(
        &literal_lengths[..hlit],
        &distance_lengths[..hdist],
        data_bits,
        exhaustive,
        0xff,
    )
}

fn plan_for_explicit_lengths_with_cost_cached(
    literal_lengths: &[u8],
    distance_lengths: &[u8],
    data_bits: u64,
    exhaustive: bool,
    cache: &mut HeaderPlanCache,
) -> Option<DynamicPlan> {
    let hlit = trim_literal(literal_lengths);
    let hdist = trim_distance(distance_lengths);
    let literal_lengths = &literal_lengths[..hlit];
    let distance_lengths = &distance_lengths[..hdist];
    if !payload_tree_shape_is_valid(literal_lengths, false)
        || !payload_tree_shape_is_valid(distance_lengths, true)
    {
        return None;
    }
    cache.price(
        literal_lengths,
        distance_lengths,
        data_bits,
        exhaustive,
        0xff,
    )
}

/// Score a dynamic header with deft4j beta-17's ordered header grid, keeping
/// the data trees fixed and trimming HLIT and HDIST (beta-17 can keep the
/// source HLIT and HDIST for its source-dynamic base). Columbo's wider header
/// search would change which intermediate states the deft4j-derived route
/// keeps.
pub(crate) fn plan_for_deft4j_lengths_with_cost(
    literal_frequencies: &[u32; 286],
    distance_frequencies: &[u32; 30],
    extra_bits: u64,
    literal_lengths: &[u8; 286],
    distance_lengths: &[u8; 30],
    strict: bool,
    policy: Deft4jHeaderPolicy,
) -> Option<DynamicPlan> {
    let mut literal_lengths = *literal_lengths;
    apply_min_code_lengths(&mut literal_lengths, literal_frequencies, strict);
    let mut distance_lengths = *distance_lengths;
    apply_min_code_lengths(&mut distance_lengths, distance_frequencies, strict);

    let data_bits = token_bits_from_frequencies(
        literal_frequencies,
        distance_frequencies,
        &literal_lengths,
        &distance_lengths,
        extra_bits,
    )?;
    let hlit = trim_literal(&literal_lengths);
    let hdist = trim_distance(&distance_lengths);
    let literal = try_clone_slice(&literal_lengths[..hlit])?;
    let distance = try_clone_slice(&distance_lengths[..hdist])?;
    if !huffman_code_lengths_are_valid(&literal)
        || !huffman_code_lengths_are_valid(&distance)
        || !payload_tree_shape_is_valid(&literal, false)
        || !payload_tree_shape_is_valid(&distance, true)
    {
        return None;
    }

    let mut combined = Vec::new();
    combined
        .try_reserve_exact(literal.len().checked_add(distance.len())?)
        .ok()?;
    combined.extend_from_slice(&literal);
    combined.extend_from_slice(&distance);

    if policy == Deft4jHeaderPolicy::DefaultRecode {
        return build_deft4j_header(
            data_bits,
            &literal,
            &distance,
            &combined,
            DEFT4J_DEFAULT_RECODE_OPTIONS,
        );
    }

    let mut best = None;
    // Keep deft4j's loop order: of equal-sized headers the first spelling
    // wins, and it becomes the block used by later transformations and merges.
    for no_repeat_zeros in [false, true] {
        for prune in [false, true] {
            for no_repeat in [false, true] {
                if no_repeat_zeros && no_repeat {
                    continue;
                }
                for no_zero_repeat in [false, true] {
                    if no_repeat_zeros && !no_zero_repeat {
                        continue;
                    }
                    for no_long_zero_repeat in [false, true] {
                        for special_repeat in [true, false] {
                            if special_repeat {
                                if no_repeat {
                                    continue;
                                }
                                for use_eight in [true, false] {
                                    for use_seven in [true, false] {
                                        if !use_eight && !use_seven {
                                            continue;
                                        }
                                        if let Some(candidate) = build_deft4j_header(
                                            data_bits,
                                            &literal,
                                            &distance,
                                            &combined,
                                            Deft4jHeaderOptions {
                                                pack: Deft4jPackOptions {
                                                    special_repeat: true,
                                                    use_eight,
                                                    use_seven,
                                                    no_repeat,
                                                    no_zero_repeat,
                                                    no_long_zero_repeat,
                                                    no_repeat_zeros,
                                                },
                                                prune,
                                                optimize_header: true,
                                            },
                                        ) {
                                            keep_better(&mut best, candidate);
                                        }
                                    }
                                }
                            } else if let Some(candidate) = build_deft4j_header(
                                data_bits,
                                &literal,
                                &distance,
                                &combined,
                                Deft4jHeaderOptions {
                                    pack: Deft4jPackOptions {
                                        special_repeat: false,
                                        use_eight: false,
                                        use_seven: false,
                                        no_repeat,
                                        no_zero_repeat,
                                        no_long_zero_repeat,
                                        no_repeat_zeros,
                                    },
                                    prune,
                                    optimize_header: true,
                                },
                            ) {
                                keep_better(&mut best, candidate);
                            }
                        }
                    }
                }
            }
        }
    }
    best
}

fn apply_min_code_lengths(lengths: &mut [u8], frequencies: &[u32], enabled: bool) {
    if !enabled || lengths.iter().filter(|&&length| length != 0).count() >= 2 {
        return;
    }
    let used = frequencies.iter().position(|&frequency| frequency != 0);
    match used {
        Some(symbol) => {
            lengths[symbol] = 1;
            lengths[usize::from(symbol == 0)] = 1;
        }
        None => {
            lengths[0] = 1;
            lengths[1] = 1;
        }
    }
}

fn build_deft4j_header(
    data_bits: u64,
    literal_lengths: &[u8],
    distance_lengths: &[u8],
    combined: &[u8],
    options: Deft4jHeaderOptions,
) -> Option<DynamicPlan> {
    let mut rle = deft4j_pack_code_lengths(combined, options.pack)?;
    let mut code_length_lengths = deft4j_code_length_tree(&rle)?;

    if options.prune {
        if let Some((pruned, _)) =
            rewrite_rle_deft4j_literals(&rle, &rle_frequencies(&rle), &code_length_lengths, true)
        {
            rle = pruned;
            code_length_lengths = deft4j_code_length_tree(&rle)?;
        }
    }

    let mut plan = deft4j_dynamic_plan(
        data_bits,
        literal_lengths,
        distance_lengths,
        rle,
        code_length_lengths,
    )?;
    if options.optimize_header {
        if let Some((optimized, _)) = rewrite_rle_deft4j_literals(
            &plan.rle,
            &rle_frequencies(&plan.rle),
            &plan.code_length_lengths,
            false,
        ) {
            if let Some(candidate) = deft4j_dynamic_plan(
                data_bits,
                literal_lengths,
                distance_lengths,
                optimized,
                plan.code_length_lengths,
            ) {
                if candidate.bits < plan.bits {
                    plan = candidate;
                }
            }
        }
    }
    Some(plan)
}

fn deft4j_code_length_tree(rle: &[RleToken]) -> Option<[u8; 19]> {
    let frequencies = rle_frequencies(rle);
    let lengths = code_length_tree(
        CodeLengthBuilder::Deft4jJavaHeap,
        &frequencies,
        &mut TreeBuildScratch::default(),
    );
    if !code_length_tree_shape_is_valid(&lengths) {
        return None;
    }
    rle.iter()
        .all(|token| lengths[usize::from(token.symbol)] != 0)
        .then_some(lengths)
}

fn deft4j_dynamic_plan(
    data_bits: u64,
    literal_lengths: &[u8],
    distance_lengths: &[u8],
    rle: Vec<RleToken>,
    code_length_lengths: [u8; 19],
) -> Option<DynamicPlan> {
    let hclen = trim_code_lengths(&code_length_lengths);
    let mut plan = DynamicPlan {
        literal_lengths: try_clone_slice(literal_lengths)?,
        distance_lengths: try_clone_slice(distance_lengths)?,
        code_length_lengths,
        rle,
        hlit: literal_lengths.len(),
        hdist: distance_lengths.len(),
        hclen,
        bits: 0,
    };
    plan.bits = dynamic_bits(data_bits, &plan)?;
    Some(plan)
}

fn deft4j_pack_code_lengths(lengths: &[u8], options: Deft4jPackOptions) -> Option<Vec<RleToken>> {
    let mut output = Vec::new();
    output
        .try_reserve_exact(MAX_DYNAMIC_CODE_LENGTH_COUNT)
        .ok()?;
    let mut index = 0;
    while index < lengths.len() {
        let value = lengths[index];
        let mut run = 1;
        while index + run < lengths.len() && lengths[index + run] == value {
            run += 1;
        }
        index += run;

        if value == 0 {
            if !options.no_long_zero_repeat {
                while run >= 11 {
                    let count = run.min(138);
                    output.push(RleToken {
                        symbol: 18,
                        extra: (count - 11) as u8,
                    });
                    run -= count;
                }
            }
            if !options.no_zero_repeat {
                while run >= 3 {
                    let count = run.min(10);
                    output.push(RleToken {
                        symbol: 17,
                        extra: (count - 3) as u8,
                    });
                    run -= count;
                }
            }
        }

        if !options.no_repeat && run != 0 && (!options.no_repeat_zeros || value != 0) {
            output.push(RleToken {
                symbol: value,
                extra: 0,
            });
            run -= 1;
            let mut count = 6;
            while count >= 3 {
                if options.special_repeat && options.use_eight && run == 8 {
                    output.push(RleToken {
                        symbol: 16,
                        extra: 1,
                    });
                    output.push(RleToken {
                        symbol: 16,
                        extra: 1,
                    });
                    run -= 8;
                    break;
                }
                if options.special_repeat && options.use_seven && run == 7 {
                    output.push(RleToken {
                        symbol: 16,
                        extra: 1,
                    });
                    output.push(RleToken {
                        symbol: 16,
                        extra: 0,
                    });
                    run -= 7;
                    break;
                }
                if run >= count {
                    output.push(RleToken {
                        symbol: 16,
                        extra: (count - 3) as u8,
                    });
                    run -= count;
                } else {
                    count -= 1;
                }
            }
        }

        output.extend(
            std::iter::repeat(RleToken {
                symbol: value,
                extra: 0,
            })
            .take(run),
        );
    }
    (output.len() <= MAX_DYNAMIC_CODE_LENGTH_COUNT).then_some(output)
}

// Out of line: each call plans a whole header.
#[inline(never)]
fn plan_for_trimmed_lengths(
    literal_lengths: &[u8],
    distance_lengths: &[u8],
    data_bits: u64,
    exhaustive: bool,
    rle_mask: u8,
) -> Option<DynamicPlan> {
    let fingerprint =
        header_plan_fingerprint(literal_lengths, distance_lengths, exhaustive, rle_mask);
    let mut plan = stored_header_plan_kernel(
        fingerprint,
        literal_lengths,
        distance_lengths,
        exhaustive,
        rle_mask,
    )?;
    plan.bits = plan.bits.checked_add(data_bits)?;
    Some(plan)
}

/// An RLE and its symbol frequencies.
type CountedRle = (Vec<RleToken>, [u32; 19]);

#[derive(Default)]
struct HeaderPlanSearch {
    heap_scratch: TreeBuildScratch,
    best: Option<DynamicPlan>,
    /// This plan's runs of equal code lengths, which every RLE spells.
    runs: Vec<LengthRun>,
    /// Shortest RLEs of this plan's code lengths, with their symbol
    /// frequencies, by code-length code: RLE seeds often reach the same
    /// feedback tree.
    shortest_rles: Vec<([u8; 19], Option<CountedRle>)>,
    shortest_rle_scratch: ShortestRleScratch,
}

impl HeaderPlanSearch {
    fn shortest_rle(&mut self, length_count: usize, tree: &[u8; 19]) -> Option<&CountedRle> {
        let index = match self.shortest_rles.iter().position(|(seen, _)| seen == tree) {
            Some(index) => index,
            None => {
                let rle = shortest_rle_from_runs(
                    &self.runs,
                    length_count,
                    tree,
                    &mut self.shortest_rle_scratch,
                );
                self.shortest_rles.push((*tree, rle));
                self.shortest_rles.len() - 1
            }
        };
        self.shortest_rles[index].1.as_ref()
    }
}

/// Plan a header for the code lengths exactly as given, trailing zeros
/// included, so HLIT and HDIST stay fixed (R4).
pub(crate) fn plan_for_advertised_lengths(
    literal_lengths: &[u8],
    distance_lengths: &[u8],
    data_bits: u64,
) -> Option<DynamicPlan> {
    if !(257..=286).contains(&literal_lengths.len()) || !(1..=32).contains(&distance_lengths.len())
    {
        return None;
    }
    plan_for_trimmed_lengths_uncached(literal_lengths, distance_lengths, data_bits, true, 0xff)
}

fn plan_for_trimmed_lengths_uncached(
    literal_lengths: &[u8],
    distance_lengths: &[u8],
    data_bits: u64,
    exhaustive: bool,
    rle_mask: u8,
) -> Option<DynamicPlan> {
    if !payload_tree_shape_is_valid(literal_lengths, false)
        || !payload_tree_shape_is_valid(distance_lengths, true)
    {
        return None;
    }
    let decoded_length_count = literal_lengths.len().checked_add(distance_lengths.len())?;
    if decoded_length_count > MAX_DYNAMIC_CODE_LENGTH_COUNT {
        return None;
    }
    let mut decoded_length_storage = [0_u8; MAX_DYNAMIC_CODE_LENGTH_COUNT];
    decoded_length_storage[..literal_lengths.len()].copy_from_slice(literal_lengths);
    decoded_length_storage[literal_lengths.len()..decoded_length_count]
        .copy_from_slice(distance_lengths);
    let decoded_lengths = &decoded_length_storage[..decoded_length_count];
    // This thread's buffers, reused between plans; the best plan and the
    // shortest RLEs belong to one plan's code lengths.
    let mut search = HEADER_PLAN_SEARCH
        .try_with(Cell::take)
        .ok()
        .flatten()
        .unwrap_or_default();
    search.best = None;
    search.shortest_rles.clear();
    // Every RLE spells the same runs, so find them once.
    length_runs_into(decoded_lengths, &mut search.runs);
    for rle in rle_seed_candidates_from_runs(&search.runs, decoded_length_count, rle_mask) {
        consider_rle(
            data_bits,
            literal_lengths,
            distance_lengths,
            decoded_lengths,
            rle,
            exhaustive,
            &mut search,
        );
    }
    let best = search.best.take();
    let _ = HEADER_PLAN_SEARCH.try_with(|cell| cell.set(Some(search)));
    best
}

thread_local! {
    /// The uncached header planner's buffers on this thread, between plans.
    static HEADER_PLAN_SEARCH: Cell<Option<HeaderPlanSearch>> = const { Cell::new(None) };
}

#[derive(Default)]
struct RleSeedOpportunities {
    balanced_repeat: bool,
    zero_repeat: [bool; 4],
    zero_17: bool,
    zero_18: bool,
}

fn rle_seed_opportunities(runs: &[LengthRun]) -> RleSeedOpportunities {
    let mut opportunities = RleSeedOpportunities::default();
    for &(value, run) in runs {
        if value != 0 {
            let repeated = run.saturating_sub(1);
            opportunities.balanced_repeat |= repeated >= 7 && matches!(repeated % 6, 1 | 2);
            continue;
        }

        opportunities.zero_17 |= run >= 3;
        opportunities.zero_18 |= run >= 11;
        for no_17 in [false, true] {
            for no_18 in [false, true] {
                let mut remaining = run;
                while !no_18 && remaining >= 11 {
                    remaining -= remaining.min(138);
                }
                while !no_17 && remaining >= 3 {
                    remaining -= remaining.min(10);
                }
                let variant = usize::from(no_17) | (usize::from(no_18) << 1);
                opportunities.zero_repeat[variant] |= remaining >= 4;
            }
        }
    }
    opportunities
}

/// Return each distinct RLE seed once, in first-seen order. Bit `m` of
/// `rle_mask` enables mask `m`; mask bits 0, 1 and 2 disable codes 16, 17 and
/// 18.
#[cfg(test)]
fn rle_seed_candidates(decoded_lengths: &[u8], rle_mask: u8) -> Vec<Vec<RleToken>> {
    rle_seed_candidates_from_runs(
        &length_runs(decoded_lengths),
        decoded_lengths.len(),
        rle_mask,
    )
}

/// [`rle_seed_candidates`] for code lengths given as their `runs`, `count`
/// lengths in all.
fn rle_seed_candidates_from_runs(
    runs: &[LengthRun],
    count: usize,
    rle_mask: u8,
) -> Vec<Vec<RleToken>> {
    let opportunities = rle_seed_opportunities(runs);
    // Always keep bit 0: it also decides whether the balanced and zero-repeat
    // spellings run, even when disabling code 16 leaves the greedy spelling
    // unchanged.
    let relevant_mask =
        1 | (u8::from(opportunities.zero_17) << 1) | (u8::from(opportunities.zero_18) << 2);
    let mut seen_masks = [false; 8];
    let mut candidates = Vec::new();
    for mask in 0..8 {
        if rle_mask & (1 << mask) == 0 {
            continue;
        }
        let mask = mask & relevant_mask;
        if seen_masks[usize::from(mask)] {
            continue;
        }
        seen_masks[usize::from(mask)] = true;
        let no_16 = mask & 1 != 0;
        let no_17 = mask & 2 != 0;
        let no_18 = mask & 4 != 0;
        let rle = greedy_rle_from_runs(runs, count, no_16, no_17, no_18);
        push_unique(&mut candidates, rle);

        if !no_16 && opportunities.balanced_repeat {
            let balanced = balanced_repeat_rle_from_runs(runs, count, no_17, no_18);
            push_unique(&mut candidates, balanced);
        }

        if !no_16 {
            let zero_variant = usize::from(no_17) | (usize::from(no_18) << 1);
            if opportunities.zero_repeat[zero_variant] {
                let zero_repeat = columbo_zero_repeat_rle_from_runs(runs, count, no_17, no_18);
                push_unique(&mut candidates, zero_repeat);
            }
        }
    }
    candidates
}

/// Most payload bits a trade-off swap (R2) may add to shorten the header.
const MAX_SWAP_PAYLOAD_TAX: i64 = 18;
const MAX_TAXED_SWAPS_PER_ALPHABET: usize = 32;

/// Count the run transitions removed by writing `values` at `positions`, which
/// must be strictly increasing. Each affected edge is visited once, including
/// the edge between the literal/length and distance code lengths.
fn transitions_removed<const N: usize>(
    lengths: &[u8],
    positions: [usize; N],
    values: [u8; N],
) -> i64 {
    debug_assert!(positions.windows(2).all(|pair| pair[0] < pair[1]));
    let mut removed = 0;
    for (i, &position) in positions.iter().enumerate() {
        let before = lengths[position];
        let after = values[i];
        // An adjacent earlier replacement already visited this left edge.
        if position > 0 && (i == 0 || positions[i - 1] != position - 1) {
            let left = lengths[position - 1];
            removed += i64::from(left != before) - i64::from(left != after);
        }
        if position + 1 < lengths.len() {
            let right = lengths[position + 1];
            let changed_right = if i + 1 < N && positions[i + 1] == position + 1 {
                values[i + 1]
            } else {
                right
            };
            removed += i64::from(before != right) - i64::from(after != changed_right);
        }
    }
    removed
}

fn swap_transitions_removed(lengths: &[u8], a: usize, b: usize) -> i64 {
    transitions_removed(lengths, [a, b], [lengths[b], lengths[a]])
}

/// List up to `MAX_TAXED_SWAPS_PER_ALPHABET` swaps that add 1 to
/// `MAX_SWAP_PAYLOAD_TAX` payload bits and remove run transitions, ranked by
/// tax minus three bits per removed transition (a heuristic, not a bound).
fn payload_taxed_swap_proposals(
    lengths: &[u8],
    offset: usize,
    frequencies: &[u32],
    stop: &mut SearchStop<'_>,
) -> Option<Vec<(i64, i64, usize, usize)>> {
    let count = frequencies.len().min(lengths.len().checked_sub(offset)?);
    let mut proposals = Vec::new();
    proposals
        .try_reserve_exact(MAX_TAXED_SWAPS_PER_ALPHABET + 1)
        .ok()?;
    for a in 0..count {
        if stop.reached() {
            return None;
        }
        let length_a = lengths[offset + a];
        if length_a == 0 {
            continue;
        }
        for b in a + 1..count {
            let length_b = lengths[offset + b];
            if length_b == 0 || length_a == length_b {
                continue;
            }
            let tax = length_swap_delta(frequencies[a], frequencies[b], length_a, length_b);
            if !(1..=MAX_SWAP_PAYLOAD_TAX).contains(&tax) {
                continue;
            }
            let removed = swap_transitions_removed(lengths, offset + a, offset + b);
            if removed <= 0 {
                continue;
            }
            let proposal = (tax - 3 * removed, tax, a, b);
            let at = proposals.binary_search(&proposal).unwrap_or_else(|at| at);
            if at < MAX_TAXED_SWAPS_PER_ALPHABET {
                proposals.insert(at, proposal);
                proposals.truncate(MAX_TAXED_SWAPS_PER_ALPHABET);
            }
        }
    }
    Some(proposals)
}

/// Re-complete a finished strict block's degenerate distance code (R1b).
///
/// Planning completes an empty or singleton distance alphabet with two one-bit
/// codes. That is cheap when code-length symbol 1 is already in the header;
/// otherwise it can force 18 code-length code lengths to be sent. It runs
/// before the other terminal header methods, so they start from the cheaper
/// completion.
pub(crate) fn plan_strict_distance_completion(
    block: &super::model::ParsedBlock,
    strict: bool,
    prices_left: &mut usize,
    stop: &mut SearchStop<'_>,
) -> Option<DynamicPlan> {
    if !strict || *prices_left == 0 || stop.reached() {
        return None;
    }
    let original = block.original_dynamic.as_ref()?;
    let completions = strict_distance_completions(&block.distance_frequencies);
    if completions.is_empty() || !original.has_strictly_compatible_huffman_codes() {
        return None;
    }
    let literal = &original.literal_lengths[..trim_literal(&original.literal_lengths)];
    let distance = &original.distance_lengths[..trim_distance(&original.distance_lengths)];
    let original_data_bits = token_bits(&block.tokens, literal, distance)?;
    let original_bits = dynamic_bits(original_data_bits, original)?;
    let mut best = None;
    let mut best_bits = original_bits;
    for completion in completions {
        if *prices_left == 0 || stop.reached() {
            break;
        }
        *prices_left -= 1;
        let completion = &completion[..trim_distance(&completion)];
        let Some(data_bits) = completed_distance_data_bits(
            original_data_bits,
            &block.distance_frequencies,
            distance,
            completion,
        ) else {
            continue;
        };
        if let Some(plan) =
            plan_for_trimmed_lengths_uncached(literal, completion, data_bits, true, 0xff)
        {
            if plan.bits < best_bits && plan.has_strictly_compatible_huffman_codes() {
                best_bits = plan.bits;
                best = Some(plan);
            }
        }
    }
    best
}

/// Reprice a block's payload after replacing a degenerate distance code: at
/// most one distance symbol is used, so only its length changes the payload.
fn completed_distance_data_bits(
    original_data_bits: u64,
    distance_frequencies: &[u32; 30],
    original: &[u8],
    completion: &[u8],
) -> Option<u64> {
    let Some(symbol) = distance_frequencies
        .iter()
        .position(|&frequency| frequency != 0)
    else {
        return Some(original_data_bits);
    };
    let frequency = u64::from(distance_frequencies[symbol]);
    let old = u64::from(*original.get(symbol)?);
    let new = u64::from(*completion.get(symbol)?);
    original_data_bits
        .checked_sub(frequency.checked_mul(old)?)?
        .checked_add(frequency.checked_mul(new)?)
}

/// Reprice the unchanged tree, then bounded swaps in each alphabet that add
/// payload bits but may shorten the header, each applied alone to the parent
/// (R2). Swapping two nonzero lengths preserves the set of used symbols, the
/// maximum depth and the Kraft sum.
pub(crate) fn plan_payload_header_tradeoff(
    block: &super::model::ParsedBlock,
    strict: bool,
    prices_left: &mut usize,
    stop: &mut SearchStop<'_>,
) -> Option<DynamicPlan> {
    if *prices_left == 0 || stop.reached() {
        return None;
    }
    let original = block.original_dynamic.as_ref()?;
    if strict && !original.has_strictly_compatible_huffman_codes() {
        return None;
    }
    let literal = &original.literal_lengths[..trim_literal(&original.literal_lengths)];
    let distance = &original.distance_lengths[..trim_distance(&original.distance_lengths)];
    let data_bits = token_bits(&block.tokens, literal, distance)?;
    let original_bits = dynamic_bits(data_bits, original)?;
    let mut best = None;
    let mut best_bits = original_bits;
    *prices_left -= 1;
    if let Some(plan) = plan_for_trimmed_lengths_uncached(literal, distance, data_bits, true, 0xff)
    {
        if plan.bits < best_bits && (!strict || plan.has_strictly_compatible_huffman_codes()) {
            best_bits = plan.bits;
            best = Some(plan);
        }
    }
    let mut lengths = [0_u8; MAX_DYNAMIC_CODE_LENGTH_COUNT];
    let count = literal.len().checked_add(distance.len())?;
    let lengths = lengths.get_mut(..count)?;
    lengths[..literal.len()].copy_from_slice(literal);
    lengths[literal.len()..].copy_from_slice(distance);
    for (offset, frequencies) in [
        (0, &block.literal_frequencies[..literal.len()]),
        (literal.len(), block.distance_frequencies.as_slice()),
    ] {
        if *prices_left == 0 || stop.reached() {
            break;
        }
        let Some(proposals) = payload_taxed_swap_proposals(lengths, offset, frequencies, stop)
        else {
            break;
        };
        for (_, tax, a, b) in proposals {
            if *prices_left == 0 || stop.reached() {
                break;
            }
            lengths.swap(offset + a, offset + b);
            *prices_left -= 1;
            let plan = plan_for_trimmed_lengths_uncached(
                &lengths[..literal.len()],
                &lengths[literal.len()..],
                data_bits.checked_add(tax as u64)?,
                true,
                0xff,
            );
            lengths.swap(offset + a, offset + b);
            if let Some(plan) = plan {
                if plan.bits < best_bits
                    && (!strict || plan.has_strictly_compatible_huffman_codes())
                {
                    best_bits = plan.bits;
                    best = Some(plan);
                }
            }
        }
    }
    best
}

/// Try every legal HLIT from the trimmed minimum to 286, padding with zero
/// lengths (R3). The padding changes the zero run that continues into the
/// distance code lengths but no codeword. Each count gets a full header
/// search, since the existing code-length code alone can miss a gain.
pub(crate) fn plan_literal_span(
    block: &super::model::ParsedBlock,
    strict: bool,
    prices_left: &mut usize,
    stop: &mut SearchStop<'_>,
) -> Option<DynamicPlan> {
    if *prices_left == 0 || stop.reached() {
        return None;
    }
    let original = block.original_dynamic.as_ref()?;
    if strict && !original.has_strictly_compatible_huffman_codes() {
        return None;
    }
    let minimum = trim_literal(&original.literal_lengths);
    let mut literal = [0_u8; 286];
    if minimum >= literal.len() {
        return None;
    }
    literal[..minimum].copy_from_slice(&original.literal_lengths[..minimum]);
    let distance = &original.distance_lengths;
    let data_bits = token_bits(&block.tokens, &literal[..minimum], distance)?;
    let mut best_bits = dynamic_bits(data_bits, original)?;
    let mut best = None;
    for span in minimum..=literal.len() {
        if *prices_left == 0 || stop.reached() {
            break;
        }
        if span == original.hlit {
            continue;
        }
        *prices_left -= 1;
        // The ordinary explicit-length wrapper would trim away the padding.
        if let Some(plan) =
            plan_for_trimmed_lengths_uncached(&literal[..span], distance, data_bits, true, 0xff)
        {
            if plan.bits < best_bits && (!strict || plan.has_strictly_compatible_huffman_codes()) {
                best_bits = plan.bits;
                best = Some(plan);
            }
        }
    }
    best
}

fn length_swap_delta(frequency_a: u32, frequency_b: u32, length_a: u8, length_b: u8) -> i64 {
    let frequency_a = i64::from(frequency_a);
    let frequency_b = i64::from(frequency_b);
    let length_a = i64::from(length_a);
    let length_b = i64::from(length_b);
    frequency_a * (length_b - length_a) + frequency_b * (length_a - length_b)
}

#[allow(clippy::too_many_arguments)]
fn improve_by_length_swaps(
    tokens: &[Token],
    literal_frequencies: &[u32; 286],
    distance_frequencies: &[u32; 30],
    seed: &DynamicPlan,
    exhaustive: bool,
    stop: &mut SearchStop<'_>,
    best: &mut Option<DynamicPlan>,
) {
    let mut literal_lengths = vec![0_u8; 286];
    let mut distance_lengths = vec![0_u8; 30];
    if seed.literal_lengths.len() > literal_lengths.len()
        || seed.distance_lengths.len() > distance_lengths.len()
    {
        return;
    }
    literal_lengths[..seed.literal_lengths.len()].copy_from_slice(&seed.literal_lengths);
    distance_lengths[..seed.distance_lengths.len()].copy_from_slice(&seed.distance_lengths);

    // Choose each swap with only the greedy RLE spelling (mask 0x01), then
    // price the committed lengths with every spelling (mask 0xff).
    let Some(mut current) = plan_for_explicit_lengths_masked(
        tokens,
        &literal_lengths,
        &distance_lengths,
        exhaustive,
        0x01,
    ) else {
        return;
    };
    keep_better(best, current.clone());

    improve_one_tree_by_swaps(
        tokens,
        literal_frequencies,
        &mut literal_lengths,
        &mut distance_lengths,
        true,
        exhaustive,
        stop,
        &mut current,
        best,
    );
    improve_one_tree_by_swaps(
        tokens,
        distance_frequencies,
        &mut literal_lengths,
        &mut distance_lengths,
        false,
        exhaustive,
        stop,
        &mut current,
        best,
    );
}

#[allow(clippy::too_many_arguments)]
fn improve_one_tree_by_swaps(
    tokens: &[Token],
    frequencies: &[u32],
    literal_lengths: &mut [u8],
    distance_lengths: &mut [u8],
    literal_tree: bool,
    exhaustive: bool,
    stop: &mut SearchStop<'_>,
    current: &mut DynamicPlan,
    best: &mut Option<DynamicPlan>,
) {
    const MAX_PASSES: usize = 12;

    for _ in 0..MAX_PASSES {
        if stop.reached() {
            break;
        }
        // Swaps keep HLIT, HDIST and the tokens, so price the payload once per
        // pass and adjust it by `length_swap_delta` per trial.
        let hlit = trim_literal(literal_lengths);
        let hdist = trim_distance(distance_lengths);
        let Some(payload_bits) =
            token_bits(tokens, &literal_lengths[..hlit], &distance_lengths[..hdist])
        else {
            break;
        };

        let mut selected_pair = None;
        let mut selected_payload_bits = payload_bits;
        let mut selected_plan = current.clone();
        for a in 0..frequencies.len() {
            if stop.reached() {
                return;
            }
            let length_a = if literal_tree {
                literal_lengths[a]
            } else {
                distance_lengths[a]
            };
            if length_a == 0 {
                continue;
            }

            for b in (a + 1)..frequencies.len() {
                let length_b = if literal_tree {
                    literal_lengths[b]
                } else {
                    distance_lengths[b]
                };
                if length_b == 0 || length_a == length_b {
                    continue;
                }
                let delta = length_swap_delta(frequencies[a], frequencies[b], length_a, length_b);
                // Keep this zero-cost guard: relaxing it would change
                // intermediate winners and so redirect later searches.
                if delta > 0 {
                    continue;
                }
                let Some(data_bits) = payload_bits.checked_add_signed(delta) else {
                    continue;
                };

                if literal_tree {
                    literal_lengths.swap(a, b);
                } else {
                    distance_lengths.swap(a, b);
                }
                debug_assert_eq!(
                    Some(data_bits),
                    token_bits(tokens, &literal_lengths[..hlit], &distance_lengths[..hdist])
                );
                let candidate = plan_for_trimmed_lengths(
                    &literal_lengths[..hlit],
                    &distance_lengths[..hdist],
                    data_bits,
                    exhaustive,
                    0x01,
                );
                if literal_tree {
                    literal_lengths.swap(a, b);
                } else {
                    distance_lengths.swap(a, b);
                }

                if let Some(candidate) = candidate {
                    if candidate.bits < selected_plan.bits {
                        selected_pair = Some((a, b));
                        selected_payload_bits = data_bits;
                        selected_plan = candidate;
                    }
                }
            }
        }

        let Some((a, b)) = selected_pair else {
            break;
        };
        if literal_tree {
            literal_lengths.swap(a, b);
        } else {
            distance_lengths.swap(a, b);
        }
        *current = selected_plan;
        keep_better(best, current.clone());

        if let Some(full_plan) = plan_for_trimmed_lengths(
            &literal_lengths[..hlit],
            &distance_lengths[..hdist],
            selected_payload_bits,
            exhaustive,
            0xff,
        ) {
            keep_better(best, full_plan);
        }
    }
}

// Out of line: each call follows a whole header plan, and replacing the
// best plan drops one.
#[inline(never)]
fn keep_better(best: &mut Option<DynamicPlan>, candidate: DynamicPlan) {
    if best
        .as_ref()
        .map_or(true, |current| candidate.bits < current.bits)
    {
        *best = Some(candidate);
    }
}

fn consider_rle(
    data_bits: u64,
    literal_lengths: &[u8],
    distance_lengths: &[u8],
    decoded_lengths: &[u8],
    initial_rle: Vec<RleToken>,
    exhaustive: bool,
    search: &mut HeaderPlanSearch,
) {
    let mut rle = initial_rle;

    let mut frequencies = rle_frequencies(&rle);
    let mut prescored_deflopt_trees = [None; 4];
    let mut local_rewrite_steps = LocalRewriteSteps::default();
    for (variant, tree) in prescored_deflopt_trees.iter_mut().enumerate() {
        *tree = consider_columbo_deflopt_local_rewrite(
            data_bits,
            literal_lengths,
            distance_lengths,
            &rle,
            &frequencies,
            variant as u32,
            &mut local_rewrite_steps,
            search,
        );
    }

    // Up to four feedback passes: price every candidate code-length code,
    // then respell the RLE under the cheapest one and repeat. The pass count
    // follows Defluff, which always emits its fourth pass.
    let passes = 4;
    let candidate_capacity = if exhaustive { 18 } else { 6 };
    let mut code_length_candidates = Vec::with_capacity(candidate_capacity);
    for pass in 0..passes {
        let variants = 4;
        code_length_candidates.clear();
        let trees = CodeLengthTrees::new(&frequencies);
        for variant in 0..variants {
            // Variant 3 breaks both ties by frequency alone, so on the same
            // frequencies its order-key tree is the DeflOpt tree the local
            // rewrite built first.
            let prescored_order_tree = (pass == 0 && variant == FREQUENCY_ONLY_HEAP_VARIANT)
                .then_some(prescored_deflopt_trees[variant as usize])
                .flatten();
            let order_lengths = prescored_order_tree.unwrap_or_else(|| {
                trees.get(
                    CodeLengthBuilder::OrderHeap(variant),
                    &mut search.heap_scratch,
                )
            });
            push_unique(&mut code_length_candidates, order_lengths);
            // deft4j's tree builder is slow on the data alphabets but cheap
            // on the code-length alphabet, so every header search includes it.
            if variant == 0 {
                let deft4j_lengths =
                    trees.get(CodeLengthBuilder::Deft4jJavaHeap, &mut search.heap_scratch);
                consider_deft4j_pruned_header(
                    data_bits,
                    literal_lengths,
                    distance_lengths,
                    &rle,
                    &frequencies,
                    &deft4j_lengths,
                    &mut search.heap_scratch,
                    &mut search.best,
                );
                push_unique(&mut code_length_candidates, deft4j_lengths);
                let defluff_limited_lengths =
                    trees.get(CodeLengthBuilder::DefluffLimited, &mut search.heap_scratch);
                push_unique(&mut code_length_candidates, defluff_limited_lengths);
            }
            if exhaustive {
                // In pass 0, reuse the trees that the local DeflOpt rewrite
                // already built and priced.
                if pass == 0 {
                    if let Some(lengths) = prescored_deflopt_trees[variant as usize] {
                        push_unique(&mut code_length_candidates, lengths);
                    }
                } else if variant != FREQUENCY_ONLY_HEAP_VARIANT {
                    // Variant 3's DeflOpt tree is the order-key tree above.
                    let deflopt_lengths = trees.get(
                        CodeLengthBuilder::DefloptHeap(variant),
                        &mut search.heap_scratch,
                    );
                    push_unique(&mut code_length_candidates, deflopt_lengths);
                }
                let columbo_lengths = trees.get(
                    CodeLengthBuilder::Columbo(variant),
                    &mut search.heap_scratch,
                );
                push_unique(&mut code_length_candidates, columbo_lengths);

                if variant == 0 {
                    let defluff_lengths =
                        trees.get(CodeLengthBuilder::DefluffExact, &mut search.heap_scratch);
                    push_unique(&mut code_length_candidates, defluff_lengths);
                }
            }
        }

        let mut feedback_tree = None;
        let mut feedback_cost = INF;
        for code_length_lengths in code_length_candidates.iter().copied() {
            if !code_length_tree_shape_is_valid(&code_length_lengths) {
                continue;
            }
            let was_prescored = pass == 0
                && prescored_deflopt_trees
                    .iter()
                    .flatten()
                    .any(|tree| tree == &code_length_lengths);
            let valid_header = was_prescored
                || consider_dynamic_header(
                    data_bits,
                    literal_lengths,
                    distance_lengths,
                    &rle,
                    &frequencies,
                    code_length_lengths,
                    &mut search.best,
                )
                .is_some();
            if valid_header {
                let candidate_cost = rle_frequency_cost(&frequencies, &code_length_lengths);
                if candidate_cost < feedback_cost {
                    feedback_cost = candidate_cost;
                    feedback_tree = Some(code_length_lengths);
                }

                // As in DeflOpt, give shorter codes to more frequent RLE
                // symbols; a changed HCLEN can pay off even when the weighted
                // symbol cost ties.
                if !was_prescored {
                    let mut reordered = code_length_lengths;
                    if reorder_code_length_lengths(&mut reordered, &frequencies) {
                        consider_dynamic_header(
                            data_bits,
                            literal_lengths,
                            distance_lengths,
                            &rle,
                            &frequencies,
                            reordered,
                            &mut search.best,
                        );
                    }
                }
            }
        }

        if pass + 1 == passes {
            break;
        }
        let Some(tree) = feedback_tree else {
            break;
        };
        let mut rewritten = rewrite_rle_deflopt_local(&rle, &frequencies, &tree);
        if exhaustive {
            if let Some(shortest) = search.shortest_rle(decoded_lengths.len(), &tree) {
                // Both carry their exact symbol counts, which price them as
                // their tokens would.
                if rewritten.as_ref().map_or(true, |(_, local)| {
                    rle_frequency_cost(&shortest.1, &tree) < rle_frequency_cost(local, &tree)
                }) {
                    rewritten = Some(shortest.clone());
                }
            }
        }
        let Some((rewritten, rewritten_frequencies)) = rewritten else {
            break;
        };
        if rewritten == rle {
            break;
        }
        rle = rewritten;
        frequencies = rewritten_frequencies;
    }
}

/// Price a header using deft4j's code-length tree and equal-cost RLE prune,
/// then rebuild the code-length code. A tied rewrite can still shorten the
/// rebuilt header.
#[allow(clippy::too_many_arguments)]
fn consider_deft4j_pruned_header(
    data_bits: u64,
    literal_lengths: &[u8],
    distance_lengths: &[u8],
    rle: &[RleToken],
    frequencies: &[u32; 19],
    deft4j_lengths: &[u8],
    heap_scratch: &mut TreeBuildScratch,
    best: &mut Option<DynamicPlan>,
) {
    if deft4j_lengths.len() != 19 || !code_length_tree_shape_is_valid(deft4j_lengths) {
        return;
    }
    let mut code_length_lengths = [0_u8; 19];
    code_length_lengths.copy_from_slice(deft4j_lengths);
    if frequencies
        .iter()
        .zip(code_length_lengths)
        .any(|(&frequency, length)| frequency != 0 && length == 0)
    {
        return;
    }

    let Some((pruned, frequencies)) =
        rewrite_rle_deft4j_literals(rle, frequencies, &code_length_lengths, true)
    else {
        return;
    };
    let code_length_lengths = code_length_tree(
        CodeLengthBuilder::Deft4jJavaHeap,
        &frequencies,
        heap_scratch,
    );
    if !code_length_tree_shape_is_valid(&code_length_lengths) {
        return;
    }
    if frequencies
        .iter()
        .zip(code_length_lengths)
        .any(|(&frequency, length)| frequency != 0 && length == 0)
    {
        return;
    }

    consider_dynamic_header(
        data_bits,
        literal_lengths,
        distance_lengths,
        &pruned,
        &frequencies,
        code_length_lengths,
        best,
    );

    // deft4j's final `optimiseHeader` step expands only strictly dearer
    // repeats under the rebuilt tree and keeps that tree unchanged.
    if let Some((optimized, optimized_frequencies)) =
        rewrite_rle_deft4j_literals(&pruned, &frequencies, &code_length_lengths, false)
    {
        consider_dynamic_header(
            data_bits,
            literal_lengths,
            distance_lengths,
            &optimized,
            &optimized_frequencies,
            code_length_lengths,
            best,
        );
    }
}

/// Steps the DeflOpt local rewrite has taken from one RLE seed. Each heap
/// variant starts from the seed, and a step depends only on the current RLE
/// and the tree the variant builds there: the prices it offers, whether the
/// loop goes on, and the next RLE. A variant that builds an explored step's
/// tree at the same RLE would offer the same prices again, which cannot
/// change the best plan, so it moves straight to the recorded outcome.
#[derive(Default)]
struct LocalRewriteSteps {
    /// RLEs reached from the seed and their symbol frequencies; state 0, the
    /// seed itself, is not stored.
    states: Vec<CountedRle>,
    steps: Vec<(usize, [u8; 19], LocalRewriteOutcome)>,
}

#[derive(Clone, Copy)]
enum LocalRewriteOutcome {
    /// The tree gives no valid header.
    Invalid,
    Stop,
    Next(usize),
}

/// One step's result, before the next RLE is stored.
enum LocalRewriteStep {
    Invalid,
    Stop,
    Next(Vec<RleToken>, [u32; 19]),
}

/// Price DeflOpt-style local rewrite/rebuild candidates from one RLE seed.
/// Each rewrite must strictly lower `repeat_rank`, which bounds the loop.
/// Returns the first tree that gave a valid header, for reuse by the caller.
#[allow(clippy::too_many_arguments)]
fn consider_columbo_deflopt_local_rewrite(
    data_bits: u64,
    literal_lengths: &[u8],
    distance_lengths: &[u8],
    initial_rle: &[RleToken],
    initial_frequencies: &[u32; 19],
    variant: u32,
    steps: &mut LocalRewriteSteps,
    search: &mut HeaderPlanSearch,
) -> Option<[u8; 19]> {
    let mut state = 0;
    let mut initial_tree = None;

    loop {
        let frequencies = match state {
            0 => *initial_frequencies,
            _ => steps.states[state - 1].1,
        };
        let code_length_lengths = code_length_tree(
            CodeLengthBuilder::DefloptHeap(variant),
            &frequencies,
            &mut search.heap_scratch,
        );
        if !code_length_tree_shape_is_valid(&code_length_lengths) {
            return initial_tree;
        }

        let explored = steps
            .steps
            .iter()
            .find(|(explored_state, tree, _)| {
                *explored_state == state && *tree == code_length_lengths
            })
            .map(|&(_, _, outcome)| outcome);
        let outcome = match explored {
            Some(outcome) => outcome,
            None => {
                let rle = match state {
                    0 => initial_rle,
                    _ => &steps.states[state - 1].0,
                };
                let outcome = match local_rewrite_step(
                    data_bits,
                    literal_lengths,
                    distance_lengths,
                    rle,
                    &frequencies,
                    code_length_lengths,
                    search,
                ) {
                    LocalRewriteStep::Invalid => LocalRewriteOutcome::Invalid,
                    LocalRewriteStep::Stop => LocalRewriteOutcome::Stop,
                    LocalRewriteStep::Next(rle, frequencies) => {
                        steps.states.push((rle, frequencies));
                        LocalRewriteOutcome::Next(steps.states.len())
                    }
                };
                steps.steps.push((state, code_length_lengths, outcome));
                outcome
            }
        };
        match outcome {
            LocalRewriteOutcome::Invalid => return initial_tree,
            LocalRewriteOutcome::Stop => {
                initial_tree.get_or_insert(code_length_lengths);
                return initial_tree;
            }
            LocalRewriteOutcome::Next(next) => {
                initial_tree.get_or_insert(code_length_lengths);
                state = next;
            }
        }
    }
}

/// One step of the DeflOpt local rewrite: price the RLE under the tree (and its
/// frequency-ordered form), then rewrite the RLE under the tree and price the
/// rewrite if it lowers `repeat_rank`. Returns the rewrite to continue from.
fn local_rewrite_step(
    data_bits: u64,
    literal_lengths: &[u8],
    distance_lengths: &[u8],
    rle: &[RleToken],
    frequencies: &[u32; 19],
    code_length_lengths: [u8; 19],
    search: &mut HeaderPlanSearch,
) -> LocalRewriteStep {
    if consider_dynamic_header(
        data_bits,
        literal_lengths,
        distance_lengths,
        rle,
        frequencies,
        code_length_lengths,
        &mut search.best,
    )
    .is_none()
    {
        return LocalRewriteStep::Invalid;
    }

    // The rewrite loop follows the unmodified DeflOpt tree, which keeps tie
    // behavior deterministic.
    let mut reordered = code_length_lengths;
    if reorder_code_length_lengths(&mut reordered, frequencies) {
        consider_dynamic_header(
            data_bits,
            literal_lengths,
            distance_lengths,
            rle,
            frequencies,
            reordered,
            &mut search.best,
        );
    }

    let rank = repeat_rank(frequencies);
    if rank == 0 {
        return LocalRewriteStep::Stop;
    }
    let Some((rewritten, rewritten_frequencies)) =
        rewrite_rle_deflopt_local(rle, frequencies, &code_length_lengths)
    else {
        return LocalRewriteStep::Stop;
    };
    if repeat_rank(&rewritten_frequencies) >= rank {
        return LocalRewriteStep::Stop;
    }

    // Price the rewrite under the existing tree too: it is a distinct legal
    // header.
    consider_dynamic_header(
        data_bits,
        literal_lengths,
        distance_lengths,
        &rewritten,
        &rewritten_frequencies,
        code_length_lengths,
        &mut search.best,
    );
    LocalRewriteStep::Next(rewritten, rewritten_frequencies)
}

/// Expand a repeat code only when its explicit spelling is strictly cheaper
/// under the given tree. A code 17 of 3 to 6 zeros after a zero becomes code
/// 16 when code 16's length is no longer. `frequencies` are the input's symbol
/// frequencies. Returns `None` if nothing changes; otherwise the rewrite and
/// its symbol frequencies, the input's adjusted at each change.
fn rewrite_rle_deflopt_local(
    input: &[RleToken],
    frequencies: &[u32; 19],
    code_length_lengths: &[u8; 19],
) -> Option<CountedRle> {
    debug_assert_eq!(*frequencies, rle_frequencies(input));
    // The last rewrite of a search usually changes nothing; this finds most
    // such rewrites without reading the RLE.
    if !some_repeat_expands(frequencies, code_length_lengths, false)
        && !(frequencies[17] != 0 && zero_repeat_takes_code_16(code_length_lengths))
    {
        return None;
    }
    // Built from the first change on: a rewrite that passes the check above
    // can still change nothing.
    let mut output: Option<Vec<RleToken>> = None;
    // Input before this index is already in `output`; unchanged runs are
    // copied whole at the next change or the end.
    let mut copied = 0;
    let mut rewritten_frequencies = *frequencies;
    let mut previous = None;

    for (index, &token) in input.iter().enumerate() {
        // The replacement: one token, or `count` explicit copies of a value.
        let (replacement, repeated) = match token.symbol {
            value @ 0..=15 => {
                previous = Some(value);
                (None, None)
            }
            18 => {
                let count = usize::from(token.extra) + 11;
                previous = Some(0);
                let expand = explicit_rle_cost(code_length_lengths, 0, count)
                    < rle_symbol_cost(code_length_lengths, 18);
                (None, expand.then_some((0, count)))
            }
            17 => {
                let count = usize::from(token.extra) + 3;
                let follows_zero = previous == Some(0);
                previous = Some(0);
                if follows_zero
                    && token.extra <= 3
                    && zero_repeat_takes_code_16(code_length_lengths)
                {
                    let repeat = RleToken {
                        symbol: 16,
                        extra: token.extra,
                    };
                    (Some(repeat), None)
                } else {
                    let expand = explicit_rle_cost(code_length_lengths, 0, count)
                        < rle_symbol_cost(code_length_lengths, 17);
                    (None, expand.then_some((0, count)))
                }
            }
            16 => {
                let previous = previous?;
                let count = usize::from(token.extra) + 3;
                let expand = explicit_rle_cost(code_length_lengths, previous, count)
                    < rle_symbol_cost(code_length_lengths, 16);
                (None, expand.then_some((previous, count)))
            }
            _ => return None,
        };

        if replacement.is_none() && repeated.is_none() {
            continue;
        }
        let output = match output.as_mut() {
            Some(output) => output,
            None => {
                let mut started = Vec::new();
                // Expansions can outgrow the input, but not a header's lengths.
                started
                    .try_reserve(input.len().max(MAX_DYNAMIC_CODE_LENGTH_COUNT))
                    .ok()?;
                output.insert(started)
            }
        };
        output.extend_from_slice(&input[copied..index]);
        copied = index + 1;
        rewritten_frequencies[usize::from(token.symbol)] -= 1;
        if let Some((value, count)) = repeated {
            output.extend(
                std::iter::repeat(RleToken {
                    symbol: value,
                    extra: 0,
                })
                .take(count),
            );
            rewritten_frequencies[usize::from(value)] += count as u32;
        } else if let Some(replacement) = replacement {
            output.push(replacement);
            rewritten_frequencies[usize::from(replacement.symbol)] += 1;
        }
    }

    output.map(|mut output| {
        output.extend_from_slice(&input[copied..]);
        debug_assert_eq!(rewritten_frequencies, rle_frequencies(&output));
        (output, rewritten_frequencies)
    })
}

/// Whether spelling out some repeat code of an RLE with these symbol
/// frequencies costs less than the code under the tree, or with
/// `include_equal` no more; if not, a rewrite need not read the RLE. A repeat
/// spelled out costs more the longer it is, so each repeat code is checked at
/// its shortest count, and code 16 after every value it can follow.
fn some_repeat_expands(
    frequencies: &[u32; 19],
    code_length_lengths: &[u8; 19],
    include_equal: bool,
) -> bool {
    let expands = |value: u8, symbol: u8, shortest: usize| {
        let explicit = explicit_rle_cost(code_length_lengths, value, shortest);
        let repeat = rle_symbol_cost(code_length_lengths, symbol);
        explicit < repeat || (include_equal && explicit == repeat)
    };
    let zero_repeats = frequencies[17] != 0 || frequencies[18] != 0;
    (frequencies[18] != 0 && expands(0, 18, 11))
        || (frequencies[17] != 0 && expands(0, 17, 3))
        || (frequencies[16] != 0
            && (0..16).any(|value| {
                (frequencies[usize::from(value)] != 0 || (value == 0 && zero_repeats))
                    && expands(value, 16, 3)
            }))
}

/// Whether a code 17 of 3 to 6 zeros after a zero becomes code 16 in
/// [`rewrite_rle_deflopt_local`].
fn zero_repeat_takes_code_16(code_length_lengths: &[u8; 19]) -> bool {
    code_length_lengths[16] != 0
        && code_length_lengths[17] != 0
        && code_length_lengths[16] <= code_length_lengths[17]
}

/// Expand repeat codes that cost more than (or, with `include_equal`, the same
/// as) their explicit spelling under a fixed tree. `frequencies` are the
/// input's symbol frequencies. Returns `None` if nothing changes; otherwise
/// the rewrite and its symbol frequencies.
///
/// This mirrors deft4j's header-only prune: `include_equal` is used before a
/// tree rebuild, since a tied rewrite can change the next tree.
fn rewrite_rle_deft4j_literals(
    input: &[RleToken],
    frequencies: &[u32; 19],
    code_length_lengths: &[u8; 19],
    include_equal: bool,
) -> Option<CountedRle> {
    debug_assert_eq!(*frequencies, rle_frequencies(input));
    if !some_repeat_expands(frequencies, code_length_lengths, include_equal) {
        return None;
    }
    let mut rewritten_frequencies = *frequencies;
    // Built from the first change on, like the DeflOpt rewrite's, with
    // unchanged runs copied whole.
    let mut output: Option<Vec<RleToken>> = None;
    let mut copied = 0;
    let mut previous = None;

    for (index, &token) in input.iter().enumerate() {
        let (value, count) = match token.symbol {
            value @ 0..=15 => {
                previous = Some(value);
                continue;
            }
            16 => (previous?, usize::from(token.extra) + 3),
            17 => (0, usize::from(token.extra) + 3),
            18 => (0, usize::from(token.extra) + 11),
            _ => return None,
        };
        let explicit = explicit_rle_cost(code_length_lengths, value, count);
        let repeat = rle_symbol_cost(code_length_lengths, token.symbol);
        if explicit < repeat || (include_equal && explicit == repeat) {
            let output = match output.as_mut() {
                Some(output) => output,
                None => {
                    let mut started = Vec::new();
                    started.try_reserve(MAX_DYNAMIC_CODE_LENGTH_COUNT).ok()?;
                    output.insert(started)
                }
            };
            output.extend_from_slice(&input[copied..index]);
            copied = index + 1;
            if output.len().checked_add(count)? > MAX_DYNAMIC_CODE_LENGTH_COUNT {
                return None;
            }
            output.extend(
                std::iter::repeat(RleToken {
                    symbol: value,
                    extra: 0,
                })
                .take(count),
            );
            rewritten_frequencies[usize::from(token.symbol)] -= 1;
            rewritten_frequencies[usize::from(value)] += count as u32;
        }
        previous = Some(value);
    }
    output.map(|mut output| {
        output.extend_from_slice(&input[copied..]);
        (output, rewritten_frequencies)
    })
}

fn explicit_rle_cost(lengths: &[u8; 19], symbol: u8, count: usize) -> u64 {
    let length = lengths[usize::from(symbol)];
    if length == 0 {
        INF
    } else {
        u64::from(length).saturating_mul(count as u64).min(INF)
    }
}

fn rle_symbol_cost(lengths: &[u8; 19], symbol: u8) -> u64 {
    let length = lengths[usize::from(symbol)];
    if length == 0 {
        INF
    } else {
        u64::from(length)
            .saturating_add(rle_extra_bits(symbol))
            .min(INF)
    }
}

/// Two for each code 17 and one for each code 16 or 18, from the RLE's symbol
/// frequencies.
fn repeat_rank(frequencies: &[u32; 19]) -> u64 {
    2 * u64::from(frequencies[17]) + u64::from(frequencies[16]) + u64::from(frequencies[18])
}

/// Whether no two coded symbols have a longer code for a strictly larger
/// frequency: every length's smallest frequency is at least the largest
/// frequency of any longer code.
fn code_lengths_follow_frequencies(lengths: &[u8; 19], frequencies: &[u32; 19]) -> bool {
    let mut smallest = [u32::MAX; 8];
    let mut largest = [0_u32; 8];
    for (&length, &frequency) in lengths.iter().zip(frequencies) {
        let length = usize::from(length);
        if length == 0 {
            continue;
        }
        if length >= smallest.len() {
            return false;
        }
        smallest[length] = smallest[length].min(frequency);
        largest[length] = largest[length].max(frequency);
    }
    let mut largest_longer = 0;
    for length in (1..smallest.len()).rev() {
        if smallest[length] < largest_longer {
            return false;
        }
        largest_longer = largest_longer.max(largest[length]);
    }
    true
}

fn reorder_code_length_lengths(lengths: &mut [u8; 19], frequencies: &[u32; 19]) -> bool {
    // The loop swaps a pair only when its lengths and frequencies differ in
    // the same direction. When no pair does, nothing ever swaps, so a linear
    // check returns the same result; optimal trees always pass it.
    if code_lengths_follow_frequencies(lengths, frequencies) {
        return false;
    }
    let mut changed = false;
    for left in 0..18 {
        if lengths[left] == 0 {
            continue;
        }
        for right in (left + 1)..19 {
            if lengths[right] == 0 {
                continue;
            }
            let should_swap = if lengths[right] < lengths[left] {
                frequencies[right] < frequencies[left]
            } else if lengths[right] > lengths[left] {
                frequencies[right] > frequencies[left]
            } else {
                false
            };
            if should_swap {
                lengths.swap(left, right);
                changed = true;
            }
        }
    }
    changed
}

/// A run of equal code lengths: the length and how many times it repeats.
type LengthRun = (u8, usize);

/// The maximal runs of equal values in `lengths`, in order.
fn length_runs(lengths: &[u8]) -> Vec<LengthRun> {
    let mut runs = Vec::new();
    length_runs_into(lengths, &mut runs);
    runs
}

/// [`length_runs`] into a reused buffer.
fn length_runs_into(lengths: &[u8], runs: &mut Vec<LengthRun>) {
    runs.clear();
    runs.reserve(lengths.len());
    let mut index = 0;
    while index < lengths.len() {
        let value = lengths[index];
        let mut run = 1;
        while index + run < lengths.len() && lengths[index + run] == value {
            run += 1;
        }
        runs.push((value, run));
        index += run;
    }
}

fn greedy_rle(lengths: &[u8], no_16: bool, no_17: bool, no_18: bool) -> Vec<RleToken> {
    greedy_rle_from_runs(&length_runs(lengths), lengths.len(), no_16, no_17, no_18)
}

fn greedy_rle_from_runs(
    runs: &[LengthRun],
    length_count: usize,
    no_16: bool,
    no_17: bool,
    no_18: bool,
) -> Vec<RleToken> {
    // Each token spells at least one length.
    let mut output = Vec::with_capacity(length_count);
    for &(value, mut run) in runs {
        if value == 0 {
            while !no_18 && run >= 11 {
                let count = run.min(138);
                output.push(RleToken {
                    symbol: 18,
                    extra: (count - 11) as u8,
                });
                run -= count;
            }
            while !no_17 && run >= 3 {
                let count = run.min(10);
                output.push(RleToken {
                    symbol: 17,
                    extra: (count - 3) as u8,
                });
                run -= count;
            }
        } else if !no_16 && run >= 4 {
            output.push(RleToken {
                symbol: value,
                extra: 0,
            });
            run -= 1;
            while run >= 3 {
                let count = run.min(6);
                output.push(RleToken {
                    symbol: 16,
                    extra: (count - 3) as u8,
                });
                run -= count;
            }
        }
        output.extend(
            std::iter::repeat(RleToken {
                symbol: value,
                extra: 0,
            })
            .take(run),
        );
    }
    output
}

#[cfg(test)]
fn balanced_repeat_rle(lengths: &[u8], no_17: bool, no_18: bool) -> Vec<RleToken> {
    balanced_repeat_rle_from_runs(&length_runs(lengths), lengths.len(), no_17, no_18)
}

/// Generalize deft4j's balanced code-16 splits (4+3 and 4+4 for seven and
/// eight repeats) to every run where greedy six-repeat codes would leave one or
/// two explicit values.
fn balanced_repeat_rle_from_runs(
    runs: &[LengthRun],
    length_count: usize,
    no_17: bool,
    no_18: bool,
) -> Vec<RleToken> {
    // Each token spells at least one length.
    let mut output = Vec::with_capacity(length_count);
    for &(value, mut run) in runs {
        if value == 0 {
            while !no_18 && run >= 11 {
                let count = run.min(138);
                output.push(RleToken {
                    symbol: 18,
                    extra: (count - 11) as u8,
                });
                run -= count;
            }
            while !no_17 && run >= 3 {
                let count = run.min(10);
                output.push(RleToken {
                    symbol: 17,
                    extra: (count - 3) as u8,
                });
                run -= count;
            }
        } else if run >= 4 {
            output.push(RleToken {
                symbol: value,
                extra: 0,
            });
            run -= 1;
            while run >= 3 {
                let count = if matches!(run % 6, 1 | 2) && run >= 7 {
                    4
                } else {
                    run.min(6)
                };
                output.push(RleToken {
                    symbol: 16,
                    extra: (count - 3) as u8,
                });
                run -= count;
            }
        }
        output.extend(
            std::iter::repeat(RleToken {
                symbol: value,
                extra: 0,
            })
            .take(run),
        );
    }
    output
}

#[cfg(test)]
fn columbo_zero_repeat_rle(lengths: &[u8], no_17: bool, no_18: bool) -> Vec<RleToken> {
    columbo_zero_repeat_rle_from_runs(&length_runs(lengths), lengths.len(), no_17, no_18)
}

/// Like `balanced_repeat_rle_from_runs`, but four or more zeros left after the
/// enabled codes 18 and 17 are spelled as one explicit zero followed by code
/// 16, as deft4j allows.
fn columbo_zero_repeat_rle_from_runs(
    runs: &[LengthRun],
    length_count: usize,
    no_17: bool,
    no_18: bool,
) -> Vec<RleToken> {
    // Each token spells at least one length.
    let mut output = Vec::with_capacity(length_count);
    for &(value, mut run) in runs {
        if value == 0 {
            while !no_18 && run >= 11 {
                let count = run.min(138);
                output.push(RleToken {
                    symbol: 18,
                    extra: (count - 11) as u8,
                });
                run -= count;
            }
            while !no_17 && run >= 3 {
                let count = run.min(10);
                output.push(RleToken {
                    symbol: 17,
                    extra: (count - 3) as u8,
                });
                run -= count;
            }
        }

        if run >= 4 {
            output.push(RleToken {
                symbol: value,
                extra: 0,
            });
            run -= 1;
            while run >= 3 {
                let count = if matches!(run % 6, 1 | 2) && run >= 7 {
                    4
                } else {
                    run.min(6)
                };
                output.push(RleToken {
                    symbol: 16,
                    extra: (count - 3) as u8,
                });
                run -= count;
            }
        }
        output.extend(
            std::iter::repeat(RleToken {
                symbol: value,
                extra: 0,
            })
            .take(run),
        );
    }
    output
}

fn rle_frequencies(rle: &[RleToken]) -> [u32; 19] {
    // Four interleaved tables, so a run of one symbol does not wait for each
    // increment's store before the next.
    let mut tables = [[0_u32; 19]; 4];
    let mut chunks = rle.chunks_exact(4);
    for chunk in &mut chunks {
        for (table, token) in tables.iter_mut().zip(chunk) {
            table[usize::from(token.symbol)] += 1;
        }
    }
    for token in chunks.remainder() {
        tables[0][usize::from(token.symbol)] += 1;
    }
    let mut frequencies = [0_u32; 19];
    for table in &tables {
        for (frequency, &count) in frequencies.iter_mut().zip(table) {
            *frequency += count;
        }
    }
    frequencies
}

fn trim_code_lengths(lengths: &[u8; 19]) -> usize {
    (4..19)
        .rev()
        .find(|&index| lengths[CODE_LENGTH_ORDER[index]] != 0)
        .map_or(4, |index| index + 1)
}

/// Bits of the RLE-coded code lengths, repeat extra bits included, from the RLE
/// histogram, so pricing many code-length codes for one spelling does not
/// rescan its tokens.
fn rle_frequency_cost(frequencies: &[u32; 19], lengths: &[u8; 19]) -> u64 {
    // A used symbol without a code makes the cost infinite. Otherwise each
    // term is below 2^41, so wrapping arithmetic never wraps, and keeps
    // release overflow checks out of the loop.
    let mut sum = 0_u64;
    let mut missing = false;
    for symbol in 0..19 {
        let frequency = u64::from(frequencies[symbol]);
        let length = lengths[symbol];
        missing |= frequency != 0 && length == 0;
        let cost = u64::from(length).wrapping_add(RLE_EXTRA_BITS[symbol]);
        sum = sum.wrapping_add(frequency.wrapping_mul(cost));
    }
    if missing {
        INF
    } else {
        sum
    }
}

fn dynamic_bits_from_rle_frequencies(
    data_bits: u64,
    hclen: usize,
    frequencies: &[u32; 19],
    lengths: &[u8; 19],
) -> Option<u64> {
    let rle_bits = rle_frequency_cost(frequencies, lengths);
    if rle_bits == INF {
        return None;
    }
    let bits = 3_u64
        .checked_add(5 + 5 + 4)?
        .checked_add(u64::try_from(hclen).ok()?.checked_mul(3)?)?
        .checked_add(rle_bits)?;
    bits.checked_add(data_bits)
}

/// Price one header and replace `best` on a strict improvement. Returns `None`
/// if the code-length code cannot spell `rle`.
#[allow(clippy::too_many_arguments)]
fn consider_dynamic_header(
    data_bits: u64,
    literal_lengths: &[u8],
    distance_lengths: &[u8],
    rle: &[RleToken],
    frequencies: &[u32; 19],
    code_length_lengths: [u8; 19],
    best: &mut Option<DynamicPlan>,
) -> Option<u64> {
    let hclen = trim_code_lengths(&code_length_lengths);
    let bits =
        dynamic_bits_from_rle_frequencies(data_bits, hclen, frequencies, &code_length_lengths)?;
    match best {
        Some(current) if bits >= current.bits => {}
        // A search improves its best plan several times; reuse its buffers.
        Some(current) => {
            for (buffer, lengths) in [
                (&mut current.literal_lengths, literal_lengths),
                (&mut current.distance_lengths, distance_lengths),
            ] {
                buffer.clear();
                buffer.extend_from_slice(lengths);
            }
            current.rle.clear();
            current.rle.extend_from_slice(rle);
            current.code_length_lengths = code_length_lengths;
            current.hlit = literal_lengths.len();
            current.hdist = distance_lengths.len();
            current.hclen = hclen;
            current.bits = bits;
        }
        None => {
            *best = Some(DynamicPlan {
                literal_lengths: literal_lengths.to_vec(),
                distance_lengths: distance_lengths.to_vec(),
                code_length_lengths,
                rle: rle.to_vec(),
                hlit: literal_lengths.len(),
                hdist: distance_lengths.len(),
                hclen,
                bits,
            });
        }
    }
    Some(bits)
}

// Out of line: each call scans the header's run-length tokens.
#[inline(never)]
fn dynamic_bits(data_bits: u64, plan: &DynamicPlan) -> Option<u64> {
    let mut bits = 3_u64 + 5 + 5 + 4 + u64::try_from(plan.hclen).ok()? * 3;
    for token in &plan.rle {
        let code_bits = *plan.code_length_lengths.get(usize::from(token.symbol))?;
        if code_bits == 0 {
            return None;
        }
        bits = bits.checked_add(u64::from(code_bits) + rle_extra_bits(token.symbol))?;
    }
    bits.checked_add(data_bits)
}

const fn rle_extra_bits(symbol: u8) -> u64 {
    match symbol {
        16 => 2,
        17 => 3,
        18 => 7,
        _ => 0,
    }
}

/// [`rle_extra_bits`] by symbol.
const RLE_EXTRA_BITS: [u64; 19] = {
    let mut bits = [0; 19];
    let mut symbol = 0;
    while symbol < bits.len() {
        bits[symbol] = rle_extra_bits(symbol as u8);
        symbol += 1;
    }
    bits
};

#[cfg(test)]
fn rle_cost(rle: &[RleToken], lengths: &[u8; 19]) -> u64 {
    rle.iter()
        .map(|token| rle_symbol_cost(lengths, token.symbol))
        .fold(0_u64, |sum, value| sum.saturating_add(value).min(INF))
}

/// Find the cheapest RLE spelling of `lengths` under a fixed code-length code.
pub(super) fn shortest_rle(lengths: &[u8], costs: &[u8; 19]) -> Option<Vec<RleToken>> {
    shortest_rle_with_frequencies(lengths, costs, &mut ShortestRleScratch::default())
        .map(|(rle, _)| rle)
}

/// Marks a run length no spelling reaches; no code-length symbol is this large.
const UNSET_RLE_STEP: RleToken = RleToken {
    symbol: u8::MAX,
    extra: 0,
};

/// Slots for run spellings: one for each zero-run key of four 3-bit costs,
/// then one for each other key of two.
const RUN_SPELLING_SLOTS: usize = (1 << 12) + (1 << 6);
/// A slot with no run spellings yet.
const NO_RUN_SPELLINGS: u16 = u16::MAX;

/// Run-spelling entries a thread keeps, 4 bytes each, before dropping them all.
const MAX_RUN_SPELLING_ENTRIES: usize = 1 << 17;

/// The cheapest spellings of runs of one code length, by run length, under the
/// costs that can spell it: its own code and code 16, and for zeros codes 17
/// and 18. No code crosses a run of equal lengths, since code 16 repeats the
/// previous length and codes 17 and 18 write zeros, so a run's spelling depends
/// only on its length and these costs. A token below 16 stands for the run's
/// own length.
#[derive(Default)]
struct RunSpellings {
    /// Whether the length is zero, then the costs these spellings are for.
    key: u64,
    /// `first[run]` starts a run of `run` lengths, or is [`UNSET_RLE_STEP`]
    /// when no spelling exists.
    first: Vec<RleToken>,
    /// `inside[left]` spells on from a later position of a run with `left`
    /// lengths still to spell, where code 16 can repeat the length before.
    inside: Vec<RleToken>,
}

/// Run spellings for [`shortest_rle_with_frequencies`], kept between searches
/// on one thread, and the buffers that build them.
#[derive(Default)]
struct ShortestRleScratch {
    /// The index in `spellings` for each cost slot, or [`NO_RUN_SPELLINGS`].
    slots: Vec<u16>,
    spellings: Vec<RunSpellings>,
    /// Run lengths held by all `spellings`.
    entries: usize,
    /// Each count of lengths left's cheapest cost, while spellings are built.
    costs: Vec<u64>,
    /// Code 18's deque while spellings are built.
    zero_repeats: Vec<usize>,
}

impl ShortestRleScratch {
    /// The index of the spellings of runs of `value` up to `run` long under
    /// `costs`, building or extending them if needed, and whether an index
    /// found earlier may now hold other spellings. `None` if buffers cannot
    /// grow.
    fn run_spellings(
        &mut self,
        value: u8,
        costs: &[u8; 19],
        run: usize,
        longest: usize,
    ) -> Option<(u16, bool)> {
        let explicit = costs[usize::from(value)];
        let zero = value == 0;
        let (short_zero, long_zero) = if zero { (costs[17], costs[18]) } else { (0, 0) };
        let key = u64::from(zero) << 32
            | u64::from(u32::from_le_bytes([
                explicit, costs[16], short_zero, long_zero,
            ]));
        let slot = if zero {
            usize::from(explicit & 7)
                | usize::from(costs[16] & 7) << 3
                | usize::from(short_zero & 7) << 6
                | usize::from(long_zero & 7) << 9
        } else {
            (1 << 12) | usize::from(explicit & 7) | usize::from(costs[16] & 7) << 3
        };
        if self.slots.is_empty() {
            self.slots.try_reserve_exact(RUN_SPELLING_SLOTS).ok()?;
            self.slots.resize(RUN_SPELLING_SLOTS, NO_RUN_SPELLINGS);
        }
        let mut index = self.slots[slot];
        if let Some(spellings) = self.spellings.get(usize::from(index)) {
            if spellings.key == key && spellings.first.len() > run {
                return Some((index, false));
            }
        }
        // Build at least twice the old length, so a run that keeps growing
        // rebuilds its spellings only a few times. Costs above 7 bits can
        // share a slot, whose spellings are then rebuilt for the new key.
        let (old, mut replaced) = match self.spellings.get(usize::from(index)) {
            Some(spellings) if spellings.key == key => (spellings.first.len(), false),
            Some(_) => (0, true),
            None => (0, false),
        };
        let length = run.max(old.saturating_mul(2).min(longest)) + 1;
        if self.entries + length > MAX_RUN_SPELLING_ENTRIES {
            replaced = true;
            self.spellings.clear();
            self.slots.fill(NO_RUN_SPELLINGS);
            self.entries = 0;
            index = NO_RUN_SPELLINGS;
        }
        if index == NO_RUN_SPELLINGS {
            index = u16::try_from(self.spellings.len()).ok()?;
            self.spellings.try_reserve(1).ok()?;
            self.spellings.push(RunSpellings::default());
            self.slots[slot] = index;
        }
        let spellings = &mut self.spellings[usize::from(index)];
        self.entries = self.entries - spellings.first.len() + length;
        spellings.key = key;
        build_run_spellings(
            spellings,
            [explicit, costs[16], short_zero, long_zero],
            zero,
            length,
            &mut self.costs,
            &mut self.zero_repeats,
        )?;
        Some((index, replaced))
    }
}

/// Fill `spellings` for runs shorter than `length` under the costs of a run's
/// own code and codes 16, 17 and 18, the last two only for `zero`. Lengths left
/// are solved from a run's end, each from shorter ones. A tie keeps the option
/// offered first: an explicit length, then codes 16, 17 and 18, each with its
/// shortest repeat. Costs stay below `INF` plus a few bits, so sums cannot
/// overflow, and a sum through an unreachable `INF` never wins.
fn build_run_spellings(
    spellings: &mut RunSpellings,
    [explicit, repeat, short_zero, long_zero]: [u8; 4],
    zero: bool,
    length: usize,
    costs: &mut Vec<u64>,
    zero_repeats: &mut Vec<usize>,
) -> Option<()> {
    let RunSpellings { first, inside, .. } = spellings;
    for buffer in [&mut *first, &mut *inside] {
        buffer.clear();
        buffer.try_reserve_exact(length).ok()?;
    }
    costs.clear();
    costs.try_reserve_exact(length).ok()?;
    zero_repeats.clear();
    zero_repeats.try_reserve_exact(length).ok()?;
    let price =
        |cost: u8, symbol: u8| (cost != 0).then(|| u64::from(cost) + rle_extra_bits(symbol));
    let explicit = price(explicit, 0);
    let repeat = price(repeat, 16);
    let short_zero = if zero { price(short_zero, 17) } else { None };
    let long_zero = if zero { price(long_zero, 18) } else { None };

    // `costs[left]` is the cheapest spelling of a run's last `left` lengths
    // past its first; a run's end costs nothing more.
    first.push(UNSET_RLE_STEP);
    inside.push(UNSET_RLE_STEP);
    costs.push(0);
    // Code 18 costs the same for every repeat from 11 to 138, so a monotonic
    // deque in `zero_repeats[front..]` keeps the cheapest of what it can leave
    // to spell. Newer entries are shorter repeats, so dropping equal costs
    // keeps the preference for the shortest.
    let mut front = 0;
    for left in 1..length {
        let mut best_inside = (INF, UNSET_RLE_STEP);
        let mut best_first = (INF, UNSET_RLE_STEP);
        let mut offer = |cost: u64, symbol: u8, extra: u8, at_first: bool| {
            let token = RleToken { symbol, extra };
            if cost < best_inside.0 {
                best_inside = (cost, token);
            }
            if at_first && cost < best_first.0 {
                best_first = (cost, token);
            }
        };
        let long_count = if long_zero.is_some() && left >= 11 {
            let newest = left - 11;
            while front < zero_repeats.len() && left - zero_repeats[front] > 138 {
                front += 1;
            }
            while zero_repeats.len() > front
                && zero_repeats
                    .last()
                    .is_some_and(|&last| costs[last] >= costs[newest])
            {
                zero_repeats.pop();
            }
            zero_repeats.push(newest);
            Some(left - zero_repeats[front])
        } else {
            None
        };
        if let Some(explicit) = explicit {
            offer(explicit + costs[left - 1], 0, 0, true);
        }
        if left >= 3 {
            // Repeats in order of count, so the earliest minimum is the
            // shortest repeat.
            let window = |most: usize| {
                let mut count = 3;
                for candidate in 4..=left.min(most) {
                    if costs[left - candidate] < costs[left - count] {
                        count = candidate;
                    }
                }
                count
            };
            if let Some(repeat) = repeat {
                let count = window(6);
                offer(repeat + costs[left - count], 16, (count - 3) as u8, false);
            }
            if let Some(short_zero) = short_zero {
                let count = window(10);
                offer(
                    short_zero + costs[left - count],
                    17,
                    (count - 3) as u8,
                    true,
                );
            }
            if let (Some(long_zero), Some(count)) = (long_zero, long_count) {
                offer(
                    long_zero + costs[left - count],
                    18,
                    (count - 11) as u8,
                    true,
                );
            }
        }
        costs.push(best_inside.0);
        inside.push(best_inside.1);
        first.push(best_first.1);
    }
    Some(())
}

/// [`shortest_rle`] with the RLE's symbol frequencies. Each run of equal
/// lengths takes its cheapest spelling, kept in `scratch` for the next search
/// with the same costs. `None` also when the buffers cannot grow.
fn shortest_rle_with_frequencies(
    lengths: &[u8],
    costs: &[u8; 19],
    scratch: &mut ShortestRleScratch,
) -> Option<CountedRle> {
    shortest_rle_from_runs(&length_runs(lengths), lengths.len(), costs, scratch)
}

/// [`shortest_rle_with_frequencies`] for code lengths given as their `runs`,
/// `length_count` lengths in all.
fn shortest_rle_from_runs(
    runs: &[LengthRun],
    length_count: usize,
    costs: &[u8; 19],
    scratch: &mut ShortestRleScratch,
) -> Option<CountedRle> {
    // Each token spells at least one length.
    let mut output = Vec::new();
    output.try_reserve_exact(length_count).ok()?;
    // This search's spellings by length, found once each.
    let mut by_value = [NO_RUN_SPELLINGS; 16];
    for &(value, mut run) in runs {
        let known = by_value
            .get(usize::from(value))
            .and_then(|&known| scratch.spellings.get(usize::from(known)))
            .filter(|spellings| spellings.first.len() > run);
        let spellings = match known {
            Some(spellings) => spellings,
            None => {
                let (found, replaced) = scratch.run_spellings(value, costs, run, length_count)?;
                if replaced {
                    by_value = [NO_RUN_SPELLINGS; 16];
                }
                if let Some(known) = by_value.get_mut(usize::from(value)) {
                    *known = found;
                }
                &scratch.spellings[usize::from(found)]
            }
        };
        let mut token = spellings.first[run];
        if run == 1 && token.symbol < 16 {
            output.push(RleToken {
                symbol: value,
                extra: 0,
            });
            continue;
        }
        loop {
            let count = match token.symbol {
                0..=15 => 1,
                16 | 17 => usize::from(token.extra) + 3,
                18 => usize::from(token.extra) + 11,
                _ => return None,
            };
            output.push(if token.symbol < 16 {
                RleToken {
                    symbol: value,
                    extra: 0,
                }
            } else {
                token
            });
            run -= count;
            if run == 0 {
                break;
            }
            token = spellings.inside[run];
        }
    }
    let frequencies = rle_frequencies(&output);
    Some((output, frequencies))
}

#[cfg(test)]
pub(crate) mod test_support;
#[cfg(test)]
mod tests;
