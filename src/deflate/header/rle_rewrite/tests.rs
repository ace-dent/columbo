// SPDX-License-Identifier: MIT
// SPDX-FileCopyrightText: 2026 Andrew C.E. Dent <https://github.com/ace-dent>

use super::*;

#[test]
fn deflopt_local_rewrite_has_a_strictly_decreasing_repeat_rank() {
    let input = [
        RleToken {
            symbol: 0,
            extra: 0,
        },
        RleToken {
            symbol: 17,
            extra: 1,
        },
    ];
    let mut code_lengths = [0_u8; 19];
    code_lengths[0] = 3;
    code_lengths[16] = 2;
    code_lengths[17] = 2;

    let (rewritten, frequencies) =
        rewrite_rle_deflopt_local(&input, &rle_frequencies(&input), &code_lengths).unwrap();
    assert_eq!(rewritten[1].symbol, 16);
    assert_eq!(frequencies, rle_frequencies(&rewritten));
    assert!(repeat_rank(&frequencies) < repeat_rank(&rle_frequencies(&input)));
}

#[test]
fn deflopt_local_rewrite_expands_only_a_strictly_dearer_repeat() {
    let input = [RleToken {
        symbol: 18,
        extra: 0, // eleven zero lengths
    }];
    let mut code_lengths = [0_u8; 19];
    code_lengths[0] = 1;
    code_lengths[18] = 7;

    let (rewritten, frequencies) =
        rewrite_rle_deflopt_local(&input, &rle_frequencies(&input), &code_lengths).unwrap();
    assert_eq!(frequencies, rle_frequencies(&rewritten));
    assert_eq!(rewritten.len(), 11);
    assert!(rewritten.iter().all(|token| token.symbol == 0));

    // A tie keeps the repeat code, matching DeflOpt's strict-improvement
    // rule.
    code_lengths[0] = 1;
    code_lengths[18] = 4;
    assert!(rewrite_rle_deflopt_local(&input, &rle_frequencies(&input), &code_lengths).is_none());
}

#[test]
fn deft4j_prune_can_rebuild_after_an_equal_cost_rewrite() {
    let input = [RleToken {
        symbol: 17,
        extra: 0, // three zero lengths
    }];
    let mut code_lengths = [0_u8; 19];
    code_lengths[0] = 2;
    code_lengths[17] = 3;

    // Three explicit zero codes and one code 17 plus its three extra bits
    // both cost six. The strict final pass keeps the repeat; deft4j's
    // pre-rebuild prune expands the tie.
    let frequencies = rle_frequencies(&input);
    assert!(rewrite_rle_deft4j_literals(&input, &frequencies, &code_lengths, false).is_none());
    assert_eq!(
        rewrite_rle_deft4j_literals(&input, &frequencies, &code_lengths, true)
            .unwrap()
            .0,
        vec![
            RleToken {
                symbol: 0,
                extra: 0
            };
            3
        ]
    );
}

/// The local rewrite before it counted frequencies and allocated on the first
/// change.
fn rewrite_rle_deflopt_local_reference(
    input: &[RleToken],
    code_length_lengths: &[u8; 19],
) -> Option<Vec<RleToken>> {
    let mut output = Vec::new();
    let mut previous = None;
    let mut changed = false;
    for &token in input {
        match token.symbol {
            value @ 0..=15 => {
                output.push(token);
                previous = Some(value);
            }
            18 => {
                let count = usize::from(token.extra) + 11;
                if explicit_rle_cost(code_length_lengths, 0, count)
                    < rle_symbol_cost(code_length_lengths, 18)
                {
                    output.extend(
                        std::iter::repeat(RleToken {
                            symbol: 0,
                            extra: 0,
                        })
                        .take(count),
                    );
                    changed = true;
                } else {
                    output.push(token);
                }
                previous = Some(0);
            }
            17 => {
                let count = usize::from(token.extra) + 3;
                if previous == Some(0)
                    && token.extra <= 3
                    && code_length_lengths[16] != 0
                    && code_length_lengths[17] != 0
                    && code_length_lengths[16] <= code_length_lengths[17]
                {
                    output.push(RleToken {
                        symbol: 16,
                        extra: token.extra,
                    });
                    changed = true;
                } else if explicit_rle_cost(code_length_lengths, 0, count)
                    < rle_symbol_cost(code_length_lengths, 17)
                {
                    output.extend(
                        std::iter::repeat(RleToken {
                            symbol: 0,
                            extra: 0,
                        })
                        .take(count),
                    );
                    changed = true;
                } else {
                    output.push(token);
                }
                previous = Some(0);
            }
            16 => {
                let previous = previous?;
                let count = usize::from(token.extra) + 3;
                if explicit_rle_cost(code_length_lengths, previous, count)
                    < rle_symbol_cost(code_length_lengths, 16)
                {
                    output.extend(
                        std::iter::repeat(RleToken {
                            symbol: previous,
                            extra: 0,
                        })
                        .take(count),
                    );
                    changed = true;
                } else {
                    output.push(token);
                }
            }
            _ => return None,
        }
    }
    changed.then_some(output)
}

#[test]
fn deflopt_local_rewrite_matches_the_reference_and_counts_its_symbols() {
    let mut random = 0x2545_f491_4f6c_dd1d_u64;
    let mut next = move || {
        random ^= random << 13;
        random ^= random >> 7;
        random ^= random << 17;
        random
    };
    for case in 0..20_000 {
        let len = 1 + next() as usize % 40;
        let input: Vec<RleToken> = (0..len)
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
        let mut code_lengths = [0_u8; 19];
        for length in &mut code_lengths {
            *length = (next() % 8) as u8;
        }
        let expected = rewrite_rle_deflopt_local_reference(&input, &code_lengths);
        let actual = rewrite_rle_deflopt_local(&input, &rle_frequencies(&input), &code_lengths);
        assert_eq!(
            actual.as_ref().map(|(rle, _)| rle),
            expected.as_ref(),
            "case {case}"
        );
        if let Some((rle, frequencies)) = actual {
            assert_eq!(frequencies, rle_frequencies(&rle), "case {case}");
        }
    }
}

/// `rewrite_rle_deft4j_literals` before it took and counted frequencies.
fn rewrite_rle_deft4j_literals_reference(
    input: &[RleToken],
    code_length_lengths: &[u8; 19],
    include_equal: bool,
) -> Option<Vec<RleToken>> {
    let mut output = Vec::new();
    let mut previous = None;
    let mut changed = false;
    for &token in input {
        let (value, count) = match token.symbol {
            value @ 0..=15 => {
                output.push(token);
                previous = Some(value);
                continue;
            }
            16 => (previous?, usize::from(token.extra) + 3),
            17 => (0, usize::from(token.extra) + 3),
            18 => (0, usize::from(token.extra) + 11),
            _ => return None,
        };
        let explicit = explicit_rle_cost(code_length_lengths, value, count);
        let repeat = rle_symbol_cost(code_length_lengths, token.symbol);
        if explicit < repeat || (include_equal && explicit == repeat) {
            if output.len() + count > MAX_DYNAMIC_CODE_LENGTH_COUNT {
                return None;
            }
            output.extend(
                std::iter::repeat(RleToken {
                    symbol: value,
                    extra: 0,
                })
                .take(count),
            );
            changed = true;
        } else {
            output.push(token);
        }
        previous = Some(value);
    }
    changed.then_some(output)
}

#[test]
fn deft4j_rewrite_matches_the_reference_and_counts_its_symbols() {
    let mut random = 0x9e37_79b9_7f4a_7c15_u64;
    let mut next = move || {
        random ^= random << 13;
        random ^= random >> 7;
        random ^= random << 17;
        random
    };
    for case in 0..20_000 {
        let len = 1 + next() as usize % 40;
        let input: Vec<RleToken> = (0..len)
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
        let mut code_lengths = [0_u8; 19];
        for length in &mut code_lengths {
            *length = (next() % 8) as u8;
        }
        for include_equal in [false, true] {
            let expected =
                rewrite_rle_deft4j_literals_reference(&input, &code_lengths, include_equal);
            let actual = rewrite_rle_deft4j_literals(
                &input,
                &rle_frequencies(&input),
                &code_lengths,
                include_equal,
            );
            assert_eq!(
                actual.as_ref().map(|(rle, _)| rle),
                expected.as_ref(),
                "case {case}"
            );
            if let Some((rle, frequencies)) = actual {
                assert_eq!(frequencies, rle_frequencies(&rle), "case {case}");
            }
        }
    }
}
