// SPDX-License-Identifier: MIT
// SPDX-FileCopyrightText: 2026 Andrew C.E. Dent <https://github.com/ace-dent>

//! Shared, dependency-free text formatting for CLI and progress output.

use std::io::{self, Write};
use std::time::Duration;

pub(crate) fn format_duration(duration: Duration) -> String {
    if duration < Duration::from_millis(1) {
        format!("{} µs", duration.subsec_micros())
    } else if duration < Duration::from_secs(1) {
        let centimilliseconds = (duration.subsec_micros() + 5) / 10;
        format!(
            "{}.{:02} ms",
            centimilliseconds / 100,
            centimilliseconds % 100
        )
    } else if duration < Duration::from_secs(10) {
        let mut seconds = duration.as_secs();
        let mut centiseconds = (duration.subsec_micros() + 5_000) / 10_000;
        if centiseconds == 100 {
            seconds += 1;
            centiseconds = 0;
        }
        format!("{seconds}.{centiseconds:02} s")
    } else {
        let mut seconds = duration.as_secs();
        let mut deciseconds = (duration.subsec_millis() + 50) / 100;
        if deciseconds == 10 {
            seconds = seconds.saturating_add(1);
            deciseconds = 0;
        }
        format!("{seconds}.{deciseconds} s")
    }
}

pub(crate) fn plural<'a>(count: usize, singular: &'a str, plural: &'a str) -> &'a str {
    if count == 1 {
        singular
    } else {
        plural
    }
}

pub(crate) fn plural_u64<'a>(count: u64, singular: &'a str, plural: &'a str) -> &'a str {
    if count == 1 {
        singular
    } else {
        plural
    }
}

/// Round the remaining time up to whole seconds, so a fresh 30-second timeout
/// shows 30, not 29, and an expired deadline shows 0 during the grace period.
pub(crate) fn countdown_seconds(remaining: Duration) -> u64 {
    remaining
        .as_secs()
        .saturating_add(u64::from(remaining.subsec_nanos() != 0))
}

/// How a spinner frame replaces the previous frame on its line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SpinnerStyle {
    /// Erase the line with ANSI, optionally styling the frame.
    Ansi { color: bool },
    /// Return to the line start and pad over the previous frame with spaces,
    /// for a console that would print escape sequences literally.
    Plain,
}

/// Write one spinner frame over the current line without allocating.
///
/// Styling uses only standard ANSI attributes and 16-color foreground codes.
/// Resetting just the foreground keeps the whole line bold while the spinner
/// and final countdown change color. A plain frame is padded to
/// `previous_width` columns. Returns the frame's own width.
pub(crate) fn write_spinner_line(
    output: &mut dyn Write,
    frame: &str,
    seconds: u64,
    style: SpinnerStyle,
    previous_width: usize,
) -> io::Result<usize> {
    let styled = style == SpinnerStyle::Ansi { color: true };
    if style == SpinnerStyle::Plain {
        write!(output, "\r")?;
    } else {
        write!(output, "\r\x1b[K")?;
    }
    if styled {
        write!(output, "\x1b[1m\x1b[36m{frame}\x1b[39m optimizing")?;
    } else {
        write!(output, "{frame} optimizing")?;
    }
    let mut width = frame.chars().count() + " optimizing".len();
    if seconds == 0 {
        let status = " · (concluding work)";
        write!(output, "{status}")?;
        width += status.chars().count();
    } else {
        write!(output, " · (timeout in ")?;
        if styled && seconds <= 3 {
            write!(output, "\x1b[31m{seconds} s\x1b[39m")?;
        } else {
            write!(output, "{seconds} s")?;
        }
        write!(output, ")")?;
        let digits = seconds.checked_ilog10().map_or(1, |log| log as usize + 1);
        width += " · (timeout in ".chars().count() + digits + " s)".len();
    }
    if styled {
        write!(output, "\x1b[0m")?;
    }
    if style == SpinnerStyle::Plain {
        write!(output, "{:1$}", "", previous_width.saturating_sub(width))?;
    }
    Ok(width)
}

/// Remove a spinner frame of `width` columns, leaving the cursor at the start
/// of its line.
pub(crate) fn clear_spinner_line(
    output: &mut dyn Write,
    style: SpinnerStyle,
    width: usize,
) -> io::Result<()> {
    if style == SpinnerStyle::Plain {
        write!(output, "\r{:1$}\r", "", width)
    } else {
        write!(output, "\r\x1b[K")
    }
}

#[cfg(test)]
mod tests;
