// SPDX-License-Identifier: MIT
// SPDX-FileCopyrightText: 2026 Andrew C.E. Dent <https://github.com/ace-dent>

use super::*;
use crate::deflate::header::reorder_code_length_lengths;
use crate::deflate::huffman::make_lengths_defluff_exact;

#[test]
fn code_length_tree_entries_keep_their_size() {
    assert_eq!(std::mem::size_of::<CodeLengthTreeEntry>(), 472);
}

#[test]
fn a_first_tree_at_the_least_cost_prices_no_worse_than_any_builder() {
    // The feedback pass prices only its first tree when that tree is an
    // unrepaired Huffman tree or, in the exhaustive search, a repaired tree
    // that costs what Defluff's exact package-merge code does: every other
    // builder's usable code, reordered or not, must cost at least as many RLE
    // bits and header bits.
    let mut state = 0x510e_527f_ade6_82d1_u64;
    let mut next = move || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state
    };
    let builders = (0..4)
        .flat_map(|variant| {
            [
                CodeLengthBuilder::DefloptHeap(variant),
                CodeLengthBuilder::OrderHeap(variant),
                CodeLengthBuilder::Columbo(variant),
            ]
        })
        .chain([
            CodeLengthBuilder::Deft4jJavaHeap,
            CodeLengthBuilder::DefluffLimited,
            CodeLengthBuilder::DefluffExact,
        ]);
    let builders: Vec<_> = builders.collect();
    let header_bits = |frequencies: &[u32; 19], lengths: &[u8; 19]| {
        let cost = rle_frequency_cost(frequencies, lengths);
        (cost != INF).then(|| 17 + 3 * trim_code_lengths(lengths) as u64 + cost)
    };
    let mut scratch = TreeBuildScratch::default();
    let mut shortcuts = [0; 2];
    for case in 0..20_000 {
        // Sparse, skewed and tied counts; some cases need repair.
        let mut frequencies = [0_u32; 19];
        for frequency in &mut frequencies {
            *frequency = match next() % 4 {
                0 => 0,
                1 => (next() % 4) as u32,
                2 => (next() % [16, 64, 200][case % 3]) as u32,
                _ => 1 << (next() % [4, 8, 12][case % 3]),
            };
        }
        let first = CodeLengthBuilder::OrderHeap(0).build(&frequencies, false, &mut scratch);
        if !code_length_tree_shape_is_valid(&first.lengths) {
            continue;
        }
        let first_cost = rle_frequency_cost(&frequencies, &first.lengths);
        let exact = CodeLengthBuilder::DefluffExact.build(&frequencies, false, &mut scratch);
        // The kept scratch builds what a fresh one does.
        assert_eq!(
            exact.lengths[..],
            make_lengths_defluff_exact(&frequencies, 7, 0)[..]
        );
        if first.huffman {
            shortcuts[0] += 1;
            assert!(code_lengths_follow_frequencies(
                &first.lengths,
                &frequencies
            ));
        } else if first_cost != INF
            && first_cost == rle_frequency_cost(&frequencies, &exact.lengths)
        {
            shortcuts[1] += 1;
        } else {
            continue;
        }
        let first_bits = header_bits(&frequencies, &first.lengths).unwrap();
        for &builder in &builders {
            let mut lengths = builder.build(&frequencies, false, &mut scratch).lengths;
            if !code_length_tree_shape_is_valid(&lengths) {
                continue;
            }
            for _ in 0..2 {
                if let Some(bits) = header_bits(&frequencies, &lengths) {
                    assert!(
                        rle_frequency_cost(&frequencies, &lengths) >= first_cost
                            && bits >= first_bits,
                        "{builder:?} {frequencies:?}",
                    );
                }
                reorder_code_length_lengths(&mut lengths, &frequencies);
            }
        }
    }
    assert!(
        shortcuts[0] > 10_000 && shortcuts[1] > 2_000,
        "{shortcuts:?} shortcut cases"
    );
}

#[test]
fn shared_code_length_trees_match_each_builders_own_tree() {
    // A histogram's first heap tree is cached for every heap variant known to
    // build it; whatever order the builders ask in, each gets its own tree.
    let mut state = 0x1f83_d9ab_fb41_bd6b_u64;
    let mut next = move || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state
    };
    let mut builders: Vec<_> = (0..4)
        .flat_map(|variant| {
            [
                CodeLengthBuilder::DefloptHeap(variant),
                CodeLengthBuilder::OrderHeap(variant),
                CodeLengthBuilder::Columbo(variant),
            ]
        })
        .chain([
            CodeLengthBuilder::Deft4jJavaHeap,
            CodeLengthBuilder::DefluffLimited,
            CodeLengthBuilder::DefluffExact,
        ])
        .collect();
    let mut scratch = TreeBuildScratch::default();
    for case in 0..20_000 {
        let mut frequencies = [0_u32; 19];
        for frequency in &mut frequencies {
            *frequency = match next() % 3 {
                0 => 0,
                _ => (next() % [2, 4, 9, 64, 1 << 12][case % 5]) as u32,
            };
        }
        for index in (1..builders.len()).rev() {
            builders.swap(index, (next() % (index as u64 + 1)) as usize);
        }
        let trees = CodeLengthTrees::new(&frequencies);
        for &builder in &builders {
            let built = builder.build(&frequencies, false, &mut scratch);
            assert_eq!(
                trees.get_with_huffman(builder, &mut scratch),
                (built.lengths, built.huffman),
                "{builder:?} {frequencies:?}"
            );
        }
    }
}
