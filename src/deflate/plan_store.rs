// SPDX-License-Identifier: MIT
// SPDX-FileCopyrightText: 2026 Andrew C.E. Dent <https://github.com/ace-dent>

//! Bounded stores of completed plans that every route on one thread shares.

use std::collections::HashMap;

/// Approximate bytes an entry keeps alive, including its table slot.
pub(crate) trait Charged {
    fn charge(&self) -> usize;
}

pub(crate) struct Generation<T> {
    pub(crate) plans: HashMap<u64, T>,
    pub(crate) bytes: usize,
}

impl<T> Default for Generation<T> {
    fn default() -> Self {
        Self {
            plans: HashMap::new(),
            bytes: 0,
        }
    }
}

/// Two generations of entries within a byte budget. When the current
/// generation reaches half the budget it replaces the previous one; a hit in
/// the previous generation moves forward, so recently used plans survive. A
/// fingerprint only selects an entry: callers compare the complete key before
/// reuse.
pub(crate) struct PlanStore<T> {
    pub(crate) current: Generation<T>,
    pub(crate) previous: Generation<T>,
    pub(crate) max_bytes: usize,
}

impl<T: Charged> PlanStore<T> {
    pub(crate) fn with_budget(max_bytes: usize) -> Self {
        Self {
            current: Generation::default(),
            previous: Generation::default(),
            max_bytes,
        }
    }

    /// Read the entry stored under `fingerprint` if `matches` accepts it. A
    /// previous-generation entry moves forward when accepted and is dropped
    /// when not.
    pub(crate) fn lookup<R>(
        &mut self,
        fingerprint: u64,
        matches: impl FnOnce(&T) -> bool,
        read: impl FnOnce(&T) -> Option<R>,
    ) -> Option<R> {
        if let Some(entry) = self.current.plans.get(&fingerprint) {
            return matches(entry).then(|| read(entry)).flatten();
        }
        let entry = self.previous.plans.remove(&fingerprint)?;
        self.previous.bytes -= entry.charge();
        if !matches(&entry) {
            return None;
        }
        let plan = read(&entry);
        self.insert(fingerprint, entry);
        plan
    }

    pub(crate) fn insert(&mut self, fingerprint: u64, entry: T) {
        if self.current.plans.try_reserve(1).is_err() {
            return;
        }
        self.current.bytes += entry.charge();
        if let Some(replaced) = self.current.plans.insert(fingerprint, entry) {
            self.current.bytes -= replaced.charge();
        }
        if self.current.bytes >= self.max_bytes / 2 {
            self.previous = std::mem::take(&mut self.current);
        }
    }
}
