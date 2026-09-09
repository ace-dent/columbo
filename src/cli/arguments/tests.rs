// SPDX-License-Identifier: MIT

use crate::cli::test_support::{parsed_command, parsed_format, parsed_options};

use super::*;

#[test]
fn unknown_option_diagnostics_escape_terminal_controls() {
    let error = match parse_args([OsString::from("--bad\x1b[2J\noption")]) {
        Err(error) => error,
        Ok(_) => panic!("unknown option should fail"),
    };
    let mut output = Vec::new();
    print_cli_error(&mut output, error, false).unwrap();
    let output = String::from_utf8(output).unwrap();
    assert!(output.starts_with("unknown option: --bad\\u{1b}[2J\\noption\n"));
    assert!(!output.contains('\x1b'));
}

#[test]
fn timeout_is_clamped_and_rounded_up() {
    assert_eq!(
        parse_timeout(OsStr::new("0.1")),
        Some(Duration::from_secs(10))
    );
    assert_eq!(
        parse_timeout(OsStr::new("10.01")),
        Some(Duration::from_secs(11))
    );
    assert_eq!(
        parse_timeout(OsStr::new("9999")),
        Some(Duration::from_secs(4_000))
    );
    assert_eq!(parse_timeout(OsStr::new("1.0")), Some(MIN_TIMEOUT));
    assert_eq!(parse_timeout(OsStr::new("0")), None);
    assert_eq!(parse_timeout(OsStr::new("0.0")), None);
    assert_eq!(parse_timeout(OsStr::new("-1")), None);
    assert_eq!(parse_timeout(OsStr::new("+10")), None);
    assert_eq!(parse_timeout(OsStr::new("1e2")), None);
    assert_eq!(parse_timeout(OsStr::new("10.")), None);
    assert_eq!(parse_timeout(OsStr::new(".1")), None);
    assert_eq!(parse_timeout(OsStr::new("1.2.3")), None);
}

#[test]
fn strict_mode_defaults_on_and_accepts_only_zero_or_one() {
    assert!(parsed_options(["in"]).strict);
    assert!(!parsed_options(["--strict", "0", "in"]).strict);
    assert!(!parsed_options(["--strict=0", "in"]).strict);
    assert!(parsed_options(["--strict", "1", "in"]).strict);
    assert!(parsed_options(["--strict=1", "in"]).strict);

    for arguments in [
        vec!["--strict", "2", "in"],
        vec!["--strict", "true", "in"],
        vec!["--strict=+1", "in"],
        vec!["--strict=01", "in"],
        vec!["--strict=-0", "in"],
        vec!["--strict"],
    ] {
        let error = match parse_args(arguments.into_iter().map(OsString::from)) {
            Err(error) => error,
            Ok(_) => panic!("invalid strict value should fail"),
        };
        assert_eq!(error.message.as_deref(), Some("--strict requires 0 or 1"));
    }
}

#[test]
fn verbose_flags_enable_progress_reporting() {
    assert!(!parsed_options(["in"]).verbose);
    assert!(parsed_options(["-v", "in"]).verbose);
    assert!(parsed_options(["--verbose", "in"]).verbose);
}

#[test]
fn visual_mode_is_distinct_from_verbose_reporting() {
    assert!(!parsed_options(["in"]).visual);
    assert!(parsed_options(["--visual", "in"]).visual);

    let error = match parse_args(
        ["--visual", "--verbose", "in"]
            .into_iter()
            .map(OsString::from),
    ) {
        Err(error) => error,
        Ok(_) => panic!("visual and verbose modes should not be combined"),
    };
    assert_eq!(
        error.message.as_deref(),
        Some("--visual cannot be combined with --verbose")
    );
}

#[test]
fn output_defaults_in_place_and_accepts_oxipng_forms_for_one_input() {
    let default = parsed_command(["in"]);
    assert_eq!(default.inputs, [PathBuf::from("in")]);
    assert!(matches!(default.destination, Destination::InPlace));

    for command in [
        parsed_command(["--out", "out", "in"]),
        parsed_command(["in", "--out", "out"]),
        parsed_command(["--out=out", "in"]),
    ] {
        assert_eq!(command.inputs, [PathBuf::from("in")]);
        assert!(matches!(
            command.destination,
            Destination::Explicit(path) if path == Path::new("out")
        ));
    }

    assert!(matches!(
        parsed_command(["--out=-output", "in"]).destination,
        Destination::Explicit(path) if path == Path::new("-output")
    ));
    assert_eq!(
        parsed_command(["--", "-input"]).inputs,
        [PathBuf::from("-input")]
    );
}

#[test]
fn positional_paths_are_all_sequential_inputs() {
    let command = parsed_command(["one", "two", "three"]);
    assert_eq!(
        command.inputs,
        [
            PathBuf::from("one"),
            PathBuf::from("two"),
            PathBuf::from("three")
        ]
    );
    assert!(matches!(command.destination, Destination::InPlace));
}

#[test]
fn output_option_rejects_ambiguous_or_missing_values() {
    let cases = [
        (
            vec!["--out", "one", "--out", "two", "in"],
            "--out may only be specified once",
        ),
        (
            vec!["--out=one", "--out=two", "in"],
            "--out may only be specified once",
        ),
        (
            vec!["--out", "out", "in", "legacy"],
            "--out cannot be used when processing multiple input files",
        ),
        (vec!["--out"], "--out requires an output filename"),
        (
            vec!["--out", "--dry-run", "in"],
            "--out requires an output filename",
        ),
        (
            vec!["--out", "--help", "in"],
            "--out requires an output filename",
        ),
        (
            vec!["--out", "-output", "in"],
            "--out requires an output filename",
        ),
        (
            vec!["--out", "-", "in"],
            "--out requires an output filename",
        ),
        (vec!["--out=", "in"], "--out requires an output filename"),
    ];
    for (arguments, expected) in cases {
        let error = match parse_args(arguments.into_iter().map(OsString::from)) {
            Err(error) => error,
            Ok(_) => panic!("invalid output arguments should fail"),
        };
        assert_eq!(error.message.as_deref(), Some(expected));
    }
}

#[cfg(unix)]
#[test]
fn equals_form_preserves_a_non_utf8_output_path() {
    use std::os::unix::ffi::{OsStrExt, OsStringExt};

    let parsed = match parse_args([
        OsString::from_vec(b"--out=\xff".to_vec()),
        OsString::from("in"),
    ]) {
        Ok(parsed) => parsed,
        Err(_) => panic!("non-UTF-8 output path should parse"),
    };
    let command = match parsed {
        ParsedCommand::Run(command) => command,
        ParsedCommand::Help => panic!("expected run command"),
    };
    let Destination::Explicit(output) = command.destination else {
        panic!("expected explicit output");
    };
    assert_eq!(output.as_os_str().as_bytes(), b"\xff");
}

#[test]
fn dry_run_accepts_multiple_inputs_and_ignores_out_for_one_input() {
    for command in [
        parsed_command(["-d", "in"]),
        parsed_command(["--dry-run", "in"]),
        parsed_command(["-d", "-d", "in"]),
        parsed_command(["--dry-run", "--out", "ignored", "in"]),
        parsed_command(["in", "--out=ignored", "--dry-run"]),
    ] {
        assert_eq!(command.inputs, [PathBuf::from("in")]);
        assert!(command.destination.is_dry_run());
    }

    let batch = parsed_command(["--dry-run", "in", "out"]);
    assert_eq!(batch.inputs, [PathBuf::from("in"), PathBuf::from("out")]);
    assert!(batch.destination.is_dry_run());

    let output_error = match parse_args(
        ["--dry-run", "--out", "ignored", "in", "out"]
            .into_iter()
            .map(OsString::from),
    ) {
        Err(error) => error,
        Ok(_) => panic!("batch --out should fail"),
    };
    assert_eq!(
        output_error.message.as_deref(),
        Some("--out cannot be used when processing multiple input files")
    );
    assert!(output_error.show_usage);

    for arguments in [vec!["--dry-run"], Vec::new()] {
        let error = match parse_args(arguments.into_iter().map(OsString::from)) {
            Err(error) => error,
            Ok(_) => panic!("missing positional argument should fail"),
        };
        assert!(error.message.is_none());
        assert!(error.show_usage);
    }
}

#[test]
fn batch_inputs_accept_all_per_file_processing_options() {
    let command = parsed_command([
        "--dry-run",
        "--raw",
        "--strip",
        "--max",
        "--strict",
        "0",
        "--verbose",
        "--timeout",
        "10",
        "one",
        "two",
    ]);

    assert_eq!(command.format, Format::Raw);
    assert!(command.options.strip_metadata);
    assert!(command.options.exhaustive);
    assert!(!command.options.strict);
    assert!(command.options.verbose);
    assert_eq!(command.options.timeout, Duration::from_secs(10));
    assert!(command.destination.is_dry_run());
    assert_eq!(command.inputs, [PathBuf::from("one"), PathBuf::from("two")]);
}

#[test]
fn retired_cli_flags_are_rejected() {
    for option in [
        "--png",
        "--zlib",
        "--gzip",
        "--zip",
        "--mincodes",
        "--allow-258-alias",
        "--inspect",
    ] {
        let error = match parse_args([OsString::from(option), OsString::from("in")]) {
            Err(error) => error,
            Ok(_) => panic!("retired option should fail"),
        };
        assert_eq!(error.message, Some(format!("unknown option: {option}")));
    }
}

#[test]
fn help_describes_only_the_merged_strict_policy() {
    let mut help = Vec::new();
    print_usage(&mut help, false).unwrap();
    let help = String::from_utf8(help).unwrap();

    assert!(help.contains("--strict 0|1"));
    assert!(help.contains("default: 1"));
    assert!(!help.contains("--mincodes"));
    assert!(!help.contains("--allow-258-alias"));
    assert!(!help.contains("--inspect"));
    assert!(!help.contains("--png"));
    assert!(!help.contains("--zlib"));
    assert!(!help.contains("--gzip"));
    assert!(!help.contains("--zip"));
    assert!(help.contains("usage: columbo [options] input [input ...]"));
    assert!(help.contains("columbo [options] --out file input"));
    assert!(help.contains("-d, --dry-run"));
    assert!(help.contains("--out <file>"));
    assert!(help.contains("instead of optimizing it in place"));
    assert!(help.contains("without writing output"));
    assert!(help.contains("Multiple inputs are processed sequentially"));
    assert!(help.contains("--out accepts only one input"));
    assert!(help.contains("meaningful Deflate bit is saved"));
    assert!(help.contains("Advanced:"));
    assert!(help.contains("--raw"));
    assert!(help.contains("ordered route timings, bit gains, and block choices"));
    assert!(help.contains("--visual"));
    assert!(help.contains("ordered Deflate block maps with aligned in/out rows"));
    assert!(!help.contains('\x1b'));

    let mut colored_help = Vec::new();
    print_usage(&mut colored_help, true).unwrap();
    let colored_help = String::from_utf8(colored_help).unwrap();
    assert!(colored_help.contains("\x1b[1mcolumbo"));
    assert!(!colored_help.contains("38;5"));
}

#[test]
fn input_format_defaults_to_auto() {
    assert_eq!(parsed_format(["in"]), Format::Auto);
}

#[test]
fn raw_is_an_idempotent_advanced_override() {
    assert_eq!(parsed_format(["--raw", "in"]), Format::Raw);
    assert_eq!(parsed_format(["in", "--raw", "--raw"]), Format::Raw);
}
