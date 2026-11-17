// SPDX-License-Identifier: MIT
// SPDX-FileCopyrightText: 2026 Andrew C.E. Dent <https://github.com/ace-dent>

//! CLI execution: read, optimize, and write each input.

mod arguments;
mod files;
mod report;

use std::env;
use std::io::{self, IsTerminal};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crate::optimize;

use crate::terminal;
use crate::terminal::spinner::Spinner;

use arguments::{parse_args, print_cli_error, print_usage, Command, Destination, ParsedCommand};
use files::{
    output_entry_exists, read_file, read_stdin, stdout_is_input, validate_output_destination,
    write_file, write_file_if_unchanged, write_new_file, write_stdout, ReadError,
};
use report::{
    print_blocking_chunk_notice, print_detailed_header, print_result, print_strict_mode_caution,
    print_timeout_notice, ExecutionTimings, InputReport, OptimizationReport, OutputAction,
    OutputChannel, ReportMode,
};

const PROGRAM_NAME: &str = "columbo";
const PROGRAM_VERSION: &str = env!("CARGO_PKG_VERSION");
const PROGRAM_STAGE: &str = "Beta";
/// Names for the pipes in reports and diagnostics.
const STDIN_NAME: &str = "<stdin>";
const STDOUT_NAME: &str = "<stdout>";

pub(crate) fn run() -> std::result::Result<(), u8> {
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
    if let Destination::Explicit(path) = &command.destination {
        if let Err(error) = validate_output_destination(path) {
            eprintln!(
                "cannot use --out destination {:?}: {}",
                path,
                error.to_string().escape_debug()
            );
            return Err(1);
        }
    }
    // Typing a file or printing one is never intended; the pipes exist for
    // scripts and tool chains.
    if command.stdin && io::stdin().is_terminal() {
        eprintln!("cannot read input '-' from a terminal; pipe or redirect a file");
        return Err(1);
    }
    if matches!(command.destination, Destination::Stdout) {
        if io::stdout().is_terminal() {
            eprintln!("cannot write binary output to a terminal; redirect stdout or use --out");
            return Err(1);
        }
        // Checked before reading, so `>>` cannot append to the input; a `>`
        // redirection has already emptied it.
        let input = command.inputs.first().map(PathBuf::as_path);
        if stdout_is_input(input) {
            eprintln!(
                "stdout is the input {:?}; redirect to another file \
                 (a > redirection empties the input before {PROGRAM_NAME} starts)",
                input.unwrap_or(Path::new(STDIN_NAME))
            );
            return Err(1);
        }
        terminal::send_reports_to_stderr();
    }

    let report_mode = ReportMode::for_options(&command.options);

    let input_count = command.inputs.len();
    let mut failed = false;
    let mut caution_printed = false;
    if command.stdin {
        failed = execute_file(&command, report_mode, None, 0, 1, &mut caution_printed).is_err();
    }
    for (index, input) in command.inputs.iter().enumerate() {
        // One input at a time bounds peak memory.
        if execute_file(
            &command,
            report_mode,
            Some(input),
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

/// Optimize one input: the file at `source`, or stdin when it is `None`.
fn execute_file(
    command: &Command,
    report_mode: ReportMode,
    source: Option<&Path>,
    input_index: usize,
    input_count: usize,
    caution_printed: &mut bool,
) -> std::result::Result<(), u8> {
    let input_path = source.unwrap_or(Path::new(STDIN_NAME));
    let overwrites_input = source.is_some_and(|path| command.destination.overwrites_input(path));
    let detailed = report_mode.detailed();
    let total_started = detailed.then(Instant::now);
    let read_started = detailed.then(Instant::now);
    let read = match source {
        Some(path) => read_file(path, command.options.max_input_bytes),
        None => read_stdin(command.options.max_input_bytes),
    };
    let input = match read {
        Ok(bytes) => bytes,
        Err(ReadError::TooLarge) => {
            eprintln!("input {:?} exceeds the 1 GiB file-size limit", input_path);
            return Err(1);
        }
        Err(ReadError::NotRegular) => {
            eprintln!("input {:?} is not a regular file", input_path);
            return Err(1);
        }
        Err(ReadError::Changed) => {
            eprintln!(
                "input {:?} changed while it was being read; skipped",
                input_path
            );
            return Err(1);
        }
        Err(ReadError::Allocation) => {
            eprintln!("not enough memory to read input {:?}", input_path);
            return Err(1);
        }
        Err(ReadError::Io(error)) => {
            // Escape terminal controls in both the path and error message.
            eprintln!(
                "could not read {:?}: {}",
                input_path,
                error.to_string().escape_debug()
            );
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
        OutputChannel::reports()
            .write(|output| print_detailed_header(output, command, &input_report, read_elapsed));
    }

    // Strictness applies to the whole batch, so print the caution once.
    if !command.options.strict && !*caution_printed {
        let channel = OutputChannel::Stderr;
        channel.write(|output| print_strict_mode_caution(output, channel.color_enabled()));
        *caution_printed = true;
    }

    let optimize_started = detailed.then(Instant::now);
    // The verbose reporter runs its own spinner.
    let spinner_deadline = Instant::now()
        .checked_add(command.options.timeout)
        .unwrap_or_else(Instant::now);
    let mut spinner = Spinner::start(!detailed, spinner_deadline);
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
        // Like a missing --out file, stdout receives the original when nothing
        // was saved, so a pipeline always ends with a complete file.
        None if matches!(command.destination, Destination::Stdout) => {
            let replace = optimized.should_replace();
            let bytes = if replace { &optimized.data } else { &input };
            if let Err(error) = write_stdout(bytes) {
                eprintln!(
                    "could not complete write to stdout: {}",
                    error.to_string().escape_debug()
                );
                return Err(1);
            }
            let output = PathBuf::from(STDOUT_NAME);
            if replace {
                OutputAction::WrittenOptimized(output)
            } else {
                OutputAction::CopiedOriginal(output)
            }
        }
        None => OutputAction::DryRun,
        Some(output) if optimized.should_replace() => {
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
                Err(error) => {
                    eprintln!(
                        "could not complete write to {:?}: {}",
                        output,
                        error.to_string().escape_debug()
                    );
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
                Err(error) => {
                    eprintln!(
                        "could not complete write to {:?}: {}",
                        output,
                        error.to_string().escape_debug()
                    );
                    return Err(1);
                }
            },
            Err(error) => {
                eprintln!(
                    "could not inspect {:?}: {}",
                    output,
                    error.to_string().escape_debug()
                );
                return Err(1);
            }
        },
    };
    let write_elapsed = if output_action.wrote_file() {
        write_started.map_or(Duration::ZERO, |started| started.elapsed())
    } else {
        Duration::ZERO
    };

    if let Some(chunk) = optimized.blocking_chunk() {
        let channel = OutputChannel::Stderr;
        channel.write(|output| {
            print_blocking_chunk_notice(output, input_path, chunk, channel.color_enabled())
        });
    }
    // Default output stays one line per file.
    if optimized.timed_out && detailed {
        OutputChannel::reports()
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
        removed_data_bytes: optimized.removed_data_bytes,
        replaces: optimized.should_replace(),
    };
    OutputChannel::reports().write(|output| {
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

#[cfg(test)]
mod test_support;
#[cfg(test)]
mod tests;
