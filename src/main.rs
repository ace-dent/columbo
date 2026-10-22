// SPDX-License-Identifier: MIT
// SPDX-FileCopyrightText: 2026 Andrew C.E. Dent <https://github.com/ace-dent>

#![forbid(unsafe_code)]

use std::process::ExitCode;

fn main() -> ExitCode {
    match columbo::run_cli() {
        Ok(()) => ExitCode::SUCCESS,
        Err(code) => ExitCode::from(code),
    }
}
