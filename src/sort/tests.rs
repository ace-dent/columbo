// SPDX-License-Identifier: MIT
// SPDX-FileCopyrightText: 2026 Andrew C.E. Dent <https://github.com/ace-dent>

use super::*;

fn xorshift(mut state: u64) -> impl FnMut() -> u64 {
    move || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state
    }
}

/// Values with few distinct keys, tagged with their position so equal keys
/// stay distinguishable.
fn tagged_values(next: &mut impl FnMut() -> u64, len: usize) -> Vec<(u64, usize)> {
    let keys = 1 + next() % 9;
    (0..len).map(|index| (next() % keys, index)).collect()
}

#[test]
fn stable_sort_matches_the_standard_stable_sort() {
    let mut next = xorshift(0x9e37_79b9_7f4a_7c15);
    for len in (0..40).chain([100, 1000, 5000]) {
        let values = tagged_values(&mut next, len);
        let mut expected = values.clone();
        expected.sort_by_key(|value| value.0);
        let mut sorted = values.clone();
        sorted.sort_shared_by_key(|value| value.0);
        assert_eq!(sorted, expected);
        let mut descending = values;
        let mut expected = descending.clone();
        expected.sort_by_key(|value| std::cmp::Reverse(value.0));
        descending.sort_shared_by(|left, right| right.0.cmp(&left.0));
        assert_eq!(descending, expected);
    }
}

#[test]
fn index_sort_matches_the_standard_unstable_sort_with_ties() {
    let mut next = xorshift(0x2545_f491_4f6c_dd1d);
    for len in (0..40).chain([100, 1000, 5000]) {
        let keys: Vec<u64> = (0..len).map(|_| next() % 7).collect();
        let mut expected: Vec<usize> = (0..len).collect();
        expected.sort_unstable_by_key(|&index| keys[index]);
        let mut sorted: Vec<usize> = (0..len).collect();
        sort_indices_unstable(&mut sorted, &mut |left, right| keys[left].cmp(&keys[right]));
        assert_eq!(sorted, expected);
    }
}

#[test]
fn in_place_fallback_and_permutation_give_the_stable_order() {
    let mut next = xorshift(0x6a09_e667_f3bc_c909);
    for len in 0..60 {
        let values = tagged_values(&mut next, len);
        let mut expected = values.clone();
        expected.sort_by_key(|value| value.0);

        let mut in_place = values.clone();
        insertion_sort(in_place.len(), &mut |index| {
            let before = in_place[index].0 < in_place[index - 1].0;
            if before {
                in_place.swap(index, index - 1);
            }
            before
        });
        assert_eq!(in_place, expected);

        let mut order: Vec<usize> = (0..len).collect();
        order.sort_by_key(|&index| values[index].0);
        let mut permuted = values;
        permute(&mut permuted, &mut order);
        assert_eq!(permuted, expected);
    }
}
