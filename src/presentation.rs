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
    checked: Option<(usize, usize)>,
    styled: bool,
) -> io::Result<()> {
    write!(output, "\r\x1b[K")?;
    if styled {
        write!(output, "\x1b[1m\x1b[36m{frame}\x1b[39m optimizing")?;
    } else {
        write!(output, "{frame} optimizing")?;
    }
    if let Some((done, total)) = checked {
        write!(output, " · {done}/{total} checked")?;
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
mod tests {
    use super::*;

    #[test]
    fn countdown_rounds_positive_fractions_up_and_expiry_to_zero() {
        assert_eq!(countdown_seconds(Duration::ZERO), 0);
        assert_eq!(countdown_seconds(Duration::from_nanos(1)), 1);
        assert_eq!(countdown_seconds(Duration::from_secs(29)), 29);
        assert_eq!(countdown_seconds(Duration::from_millis(29_001)), 30);
    }

    #[test]
    fn spinner_line_is_bold_and_warns_during_the_final_three_seconds() {
        let mut ordinary = Vec::new();
        write_spinner_line(&mut ordinary, "⠋", 4, Some((2, 5)), true).unwrap();
        assert_eq!(
            String::from_utf8(ordinary).unwrap(),
            "\r\x1b[K\x1b[1m\x1b[36m⠋\x1b[39m optimizing · 2/5 checked · (timeout in 4 s)\x1b[0m"
        );

        let mut warning = Vec::new();
        write_spinner_line(&mut warning, "⠙", 3, None, true).unwrap();
        assert_eq!(
            String::from_utf8(warning).unwrap(),
            "\r\x1b[K\x1b[1m\x1b[36m⠙\x1b[39m optimizing · (timeout in \x1b[31m3 s\x1b[39m)\x1b[0m"
        );
    }

    #[test]
    fn spinner_line_has_no_style_when_colour_is_disabled() {
        let mut line = Vec::new();
        write_spinner_line(&mut line, "⠋", 2, None, false).unwrap();
        assert_eq!(
            String::from_utf8(line).unwrap(),
            "\r\x1b[K⠋ optimizing · (timeout in 2 s)"
        );
    }
}
