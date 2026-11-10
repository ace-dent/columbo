// SPDX-License-Identifier: MIT
// SPDX-FileCopyrightText: 2026 Andrew C.E. Dent <https://github.com/ace-dent>

use super::super::test_support::comparison_candidate;
use super::*;
use crate::deflate::parse::parse_stream;

#[test]
fn compact_split_parents_are_ordered_by_complete_stream_score() {
    fn seed(bytes: usize, bits: u64) -> CompactSplitSeed {
        CompactSplitSeed {
            data: vec![0; bytes],
            bits,
            stream: parse_stream(&[0x03, 0x00], 1).unwrap(),
        }
    }

    let largest = seed(12, 80);
    let bit_tie = seed(10, 79);
    let smallest = seed(10, 78);
    let ordered = ordered_compact_split_seeds([Some(&largest), Some(&bit_tie), Some(&smallest)]);

    assert_eq!(
        ordered
            .iter()
            .map(|seed| (seed.data.len(), seed.bits))
            .collect::<Vec<_>>(),
        [(10, 78), (10, 79), (12, 80)]
    );

    let tied_first = seed(10, 78);
    let tied_second = seed(10, 78);
    let ordered =
        ordered_compact_split_seeds([Some(&tied_first), Some(&tied_second), Some(&largest)]);
    assert!(std::ptr::eq(ordered[0], &tied_first));
    assert!(std::ptr::eq(ordered[1], &tied_second));
}

#[test]
fn completed_compact_split_parent_requires_exact_encoded_identity() {
    let candidate = comparison_candidate(4, 29, 7);

    assert!(compact_split_parent_is_completed(
        &candidate,
        Some(&[7, 7, 7, 7])
    ));
    assert!(!compact_split_parent_is_completed(
        &candidate,
        Some(&[7, 7, 7, 6])
    ));
    assert!(!compact_split_parent_is_completed(&candidate, None));
}
