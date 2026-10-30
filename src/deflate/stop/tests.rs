// SPDX-License-Identifier: MIT
// SPDX-FileCopyrightText: 2026 Andrew C.E. Dent <https://github.com/ace-dent>

use super::*;

#[test]
fn bounded_finalization_belongs_to_one_graced_hard_deadline() {
    let graced = Deadline::with_grace(
        Instant::now(),
        Duration::from_secs(1),
        Duration::from_secs(1),
    );
    assert!(graced.hard_stop().permits_bounded_finalization());
    assert!(!graced.bounded_stop(true).permits_bounded_finalization());
    assert!(!RouteWindow::full(&graced)
        .stop()
        .permits_bounded_finalization());

    let ungraced = Deadline::with_grace(Instant::now(), Duration::from_secs(1), Duration::ZERO);
    assert!(!ungraced.hard_stop().permits_bounded_finalization());

    graced.cancel_routes();
    assert!(!graced.hard_stop().permits_bounded_finalization());
    assert!(SearchStop::always().permits_bounded_finalization());
}

#[test]
fn finalization_only_keeps_the_hard_stop_and_starts_no_route() {
    let deadline = Deadline::with_grace(
        Instant::now(),
        Duration::from_secs(60),
        Duration::from_secs(7),
    );
    let finalization = deadline.finalization_only();
    assert!(deadline.can_start_route());
    assert!(!finalization.can_start_route());
    assert!(!finalization.expired());
    assert!(finalization.hard_stop().permits_bounded_finalization());
    assert_eq!(
        finalization.duration + finalization.grace,
        deadline.duration + deadline.grace
    );

    let spent = Deadline::with_grace(Instant::now(), Duration::ZERO, Duration::ZERO);
    assert!(spent.finalization_only().expired());
}
