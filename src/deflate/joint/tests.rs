// SPDX-License-Identifier: MIT
// SPDX-FileCopyrightText: 2026 Andrew C.E. Dent <https://github.com/ace-dent>

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

/// Reference solver: enumerate every complete length vector for both
/// alphabets, then run the shortest-RLE solver on each. It shares no states,
/// suffix bounds or alphabet-boundary handling with `solve`.
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

/// The solver before its edges were specialized and its zero runs merged:
/// one closure per edge, charging the budget one edge at a time.
fn reference_solve(
    problem: &Problem<'_>,
    mut target: u64,
    budget: &mut JointBudget,
    stop: &mut SearchStop<'_>,
) -> Option<Solution> {
    let Problem {
        frequencies,
        split,
        reserved_from,
        costs: cl,
        max_depth,
    } = *problem;
    let n = frequencies.len();
    if stop.reached()
        || n > MAX_DYNAMIC_CODE_LENGTH_COUNT
        || split == 0
        || split >= n
        || reserved_from < split
        || reserved_from > n
        || max_depth > MAX_DEPTH
        || target > INF
    {
        return None;
    }
    // Each nonzero length needs its own code-length symbol (16 only repeats
    // one), so no length can exceed the deepest coded one.
    let depth = (1..=max_depth).rev().find(|&length| cl[length] > 0)?;
    let capacity = 1usize << depth;
    let previous_count = depth + 2;
    let none = depth + 1;
    let stride = (capacity + 1) * previous_count;
    let cells = (n + 1) * stride;
    let allowed: Vec<_> = (1..=depth)
        .filter(|&length| cl[length] > 0)
        .map(|length| (length, 1 << (depth - length)))
        .collect();
    // Charge an upper bound for preparation, allocation and dense scans
    // before allocating, so blocks with no solution also use budget.
    let preparation = 3 * cells
        + (n + 2) * (capacity + 1) * (allowed.len() + 3)
        + n * (depth + 1 + 138 + previous_count * 4);
    if !budget.spend(preparation as u64) {
        return None;
    }
    let zero = cl[0] > 0 || cl[17] > 0 || cl[18] > 0;
    let literal_bound =
        payload_suffix(&frequencies[..split], split, capacity, &allowed, zero, stop)?;
    let distance_bound = payload_suffix(
        &frequencies[split..],
        reserved_from - split,
        capacity,
        &allowed,
        zero,
        stop,
    )?;
    let distance_minimum = distance_bound[capacity];
    let payload_bound = |i: usize, remaining: usize| -> u64 {
        if i < split {
            (literal_bound[i * (capacity + 1) + remaining] + distance_minimum).min(INF)
        } else {
            distance_bound[(i - split) * (capacity + 1) + remaining]
        }
    };
    let mut sums = [0u64; MAX_DYNAMIC_CODE_LENGTH_COUNT + 1];
    let mut used = [0usize; MAX_DYNAMIC_CODE_LENGTH_COUNT + 1];
    let mut zero_run = [0usize; MAX_DYNAMIC_CODE_LENGTH_COUNT + 1];
    for i in 0..n {
        sums[i + 1] = sums[i] + u64::from(frequencies[i]);
        used[i + 1] = used[i] + usize::from(frequencies[i] > 0);
    }
    for i in (0..n).rev() {
        if frequencies[i] == 0 {
            zero_run[i] = zero_run[i + 1] + 1;
        }
    }
    let legal = |i: usize, count: usize, length: usize| -> bool {
        i + count <= n
            && if length == 0 {
                used[i + count] == used[i]
            } else {
                i + count <= reserved_from
            }
    };
    // The payload and header suffix minima are computed independently and may
    // assume different lengths; their sum is still a safe lower bound.
    let mut header = filled((n + 1) * previous_count, INF)?;
    header[n * previous_count..].fill(0);
    for i in (0..n).rev() {
        if stop.reached() {
            return None;
        }
        let mut common = INF;
        for length in 0..=depth {
            if cl[length] > 0 && legal(i, 1, length) {
                common =
                    common.min(u64::from(cl[length]) + header[(i + 1) * previous_count + length]);
            }
        }
        for (symbol, minimum, maximum, extra) in [(17, 3, 10, 3), (18, 11, 138, 7)] {
            if cl[symbol] > 0 {
                for count in minimum..=zero_run[i].min(maximum) {
                    common = common
                        .min(u64::from(cl[symbol]) + extra + header[(i + count) * previous_count]);
                }
            }
        }
        for previous in 0..previous_count {
            let mut best = common;
            if previous != none && cl[16] > 0 {
                for count in 3..=6 {
                    if legal(i, count, previous) {
                        best = best.min(
                            u64::from(cl[16]) + 2 + header[(i + count) * previous_count + previous],
                        );
                    }
                }
            }
            header[i * previous_count + previous] = best;
        }
    }
    if payload_bound(0, capacity) + header[none] >= target {
        return None;
    }
    // At depth 9 the two tables plus suffix bounds take less than 24 MiB,
    // held for one block at a time.
    let mut cost = filled(cells, INF)?;
    let mut trace = filled(cells, u32::MAX)?;
    let start = capacity * previous_count + none;
    cost[start] = 0;
    let mut winner = None;
    'search: for i in 0..n {
        for remaining in 0..=capacity {
            if remaining % 32 == 0 && (budget.work_left == 0 || stop.reached()) {
                break 'search;
            }
            for previous in 0..previous_count {
                let from = i * stride + remaining * previous_count + previous;
                let old = cost[from];
                if old == INF
                    || old + payload_bound(i, remaining) + header[i * previous_count + previous]
                        >= target
                {
                    continue;
                }
                let mut edge = |count: usize, length: usize, symbol: usize, kind: u32| {
                    if !budget.spend(1) || !legal(i, count, length) {
                        return;
                    }
                    let j = i + count;
                    let units = if length == 0 {
                        0
                    } else {
                        1 << (depth - length)
                    };
                    let next = if i < split && j >= split {
                        if remaining != (split - i) * units || (j - split) * units > capacity {
                            return;
                        }
                        capacity - (j - split) * units
                    } else {
                        if count * units > remaining {
                            return;
                        }
                        remaining - count * units
                    };
                    let extra = match symbol {
                        16 => 2,
                        17 => 3,
                        18 => 7,
                        _ => 0,
                    };
                    let score =
                        old + u64::from(cl[symbol]) + extra + (sums[j] - sums[i]) * length as u64;
                    if score + payload_bound(j, next) + header[j * previous_count + length]
                        >= target
                    {
                        return;
                    }
                    let to = j * stride + next * previous_count + length;
                    if score < cost[to] {
                        cost[to] = score;
                        // Position always advances, so a predecessor's trace
                        // is final before it is read. There are fewer than
                        // 2^21 cells, so `from << 2` fits in a u32.
                        trace[to] = ((from as u32) << 2) | kind;
                        if j == n {
                            debug_assert_eq!(next, 0);
                            target = score;
                            winner = Some(to);
                        }
                    }
                };
                for (length, &cl_bits) in cl.iter().enumerate().take(depth + 1) {
                    if cl_bits > 0 {
                        edge(1, length, length, 0);
                    }
                }
                if previous != none && cl[16] > 0 {
                    for count in 3..=6 {
                        edge(count, previous, 16, 1);
                    }
                }
                if cl[17] > 0 {
                    for count in 3..=zero_run[i].min(10) {
                        edge(count, 0, 17, 2);
                    }
                }
                if cl[18] > 0 {
                    for count in 11..=zero_run[i].min(138) {
                        edge(count, 0, 18, 3);
                    }
                }
            }
        }
    }
    let mut at = winner?;
    let bits = cost[at];
    let mut rle = Vec::new();
    rle.try_reserve_exact(n).ok()?;
    while at != start {
        let link = trace[at];
        debug_assert_ne!(link, u32::MAX);
        let from = (link >> 2) as usize;
        let count = at / stride - from / stride;
        let length = at % previous_count;
        rle.push(match link & 3 {
            0 => {
                debug_assert_eq!(count, 1);
                RleToken {
                    symbol: length as u8,
                    extra: 0,
                }
            }
            1 => RleToken {
                symbol: 16,
                extra: (count - 3) as u8,
            },
            2 => RleToken {
                symbol: 17,
                extra: (count - 3) as u8,
            },
            _ => RleToken {
                symbol: 18,
                extra: (count - 11) as u8,
            },
        });
        at = from;
    }
    rle.reverse();
    let mut lengths = Vec::new();
    lengths.try_reserve_exact(n).ok()?;
    for token in &rle {
        match token.symbol {
            0..=15 => lengths.push(token.symbol),
            16 => {
                let value = *lengths.last()?;
                lengths.resize(lengths.len() + usize::from(token.extra) + 3, value);
            }
            17 => lengths.resize(lengths.len() + usize::from(token.extra) + 3, 0),
            18 => lengths.resize(lengths.len() + usize::from(token.extra) + 11, 0),
            _ => unreachable!(),
        }
    }
    debug_assert_eq!(lengths.len(), n);
    Some(Solution { lengths, rle, bits })
}

#[test]
fn specialized_search_matches_per_edge_reference() {
    let mut seed = 0x2f6b_93a1u32;
    let mut next = move || {
        seed ^= seed << 13;
        seed ^= seed >> 17;
        seed ^= seed << 5;
        seed
    };
    let mut cut_off = 0;
    for case in 0..2_000 {
        let large = case % 200 == 0;
        let n = if large {
            200 + next() as usize % 117
        } else {
            4 + next() as usize % 120
        };
        let split = 1 + next() as usize % (n - 1);
        let reserved_from = split + next() as usize % (n - split + 1);
        let max_depth = if large { 9 } else { 2 + next() as usize % 6 };
        // Long zero runs, some reaching the end, exercise 18 repeats near
        // the target-lowering final positions.
        let mut frequencies = vec![0u32; n];
        let mut i = 0;
        while i < n {
            let run = match next() % 4 {
                0 => 1 + next() as usize % 150,
                1 => 1 + next() as usize % 12,
                _ => 0,
            };
            if run > 0 {
                i += run;
                continue;
            }
            frequencies[i] = match next() % 8 {
                0 => next() % 100_000,
                _ => 1 + next() % 9,
            };
            i += 1;
        }
        frequencies[reserved_from..].fill(0);
        let mut cl_frequencies = [0u32; 19];
        for frequency in &mut cl_frequencies {
            if next() % 3 != 0 {
                *frequency = next() % 20;
            }
        }
        cl_frequencies[1 + next() as usize % max_depth] += 1;
        let costs: [u8; 19] = make_lengths(&cl_frequencies, 7, 0).try_into().unwrap();
        let problem = Problem {
            frequencies: &frequencies,
            split,
            reserved_from,
            costs: &costs,
            max_depth,
        };
        let mut full = JointBudget::new();
        let unlimited = reference_solve(&problem, INF, &mut full, &mut SearchStop::never());
        let work = (1 << 27) - full.work_left;
        let target = match (case % 3, &unlimited) {
            (1, Some(solution)) => solution.bits + u64::from(next() % 3),
            _ => INF,
        };
        let limit = if case % 4 == 3 {
            u64::from(next()) % (work + 1)
        } else {
            1 << 27
        };
        let stop_after = (case % 5 == 4).then(|| 1 + next() % 12);
        let run = |solver: fn(
            &Problem<'_>,
            u64,
            &mut JointBudget,
            &mut SearchStop<'_>,
        ) -> Option<Solution>| {
            let mut budget = JointBudget { work_left: limit };
            let mut calls = 0;
            let mut should_stop = || {
                calls += 1;
                stop_after.is_some_and(|after| calls >= after)
            };
            let solution = solver(
                &problem,
                target,
                &mut budget,
                &mut SearchStop::callback(&mut should_stop),
            );
            (
                solution.map(|s| (s.lengths, s.rle, s.bits)),
                budget.work_left,
                calls,
            )
        };
        let expected = run(reference_solve);
        cut_off += usize::from(expected.1 == 0);
        assert_eq!(run(solve), expected, "case {case}");
    }
    assert!(cut_off > 50);
}
