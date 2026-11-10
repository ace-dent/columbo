// SPDX-License-Identifier: MIT
// SPDX-FileCopyrightText: 2026 Andrew C.E. Dent <https://github.com/ace-dent>

use super::HEADER_PLAN_STORE;

/// The plans in this thread's header-plan store.
pub(in crate::deflate::header) fn stored_header_plan_count() -> usize {
    HEADER_PLAN_STORE.with(|store| {
        let store = store.borrow();
        store.current.offsets.len() + store.previous.offsets.len()
    })
}
