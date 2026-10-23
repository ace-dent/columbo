// SPDX-License-Identifier: MIT
// SPDX-FileCopyrightText: 2026 Andrew C.E. Dent <https://github.com/ace-dent>

//! Per-file CLI reports: the verbose header, notices, and the result, which is
//! one line by default and a short block in verbose mode.

use std::ffi::OsStr;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::Options;

use crate::terminal;
use crate::terminal::formatting::{
    format_duration as format_elapsed, plural, plural_u64, style, YELLOW,
};

use super::arguments::Command;
use super::{PROGRAM_NAME, PROGRAM_STAGE, PROGRAM_VERSION};

pub(super) enum OutputAction {
    DryRun,
    WrittenOptimized(PathBuf),
    CopiedOriginal(PathBuf),
    Preserved(PathBuf),
}

impl OutputAction {
    pub(super) fn wrote_file(&self) -> bool {
        matches!(self, Self::WrittenOptimized(_) | Self::CopiedOriginal(_))
    }
}

pub(super) struct ExecutionTimings {
    pub(super) read: Duration,
    pub(super) optimize: Duration,
    pub(super) write: Duration,
    pub(super) total: Duration,
}

pub(super) struct InputReport<'a> {
    pub(super) path: &'a Path,
    pub(super) index: usize,
    pub(super) count: usize,
    pub(super) bytes: usize,
}

pub(super) struct OptimizationReport {
    pub(super) bytes: usize,
    pub(super) bits_saved: u64,
    pub(super) removed_data_bytes: u64,
}

/// Reports go to stdout; warnings, errors, and the spinner go to stderr.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum OutputChannel {
    Stdout,
    Stderr,
}

impl OutputChannel {
    pub(super) fn color_enabled(self) -> bool {
        match self {
            Self::Stdout => terminal::stdout_color_enabled(),
            Self::Stderr => terminal::stderr_color_enabled(),
        }
    }

    /// Run `operation` on the locked stream, ignoring write errors such as a
    /// closed pipe.
    pub(super) fn write(self, operation: impl FnOnce(&mut dyn Write) -> io::Result<()>) {
        let _ = match self {
            Self::Stdout => operation(&mut io::stdout().lock()),
            Self::Stderr => operation(&mut io::stderr().lock()),
        };
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ReportMode {
    /// One result line per file.
    Default,
    /// A header and result block per file, around the library's stream
    /// reports, with the timeout notice.
    Verbose,
}

impl ReportMode {
    pub(super) fn for_options(options: &Options) -> Self {
        if options.verbose {
            Self::Verbose
        } else {
            Self::Default
        }
    }

    pub(super) fn detailed(self) -> bool {
        self == Self::Verbose
    }
}

pub(super) fn print_detailed_header(
    output: &mut dyn Write,
    command: &Command,
    input: &InputReport<'_>,
    read_elapsed: Duration,
) -> io::Result<()> {
    writeln!(output)?;
    let title = format!("{PROGRAM_NAME} v{PROGRAM_VERSION} {PROGRAM_STAGE} · verbose");
    writeln!(output, "{title}")?;
    writeln!(output, "{}", "─".repeat(title.chars().count()))?;
    if input.count > 1 {
        writeln!(output, "File     {} of {}", input.index + 1, input.count)?;
    }
    writeln!(
        output,
        "Input    {:?} · {} {} · read {}",
        display_name(input.path),
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
        )
    } else {
        writeln!(output, "Mode     normal · {strictness}{dry_run}")
    }
}

pub(super) fn print_strict_mode_caution(output: &mut dyn Write, color: bool) -> io::Result<()> {
    let (yellow, reset) = style(color, YELLOW);
    writeln!(
        output,
        "{yellow}Caution:{reset} strict mode disabled; enabling compact empty/singleton \
         Huffman alphabets and the non-standard length-258 alias"
    )
}

pub(super) fn print_blocking_chunk_notice(
    output: &mut dyn Write,
    input_path: &Path,
    chunk: &str,
    color: bool,
) -> io::Result<()> {
    let (yellow, reset) = style(color, YELLOW);
    writeln!(
        output,
        "{yellow}Note:{reset} {chunk} chunk in {input_path:?} prevents optimization; \
         run again with --strip to remove it"
    )
}

pub(super) fn print_timeout_notice(
    output: &mut dyn Write,
    action: &OutputAction,
    timeout: Duration,
) -> io::Result<()> {
    let seconds = timeout.as_secs();
    let outcome = match action {
        OutputAction::DryRun => "reporting best result found so far",
        OutputAction::WrittenOptimized(_) => "wrote best output found so far",
        OutputAction::CopiedOriginal(_) | OutputAction::Preserved(_) => {
            "no smaller output was written"
        }
    };
    writeln!(
        output,
        "Timeout triggered after {seconds} seconds; {outcome}."
    )
}

pub(super) fn print_result(
    output: &mut dyn Write,
    report_mode: ReportMode,
    input: &InputReport<'_>,
    action: &OutputAction,
    optimized: &OptimizationReport,
    timings: &ExecutionTimings,
) -> io::Result<()> {
    match report_mode {
        ReportMode::Default => print_default_result(output, input, action, optimized),
        ReportMode::Verbose => print_detailed_result(output, input, action, optimized, timings),
    }?;
    if optimized.removed_data_bytes != 0 {
        writeln!(
            output,
            "  Stripped {} bytes of extraneous data (separate from Deflate savings)",
            optimized.removed_data_bytes
        )?;
    }
    Ok(())
}

fn print_default_result(
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
        OutputAction::WrittenOptimized(_) if input_bytes == optimized_bytes && bits_saved == 0 => {
            writeln!(
                output,
                "{input_name:?} {input_bytes} -> {optimized_bytes} bytes \
                 (representation normalized; no size saving)"
            )
        }
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
             (no savings; copied original to {output_path:?})"
        ),
        OutputAction::Preserved(output_path) => writeln!(
            output,
            "{input_name:?} {input_bytes} -> {input_bytes} bytes \
             (no savings; left {output_path:?} unchanged)"
        ),
    }
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
            writeln!(output, "  Output  {:?}", display_name(path))?;
        }
        OutputAction::CopiedOriginal(path) => writeln!(
            output,
            "  Output  {:?} · original copied · no savings",
            display_name(path)
        )?,
        OutputAction::Preserved(path) => writeln!(
            output,
            "  Output  {:?} · preserved · no savings",
            display_name(path)
        )?,
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
    let write = if action.wrote_file() {
        format!(" · write {}", format_elapsed(timings.write))
    } else {
        String::new()
    };
    writeln!(
        output,
        "  Time    {} total · read {} · optimize {}{write}",
        format_elapsed(timings.total),
        format_elapsed(timings.read),
        format_elapsed(timings.optimize),
    )
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
            // Scale in u64: a 1 GiB input times 10,000 overflows 32-bit `usize`.
            let input = input_bytes as u64;
            let percentage_hundredths = (saved as u64 * 10_000 + input / 2) / input;
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

#[cfg(test)]
mod tests;
