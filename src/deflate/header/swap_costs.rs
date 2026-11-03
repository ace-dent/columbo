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

/// The most trial slots a thread keeps, 12 bytes each, 6 MiB. Repeated trials
/// on the benchmark sample were mostly 32,000 to 256,000 trials apart, and on
/// a traced run these slots kept 93% of them, against 79% with half as many.
const MAX_TRIAL_SLOTS: usize = 1 << 19;
/// The slots a thread starts with. They grow fourfold each time they have
/// taken as many new trials as they hold, so a short search keeps little.
const MIN_TRIAL_SLOTS: usize = 1 << 12;
/// Slots in each set a key hashes to, most recently used first: on a traced
/// run, four ways kept as many repeats as twice the slots mapped directly.
const TRIAL_WAYS: usize = 4;
/// Start sequences a thread keeps before dropping them all.
const MAX_STARTS: usize = 4096;
/// A slot with no trial: no start reaches the last identifier.
const EMPTY_KEY: u64 = u64::MAX;
/// The cost of a trial that has no plan.
const NO_PLAN: u32 = u32::MAX;

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
    keys: Vec<u64>,
    costs: Vec<u32>,
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
        if self.next_id == u32::MAX {
            *self = Self::default();
        }
        if self.starts.len() >= MAX_STARTS {
            self.starts.clear();
        }
        if self.keys.is_empty() && !self.resize(MIN_TRIAL_SLOTS) {
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
        let set_bits = (self.keys.len() / TRIAL_WAYS).trailing_zeros();
        (key.wrapping_mul(0x9e37_79b9_7f4a_7c15) >> (u64::BITS - set_bits)) as usize * TRIAL_WAYS
    }

    /// The kept cost of the trial, which becomes its set's most recent.
    fn get(&mut self, key: u64) -> Option<u32> {
        if self.keys.is_empty() {
            return None;
        }
        let set = self.set(key);
        let way = self.keys[set..set + TRIAL_WAYS]
            .iter()
            .position(|&kept| kept == key)?;
        self.keys[set..=set + way].rotate_right(1);
        self.costs[set..=set + way].rotate_right(1);
        Some(self.costs[set])
    }

    fn insert(&mut self, key: u64, cost: u32) {
        if self.keys.is_empty() {
            return;
        }
        self.place(key, cost);
        self.inserted += 1;
        if self.inserted >= self.keys.len() && self.keys.len() < MAX_TRIAL_SLOTS {
            self.resize((self.keys.len() * 4).min(MAX_TRIAL_SLOTS));
        }
    }

    /// Make the trial its set's most recent, replacing the least recent.
    fn place(&mut self, key: u64, cost: u32) {
        let set = self.set(key);
        self.keys[set..set + TRIAL_WAYS].rotate_right(1);
        self.costs[set..set + TRIAL_WAYS].rotate_right(1);
        self.keys[set] = key;
        self.costs[set] = cost;
    }

    /// Move the trials to `slots` slots, each set's least recent first so that
    /// its order of use survives, or keep them where they are if the slots
    /// cannot be allocated.
    fn resize(&mut self, slots: usize) -> bool {
        let mut keys = Vec::new();
        let mut costs = Vec::new();
        if keys.try_reserve_exact(slots).is_err() || costs.try_reserve_exact(slots).is_err() {
            return false;
        }
        keys.resize(slots, EMPTY_KEY);
        costs.resize(slots, NO_PLAN);
        let old_keys = std::mem::replace(&mut self.keys, keys);
        let old_costs = std::mem::replace(&mut self.costs, costs);
        for (keys, costs) in old_keys
            .chunks_exact(TRIAL_WAYS)
            .zip(old_costs.chunks_exact(TRIAL_WAYS))
        {
            for (&key, &cost) in keys.iter().zip(costs).rev() {
                if key != EMPTY_KEY {
                    self.place(key, cost);
                }
            }
        }
        self.inserted = 0;
        true
    }
}

fn trial_key(start: u32, literal_tree: bool, a: usize, b: usize) -> u64 {
    debug_assert!(a < 1 << 15 && b < 1 << 16);
    (u64::from(start) << 32) | (u64::from(literal_tree) << 31) | ((a as u64) << 16) | b as u64
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
    let cost = match cost.map(u32::try_from) {
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
