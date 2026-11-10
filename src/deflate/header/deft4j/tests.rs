// SPDX-License-Identifier: MIT
// SPDX-FileCopyrightText: 2026 Andrew C.E. Dent <https://github.com/ace-dent>

use super::*;

#[test]
fn deft4j_zero_runs_use_maximal_repeat_chunks() {
    let chunks = |tokens: &[RleToken], symbol: u8, base: usize| -> Vec<usize> {
        tokens
            .iter()
            .filter(|token| token.symbol == symbol)
            .map(|token| usize::from(token.extra) + base)
            .collect()
    };
    for run in 0..=316_usize {
        for (no_long, no_short) in [(false, false), (true, false), (false, true), (true, true)] {
            // Literal remainders keep the repeat-16 path out of this check.
            let options = Deft4jPackOptions {
                special_repeat: false,
                use_eight: false,
                use_seven: false,
                no_repeat: true,
                no_zero_repeat: no_short,
                no_long_zero_repeat: no_long,
                no_repeat_zeros: false,
            };
            let Some(tokens) = deft4j_pack_code_lengths(&vec![0; run], options) else {
                continue;
            };
            let long = chunks(&tokens, 18, 11);
            let short = chunks(&tokens, 17, 3);
            let literal = tokens.iter().filter(|token| token.symbol == 0).count();
            assert_eq!(
                long.iter().sum::<usize>() + short.iter().sum::<usize>() + literal,
                run
            );
            if let Some((last, full)) = long.split_last() {
                assert!(full.iter().all(|&count| count == 138));
                assert!((11..=138).contains(last));
            }
            if let Some((last, full)) = short.split_last() {
                assert!(full.iter().all(|&count| count == 10));
                assert!((3..=10).contains(last));
            }
            let after_long = run - long.iter().sum::<usize>();
            assert!(no_long || after_long < 11, "{run} {options:?}");
            assert!(no_short || literal < 3, "{run} {options:?}");
        }
    }
}
