// SPDX-License-Identifier: MIT
// SPDX-FileCopyrightText: 2026 Andrew C.E. Dent <https://github.com/ace-dent>

use super::*;

#[test]
fn completed_streams_release_only_the_contiguous_physical_prefix() {
    let mut coordinator = ReportCoordinator::default();
    coordinator.reset(4, Duration::from_secs(30));

    coordinator.complete(3, &[], PRIMARY_STREAM_PRODUCER);
    coordinator.complete(1, &[2], PRIMARY_STREAM_PRODUCER);
    assert_eq!(coordinator.next_sealed(), Some(1));
    coordinator.advance(1);
    assert_eq!(coordinator.next_sealed(), Some(2));
    coordinator.advance(2);
    assert_eq!(coordinator.next_sealed(), Some(3));
    coordinator.advance(3);
    assert_eq!(coordinator.next_sealed(), None);

    coordinator.complete(4, &[], PRIMARY_STREAM_PRODUCER);
    assert_eq!(coordinator.next_sealed(), Some(4));
}

#[test]
fn every_registered_lineage_must_finish_before_a_stream_is_sealed() {
    let mut coordinator = ReportCoordinator::default();
    coordinator.reset(2, Duration::from_secs(30));
    coordinator.set_expected_producers(&[1, 2, 3]);

    coordinator.complete(1, &[], 2);
    coordinator.complete(1, &[], 1);
    assert_eq!(coordinator.next_sealed(), None);
    coordinator.complete(2, &[], 1);
    coordinator.complete(2, &[], 2);
    coordinator.complete(2, &[], 3);
    assert_eq!(coordinator.next_sealed(), None);
    coordinator.complete(1, &[], 3);
    assert_eq!(coordinator.next_sealed(), Some(1));
}
