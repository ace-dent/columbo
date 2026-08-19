// SPDX-License-Identifier: MIT

#![forbid(unsafe_code)]

mod presentation;
mod terminal;

use std::env;
use std::ffi::{OsStr, OsString};
use std::fs::{self, File, OpenOptions};
use std::io::{self, IsTerminal, Read, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use columbo::{
    optimize, Format, Options, MAX_EXPANSION_RATIO, MAX_TIMEOUT, MIN_EXPANSION_LIMIT_BYTES,
    MIN_TIMEOUT,
};
use presentation::{
    countdown_seconds, format_duration as format_elapsed, plural, plural_u64, write_spinner_line,
};

const PROGRAM_NAME: &str = "columbo";
const PROGRAM_VERSION: &str = concat!(
    env!("CARGO_PKG_VERSION_MAJOR"),
    ".",
    env!("CARGO_PKG_VERSION_MINOR")
);
const PROGRAM_STAGE: &str = "Beta";
const READ_BUFFER_BYTES: usize = 64 * 1024;
const TEMP_FILE_ATTEMPTS: usize = 128;
static TEMP_FILE_COUNTER: AtomicU64 = AtomicU64::new(0);

struct Command {
    format: Format,
    options: Options,
    inputs: Vec<PathBuf>,
    destination: Destination,
}

enum Destination {
    InPlace,
    Explicit(PathBuf),
    DryRun,
}

impl Destination {
    fn is_dry_run(&self) -> bool {
        matches!(self, Self::DryRun)
    }

    fn output_path<'a>(&'a self, input: &'a Path) -> Option<&'a Path> {
        match self {
            Self::InPlace => Some(input),
            Self::Explicit(path) => Some(path),
            Self::DryRun => None,
        }
    }

    fn overwrites_input(&self, input: &Path) -> bool {
        match self {
            Self::InPlace => true,
            Self::Explicit(path) => paths_refer_to_same_file(input, path),
            Self::DryRun => false,
        }
    }
}

enum OutputAction {
    DryRun,
    WrittenOptimized(PathBuf),
    CopiedOriginal(PathBuf),
    Preserved(PathBuf),
}

impl OutputAction {
    fn wrote_file(&self) -> bool {
        matches!(self, Self::WrittenOptimized(_) | Self::CopiedOriginal(_))
    }
}

struct ExecutionTimings {
    read: Duration,
    optimize: Duration,
    write: Duration,
    total: Duration,
}

struct InputReport<'a> {
    path: &'a Path,
    index: usize,
    count: usize,
    bytes: usize,
}

struct OptimizationReport {
    bytes: usize,
    bits_saved: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OutputChannel {
    Stdout,
    Stderr,
}

impl OutputChannel {
    fn color_enabled(self) -> bool {
        match self {
            Self::Stdout => terminal::stdout_color_enabled(),
            Self::Stderr => terminal::stderr_color_enabled(),
        }
    }

    fn write(self, operation: impl FnOnce(&mut dyn Write) -> io::Result<()>) {
        let result = match self {
            Self::Stdout => operation(&mut io::stdout().lock()),
            Self::Stderr => operation(&mut io::stderr().lock()),
        };
        let _ = result;
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ReportMode {
    Default,
    Verbose,
    Visual,
}

impl ReportMode {
    fn for_options(options: &Options) -> Self {
        if options.visual {
            Self::Visual
        } else if options.verbose {
            Self::Verbose
        } else {
            Self::Default
        }
    }

    fn channel(self) -> OutputChannel {
        match self {
            Self::Verbose => OutputChannel::Stdout,
            Self::Default | Self::Visual => OutputChannel::Stderr,
        }
    }

    fn detailed(self) -> bool {
        !matches!(self, Self::Default)
    }

    fn label(self) -> &'static str {
        match self {
            Self::Default => "default",
            Self::Verbose => "verbose",
            Self::Visual => "visual",
        }
    }

    fn reports_timeout(self) -> bool {
        self.detailed()
    }
}

enum ParsedCommand {
    Help,
    Run(Command),
}

struct CliError {
    message: Option<String>,
    show_usage: bool,
}

impl CliError {
    fn message(message: impl Into<String>, show_usage: bool) -> Self {
        Self {
            message: Some(message.into()),
            show_usage,
        }
    }

    fn usage() -> Self {
        Self {
            message: None,
            show_usage: true,
        }
    }
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(code) => ExitCode::from(code),
    }
}

fn run() -> std::result::Result<(), u8> {
    let command = match parse_args(env::args_os().skip(1)) {
        Ok(ParsedCommand::Help) => {
            OutputChannel::Stdout
                .write(|output| print_usage(output, OutputChannel::Stdout.color_enabled()));
            return Ok(());
        }
        Ok(ParsedCommand::Run(command)) => command,
        Err(error) => {
            OutputChannel::Stderr.write(|output| {
                print_cli_error(output, error, OutputChannel::Stderr.color_enabled())
            });
            return Err(2);
        }
    };

    execute(command)
}

fn execute(command: Command) -> std::result::Result<(), u8> {
    let report_mode = ReportMode::for_options(&command.options);
    if command.options.visual && !visual_terminal_available() {
        eprintln!("visual mode needs an interactive terminal; continuing without stream maps");
    }

    let input_count = command.inputs.len();
    let mut failed = false;
    let mut caution_printed = false;
    for (index, input) in command.inputs.iter().enumerate() {
        // Each input owns its complete read/optimize/write lifetime. Besides
        // making the processing order explicit, this releases potentially
        // large buffers before the next file starts.
        if execute_file(
            &command,
            report_mode,
            input,
            index,
            input_count,
            &mut caution_printed,
        )
        .is_err()
        {
            failed = true;
        }
    }

    if failed {
        Err(1)
    } else {
        Ok(())
    }
}

fn execute_file(
    command: &Command,
    report_mode: ReportMode,
    input_path: &Path,
    input_index: usize,
    input_count: usize,
    caution_printed: &mut bool,
) -> std::result::Result<(), u8> {
    let overwrites_input = command.destination.overwrites_input(input_path);
    let detailed = report_mode.detailed();
    let total_started = detailed.then(Instant::now);
    let read_started = detailed.then(Instant::now);
    let input = match read_file(input_path, command.options.max_input_bytes) {
        Ok(bytes) => bytes,
        Err(ReadError::TooLarge) => {
            eprintln!("input {:?} exceeds the 1 GiB file-size limit", input_path);
            return Err(1);
        }
        Err(ReadError::Allocation) => {
            eprintln!("not enough memory to read input {:?}", input_path);
            return Err(1);
        }
        Err(ReadError::Io) => {
            // Path's Debug formatter escapes terminal control characters.
            eprintln!("could not read {:?}", input_path);
            return Err(1);
        }
    };
    let read_elapsed = read_started.map_or(Duration::ZERO, |started| started.elapsed());
    let input_report = InputReport {
        path: input_path,
        index: input_index,
        count: input_count,
        bytes: input.len(),
    };

    if detailed {
        report_mode.channel().write(|output| {
            print_detailed_header(output, report_mode, command, &input_report, read_elapsed)
        });
    }

    // Strictness is a batch-wide policy, so one caution is sufficient even
    // though each input receives its own detailed header and result.
    if !command.options.strict && !*caution_printed {
        let channel = report_mode.channel();
        channel.write(|output| print_strict_mode_caution(output, channel.color_enabled()));
        *caution_printed = true;
    }

    let optimize_started = detailed.then(Instant::now);
    let mut spinner = Spinner::start(report_mode == ReportMode::Default, command.options.timeout);
    let result = optimize(&input, command.format, &command.options);
    spinner.stop();
    let optimize_elapsed = optimize_started.map_or(Duration::ZERO, |started| started.elapsed());
    let optimized = match result {
        Ok(result) => result,
        Err(error) => {
            eprintln!("could not optimize {:?}: {error}", input_path);
            return Err(1);
        }
    };

    let write_started = detailed.then(Instant::now);
    let output_action = match command.destination.output_path(input_path) {
        None => OutputAction::DryRun,
        Some(output) if optimized.bits_saved != 0 => {
            let written = if overwrites_input {
                write_file_if_unchanged(output, &input, &optimized.data)
            } else {
                write_file(output, &optimized.data).map(|()| true)
            };
            match written {
                Ok(true) => {}
                Ok(false) => {
                    eprintln!(
                        "input changed while it was being optimized; left {:?} unchanged",
                        output
                    );
                    return Err(1);
                }
                Err(_) => {
                    eprintln!("could not write {:?}", output);
                    return Err(1);
                }
            }
            OutputAction::WrittenOptimized(output.to_path_buf())
        }
        Some(output) if matches!(&command.destination, Destination::InPlace) => {
            OutputAction::Preserved(output.to_path_buf())
        }
        Some(output) => match output_entry_exists(output) {
            Ok(true) => OutputAction::Preserved(output.to_path_buf()),
            Ok(false) => match write_new_file(output, &input) {
                Ok(true) => OutputAction::CopiedOriginal(output.to_path_buf()),
                // Another writer won the race after the existence check.
                Ok(false) => OutputAction::Preserved(output.to_path_buf()),
                Err(_) => {
                    eprintln!("could not write {:?}", output);
                    return Err(1);
                }
            },
            Err(_) => {
                eprintln!("could not inspect {:?}", output);
                return Err(1);
            }
        },
    };
    let write_elapsed = if output_action.wrote_file() {
        write_started.map_or(Duration::ZERO, |started| started.elapsed())
    } else {
        Duration::ZERO
    };

    if optimized.timed_out && report_mode.reports_timeout() {
        report_mode
            .channel()
            .write(|output| print_timeout_notice(output, &output_action, command.options.timeout));
    }
    let timings = ExecutionTimings {
        read: read_elapsed,
        optimize: optimize_elapsed,
        write: write_elapsed,
        total: total_started.map_or(Duration::ZERO, |started| started.elapsed()),
    };
    let optimization_report = OptimizationReport {
        bytes: optimized.data.len(),
        bits_saved: optimized.bits_saved,
    };
    report_mode.channel().write(|output| {
        print_result(
            output,
            report_mode,
            &input_report,
            &output_action,
            &optimization_report,
            &timings,
        )
    });
    Ok(())
}

fn print_result(
    output: &mut dyn Write,
    report_mode: ReportMode,
    input: &InputReport<'_>,
    action: &OutputAction,
    optimized: &OptimizationReport,
    timings: &ExecutionTimings,
) -> io::Result<()> {
    match report_mode {
        ReportMode::Default => print_quiet_result(output, input, action, optimized),
        ReportMode::Verbose | ReportMode::Visual => {
            print_detailed_result(output, input, action, optimized, timings)
        }
    }
}

fn print_quiet_result(
    output: &mut dyn Write,
    input: &InputReport<'_>,
    action: &OutputAction,
    optimized: &OptimizationReport,
) -> io::Result<()> {
    let input_name = display_name(input.path);
    let input_bytes = input.bytes;
    let optimized_bytes = optimized.bytes;
    let bits_saved = optimized.bits_saved;
    match action {
        OutputAction::DryRun => writeln!(
            output,
            "{input_name:?} {input_bytes} -> {optimized_bytes} bytes (dry run; no output written)"
        ),
        OutputAction::WrittenOptimized(_) if input_bytes == optimized_bytes => writeln!(
            output,
            "{input_name:?} {input_bytes} -> {optimized_bytes} bytes (saved {bits_saved} meaningful {})",
            plural_u64(bits_saved, "bit", "bits")
        ),
        OutputAction::WrittenOptimized(_) => writeln!(
            output,
            "{input_name:?} {input_bytes} -> {optimized_bytes} bytes"
        ),
        OutputAction::CopiedOriginal(output_path) => writeln!(
            output,
            "{input_name:?} {input_bytes} -> {input_bytes} bytes \
             (no savings; copied original to {:?})",
            output_path
        ),
        OutputAction::Preserved(output_path) => writeln!(
            output,
            "{input_name:?} {input_bytes} -> {input_bytes} bytes \
             (no savings; left {:?} unchanged)",
            output_path
        ),
    }
}

fn print_timeout_notice(
    output: &mut dyn Write,
    action: &OutputAction,
    timeout: Duration,
) -> io::Result<()> {
    let seconds = timeout.as_secs();
    match action {
        OutputAction::DryRun => writeln!(
            output,
            "Timeout triggered after {seconds} seconds; reporting best result found so far."
        ),
        OutputAction::WrittenOptimized(_) => writeln!(
            output,
            "Timeout triggered after {seconds} seconds; wrote best output found so far."
        ),
        OutputAction::CopiedOriginal(_) | OutputAction::Preserved(_) => writeln!(
            output,
            "Timeout triggered after {seconds} seconds; no smaller output was written."
        ),
    }
}

fn print_detailed_header(
    output: &mut dyn Write,
    report_mode: ReportMode,
    command: &Command,
    input: &InputReport<'_>,
    read_elapsed: Duration,
) -> io::Result<()> {
    let input_name = display_name(input.path);
    writeln!(output)?;
    let title = format!(
        "{PROGRAM_NAME} v{PROGRAM_VERSION} {PROGRAM_STAGE} · {}",
        report_mode.label()
    );
    writeln!(output, "{title}")?;
    writeln!(output, "{}", "─".repeat(title.chars().count()))?;
    if input.count > 1 {
        writeln!(output, "File     {} of {}", input.index + 1, input.count)?;
    }
    writeln!(
        output,
        "Input    {:?} · {} {} · read {}",
        input_name,
        input.bytes,
        plural(input.bytes, "byte", "bytes"),
        format_elapsed(read_elapsed)
    )?;
    let strictness = if command.options.strict {
        "strict"
    } else {
        "relaxed"
    };
    let dry_run = if command.destination.is_dry_run() {
        " · dry run"
    } else {
        ""
    };
    if command.options.exhaustive {
        writeln!(
            output,
            "Mode     max · {strictness}{dry_run} · {} file-wide budget",
            format_elapsed(command.options.timeout),
        )?;
    } else {
        writeln!(output, "Mode     normal · {strictness}{dry_run}")?;
    }
    Ok(())
}

fn print_strict_mode_caution(output: &mut dyn Write, color: bool) -> io::Result<()> {
    let (yellow, reset) = if color {
        ("\x1b[33m", "\x1b[0m")
    } else {
        ("", "")
    };
    writeln!(
        output,
        "{yellow}Caution:{reset} strict mode disabled; enabling compact empty/singleton \
         Huffman alphabets and the non-standard length-258 alias"
    )
}

fn print_detailed_result(
    output: &mut dyn Write,
    input: &InputReport<'_>,
    action: &OutputAction,
    optimized: &OptimizationReport,
    timings: &ExecutionTimings,
) -> io::Result<()> {
    writeln!(output)?;
    writeln!(output, "Result")?;
    match action {
        OutputAction::WrittenOptimized(path) => {
            let output_name = display_name(path);
            writeln!(output, "  Output  {:?}", output_name)?;
        }
        OutputAction::CopiedOriginal(path) => {
            let output_name = display_name(path);
            writeln!(
                output,
                "  Output  {:?} · original copied · no savings",
                output_name
            )?;
        }
        OutputAction::Preserved(path) => {
            let output_name = display_name(path);
            writeln!(
                output,
                "  Output  {:?} · preserved · no savings",
                output_name
            )?;
        }
        OutputAction::DryRun => writeln!(output, "  Output  not written · dry run")?,
    }
    writeln!(
        output,
        "  Size    {} → {} {} · {}",
        input.bytes,
        optimized.bytes,
        plural(optimized.bytes, "byte", "bytes"),
        describe_optimization_change(input.bytes, optimized.bytes, optimized.bits_saved)
    )?;
    if !action.wrote_file() {
        writeln!(
            output,
            "  Time    {} total · read {} · optimize {}",
            format_elapsed(timings.total),
            format_elapsed(timings.read),
            format_elapsed(timings.optimize),
        )
    } else {
        writeln!(
            output,
            "  Time    {} total · read {} · optimize {} · write {}",
            format_elapsed(timings.total),
            format_elapsed(timings.read),
            format_elapsed(timings.optimize),
            format_elapsed(timings.write)
        )
    }
}

fn display_name(path: &Path) -> &OsStr {
    path.file_name().unwrap_or(path.as_os_str())
}

fn describe_optimization_change(
    input_bytes: usize,
    output_bytes: usize,
    bits_saved: u64,
) -> String {
    if input_bytes == output_bytes && bits_saved != 0 {
        format!(
            "saved {bits_saved} meaningful {}",
            plural_u64(bits_saved, "bit", "bits")
        )
    } else {
        describe_byte_change(input_bytes, output_bytes)
    }
}

fn describe_byte_change(input_bytes: usize, output_bytes: usize) -> String {
    match input_bytes.cmp(&output_bytes) {
        std::cmp::Ordering::Greater => {
            let saved = input_bytes - output_bytes;
            // CLI inputs are capped at 1 GiB, so basis-point arithmetic is
            // exact here and avoids pulling floating-point formatting into
            // speed-first distribution binaries.
            let percentage_hundredths = (saved * 10_000 + input_bytes / 2) / input_bytes;
            format!(
                "saved {saved} {} ({}.{:02}%)",
                plural(saved, "byte", "bytes"),
                percentage_hundredths / 100,
                percentage_hundredths % 100
            )
        }
        std::cmp::Ordering::Less => {
            let added = output_bytes - input_bytes;
            format!("added {added} {}", plural(added, "byte", "bytes"))
        }
        std::cmp::Ordering::Equal => "unchanged".to_owned(),
    }
}

fn parse_args(arguments: impl IntoIterator<Item = OsString>) -> Result<ParsedCommand, CliError> {
    let arguments: Vec<OsString> = arguments.into_iter().collect();
    let mut options = Options::default();
    let mut format = Format::Auto;
    let mut dry_run = false;
    let mut output = None;
    let mut positional = Vec::new();
    let mut parse_options = true;
    let mut index = 0_usize;

    while index < arguments.len() {
        if parse_options && arguments[index] == OsStr::new("--") {
            parse_options = false;
            index += 1;
            continue;
        }
        if !parse_options || !starts_with_dash(&arguments[index]) {
            positional.push(arguments[index].clone());
            index += 1;
            continue;
        }

        if let Some(value) = equals_output_value(&arguments[index]) {
            set_output(&mut output, value)?;
            index += 1;
            continue;
        }

        let argument = arguments[index].to_string_lossy();
        match argument.as_ref() {
            "-h" | "--help" => return Ok(ParsedCommand::Help),
            "--raw" => format = Format::Raw,
            "-v" | "--verbose" => options.verbose = true,
            "--visual" => options.visual = true,
            "-m" | "--max" => options.exhaustive = true,
            "-d" | "--dry-run" => dry_run = true,
            "--out" => {
                index += 1;
                let value = arguments.get(index).ok_or_else(output_error)?;
                if starts_with_dash(value) {
                    return Err(output_error());
                }
                set_output(&mut output, value.clone())?;
            }
            "--strip" => options.strip_metadata = true,
            "-t" | "--timeout" => {
                index += 1;
                let value = arguments
                    .get(index)
                    .ok_or_else(timeout_error)
                    .and_then(|value| parse_timeout(value).ok_or_else(timeout_error))?;
                options.timeout = value;
            }
            "--strict" => {
                index += 1;
                options.strict = arguments
                    .get(index)
                    .ok_or_else(strict_error)
                    .and_then(|value| parse_strict(value).ok_or_else(strict_error))?;
            }
            _ if argument.starts_with("--timeout=") => {
                options.timeout =
                    parse_timeout(OsStr::new(&argument[10..])).ok_or_else(timeout_error)?;
            }
            _ if argument.starts_with("--strict=") => {
                options.strict =
                    parse_strict(OsStr::new(&argument[9..])).ok_or_else(strict_error)?;
            }
            _ => {
                return Err(CliError::message(
                    format!("unknown option: {}", arguments[index].to_string_lossy()),
                    true,
                ));
            }
        }
        index += 1;
    }

    if options.verbose && options.visual {
        return Err(CliError::message(
            "--visual cannot be combined with --verbose",
            false,
        ));
    }

    if positional.is_empty() {
        return Err(CliError::usage());
    }
    if positional.len() > 1 && output.is_some() {
        return Err(CliError::message(
            "--out cannot be used when processing multiple input files",
            true,
        ));
    }

    let inputs = positional.into_iter().map(PathBuf::from).collect();
    let destination = if dry_run {
        Destination::DryRun
    } else {
        output.map_or(Destination::InPlace, Destination::Explicit)
    };

    Ok(ParsedCommand::Run(Command {
        format,
        options,
        inputs,
        destination,
    }))
}

fn set_output(output: &mut Option<PathBuf>, value: OsString) -> Result<(), CliError> {
    if output.is_some() {
        return Err(CliError::message("--out may only be specified once", false));
    }
    if value.is_empty() {
        return Err(output_error());
    }
    *output = Some(PathBuf::from(value));
    Ok(())
}

#[cfg(unix)]
fn equals_output_value(argument: &OsStr) -> Option<OsString> {
    use std::os::unix::ffi::{OsStrExt, OsStringExt};

    argument
        .as_bytes()
        .strip_prefix(b"--out=")
        .map(|value| OsString::from_vec(value.to_vec()))
}

#[cfg(windows)]
fn equals_output_value(argument: &OsStr) -> Option<OsString> {
    use std::os::windows::ffi::{OsStrExt, OsStringExt};

    const PREFIX: &[u16] = &[
        b'-' as u16,
        b'-' as u16,
        b'o' as u16,
        b'u' as u16,
        b't' as u16,
        b'=' as u16,
    ];
    let encoded: Vec<u16> = argument.encode_wide().collect();
    encoded.strip_prefix(PREFIX).map(OsString::from_wide)
}

#[cfg(not(any(unix, windows)))]
fn equals_output_value(argument: &OsStr) -> Option<OsString> {
    argument
        .to_string_lossy()
        .strip_prefix("--out=")
        .map(OsString::from)
}

fn starts_with_dash(value: &OsStr) -> bool {
    value.to_string_lossy().starts_with('-')
}

fn parse_timeout(value: &OsStr) -> Option<Duration> {
    // strtod(), used by the original Columbo C CLI, accepts leading but not
    // trailing space.
    let text = value.to_str()?.trim_start();
    let seconds: f64 = text.parse().ok()?;
    if !seconds.is_finite() || seconds < 0.0 {
        return None;
    }
    let seconds = seconds
        .clamp(MIN_TIMEOUT.as_secs_f64(), MAX_TIMEOUT.as_secs_f64())
        .ceil() as u64;
    Some(Duration::from_secs(seconds))
}

fn parse_strict(value: &OsStr) -> Option<bool> {
    match value.to_str()?.trim_start().parse::<i64>().ok()? {
        0 => Some(false),
        1 => Some(true),
        _ => None,
    }
}

fn timeout_error() -> CliError {
    CliError::message("--timeout requires a non-negative number of seconds", false)
}

fn strict_error() -> CliError {
    CliError::message("--strict requires 0 or 1", false)
}

fn output_error() -> CliError {
    CliError::message("--out requires an output filename", false)
}

fn print_cli_error(output: &mut dyn Write, error: CliError, color: bool) -> io::Result<()> {
    if let Some(message) = error.message {
        writeln!(output, "{message}")?;
    }
    if error.show_usage {
        print_usage(output, color)?;
    }
    Ok(())
}

fn print_usage(output: &mut dyn Write, color: bool) -> io::Result<()> {
    writeln!(
        output,
        "🕵🏻‍♂️  {}{PROGRAM_NAME} v{PROGRAM_VERSION} {PROGRAM_STAGE}{}",
        if color { "\x1b[1m" } else { "" },
        if color { "\x1b[0m" } else { "" },
    )?;
    writeln!(
        output,
        "\"Just One More Thing\" - optimize the last few bytes in Deflate streams."
    )?;
    writeln!(
        output,
        "usage: {} [options] input [input ...]",
        PROGRAM_NAME
    )?;
    writeln!(output, "       {} [options] --out file input", PROGRAM_NAME)?;
    writeln!(output)?;
    writeln!(output, "Options:")?;
    writeln!(output, "  -h, --help             show this help and exit")?;
    writeln!(
        output,
        "  -v, --verbose          show ordered route timings, bit gains, and block choices"
    )?;
    writeln!(
        output,
        "      --visual           show ordered Deflate block maps with aligned in/out rows"
    )?;
    writeln!(
        output,
        "  -m, --max              enable slower byte-seeking searches"
    )?;
    writeln!(
        output,
        "  -d, --dry-run          fully optimize and report savings without writing output"
    )?;
    writeln!(
        output,
        "      --out <file>       write one input to file instead of optimizing it in place"
    )?;
    writeln!(
        output,
        concat!(
            "  -t, --timeout          stop starting search routes after this many seconds",
            "\n                         (default: 180; range: 10..4000;",
            " fractions round up;",
            "\n                         active route grace: 10% + 1 second)"
        )
    )?;
    writeln!(
        output,
        concat!(
            "      --strict 0|1       emit conservative Deflate for strict and old",
            "\n                         decoders (default: 1); 0 permits compact",
            "\n                         empty/singleton Huffman alphabets and the",
            "\n                         non-standard 258 alias"
        )
    )?;
    writeln!(
        output,
        "      --strip            strip metadata, comments, and embedded credentials"
    )?;
    writeln!(output)?;
    writeln!(output, "Advanced:")?;
    writeln!(
        output,
        "      --raw              force input to be treated as raw Deflate"
    )?;
    writeln!(output)?;
    writeln!(
        output,
        "By default, PNG/GZIP/ZIP metadata and comments are preserved."
    )?;
    writeln!(
        output,
        "Multiple inputs are processed sequentially; --out accepts only one input."
    )?;
    writeln!(
        output,
        "Existing files are replaced only when byte size decreases or at least one"
    )?;
    writeln!(output, "meaningful Deflate bit is saved.")?;
    writeln!(
        output,
        "Input and decoded Deflate data are limited to 1 GiB."
    )?;
    writeln!(
        output,
        "Decoded data is also limited to {MAX_EXPANSION_RATIO}x input size after a {} MiB allowance.",
        MIN_EXPANSION_LIMIT_BYTES / (1024 * 1024)
    )
}

#[derive(Debug, PartialEq, Eq)]
enum ReadError {
    Io,
    TooLarge,
    Allocation,
}

fn read_file(path: &Path, maximum_size: u64) -> Result<Vec<u8>, ReadError> {
    let file = File::open(path).map_err(|_| ReadError::Io)?;
    if file.metadata().map_err(|_| ReadError::Io)?.len() > maximum_size {
        return Err(ReadError::TooLarge);
    }

    read_bounded(file, maximum_size)
}

/// Read through a fixed stack buffer so allocation failure is recoverable.
///
/// `Read::read_to_end` grows its destination internally. Explicit fallible
/// reservations keep a hostile size-changing or seekless input from turning
/// memory pressure into an allocation panic or abort.
fn read_bounded(reader: impl Read, maximum_size: u64) -> Result<Vec<u8>, ReadError> {
    let mut bytes = Vec::new();
    let mut reader = reader.take(maximum_size.saturating_add(1));
    let mut buffer = [0_u8; READ_BUFFER_BYTES];
    loop {
        let count = reader.read(&mut buffer).map_err(|_| ReadError::Io)?;
        if count == 0 {
            break;
        }
        let new_length = u64::try_from(bytes.len())
            .ok()
            .and_then(|length| length.checked_add(count as u64))
            .ok_or(ReadError::TooLarge)?;
        if new_length > maximum_size {
            return Err(ReadError::TooLarge);
        }
        bytes
            .try_reserve(count)
            .map_err(|_| ReadError::Allocation)?;
        bytes.extend_from_slice(&buffer[..count]);
    }
    Ok(bytes)
}

/// Commit output through a new sibling file.
///
/// `create_new` prevents a symlink race on the temporary name, and the final
/// rename replaces the directory entry rather than following a destination
/// symlink. A failed write or pre-commit sync leaves any existing output
/// untouched.
fn write_file(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let (mut temporary, file) = stage_private_output(path, bytes)?;
    commit_temporary_output(path, &mut temporary, file)
}

/// Recheck that an input still contains the snapshot that was optimized. The
/// comparison happens after staging, immediately before the atomic rename, so
/// a long optimization cannot silently discard a newer source version. Like
/// any portable path check followed by rename, a very small race remains.
fn write_file_if_unchanged(path: &Path, expected: &[u8], bytes: &[u8]) -> io::Result<bool> {
    let (mut temporary, file) = stage_private_output(path, bytes)?;
    if !file_contents_equal(path, expected)? {
        return Ok(false);
    }
    commit_temporary_output(path, &mut temporary, file)?;
    Ok(true)
}

fn file_contents_equal(path: &Path, expected: &[u8]) -> io::Result<bool> {
    let mut file = match File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error),
    };
    if file.metadata()?.len() != expected.len() as u64 {
        return Ok(false);
    }

    let mut buffer = [0_u8; READ_BUFFER_BYTES];
    let mut position = 0_usize;
    while position < expected.len() {
        let count = file.read(&mut buffer)?;
        if count == 0 || expected[position..].get(..count) != Some(&buffer[..count]) {
            return Ok(false);
        }
        position += count;
    }
    Ok(file.read(&mut buffer[..1])? == 0)
}

/// Install a synced sibling only if no directory entry exists at `path`.
///
/// A hard link provides an atomic no-clobber commit: unlike `rename`, it fails
/// when another process creates the destination between the caller's
/// existence check and this commit. The sibling lives on the same filesystem,
/// so a successful link cannot fail because of a cross-device boundary.
fn write_new_file(path: &Path, bytes: &[u8]) -> io::Result<bool> {
    let (mut temporary, file) = stage_private_output(path, bytes)?;
    drop(file);
    match fs::hard_link(&temporary.path, path) {
        Ok(()) => {
            fs::remove_file(&temporary.path)?;
            temporary.committed = true;
            Ok(true)
        }
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => Ok(false),
        Err(error) => Err(error),
    }
}

fn output_entry_exists(path: &Path) -> io::Result<bool> {
    match fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error),
    }
}

/// Write and sync a collision-safe sibling before replacing the destination.
///
/// Unix builds force mode `0600` throughout staging. Keeping this phase
/// separate from the final commit avoids exposing partial data through the
/// destination's ordinary access bits.
/// Platform ACLs and Windows security descriptors are outside Rust's standard
/// file API: a sibling can inherit them from its directory, and atomic rename
/// installs the sibling's metadata instead of preserving the destination ACL.
/// Extended authorization can therefore change when replacing an existing
/// file; ACL-sensitive callers should use a new output path and a native,
/// policy-aware replacement step.
fn stage_private_output(path: &Path, bytes: &[u8]) -> io::Result<(TemporaryOutput, File)> {
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    if path.file_name().is_none() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "output has no file name",
        ));
    }

    let mut opened = None;
    for _ in 0..TEMP_FILE_ATTEMPTS {
        let sequence = TEMP_FILE_COUNTER.fetch_add(1, Ordering::Relaxed);
        let Some(temporary_path) = temporary_output_candidate(path, parent, sequence) else {
            continue;
        };
        match create_temporary_output(&temporary_path) {
            Ok(file) => {
                opened = Some((temporary_path, file));
                break;
            }
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error),
        }
    }
    let (temporary_path, mut file) = opened.ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::AlreadyExists,
            "could not create a unique temporary output",
        )
    })?;
    let temporary = TemporaryOutput::new(temporary_path);
    file.write_all(bytes)?;
    file.sync_all()?;
    Ok((temporary, file))
}

fn temporary_output_candidate(path: &Path, parent: &Path, sequence: u64) -> Option<PathBuf> {
    let temporary_name = OsString::from(format!(".columbo-{}-{sequence}.tmp", std::process::id()));
    let candidate = parent.join(temporary_name);
    (candidate.file_name() != path.file_name()).then_some(candidate)
}

fn paths_refer_to_same_file(left: &Path, right: &Path) -> bool {
    if left == right {
        return true;
    }

    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;

        if let (Ok(left), Ok(right)) = (fs::metadata(left), fs::metadata(right)) {
            return left.dev() == right.dev() && left.ino() == right.ino();
        }
    }

    matches!(
        (fs::canonicalize(left), fs::canonicalize(right)),
        (Ok(left), Ok(right)) if left == right
    )
}

/// Atomically install a synced private sibling.
///
/// Unix permits renaming an open file. Its handle is deliberately retained so
/// a path substitution after the rename cannot redirect the permission update.
#[cfg(unix)]
fn commit_temporary_output(
    path: &Path,
    temporary: &mut TemporaryOutput,
    file: File,
) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;

    let destination_mode = existing_regular_output_mode(path)?;
    fs::rename(&temporary.path, path)?;
    temporary.committed = true;

    if let Some(mode) = destination_mode {
        file.set_permissions(fs::Permissions::from_mode(mode))?;
        // The data was synced while private. Sync again after chmod so success
        // also means the restored access bits have reached the filesystem.
        file.sync_all()?;
    }
    Ok(())
}

/// Close before rename on platforms such as Windows, preserving prior
/// behavior where an open handle cannot be atomically moved over the output.
#[cfg(not(unix))]
fn commit_temporary_output(
    path: &Path,
    temporary: &mut TemporaryOutput,
    file: File,
) -> io::Result<()> {
    drop(file);
    fs::rename(&temporary.path, path)?;
    temporary.committed = true;
    Ok(())
}

#[cfg(unix)]
fn create_temporary_output(path: &Path) -> io::Result<File> {
    use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};

    let mut options = OpenOptions::new();
    options.write(true).create_new(true).mode(0o600);
    // Do not expose partially written data through the temporary name in a
    // shared directory. Existing destination access bits are restored only
    // after the private file has been renamed; a new output remains private.
    let file = options.open(path)?;
    if let Err(error) = file.set_permissions(fs::Permissions::from_mode(0o600)) {
        drop(file);
        let _ = fs::remove_file(path);
        return Err(error);
    }
    Ok(file)
}

#[cfg(not(unix))]
fn create_temporary_output(path: &Path) -> io::Result<File> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    options.open(path)
}

/// Preserve ordinary Unix access bits when replacing an existing output.
/// Special mode bits are intentionally not copied to newly written data.
#[cfg(unix)]
fn existing_regular_output_mode(path: &Path) -> io::Result<Option<u32>> {
    use std::os::unix::fs::PermissionsExt;

    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_file() => {
            Ok(Some(metadata.permissions().mode() & 0o777))
        }
        Ok(_) => Ok(None),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

struct TemporaryOutput {
    path: PathBuf,
    committed: bool,
}

impl TemporaryOutput {
    fn new(path: PathBuf) -> Self {
        Self {
            path,
            committed: false,
        }
    }
}

impl Drop for TemporaryOutput {
    fn drop(&mut self) {
        if !self.committed {
            let _ = fs::remove_file(&self.path);
        }
    }
}

struct Spinner {
    running: Option<Arc<AtomicBool>>,
    worker: Option<JoinHandle<()>>,
}

fn visual_terminal_available() -> bool {
    io::stderr().is_terminal() && env::var_os("TERM").map_or(true, |term| term != "dumb")
}

impl Spinner {
    fn start(enabled: bool, timeout: Duration) -> Self {
        if !enabled || !visual_terminal_available() {
            return Self {
                running: None,
                worker: None,
            };
        }

        let running = Arc::new(AtomicBool::new(true));
        let worker_running = Arc::clone(&running);
        let color = terminal::stderr_color_enabled();
        let deadline = Instant::now()
            .checked_add(timeout)
            .unwrap_or_else(Instant::now);
        let worker = thread::spawn(move || {
            const FRAMES: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
            let mut frame = 0;
            let mut drawn = false;
            // Avoid flashing a spinner for work that completes quickly.
            thread::park_timeout(Duration::from_millis(300));
            while worker_running.load(Ordering::Relaxed) {
                let seconds = countdown_seconds(deadline.saturating_duration_since(Instant::now()));
                {
                    let stderr = io::stderr();
                    let mut output = stderr.lock();
                    let _ = write_spinner_line(&mut output, FRAMES[frame], seconds, None, color);
                    let _ = output.flush();
                }
                drawn = true;
                frame = (frame + 1) % FRAMES.len();
                thread::park_timeout(Duration::from_millis(500));
            }
            if drawn {
                eprint!("\r\x1b[K");
                let _ = io::stderr().flush();
            }
        });
        Self {
            running: Some(running),
            worker: Some(worker),
        }
    }

    fn stop(&mut self) {
        if let Some(running) = self.running.take() {
            running.store(false, Ordering::Relaxed);
        }
        if let Some(worker) = self.worker.take() {
            // Wake an initial-delay or between-frame park so stopping never
            // adds up to 300 ms to the measured command time.
            worker.thread().unpark();
            let _ = worker.join();
        }
    }
}

impl Drop for Spinner {
    fn drop(&mut self) {
        self.stop();
    }
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use super::*;

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
        assert_eq!(parse_timeout(OsStr::new("-1")), None);
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

    #[test]
    fn input_format_defaults_to_auto() {
        assert_eq!(parsed_format(["in"]), Format::Auto);
    }

    #[test]
    fn raw_is_an_idempotent_advanced_override() {
        assert_eq!(parsed_format(["--raw", "in"]), Format::Raw);
        assert_eq!(parsed_format(["in", "--raw", "--raw"]), Format::Raw);
    }

    #[test]
    fn bounded_reader_rejects_bytes_past_the_limit() {
        assert_eq!(
            read_bounded(Cursor::new(b"abcd"), 3),
            Err(ReadError::TooLarge)
        );
        assert_eq!(read_bounded(Cursor::new(b"abc"), 3).unwrap(), b"abc");
    }

    #[test]
    fn dry_run_optimizes_without_creating_an_output() {
        let directory = unique_test_directory();
        let input = directory.join("input.deflate");
        fs::write(&input, [0x03, 0x00]).unwrap();

        execute(Command {
            format: Format::Raw,
            options: Options::default(),
            inputs: vec![input.clone()],
            destination: Destination::DryRun,
        })
        .unwrap();

        assert_eq!(fs::read(&input).unwrap(), [0x03, 0x00]);
        assert_eq!(fs::read_dir(&directory).unwrap().count(), 1);
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn batch_continues_sequentially_after_a_file_error() {
        let source = [
            0x75, 0xc0, 0x41, 0x0d, 0x00, 0x00, 0x0c, 0x03, 0x21, 0x6d, 0xf8, 0x37, 0xb5, 0x7f,
            0x97, 0x03, 0xcb, 0xb2, 0x3c, 0x82, 0x20, 0x08, 0x0e,
        ];
        let expected = optimize(&source, Format::Raw, &Options::default()).unwrap();
        assert_eq!(expected.bits_saved, 1);

        let directory = unique_test_directory();
        let missing = directory.join("missing.deflate");
        let valid = directory.join("valid.deflate");
        fs::write(&valid, source).unwrap();

        let result = execute(Command {
            format: Format::Raw,
            options: Options::default(),
            inputs: vec![missing, valid.clone()],
            destination: Destination::InPlace,
        });

        assert_eq!(result, Err(1));
        assert_eq!(fs::read(valid).unwrap(), expected.data);
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn batch_dry_run_leaves_every_input_unchanged() {
        let directory = unique_test_directory();
        let first = directory.join("first.deflate");
        let second = directory.join("second.deflate");
        fs::write(&first, [0x03, 0x00]).unwrap();
        fs::write(&second, [0x03, 0x00]).unwrap();

        execute(Command {
            format: Format::Raw,
            options: Options::default(),
            inputs: vec![first.clone(), second.clone()],
            destination: Destination::DryRun,
        })
        .unwrap();

        assert_eq!(fs::read(first).unwrap(), [0x03, 0x00]);
        assert_eq!(fs::read(second).unwrap(), [0x03, 0x00]);
        assert_eq!(fs::read_dir(&directory).unwrap().count(), 2);
        fs::remove_dir_all(directory).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn no_gain_in_place_preserves_the_original_directory_entry() {
        use std::os::unix::fs::MetadataExt;

        let directory = unique_test_directory();
        let input = directory.join("input.deflate");
        fs::write(&input, [0x03, 0x00]).unwrap();
        let inode = fs::symlink_metadata(&input).unwrap().ino();

        execute(Command {
            format: Format::Raw,
            options: Options::default(),
            inputs: vec![input.clone()],
            destination: Destination::InPlace,
        })
        .unwrap();

        assert_eq!(fs::read(&input).unwrap(), [0x03, 0x00]);
        assert_eq!(fs::symlink_metadata(&input).unwrap().ino(), inode);
        assert_eq!(fs::read_dir(&directory).unwrap().count(), 1);
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn no_gain_preserves_an_existing_explicit_output() {
        let directory = unique_test_directory();
        let input = directory.join("input.deflate");
        let output = directory.join("output.deflate");
        fs::write(&input, [0x03, 0x00]).unwrap();
        fs::write(&output, b"existing output").unwrap();

        execute(Command {
            format: Format::Raw,
            options: Options::default(),
            inputs: vec![input],
            destination: Destination::Explicit(output.clone()),
        })
        .unwrap();

        assert_eq!(fs::read(output).unwrap(), b"existing output");
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn no_gain_creates_a_missing_explicit_output_from_the_original() {
        let directory = unique_test_directory();
        let input = directory.join("input.deflate");
        let output = directory.join("output.deflate");
        fs::write(&input, [0x03, 0x00]).unwrap();

        execute(Command {
            format: Format::Raw,
            options: Options::default(),
            inputs: vec![input],
            destination: Destination::Explicit(output.clone()),
        })
        .unwrap();

        assert_eq!(fs::read(output).unwrap(), [0x03, 0x00]);
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn equal_byte_output_with_a_one_bit_gain_replaces_in_place() {
        let source = [
            0x75, 0xc0, 0x41, 0x0d, 0x00, 0x00, 0x0c, 0x03, 0x21, 0x6d, 0xf8, 0x37, 0xb5, 0x7f,
            0x97, 0x03, 0xcb, 0xb2, 0x3c, 0x82, 0x20, 0x08, 0x0e,
        ];
        let expected = optimize(&source, Format::Raw, &Options::default()).unwrap();
        assert_eq!(expected.data.len(), source.len());
        assert_eq!(expected.bits_saved, 1);

        let directory = unique_test_directory();
        let input = directory.join("input.deflate");
        fs::write(&input, source).unwrap();
        execute(Command {
            format: Format::Raw,
            options: Options::default(),
            inputs: vec![input.clone()],
            destination: Destination::InPlace,
        })
        .unwrap();

        assert_eq!(fs::read(input).unwrap(), expected.data);
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn padding_only_rewrite_without_a_meaningful_saving_does_not_replace_in_place() {
        let source = [0x03, 0xfc];
        let optimized = optimize(&source, Format::Raw, &Options::default()).unwrap();
        assert_eq!(optimized.bits_saved, 0);

        let directory = unique_test_directory();
        let input = directory.join("input.deflate");
        fs::write(&input, source).unwrap();
        execute(Command {
            format: Format::Raw,
            options: Options::default(),
            inputs: vec![input.clone()],
            destination: Destination::InPlace,
        })
        .unwrap();

        assert_eq!(fs::read(input).unwrap(), source);
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn output_commit_replaces_an_existing_file() {
        let directory = unique_test_directory();
        let output = directory.join("output.bin");
        fs::write(&output, b"old").unwrap();

        write_file(&output, b"new").unwrap();
        assert_eq!(fs::read(&output).unwrap(), b"new");
        assert_eq!(fs::read_dir(&directory).unwrap().count(), 1);

        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn guarded_commit_refuses_to_replace_a_changed_input_snapshot() {
        let directory = unique_test_directory();
        let output = directory.join("output.bin");
        fs::write(&output, b"newer source").unwrap();

        assert!(!write_file_if_unchanged(&output, b"old source", b"optimized").unwrap());
        assert_eq!(fs::read(&output).unwrap(), b"newer source");
        assert!(write_file_if_unchanged(&output, b"newer source", b"optimized").unwrap());
        assert_eq!(fs::read(&output).unwrap(), b"optimized");
        assert_eq!(fs::read_dir(&directory).unwrap().count(), 1);

        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn staging_skips_a_candidate_equal_to_the_requested_output() {
        let directory = unique_test_directory();
        let sequence = 12_345;
        let name = format!(".columbo-{}-{sequence}.tmp", std::process::id());
        let output = directory.join(&name);

        assert_eq!(
            temporary_output_candidate(&output, &directory, sequence),
            None
        );
        assert_eq!(
            temporary_output_candidate(Path::new(&name), Path::new("."), sequence),
            None
        );

        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn no_clobber_commit_creates_only_a_missing_destination() {
        let directory = unique_test_directory();
        let output = directory.join("output.bin");

        assert!(write_new_file(&output, b"first").unwrap());
        assert_eq!(fs::read(&output).unwrap(), b"first");
        assert!(!write_new_file(&output, b"second").unwrap());
        assert_eq!(fs::read(&output).unwrap(), b"first");
        assert_eq!(fs::read_dir(&directory).unwrap().count(), 1);

        fs::remove_dir_all(directory).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn staged_output_is_private_and_commit_preserves_destination_mode() {
        use std::os::unix::fs::PermissionsExt;

        let directory = unique_test_directory();
        let output = directory.join("output.bin");
        fs::write(&output, b"old").unwrap();
        fs::set_permissions(&output, fs::Permissions::from_mode(0o751)).unwrap();

        let (mut temporary, file) = stage_private_output(&output, b"new").unwrap();
        assert_eq!(fs::read(&temporary.path).unwrap(), b"new");
        assert_eq!(
            fs::metadata(&temporary.path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert_eq!(fs::read(&output).unwrap(), b"old");

        commit_temporary_output(&output, &mut temporary, file).unwrap();
        assert_eq!(fs::read(&output).unwrap(), b"new");
        assert_eq!(
            fs::metadata(&output).unwrap().permissions().mode() & 0o777,
            0o751
        );
        assert_eq!(fs::read_dir(&directory).unwrap().count(), 1);

        fs::remove_dir_all(directory).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn output_commit_replaces_a_symlink_without_following_it() {
        use std::os::unix::fs::{symlink, PermissionsExt};

        let directory = unique_test_directory();
        let protected = directory.join("protected.bin");
        let output = directory.join("output.bin");
        fs::write(&protected, b"protected").unwrap();
        fs::set_permissions(&protected, fs::Permissions::from_mode(0o644)).unwrap();
        symlink(&protected, &output).unwrap();

        write_file(&output, b"optimized").unwrap();
        assert_eq!(fs::read(&protected).unwrap(), b"protected");
        assert_eq!(fs::read(&output).unwrap(), b"optimized");
        assert!(!fs::symlink_metadata(&output)
            .unwrap()
            .file_type()
            .is_symlink());
        assert_eq!(
            fs::metadata(&output).unwrap().permissions().mode() & 0o777,
            0o600
        );

        fs::remove_dir_all(directory).unwrap();
    }

    fn unique_test_directory() -> PathBuf {
        for _ in 0..TEMP_FILE_ATTEMPTS {
            let sequence = TEMP_FILE_COUNTER.fetch_add(1, Ordering::Relaxed);
            let path = env::temp_dir().join(format!(
                "columbo-output-test-{}-{sequence}",
                std::process::id()
            ));
            match fs::create_dir(&path) {
                Ok(()) => return path,
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(error) => panic!("could not create test directory: {error}"),
            }
        }
        panic!("could not create a unique test directory");
    }

    fn parsed_options<const N: usize>(arguments: [&str; N]) -> Options {
        parsed_command(arguments).options
    }

    fn parsed_command<const N: usize>(arguments: [&str; N]) -> Command {
        let parsed = match parse_args(arguments.into_iter().map(OsString::from)) {
            Ok(parsed) => parsed,
            Err(_) => panic!("expected valid command arguments"),
        };
        match parsed {
            ParsedCommand::Run(command) => command,
            ParsedCommand::Help => panic!("expected a runnable command"),
        }
    }

    fn parsed_format<const N: usize>(arguments: [&str; N]) -> Format {
        parsed_command(arguments).format
    }
}
