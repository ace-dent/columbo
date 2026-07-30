// SPDX-License-Identifier: MIT

#![forbid(unsafe_code)]

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

use columbo::{optimize, Format, Options, MAX_TIMEOUT, MIN_TIMEOUT};

const PROGRAM_NAME: &str = "columbo";
const PROGRAM_VERSION: &str = concat!(
    env!("CARGO_PKG_VERSION_MAJOR"),
    ".",
    env!("CARGO_PKG_VERSION_MINOR")
);
const PROGRAM_STAGE: &str = "Alpha";
const READ_BUFFER_BYTES: usize = 64 * 1024;
const TEMP_FILE_ATTEMPTS: usize = 128;
static TEMP_FILE_COUNTER: AtomicU64 = AtomicU64::new(0);

struct Command {
    format: Format,
    options: Options,
    input: PathBuf,
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
            let _ = print_usage(&mut io::stdout());
            return Ok(());
        }
        Ok(ParsedCommand::Run(command)) => command,
        Err(error) => {
            if let Some(message) = error.message {
                eprintln!("{message}");
            }
            if error.show_usage {
                let _ = print_usage(&mut io::stderr());
            }
            return Err(2);
        }
    };

    execute(command)
}

fn execute(command: Command) -> std::result::Result<(), u8> {
    let overwrites_input = command.destination.overwrites_input(&command.input);
    let total_started = command.options.verbose.then(Instant::now);
    let read_started = command.options.verbose.then(Instant::now);
    let input = match read_file(&command.input, command.options.max_input_bytes) {
        Ok(bytes) => bytes,
        Err(ReadError::TooLarge) => {
            eprintln!("input exceeds the 1 GiB file-size limit");
            return Err(1);
        }
        Err(ReadError::Allocation) => {
            eprintln!("not enough memory to read input");
            return Err(1);
        }
        Err(ReadError::Io) => {
            // Path's Debug formatter escapes terminal control characters.
            eprintln!("could not read {:?}", command.input);
            return Err(1);
        }
    };
    let read_elapsed = read_started.map_or(Duration::ZERO, |started| started.elapsed());

    if command.options.verbose {
        print_verbose_header(&command, input.len(), read_elapsed);
    }

    if command.options.verbose && !command.options.strict {
        println!(
            "note: strict mode disabled; enabling compact empty/singleton Huffman alphabets \
             and the non-standard length-258 alias"
        );
    }

    let optimize_started = command.options.verbose.then(Instant::now);
    let mut spinner = Spinner::start(command.options.exhaustive && !command.options.verbose);
    let result = optimize(&input, command.format, &command.options);
    spinner.stop();
    let optimize_elapsed = optimize_started.map_or(Duration::ZERO, |started| started.elapsed());
    let optimized = match result {
        Ok(result) => result,
        Err(error) => {
            eprintln!("optimize failed: {error}");
            return Err(1);
        }
    };

    let write_started = command.options.verbose.then(Instant::now);
    let output_action = match command.destination.output_path(&command.input) {
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

    if optimized.timed_out {
        match &output_action {
            OutputAction::DryRun => eprintln!(
                "Timeout triggered after {} seconds; reporting best result found so far.",
                command.options.timeout.as_secs()
            ),
            OutputAction::WrittenOptimized(_) => eprintln!(
                "Timeout triggered after {} seconds; wrote best output found so far.",
                command.options.timeout.as_secs()
            ),
            OutputAction::CopiedOriginal(_) | OutputAction::Preserved(_) => eprintln!(
                "Timeout triggered after {} seconds; no smaller output was written.",
                command.options.timeout.as_secs()
            ),
        }
    }
    if command.options.verbose {
        print_verbose_result(
            &output_action,
            input.len(),
            optimized.data.len(),
            optimized.bits_saved,
            &ExecutionTimings {
                read: read_elapsed,
                optimize: optimize_elapsed,
                write: write_elapsed,
                total: total_started.map_or(Duration::ZERO, |started| started.elapsed()),
            },
        );
    } else {
        print_quiet_result(
            &output_action,
            input.len(),
            optimized.data.len(),
            optimized.bits_saved,
        );
    }
    Ok(())
}

fn print_quiet_result(
    action: &OutputAction,
    input_bytes: usize,
    optimized_bytes: usize,
    bits_saved: u64,
) {
    match action {
        OutputAction::DryRun => {
            eprintln!("{input_bytes} -> {optimized_bytes} bytes (dry run; no output written)")
        }
        OutputAction::WrittenOptimized(_) if input_bytes == optimized_bytes => eprintln!(
            "{input_bytes} -> {optimized_bytes} bytes (saved {bits_saved} meaningful {})",
            plural_u64(bits_saved, "bit", "bits")
        ),
        OutputAction::WrittenOptimized(_) => {
            eprintln!("{input_bytes} -> {optimized_bytes} bytes");
        }
        OutputAction::CopiedOriginal(output) => eprintln!(
            "{input_bytes} -> {input_bytes} bytes \
             (no savings; copied original to {:?})",
            output
        ),
        OutputAction::Preserved(output) => eprintln!(
            "{input_bytes} -> {input_bytes} bytes \
             (no savings; left {:?} unchanged)",
            output
        ),
    }
}

fn print_verbose_header(command: &Command, input_bytes: usize, read_elapsed: Duration) {
    let input_name = command
        .input
        .file_name()
        .unwrap_or(command.input.as_os_str());
    println!();
    println!("{PROGRAM_NAME} v{PROGRAM_VERSION} {PROGRAM_STAGE} · verbose");
    println!("────────────────────────");
    println!(
        "Input    {:?} · {input_bytes} {} · read {}",
        input_name,
        plural_bytes(input_bytes),
        format_elapsed(read_elapsed)
    );
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
        println!(
            "Mode     max · {strictness}{dry_run} · {} file-wide budget",
            format_elapsed(command.options.timeout),
        );
    } else {
        println!("Mode     normal · {strictness}{dry_run}");
    }
}

fn print_verbose_result(
    action: &OutputAction,
    input_bytes: usize,
    output_bytes: usize,
    bits_saved: u64,
    timings: &ExecutionTimings,
) {
    println!();
    println!("Result");
    match action {
        OutputAction::WrittenOptimized(output) => {
            let output_name = output.file_name().unwrap_or(output.as_os_str());
            println!("  Output  {:?}", output_name);
        }
        OutputAction::CopiedOriginal(output) => {
            let output_name = output.file_name().unwrap_or(output.as_os_str());
            println!("  Output  {:?} · original copied · no savings", output_name);
        }
        OutputAction::Preserved(output) => {
            let output_name = output.file_name().unwrap_or(output.as_os_str());
            println!("  Output  {:?} · preserved · no savings", output_name);
        }
        OutputAction::DryRun => println!("  Output  not written · dry run"),
    }
    println!(
        "  Size    {input_bytes} → {output_bytes} {} · {}",
        plural_bytes(output_bytes),
        describe_optimization_change(input_bytes, output_bytes, bits_saved)
    );
    if !action.wrote_file() {
        println!(
            "  Time    {} total · read {} · optimize {}",
            format_elapsed(timings.total),
            format_elapsed(timings.read),
            format_elapsed(timings.optimize),
        );
    } else {
        println!(
            "  Time    {} total · read {} · optimize {} · write {}",
            format_elapsed(timings.total),
            format_elapsed(timings.read),
            format_elapsed(timings.optimize),
            format_elapsed(timings.write)
        );
    }
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

fn format_elapsed(duration: Duration) -> String {
    if duration < Duration::from_millis(1) {
        format!("{} µs", duration.subsec_micros())
    } else if duration < Duration::from_secs(1) {
        let centimilliseconds = (duration.subsec_micros() + 5) / 10;
        format!(
            "{}.{:02} ms",
            centimilliseconds / 100,
            centimilliseconds % 100
        )
    } else if duration < Duration::from_secs(10) {
        let mut seconds = duration.as_secs();
        let mut centiseconds = (duration.subsec_micros() + 5_000) / 10_000;
        if centiseconds == 100 {
            seconds += 1;
            centiseconds = 0;
        }
        format!("{seconds}.{centiseconds:02} s")
    } else {
        let mut seconds = duration.as_secs();
        let mut deciseconds = (duration.subsec_millis() + 50) / 100;
        if deciseconds == 10 {
            seconds = seconds.saturating_add(1);
            deciseconds = 0;
        }
        format!("{seconds}.{deciseconds} s")
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
                plural_bytes(saved),
                percentage_hundredths / 100,
                percentage_hundredths % 100
            )
        }
        std::cmp::Ordering::Less => {
            let added = output_bytes - input_bytes;
            format!("added {added} {}", plural_bytes(added))
        }
        std::cmp::Ordering::Equal => "unchanged".to_owned(),
    }
}

fn plural_bytes(count: usize) -> &'static str {
    if count == 1 {
        "byte"
    } else {
        "bytes"
    }
}

fn plural_u64(count: u64, singular: &'static str, plural: &'static str) -> &'static str {
    if count == 1 {
        singular
    } else {
        plural
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

    let (input, destination) = if dry_run {
        match positional.as_slice() {
            [input] => (PathBuf::from(input), Destination::DryRun),
            [_, _] => {
                return Err(CliError::message(
                    "--dry-run does not accept a positional output filename; \
                     --out is ignored in dry-run mode",
                    true,
                ));
            }
            _ => return Err(CliError::usage()),
        }
    } else {
        match positional.as_slice() {
            [input] => {
                let destination = output.map_or(Destination::InPlace, Destination::Explicit);
                (PathBuf::from(input), destination)
            }
            [input, legacy_output] if output.is_none() => (
                PathBuf::from(input),
                Destination::Explicit(PathBuf::from(legacy_output)),
            ),
            [_, _] => {
                return Err(CliError::message(
                    "--out cannot be combined with a positional output filename",
                    true,
                ));
            }
            _ => return Err(CliError::usage()),
        }
    };

    Ok(ParsedCommand::Run(Command {
        format,
        options,
        input,
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

fn print_usage(output: &mut dyn Write) -> io::Result<()> {
    writeln!(
        output,
        "🕵🏻‍♂️  \x1b[1m{PROGRAM_NAME} v{PROGRAM_VERSION} {PROGRAM_STAGE}\x1b[0m"
    )?;
    writeln!(
        output,
        "\"Just One More Thing\" - optimize the last few bytes in Deflate streams."
    )?;
    writeln!(
        output,
        "usage: {} [options] [--out file] input",
        PROGRAM_NAME
    )?;
    writeln!(output, "       {} --dry-run [options] input", PROGRAM_NAME)?;
    writeln!(output)?;
    writeln!(output, "Options:")?;
    writeln!(output, "  -h, --help             show this help and exit")?;
    writeln!(
        output,
        "  -v, --verbose          show live route timings, bit gains, and block choices"
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
        "      --out <file>       write to file instead of optimizing input in place"
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
        "      --strip            strip supported metadata/comment wrapper fields"
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
        "Existing files are replaced only when byte size decreases or at least one"
    )?;
    writeln!(output, "meaningful Deflate bit is saved.")?;
    writeln!(
        output,
        "Input and decoded Deflate data are limited to 1 GiB."
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

impl Spinner {
    fn start(enabled: bool) -> Self {
        if !enabled || !io::stderr().is_terminal() {
            return Self {
                running: None,
                worker: None,
            };
        }

        let running = Arc::new(AtomicBool::new(true));
        let worker_running = Arc::clone(&running);
        let worker = thread::spawn(move || {
            const FRAMES: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
            let mut frame = 0;
            // Avoid flashing a spinner for work that completes quickly.
            thread::park_timeout(Duration::from_millis(300));
            while worker_running.load(Ordering::Relaxed) {
                eprint!(
                    "\r\x1b[36m{}\x1b[0m maximizing compression...",
                    FRAMES[frame]
                );
                let _ = io::stderr().flush();
                frame = (frame + 1) % FRAMES.len();
                thread::park_timeout(Duration::from_millis(200));
            }
            eprint!("\r\x1b[K");
            let _ = io::stderr().flush();
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
    fn output_defaults_in_place_and_accepts_oxipng_forms() {
        let default = parsed_command(["in"]);
        assert_eq!(default.input, Path::new("in"));
        assert!(matches!(default.destination, Destination::InPlace));

        for command in [
            parsed_command(["--out", "out", "in"]),
            parsed_command(["in", "--out", "out"]),
            parsed_command(["--out=out", "in"]),
        ] {
            assert_eq!(command.input, Path::new("in"));
            assert!(matches!(
                command.destination,
                Destination::Explicit(path) if path == Path::new("out")
            ));
        }

        // Retain the old positional destination as a compatibility alias,
        // while all help and repository tooling use --out.
        assert!(matches!(
            parsed_command(["in", "out"]).destination,
            Destination::Explicit(path) if path == Path::new("out")
        ));
        assert!(matches!(
            parsed_command(["--out=-output", "in"]).destination,
            Destination::Explicit(path) if path == Path::new("-output")
        ));
        assert_eq!(parsed_command(["--", "-input"]).input, Path::new("-input"));
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
                "--out cannot be combined with a positional output filename",
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
    fn dry_run_accepts_out_but_rejects_a_legacy_positional_output() {
        for command in [
            parsed_command(["-d", "in"]),
            parsed_command(["--dry-run", "in"]),
            parsed_command(["-d", "-d", "in"]),
            parsed_command(["--dry-run", "--out", "ignored", "in"]),
            parsed_command(["in", "--out=ignored", "--dry-run"]),
        ] {
            assert_eq!(command.input, Path::new("in"));
            assert!(command.destination.is_dry_run());
        }

        let output_error =
            match parse_args(["--dry-run", "in", "out"].into_iter().map(OsString::from)) {
                Err(error) => error,
                Ok(_) => panic!("dry-run output filename should fail"),
            };
        assert_eq!(
            output_error.message.as_deref(),
            Some(
                "--dry-run does not accept a positional output filename; \
                 --out is ignored in dry-run mode"
            )
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
        print_usage(&mut help).unwrap();
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
        assert!(help.contains("usage: columbo [options] [--out file] input"));
        assert!(help.contains("columbo --dry-run [options] input"));
        assert!(help.contains("-d, --dry-run"));
        assert!(help.contains("--out <file>"));
        assert!(help.contains("instead of optimizing input in place"));
        assert!(help.contains("without writing output"));
        assert!(help.contains("meaningful Deflate bit is saved"));
        assert!(help.contains("Advanced:"));
        assert!(help.contains("--raw"));
        assert!(help.contains("live route timings, bit gains, and block choices"));
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
            input: input.clone(),
            destination: Destination::DryRun,
        })
        .unwrap();

        assert_eq!(fs::read(&input).unwrap(), [0x03, 0x00]);
        assert_eq!(fs::read_dir(&directory).unwrap().count(), 1);
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
            input: input.clone(),
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
            input,
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
            input,
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
            input: input.clone(),
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
            input: input.clone(),
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
