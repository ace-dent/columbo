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

    /// Return the cheapest aligned bit count without cloning the representation.
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

fn plan_reusable_block_with_header_cache(
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

    #[cfg(test)]
    pub(crate) fn with_limits(max_entries: usize, max_token_bytes: usize) -> Self {
        Self {
            max_entries,
            max_token_bytes,
            ..Self::new()
        }
    }

    #[cfg(test)]
    pub(crate) fn stats(&self) -> CanonicalPlanCacheStats {
        self.stats
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
        .ok_or_else(|| Error::new("internal invalid literal/length plan"))?;
    let distance = Huffman::build(&plan.distance_lengths)
        .ok_or_else(|| Error::new("internal invalid distance plan"))?;
    let code_length = Huffman::build(&plan.code_length_lengths)
        .ok_or_else(|| Error::new("internal invalid code-length plan"))?;

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
                emit_symbol(writer, literal, usize::from(length_symbol))?;
                writer.write(u32::from(length_extra), length_extra_bits)?;
                emit_symbol(writer, distance, usize::from(distance_symbol))?;
                writer.write(u32::from(distance_extra), distance_extra_bits)?;
            }
        }
    }
    emit_symbol(writer, literal, 256)
}

fn emit_symbol(writer: &mut BitWriter, tree: &Huffman, symbol: usize) -> Result<()> {
    let code = tree
        .code(symbol)
        .ok_or_else(|| Error::new("internal Huffman plan does not cover token"))?;
    writer.write(u32::from(code.code), code.length)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::deflate::model::count_frequencies;

    fn literal_block(bytes: &[u8]) -> ParsedBlock {
        let tokens: Vec<_> = bytes.iter().copied().map(Token::Literal).collect();
        let (literal_frequencies, distance_frequencies) = count_frequencies(&tokens);
        ParsedBlock {
            tokens: Arc::new(tokens),
            plain: Arc::new(bytes.to_vec()),
            literal_frequencies,
            distance_frequencies,
            original_literal_lengths: None,
            original_distance_lengths: None,
            original_dynamic: None,
            original: None,
            source_splits: Vec::new(),
            source_type: SourceBlockType::Dynamic,
        }
    }

    fn assert_same_plan(left: &PlannedBlock, right: &PlannedBlock) {
        assert_eq!(left.bits, right.bits);
        assert_eq!(left.tokens, right.tokens);
        assert_eq!(left.plain, right.plain);
        assert_eq!(left.source_type, right.source_type);
        match (&left.representation, &right.representation) {
            (Representation::Original(left), Representation::Original(right)) => {
                assert_eq!(left, right);
            }
            (Representation::Stored, Representation::Stored)
            | (Representation::Fixed, Representation::Fixed) => {}
            (Representation::Dynamic(left), Representation::Dynamic(right)) => {
                assert_eq!(left, right);
            }
            pair => panic!("different representations: {pair:?}"),
        }
    }

    fn block_with_original(
        block_type: SourceBlockType,
        alignment: u8,
        distance_lengths: Option<Vec<u8>>,
    ) -> ParsedBlock {
        let mut literal_lengths = vec![0; 257];
        literal_lengths[0] = 1;
        literal_lengths[256] = 1;
        let mut code_length_lengths = [0; 19];
        code_length_lengths[0] = 1;
        code_length_lengths[1] = 1;
        ParsedBlock {
            tokens: std::sync::Arc::new(Vec::new()),
            plain: std::sync::Arc::new(Vec::new()),
            literal_frequencies: [0; 286],
            distance_frequencies: [0; 30],
            original_literal_lengths: None,
            original_distance_lengths: None,
            original_dynamic: distance_lengths.map(|distance_lengths| DynamicPlan {
                literal_lengths,
                distance_lengths,
                code_length_lengths,
                rle: Vec::new(),
                hlit: 0,
                hdist: 0,
                hclen: 0,
                bits: 0,
            }),
            original: Some(OriginalBits {
                start: 0,
                len: 10,
                alignment,
                block_type,
            }),
            source_splits: Vec::new(),
            source_type: block_type,
        }
    }

    #[test]
    fn original_reuse_enforces_alignment_and_distance_policy() {
        let stored = block_with_original(SourceBlockType::Stored, 3, None);
        assert!(reusable_original_bits(&stored, 3, false).is_some());
        assert!(reusable_original_bits(&stored, 2, false).is_none());

        let fixed = block_with_original(SourceBlockType::Fixed, 3, None);
        assert!(reusable_original_bits(&fixed, 7, true).is_some());

        let no_dynamic_header = block_with_original(SourceBlockType::Dynamic, 0, None);
        assert!(reusable_original_bits(&no_dynamic_header, 0, false).is_some());
        assert!(reusable_original_bits(&no_dynamic_header, 0, true).is_none());

        let one_code = block_with_original(SourceBlockType::Dynamic, 0, Some(vec![1]));
        assert!(reusable_original_bits(&one_code, 0, true).is_none());
        let two_codes = block_with_original(SourceBlockType::Dynamic, 0, Some(vec![1, 1]));
        assert!(reusable_original_bits(&two_codes, 0, true).is_some());

        let mut singleton_literal =
            block_with_original(SourceBlockType::Dynamic, 0, Some(vec![1, 1]));
        let dynamic = singleton_literal.original_dynamic.as_mut().unwrap();
        dynamic.literal_lengths.fill(0);
        dynamic.literal_lengths[256] = 1;
        assert!(reusable_original_bits(&singleton_literal, 0, false).is_some());
        assert!(reusable_original_bits(&singleton_literal, 0, true).is_none());

        let mut reserved_only = vec![0; 32];
        reserved_only[30] = 1;
        reserved_only[31] = 1;
        let reserved_only = block_with_original(SourceBlockType::Dynamic, 0, Some(reserved_only));
        assert!(reusable_original_bits(&reserved_only, 0, true).is_none());
    }

    #[test]
    fn stored_cost_includes_alignment_and_chunks() {
        assert_eq!(stored_block_bits(0, 0), 40);
        assert_eq!(stored_block_bits(5, 0), 35);
        assert_eq!(
            stored_block_bits(0, 65_536),
            3 + 5 + 32 + 65_535 * 8 + 3 + 5 + 32 + 8
        );
    }

    #[test]
    fn reusable_huffman_plan_still_selects_stored_per_alignment() {
        let mut block = block_with_original(SourceBlockType::Dynamic, 0, None);
        block.plain = std::sync::Arc::new(vec![0; 26]);

        // A 26-byte stored block costs 243 bits with no padding and 250 bits
        // with seven padding bits. The reusable Huffman kernel must therefore
        // remain a candidate rather than blindly reusing one aligned winner.
        let (no_padding, no_padding_bits) = select_aligned_representation(
            block.plain.len(),
            reusable_original_bits(&block, 5, true),
            5,
            Representation::Fixed,
            244,
        );
        assert!(matches!(no_padding, Representation::Stored));
        assert_eq!(no_padding_bits, 243);

        let (seven_padding, seven_padding_bits) = select_aligned_representation(
            block.plain.len(),
            reusable_original_bits(&block, 6, true),
            6,
            Representation::Fixed,
            244,
        );
        assert!(matches!(seven_padding, Representation::Fixed));
        assert_eq!(seven_padding_bits, 244);
    }

    #[test]
    fn canonical_cache_hits_for_equal_tokens_with_distinct_arcs() {
        let first = literal_block(b"canonical interval");
        let mut second = first.try_clone_shared().unwrap();
        second.tokens = Arc::new(first.tokens.as_ref().clone());
        second.plain = Arc::new(first.plain.as_ref().clone());
        assert!(!Arc::ptr_eq(&first.tokens, &second.tokens));

        let options = Options::default();
        let mut cache = CanonicalPlanCache::new();
        let expected = plan_block(&first, 3, &options, &mut SearchStop::never());
        let first_cached = plan_block_cached(&first, 3, &options, &mut cache);
        let second_cached = plan_block_cached(&second, 3, &options, &mut cache);

        assert_same_plan(&first_cached, &expected);
        assert_same_plan(&second_cached, &expected);
        assert_eq!(
            cache.stats(),
            CanonicalPlanCacheStats {
                lookups: 2,
                hits: 1,
                misses: 1,
                inserts: 1,
                collision_checks: 1,
                saturated: 0,
                retained_token_bytes: first.tokens.capacity() * std::mem::size_of::<Token>(),
            }
        );
    }

    #[test]
    fn header_cache_reuses_trees_across_distinct_token_orders() {
        let first = literal_block(b"header kernel reuse");
        let mut reversed = b"header kernel reuse".to_vec();
        reversed.reverse();
        let second = literal_block(&reversed);
        assert_eq!(first.literal_frequencies, second.literal_frequencies);
        assert_ne!(first.tokens, second.tokens);

        let options = Options::default();
        let mut cache = CanonicalPlanCache::new();
        cache.plan_reusable_complete(&first, &options);
        let first_header_stats = cache.header_cache.stats();
        cache.plan_reusable_complete(&second, &options);
        let second_header_stats = cache.header_cache.stats();

        assert_eq!(cache.stats().hits, 0);
        assert_eq!(cache.stats().misses, 2);
        assert!(second_header_stats.hits > first_header_stats.hits);
        assert!(second_header_stats.inserts >= first_header_stats.inserts);
    }

    #[test]
    fn canonical_cache_verifies_exact_state_after_a_hash_collision() {
        let first = literal_block(b"first collision state");
        let second = literal_block(b"second collision state");
        let options = Options::default();
        let mut cache = CanonicalPlanCache::new();
        cache.plan_reusable_complete(&first, &options);

        // Force the first entry into the second state's bucket. Production
        // hashes are only accelerators, so an exact-state mismatch must still
        // miss and append a separate entry to the collision chain.
        let collision = canonical_plan_fingerprint(&second, &options);
        cache.entries[0].fingerprint = collision;
        cache.first_by_hash.clear();
        cache.first_by_hash.insert(collision, 0);

        let expected = plan_block(&second, 5, &options, &mut SearchStop::never());
        let actual = plan_block_cached(&second, 5, &options, &mut cache);
        assert_same_plan(&actual, &expected);
        assert_eq!(cache.entries.len(), 2);
        assert_eq!(cache.entries[1].next_same_hash, Some(0));
        let stats = cache.stats();
        assert_eq!(stats.hits, 0);
        assert_eq!(stats.misses, 2);
        assert_eq!(stats.inserts, 2);
        assert_eq!(stats.collision_checks, 1);
    }

    #[test]
    fn canonical_cache_isolates_policy_and_source_tree_seed() {
        let block = literal_block(b"policy");
        let relaxed = Options {
            strict: false,
            ..Options::default()
        };
        let exhaustive = Options {
            exhaustive: true,
            ..Options::default()
        };

        let mut cache = CanonicalPlanCache::new();
        cache.plan_reusable_complete(&block, &Options::default());
        cache.plan_reusable_complete(&block, &relaxed);
        cache.plan_reusable_complete(&block, &exhaustive);

        let mut seeded = block.try_clone_shared().unwrap();
        seeded.original_dynamic =
            block_with_original(SourceBlockType::Dynamic, 0, Some(vec![1, 1])).original_dynamic;
        cache.plan_reusable_complete(&seeded, &Options::default());

        let stats = cache.stats();
        assert_eq!(stats.hits, 0);
        assert_eq!(stats.misses, 4);
        assert_eq!(stats.inserts, 4);
    }

    #[test]
    fn cached_kernel_layers_current_original_and_alignment() {
        let mut first = literal_block(b"same generated payload");
        first.source_type = SourceBlockType::Fixed;
        first.original = Some(OriginalBits {
            start: 11,
            len: 1,
            alignment: 0,
            block_type: SourceBlockType::Fixed,
        });
        let mut second = first.try_clone_shared().unwrap();
        second.source_type = SourceBlockType::Dynamic;
        second.original = Some(OriginalBits {
            start: 97,
            len: 2,
            alignment: 7,
            block_type: SourceBlockType::Fixed,
        });

        let options = Options {
            strict: false,
            ..Options::default()
        };
        let mut cache = CanonicalPlanCache::new();
        let first_plan = plan_block_cached(&first, 0, &options, &mut cache);
        let second_plan = plan_block_cached(&second, 5, &options, &mut cache);
        assert!(matches!(
            first_plan.representation,
            Representation::Original(OriginalBits { start: 11, .. })
        ));
        assert!(matches!(
            second_plan.representation,
            Representation::Original(OriginalBits { start: 97, .. })
        ));
        assert_eq!(second_plan.source_type, SourceBlockType::Dynamic);
        assert_eq!(cache.stats().hits, 1);

        let mut stored = second;
        stored.source_type = SourceBlockType::Stored;
        stored.original = Some(OriginalBits {
            start: 123,
            len: 1,
            alignment: 3,
            block_type: SourceBlockType::Stored,
        });
        let aligned = plan_block_cached(&stored, 3, &options, &mut cache);
        let unaligned = plan_block_cached(&stored, 2, &options, &mut cache);
        assert!(matches!(
            aligned.representation,
            Representation::Original(_)
        ));
        assert!(!matches!(
            unaligned.representation,
            Representation::Original(_)
        ));
    }

    #[test]
    fn cache_saturation_recomputes_without_changing_the_plan() {
        let block = literal_block(b"not retained");
        let options = Options::default();
        let mut cache = CanonicalPlanCache::with_limits(1, 0);

        let expected = plan_block(&block, 4, &options, &mut SearchStop::never());
        let first = plan_block_cached(&block, 4, &options, &mut cache);
        let second = plan_block_cached(&block, 4, &options, &mut cache);
        assert_same_plan(&first, &expected);
        assert_same_plan(&second, &expected);

        let stats = cache.stats();
        assert_eq!(stats.hits, 0);
        assert_eq!(stats.misses, 2);
        assert_eq!(stats.inserts, 0);
        assert_eq!(stats.saturated, 2);
        assert_eq!(stats.retained_token_bytes, 0);
    }

    #[test]
    fn empty_fixed_block_has_only_a_header_and_end_code() {
        assert_eq!(fixed_block_bits(&[]), Some(10));
    }
}
