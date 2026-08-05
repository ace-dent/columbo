// SPDX-License-Identifier: MIT

//! Shared terminal capability policy for the library progress reporters and CLI.

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
mod tests {
    use super::*;

    #[test]
    fn no_color_disables_styling_even_when_empty() {
        assert!(!color_enabled_for(
            true,
            Some(OsString::new()),
            Some(OsString::from("xterm"))
        ));
        assert!(!color_enabled_for(
            true,
            Some(OsString::from("1")),
            Some(OsString::from("xterm"))
        ));
    }

    #[test]
    fn styling_also_requires_a_capable_terminal() {
        assert!(!color_enabled_for(false, None, None));
        assert!(!color_enabled_for(true, None, Some(OsString::from("dumb"))));
        assert!(color_enabled_for(true, None, Some(OsString::from("xterm"))));
    }
}
