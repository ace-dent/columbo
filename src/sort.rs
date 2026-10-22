// SPDX-License-Identifier: MIT
// SPDX-FileCopyrightText: 2026 Andrew C.E. Dent <https://github.com/ace-dent>

//! Shared sorting for code off the hot paths. The standard library compiles a
//! separate sort for each element type and comparator; these helpers send
//! their callers through two shared instances that sort index slices.

use std::cmp::Ordering;

/// Sort `indices` with the standard unstable sort. Every caller shares this
/// instance, and for a `usize` slice it gives the same order as
/// `sort_unstable_by` with the same comparisons, ties included.
#[inline(never)]
pub(crate) fn sort_indices_unstable(
    indices: &mut [usize],
    compare: &mut dyn FnMut(usize, usize) -> Ordering,
) {
    indices.sort_unstable_by(|&left, &right| compare(left, right));
}

#[inline(never)]
fn sort_indices_stable(indices: &mut [usize], compare: &mut dyn FnMut(usize, usize) -> Ordering) {
    indices.sort_by(|&left, &right| compare(left, right));
}

/// Stable sorts through one shared index sort. A stable order is unique, so
/// these give the order of `sort_by` and `sort_by_key`, and of the unstable
/// sorts when no two distinct values compare equal.
pub(crate) trait SortShared<T> {
    fn sort_shared_by(&mut self, compare: impl FnMut(&T, &T) -> Ordering);

    fn sort_shared_by_key<K: Ord>(&mut self, mut key: impl FnMut(&T) -> K) {
        self.sort_shared_by(|left, right| key(left).cmp(&key(right)));
    }
}

impl<T> SortShared<T> for [T] {
    fn sort_shared_by(&mut self, mut compare: impl FnMut(&T, &T) -> Ordering) {
        sort_values_stable(self, &mut compare);
    }
}

/// The body of [`SortShared::sort_shared_by`], compiled once per element type
/// rather than once per comparator.
fn sort_values_stable<T>(values: &mut [T], compare: &mut dyn FnMut(&T, &T) -> Ordering) {
    let mut order = Vec::new();
    if order.try_reserve_exact(values.len()).is_err() {
        // Without an index buffer, sort in place.
        insertion_sort(values.len(), &mut |index| {
            let before = compare(&values[index], &values[index - 1]).is_lt();
            if before {
                values.swap(index, index - 1);
            }
            before
        });
        return;
    }
    order.extend(0..values.len());
    sort_indices_stable(&mut order, &mut |left, right| {
        compare(&values[left], &values[right])
    });
    permute(values, &mut order);
}

/// Stable insertion sort of positions `0..len`; `step_back(index)` moves the
/// value at `index` before its predecessor when it sorts first, and reports
/// whether it did.
#[inline(never)]
fn insertion_sort(len: usize, step_back: &mut dyn FnMut(usize) -> bool) {
    for start in 1..len {
        let mut index = start;
        while index > 0 && step_back(index) {
            index -= 1;
        }
    }
}

/// Put the value at `order[i]` at position `i`, following each cycle once.
/// Consumes `order`.
fn permute<T>(values: &mut [T], order: &mut [usize]) {
    for start in 0..order.len() {
        let mut position = start;
        while order[position] != usize::MAX {
            let source = std::mem::replace(&mut order[position], usize::MAX);
            if source == start {
                break;
            }
            values.swap(position, source);
            position = source;
        }
    }
}

#[cfg(test)]
mod tests;
