// SPDX-License-Identifier: MIT
// SPDX-FileCopyrightText: 2026 Andrew C.E. Dent <https://github.com/ace-dent>

//! Planning and emission for one structural Deflate block.

use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::Arc;

use crate::{Error, Options, Result};

use super::bitstream::BitWriter;
use super::header::{best_dynamic_plan_cached, token_bits, HeaderPlanCache, PackedDynamicPlan};
use super::huffman::{
    fixed_trees, Huffman, FIXED_DISTANCE_CODE_LENGTHS, FIXED_LITERAL_CODE_LENGTHS,
};
use super::model::{
    same_tokens, token_extra_bits, DynamicPlan, OriginalBits, ParsedBlock, PlannedBlock,
    Representation, SourceBlockType, Token, CODE_LENGTH_ORDER,
};
use super::plan_store::{Charged, PlanStore};
use super::stop::SearchStop;

/// Per-route limits for [`CanonicalPlanCache`]. Keys hold `Arc` token buffers,
/// so the byte limit bounds memory kept alive for discarded candidates; entries
/// sharing one `Arc` are each charged in full.
const MAX_CANONICAL_PLAN_CACHE_ENTRIES: usize = 512;
const MAX_CANONICAL_PLAN_CACHE_TOKEN_BYTES: usize = 16 * 1024 * 1024;

/// Charged bytes of completed block plans each thread keeps across routes.
const MAX_THREAD_BLOCK_PLAN_BYTES: usize = 8 << 20;

/// Return the original block's bit range if it can be copied verbatim at
/// `alignment` (validity only, not cost). A stored block's padding depends on
/// its alignment.
// Out of line: called once per block plan, from many places.
#[inline(never)]
pub(crate) fn reusable_original_bits(
    block: &ParsedBlock,
    alignment: u8,
    strict: bool,
) -> Option<OriginalBits> {
    let original = block.original?;
    let alignment_is_usable =
        original.block_type != SourceBlockType::Stored || original.alignment == alignment;
    let huffman_alphabets_are_usable = !strict
        || original.block_type != SourceBlockType::Dynamic
        || block
            .original_dynamic
            .as_ref()
            .is_some_and(DynamicPlan::has_strictly_compatible_huffman_codes);
    (alignment_is_usable && huffman_alphabets_are_usable).then_some(original)
}

pub(crate) fn plan_block(
    block: &ParsedBlock,
    alignment: u8,
    options: &Options,
    stop: &mut SearchStop<'_>,
) -> PlannedBlock {
    let (representation, bits) = plan_representation(block, alignment, options, stop);

    PlannedBlock {
        tokens: block.tokens.clone(),
        plain: block.plain.clone(),
        representation,
        bits,
        source_type: block.source_type,
    }
}

/// Like [`plan_block`], but moves the block's token and decoded-byte buffers
/// into the plan instead of cloning their `Arc`s.
pub(crate) fn plan_owned_block(
    block: ParsedBlock,
    alignment: u8,
    options: &Options,
    stop: &mut SearchStop<'_>,
) -> PlannedBlock {
    let (representation, bits) = plan_representation(&block, alignment, options, stop);
    PlannedBlock {
        tokens: block.tokens,
        plain: block.plain,
        representation,
        bits,
        source_type: block.source_type,
    }
}

/// Cheapest fixed or dynamic representation of one token sequence.
///
/// Its cost does not depend on the block's bit alignment, so boundary search
/// computes it once and compares it with the stored and original-copy costs at
/// each alignment.
pub(crate) struct ReusableBlockPlan {
    representation: Representation,
    bits: u64,
}

impl ReusableBlockPlan {
    fn try_clone(&self) -> Option<Self> {
        Some(Self {
            representation: self.representation.try_clone()?,
            bits: self.bits,
        })
    }

    /// Return the cheapest bit count at `alignment` without cloning the plan.
    pub(crate) fn bits_at_alignment(
        &self,
        plain_len: usize,
        alignment: u8,
        original: Option<OriginalBits>,
    ) -> u64 {
        let mut bits = stored_block_bits(alignment, plain_len).min(self.bits);
        if let Some(original) = original {
            bits = bits.min(original.len);
        }
        bits
    }

    /// Select the cheapest representation at `alignment`, keeping `self` for
    /// reuse.
    ///
    /// If cloning the dynamic plan fails, fall back to the cheaper of stored
    /// and original copy so the boundary-search edge survives.
    pub(crate) fn at_alignment(
        &self,
        plain_len: usize,
        alignment: u8,
        original: Option<OriginalBits>,
    ) -> (Representation, u64) {
        let stored_bits = stored_block_bits(alignment, plain_len);
        let selected_bits = stored_bits.min(self.bits);
        if let Some(original) = original.filter(|original| original.len <= selected_bits) {
            return (Representation::Original(original), original.len);
        }
        if stored_bits <= self.bits {
            return (Representation::Stored, stored_bits);
        }
        if let Some(representation) = self.representation.try_clone() {
            return (representation, self.bits);
        }

        // The original copy wins a tie with stored.
        if let Some(original) = original.filter(|original| original.len <= stored_bits) {
            (Representation::Original(original), original.len)
        } else {
            (Representation::Stored, stored_bits)
        }
    }

    fn into_alignment(
        self,
        block: &ParsedBlock,
        alignment: u8,
        strict: bool,
    ) -> (Representation, u64) {
        select_aligned_representation(
            block.plain.len(),
            reusable_original_bits(block, alignment, strict),
            alignment,
            self.representation,
            self.bits,
        )
    }
}

pub(crate) fn plan_reusable_block(
    block: &ParsedBlock,
    options: &Options,
    stop: &mut SearchStop<'_>,
) -> ReusableBlockPlan {
    plan_reusable_block_with_header_cache(block, options, stop, &mut HeaderPlanCache::new())
}

/// Plan `block`, reusing this thread's copy of a plan that finished before its
/// stop. A call whose stop has already been reached plans without the store,
/// so it still returns the plan its deadline allows.
pub(crate) fn plan_reusable_block_with_header_cache(
    block: &ParsedBlock,
    options: &Options,
    stop: &mut SearchStop<'_>,
    header_cache: &mut HeaderPlanCache,
) -> ReusableBlockPlan {
    let key = (!stop.reached()).then(|| BlockPlanKey::new(block, options));
    if let Some(key) = &key {
        let stored = BLOCK_PLAN_STORE
            .try_with(|store| store.try_borrow_mut().ok()?.get(key, block))
            .ok()
            .flatten();
        if let Some(plan) = stored {
            return plan;
        }
    }
    let plan = plan_reusable_block_uncached(block, options, stop, header_cache);
    // Stops only move forward, so one that is not reached now was not reached
    // while planning: the plan is complete.
    if let Some(key) = key.filter(|_| !stop.reached()) {
        if let Some(entry) = StoredBlockPlan::pack(&key, block, &plan) {
            let _ = BLOCK_PLAN_STORE.try_with(|store| {
                if let Ok(mut store) = store.try_borrow_mut() {
                    store.insert(key.fingerprint, entry);
                }
            });
        }
    }
    plan
}

fn plan_reusable_block_uncached(
    block: &ParsedBlock,
    options: &Options,
    stop: &mut SearchStop<'_>,
    header_cache: &mut HeaderPlanCache,
) -> ReusableBlockPlan {
    let fixed_bits = fixed_block_bits(&block.tokens).unwrap_or(u64::MAX);
    let dynamic = best_dynamic_plan_cached(
        &block.tokens,
        &block.literal_frequencies,
        &block.distance_frequencies,
        block.original_dynamic.as_ref(),
        options.strict,
        options.exhaustive,
        stop,
        header_cache,
    );

    let (representation, bits) = if dynamic
        .as_ref()
        .map_or(true, |candidate| fixed_bits <= candidate.bits)
    {
        (Representation::Fixed, fixed_bits)
    } else {
        let dynamic = dynamic.expect("the dynamic branch requires a plan");
        let bits = dynamic.bits;
        (Representation::Dynamic(dynamic), bits)
    };

    ReusableBlockPlan {
        representation,
        bits,
    }
}

/// The inputs of a block plan besides its symbol counts. Planning reads the
/// tokens only through their symbol counts and extra bits, so two blocks with
/// equal counts, extra bits, source header and policy get the same plan in
/// any token order.
struct BlockPlanKey {
    fingerprint: u64,
    token_count: usize,
    extra_bits: u64,
    strict: bool,
    exhaustive: bool,
}

impl BlockPlanKey {
    fn new(block: &ParsedBlock, options: &Options) -> Self {
        let token_count = block.tokens.len();
        let extra_bits = token_extra_bits(&block.tokens);
        let mut fingerprint = 0xcbf2_9ce4_8422_2325_u64;
        for &count in block
            .literal_frequencies
            .iter()
            .chain(&block.distance_frequencies)
        {
            mix_plan_fingerprint(&mut fingerprint, u64::from(count));
        }
        mix_plan_fingerprint(&mut fingerprint, token_count as u64);
        mix_plan_fingerprint(&mut fingerprint, extra_bits);
        mix_plan_fingerprint(
            &mut fingerprint,
            u64::from(options.strict) | (u64::from(options.exhaustive) << 1),
        );
        if let Some(original) = &block.original_dynamic {
            mix_plan_fingerprint(&mut fingerprint, original.bits);
            for &length in original
                .literal_lengths
                .iter()
                .chain(&original.distance_lengths)
                .chain(&original.code_length_lengths)
            {
                mix_plan_fingerprint(&mut fingerprint, u64::from(length));
            }
            for token in &original.rle {
                mix_plan_fingerprint(
                    &mut fingerprint,
                    u64::from(token.symbol) | (u64::from(token.extra) << 8),
                );
            }
        }
        Self {
            fingerprint,
            token_count,
            extra_bits,
            strict: options.strict,
            exhaustive: options.exhaustive,
        }
    }
}

/// A completed block plan with its complete key: the used symbols as a bit
/// set and their counts in symbol order, and the source header packed.
struct StoredBlockPlan {
    token_count: usize,
    extra_bits: u64,
    strict: bool,
    exhaustive: bool,
    used_symbols: [u64; 5],
    counts: Box<[u32]>,
    original: Option<PackedDynamicPlan>,
    bits: u64,
    /// `None` for fixed codes.
    dynamic: Option<PackedDynamicPlan>,
}

fn used_symbols(block: &ParsedBlock) -> [u64; 5] {
    let mut used = [0_u64; 5];
    for (symbol, &count) in block
        .literal_frequencies
        .iter()
        .chain(&block.distance_frequencies)
        .enumerate()
    {
        if count != 0 {
            used[symbol / 64] |= 1 << (symbol % 64);
        }
    }
    used
}

impl StoredBlockPlan {
    fn pack(key: &BlockPlanKey, block: &ParsedBlock, plan: &ReusableBlockPlan) -> Option<Self> {
        let dynamic = match &plan.representation {
            Representation::Fixed => None,
            Representation::Dynamic(dynamic) => Some(PackedDynamicPlan::pack(dynamic)?),
            Representation::Original(_) | Representation::Stored => return None,
        };
        let original = match &block.original_dynamic {
            Some(original) => Some(PackedDynamicPlan::pack(original)?),
            None => None,
        };
        let mut counts = Vec::new();
        let frequencies = block
            .literal_frequencies
            .iter()
            .chain(&block.distance_frequencies);
        counts
            .try_reserve_exact(frequencies.clone().filter(|&&count| count != 0).count())
            .ok()?;
        counts.extend(frequencies.filter(|&&count| count != 0));
        Some(Self {
            token_count: key.token_count,
            extra_bits: key.extra_bits,
            strict: key.strict,
            exhaustive: key.exhaustive,
            used_symbols: used_symbols(block),
            counts: counts.into_boxed_slice(),
            original,
            bits: plan.bits,
            dynamic,
        })
    }

    fn matches(&self, key: &BlockPlanKey, block: &ParsedBlock) -> bool {
        self.token_count == key.token_count
            && self.extra_bits == key.extra_bits
            && self.strict == key.strict
            && self.exhaustive == key.exhaustive
            && self.used_symbols == used_symbols(block)
            && block
                .literal_frequencies
                .iter()
                .chain(&block.distance_frequencies)
                .filter(|&&count| count != 0)
                .eq(self.counts.iter())
            && match (&self.original, &block.original_dynamic) {
                (None, None) => true,
                (Some(stored), Some(original)) => stored.matches(original),
                _ => false,
            }
    }

    fn unpack(&self) -> Option<ReusableBlockPlan> {
        let representation = match &self.dynamic {
            None => Representation::Fixed,
            Some(dynamic) => Representation::Dynamic(dynamic.unpack()?),
        };
        Some(ReusableBlockPlan {
            representation,
            bits: self.bits,
        })
    }
}

impl Charged for StoredBlockPlan {
    /// A table slot with growth slack, and the count and plan allocations.
    fn charge(&self) -> usize {
        2 * std::mem::size_of::<(u64, Self)>()
            + (4 * self.counts.len() + 16).next_multiple_of(16)
            + self
                .original
                .as_ref()
                .map_or(0, PackedDynamicPlan::heap_charge)
            + self
                .dynamic
                .as_ref()
                .map_or(0, PackedDynamicPlan::heap_charge)
    }
}

/// Completed block plans shared by every route on one thread. Token-spelling
/// searches return to spellings they have planned, and the boundary graph and
/// structural ranges replan ranges; most of those calls run under a stop, so
/// [`CanonicalPlanCache`] does not keep them.
type BlockPlanStore = PlanStore<StoredBlockPlan>;

impl Default for BlockPlanStore {
    fn default() -> Self {
        Self::with_budget(MAX_THREAD_BLOCK_PLAN_BYTES)
    }
}

impl BlockPlanStore {
    fn get(&mut self, key: &BlockPlanKey, block: &ParsedBlock) -> Option<ReusableBlockPlan> {
        self.lookup(
            key.fingerprint,
            |entry| entry.matches(key, block),
            StoredBlockPlan::unpack,
        )
    }
}

thread_local! {
    static BLOCK_PLAN_STORE: RefCell<BlockPlanStore> = RefCell::new(BlockPlanStore::default());
}

/// Drop this thread's stored block plans. A top-level optimization calls this
/// on return, so library callers keep no plans between inputs; worker threads
/// free theirs when they exit.
pub(crate) fn release_thread_block_plans() {
    let _ = BLOCK_PLAN_STORE.try_with(|store| {
        if let Ok(mut store) = store.try_borrow_mut() {
            *store = BlockPlanStore::default();
        }
    });
}

/// Counters for [`CanonicalPlanCache`], read by tests to confirm real hits.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct CanonicalPlanCacheStats {
    pub(crate) lookups: usize,
    pub(crate) hits: usize,
    pub(crate) misses: usize,
    pub(crate) inserts: usize,
    pub(crate) collision_checks: usize,
    pub(crate) saturated: usize,
    pub(crate) retained_token_bytes: usize,
}

struct CachedReusablePlan {
    fingerprint: u64,
    next_same_hash: Option<usize>,
    tokens: Arc<Vec<Token>>,
    literal_frequencies: [u32; 286],
    distance_frequencies: [u32; 30],
    original_dynamic: Option<DynamicPlan>,
    strict: bool,
    exhaustive: bool,
    plan: ReusableBlockPlan,
}

/// Completed [`ReusableBlockPlan`]s shared within one planning run. Callers
/// under a deadline may look up entries, but only
/// [`Self::plan_reusable_complete`], which plans without a stop, inserts them.
pub(crate) struct CanonicalPlanCache {
    first_by_hash: HashMap<u64, usize>,
    entries: Vec<CachedReusablePlan>,
    stats: CanonicalPlanCacheStats,
    max_entries: usize,
    max_token_bytes: usize,
    header_cache: HeaderPlanCache,
}

impl Default for CanonicalPlanCache {
    fn default() -> Self {
        Self::new()
    }
}

impl CanonicalPlanCache {
    pub(crate) fn new() -> Self {
        Self {
            first_by_hash: HashMap::new(),
            entries: Vec::new(),
            stats: CanonicalPlanCacheStats::default(),
            max_entries: MAX_CANONICAL_PLAN_CACHE_ENTRIES,
            max_token_bytes: MAX_CANONICAL_PLAN_CACHE_TOKEN_BYTES,
            header_cache: HeaderPlanCache::new(),
        }
    }

    pub(crate) fn lookup_reusable(
        &mut self,
        block: &ParsedBlock,
        options: &Options,
    ) -> Option<ReusableBlockPlan> {
        let fingerprint = canonical_plan_fingerprint(block, options);
        self.lookup_reusable_with_fingerprint(block, options, fingerprint)
    }

    fn lookup_reusable_with_fingerprint(
        &mut self,
        block: &ParsedBlock,
        options: &Options,
        fingerprint: u64,
    ) -> Option<ReusableBlockPlan> {
        self.stats.lookups = self.stats.lookups.saturating_add(1);
        let mut candidate = self.first_by_hash.get(&fingerprint).copied();
        while let Some(index) = candidate {
            self.stats.collision_checks = self.stats.collision_checks.saturating_add(1);
            let entry = self.entries.get(index)?;
            let next = entry.next_same_hash;
            let matches = entry.fingerprint == fingerprint
                && entry.strict == options.strict
                && entry.exhaustive == options.exhaustive
                && entry.literal_frequencies == block.literal_frequencies
                && entry.distance_frequencies == block.distance_frequencies
                && entry.original_dynamic.as_ref() == block.original_dynamic.as_ref()
                && (Arc::ptr_eq(&entry.tokens, &block.tokens)
                    || same_tokens(&entry.tokens, &block.tokens));
            if matches {
                if let Some(plan) = entry.plan.try_clone() {
                    self.stats.hits = self.stats.hits.saturating_add(1);
                    return Some(plan);
                }
                break;
            }
            candidate = next;
        }
        self.stats.misses = self.stats.misses.saturating_add(1);
        None
    }

    /// Return the cached plan, or plan without a stop and cache the result.
    pub(crate) fn plan_reusable_complete(
        &mut self,
        block: &ParsedBlock,
        options: &Options,
    ) -> ReusableBlockPlan {
        let fingerprint = canonical_plan_fingerprint(block, options);
        if let Some(plan) = self.lookup_reusable_with_fingerprint(block, options, fingerprint) {
            return plan;
        }
        let plan = plan_reusable_block_with_header_cache(
            block,
            options,
            &mut SearchStop::never(),
            &mut self.header_cache,
        );
        self.insert(block, options, fingerprint, &plan);
        plan
    }

    fn insert(
        &mut self,
        block: &ParsedBlock,
        options: &Options,
        fingerprint: u64,
        plan: &ReusableBlockPlan,
    ) {
        let Some(token_bytes) = block
            .tokens
            .capacity()
            .checked_mul(std::mem::size_of::<Token>())
        else {
            self.stats.saturated = self.stats.saturated.saturating_add(1);
            return;
        };
        let Some(retained_token_bytes) = self.stats.retained_token_bytes.checked_add(token_bytes)
        else {
            self.stats.saturated = self.stats.saturated.saturating_add(1);
            return;
        };
        if self.entries.len() >= self.max_entries || retained_token_bytes > self.max_token_bytes {
            self.stats.saturated = self.stats.saturated.saturating_add(1);
            return;
        }

        let original_dynamic = match block.original_dynamic.as_ref() {
            Some(dynamic) => match dynamic.try_clone() {
                Some(dynamic) => Some(dynamic),
                None => return,
            },
            None => None,
        };
        let Some(cached_plan) = plan.try_clone() else {
            return;
        };
        if self.entries.try_reserve(1).is_err() || self.first_by_hash.try_reserve(1).is_err() {
            return;
        }

        let next_same_hash = self.first_by_hash.get(&fingerprint).copied();
        let index = self.entries.len();
        self.entries.push(CachedReusablePlan {
            fingerprint,
            next_same_hash,
            tokens: Arc::clone(&block.tokens),
            literal_frequencies: block.literal_frequencies,
            distance_frequencies: block.distance_frequencies,
            original_dynamic,
            strict: options.strict,
            exhaustive: options.exhaustive,
            plan: cached_plan,
        });
        self.first_by_hash.insert(fingerprint, index);
        self.stats.inserts = self.stats.inserts.saturating_add(1);
        self.stats.retained_token_bytes = retained_token_bytes;
    }
}

/// FNV-1a-style hash of the tokens and options that only selects a collision
/// chain; lookups compare every field, so a collision cannot cause a false hit.
fn canonical_plan_fingerprint(block: &ParsedBlock, options: &Options) -> u64 {
    let mut fingerprint = 0xcbf2_9ce4_8422_2325_u64;
    mix_plan_fingerprint(&mut fingerprint, block.tokens.len() as u64);
    for &token in block.tokens.iter() {
        match token {
            Token::Literal(value) => {
                mix_plan_fingerprint(&mut fingerprint, u64::from(value));
            }
            Token::Match {
                length,
                distance,
                length_symbol,
                distance_symbol,
                length_extra,
                distance_extra,
                length_extra_bits,
                distance_extra_bits,
            } => {
                mix_plan_fingerprint(
                    &mut fingerprint,
                    1_u64 << 63
                        | u64::from(length)
                        | (u64::from(distance) << 16)
                        | (u64::from(length_symbol) << 32)
                        | (u64::from(distance_symbol) << 48)
                        | (u64::from(length_extra_bits) << 56),
                );
                mix_plan_fingerprint(
                    &mut fingerprint,
                    u64::from(length_extra)
                        | (u64::from(distance_extra) << 16)
                        | (u64::from(distance_extra_bits) << 32),
                );
            }
        }
    }
    mix_plan_fingerprint(
        &mut fingerprint,
        u64::from(options.strict) | (u64::from(options.exhaustive) << 1),
    );
    fingerprint
}

#[inline]
fn mix_plan_fingerprint(fingerprint: &mut u64, value: u64) {
    *fingerprint ^= value;
    *fingerprint = fingerprint.wrapping_mul(0x0000_0100_0000_01b3);
    *fingerprint ^= *fingerprint >> 32;
}

pub(crate) fn plan_block_cached(
    block: &ParsedBlock,
    alignment: u8,
    options: &Options,
    cache: &mut CanonicalPlanCache,
) -> PlannedBlock {
    let (representation, bits) = cache.plan_reusable_complete(block, options).into_alignment(
        block,
        alignment,
        options.strict,
    );
    PlannedBlock {
        tokens: Arc::clone(&block.tokens),
        plain: Arc::clone(&block.plain),
        representation,
        bits,
        source_type: block.source_type,
    }
}

pub(crate) fn lookup_block_cached(
    block: &ParsedBlock,
    alignment: u8,
    options: &Options,
    cache: &mut CanonicalPlanCache,
) -> Option<PlannedBlock> {
    let (representation, bits) =
        cache
            .lookup_reusable(block, options)?
            .into_alignment(block, alignment, options.strict);
    Some(PlannedBlock {
        tokens: Arc::clone(&block.tokens),
        plain: Arc::clone(&block.plain),
        representation,
        bits,
        source_type: block.source_type,
    })
}

fn plan_representation(
    block: &ParsedBlock,
    alignment: u8,
    options: &Options,
    stop: &mut SearchStop<'_>,
) -> (Representation, u64) {
    plan_reusable_block(block, options, stop).into_alignment(block, alignment, options.strict)
}

fn select_aligned_representation(
    plain_len: usize,
    original: Option<OriginalBits>,
    alignment: u8,
    huffman_representation: Representation,
    huffman_bits: u64,
) -> (Representation, u64) {
    let stored_bits = stored_block_bits(alignment, plain_len);
    // Tie order: stored, fixed, then dynamic (as in Columbo's C
    // implementation). A reusable original block wins any tie.
    let (mut representation, mut bits) = if stored_bits <= huffman_bits {
        (Representation::Stored, stored_bits)
    } else {
        (huffman_representation, huffman_bits)
    };

    if let Some(original) = original {
        if original.len <= bits {
            representation = Representation::Original(original);
            bits = original.len;
        }
    }

    (representation, bits)
}

pub(crate) fn fixed_block_bits(tokens: &[Token]) -> Option<u64> {
    token_bits(
        tokens,
        &FIXED_LITERAL_CODE_LENGTHS,
        &FIXED_DISTANCE_CODE_LENGTHS,
    )?
    .checked_add(3)
}

pub(crate) fn stored_block_bits(mut alignment: u8, plain_size: usize) -> u64 {
    let mut remaining = plain_size;
    let mut bits = 0_u64;
    loop {
        let chunk = remaining.min(65_535);
        let after_header = (alignment + 3) & 7;
        let padding = if after_header == 0 {
            0
        } else {
            8 - after_header
        };
        bits += 3 + u64::from(padding) + 32 + (chunk as u64) * 8;
        remaining -= chunk;
        if remaining == 0 {
            break;
        }
        alignment = 0;
    }
    bits
}

pub(crate) fn emit_block(
    writer: &mut BitWriter,
    input: &[u8],
    block: &PlannedBlock,
    final_block: bool,
) -> Result<()> {
    match &block.representation {
        Representation::Original(original) => {
            writer.write(u32::from(final_block), 1)?;
            // Only BFINAL depends on the new stream layout; copy the rest
            // verbatim.
            let copied_start = original
                .start
                .checked_add(1)
                .ok_or_else(|| Error::new("original Deflate bit range is out of bounds"))?;
            let copied_bits = original
                .len
                .checked_sub(1)
                .ok_or_else(|| Error::new("original Deflate bit range is empty"))?;
            writer.write_bits_from(input, copied_start, copied_bits)
        }
        Representation::Stored => emit_stored(writer, final_block, &block.plain),
        Representation::Fixed => emit_fixed(writer, final_block, &block.tokens),
        Representation::Dynamic(dynamic) => {
            emit_dynamic(writer, final_block, &block.tokens, dynamic)
        }
    }
}

fn emit_stored(writer: &mut BitWriter, final_block: bool, plain: &[u8]) -> Result<()> {
    let mut offset = 0;
    loop {
        let chunk = (plain.len() - offset).min(65_535);
        let is_last = offset + chunk == plain.len();
        writer.write(u32::from(final_block && is_last), 1)?;
        writer.write(0, 2)?;
        writer.align_to_byte()?;
        let length = chunk as u16;
        writer.write(u32::from(length), 16)?;
        writer.write(u32::from(!length), 16)?;
        writer.write_aligned_bytes(&plain[offset..offset + chunk])?;
        offset += chunk;
        if is_last {
            break;
        }
    }
    Ok(())
}

fn emit_fixed(writer: &mut BitWriter, final_block: bool, tokens: &[Token]) -> Result<()> {
    let (literal, distance) = fixed_trees();
    writer.write(u32::from(final_block), 1)?;
    writer.write(1, 2)?;
    emit_tokens(writer, tokens, literal, distance)
}

fn emit_dynamic(
    writer: &mut BitWriter,
    final_block: bool,
    tokens: &[Token],
    plan: &DynamicPlan,
) -> Result<()> {
    let literal = Huffman::build(&plan.literal_lengths)
        .ok_or_else(|| Error::internal("internal invalid literal/length plan"))?;
    let distance = Huffman::build(&plan.distance_lengths)
        .ok_or_else(|| Error::internal("internal invalid distance plan"))?;
    let code_length = Huffman::build(&plan.code_length_lengths)
        .ok_or_else(|| Error::internal("internal invalid code-length plan"))?;

    writer.write(u32::from(final_block), 1)?;
    writer.write(2, 2)?;
    writer.write((plan.hlit - 257) as u32, 5)?;
    writer.write((plan.hdist - 1) as u32, 5)?;
    writer.write((plan.hclen - 4) as u32, 4)?;
    for &symbol in &CODE_LENGTH_ORDER[..plan.hclen] {
        writer.write(u32::from(plan.code_length_lengths[symbol]), 3)?;
    }
    for rle in &plan.rle {
        emit_symbol(writer, &code_length, usize::from(rle.symbol))?;
        let extra_bits = match rle.symbol {
            16 => 2,
            17 => 3,
            18 => 7,
            _ => 0,
        };
        writer.write(u32::from(rle.extra), extra_bits)?;
    }
    emit_tokens(writer, tokens, &literal, &distance)
}

fn emit_tokens(
    writer: &mut BitWriter,
    tokens: &[Token],
    literal: &Huffman,
    distance: &Huffman,
) -> Result<()> {
    for token in tokens {
        match *token {
            Token::Literal(value) => emit_symbol(writer, literal, usize::from(value))?,
            Token::Match {
                length_symbol,
                distance_symbol,
                length_extra,
                distance_extra,
                length_extra_bits,
                distance_extra_bits,
                ..
            } => {
                emit_symbol_with_extra(
                    writer,
                    literal,
                    usize::from(length_symbol),
                    length_extra,
                    length_extra_bits,
                )?;
                emit_symbol_with_extra(
                    writer,
                    distance,
                    usize::from(distance_symbol),
                    distance_extra,
                    distance_extra_bits,
                )?;
            }
        }
    }
    emit_symbol(writer, literal, 256)
}

fn emit_symbol(writer: &mut BitWriter, tree: &Huffman, symbol: usize) -> Result<()> {
    let code = tree
        .code(symbol)
        .ok_or_else(|| Error::internal("internal Huffman plan does not cover token"))?;
    writer.write(u32::from(code.code), code.length)
}

fn emit_symbol_with_extra(
    writer: &mut BitWriter,
    tree: &Huffman,
    symbol: usize,
    extra: u16,
    extra_bits: u8,
) -> Result<()> {
    let code = tree
        .code(symbol)
        .ok_or_else(|| Error::internal("internal Huffman plan does not cover token"))?;
    let bits = code.length + extra_bits;
    debug_assert!(bits <= 32);
    let packed = u32::from(code.code) | (u32::from(extra) << code.length);
    writer.write(packed, bits)
}

#[cfg(test)]
mod test_support;
#[cfg(test)]
mod tests;
