// SPDX-License-Identifier: MIT

use std::env;
use std::ffi::OsString;
use std::fs;
use std::io;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use columbo::{Format, Options};

use super::arguments::{parse_args, Command, ParsedCommand};

const TEMP_FILE_ATTEMPTS: usize = 128;
static TEMP_FILE_COUNTER: AtomicU64 = AtomicU64::new(0);

pub(super) fn unique_test_directory() -> PathBuf {
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

pub(super) fn parsed_options<const N: usize>(arguments: [&str; N]) -> Options {
    parsed_command(arguments).options
}

pub(super) fn parsed_command<const N: usize>(arguments: [&str; N]) -> Command {
    let parsed = match parse_args(arguments.into_iter().map(OsString::from)) {
        Ok(parsed) => parsed,
        Err(_) => panic!("expected valid command arguments"),
    };
    match parsed {
        ParsedCommand::Run(command) => command,
        ParsedCommand::Help => panic!("expected a runnable command"),
    }
}

pub(super) fn parsed_format<const N: usize>(arguments: [&str; N]) -> Format {
    parsed_command(arguments).format
}
