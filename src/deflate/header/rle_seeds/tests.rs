// SPDX-License-Identifier: MIT
// SPDX-FileCopyrightText: 2026 Andrew C.E. Dent <https://github.com/ace-dent>

use super::*;
use crate::deflate::header::rle_frequencies;

#[test]
fn rle_round_trip_shape() {
    let lengths = [0, 0, 0, 0, 3, 3, 3, 3, 3, 0, 0, 0];
    let rle = greedy_rle(&lengths, false, false, false);
    assert!(rle.iter().any(|token| token.symbol == 16));
    assert!(rle.iter().any(|token| token.symbol == 17));
}

fn unpruned_rle_seed_candidates(lengths: &[u8], rle_mask: u8) -> Vec<Vec<RleToken>> {
    let mut candidates = Vec::new();
    for mask in 0..8 {
        if rle_mask & (1 << mask) == 0 {
            continue;
        }
        let no_16 = mask & 1 != 0;
        let no_17 = mask & 2 != 0;
        let no_18 = mask & 4 != 0;
        push_unique(&mut candidates, greedy_rle(lengths, no_16, no_17, no_18));
        if !no_16 {
            push_unique(&mut candidates, balanced_repeat_rle(lengths, no_17, no_18));
            push_unique(
                &mut candidates,
                columbo_zero_repeat_rle(lengths, no_17, no_18),
            );
        }
    }
    candidates
}

#[test]
fn rle_opportunity_gates_preserve_every_distinct_seed() {
    let masks = [0x01, 0x55, 0xaa, 0xff];
    for encoded in 0..4_u32.pow(6) {
        let mut encoded = encoded;
        let mut lengths = [0_u8; 6];
        for length in &mut lengths {
            *length = (encoded % 4) as u8;
            encoded /= 4;
        }
        for mask in masks {
            assert_eq!(
                rle_seed_candidates(&lengths, mask),
                unpruned_rle_seed_candidates(&lengths, mask),
            );
        }
    }

    for lengths in [
        vec![3; 8],
        vec![3; 9],
        vec![3; 14],
        vec![3; 15],
        vec![0; 11],
        vec![0; 138],
        vec![0; 139],
        vec![0; 141],
        [vec![2; 15], vec![0; 149], vec![4; 8]].concat(),
    ] {
        assert_eq!(
            rle_seed_candidates(&lengths, 0xff),
            unpruned_rle_seed_candidates(&lengths, 0xff),
        );
    }

    // Long zero runs leave remainders after code 18 that code 17 may or may
    // not need, and long nonzero runs code 16, so masks repeat greedy seeds.
    let mut state = 0x9b05_688c_u32;
    let mut next = move || {
        state ^= state << 13;
        state ^= state >> 17;
        state ^= state << 5;
        state
    };
    for case in 0..5_000 {
        let mut lengths = Vec::new();
        while lengths.len() < 20 + case % 300 {
            let (value, run) = if next() % 2 == 0 {
                (0, 1 + next() % [3, 14, 150, 300][case % 4])
            } else {
                (1 + (next() % 15) as u8, 1 + next() % [3, 6, 12][case % 3])
            };
            lengths.extend(std::iter::repeat(value).take(run as usize));
        }
        for mask in masks {
            assert_eq!(
                rle_seed_candidates(&lengths, mask),
                unpruned_rle_seed_candidates(&lengths, mask),
                "case {case}, mask {mask:#x}"
            );
        }
    }
}

#[test]
fn balanced_repeat_avoids_a_two_literal_tail() {
    let lengths = [5_u8; 9];
    let greedy = greedy_rle(&lengths, false, false, false);
    let balanced = balanced_repeat_rle(&lengths, false, false);

    assert_eq!(greedy.len(), 4);
    assert_eq!(balanced.len(), 3);
    assert_eq!(
        balanced[0],
        RleToken {
            symbol: 5,
            extra: 0
        }
    );
    assert_eq!(
        balanced[1],
        RleToken {
            symbol: 16,
            extra: 1
        }
    );
    assert_eq!(
        balanced[2],
        RleToken {
            symbol: 16,
            extra: 1
        }
    );
}

#[test]
fn columbo_zero_repeat_can_continue_a_literal_zero() {
    let lengths = [0_u8; 5];
    let ordinary = greedy_rle(&lengths, false, true, false);
    let source_shaped = columbo_zero_repeat_rle(&lengths, true, false);

    assert_eq!(
        ordinary,
        vec![
            RleToken {
                symbol: 0,
                extra: 0
            };
            5
        ]
    );
    assert_eq!(
        source_shaped,
        [
            RleToken {
                symbol: 0,
                extra: 0,
            },
            RleToken {
                symbol: 16,
                extra: 1,
            },
        ]
    );
}

/// The scanning spellers before they shared one run list.
fn reference_greedy_rle(lengths: &[u8], no_16: bool, no_17: bool, no_18: bool) -> Vec<RleToken> {
    // Each token spells at least one length.
    let mut output = Vec::with_capacity(lengths.len());
    let mut index = 0;
    while index < lengths.len() {
        let value = lengths[index];
        let mut run = 1;
        while index + run < lengths.len() && lengths[index + run] == value {
            run += 1;
        }
        index += run;

        if value == 0 {
            while !no_18 && run >= 11 {
                let count = run.min(138);
                output.push(RleToken {
                    symbol: 18,
                    extra: (count - 11) as u8,
                });
                run -= count;
            }
            while !no_17 && run >= 3 {
                let count = run.min(10);
                output.push(RleToken {
                    symbol: 17,
                    extra: (count - 3) as u8,
                });
                run -= count;
            }
        } else if !no_16 && run >= 4 {
            output.push(RleToken {
                symbol: value,
                extra: 0,
            });
            run -= 1;
            while run >= 3 {
                let count = run.min(6);
                output.push(RleToken {
                    symbol: 16,
                    extra: (count - 3) as u8,
                });
                run -= count;
            }
        }
        output.extend(
            std::iter::repeat(RleToken {
                symbol: value,
                extra: 0,
            })
            .take(run),
        );
    }
    output
}

fn reference_balanced_repeat_rle(lengths: &[u8], no_17: bool, no_18: bool) -> Vec<RleToken> {
    // Each token spells at least one length.
    let mut output = Vec::with_capacity(lengths.len());
    let mut index = 0;
    while index < lengths.len() {
        let value = lengths[index];
        let mut run = 1;
        while index + run < lengths.len() && lengths[index + run] == value {
            run += 1;
        }
        index += run;

        if value == 0 {
            while !no_18 && run >= 11 {
                let count = run.min(138);
                output.push(RleToken {
                    symbol: 18,
                    extra: (count - 11) as u8,
                });
                run -= count;
            }
            while !no_17 && run >= 3 {
                let count = run.min(10);
                output.push(RleToken {
                    symbol: 17,
                    extra: (count - 3) as u8,
                });
                run -= count;
            }
        } else if run >= 4 {
            output.push(RleToken {
                symbol: value,
                extra: 0,
            });
            run -= 1;
            while run >= 3 {
                let count = if matches!(run % 6, 1 | 2) && run >= 7 {
                    4
                } else {
                    run.min(6)
                };
                output.push(RleToken {
                    symbol: 16,
                    extra: (count - 3) as u8,
                });
                run -= count;
            }
        }
        output.extend(
            std::iter::repeat(RleToken {
                symbol: value,
                extra: 0,
            })
            .take(run),
        );
    }
    output
}

fn reference_columbo_zero_repeat_rle(lengths: &[u8], no_17: bool, no_18: bool) -> Vec<RleToken> {
    // Each token spells at least one length.
    let mut output = Vec::with_capacity(lengths.len());
    let mut index = 0;
    while index < lengths.len() {
        let value = lengths[index];
        let mut run = 1;
        while index + run < lengths.len() && lengths[index + run] == value {
            run += 1;
        }
        index += run;

        if value == 0 {
            while !no_18 && run >= 11 {
                let count = run.min(138);
                output.push(RleToken {
                    symbol: 18,
                    extra: (count - 11) as u8,
                });
                run -= count;
            }
            while !no_17 && run >= 3 {
                let count = run.min(10);
                output.push(RleToken {
                    symbol: 17,
                    extra: (count - 3) as u8,
                });
                run -= count;
            }
        }

        if run >= 4 {
            output.push(RleToken {
                symbol: value,
                extra: 0,
            });
            run -= 1;
            while run >= 3 {
                let count = if matches!(run % 6, 1 | 2) && run >= 7 {
                    4
                } else {
                    run.min(6)
                };
                output.push(RleToken {
                    symbol: 16,
                    extra: (count - 3) as u8,
                });
                run -= count;
            }
        }
        output.extend(
            std::iter::repeat(RleToken {
                symbol: value,
                extra: 0,
            })
            .take(run),
        );
    }
    output
}

#[test]
fn run_list_spellings_match_the_scanning_spellers() {
    let mut state = 0x3c6e_f372_u32;
    let mut next = move || {
        state ^= state << 13;
        state ^= state >> 17;
        state ^= state << 5;
        state
    };
    for case in 0..20_000 {
        // Long zero runs and repeated values, so every repeat code appears.
        let mut lengths = Vec::new();
        while lengths.len() < 320 {
            let value = if next() % 3 == 0 {
                0
            } else {
                (next() % 15 + 1) as u8
            };
            let run = [1, 2, 3, 4, 7, 8, 11, 13, 40, 139, 150][(next() % 11) as usize];
            lengths.extend(std::iter::repeat(value).take(run));
        }
        lengths.truncate((next() % 320) as usize);
        let runs = length_runs(&lengths);
        let count = lengths.len();
        // A recycled buffer still holds an earlier spelling.
        let stale = || {
            vec![
                RleToken {
                    symbol: 18,
                    extra: 9
                };
                3
            ]
        };
        for mask in 0..8_u8 {
            let (no_16, no_17, no_18) = (mask & 1 != 0, mask & 2 != 0, mask & 4 != 0);
            assert_eq!(
                {
                    let (rle, frequencies) =
                        greedy_rle_from_runs(&runs, count, no_16, no_17, no_18, stale());
                    assert_eq!(frequencies, rle_frequencies(&rle));
                    rle
                },
                reference_greedy_rle(&lengths, no_16, no_17, no_18),
                "case {case} mask {mask}"
            );
            assert_eq!(
                {
                    let (rle, frequencies) =
                        balanced_repeat_rle_from_runs(&runs, count, no_17, no_18, stale());
                    assert_eq!(frequencies, rle_frequencies(&rle));
                    rle
                },
                reference_balanced_repeat_rle(&lengths, no_17, no_18),
                "case {case} mask {mask}"
            );
            assert_eq!(
                {
                    let (rle, frequencies) =
                        columbo_zero_repeat_rle_from_runs(&runs, count, no_17, no_18, stale());
                    assert_eq!(frequencies, rle_frequencies(&rle));
                    rle
                },
                reference_columbo_zero_repeat_rle(&lengths, no_17, no_18),
                "case {case} mask {mask}"
            );
        }
    }
}
