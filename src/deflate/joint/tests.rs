// SPDX-License-Identifier: MIT

use super::super::header::shortest_rle;
use super::super::huffman::make_lengths;
use super::*;

#[test]
fn payload_suffix_matches_exhaustive_assignments_at_every_capacity() {
    fn enumerate(
        frequencies: &[u32],
        reserved: usize,
        allowed: &[(usize, usize)],
        zero: bool,
        remaining: usize,
    ) -> u64 {
        let Some((&frequency, rest)) = frequencies.split_first() else {
            return if remaining == 0 { 0 } else { INF };
        };
        let next_reserved = reserved.saturating_sub(1);
        let mut best = if zero && frequency == 0 {
            enumerate(rest, next_reserved, allowed, zero, remaining)
        } else {
            INF
        };
        if reserved > 0 {
            for &(length, units) in allowed {
                if units <= remaining {
                    best = best.min(
                        u64::from(frequency) * length as u64
                            + enumerate(rest, next_reserved, allowed, zero, remaining - units),
                    );
                }
            }
        }
        best
    }

    let frequencies = [0, 1, u32::MAX, 0, 2];
    let capacity = 8;
    for mask in 0..8 {
        let allowed: Vec<_> = (1..=3)
            .filter(|&length| mask & (1 << (length - 1)) != 0)
            .map(|length| (length, 1 << (3 - length)))
            .collect();
        for reserved in 0..=frequencies.len() {
            for zero in [false, true] {
                let actual = payload_suffix(
                    &frequencies,
                    reserved,
                    capacity,
                    &allowed,
                    zero,
                    &mut SearchStop::never(),
                )
                .unwrap();
                for i in 0..=frequencies.len() {
                    for remaining in 0..=capacity {
                        let expected = enumerate(
                            &frequencies[i..],
                            reserved.saturating_sub(i),
                            &allowed,
                            zero,
                            remaining,
                        );
                        assert_eq!(
                            actual[i * (capacity + 1) + remaining],
                            expected,
                            "mask {mask}, reserved {reserved}, zero {zero}, suffix {i}, capacity {remaining}"
                        );
                    }
                }
            }
        }
    }
}

fn complete_vectors(frequencies: &[u32], reserved: usize, depth: usize) -> Vec<Vec<u8>> {
    fn visit(
        frequencies: &[u32],
        reserved: usize,
        depth: usize,
        capacity: usize,
        lengths: &mut Vec<u8>,
        output: &mut Vec<Vec<u8>>,
    ) {
        let i = lengths.len();
        if i == frequencies.len() {
            if capacity == 0 {
                output.push(lengths.clone());
            }
            return;
        }
        for length in 0..=depth {
            if (length == 0 && frequencies[i] > 0) || (length > 0 && i >= reserved) {
                continue;
            }
            let units = if length == 0 {
                0
            } else {
                1 << (depth - length)
            };
            if units <= capacity {
                lengths.push(length as u8);
                visit(
                    frequencies,
                    reserved,
                    depth,
                    capacity - units,
                    lengths,
                    output,
                );
                lengths.pop();
            }
        }
    }
    let mut output = Vec::new();
    visit(
        frequencies,
        reserved,
        depth,
        1 << depth,
        &mut Vec::new(),
        &mut output,
    );
    output
}

fn spelling_bits(rle: &[RleToken], costs: &[u8; 19]) -> u64 {
    rle.iter()
        .map(|t| {
            u64::from(costs[usize::from(t.symbol)])
                + match t.symbol {
                    16 => 2,
                    17 => 3,
                    18 => 7,
                    _ => 0,
                }
        })
        .sum()
}

/// Independent enumeration of every complete length vector in both
/// alphabets, followed by the existing fixed-sequence shortest-RLE solver.
/// No joint states, suffix bounds, or seam transitions are reused here.
fn oracle(problem: &Problem<'_>) -> Option<u64> {
    let literals = complete_vectors(
        &problem.frequencies[..problem.split],
        problem.split,
        problem.max_depth,
    );
    let distances = complete_vectors(
        &problem.frequencies[problem.split..],
        problem.reserved_from - problem.split,
        problem.max_depth,
    );
    let mut best = None;
    for literal in &literals {
        for distance in &distances {
            let lengths: Vec<_> = literal.iter().chain(distance).copied().collect();
            if let Some(rle) = shortest_rle(&lengths, problem.costs) {
                let bits = spelling_bits(&rle, problem.costs)
                    + lengths
                        .iter()
                        .zip(problem.frequencies)
                        .map(|(&l, &f)| u64::from(l) * u64::from(f))
                        .sum::<u64>();
                best = Some(best.map_or(bits, |old: u64| old.min(bits)));
            }
        }
    }
    best
}

fn verify_solution(problem: &Problem<'_>, solution: &Solution) {
    let mut decoded = Vec::new();
    for token in &solution.rle {
        assert_ne!(problem.costs[usize::from(token.symbol)], 0);
        let (length, count) = match token.symbol {
            0..=15 => (token.symbol, 1),
            16 => {
                assert!(token.extra <= 3);
                (*decoded.last().unwrap(), usize::from(token.extra) + 3)
            }
            17 => {
                assert!(token.extra <= 7);
                (0, usize::from(token.extra) + 3)
            }
            18 => {
                assert!(token.extra <= 127);
                (0, usize::from(token.extra) + 11)
            }
            _ => panic!("invalid repeat"),
        };
        decoded.extend(std::iter::repeat(length).take(count));
    }
    assert_eq!(decoded, solution.lengths);
    assert_eq!(decoded.len(), problem.frequencies.len());
    for alphabet in [&decoded[..problem.split], &decoded[problem.split..]] {
        let units: usize = alphabet
            .iter()
            .filter(|&&l| l != 0)
            .map(|&l| 1 << (problem.max_depth - usize::from(l)))
            .sum();
        assert_eq!(units, 1 << problem.max_depth);
    }
    for (i, (&l, &f)) in decoded.iter().zip(problem.frequencies).enumerate() {
        assert!(f == 0 || l != 0);
        assert!(i < problem.reserved_from || l == 0);
    }
    assert_eq!(
        solution.bits,
        spelling_bits(&solution.rle, problem.costs)
            + decoded
                .iter()
                .zip(problem.frequencies)
                .map(|(&l, &f)| u64::from(l) * u64::from(f))
                .sum::<u64>()
    );
}

#[test]
fn joint_search_matches_exhaustive_tree_and_rle_oracle() {
    let mut seed = 0x459ecaffu32;
    for case in 0..96 {
        let mut next = || {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            seed
        };
        let mut frequencies = [0u32; 7];
        for f in &mut frequencies {
            *f = next() % 5;
        }
        let reserved_from = if case % 3 == 0 { 6 } else { 7 };
        frequencies[reserved_from..].fill(0);
        let mut cl_frequencies = [0u32; 19];
        for i in [0, 1, 2, 3, 16, 17, 18] {
            cl_frequencies[i] = next() % 4;
        }
        let costs: [u8; 19] = make_lengths(&cl_frequencies, 7, 0).try_into().unwrap();
        let problem = Problem {
            frequencies: &frequencies,
            split: 4,
            reserved_from,
            costs: &costs,
            max_depth: 3,
        };
        let expected = oracle(&problem);
        let result = solve(
            &problem,
            INF,
            &mut JointBudget::new(),
            &mut SearchStop::never(),
        );
        assert_eq!(
            result.as_ref().map(|s| s.bits),
            expected,
            "case {case}: {frequencies:?} / {costs:?}"
        );
        if let Some(result) = result {
            verify_solution(&problem, &result);
            assert!(solve(
                &problem,
                result.bits,
                &mut JointBudget::new(),
                &mut SearchStop::never()
            )
            .is_none());
            assert_eq!(
                solve(
                    &problem,
                    result.bits + 1,
                    &mut JointBudget::new(),
                    &mut SearchStop::never()
                )
                .unwrap()
                .bits,
                result.bits
            );
        }
    }
}

#[test]
fn repeat_instructions_cross_separate_kraft_capacities() {
    for zero_repeat in [false, true] {
        let mut costs = [0; 19];
        let mut frequencies = vec![1; if zero_repeat { 16 } else { 8 }];
        if zero_repeat {
            frequencies[2..14].fill(0);
            costs[1] = 1;
            costs[18] = 1;
        } else {
            costs[2] = 1;
            costs[16] = 1;
        }
        let problem = Problem {
            frequencies: &frequencies,
            split: frequencies.len() / 2,
            reserved_from: frequencies.len(),
            costs: &costs,
            max_depth: 2,
        };
        let result = solve(
            &problem,
            INF,
            &mut JointBudget::new(),
            &mut SearchStop::never(),
        )
        .unwrap();
        assert_eq!(result.bits, if zero_repeat { 16 } else { 21 });
        assert_eq!(Some(result.bits), oracle(&problem));
        verify_solution(&problem, &result);
        let mut position = 0;
        assert!(result.rle.iter().any(|token| {
            let start = position;
            position += match token.symbol {
                16 | 17 => usize::from(token.extra) + 3,
                18 => usize::from(token.extra) + 11,
                _ => 1,
            };
            start < problem.split && position > problem.split
        }));
    }
}

#[test]
fn cutoff_keeps_only_complete_improvements_and_budget_is_shared() {
    let frequencies = [9, 0, 2, 1, 3, 0, 1, 0];
    let mut costs = [0; 19];
    for i in [0, 1, 2, 3, 16, 17, 18] {
        costs[i] = 3;
    }
    let problem = Problem {
        frequencies: &frequencies,
        split: 4,
        reserved_from: 8,
        costs: &costs,
        max_depth: 3,
    };
    let best = oracle(&problem).unwrap();
    let mut full_budget = JointBudget::new();
    solve(&problem, INF, &mut full_budget, &mut SearchStop::never()).unwrap();
    let work = (1 << 27) - full_budget.work_left;
    let mut completed_cutoff = false;
    for limit in 0..=work {
        let mut budget = JointBudget { work_left: limit };
        if let Some(result) = solve(&problem, INF, &mut budget, &mut SearchStop::never()) {
            verify_solution(&problem, &result);
            assert!(result.bits >= best);
            completed_cutoff |= budget.work_left == 0 && limit < work;
        }
        assert!(budget.work_left <= limit);
        if budget.work_left == 0 {
            assert!(solve(&problem, INF, &mut budget, &mut SearchStop::never()).is_none());
        }
    }
    assert!(completed_cutoff);
    assert!(solve(
        &problem,
        INF,
        &mut JointBudget::new(),
        &mut SearchStop::always()
    )
    .is_none());
    for cutoff in 1..40 {
        let mut calls = 0;
        let mut should_stop = || {
            calls += 1;
            calls >= cutoff
        };
        if let Some(result) = solve(
            &problem,
            INF,
            &mut JointBudget::new(),
            &mut SearchStop::callback(&mut should_stop),
        ) {
            verify_solution(&problem, &result);
            assert!(result.bits >= best);
        }
    }
}
