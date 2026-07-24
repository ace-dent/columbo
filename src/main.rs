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
use std::time::Duration;

use columbo::{optimize, Format, Options, MAX_TIMEOUT, MIN_TIMEOUT};

const PROGRAM_NAME: &str = "columbo";
const PROGRAM_VERSION: &str = concat!(
    env!("CARGO_PKG_VERSION_MAJOR"),
    ".",
    env!("CARGO_PKG_VERSION_MINOR")
);
const READ_BUFFER_BYTES: usize = 64 * 1024;
const TEMP_FILE_ATTEMPTS: usize = 128;
static TEMP_FILE_COUNTER: AtomicU64 = AtomicU64::new(0);

struct Command {
    format: Format,
    options: Options,
    verbose: bool,
    input: PathBuf,
    output: PathBuf,
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

    let mut spinner = Spinner::start(command.options.exhaustive && !command.options.inspect);
    let result = optimize(&input, command.format, &command.options);
    spinner.stop();
    let optimized = match result {
        Ok(result) => result,
        Err(error) => {
            eprintln!("optimize failed: {error}");
            return Err(1);
        }
    };

    if write_file(&command.output, &optimized.data).is_err() {
        eprintln!("could not write {:?}", command.output);
        return Err(1);
    }

    if command.verbose && optimized.data.len() >= input.len() {
        eprintln!("no bytes saved; wrote original-size output");
    }
    if command.verbose && optimized.timed_out {
        eprintln!(
            "Timeout triggered after {} seconds; wrote best output found so far.",
            command.options.timeout.as_secs()
        );
    }
    eprintln!("{} -> {} bytes", input.len(), optimized.data.len());
    Ok(())
}

fn parse_args(arguments: impl IntoIterator<Item = OsString>) -> Result<ParsedCommand, CliError> {
    let arguments: Vec<OsString> = arguments.into_iter().collect();
    let mut options = Options::default();
    let mut format = Format::Auto;
    let mut format_conflict = false;
    let mut verbose = false;
    let mut index = 0_usize;

    while index < arguments.len() && starts_with_dash(&arguments[index]) {
        let argument = arguments[index].to_string_lossy();
        match argument.as_ref() {
            "-h" | "--help" => return Ok(ParsedCommand::Help),
            "--raw" => select_format(&mut format, &mut format_conflict, Format::Raw),
            "--png" => select_format(&mut format, &mut format_conflict, Format::Png),
            "--zlib" => select_format(&mut format, &mut format_conflict, Format::Zlib),
            "--gzip" => select_format(&mut format, &mut format_conflict, Format::Gzip),
            "--zip" => select_format(&mut format, &mut format_conflict, Format::Zip),
            "-v" | "--verbose" => verbose = true,
            "--inspect" => options.inspect = true,
            "-m" | "--max" => options.exhaustive = true,
            "--allow-258-alias" => options.allow_258_alias = true,
            "--strip" => options.strip_metadata = true,
            "-t" | "--timeout" => {
                index += 1;
                let value = arguments
                    .get(index)
                    .ok_or_else(timeout_error)
                    .and_then(|value| parse_timeout(value).ok_or_else(timeout_error))?;
                options.timeout = value;
            }
            "--mincodes" => {
                index += 1;
                options.min_distance_codes = arguments
                    .get(index)
                    .ok_or_else(mincodes_error)
                    .and_then(|value| parse_mincodes(value).ok_or_else(mincodes_error))?;
            }
            _ if argument.starts_with("--timeout=") => {
                options.timeout =
                    parse_timeout(OsStr::new(&argument[10..])).ok_or_else(timeout_error)?;
            }
            _ if argument.starts_with("--mincodes=") => {
                options.min_distance_codes =
                    parse_mincodes(OsStr::new(&argument[11..])).ok_or_else(mincodes_error)?;
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

    if format_conflict {
        return Err(CliError::message("choose only one input mode", false));
    }
    if arguments.len() - index != 2 {
        return Err(CliError::usage());
    }

    Ok(ParsedCommand::Run(Command {
        format,
        options,
        verbose,
        input: PathBuf::from(&arguments[index]),
        output: PathBuf::from(&arguments[index + 1]),
    }))
}

fn select_format(format: &mut Format, conflict: &mut bool, selected: Format) {
    // Repeating one mode is harmless in the original Columbo C CLI; selecting
    // two distinct modes is the error.
    if *format != selected {
        *conflict |= *format != Format::Auto;
        *format = selected;
    }
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

fn parse_mincodes(value: &OsStr) -> Option<bool> {
    match value.to_str()?.trim_start().parse::<i64>().ok()? {
        0 => Some(false),
        1 => Some(true),
        _ => None,
    }
}

fn timeout_error() -> CliError {
    CliError::message("--timeout requires a non-negative number of seconds", false)
}

fn mincodes_error() -> CliError {
    CliError::message("--mincodes requires 0 or 1", false)
}

fn print_usage(output: &mut dyn Write) -> io::Result<()> {
    writeln!(
        output,
        "🕵🏻‍♂️  \x1b[1m{PROGRAM_NAME} v{PROGRAM_VERSION} alpha\x1b[0m"
    )?;
    writeln!(
        output,
        "\"Just One More Thing\" - optimize the last few bytes in Deflate streams."
    )?;
    writeln!(
        output,
        concat!(
            "usage: {} [--help|-h] [--verbose|-v] [--inspect]",
            " [--max|-m] [--timeout|-t seconds]",
            "\n       [--mincodes 0|1] [--allow-258-alias] [--strip]",
            "\n       [--raw|--png|--zlib|--gzip|--zip] input output"
        ),
        PROGRAM_NAME
    )?;
    writeln!(output)?;
    writeln!(output, "Options:")?;
    writeln!(output, "  -h, --help             show this help and exit")?;
    writeln!(
        output,
        "  -v, --verbose          report when no bytes were saved"
    )?;
    writeln!(
        output,
        "      --inspect          print block-level optimization choices to stderr"
    )?;
    writeln!(
        output,
        "  -m, --max              enable slower byte-seeking searches"
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
            "      --mincodes 0|1     add Deflate distance codes for old decoder",
            "\n                         compatibility (default: 0)"
        )
    )?;
    writeln!(
        output,
        "      --allow-258-alias  allow Columbo's non-standard symbol-284 length-258 spelling"
    )?;
    writeln!(
        output,
        "                         (strict Deflate decoders may reject it)"
    )?;
    writeln!(
        output,
        "      --strip            strip supported metadata/comment wrapper fields"
    )?;
    writeln!(
        output,
        "      --raw              treat input as a raw Deflate stream"
    )?;
    writeln!(output, "      --png              treat input as PNG")?;
    writeln!(
        output,
        "      --zlib             treat input as zlib-wrapped Deflate"
    )?;
    writeln!(output, "      --gzip             treat input as GZIP")?;
    writeln!(output, "      --zip              treat input as ZIP")?;
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

/// Write and sync a private sibling without changing its access bits.
///
/// Keeping this phase separate from the final commit ensures that a temporary
/// name never exposes partially written output using the destination's mode.
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
            while worker_running.load(Ordering::Relaxed) {
                eprint!(
                    "\r\x1b[36m{}\x1b[0m maximizing compression...",
                    FRAMES[frame]
                );
                let _ = io::stderr().flush();
                frame = (frame + 1) % FRAMES.len();
                thread::sleep(Duration::from_millis(200));
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
    fn parser_rejects_multiple_explicit_formats() {
        let error = match parse_args([
            OsString::from("--raw"),
            OsString::from("--gzip"),
            OsString::from("in"),
            OsString::from("out"),
        ]) {
            Err(error) => error,
            Ok(_) => panic!("multiple formats should fail"),
        };
        assert_eq!(error.message.as_deref(), Some("choose only one input mode"));
    }

    #[test]
    fn many_conflicting_format_flags_cannot_wrap_the_conflict_state() {
        let mut arguments = Vec::new();
        for index in 0..300 {
            arguments.push(OsString::from(if index % 2 == 0 {
                "--raw"
            } else {
                "--gzip"
            }));
        }
        arguments.extend([OsString::from("in"), OsString::from("out")]);

        let error = match parse_args(arguments) {
            Err(error) => error,
            Ok(_) => panic!("conflicting formats should fail"),
        };
        assert_eq!(error.message.as_deref(), Some("choose only one input mode"));
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
}
