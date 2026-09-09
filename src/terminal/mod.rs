// SPDX-License-Identifier: MIT

//! Shared terminal capability policy for library progress reporters and the
//! CLI.

pub(crate) mod formatting;
pub(crate) mod spinner;

use std::env;
use std::ffi::{OsStr, OsString};
use std::io::{self, IsTerminal};

pub(crate) fn stdout_color_enabled() -> bool {
    color_enabled_for(
        io::stdout().is_terminal(),
        env::var_os("NO_COLOR"),
        env::var_os("TERM"),
    )
}

pub(crate) fn stderr_color_enabled() -> bool {
    color_enabled_for(
        io::stderr().is_terminal(),
        env::var_os("NO_COLOR"),
        env::var_os("TERM"),
    )
}

pub(crate) fn stderr_interactive() -> bool {
    io::stderr().is_terminal()
        && env::var_os("TERM").map_or(true, |term| term != OsStr::new("dumb"))
}

pub(crate) fn color_enabled_for(
    is_terminal: bool,
    no_color: Option<OsString>,
    term: Option<OsString>,
) -> bool {
    is_terminal
        && no_color.is_none()
        && term
            .as_deref()
            .map_or(true, |value| value != OsStr::new("dumb"))
}

#[cfg(test)]
mod tests;
