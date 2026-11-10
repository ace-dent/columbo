// SPDX-License-Identifier: MIT
// SPDX-FileCopyrightText: 2026 Andrew C.E. Dent <https://github.com/ace-dent>

//! RLE seeds for the header search: a greedy spelling for each mask that
//! disables some of codes 16, 17 and 18, and, while code 16 is allowed, the
//! balanced and zero-repeat spellings. Seeds come from the code lengths' runs,
//! which also show which masks and spellings could not differ from seeds
//! already made, so those are skipped.

use super::{length_runs, push_unique, CountedRle, LengthRun};
use crate::deflate::model::RleToken;

#[derive(Default)]
struct RleSeedOpportunities {
    balanced_repeat: bool,
    zero_repeat: [bool; 4],
    zero_17: bool,
    zero_18: bool,
    /// The greedy spelling uses code 16: some nonzero run has at least 4.
    greedy_16: bool,
    /// By whether code 18 is disabled: the greedy spelling uses code 17, as
    /// some zero run keeps at least 3 after its code-18 repeats.
    greedy_17: [bool; 2],
}

fn rle_seed_opportunities(runs: &[LengthRun]) -> RleSeedOpportunities {
    let mut opportunities = RleSeedOpportunities::default();
    for &(value, run) in runs {
        if value != 0 {
            let repeated = run.saturating_sub(1);
            opportunities.balanced_repeat |= repeated >= 7 && matches!(repeated % 6, 1 | 2);
            opportunities.greedy_16 |= run >= 4;
            continue;
        }

        opportunities.zero_17 |= run >= 3;
        opportunities.zero_18 |= run >= 11;
        for no_17 in [false, true] {
            for no_18 in [false, true] {
                let mut remaining = run;
                while !no_18 && remaining >= 11 {
                    remaining -= remaining.min(138);
                }
                opportunities.greedy_17[usize::from(no_18)] |= remaining >= 3;
                while !no_17 && remaining >= 3 {
                    remaining -= remaining.min(10);
                }
                let variant = usize::from(no_17) | (usize::from(no_18) << 1);
                opportunities.zero_repeat[variant] |= remaining >= 4;
            }
        }
    }
    opportunities
}

/// [`rle_seed_candidates_from_runs_into`] for `decoded_lengths`, returning
/// the seeds without their symbol frequencies.
#[cfg(test)]
fn rle_seed_candidates(decoded_lengths: &[u8], rle_mask: u8) -> Vec<Vec<RleToken>> {
    let mut candidates = Vec::new();
    rle_seed_candidates_from_runs_into(
        &length_runs(decoded_lengths),
        decoded_lengths.len(),
        rle_mask,
        &mut Vec::new(),
        &mut candidates,
    );
    candidates.into_iter().map(|(rle, _)| rle).collect()
}

/// Collect each distinct RLE seed once, in first-seen order, for code lengths
/// given as their `runs`, `count` lengths in all, into `candidates`, which
/// must be empty. Bit `m` of `rle_mask` enables mask `m`; mask bits 0, 1 and 2
/// disable codes 16, 17 and 18.
pub(super) fn rle_seed_candidates_from_runs_into(
    runs: &[LengthRun],
    count: usize,
    rle_mask: u8,
    spare: &mut Vec<Vec<RleToken>>,
    candidates: &mut Vec<CountedRle>,
) {
    debug_assert!(candidates.is_empty());
    let opportunities = rle_seed_opportunities(runs);
    // Always keep bit 0: it also decides whether the balanced and zero-repeat
    // spellings run, even when disabling code 16 leaves the greedy spelling
    // unchanged.
    let relevant_mask =
        1 | (u8::from(opportunities.zero_17) << 1) | (u8::from(opportunities.zero_18) << 2);
    let mut seen_masks = [false; 8];
    // The greedy spellings already made, by the codes they actually leave out:
    // disabling a code the greedy spelling would not use repeats one.
    let mut seen_greedy = [false; 8];
    for mask in 0..8 {
        if rle_mask & (1 << mask) == 0 {
            continue;
        }
        let mask = mask & relevant_mask;
        if seen_masks[usize::from(mask)] {
            continue;
        }
        seen_masks[usize::from(mask)] = true;
        let no_16 = mask & 1 != 0;
        let no_17 = mask & 2 != 0;
        let no_18 = mask & 4 != 0;
        let greedy = usize::from(no_16 && opportunities.greedy_16)
            | usize::from(no_17 && opportunities.greedy_17[usize::from(no_18)]) << 1
            | usize::from(no_18) << 2;
        if !std::mem::replace(&mut seen_greedy[greedy], true) {
            let rle = greedy_rle_from_runs(
                runs,
                count,
                no_16,
                no_17,
                no_18,
                spare.pop().unwrap_or_default(),
            );
            push_unique(candidates, rle);
        }

        if !no_16 && opportunities.balanced_repeat {
            let balanced = balanced_repeat_rle_from_runs(
                runs,
                count,
                no_17,
                no_18,
                spare.pop().unwrap_or_default(),
            );
            push_unique(candidates, balanced);
        }

        if !no_16 {
            let zero_variant = usize::from(no_17) | (usize::from(no_18) << 1);
            if opportunities.zero_repeat[zero_variant] {
                let zero_repeat = columbo_zero_repeat_rle_from_runs(
                    runs,
                    count,
                    no_17,
                    no_18,
                    spare.pop().unwrap_or_default(),
                );
                push_unique(candidates, zero_repeat);
            }
        }
    }
}

pub(super) fn greedy_rle(lengths: &[u8], no_16: bool, no_17: bool, no_18: bool) -> Vec<RleToken> {
    greedy_rle_from_runs(
        &length_runs(lengths),
        lengths.len(),
        no_16,
        no_17,
        no_18,
        Vec::new(),
    )
    .0
}

fn greedy_rle_from_runs(
    runs: &[LengthRun],
    length_count: usize,
    no_16: bool,
    no_17: bool,
    no_18: bool,
    mut output: Vec<RleToken>,
) -> CountedRle {
    // Each token spells at least one length, so `length_count` slots hold the
    // spelling, and two more let a short run write three: write in place and
    // cut off the rest.
    output.clear();
    output.resize(
        length_count + 2,
        RleToken {
            symbol: 0,
            extra: 0,
        },
    );
    let tokens = output.as_mut_slice();
    let mut next = 0;
    // Counted as spelled: each run's explicit lengths, and the repeat codes.
    let mut frequencies = [0_u32; 19];
    let mut repeats = [0_u32; 3];
    for &(value, mut run) in runs {
        // No repeat code spells fewer than 3 zeros or 4 copies of a length,
        // so a shorter run is explicit lengths: write three without branching.
        if run < 3 + usize::from(value != 0) {
            tokens[next..next + 3].fill(RleToken {
                symbol: value,
                extra: 0,
            });
            next += run;
            frequencies[usize::from(value)] += run as u32;
            continue;
        }
        if value == 0 {
            while !no_18 && run >= 11 {
                let count = run.min(138);
                tokens[next] = RleToken {
                    symbol: 18,
                    extra: (count - 11) as u8,
                };
                next += 1;
                repeats[2] += 1;
                run -= count;
            }
            while !no_17 && run >= 3 {
                let count = run.min(10);
                tokens[next] = RleToken {
                    symbol: 17,
                    extra: (count - 3) as u8,
                };
                next += 1;
                repeats[1] += 1;
                run -= count;
            }
        } else if !no_16 && run >= 4 {
            tokens[next] = RleToken {
                symbol: value,
                extra: 0,
            };
            next += 1;
            frequencies[usize::from(value)] += 1;
            run -= 1;
            while run >= 3 {
                let count = run.min(6);
                tokens[next] = RleToken {
                    symbol: 16,
                    extra: (count - 3) as u8,
                };
                next += 1;
                repeats[0] += 1;
                run -= count;
            }
        }
        tokens[next..next + run].fill(RleToken {
            symbol: value,
            extra: 0,
        });
        next += run;
        frequencies[usize::from(value)] += run as u32;
    }
    output.truncate(next);
    frequencies[16..].copy_from_slice(&repeats);
    (output, frequencies)
}

#[cfg(test)]
fn balanced_repeat_rle(lengths: &[u8], no_17: bool, no_18: bool) -> Vec<RleToken> {
    balanced_repeat_rle_from_runs(
        &length_runs(lengths),
        lengths.len(),
        no_17,
        no_18,
        Vec::new(),
    )
    .0
}

/// Generalize deft4j's balanced code-16 splits (4+3 and 4+4 for seven and
/// eight repeats) to every run where greedy six-repeat codes would leave one or
/// two explicit values.
fn balanced_repeat_rle_from_runs(
    runs: &[LengthRun],
    length_count: usize,
    no_17: bool,
    no_18: bool,
    mut output: Vec<RleToken>,
) -> CountedRle {
    // Each token spells at least one length.
    output.clear();
    output.reserve(length_count);
    // Counted as spelled: each run's explicit lengths, and the repeat codes.
    let mut frequencies = [0_u32; 19];
    let mut repeats = [0_u32; 3];
    for &(value, mut run) in runs {
        if value == 0 {
            while !no_18 && run >= 11 {
                let count = run.min(138);
                output.push(RleToken {
                    symbol: 18,
                    extra: (count - 11) as u8,
                });
                repeats[2] += 1;
                run -= count;
            }
            while !no_17 && run >= 3 {
                let count = run.min(10);
                output.push(RleToken {
                    symbol: 17,
                    extra: (count - 3) as u8,
                });
                repeats[1] += 1;
                run -= count;
            }
        } else if run >= 4 {
            output.push(RleToken {
                symbol: value,
                extra: 0,
            });
            frequencies[usize::from(value)] += 1;
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
                repeats[0] += 1;
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
        frequencies[usize::from(value)] += run as u32;
    }
    frequencies[16..].copy_from_slice(&repeats);
    (output, frequencies)
}

#[cfg(test)]
fn columbo_zero_repeat_rle(lengths: &[u8], no_17: bool, no_18: bool) -> Vec<RleToken> {
    columbo_zero_repeat_rle_from_runs(
        &length_runs(lengths),
        lengths.len(),
        no_17,
        no_18,
        Vec::new(),
    )
    .0
}

/// Like `balanced_repeat_rle_from_runs`, but four or more zeros left after the
/// enabled codes 18 and 17 are spelled as one explicit zero followed by code
/// 16, as deft4j allows.
fn columbo_zero_repeat_rle_from_runs(
    runs: &[LengthRun],
    length_count: usize,
    no_17: bool,
    no_18: bool,
    mut output: Vec<RleToken>,
) -> CountedRle {
    // Each token spells at least one length.
    output.clear();
    output.reserve(length_count);
    // Counted as spelled: each run's explicit lengths, and the repeat codes.
    let mut frequencies = [0_u32; 19];
    let mut repeats = [0_u32; 3];
    for &(value, mut run) in runs {
        if value == 0 {
            while !no_18 && run >= 11 {
                let count = run.min(138);
                output.push(RleToken {
                    symbol: 18,
                    extra: (count - 11) as u8,
                });
                repeats[2] += 1;
                run -= count;
            }
            while !no_17 && run >= 3 {
                let count = run.min(10);
                output.push(RleToken {
                    symbol: 17,
                    extra: (count - 3) as u8,
                });
                repeats[1] += 1;
                run -= count;
            }
        }

        if run >= 4 {
            output.push(RleToken {
                symbol: value,
                extra: 0,
            });
            frequencies[usize::from(value)] += 1;
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
                repeats[0] += 1;
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
        frequencies[usize::from(value)] += run as u32;
    }
    frequencies[16..].copy_from_slice(&repeats);
    (output, frequencies)
}

#[cfg(test)]
mod tests;
