// SPDX-License-Identifier: MIT
// SPDX-FileCopyrightText: 2026 Andrew C.E. Dent <https://github.com/ace-dent>

use super::*;

#[test]
fn long_route_heartbeat_cadence_is_bounded() {
    assert_eq!(
        route_heartbeat_interval(Duration::from_secs(10)),
        Duration::from_secs(3)
    );
    assert_eq!(
        route_heartbeat_interval(Duration::from_secs(180)),
        Duration::from_secs(6)
    );
    assert_eq!(
        route_heartbeat_interval(Duration::from_secs(4_000)),
        Duration::from_secs(60)
    );
}
