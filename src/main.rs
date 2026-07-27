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
const PROGRAM_STAGE: &str = "Beta";
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
    File(PathBuf),
    DryRun,
}

impl Destination {
    fn is_dry_run(&self) -> bool {
        matches!(self, Self::DryRun)
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

    let write_elapsed = match &command.destination {
        Destination::File(output) => {
            let write_started = command.options.verbose.then(Instant::now);
            if write_file(output, &optimized.data).is_err() {
                eprintln!("could not write {:?}", output);
                return Err(1);
            }
            write_started.map_or(Duration::ZERO, |started| started.elapsed())
        }
        Destination::DryRun => Duration::ZERO,
    };

    if optimized.timed_out {
        if command.destination.is_dry_run() {
            eprintln!(
                "Timeout triggered after {} seconds; reporting best result found so far.",
                command.options.timeout.as_secs()
            );
        } else {
            eprintln!(
                "Timeout triggered after {} seconds; wrote best output found so far.",
                command.options.timeout.as_secs()
            );
        }
    }
    if command.options.verbose {
        print_verbose_result(
            &command.destination,
            input.len(),
            optimized.data.len(),
            read_elapsed,
            optimize_elapsed,
            write_elapsed,
            total_started.map_or(Duration::ZERO, |started| started.elapsed()),
        );
    } else if command.destination.is_dry_run() {
        eprintln!(
            "{} -> {} bytes (dry run; no output written)",
            input.len(),
            optimized.data.len()
        );
    } else {
        eprintln!("{} -> {} bytes", input.len(), optimized.data.len());
    }
    Ok(())
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
    destination: &Destination,
    input_bytes: usize,
    output_bytes: usize,
    read_elapsed: Duration,
    optimize_elapsed: Duration,
    write_elapsed: Duration,
    total_elapsed: Duration,
) {
    println!();
    println!("Result");
    match destination {
        Destination::File(output) => {
            let output_name = output.file_name().unwrap_or(output.as_os_str());
            println!("  Output  {:?}", output_name);
        }
        Destination::DryRun => println!("  Output  not written · dry run"),
    }
    println!(
        "  Size    {input_bytes} → {output_bytes} {} · {}",
        plural_bytes(output_bytes),
        describe_byte_change(input_bytes, output_bytes)
    );
    if destination.is_dry_run() {
        println!(
            "  Time    {} total · read {} · optimize {}",
            format_elapsed(total_elapsed),
            format_elapsed(read_elapsed),
            format_elapsed(optimize_elapsed),
        );
    } else {
        println!(
            "  Time    {} total · read {} · optimize {} · write {}",
            format_elapsed(total_elapsed),
            format_elapsed(read_elapsed),
            format_elapsed(optimize_elapsed),
            format_elapsed(write_elapsed)
        );
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

fn parse_args(arguments: impl IntoIterator<Item = OsString>) -> Result<ParsedCommand, CliError> {
    let arguments: Vec<OsString> = arguments.into_iter().collect();
    let mut options = Options::default();
    let mut format = Format::Auto;
    let mut dry_run = false;
    let mut index = 0_usize;

    while index < arguments.len() && starts_with_dash(&arguments[index]) {
        let argument = arguments[index].to_string_lossy();
        match argument.as_ref() {
            "-h" | "--help" => return Ok(ParsedCommand::Help),
            "--raw" => format = Format::Raw,
            "-v" | "--verbose" => options.verbose = true,
            "-m" | "--max" => options.exhaustive = true,
            "-d" | "--dry-run" => dry_run = true,
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

    let positional = arguments.len() - index;
    let destination = if dry_run {
        match positional {
            1 => Destination::DryRun,
            2 => {
                return Err(CliError::message(
                    "--dry-run does not accept an output filename",
                    true,
                ));
            }
            _ => return Err(CliError::usage()),
        }
    } else if positional == 2 {
        Destination::File(PathBuf::from(&arguments[index + 1]))
    } else {
        return Err(CliError::usage());
    };

    Ok(ParsedCommand::Run(Command {
        format,
        options,
        input: PathBuf::from(&arguments[index]),
        destination,
    }))
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

fn print_usage(output: &mut dyn Write) -> io::Result<()> {
    writeln!(
        output,
        "🕵🏻‍♂️  \x1b[1m{PROGRAM_NAME} v{PROGRAM_VERSION} {PROGRAM_STAGE}\x1b[0m"
    )?;
    writeln!(
        output,
        "\"Just One More Thing\" - optimize the last few bytes in Deflate streams."
    )?;
    writeln!(output, "usage: {} [options] input output", PROGRAM_NAME)?;
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
        concat!(
            "  -t, --timeout          stop byte-seeking searches after this many seconds",
            "\n                         (default: 180; range: 10..4000;",
            " fractions round up)"
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
        let temporary_name =
            OsString::from(format!(".columbo-{}-{sequence}.tmp", std::process::id()));
        let temporary_path = parent.join(temporary_name);
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
        assert!(parsed_options(["in", "out"]).strict);
        assert!(!parsed_options(["--strict", "0", "in", "out"]).strict);
        assert!(!parsed_options(["--strict=0", "in", "out"]).strict);
        assert!(parsed_options(["--strict", "1", "in", "out"]).strict);
        assert!(parsed_options(["--strict=1", "in", "out"]).strict);

        for arguments in [
            vec!["--strict", "2", "in", "out"],
            vec!["--strict", "true", "in", "out"],
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
        assert!(!parsed_options(["in", "out"]).verbose);
        assert!(parsed_options(["-v", "in", "out"]).verbose);
        assert!(parsed_options(["--verbose", "in", "out"]).verbose);
    }

    #[test]
    fn dry_run_requires_one_input_and_rejects_an_output() {
        for command in [
            parsed_command(["-d", "in"]),
            parsed_command(["--dry-run", "in"]),
            parsed_command(["-d", "-d", "in"]),
        ] {
            assert_eq!(command.input, Path::new("in"));
            assert!(command.destination.is_dry_run());
        }
        assert!(matches!(
            parsed_command(["in", "out"]).destination,
            Destination::File(path) if path == Path::new("out")
        ));

        let output_error =
            match parse_args(["--dry-run", "in", "out"].into_iter().map(OsString::from)) {
                Err(error) => error,
                Ok(_) => panic!("dry-run output filename should fail"),
            };
        assert_eq!(
            output_error.message.as_deref(),
            Some("--dry-run does not accept an output filename")
        );
        assert!(output_error.show_usage);

        for arguments in [vec!["--dry-run"], vec!["in"]] {
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
            let error = match parse_args([
                OsString::from(option),
                OsString::from("in"),
                OsString::from("out"),
            ]) {
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
        assert!(help.contains("usage: columbo [options] input output"));
        assert!(help.contains("columbo --dry-run [options] input"));
        assert!(help.contains("-d, --dry-run"));
        assert!(help.contains("without writing output"));
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
        assert_eq!(parsed_format(["in", "out"]), Format::Auto);
    }

    #[test]
    fn raw_is_an_idempotent_advanced_override() {
        assert_eq!(parsed_format(["--raw", "in", "out"]), Format::Raw);
        assert_eq!(parsed_format(["--raw", "--raw", "in", "out"]), Format::Raw);
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
