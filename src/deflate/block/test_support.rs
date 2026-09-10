// SPDX-License-Identifier: MIT

//! Cache controls and observations shared by planner tests.

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
