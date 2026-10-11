// SPDX-License-Identifier: MIT
// SPDX-FileCopyrightText: 2026 Andrew C.E. Dent <https://github.com/ace-dent>

//! Heap-built code lengths shared by Huffman and search tests.

use super::make_lengths_deflopt_heap_into;

/// Build the code lengths of DeflOpt's frequency/height heap.
pub(crate) fn make_lengths_deflopt_heap(
    frequencies: &[u32],
    max_bits: u8,
    variant: u32,
) -> Vec<u8> {
    let mut lengths = vec![0; frequencies.len()];
    make_lengths_deflopt_heap_into(frequencies, &mut lengths, max_bits, variant);
    lengths
}
