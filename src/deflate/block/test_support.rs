// SPDX-License-Identifier: MIT
// SPDX-FileCopyrightText: 2026 Andrew C.E. Dent <https://github.com/ace-dent>

//! Cache limits and counters shared by planner tests.

use super::{CanonicalPlanCache, CanonicalPlanCacheStats};

impl CanonicalPlanCache {
    pub(super) fn with_limits(max_entries: usize, max_token_bytes: usize) -> Self {
        Self {
            max_entries,
            max_token_bytes,
            ..Self::new()
        }
    }

    pub(crate) fn stats(&self) -> CanonicalPlanCacheStats {
        self.stats
    }
}
