// SPDX-License-Identifier: MIT

use super::*;
use crate::progress::report_order;

#[test]
fn finished_reports_flush_in_physical_stream_order() {
    let mut reports = Reports::default();
    reports.insert(30, 1, 0, "stream one\n".to_owned());
    reports.insert(10, 3, 0, "stream three\n".to_owned());
    reports.insert(20, 2, 0, "stream two\n".to_owned());

    reports.finish(10);
    reports.finish(20);
    reports.finish(30);
    let ordered: Vec<_> = reports
        .take_finished_in_stream_order()
        .into_iter()
        .map(|report| (report.stream_id, report.text))
        .collect();

    assert_eq!(
        ordered,
        [
            (1, "stream one\n".to_owned()),
            (2, "stream two\n".to_owned()),
            (3, "stream three\n".to_owned()),
        ]
    );
}

#[test]
fn one_finished_stream_can_be_removed_without_draining_later_reports() {
    let mut reports = Reports::default();
    reports.insert(1, 2, 0, "stream two\n".to_owned());
    reports.insert(2, 1, 1, "stream one second\n".to_owned());
    reports.insert(3, 1, 0, "stream one first\n".to_owned());
    reports.finish(1);
    reports.finish(2);
    reports.finish(3);

    let first: Vec<_> = reports
        .take_finished_stream(1)
        .unwrap()
        .into_iter()
        .map(|report| report.text)
        .collect();
    assert_eq!(first, ["stream one first\n", "stream one second\n"]);
    assert_eq!(reports.reports.len(), 1);
    assert_eq!(reports.reports[&1].stream_id, 2);
}

#[test]
fn incomplete_reports_are_not_emitted() {
    let mut reports = Reports::default();
    reports.insert(1, 1, 0, "complete\n".to_owned());
    reports.insert(2, 2, 0, "partial\n".to_owned());
    reports.finish(1);

    let ordered = reports.take_finished_in_stream_order();
    assert_eq!(ordered.len(), 1);
    assert_eq!(ordered[0].stream_id, 1);
    assert_eq!(ordered[0].text, "complete\n");
}

#[test]
fn lineages_have_a_stable_order_within_one_stream() {
    let mut reports = Reports::default();
    reports.insert(
        1,
        4,
        report_order(Some("refined default")),
        "refined".to_owned(),
    );
    reports.insert(2, 4, report_order(Some("direct max")), "direct".to_owned());
    reports.insert(
        3,
        4,
        report_order(Some("default floor")),
        "floor".to_owned(),
    );
    reports.finish(1);
    reports.finish(2);
    reports.finish(3);

    let ordered: Vec<_> = reports
        .take_finished_in_stream_order()
        .into_iter()
        .map(|report| report.text)
        .collect();
    assert_eq!(ordered, ["floor", "direct", "refined"]);
}
