// SPDX-License-Identifier: MIT
// SPDX-FileCopyrightText: 2026 Andrew C.E. Dent <https://github.com/ace-dent>

use super::*;

#[test]
fn file_deadline_is_shared_in_normal_mode_too() {
    let options = Options {
        timeout: Duration::from_secs(10),
        ..Options::default()
    };
    let expired = SearchDeadline {
        started: Instant::now() - Duration::from_secs(2),
        timeout: Duration::from_secs(1),
    };

    assert_eq!(expired.options_for_call(&options).timeout, Duration::ZERO);
    assert!(expired.is_expired());

    let active = SearchDeadline {
        started: Instant::now(),
        timeout: Duration::from_secs(10),
    };
    assert!(!active.is_expired());
}

#[test]
fn only_a_child_owning_the_file_remainder_receives_global_grace() {
    let options = Options {
        timeout: Duration::from_secs(10),
        ..Options::default()
    };
    let deadline = SearchDeadline::new(&options);

    assert_eq!(
        deadline.grace_for_call(Duration::from_secs(1)),
        Duration::from_millis(200)
    );
    let remainder = deadline.remaining();
    let grace = deadline.grace_for_call(remainder);
    assert!(grace > Duration::from_millis(1_900));
    assert!(grace <= Duration::from_secs(2));
}

#[test]
fn a_stage_keeps_its_own_grace_within_the_file_hard_stop() {
    let fresh = SearchDeadline {
        started: Instant::now(),
        timeout: Duration::from_secs(10),
    };
    // A 2-second stage keeps the raw optimizer's 1 + 0.2 seconds, where a
    // sibling slice of the same length gets 2/10 of the file's 2 seconds.
    assert_eq!(
        fresh.grace_for_stage(Duration::from_secs(2)),
        Duration::from_millis(1_200)
    );
    assert_eq!(
        fresh.grace_for_call(Duration::from_secs(2)),
        Duration::from_millis(400)
    );

    // Eleven seconds into a 10-second file, the hard stop is 1 second away.
    let late = SearchDeadline {
        started: Instant::now() - Duration::from_secs(11),
        timeout: Duration::from_secs(10),
    };
    assert!(late.grace_for_stage(Duration::from_millis(500)) <= Duration::from_millis(500));
    assert_eq!(late.grace_for_stage(Duration::from_secs(1)), Duration::ZERO);

    let zero = SearchDeadline {
        started: Instant::now(),
        timeout: Duration::ZERO,
    };
    assert_eq!(zero.grace_for_stage(Duration::ZERO), Duration::ZERO);
}

#[test]
fn duration_scaling_handles_extreme_public_options() {
    let scaled = scale_duration(Duration::MAX, 0.98);
    assert!(scaled < Duration::MAX);
    assert_eq!(scale_duration(Duration::MAX, 1.0), Duration::MAX);
    assert_eq!(scale_duration(Duration::MAX, f64::NAN), Duration::ZERO);
}
