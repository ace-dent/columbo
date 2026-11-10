// SPDX-License-Identifier: MIT
// SPDX-FileCopyrightText: 2026 Andrew C.E. Dent <https://github.com/ace-dent>

use super::*;

#[test]
fn equal_frequency_arrangement_preserves_payload_cost_and_histogram() {
    let frequencies = [7, 2, 7, 7, 2, 1];
    let original = [4, 3, 2, 3, 5, 2];
    let original_cost: u32 = frequencies
        .iter()
        .zip(original)
        .map(|(&frequency, length)| frequency * u32::from(length))
        .sum();

    let mut ascending = original;
    arrange_equal_frequency_lengths(&frequencies, &mut ascending, false);
    assert_eq!(ascending, [2, 3, 3, 4, 5, 2]);

    let mut descending = original;
    arrange_equal_frequency_lengths(&frequencies, &mut descending, true);
    assert_eq!(descending, [4, 5, 3, 2, 3, 2]);

    for arranged in [ascending, descending] {
        let arranged_cost: u32 = frequencies
            .iter()
            .zip(arranged)
            .map(|(&frequency, length)| frequency * u32::from(length))
            .sum();
        let mut histogram = arranged;
        histogram.sort_unstable();
        let mut original_histogram = original;
        original_histogram.sort_unstable();
        assert_eq!(arranged_cost, original_cost);
        assert_eq!(histogram, original_histogram);
    }
}
