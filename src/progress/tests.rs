// SPDX-License-Identifier: MIT

use super::*;

#[test]
fn duration_units_are_stable_at_boundaries() {
    assert_eq!(format_duration(Duration::ZERO), "0 µs");
    assert_eq!(format_duration(Duration::from_micros(999)), "999 µs");
    assert_eq!(format_duration(Duration::from_millis(1)), "1.00 ms");
    assert_eq!(format_duration(Duration::from_millis(999)), "999.00 ms");
    assert_eq!(format_duration(Duration::from_secs(1)), "1.00 s");
    assert_eq!(format_duration(Duration::from_secs(10)), "10.0 s");
}

#[test]
fn verbose_and_visual_share_the_format_summary_rows() {
    let mut summary = Vec::new();
    write_format_summary(&mut summary, "ZIP", Some(3)).unwrap();
    assert_eq!(
        String::from_utf8(summary).unwrap(),
        "Format   ZIP\nDeflate streams  3\n"
    );
}

#[test]
fn verbose_and_visual_share_selected_zip_store_reporting() {
    let mut report = Vec::new();
    write_zip_store_changes(
        &mut report,
        &[
            ZipStoreProgress {
                stream_id: 2,
                deflate_bytes: 50,
                stored_bytes: 45,
            },
            ZipStoreProgress {
                stream_id: 7,
                deflate_bytes: 6,
                stored_bytes: 1,
            },
            ZipStoreProgress {
                stream_id: 9,
                deflate_bytes: 4,
                stored_bytes: 4,
            },
        ],
    )
    .unwrap();
    assert_eq!(
        String::from_utf8(report).unwrap(),
        "\nZIP method changes  3 entries\n\
         \x20 S2 Final wrapper · Deflate 50 bytes → Store 45 bytes · saved 5 bytes (10.0%)\n\
         \x20 S7 Final wrapper · Deflate 6 bytes → Store 1 byte · saved 5 bytes (83.3%)\n\
         \x20 S9 Final wrapper · Deflate 4 bytes → Store 4 bytes · saved 0 bytes (0.0%)\n"
    );
}

#[test]
fn completed_streams_release_only_the_contiguous_physical_prefix() {
    let mut coordinator = ReportCoordinator::default();
    coordinator.reset(ProgressMode::Verbose, 4, Duration::from_secs(30));

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
    coordinator.reset(ProgressMode::Visual, 2, Duration::from_secs(30));
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

#[test]
fn bit_change_wording_handles_savings_ties_and_growth() {
    assert_eq!(describe_bit_change(100, 99), "saved 1 bit vs source");
    assert_eq!(describe_bit_change(100, 90), "saved 10 bits vs source");
    assert_eq!(describe_bit_change(100, 100), "same bit length as source");
    assert_eq!(describe_bit_change(100, 101), "added 1 bit vs source");
    assert_eq!(describe_bit_change(100, 111), "added 11 bits vs source");
}

#[test]
fn long_route_heartbeat_cadence_is_bounded() {
    assert_eq!(
        route_heartbeat_interval(Duration::from_secs(10)),
        Duration::from_secs(3)
    );
    assert_eq!(
        route_heartbeat_interval(Duration::from_secs(180)),
        Duration::from_secs(6)
    );
    assert_eq!(
        route_heartbeat_interval(Duration::from_secs(4_000)),
        Duration::from_secs(60)
    );
}
