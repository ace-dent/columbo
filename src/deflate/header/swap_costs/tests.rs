// SPDX-License-Identifier: MIT
// SPDX-FileCopyrightText: 2026 Andrew C.E. Dent <https://github.com/ace-dent>

use super::*;

/// A distinct trial key for each `n`.
fn key(start: u32, n: usize) -> u64 {
    let a = n / 256;
    trial_key(start, true, a, a + 1 + n % 256)
}

#[test]
fn trial_slots_grow_with_use_and_keep_recent_trials() {
    let mut costs = SwapTrialCosts::default();
    let start = costs.start(1, &[1, 2, 2], &[1], true).unwrap();
    assert_eq!(costs.keys.len(), MIN_TRIAL_SLOTS);
    let cost = |n: usize| u32::try_from(n % 1000).unwrap();

    // The first growth moves every trial; the most recent are all found.
    for n in 0..MIN_TRIAL_SLOTS {
        costs.insert(key(start, n), cost(n));
    }
    assert_eq!(costs.keys.len(), 4 * MIN_TRIAL_SLOTS);
    for n in MIN_TRIAL_SLOTS - 100..MIN_TRIAL_SLOTS {
        assert_eq!(costs.get(key(start, n)), Some(cost(n)));
    }

    let total = 2 * MAX_TRIAL_SLOTS;
    for n in MIN_TRIAL_SLOTS..total {
        costs.insert(key(start, n), cost(n));
    }
    assert_eq!(costs.keys.len(), MAX_TRIAL_SLOTS);
    for n in total - 1000..total {
        assert_eq!(costs.get(key(start, n)), Some(cost(n)));
    }
    assert_eq!(costs.get(key(start + 1, total - 1)), None);
}

#[test]
fn starts_are_verified_in_full_and_never_reuse_an_identifier() {
    let mut costs = SwapTrialCosts::default();
    let literal = [1, 2, 2, 0, 3];
    let first = costs.start(7, &literal, &[1], true).unwrap();
    assert_eq!(costs.start(7, &literal, &[1], true), Some(first));
    costs.insert(key(first, 0), 5);

    // The same fingerprint for other lengths, policy or split is another start.
    let other = costs.start(7, &[1, 2, 2, 0, 4], &[1], true).unwrap();
    assert_ne!(other, first);
    assert_eq!(costs.get(key(other, 0)), None);
    let exhaustive = costs.start(7, &[1, 2, 2, 0, 4], &[1], false).unwrap();
    let split = costs.start(7, &[1, 2, 2, 0], &[4, 1], false).unwrap();
    assert!(![first, other].contains(&exhaustive) && ![first, other, exhaustive].contains(&split));

    // Dropped starts come back under new identifiers, without their trials.
    let again = costs.start(7, &literal, &[1], true).unwrap();
    assert!(again > split);
    assert_eq!(costs.get(key(again, 0)), None);
    for fingerprint in 0..MAX_STARTS as u64 {
        costs
            .start(100 + fingerprint, &literal, &[1], true)
            .unwrap();
    }
    assert!(costs.starts.len() <= MAX_STARTS);
}
