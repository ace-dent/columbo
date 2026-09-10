// SPDX-License-Identifier: MIT

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
fn duration_scaling_handles_extreme_public_options() {
    let scaled = scale_duration(Duration::MAX, 0.98);
    assert!(scaled < Duration::MAX);
    assert_eq!(scale_duration(Duration::MAX, 1.0), Duration::MAX);
    assert_eq!(scale_duration(Duration::MAX, f64::NAN), Duration::ZERO);
}
