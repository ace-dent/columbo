// SPDX-License-Identifier: MIT

//! Per-file CLI summaries and output-channel selection.

use std::ffi::OsStr;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use columbo::Options;

use crate::terminal;
use crate::terminal::formatting::{format_duration as format_elapsed, plural, plural_u64};

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
}

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

    pub(super) fn write(self, operation: impl FnOnce(&mut dyn Write) -> io::Result<()>) {
        let result = match self {
            Self::Stdout => operation(&mut io::stdout().lock()),
            Self::Stderr => operation(&mut io::stderr().lock()),
        };
        let _ = result;
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ReportMode {
    Default,
    Verbose,
    Visual,
}

impl ReportMode {
    pub(super) fn for_options(options: &Options) -> Self {
        if options.visual {
            Self::Visual
        } else if options.verbose {
            Self::Verbose
        } else {
            Self::Default
        }
    }

    pub(super) fn channel(self) -> OutputChannel {
        match self {
            Self::Verbose => OutputChannel::Stdout,
            Self::Default | Self::Visual => OutputChannel::Stderr,
        }
    }

    pub(super) fn detailed(self) -> bool {
        !matches!(self, Self::Default)
    }

    fn label(self) -> &'static str {
        match self {
            Self::Default => "default",
            Self::Verbose => "verbose",
            Self::Visual => "visual",
        }
    }

    pub(super) fn reports_timeout(self) -> bool {
        self.detailed()
    }
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
        OutputAction::WrittenOptimized(_)
            if input_bytes == optimized_bytes && optimized.bits_saved == 0 =>
        {
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

pub(super) fn print_timeout_notice(
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

pub(super) fn print_detailed_header(
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

pub(super) fn print_strict_mode_caution(output: &mut dyn Write, color: bool) -> io::Result<()> {
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
            // CLI inputs are capped at 1 GiB. Widen before multiplying so
            // scaling to basis points is also safe on 32-bit targets.
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
