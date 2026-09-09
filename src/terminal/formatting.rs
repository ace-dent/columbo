// SPDX-License-Identifier: MIT

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

/// Round a remaining timeout up so a fresh 30-second allowance displays 30,
/// not 29, while an expired deadline remains at zero during route grace.
pub(crate) fn countdown_seconds(remaining: Duration) -> u64 {
    remaining
        .as_secs()
        .saturating_add(u64::from(remaining.subsec_nanos() != 0))
}

/// Render one overwrite-in-place spinner frame without allocating a line.
///
/// Styling deliberately uses only standard ANSI attributes and 16-colour
/// foreground codes. Resetting just the foreground keeps the whole line bold
/// while the spinner and final countdown change colour.
pub(crate) fn write_spinner_line(
    output: &mut dyn Write,
    frame: &str,
    seconds: u64,
    styled: bool,
) -> io::Result<()> {
    write!(output, "\r\x1b[K")?;
    if styled {
        write!(output, "\x1b[1m\x1b[36m{frame}\x1b[39m optimizing")?;
    } else {
        write!(output, "{frame} optimizing")?;
    }
    write!(output, " · (timeout in ")?;
    if styled && seconds <= 3 {
        write!(output, "\x1b[31m{seconds} s\x1b[39m")?;
    } else {
        write!(output, "{seconds} s")?;
    }
    if styled {
        write!(output, ")\x1b[0m")
    } else {
        write!(output, ")")
    }
}

#[cfg(test)]
mod tests;
