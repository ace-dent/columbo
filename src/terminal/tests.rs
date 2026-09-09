// SPDX-License-Identifier: MIT

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
