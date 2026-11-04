// SPDX-License-Identifier: MIT
// SPDX-FileCopyrightText: 2026 Andrew C.E. Dent <https://github.com/ace-dent>

use super::*;

/// A distinct trial key for each `n`, under starts from `start` on: each start
/// has fewer than 2^17 pairs.
fn key(start: u32, n: usize) -> u64 {
    let pair = n % (1 << 16);
    let a = pair / 256;
    let start = start + u32::try_from(n >> 16).unwrap();
    trial_key(start, true, a, a + 1 + pair % 256)
}

#[test]
fn trial_slots_grow_with_use_and_keep_recent_trials() {
    let mut costs = SwapTrialCosts::default();
    let start = costs.start(1, &[1, 2, 2], &[1], true).unwrap();
    assert_eq!(costs.slots.len(), MIN_TRIAL_SLOTS);
    let cost = |n: usize| u16::try_from(n % 1000).unwrap();

    // The first growth moves every trial; the most recent are all found.
    for n in 0..MIN_TRIAL_SLOTS {
        costs.insert(key(start, n), cost(n));
    }
    assert_eq!(costs.slots.len(), 4 * MIN_TRIAL_SLOTS);
    for n in MIN_TRIAL_SLOTS - 100..MIN_TRIAL_SLOTS {
        assert_eq!(costs.get(key(start, n)), Some(cost(n)));
    }

    let total = 2 * MAX_TRIAL_SLOTS;
    for n in MIN_TRIAL_SLOTS..total {
        costs.insert(key(start, n), cost(n));
    }
    assert_eq!(costs.slots.len(), MAX_TRIAL_SLOTS);
    for n in total - 1000..total {
        assert_eq!(costs.get(key(start, n)), Some(cost(n)));
    }
    assert_eq!(costs.get(key(START_ID_LIMIT - 1, 0)), None);
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

#[test]
fn keys_fit_beside_their_costs_and_identifiers_start_over() {
    let largest = trial_key(START_ID_LIMIT - 1, true, 284, 285);
    assert!(largest < EMPTY_SLOT >> 16);
    let mut costs = SwapTrialCosts::default();
    let start = costs.start(1, &[1, 2, 2], &[1], true).unwrap();
    costs.insert(key(start, 0), 9);
    // Running out of identifiers drops every start and trial.
    costs.next_id = START_ID_LIMIT;
    assert_eq!(costs.start(2, &[1, 2, 2], &[2], true), Some(0));
    assert_eq!(costs.get(key(start, 0)), None);
    costs.insert(largest, NO_PLAN - 1);
    costs.insert(key(0, 1), NO_PLAN);
    assert_eq!(costs.get(largest), Some(NO_PLAN - 1));
    assert_eq!(costs.get(key(0, 1)), Some(NO_PLAN));
}
