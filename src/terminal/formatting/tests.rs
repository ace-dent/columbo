// SPDX-License-Identifier: MIT

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
    write_spinner_line(&mut ordinary, "⠋", 4, true).unwrap();
    assert_eq!(
        String::from_utf8(ordinary).unwrap(),
        "\r\x1b[K\x1b[1m\x1b[36m⠋\x1b[39m optimizing · (timeout in 4 s)\x1b[0m"
    );

    let mut warning = Vec::new();
    write_spinner_line(&mut warning, "⠙", 3, true).unwrap();
    assert_eq!(
        String::from_utf8(warning).unwrap(),
        "\r\x1b[K\x1b[1m\x1b[36m⠙\x1b[39m optimizing · (timeout in \x1b[31m3 s\x1b[39m)\x1b[0m"
    );
}

#[test]
fn spinner_line_has_no_style_when_colour_is_disabled() {
    let mut line = Vec::new();
    write_spinner_line(&mut line, "⠋", 2, false).unwrap();
    assert_eq!(
        String::from_utf8(line).unwrap(),
        "\r\x1b[K⠋ optimizing · (timeout in 2 s)"
    );
}
