// SPDX-License-Identifier: MIT
// SPDX-FileCopyrightText: 2026 Andrew C.E. Dent <https://github.com/ace-dent>

//! Terminal capability checks shared by the library's verbose reporter and the
//! CLI.

pub(crate) mod formatting;
pub(crate) mod spinner;

use std::env;
use std::ffi::{OsStr, OsString};
use std::io::{self, IsTerminal, Write};
use std::sync::atomic::{AtomicBool, Ordering};

/// Set once by the CLI before optimizing when stdout carries output data.
static REPORTS_ON_STDERR: AtomicBool = AtomicBool::new(false);

/// `env::var_os`, compiled once for every check here.
#[inline(never)]
fn env_value(name: &str) -> Option<OsString> {
    env::var_os(name)
}

/// Send the CLI's and the verbose reporter's reports to stderr for the rest of
/// the process, keeping stdout for output data.
pub(crate) fn send_reports_to_stderr() {
    REPORTS_ON_STDERR.store(true, Ordering::Relaxed);
}

pub(crate) fn reports_on_stderr() -> bool {
    REPORTS_ON_STDERR.load(Ordering::Relaxed)
}

pub(crate) fn report_color_enabled() -> bool {
    if reports_on_stderr() {
        stderr_color_enabled()
    } else {
        stdout_color_enabled()
    }
}

/// Run `operation` on the locked report stream, ignoring write errors such as
/// a closed pipe. Out of line, so report writers share one copy of the stream
/// choice.
#[inline(never)]
pub(crate) fn write_report(operation: &mut dyn FnMut(&mut dyn Write) -> io::Result<()>) {
    let _ = if reports_on_stderr() {
        operation(&mut io::stderr().lock())
    } else {
        operation(&mut io::stdout().lock())
    };
}

pub(crate) fn stdout_color_enabled() -> bool {
    color_enabled_for(stdout_escapes_enabled(), env_value("NO_COLOR"))
}

pub(crate) fn stderr_color_enabled() -> bool {
    color_enabled_for(stderr_escapes_enabled(), env_value("NO_COLOR"))
}

pub(crate) fn stderr_interactive() -> bool {
    io::stderr().is_terminal() && !term_is_dumb(env_value("TERM").as_deref())
}

fn stdout_escapes_enabled() -> bool {
    escapes_enabled_for(
        io::stdout().is_terminal(),
        env_value("TERM").as_deref(),
        host_announces_escapes(),
    )
}

/// Whether stderr interprets ANSI escape sequences for color and cursor control.
pub(crate) fn stderr_escapes_enabled() -> bool {
    escapes_enabled_for(
        io::stderr().is_terminal(),
        env_value("TERM").as_deref(),
        host_announces_escapes(),
    )
}

/// Decide whether a terminal stream may receive ANSI escape sequences.
///
/// A `TERM` value decides when present: `dumb` cannot interpret escapes, any
/// other value can. Without `TERM`, a Unix terminal is assumed capable; a
/// Windows host must announce support, because enabling virtual-terminal
/// processing needs FFI, which this crate forbids.
pub(crate) fn escapes_enabled_for(
    is_terminal: bool,
    term: Option<&OsStr>,
    host_announces_escapes: bool,
) -> bool {
    is_terminal
        && match term {
            Some(term) => !term_is_dumb(Some(term)),
            None => host_announces_escapes,
        }
}

fn host_announces_escapes() -> bool {
    !cfg!(windows)
        || windows_host_announces_escapes(
            env_value("WT_SESSION").as_deref(),
            env_value("ANSICON").as_deref(),
            env_value("ConEmuANSI").as_deref(),
        )
}

/// Windows hosts that interpret escapes without the program enabling
/// virtual-terminal processing: Windows Terminal, ANSICON, and ConEmu with
/// its ANSI processing on.
pub(crate) fn windows_host_announces_escapes(
    wt_session: Option<&OsStr>,
    ansicon: Option<&OsStr>,
    conemu_ansi: Option<&OsStr>,
) -> bool {
    wt_session.is_some() || ansicon.is_some() || conemu_ansi == Some(OsStr::new("ON"))
}

fn term_is_dumb(term: Option<&OsStr>) -> bool {
    term == Some(OsStr::new("dumb"))
}

pub(crate) fn color_enabled_for(escapes_enabled: bool, no_color: Option<OsString>) -> bool {
    escapes_enabled && no_color.is_none()
}

#[cfg(test)]
mod tests;
