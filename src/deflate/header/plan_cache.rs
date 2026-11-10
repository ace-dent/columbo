// SPDX-License-Identifier: MIT
// SPDX-FileCopyrightText: 2026 Andrew C.E. Dent <https://github.com/ace-dent>

//! Reuse of completed header plans. A header plan's cost depends only on the
//! trimmed code lengths and the search policy, so a plan priced with zero
//! payload bits serves any block with those code lengths. A route-local
//! [`HeaderPlanCache`] keeps plans for one owner, and each thread's
//! [`HeaderPlanStore`] keeps them compactly for every route on the thread.
//! [`PackedDynamicPlan`] packs a complete plan into one allocation.

use std::cell::RefCell;
use std::collections::HashMap;
use std::hash::{BuildHasherDefault, Hasher};

use super::{plan_for_trimmed_lengths_uncached, plan_for_valid_trimmed_lengths_uncached};
use crate::deflate::model::{DynamicPlan, RleToken};

const MAX_HEADER_PLAN_CACHE_ENTRIES: usize = 512;

/// A table keyed by [`header_plan_fingerprint`], already a mixed 64-bit hash,
/// which the table uses as its own instead of hashing it again: the
/// per-thread store looks one up for every header plan.
type FingerprintMap<V> = HashMap<u64, V, BuildHasherDefault<FingerprintHasher>>;

#[derive(Default)]
struct FingerprintHasher(u64);

impl Hasher for FingerprintHasher {
    fn finish(&self) -> u64 {
        self.0
    }

    fn write(&mut self, bytes: &[u8]) {
        // A `u64` key writes itself whole; fold in anything else.
        for &byte in bytes {
            self.0 = (self.0.rotate_left(8) ^ u64::from(byte)).wrapping_mul(0x9e37_79b9_7f4a_7c15);
        }
    }

    fn write_u64(&mut self, value: u64) {
        self.0 = value;
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
    pub(super) stats: HeaderPlanCacheStats,
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

    /// The plan for these trimmed lengths, whose payload trees the caller has
    /// checked for a valid shape, with `data_bits` added.
    pub(super) fn price(
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

        // Its one caller has checked both payload trees' shapes.
        let mut plan = stored_header_plan_kernel_keeping(
            fingerprint,
            literal_lengths,
            distance_lengths,
            exhaustive,
            rle_mask,
            true,
            true,
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

/// Bytes of completed header plans each thread keeps across routes: the
/// arenas' used bytes and the indexes' slots. On the Default benchmark sample,
/// 16 MiB raised peak memory by at most 23 MiB and lowered the median.
const MAX_THREAD_HEADER_PLAN_BYTES: usize = 16 << 20;

/// The budget of a thread that has planned a block exhaustively, which only
/// Max does. Long Max searches plan hundreds of thousands of distinct headers,
/// about 100 bytes each when stored, and return to many of them after the
/// default budget has dropped them.
const MAX_THREAD_EXHAUSTIVE_HEADER_PLAN_BYTES: usize = 64 << 20;

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

/// One generation of a [`HeaderPlanStore`]: each plan's bytes appended to one
/// arena and found by fingerprint.
#[derive(Default)]
struct HeaderPlanGeneration {
    offsets: FingerprintMap<u32>,
    arena: Vec<u8>,
}

impl HeaderPlanGeneration {
    /// The bytes this generation holds, its index slots included. The arena
    /// grows in steps bounded by the budget, so its length stands for it.
    fn charge(&self) -> usize {
        self.arena.len() + self.offsets.capacity() * (std::mem::size_of::<(u64, u32)>() + 1)
    }

    /// Make room for `additional` arena bytes and one index slot: double the
    /// arena, from 64 KiB, but not past `limit` bytes beyond the new entry.
    fn reserve(&mut self, additional: usize, limit: usize) -> bool {
        let length = self.arena.len();
        if self.arena.capacity() - length < additional {
            let target = (2 * self.arena.capacity())
                .max(1 << 16)
                .min(limit)
                .max(length)
                + additional;
            if self.arena.try_reserve_exact(target - length).is_err() {
                return false;
            }
        }
        self.offsets.try_reserve(1).is_ok()
    }
}

/// A stored plan's fixed fields: HLIT (2 bytes), HDIST, HCLEN, the
/// `exhaustive` flag, the RLE mask, the 19 code-length code lengths two per
/// byte (10), the bits (4) and the RLE token count (2). One byte per token
/// follows.
const STORED_HEADER_PLAN_FIELDS: usize = 22;

/// A code-length symbol and its extra bits in one byte: lengths 0 to 15 as
/// themselves, then code 16 from 16, code 17 from 20 and code 18 from 28.
fn rle_token_byte(token: RleToken) -> Option<u8> {
    let (base, extra_limit) = match token.symbol {
        0..=15 => return (token.extra == 0).then_some(token.symbol),
        16 => (16, 3),
        17 => (20, 7),
        18 => (28, 127),
        _ => return None,
    };
    (token.extra <= extra_limit).then(|| base + token.extra)
}

fn rle_token_from_byte(byte: u8) -> RleToken {
    let (symbol, extra) = match byte {
        0..=15 => (byte, 0),
        16..=19 => (16, byte - 16),
        20..=27 => (17, byte - 20),
        _ => (18, byte - 28),
    };
    RleToken { symbol, extra }
}

/// Completed header plans shared by every route on one thread. Routes replan
/// the same code lengths from different token states, and each route-local
/// [`HeaderPlanCache`] starts empty. Two generations within a byte budget, as
/// in [`PlanStore`](crate::deflate::plan_store::PlanStore): when the current generation
/// reaches half the budget it replaces the previous one, and a hit in the
/// previous generation moves forward. A plan is stored compactly, without its
/// code lengths, which its RLE spells: every hash hit decodes the RLE and
/// compares the complete code lengths and policy before reuse.
pub(super) struct HeaderPlanStore {
    current: HeaderPlanGeneration,
    previous: HeaderPlanGeneration,
    max_bytes: usize,
}

impl Default for HeaderPlanStore {
    fn default() -> Self {
        Self::with_budget(MAX_THREAD_HEADER_PLAN_BYTES)
    }
}

thread_local! {
    pub(super) static HEADER_PLAN_STORE: RefCell<HeaderPlanStore> =
        RefCell::new(HeaderPlanStore::default());
}

impl HeaderPlanStore {
    fn with_budget(max_bytes: usize) -> Self {
        Self {
            current: HeaderPlanGeneration::default(),
            previous: HeaderPlanGeneration::default(),
            max_bytes,
        }
    }

    fn get(
        &mut self,
        fingerprint: u64,
        literal_lengths: &[u8],
        distance_lengths: &[u8],
        exhaustive: bool,
        rle_mask: u8,
    ) -> Option<DynamicPlan> {
        let read = |arena: &[u8], offset: u32| {
            read_stored_header_plan(
                arena.get(offset as usize..)?,
                literal_lengths,
                distance_lengths,
                exhaustive,
                rle_mask,
            )
        };
        if let Some(&offset) = self.current.offsets.get(&fingerprint) {
            return read(&self.current.arena, offset).map(|(plan, _)| plan);
        }
        let offset = self.previous.offsets.remove(&fingerprint)?;
        let (plan, length) = read(&self.previous.arena, offset)?;
        // The entry moves forward; its old bytes go with the previous
        // generation.
        let start = offset as usize;
        if let Some(new_offset) = self.make_room(length) {
            self.current
                .arena
                .extend_from_slice(&self.previous.arena[start..start + length]);
            self.index(fingerprint, new_offset);
        }
        Some(plan)
    }

    /// Store `plan`, planned for its own code lengths under this policy.
    fn insert(&mut self, fingerprint: u64, exhaustive: bool, rle_mask: u8, plan: &DynamicPlan) {
        let mut entry = [0_u8; STORED_HEADER_PLAN_FIELDS];
        let (Ok(hlit), Ok(hdist), Ok(hclen), Ok(bits), Ok(tokens)) = (
            u16::try_from(plan.hlit),
            u8::try_from(plan.hdist),
            u8::try_from(plan.hclen),
            u32::try_from(plan.bits),
            u16::try_from(plan.rle.len()),
        ) else {
            return;
        };
        if plan.literal_lengths.len() != plan.hlit
            || plan.distance_lengths.len() != plan.hdist
            || plan.code_length_lengths.iter().any(|&length| length > 15)
        {
            return;
        }
        debug_assert!(rle_spells(
            &plan.rle,
            plan.literal_lengths.iter().chain(&plan.distance_lengths)
        ));
        entry[0..2].copy_from_slice(&hlit.to_le_bytes());
        entry[2] = hdist;
        entry[3] = hclen;
        entry[4] = u8::from(exhaustive);
        entry[5] = rle_mask;
        for (byte, pair) in entry[6..16]
            .iter_mut()
            .zip(plan.code_length_lengths.chunks(2))
        {
            *byte = pair[0] | pair.get(1).map_or(0, |&high| high << 4);
        }
        entry[16..20].copy_from_slice(&bits.to_le_bytes());
        entry[20..22].copy_from_slice(&tokens.to_le_bytes());
        let Some(offset) = self.make_room(STORED_HEADER_PLAN_FIELDS + plan.rle.len()) else {
            return;
        };
        let arena = &mut self.current.arena;
        arena.extend_from_slice(&entry);
        for &token in &plan.rle {
            let Some(byte) = rle_token_byte(token) else {
                arena.truncate(offset as usize);
                return;
            };
            arena.push(byte);
        }
        self.index(fingerprint, offset);
    }

    /// Reserve `length` bytes at the end of the current generation, returning
    /// their offset.
    fn make_room(&mut self, length: usize) -> Option<u32> {
        let offset = u32::try_from(self.current.arena.len()).ok()?;
        self.current
            .reserve(length, self.max_bytes / 2)
            .then_some(offset)
    }

    /// Index the entry written at `offset`, then start a new generation once
    /// this one reaches half the budget.
    fn index(&mut self, fingerprint: u64, offset: u32) {
        self.current.offsets.insert(fingerprint, offset);
        if self.current.charge() >= self.max_bytes / 2 {
            self.previous = std::mem::take(&mut self.current);
        }
    }
}

/// Whether `rle` spells exactly `lengths`.
fn rle_spells<'a>(rle: &[RleToken], lengths: impl IntoIterator<Item = &'a u8>) -> bool {
    let mut lengths = lengths.into_iter().copied();
    let mut previous = None;
    for token in rle {
        let (value, count) = match token.symbol {
            0..=15 => (token.symbol, 1),
            16 => match previous {
                Some(value) => (value, usize::from(token.extra) + 3),
                None => return false,
            },
            17 => (0, usize::from(token.extra) + 3),
            18 => (0, usize::from(token.extra) + 11),
            _ => return false,
        };
        if !(0..count).all(|_| lengths.next() == Some(value)) {
            return false;
        }
        previous = Some(value);
    }
    lengths.next().is_none()
}

/// The plan stored at the start of `entry` if it was planned for these code
/// lengths under this policy, and the entry's length.
fn read_stored_header_plan(
    entry: &[u8],
    literal_lengths: &[u8],
    distance_lengths: &[u8],
    exhaustive: bool,
    rle_mask: u8,
) -> Option<(DynamicPlan, usize)> {
    let fields = entry.get(..STORED_HEADER_PLAN_FIELDS)?;
    let hlit = usize::from(u16::from_le_bytes([fields[0], fields[1]]));
    let hdist = usize::from(fields[2]);
    if hlit != literal_lengths.len()
        || hdist != distance_lengths.len()
        || fields[4] != u8::from(exhaustive)
        || fields[5] != rle_mask
    {
        return None;
    }
    let token_count = usize::from(u16::from_le_bytes([fields[20], fields[21]]));
    let length = STORED_HEADER_PLAN_FIELDS + token_count;
    let tokens = entry.get(STORED_HEADER_PLAN_FIELDS..length)?;
    let mut rle = Vec::new();
    rle.try_reserve_exact(token_count).ok()?;
    rle.extend(tokens.iter().map(|&byte| rle_token_from_byte(byte)));
    if !rle_spells(&rle, literal_lengths.iter().chain(distance_lengths)) {
        return None;
    }
    let mut code_length_lengths = [0_u8; 19];
    for (index, length) in code_length_lengths.iter_mut().enumerate() {
        *length = (fields[6 + index / 2] >> (4 * (index % 2))) & 15;
    }
    let mut literal = Vec::new();
    literal.try_reserve_exact(hlit).ok()?;
    literal.extend_from_slice(literal_lengths);
    let mut distance = Vec::new();
    distance.try_reserve_exact(hdist).ok()?;
    distance.extend_from_slice(distance_lengths);
    let plan = DynamicPlan {
        literal_lengths: literal,
        distance_lengths: distance,
        code_length_lengths,
        rle,
        hlit,
        hdist,
        hclen: usize::from(fields[3]),
        bits: u64::from(u32::from_le_bytes([
            fields[16], fields[17], fields[18], fields[19],
        ])),
    };
    Some((plan, length))
}

/// Plan a header for trimmed code lengths with zero payload bits, reusing this
/// thread's completed plan when one matches exactly.
pub(super) fn stored_header_plan_kernel(
    fingerprint: u64,
    literal_lengths: &[u8],
    distance_lengths: &[u8],
    exhaustive: bool,
    rle_mask: u8,
) -> Option<DynamicPlan> {
    stored_header_plan_kernel_keeping(
        fingerprint,
        literal_lengths,
        distance_lengths,
        exhaustive,
        rle_mask,
        true,
        false,
    )
}

/// [`stored_header_plan_kernel`], storing a newly made plan only if `keep`.
/// With `shapes_valid`, the caller vouches that both payload trees have a
/// valid shape, which the planner then does not check again.
#[allow(clippy::too_many_arguments)]
pub(super) fn stored_header_plan_kernel_keeping(
    fingerprint: u64,
    literal_lengths: &[u8],
    distance_lengths: &[u8],
    exhaustive: bool,
    rle_mask: u8,
    keep: bool,
    shapes_valid: bool,
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
    let plan = if shapes_valid {
        plan_for_valid_trimmed_lengths_uncached(
            literal_lengths,
            distance_lengths,
            0,
            exhaustive,
            rle_mask,
        )
    } else {
        plan_for_trimmed_lengths_uncached(
            literal_lengths,
            distance_lengths,
            0,
            exhaustive,
            rle_mask,
        )
    }?;
    // The planner keeps the code lengths it is given, which the store then
    // need not hold.
    if keep && plan.literal_lengths == literal_lengths && plan.distance_lengths == distance_lengths
    {
        let _ = HEADER_PLAN_STORE.try_with(|store| {
            if let Ok(mut store) = store.try_borrow_mut() {
                store.insert(fingerprint, exhaustive, rle_mask, &plan);
            }
        });
    }
    Some(plan)
}

/// Give this thread's header-plan store Max's budget, which it keeps until
/// released.
pub(super) fn use_exhaustive_header_plan_budget() {
    let _ = HEADER_PLAN_STORE.try_with(|store| {
        if let Ok(mut store) = store.try_borrow_mut() {
            store.max_bytes = store.max_bytes.max(MAX_THREAD_EXHAUSTIVE_HEADER_PLAN_BYTES);
        }
    });
}

pub(super) fn header_plan_fingerprint(
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

#[cfg(test)]
pub(super) mod test_support;
#[cfg(test)]
mod tests;
