// SPDX-License-Identifier: MIT
// SPDX-FileCopyrightText: 2026 Andrew C.E. Dent <https://github.com/ace-dent>

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
    let style = SpinnerStyle::Ansi { color: true };
    let mut ordinary = Vec::new();
    write_spinner_line(&mut ordinary, "⠋", 4, style, 0).unwrap();
    assert_eq!(
        String::from_utf8(ordinary).unwrap(),
        "\r\x1b[K\x1b[1m\x1b[36m⠋\x1b[39m optimizing · (timeout in 4 s)\x1b[0m"
    );

    let mut warning = Vec::new();
    write_spinner_line(&mut warning, "⠙", 3, style, 0).unwrap();
    assert_eq!(
        String::from_utf8(warning).unwrap(),
        "\r\x1b[K\x1b[1m\x1b[36m⠙\x1b[39m optimizing · (timeout in \x1b[31m3 s\x1b[39m)\x1b[0m"
    );
}

#[test]
fn spinner_line_has_no_style_when_color_is_disabled() {
    let mut line = Vec::new();
    write_spinner_line(&mut line, "⠋", 2, SpinnerStyle::Ansi { color: false }, 0).unwrap();
    assert_eq!(
        String::from_utf8(line).unwrap(),
        "\r\x1b[K⠋ optimizing · (timeout in 2 s)"
    );
}

#[test]
fn plain_spinner_overwrites_and_clears_without_escape_sequences() {
    let mut first = Vec::new();
    let width = write_spinner_line(&mut first, "|", 100, SpinnerStyle::Plain, 0).unwrap();
    let first = String::from_utf8(first).unwrap();
    assert_eq!(first, "\r| optimizing · (timeout in 100 s)");
    assert_eq!(width, first.chars().count() - 1);

    // A shorter countdown and the final status pad over the longer frame.
    let mut shorter = Vec::new();
    let shorter_width =
        write_spinner_line(&mut shorter, "/", 9, SpinnerStyle::Plain, width).unwrap();
    assert_eq!(
        String::from_utf8(shorter).unwrap(),
        "\r/ optimizing · (timeout in 9 s)  "
    );
    assert_eq!(shorter_width, width - 2);
    let mut concluding = Vec::new();
    let concluding_width =
        write_spinner_line(&mut concluding, "-", 0, SpinnerStyle::Plain, width).unwrap();
    assert_eq!(
        String::from_utf8(concluding).unwrap(),
        "\r- optimizing · (concluding work) "
    );
    assert_eq!(concluding_width, width - 1);

    let mut cleared = Vec::new();
    clear_spinner_line(&mut cleared, SpinnerStyle::Plain, width).unwrap();
    assert_eq!(
        String::from_utf8(cleared).unwrap(),
        format!("\r{}\r", " ".repeat(width))
    );
    let mut erased = Vec::new();
    clear_spinner_line(&mut erased, SpinnerStyle::Ansi { color: false }, width).unwrap();
    assert_eq!(String::from_utf8(erased).unwrap(), "\r\x1b[K");
}
