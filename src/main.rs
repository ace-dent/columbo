// SPDX-License-Identifier: MIT

#![forbid(unsafe_code)]

mod cli;
mod terminal;

use std::process::ExitCode;

fn main() -> ExitCode {
    match cli::run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(code) => ExitCode::from(code),
    }
}
