// SPDX-License-Identifier: MIT

use crate::cli::test_support::{parsed_command, parsed_options};

use super::*;

#[test]
fn percentage_reporting_handles_the_input_limit_on_32_bit_targets() {
    assert_eq!(
        describe_byte_change(1 << 30, 1 << 29),
        "saved 536870912 bytes (50.00%)"
    );
    assert_eq!(
        describe_byte_change(1 << 30, 0),
        "saved 1073741824 bytes (100.00%)"
    );
}

#[test]
fn visual_mode_reuses_the_detailed_start_and_end_summaries() {
    let visual = parsed_command(["--visual", "--dry-run", "in"]);
    let verbose = parsed_command(["--verbose", "--dry-run", "in"]);
    let mut visual_header = Vec::new();
    let mut verbose_header = Vec::new();
    let input = InputReport {
        path: Path::new("in"),
        index: 0,
        count: 1,
        bytes: 1_024,
    };
    print_detailed_header(
        &mut visual_header,
        ReportMode::Visual,
        &visual,
        &input,
        Duration::from_millis(2),
    )
    .unwrap();
    print_detailed_header(
        &mut verbose_header,
        ReportMode::Verbose,
        &verbose,
        &input,
        Duration::from_millis(2),
    )
    .unwrap();

    let visual_header = String::from_utf8(visual_header).unwrap();
    let verbose_header = String::from_utf8(verbose_header).unwrap();
    assert!(visual_header.contains("· visual\n"));
    assert!(verbose_header.contains("· verbose\n"));
    assert_eq!(
        visual_header.lines().skip(3).collect::<Vec<_>>(),
        verbose_header.lines().skip(3).collect::<Vec<_>>()
    );
    let mut header_lines = visual_header.lines();
    assert_eq!(header_lines.next(), Some(""));
    let title = header_lines.next().unwrap();
    let underline = header_lines.next().unwrap();
    assert_eq!(title.chars().count(), underline.chars().count());
    assert!(underline.chars().all(|character| character == '─'));
    assert!(visual_header.contains("Input    \"in\" · 1024 bytes · read 2.00 ms"));
    assert!(visual_header.contains("Mode     normal · strict · dry run"));
    assert!(visual_header.ends_with("Mode     normal · strict · dry run\n"));

    let mut result = Vec::new();
    let optimized = OptimizationReport {
        bytes: 1_000,
        bits_saved: 192,
    };
    print_detailed_result(
        &mut result,
        &input,
        &OutputAction::DryRun,
        &optimized,
        &ExecutionTimings {
            read: Duration::from_millis(2),
            optimize: Duration::from_millis(30),
            write: Duration::ZERO,
            total: Duration::from_millis(32),
        },
    )
    .unwrap();
    let result = String::from_utf8(result).unwrap();
    assert!(result.contains("Result\n"));
    assert!(result.contains("Output  not written · dry run"));
    assert!(result.contains("Size    1024 → 1000 bytes · saved 24 bytes (2.34%)"));
    assert!(result.contains("Time    32.00 ms total"));
}

#[test]
fn default_result_prefixes_the_quoted_input_filename() {
    let mut result = Vec::new();
    let input = InputReport {
        path: Path::new("directory/abc.file"),
        index: 0,
        count: 1,
        bytes: 100,
    };
    let optimized = OptimizationReport {
        bytes: 90,
        bits_saved: 80,
    };
    print_quiet_result(
        &mut result,
        &input,
        &OutputAction::WrittenOptimized(PathBuf::from("output.file")),
        &optimized,
    )
    .unwrap();

    assert_eq!(
        String::from_utf8(result).unwrap(),
        "\"abc.file\" 100 -> 90 bytes\n"
    );
}

#[test]
fn default_result_describes_equal_sized_wrapper_normalization() {
    let mut result = Vec::new();
    let input = InputReport {
        path: Path::new("input.zip"),
        index: 0,
        count: 1,
        bytes: 100,
    };
    let optimized = OptimizationReport {
        bytes: 100,
        bits_saved: 0,
    };
    print_quiet_result(
        &mut result,
        &input,
        &OutputAction::WrittenOptimized(PathBuf::from("input.zip")),
        &optimized,
    )
    .unwrap();

    assert_eq!(
        String::from_utf8(result).unwrap(),
        "\"input.zip\" 100 -> 100 bytes (representation normalized; no size saving)\n"
    );
}

#[test]
fn verbose_and_visual_share_the_detailed_result_renderer() {
    let input = InputReport {
        path: Path::new("directory/abc.file"),
        index: 0,
        count: 1,
        bytes: 100,
    };
    let optimized = OptimizationReport {
        bytes: 90,
        bits_saved: 80,
    };
    let timings = ExecutionTimings {
        read: Duration::from_millis(1),
        optimize: Duration::from_millis(2),
        write: Duration::ZERO,
        total: Duration::from_millis(3),
    };
    let mut verbose = Vec::new();
    let mut visual = Vec::new();
    print_result(
        &mut verbose,
        ReportMode::Verbose,
        &input,
        &OutputAction::DryRun,
        &optimized,
        &timings,
    )
    .unwrap();
    print_result(
        &mut visual,
        ReportMode::Visual,
        &input,
        &OutputAction::DryRun,
        &optimized,
        &timings,
    )
    .unwrap();

    assert_eq!(verbose, visual);
    let detailed = String::from_utf8(verbose).unwrap();
    assert!(detailed.starts_with("\nResult\n"));
    assert!(detailed.contains("Size    100 → 90 bytes · saved 10 bytes (10.00%)"));
}

#[test]
fn timeout_notice_describes_the_output_action() {
    let timeout = Duration::from_secs(180);
    for (action, expected) in [
        (OutputAction::DryRun, "reporting best result found so far"),
        (
            OutputAction::WrittenOptimized(PathBuf::from("out")),
            "wrote best output found so far",
        ),
        (
            OutputAction::Preserved(PathBuf::from("out")),
            "no smaller output was written",
        ),
    ] {
        let mut notice = Vec::new();
        print_timeout_notice(&mut notice, &action, timeout).unwrap();
        let notice = String::from_utf8(notice).unwrap();
        assert!(notice.starts_with("Timeout triggered after 180 seconds; "));
        assert!(notice.contains(expected));
    }
}

#[test]
fn timeout_notice_is_limited_to_verbose_and_visual_modes() {
    assert!(!ReportMode::for_options(&parsed_options(["in"])).reports_timeout());
    assert!(ReportMode::for_options(&parsed_options(["--verbose", "in"])).reports_timeout());
    assert!(ReportMode::for_options(&parsed_options(["--visual", "in"])).reports_timeout());
}

#[test]
fn verbose_and_visual_batch_headers_report_the_file_position() {
    for (flag, mode) in [("--verbose", "verbose"), ("--visual", "visual")] {
        let command = parsed_command([flag, "first", "second", "third"]);
        let mut header = Vec::new();
        let input = InputReport {
            path: Path::new("second"),
            index: 1,
            count: 3,
            bytes: 42,
        };

        print_detailed_header(
            &mut header,
            ReportMode::for_options(&command.options),
            &command,
            &input,
            Duration::ZERO,
        )
        .unwrap();

        let header = String::from_utf8(header).unwrap();
        assert!(header.contains(&format!("· {mode}\n")));
        assert!(header.contains("File     2 of 3\n"));
        assert!(header.contains("Input    \"second\" · 42 bytes"));
    }
}

#[test]
fn relaxed_mode_caution_colors_only_its_label() {
    let message = "Caution: strict mode disabled; enabling compact empty/singleton \
                   Huffman alphabets and the non-standard length-258 alias\n";

    let mut plain = Vec::new();
    print_strict_mode_caution(&mut plain, false).unwrap();
    assert_eq!(String::from_utf8(plain).unwrap(), message);

    let mut colored = Vec::new();
    print_strict_mode_caution(&mut colored, true).unwrap();
    assert_eq!(
        String::from_utf8(colored).unwrap(),
        message.replacen("Caution:", "\x1b[33mCaution:\x1b[0m", 1)
    );
}

#[test]
fn report_modes_have_one_consistent_output_channel() {
    let strict = parsed_command(["in"]);
    let default = parsed_command(["--strict", "0", "in"]);
    let verbose = parsed_command(["--strict", "0", "--verbose", "in"]);
    let visual = parsed_command(["--strict", "0", "--visual", "in"]);

    assert!(strict.options.strict);
    assert!(!default.options.strict);
    assert!(!verbose.options.strict);
    assert!(!visual.options.strict);

    let default = ReportMode::for_options(&default.options);
    assert_eq!(default, ReportMode::Default);
    assert_eq!(default.channel(), OutputChannel::Stderr);
    assert!(!default.detailed());
    assert!(!default.reports_timeout());

    let verbose = ReportMode::for_options(&verbose.options);
    assert_eq!(verbose, ReportMode::Verbose);
    assert_eq!(verbose.channel(), OutputChannel::Stdout);
    assert!(verbose.detailed());
    assert!(verbose.reports_timeout());

    let visual = ReportMode::for_options(&visual.options);
    assert_eq!(visual, ReportMode::Visual);
    assert_eq!(visual.channel(), OutputChannel::Stderr);
    assert!(visual.detailed());
    assert!(visual.reports_timeout());
}

#[test]
fn verbose_measurements_use_readable_units_and_change_wording() {
    assert_eq!(format_elapsed(Duration::ZERO), "0 µs");
    assert_eq!(format_elapsed(Duration::from_micros(999)), "999 µs");
    assert_eq!(format_elapsed(Duration::from_millis(1)), "1.00 ms");
    assert_eq!(format_elapsed(Duration::from_secs(1)), "1.00 s");

    assert_eq!(describe_byte_change(100, 90), "saved 10 bytes (10.00%)");
    assert_eq!(describe_byte_change(100, 99), "saved 1 byte (1.00%)");
    assert_eq!(describe_byte_change(347, 346), "saved 1 byte (0.29%)");
    assert_eq!(describe_byte_change(100, 100), "unchanged");
    assert_eq!(describe_byte_change(100, 101), "added 1 byte");
    assert_eq!(describe_byte_change(100, 111), "added 11 bytes");
}
