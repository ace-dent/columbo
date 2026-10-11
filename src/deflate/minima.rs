// SPDX-License-Identifier: MIT
// SPDX-FileCopyrightText: 2026 Andrew C.E. Dent <https://github.com/ace-dent>

//! Range minima shared by searches over the lengths of one length symbol.

/// Sparse table of range minima over positions, filled from the end backward.
///
/// One length symbol covers at most 32 lengths, so six power-of-two levels
/// answer each query with two lookups. Entries store positions so that equal
/// costs resolve to the lowest position.
pub(crate) struct SuffixMinima {
    positions: [[u16; 259]; 6],
}

impl SuffixMinima {
    pub(crate) fn new(end: usize) -> Self {
        Self {
            positions: [[end as u16; 259]; 6],
        }
    }

    pub(crate) fn insert(&mut self, start: usize, end: usize, costs: &[u64; 259]) {
        self.positions[0][start] = start as u16;
        for level in 1..self.positions.len() {
            let half = 1 << (level - 1);
            if start + 2 * half > end + 1 {
                break;
            }
            let a = self.positions[level - 1][start];
            let b = self.positions[level - 1][start + half];
            self.positions[level][start] = if costs[usize::from(a)] <= costs[usize::from(b)] {
                a
            } else {
                b
            };
        }
    }

    pub(crate) fn minimum(&self, start: usize, end: usize, costs: &[u64; 259]) -> usize {
        let width = end - start + 1;
        let level = (usize::BITS - 1 - width.leading_zeros()) as usize;
        let a = usize::from(self.positions[level][start]);
        let b = usize::from(self.positions[level][end + 1 - (1 << level)]);
        if (costs[a], a) <= (costs[b], b) {
            a
        } else {
            b
        }
    }
}
