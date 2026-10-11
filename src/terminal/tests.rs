// SPDX-License-Identifier: MIT
// SPDX-FileCopyrightText: 2026 Andrew C.E. Dent <https://github.com/ace-dent>

use super::*;

#[test]
fn no_color_disables_styling_even_when_empty() {
    assert!(!color_enabled_for(true, Some(OsString::new())));
    assert!(!color_enabled_for(true, Some(OsString::from("1"))));
    assert!(color_enabled_for(true, None));
}

#[test]
fn styling_also_requires_escape_sequences() {
    assert!(!color_enabled_for(false, None));
}

#[test]
fn escapes_need_a_terminal_that_is_not_dumb() {
    let xterm = Some(OsStr::new("xterm"));
    let dumb = Some(OsStr::new("dumb"));
    for host in [false, true] {
        assert!(!escapes_enabled_for(false, xterm, host));
        assert!(!escapes_enabled_for(true, dumb, host));
        assert!(escapes_enabled_for(true, xterm, host));
    }
}

#[test]
fn without_term_escapes_follow_the_host() {
    assert!(escapes_enabled_for(true, None, true));
    assert!(!escapes_enabled_for(true, None, false));
}

#[test]
fn windows_hosts_announce_escape_support() {
    let set = Some(OsStr::new("1"));
    assert!(!windows_host_announces_escapes(None, None, None));
    assert!(windows_host_announces_escapes(set, None, None));
    assert!(windows_host_announces_escapes(None, set, None));
    assert!(windows_host_announces_escapes(
        None,
        None,
        Some(OsStr::new("ON"))
    ));
    assert!(!windows_host_announces_escapes(
        None,
        None,
        Some(OsStr::new("OFF"))
    ));
}
