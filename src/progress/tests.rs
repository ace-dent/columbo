// SPDX-License-Identifier: MIT
// SPDX-FileCopyrightText: 2026 Andrew C.E. Dent <https://github.com/ace-dent>

use super::*;

#[test]
fn verbose_reports_print_the_format_summary_rows() {
    let mut summary = Vec::new();
    write_format_summary(&mut summary, "ZIP", Some(3)).unwrap();
    assert_eq!(
        String::from_utf8(summary).unwrap(),
        "Format   ZIP\nDeflate streams  3\n"
    );
}

#[test]
fn verbose_reports_print_selected_zip_store_changes() {
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
fn bit_change_wording_handles_savings_ties_and_growth() {
    assert_eq!(describe_bit_change(100, 99), "saved 1 bit vs source");
    assert_eq!(describe_bit_change(100, 90), "saved 10 bits vs source");
    assert_eq!(describe_bit_change(100, 100), "same bit length as source");
    assert_eq!(describe_bit_change(100, 101), "added 1 bit vs source");
    assert_eq!(describe_bit_change(100, 111), "added 11 bits vs source");
}

#[test]
fn reporting_calls_hold_an_exclusive_session_while_quiet_calls_do_not() {
    let verbose = Options {
        verbose: true,
        ..Options::default()
    };
    let first = begin_report_session(&verbose).expect("verbose calls report");
    // A quiet call neither takes nor waits for the session.
    assert!(begin_report_session(&Options::default()).is_none());

    let (sender, receiver) = std::sync::mpsc::channel();
    let waiting = verbose.clone();
    let waiter = std::thread::spawn(move || {
        let _second = begin_report_session(&waiting);
        sender.send(()).unwrap();
    });
    assert!(receiver
        .recv_timeout(std::time::Duration::from_millis(100))
        .is_err());
    drop(first);
    receiver
        .recv_timeout(std::time::Duration::from_secs(10))
        .unwrap();
    waiter.join().unwrap();
}
