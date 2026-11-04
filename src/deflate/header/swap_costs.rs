// SPDX-License-Identifier: MIT
// SPDX-FileCopyrightText: 2026 Andrew C.E. Dent <https://github.com/ace-dent>

//! Header costs of the greedy length-swap trials, kept per thread across
//! searches. A pass prices every swap of its start lengths with the greedy RLE
//! spelling alone, and blocks that differ only in their counts often start a
//! pass from the same lengths: the queue's children mostly keep their
//! parent's code lengths. A trial's header depends only on the start lengths
//! and the swapped pair, so its cost can be kept without its plan, in a
//! fraction of a stored plan's bytes.

use std::cell::RefCell;
use std::collections::HashMap;

/// The most trial slots a thread keeps, 8 bytes each, 32 MiB. A completing
/// Max run of `star.png` prices about 22 million trials, a quarter of them
/// distinct, and most of the rest repeat 100,000 to 1,000,000 trials apart:
/// these slots cut that run's instructions by a third, against 7% for 2^20.
const MAX_TRIAL_SLOTS: usize = 1 << 22;
/// The slots a thread starts with. They grow fourfold each time they have
/// taken half as many new trials as they hold, before many sets are full, so
/// a short search keeps little.
const MIN_TRIAL_SLOTS: usize = 1 << 12;
/// Slots in each set a key hashes to, most recently used first: eight 8-byte
/// slots fill one 64-byte cache line.
const TRIAL_WAYS: usize = 8;
/// Start sequences a thread keeps before dropping them all.
const MAX_STARTS: usize = 4096;
/// Start identifiers stay below this, so a trial's key fits in 48 bits.
const START_ID_LIMIT: u32 = (1 << 29) - 1;
/// A slot with no trial: its key would need the identifier `START_ID_LIMIT`.
const EMPTY_SLOT: u64 = u64::MAX;
/// The cost of a trial that has no plan; a header this long is not kept.
const NO_PLAN: u16 = u16::MAX;

thread_local! {
    static SWAP_TRIAL_COSTS: RefCell<SwapTrialCosts> = RefCell::new(SwapTrialCosts::default());
}

/// A pass's start lengths, found by fingerprint and compared in full.
struct Start {
    id: u32,
    exhaustive: bool,
    literal_count: usize,
    lengths: Box<[u8]>,
}

#[derive(Default)]
struct SwapTrialCosts {
    starts: HashMap<u64, Start>,
    /// Identifiers are never reused, so a trial kept under a dropped start
    /// cannot answer for another.
    next_id: u32,
    /// Each trial's 48-bit key above its 16-bit cost.
    slots: Vec<u64>,
    /// New trials since the slots last grew.
    inserted: usize,
}

impl SwapTrialCosts {
    fn start(
        &mut self,
        fingerprint: u64,
        literal_lengths: &[u8],
        distance_lengths: &[u8],
        exhaustive: bool,
    ) -> Option<u32> {
        if let Some(start) = self.starts.get(&fingerprint) {
            if start.exhaustive == exhaustive
                && start.literal_count == literal_lengths.len()
                && start.lengths.len() == literal_lengths.len() + distance_lengths.len()
                && start.lengths[..literal_lengths.len()] == *literal_lengths
                && start.lengths[literal_lengths.len()..] == *distance_lengths
            {
                return Some(start.id);
            }
        }
        if self.next_id == START_ID_LIMIT {
            *self = Self::default();
        }
        if self.starts.len() >= MAX_STARTS {
            self.starts.clear();
        }
        if self.slots.is_empty() && !self.resize(MIN_TRIAL_SLOTS) {
            return None;
        }
        let mut lengths = Vec::new();
        lengths
            .try_reserve_exact(literal_lengths.len() + distance_lengths.len())
            .ok()?;
        lengths.extend_from_slice(literal_lengths);
        lengths.extend_from_slice(distance_lengths);
        let id = self.next_id;
        self.next_id += 1;
        self.starts.insert(
            fingerprint,
            Start {
                id,
                exhaustive,
                literal_count: literal_lengths.len(),
                lengths: lengths.into_boxed_slice(),
            },
        );
        Some(id)
    }

    /// The first slot of the key's set.
    fn set(&self, key: u64) -> usize {
        let set_bits = (self.slots.len() / TRIAL_WAYS).trailing_zeros();
        (key.wrapping_mul(0x9e37_79b9_7f4a_7c15) >> (u64::BITS - set_bits)) as usize * TRIAL_WAYS
    }

    /// The kept cost of the trial, which becomes its set's most recent.
    fn get(&mut self, key: u64) -> Option<u16> {
        if self.slots.is_empty() {
            return None;
        }
        let set = self.set(key);
        let way = self.slots[set..set + TRIAL_WAYS]
            .iter()
            .position(|&slot| slot >> 16 == key)?;
        self.slots[set..=set + way].rotate_right(1);
        Some(self.slots[set] as u16)
    }

    fn insert(&mut self, key: u64, cost: u16) {
        if self.slots.is_empty() {
            return;
        }
        self.place(key << 16 | u64::from(cost));
        self.inserted += 1;
        if self.inserted >= self.slots.len() / 2 && self.slots.len() < MAX_TRIAL_SLOTS {
            self.resize((self.slots.len() * 4).min(MAX_TRIAL_SLOTS));
        }
    }

    /// Make the slot its set's most recent, replacing the least recent.
    fn place(&mut self, slot: u64) {
        let set = self.set(slot >> 16);
        self.slots[set..set + TRIAL_WAYS].rotate_right(1);
        self.slots[set] = slot;
    }

    /// Move the trials to `count` slots, each set's least recent first so that
    /// its order of use survives, or keep them where they are if the slots
    /// cannot be allocated.
    fn resize(&mut self, count: usize) -> bool {
        let mut slots = Vec::new();
        if slots.try_reserve_exact(count).is_err() {
            return false;
        }
        slots.resize(count, EMPTY_SLOT);
        let old = std::mem::replace(&mut self.slots, slots);
        for set in old.chunks_exact(TRIAL_WAYS) {
            for &slot in set.iter().rev() {
                if slot != EMPTY_SLOT {
                    self.place(slot);
                }
            }
        }
        self.inserted = 0;
        true
    }
}

/// A trial's 48-bit key: its start, its tree and its pair of symbols, both
/// below 286.
fn trial_key(start: u32, literal_tree: bool, a: usize, b: usize) -> u64 {
    debug_assert!(start < START_ID_LIMIT && a < 1 << 9 && b < 1 << 9);
    (u64::from(start) << 19) | (u64::from(literal_tree) << 18) | ((a as u64) << 9) | b as u64
}

/// The identifier of these start lengths under this policy, or `None` when
/// the thread cannot keep trials.
pub(super) fn swap_trial_start(
    fingerprint: u64,
    literal_lengths: &[u8],
    distance_lengths: &[u8],
    exhaustive: bool,
) -> Option<u32> {
    SWAP_TRIAL_COSTS
        .try_with(|costs| {
            costs.try_borrow_mut().ok()?.start(
                fingerprint,
                literal_lengths,
                distance_lengths,
                exhaustive,
            )
        })
        .ok()
        .flatten()
}

/// The kept header cost of swapping `a` and `b` in the start's literal/length
/// or distance lengths: `Some(None)` when that trial has no plan, `None` when
/// it is not kept.
pub(super) fn kept_swap_trial(
    start: u32,
    literal_tree: bool,
    a: usize,
    b: usize,
) -> Option<Option<u64>> {
    let key = trial_key(start, literal_tree, a, b);
    SWAP_TRIAL_COSTS
        .try_with(|costs| {
            let cost = costs.try_borrow_mut().ok()?.get(key)?;
            Some((cost != NO_PLAN).then_some(u64::from(cost)))
        })
        .ok()
        .flatten()
}

/// Keep the header cost of a trial, or that it has no plan.
pub(super) fn keep_swap_trial(
    start: u32,
    literal_tree: bool,
    a: usize,
    b: usize,
    cost: Option<u64>,
) {
    let cost = match cost.map(u16::try_from) {
        Some(Ok(cost)) if cost != NO_PLAN => cost,
        Some(_) => return,
        None => NO_PLAN,
    };
    let key = trial_key(start, literal_tree, a, b);
    let _ = SWAP_TRIAL_COSTS.try_with(|costs| {
        if let Ok(mut costs) = costs.try_borrow_mut() {
            costs.insert(key, cost);
        }
    });
}

/// Drop this thread's trials.
pub(super) fn release_swap_trials() {
    let _ = SWAP_TRIAL_COSTS.try_with(|costs| {
        if let Ok(mut costs) = costs.try_borrow_mut() {
            *costs = SwapTrialCosts::default();
        }
    });
}

#[cfg(test)]
pub(super) mod test_support;
#[cfg(test)]
mod tests;
