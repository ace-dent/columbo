// SPDX-License-Identifier: MIT
// SPDX-FileCopyrightText: 2026 Andrew C.E. Dent <https://github.com/ace-dent>

//! The last tree steps of Max's block planner, which adjust the best plan's
//! code lengths: equal-frequency rearrangement, which keeps the payload cost,
//! and greedy length swaps, which do not raise it.

use super::plan_cache::{header_plan_fingerprint, stored_header_plan_kernel_keeping};
use super::swap_costs::{keep_swap_trial, kept_swap_trial, swap_trial_start};
use super::{
    keep_better, length_swap_delta, plan_for_explicit_lengths_masked, plan_for_trimmed_lengths,
    token_bits, trim_distance, trim_literal,
};
use crate::deflate::model::{DynamicPlan, Token};
use crate::deflate::stop::SearchStop;
use crate::sort::SortShared;

/// Sort one tree's code lengths within each group of equal-frequency symbols,
/// which preserves the Kraft sum and payload cost. Ascending and descending
/// order give two cheap header candidates instead of a permutation search.
pub(super) fn arrange_equal_frequency_lengths<const N: usize>(
    frequencies: &[u32; N],
    lengths: &mut [u8; N],
    descending: bool,
) {
    let mut visited = [false; N];
    for first in 0..N {
        if visited[first] || frequencies[first] == 0 || lengths[first] == 0 {
            continue;
        }

        let frequency = frequencies[first];
        let mut positions = Vec::new();
        let mut assigned_lengths = Vec::new();
        for symbol in first..N {
            if !visited[symbol] && frequencies[symbol] == frequency && lengths[symbol] != 0 {
                visited[symbol] = true;
                positions.push(symbol);
                assigned_lengths.push(lengths[symbol]);
            }
        }
        if positions.len() < 2 {
            continue;
        }

        assigned_lengths.sort_shared_by(Ord::cmp);
        if descending {
            assigned_lengths.reverse();
        }
        for (symbol, length) in positions.into_iter().zip(assigned_lengths) {
            lengths[symbol] = length;
        }
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) fn improve_by_length_swaps(
    tokens: &[Token],
    literal_frequencies: &[u32; 286],
    distance_frequencies: &[u32; 30],
    seed: &DynamicPlan,
    exhaustive: bool,
    stop: &mut SearchStop<'_>,
    best: &mut Option<DynamicPlan>,
) {
    let mut literal_lengths = vec![0_u8; 286];
    let mut distance_lengths = vec![0_u8; 30];
    if seed.literal_lengths.len() > literal_lengths.len()
        || seed.distance_lengths.len() > distance_lengths.len()
    {
        return;
    }
    literal_lengths[..seed.literal_lengths.len()].copy_from_slice(&seed.literal_lengths);
    distance_lengths[..seed.distance_lengths.len()].copy_from_slice(&seed.distance_lengths);

    // Choose each swap with only the greedy RLE spelling (mask 0x01), then
    // price the committed lengths with every spelling (mask 0xff).
    let Some(mut current) = plan_for_explicit_lengths_masked(
        tokens,
        &literal_lengths,
        &distance_lengths,
        exhaustive,
        0x01,
    ) else {
        return;
    };
    keep_better(best, current.clone());

    improve_one_tree_by_swaps(
        tokens,
        literal_frequencies,
        &mut literal_lengths,
        &mut distance_lengths,
        true,
        exhaustive,
        stop,
        &mut current,
        best,
    );
    improve_one_tree_by_swaps(
        tokens,
        distance_frequencies,
        &mut literal_lengths,
        &mut distance_lengths,
        false,
        exhaustive,
        stop,
        &mut current,
        best,
    );
}

#[allow(clippy::too_many_arguments)]
fn improve_one_tree_by_swaps(
    tokens: &[Token],
    frequencies: &[u32],
    literal_lengths: &mut [u8],
    distance_lengths: &mut [u8],
    literal_tree: bool,
    exhaustive: bool,
    stop: &mut SearchStop<'_>,
    current: &mut DynamicPlan,
    best: &mut Option<DynamicPlan>,
) {
    const MAX_PASSES: usize = 12;

    for _ in 0..MAX_PASSES {
        if stop.reached() {
            break;
        }
        // Swaps keep HLIT, HDIST and the tokens, so price the payload once per
        // pass and adjust it by `length_swap_delta` per trial.
        let hlit = trim_literal(literal_lengths);
        let hdist = trim_distance(distance_lengths);
        let Some(payload_bits) =
            token_bits(tokens, &literal_lengths[..hlit], &distance_lengths[..hdist])
        else {
            break;
        };
        // A trial's header depends only on these lengths and its pair, so a
        // pass from the same lengths, under other counts, reuses its cost.
        let start = swap_trial_start(
            header_plan_fingerprint(
                &literal_lengths[..hlit],
                &distance_lengths[..hdist],
                exhaustive,
                0x01,
            ),
            &literal_lengths[..hlit],
            &distance_lengths[..hdist],
            exhaustive,
        );

        let mut selected_pair = None;
        let mut selected_payload_bits = payload_bits;
        let mut selected_bits = current.bits;
        // `None` when the selected trial's cost was kept: its plan is made
        // again once the pass ends.
        let mut selected_plan = None;
        for a in 0..frequencies.len() {
            if stop.reached() {
                return;
            }
            let length_a = if literal_tree {
                literal_lengths[a]
            } else {
                distance_lengths[a]
            };
            if length_a == 0 {
                continue;
            }

            for b in (a + 1)..frequencies.len() {
                let length_b = if literal_tree {
                    literal_lengths[b]
                } else {
                    distance_lengths[b]
                };
                if length_b == 0 || length_a == length_b {
                    continue;
                }
                let delta = length_swap_delta(frequencies[a], frequencies[b], length_a, length_b);
                // Keep this zero-cost guard: relaxing it would change
                // intermediate winners and so redirect later searches.
                if delta > 0 {
                    continue;
                }
                let Some(data_bits) = payload_bits.checked_add_signed(delta) else {
                    continue;
                };

                if literal_tree {
                    literal_lengths.swap(a, b);
                } else {
                    distance_lengths.swap(a, b);
                }
                debug_assert_eq!(
                    Some(data_bits),
                    token_bits(tokens, &literal_lengths[..hlit], &distance_lengths[..hdist])
                );
                let kept = start.and_then(|start| kept_swap_trial(start, literal_tree, a, b));
                let (header_bits, plan) = match kept {
                    Some(header_bits) => (header_bits, None),
                    None => {
                        // A trial whose cost is kept here is not stored: trials
                        // far outnumber other plans and would push them out
                        // of the thread's store. A swap keeps each alphabet's
                        // lengths, so the trees keep the start's valid shapes.
                        let plan = stored_header_plan_kernel_keeping(
                            header_plan_fingerprint(
                                &literal_lengths[..hlit],
                                &distance_lengths[..hdist],
                                exhaustive,
                                0x01,
                            ),
                            &literal_lengths[..hlit],
                            &distance_lengths[..hdist],
                            exhaustive,
                            0x01,
                            start.is_none(),
                            true,
                        );
                        let header_bits = plan.as_ref().map(|plan| plan.bits);
                        if let Some(start) = start {
                            keep_swap_trial(start, literal_tree, a, b, header_bits);
                        }
                        (header_bits, plan)
                    }
                };
                if literal_tree {
                    literal_lengths.swap(a, b);
                } else {
                    distance_lengths.swap(a, b);
                }

                let Some(candidate_bits) =
                    header_bits.and_then(|header_bits| header_bits.checked_add(data_bits))
                else {
                    continue;
                };
                if candidate_bits < selected_bits {
                    selected_pair = Some((a, b));
                    selected_payload_bits = data_bits;
                    selected_bits = candidate_bits;
                    selected_plan = plan.map(|mut plan| {
                        plan.bits = candidate_bits;
                        plan
                    });
                }
            }
        }

        let Some((a, b)) = selected_pair else {
            break;
        };
        if literal_tree {
            literal_lengths.swap(a, b);
        } else {
            distance_lengths.swap(a, b);
        }
        let selected_plan = selected_plan.or_else(|| {
            plan_for_trimmed_lengths(
                &literal_lengths[..hlit],
                &distance_lengths[..hdist],
                selected_payload_bits,
                exhaustive,
                0x01,
            )
        });
        let Some(selected_plan) = selected_plan else {
            if literal_tree {
                literal_lengths.swap(a, b);
            } else {
                distance_lengths.swap(a, b);
            }
            break;
        };
        debug_assert_eq!(selected_plan.bits, selected_bits);
        *current = selected_plan;
        keep_better(best, current.clone());

        if let Some(full_plan) = plan_for_trimmed_lengths(
            &literal_lengths[..hlit],
            &distance_lengths[..hdist],
            selected_payload_bits,
            exhaustive,
            0xff,
        ) {
            keep_better(best, full_plan);
        }
    }
}

#[cfg(test)]
mod tests;
