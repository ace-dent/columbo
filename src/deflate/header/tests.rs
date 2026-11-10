// SPDX-License-Identifier: MIT
// SPDX-FileCopyrightText: 2026 Andrew C.E. Dent <https://github.com/ace-dent>

use super::plan_cache::test_support::stored_header_plan_count;
use super::test_support::{best_dynamic_plan, literal_span_test_block};
use super::*;
use crate::deflate::huffman::{make_lengths, make_lengths_into, Huffman};
use crate::deflate::model::count_frequencies;

#[test]
fn literal_span_padding_saves_a_bit_without_changing_payload_codes() {
    let block = literal_span_test_block();
    let parent = block.original_dynamic.as_ref().unwrap();
    let result = plan_literal_span(&block, true, &mut 1024, &mut SearchStop::never()).unwrap();
    assert_eq!(parent.hlit, 269);
    assert_eq!((result.hlit, result.bits), (277, 1293));
    assert_eq!(block.original.unwrap().len, 1294);
    assert_eq!(
        &result.literal_lengths[..parent.hlit],
        parent.literal_lengths
    );
    assert!(result.literal_lengths[parent.hlit..]
        .iter()
        .all(|&n| n == 0));
    assert_eq!(result.distance_lengths, parent.distance_lengths);
    assert!(result.has_strictly_compatible_huffman_codes());
    let data_bits = token_bits(
        &block.tokens,
        &parent.literal_lengths,
        &parent.distance_lengths,
    )
    .unwrap();
    assert_eq!(
        token_bits(
            &block.tokens,
            &result.literal_lengths,
            &result.distance_lengths
        ),
        Some(data_bits)
    );
    let control = plan_for_trimmed_lengths_uncached(
        &parent.literal_lengths,
        &parent.distance_lengths,
        data_bits,
        true,
        0xff,
    )
    .unwrap();
    assert_eq!(control.bits, 1294);
    // Longer zero runs alone are not enough: this saving also needs a
    // different code-length code, so a search with that code fixed misses it.
    let lengths: Vec<_> = result
        .literal_lengths
        .iter()
        .chain(&result.distance_lengths)
        .copied()
        .collect();
    if let Some(rle) = shortest_rle(&lengths, &parent.code_length_lengths) {
        let mut frozen = result.clone();
        frozen.code_length_lengths = parent.code_length_lengths;
        frozen.hclen = parent.hclen;
        frozen.rle = rle;
        assert!(dynamic_bits(data_bits, &frozen).unwrap() >= 1294);
    }
}

#[test]
fn token_bits_counts_huffman_codes_and_match_extras_together() {
    let tokens = [
        Token::Literal(b'A'),
        Token::Match {
            length: 12,
            distance: 5,
            length_symbol: 265,
            distance_symbol: 4,
            length_extra: 1,
            distance_extra: 0,
            length_extra_bits: 1,
            distance_extra_bits: 1,
        },
    ];
    let mut literal_lengths = [0_u8; 286];
    literal_lengths[usize::from(b'A')] = 8;
    literal_lengths[256] = 7;
    literal_lengths[265] = 7;
    let mut distance_lengths = [0_u8; 30];
    distance_lengths[4] = 5;

    // literal + length code + distance code + both extras + end code
    assert_eq!(
        token_bits(&tokens, &literal_lengths, &distance_lengths),
        Some(8 + 7 + 5 + 1 + 1 + 7)
    );
}

#[test]
fn frequency_header_score_matches_token_scan() {
    let rle = vec![
        RleToken {
            symbol: 0,
            extra: 0,
        },
        RleToken {
            symbol: 16,
            extra: 2,
        },
        RleToken {
            symbol: 17,
            extra: 4,
        },
        RleToken {
            symbol: 18,
            extra: 40,
        },
        RleToken {
            symbol: 0,
            extra: 0,
        },
    ];
    let mut code_length_lengths = [0_u8; 19];
    for symbol in [0, 16, 17, 18] {
        code_length_lengths[symbol] = 2;
    }
    let hclen = trim_code_lengths(&code_length_lengths);
    let plan = DynamicPlan {
        literal_lengths: Vec::new(),
        distance_lengths: Vec::new(),
        code_length_lengths,
        rle: rle.clone(),
        hlit: 0,
        hdist: 0,
        hclen,
        bits: 0,
    };

    assert_eq!(
        dynamic_bits_from_rle_frequencies(73, hclen, &rle_frequencies(&rle), &code_length_lengths,),
        dynamic_bits(73, &plan),
    );
}

#[test]
fn distance_symbol_one_is_not_trimmed_away() {
    let mut lengths = [0_u8; 30];
    lengths[1] = 1;
    assert_eq!(trim_distance(&lengths), 2);
}

#[test]
fn package_merge_can_win_the_code_length_alphabet() {
    // On this histogram Defluff's package-merge tree saves one bit over every
    // heap tree. No adjacent repeats, so the test isolates the 19-symbol tree
    // choice from RLE packing.
    let mut remaining = [134_u32, 2, 0, 1, 1, 6, 5, 39, 33, 21, 74];
    let mut lengths = Vec::new();
    lengths.try_reserve_exact(316).unwrap();
    while lengths.len() < 316 {
        let previous = lengths.last().copied();
        let symbol = remaining
            .iter()
            .enumerate()
            .filter(|&(symbol, &count)| count != 0 && Some(symbol as u8) != previous)
            .max_by_key(|&(symbol, &count)| (count, std::cmp::Reverse(symbol)))
            .map(|(symbol, _)| symbol)
            .unwrap();
        remaining[symbol] -= 1;
        lengths.push(symbol as u8);
    }
    assert!(remaining.iter().all(|&count| count == 0));
    assert!(lengths.windows(2).all(|pair| pair[0] != pair[1]));

    let rle: Vec<_> = lengths
        .iter()
        .copied()
        .map(|symbol| RleToken { symbol, extra: 0 })
        .collect();
    let mut search = HeaderPlanSearch::default();
    consider_rle(
        4_496,
        &lengths[..286],
        &lengths[286..],
        &lengths,
        (rle.clone(), rle_frequencies(&rle)),
        false,
        &mut search,
    );

    let best = search.best.unwrap();
    assert_eq!(
        best.code_length_lengths,
        [1, 6, 0, 7, 7, 6, 6, 4, 4, 4, 2, 0, 0, 0, 0, 0, 0, 0, 0]
    );
}

#[test]
fn generated_plans_reject_incomplete_multi_symbol_payload_trees() {
    let mut literal_lengths = vec![0_u8; 257];
    literal_lengths[0] = 15;
    literal_lengths[256] = 15;

    assert!(Huffman::build(&literal_lengths).is_some());
    assert!(plan_for_explicit_lengths(&[], &literal_lengths, &[1], false).is_none());
}

#[test]
fn strict_empty_dynamic_plan_uses_complete_huffman_codes() {
    // An end-of-block-only literal/length code and an empty distance code, as
    // in libdeflate issue #323. Strict output completes both codes.
    let mut literal_frequencies = [0_u32; 286];
    literal_frequencies[256] = 1;
    let distance_frequencies = [0_u32; 30];

    let strict = best_dynamic_plan(
        &[],
        &literal_frequencies,
        &distance_frequencies,
        None,
        true,
        false,
        &mut SearchStop::never(),
    )
    .unwrap();
    assert!(strict.has_strictly_compatible_huffman_codes());
    assert_eq!(
        strict
            .literal_lengths
            .iter()
            .filter(|&&length| length != 0)
            .count(),
        2
    );
    assert_eq!(
        strict
            .distance_lengths
            .iter()
            .take(30)
            .filter(|&&length| length != 0)
            .count(),
        2
    );

    let relaxed = best_dynamic_plan(
        &[],
        &literal_frequencies,
        &distance_frequencies,
        None,
        false,
        false,
        &mut SearchStop::never(),
    )
    .unwrap();
    assert!(!relaxed.has_strictly_compatible_huffman_codes());
}

/// `reorder_code_length_lengths` before its linear early exit.
fn reference_reorder_code_length_lengths(lengths: &mut [u8; 19], frequencies: &[u32; 19]) -> bool {
    let mut changed = false;
    for left in 0..18 {
        if lengths[left] == 0 {
            continue;
        }
        for right in (left + 1)..19 {
            if lengths[right] == 0 {
                continue;
            }
            let should_swap = if lengths[right] < lengths[left] {
                frequencies[right] < frequencies[left]
            } else if lengths[right] > lengths[left] {
                frequencies[right] > frequencies[left]
            } else {
                false
            };
            if should_swap {
                lengths.swap(left, right);
                changed = true;
            }
        }
    }
    changed
}

#[test]
fn code_length_reorder_early_exit_matches_the_full_pass() {
    let mut state = 0x2545_f491_4f6c_dd1d_u64;
    let mut next = move || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state
    };
    let mut early_exits = 0;
    let mut scratch = TreeBuildScratch::default();
    for case in 0..200_000 {
        let mut frequencies = [0_u32; 19];
        let mut lengths = [0_u8; 19];
        for frequency in frequencies.iter_mut() {
            if next() % 4 != 0 {
                *frequency = (next() % [3, 8, 60][case % 3]) as u32;
            }
        }
        if case % 2 == 0 {
            // Trees from the builders themselves, which mostly pass the check.
            make_lengths_into(
                &frequencies,
                &mut lengths,
                7,
                (case % 4) as u32,
                &mut scratch,
            );
        } else {
            for length in lengths.iter_mut() {
                *length = (next() % 8) as u8;
            }
        }
        let mut expected = lengths;
        let mut actual = lengths;
        let expected_changed = reference_reorder_code_length_lengths(&mut expected, &frequencies);
        let actual_changed = reorder_code_length_lengths(&mut actual, &frequencies);
        assert_eq!(
            (actual_changed, actual),
            (expected_changed, expected),
            "case {case}"
        );
        early_exits += usize::from(code_lengths_follow_frequencies(&lengths, &frequencies));
    }
    assert!(early_exits > 50_000);
}

#[test]
fn swap_searches_with_kept_trial_costs_match_cold_searches() {
    let mut state = 0xbb67_ae85_84ca_a73b_u64;
    let mut next = move || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state
    };
    // A literal-heavy block with many rare symbols, whose equal counts give
    // the swap search payload-neutral trials, and a few matches.
    let mut base = Vec::new();
    for symbol in 0..120_u8 {
        let count = 1 + next() % if symbol % 7 == 0 { 12 } else { 3 };
        base.extend((0..count).map(|_| Token::Literal(symbol.wrapping_mul(2))));
    }
    for (length_symbol, distance_symbol) in [(257, 0), (258, 3), (260, 4), (257, 4)] {
        base.push(Token::Match {
            length: 3,
            distance: 1,
            length_symbol,
            distance_symbol,
            length_extra: 0,
            distance_extra: 0,
            length_extra_bits: 0,
            distance_extra_bits: (distance_symbol / 2).saturating_sub(1),
        });
    }
    let (literal, distance) = count_frequencies(&base);
    let seed = DynamicPlan {
        literal_lengths: make_lengths(&literal, 15, 0),
        distance_lengths: make_lengths(&distance, 15, 0),
        code_length_lengths: [0; 19],
        rle: Vec::new(),
        hlit: 286,
        hdist: 30,
        hclen: 19,
        bits: 0,
    };
    // Variants keep the seed's used symbols and change their counts, as the
    // queue's children do.
    let variants: Vec<Vec<Token>> = (0..6)
        .map(|_| {
            let mut tokens = base.clone();
            for _ in 0..8 {
                let index = usize::try_from(next() % base.len() as u64).unwrap();
                tokens.push(base[index]);
            }
            tokens
        })
        .collect();
    let search = |tokens: &[Token]| {
        let (literal, distance) = count_frequencies(tokens);
        let mut best = None;
        improve_by_length_swaps(
            tokens,
            &literal,
            &distance,
            &seed,
            true,
            &mut SearchStop::never(),
            &mut best,
        );
        best
    };

    release_thread_header_plans();
    let mut warm = Vec::new();
    for tokens in &variants {
        // The second search finds every trial kept.
        let first = search(tokens);
        assert_eq!(search(tokens), first);
        warm.push(first);
    }
    // The trials' costs are kept, but not their plans: the store holds only
    // each pass's start and committed plans.
    let kept_trials = swap_costs::test_support::kept_swap_trial_count();
    assert!(kept_trials > 0);
    assert!(stored_header_plan_count() < kept_trials / 4);
    for (tokens, warm) in variants.iter().zip(warm) {
        release_thread_header_plans();
        assert_eq!(search(tokens), warm);
    }
    release_thread_header_plans();
    assert_eq!(swap_costs::test_support::kept_swap_trial_count(), 0);
}

#[test]
fn word_run_finding_matches_a_scan() {
    let mut state = 0x243f_6a88_85a3_08d3_u64;
    let mut next = move || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state
    };
    // A stale buffer, as the planner's is.
    let mut runs = vec![(9, 9)];
    for _ in 0..20_000 {
        let count = (next() % 320) as usize;
        // Few values give long runs, sixteen give short ones.
        let values = [1, 2, 6, 16][(next() % 4) as usize];
        let lengths: Vec<u8> = (0..count).map(|_| (next() % values) as u8).collect();
        let mut expected: Vec<LengthRun> = Vec::new();
        for &length in &lengths {
            match expected.last_mut() {
                Some((value, run)) if *value == length => *run += 1,
                _ => expected.push((length, 1)),
            }
        }
        length_runs_into(&lengths, &mut runs);
        assert_eq!(runs, expected, "{lengths:?}");
    }
}

#[test]
fn counted_rle_cost_matches_the_token_walk() {
    let mut state = 0x5be0_cd19_137e_2179_u64;
    let mut next = move || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state
    };
    for _ in 0..20_000 {
        let rle: Vec<RleToken> = (0..(next() % 64) as usize)
            .map(|_| {
                let symbol = (next() % 19) as u8;
                let extra = match symbol {
                    16 => next() % 4,
                    17 => next() % 8,
                    18 => next() % 128,
                    _ => 0,
                } as u8;
                RleToken { symbol, extra }
            })
            .collect();
        // Zero lengths, used or not, test the unusable-code case.
        let mut lengths = [0_u8; 19];
        for length in lengths.iter_mut() {
            *length = (next() % 9) as u8;
        }
        assert_eq!(
            rle_frequency_cost(&rle_frequencies(&rle), &lengths),
            rle_cost(&rle, &lengths)
        );
    }
}

/// The payload sum before it ran without branches: checked additions, and
/// `None` at the first used symbol without a code.
fn reference_alphabet_payload_bits(frequencies: &[u32], lengths: &[u8]) -> Option<u64> {
    let mut bits = 0_u64;
    for (symbol, &frequency) in frequencies.iter().enumerate() {
        if frequency == 0 {
            continue;
        }
        let length = *lengths.get(symbol)?;
        if length == 0 {
            return None;
        }
        bits = bits.checked_add(u64::from(frequency) * u64::from(length))?;
    }
    Some(bits)
}

#[test]
fn branch_free_payload_bits_match_checked_sum() {
    let mut state = 0x5d1c_7e43u32;
    let mut next = move || {
        state ^= state << 13;
        state ^= state >> 17;
        state ^= state << 5;
        state
    };
    let mut uncoded = 0;
    for case in 0..20_000 {
        let size = if case % 2 == 0 { 286 } else { 30 };
        let frequencies: Vec<u32> = (0..size)
            .map(|_| match next() % 4 {
                0 => 0,
                1 => u32::MAX - next() % 4,
                _ => next() % 1000,
            })
            .collect();
        // A few lengths are 0 or missing, so some used symbols have no code.
        let missing = if next() % 8 == 0 { 1 + next() % 2 } else { 0 };
        let lengths: Vec<u8> = (0..size - missing as usize)
            .map(|_| {
                if next() % 4000 == 0 {
                    0
                } else {
                    (next() % 15 + 1) as u8
                }
            })
            .collect();
        let expected = reference_alphabet_payload_bits(&frequencies, &lengths);
        uncoded += usize::from(expected.is_none());
        assert_eq!(
            alphabet_payload_bits(&frequencies, &lengths),
            expected,
            "case {case}"
        );
    }
    assert!(uncoded > 1_000 && uncoded < 10_000);
}
