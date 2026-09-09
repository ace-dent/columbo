// SPDX-License-Identifier: MIT

//! Command-line parsing, validation, and usage text.

use std::ffi::{OsStr, OsString};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use columbo::{
    Format, Options, MAX_EXPANSION_RATIO, MAX_TIMEOUT, MIN_EXPANSION_LIMIT_BYTES, MIN_TIMEOUT,
};

use super::files::paths_refer_to_same_file;
use super::{PROGRAM_NAME, PROGRAM_STAGE, PROGRAM_VERSION};

pub(super) struct Command {
    pub(super) format: Format,
    pub(super) options: Options,
    pub(super) inputs: Vec<PathBuf>,
    pub(super) destination: Destination,
}

pub(super) enum Destination {
    InPlace,
    Explicit(PathBuf),
    DryRun,
}

impl Destination {
    pub(super) fn is_dry_run(&self) -> bool {
        matches!(self, Self::DryRun)
    }

    pub(super) fn output_path<'a>(&'a self, input: &'a Path) -> Option<&'a Path> {
        match self {
            Self::InPlace => Some(input),
            Self::Explicit(path) => Some(path),
            Self::DryRun => None,
        }
    }

    pub(super) fn overwrites_input(&self, input: &Path) -> bool {
        match self {
            Self::InPlace => true,
            Self::Explicit(path) => paths_refer_to_same_file(input, path),
            Self::DryRun => false,
        }
    }
}

pub(super) enum ParsedCommand {
    Help,
    Run(Command),
}

pub(super) struct CliError {
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

pub(super) fn parse_args(
    arguments: impl IntoIterator<Item = OsString>,
) -> Result<ParsedCommand, CliError> {
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
                    format!(
                        "unknown option: {}",
                        arguments[index].to_string_lossy().escape_debug()
                    ),
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
    let text = value.to_str()?.as_bytes();
    let limit = MAX_TIMEOUT.as_secs().saturating_add(1);
    let mut seconds = 0_u64;
    let mut decimal = false;
    let mut integer_digit = false;
    let mut fractional_digit = false;
    let mut fractional_value = false;
    for &byte in text {
        match byte {
            b'0'..=b'9' => {
                if decimal {
                    fractional_digit = true;
                    fractional_value |= byte != b'0';
                } else {
                    integer_digit = true;
                    seconds = seconds
                        .saturating_mul(10)
                        .saturating_add(u64::from(byte - b'0'))
                        .min(limit);
                }
            }
            b'.' if !decimal && integer_digit => decimal = true,
            _ => return None,
        }
    }
    if !integer_digit || (decimal && !fractional_digit) || (seconds == 0 && !fractional_value) {
        return None;
    }
    seconds = seconds
        .saturating_add(u64::from(fractional_value))
        .clamp(MIN_TIMEOUT.as_secs(), MAX_TIMEOUT.as_secs());
    Some(Duration::from_secs(seconds))
}

fn parse_strict(value: &OsStr) -> Option<bool> {
    match value.to_str()?.trim_start() {
        "0" => Some(false),
        "1" => Some(true),
        _ => None,
    }
}

fn timeout_error() -> CliError {
    CliError::message("--timeout requires a positive number of seconds", false)
}

fn strict_error() -> CliError {
    CliError::message("--strict requires 0 or 1", false)
}

fn output_error() -> CliError {
    CliError::message("--out requires an output filename", false)
}

pub(super) fn print_cli_error(
    output: &mut dyn Write,
    error: CliError,
    color: bool,
) -> io::Result<()> {
    if let Some(message) = error.message {
        writeln!(output, "{message}")?;
    }
    if error.show_usage {
        print_usage(output, color)?;
    }
    Ok(())
}

pub(super) fn print_usage(output: &mut dyn Write, color: bool) -> io::Result<()> {
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

#[cfg(test)]
mod tests;
