// SPDX-License-Identifier: MIT
// SPDX-FileCopyrightText: 2026 Andrew C.E. Dent <https://github.com/ace-dent>

use super::{EMPTY_SLOT, SWAP_TRIAL_COSTS};

/// The trials this thread keeps.
pub(in crate::deflate::header) fn kept_swap_trial_count() -> usize {
    SWAP_TRIAL_COSTS.with(|costs| {
        costs
            .borrow()
            .slots
            .iter()
            .filter(|&&slot| slot != EMPTY_SLOT)
            .count()
    })
}
