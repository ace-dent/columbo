// SPDX-License-Identifier: MIT

//! CLI execution: read, optimize, and write each input.

mod arguments;
mod files;
mod report;

use std::env;
use std::path::Path;
use std::time::{Duration, Instant};

use columbo::optimize;

use crate::terminal::{self, spinner::Spinner};

use arguments::{parse_args, print_cli_error, print_usage, Command, Destination, ParsedCommand};
use files::{
    output_entry_exists, read_file, write_file, write_file_if_unchanged, write_new_file, ReadError,
};
use report::{
    print_detailed_header, print_result, print_strict_mode_caution, print_timeout_notice,
    ExecutionTimings, InputReport, OptimizationReport, OutputAction, OutputChannel, ReportMode,
};

const PROGRAM_NAME: &str = "columbo";
const PROGRAM_VERSION: &str = concat!(
    env!("CARGO_PKG_VERSION_MAJOR"),
    ".",
    env!("CARGO_PKG_VERSION_MINOR")
);
const PROGRAM_STAGE: &str = "Beta";

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
    let report_mode = ReportMode::for_options(&command.options);
    if command.options.visual && !terminal::stderr_interactive() {
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
    // Detailed reporters start the same spinner once their output is ready.
    let spinner_deadline = Instant::now()
        .checked_add(command.options.timeout)
        .unwrap_or_else(Instant::now);
    let mut spinner = Spinner::start(report_mode == ReportMode::Default, spinner_deadline);
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
                        "could not write {:?}: {}",
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
                        "could not write {:?}: {}",
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

#[cfg(test)]
mod test_support;
#[cfg(test)]
mod tests;
