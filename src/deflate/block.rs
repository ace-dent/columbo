// SPDX-License-Identifier: MIT

//! Planning and emission for one structural Deflate block.

use std::collections::HashMap;
use std::sync::Arc;

use crate::{Error, Options, Result};

use super::bitstream::BitWriter;
use super::header::{best_dynamic_plan_cached, token_bits, HeaderPlanCache};
use super::huffman::{
    fixed_trees, Huffman, FIXED_DISTANCE_CODE_LENGTHS, FIXED_LITERAL_CODE_LENGTHS,
};
use super::model::{
    DynamicPlan, OriginalBits, ParsedBlock, PlannedBlock, Representation, SourceBlockType, Token,
    CODE_LENGTH_ORDER,
};
use super::stop::SearchStop;

/// Route-local ceiling for completed canonical Huffman kernels.
///
/// Token vectors are reference-counted rather than copied, but retaining a
/// transformed route solely as a cache key can otherwise extend its lifetime
/// indefinitely. Both limits are conservative charges: repeated `Arc`s count
/// their full token bytes again, keeping peak memory bounded without a second
/// allocation-identity table.
const MAX_CANONICAL_PLAN_CACHE_ENTRIES: usize = 512;
const MAX_CANONICAL_PLAN_CACHE_TOKEN_BYTES: usize = 16 * 1024 * 1024;

/// Return original block bits that remain safe at the requested alignment.
///
/// This answers only wire-format and compatibility questions, not whether the
/// original is the cheapest representation. Stored blocks contain alignment
/// padding, while strict dynamic originals must use complete Huffman codes.
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

/// Price an owned candidate without cloning its potentially large token and
/// decoded-byte vectors into the returned plan.
///
/// Structural search builds those vectors with fallible allocation. Moving
/// them here preserves that safety boundary; the ordinary borrowed planner
/// above remains convenient for persistent parsed blocks.
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

/// Best generated fixed or dynamic representation for one token set.
///
/// Huffman payload and header costs do not depend on the block's starting bit
/// alignment. Stream boundary search can therefore build this comparatively
/// expensive part once, then compare it with the eight cheap stored-padding
/// and exact-source possibilities. Exact source ranges deliberately remain
/// outside this kernel because they refer to one particular compressed input.
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

    /// Return the cheapest aligned bit count without cloning the block.
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

    /// Select one aligned representation while retaining this reusable kernel.
    ///
    /// A dynamic-table clone is optional work. If allocation fails, retain the
    /// cheapest allocation-free stored or exact-original representation rather
    /// than discarding the complete boundary-DP edge.
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

        // The reusable dynamic table could not be copied. Exact source bits
        // remain preferable to stored bytes on an equal-bit fallback.
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

/// Price the alignment-independent representation kernel for one block.
pub(crate) fn plan_reusable_block(
    block: &ParsedBlock,
    options: &Options,
    stop: &mut SearchStop<'_>,
) -> ReusableBlockPlan {
    plan_reusable_block_with_header_cache(block, options, stop, &mut HeaderPlanCache::new())
}

pub(crate) fn plan_reusable_block_with_header_cache(
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

/// Observability for the route-local canonical plan cache.
///
/// These counters are intentionally internal. They let route tests and future
/// verbose reporting distinguish genuine sharing from a cache that merely
/// preserves output while never finding an identical state.
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

/// Completed canonical fixed/dynamic/header work shared by one planning run.
///
/// Hashes only select a short collision chain. Every hit verifies the complete
/// token spelling, frequencies, source-tree seed and planning policy before a
/// cached kernel is reused. Deadline-sensitive callers may look up an earlier
/// completed entry, but only the explicit complete-planning API inserts.
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

    /// Return an exactly matching completed kernel without starting new work.
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
                    || entry.tokens.as_slice() == block.tokens.as_slice());
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

    /// Complete deterministic planning and retain its reusable kernel.
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

fn canonical_plan_fingerprint(block: &ParsedBlock, options: &Options) -> u64 {
    // This is a bounded cache-bucket accelerator, never an identity proof.
    // Exact token, frequency, source-tree, and policy comparisons follow every
    // bucket hit. Hash the canonical token state directly instead of running a
    // cryptographic-strength SipHash over the tokens and then over both derived
    // frequency arrays. Omitting the source-tree seed may lengthen a rare
    // collision chain, but cannot produce a false cache hit.
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

/// Plan one block through a completed route-local canonical kernel.
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

/// Instantiate a cached kernel if a prior deterministic route completed it.
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
    // Rewritten candidates intentionally use the original Columbo C tie order:
    // stored before fixed before dynamic. A usable exact-original candidate is
    // considered afterward and wins an equal-bit tie, avoiding pointless churn.
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
            // BFINAL is the only bit whose meaning depends on the new stream
            // layout. Everything after it can be copied verbatim.
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
