// SPDX-License-Identifier: MIT
// SPDX-FileCopyrightText: 2026 Andrew C.E. Dent <https://github.com/ace-dent>

use super::*;
use crate::deflate::header::rle_cost;
use crate::deflate::model::MAX_DYNAMIC_CODE_LENGTH_COUNT;

fn enumerate_rle_paths(
    lengths: &[u8],
    index: usize,
    current: &mut Vec<RleToken>,
    output: &mut Vec<Vec<RleToken>>,
) {
    if index == lengths.len() {
        output.push(current.clone());
        return;
    }
    let mut run = 1;
    while index + run < lengths.len() && lengths[index + run] == lengths[index] {
        run += 1;
    }
    let mut visit = |count: usize, symbol: u8, extra: u8| {
        current.push(RleToken { symbol, extra });
        enumerate_rle_paths(lengths, index + count, current, output);
        current.pop();
    };

    visit(1, lengths[index], 0);
    if index > 0 && lengths[index] == lengths[index - 1] {
        for count in 3..=run.min(6) {
            visit(count, 16, (count - 3) as u8);
        }
    }
    if lengths[index] == 0 {
        for count in 3..=run.min(10) {
            visit(count, 17, (count - 3) as u8);
        }
        for count in 11..=run.min(138) {
            visit(count, 18, (count - 11) as u8);
        }
    }
}

fn exhaustive_shortest_rle(lengths: &[u8], costs: &[u8; 19]) -> Option<Vec<RleToken>> {
    let mut spellings = Vec::new();
    enumerate_rle_paths(lengths, 0, &mut Vec::new(), &mut spellings);
    spellings.retain(|rle| {
        rle.iter()
            .all(|token| costs[usize::from(token.symbol)] != 0)
    });
    spellings.sort_by_key(|rle| rle_cost(rle, costs));
    spellings.into_iter().next()
}

#[test]
fn shortest_rle_matches_exhaustive_short_sequence_oracle() {
    let mut cost_sets = Vec::new();
    let mut balanced = [3_u8; 19];
    balanced[16] = 2;
    balanced[17] = 3;
    balanced[18] = 4;
    cost_sets.push(balanced);
    let mut literal_friendly = [5_u8; 19];
    literal_friendly[0] = 1;
    literal_friendly[3] = 1;
    literal_friendly[16] = 7;
    literal_friendly[17] = 7;
    literal_friendly[18] = 7;
    cost_sets.push(literal_friendly);
    let mut repeat_friendly = [6_u8; 19];
    repeat_friendly[0] = 4;
    repeat_friendly[3] = 4;
    repeat_friendly[16] = 1;
    repeat_friendly[17] = 1;
    repeat_friendly[18] = 1;
    cost_sets.push(repeat_friendly);

    for length in 1..=8 {
        for mask in 0..1_usize << length {
            let sequence: Vec<_> = (0..length)
                .map(|bit| if mask & (1 << bit) == 0 { 0 } else { 3 })
                .collect();
            for costs in &cost_sets {
                assert_eq!(
                    shortest_rle(&sequence, costs),
                    exhaustive_shortest_rle(&sequence, costs),
                    "sequence {sequence:?}, costs {costs:?}",
                );
            }
        }
    }

    let long_zero_run = [0_u8; 12];
    for costs in &cost_sets {
        assert_eq!(
            shortest_rle(&long_zero_run, costs),
            exhaustive_shortest_rle(&long_zero_run, costs),
        );
    }
}

/// Slow oracle: scan every legal edge and keep whole suffix spellings. It
/// shares no sliding-window state with `shortest_rle`.
fn scanned_shortest_rle(lengths: &[u8], costs: &[u8; 19]) -> Option<Vec<RleToken>> {
    let mut suffixes = vec![None; lengths.len() + 1];
    suffixes[lengths.len()] = Some((0_u64, Vec::<RleToken>::new()));
    for start in (0..lengths.len()).rev() {
        let run = lengths[start..]
            .iter()
            .take_while(|&&value| value == lengths[start])
            .count();
        let mut edges = vec![(1, lengths[start], 0)];
        for (symbol, minimum, maximum, allowed) in [
            (16, 3, 6, start > 0 && lengths[start - 1] == lengths[start]),
            (17, 3, 10, lengths[start] == 0),
            (18, 11, 138, lengths[start] == 0),
        ] {
            if allowed {
                edges.extend(
                    (minimum..=run.min(maximum))
                        .map(|count| (count, symbol, (count - minimum) as u8)),
                );
            }
        }
        for (count, symbol, extra) in edges {
            if costs[usize::from(symbol)] == 0 {
                continue;
            }
            let Some((suffix_cost, suffix)) = &suffixes[start + count] else {
                continue;
            };
            let cost = suffix_cost + u64::from(costs[usize::from(symbol)]) + rle_extra_bits(symbol);
            if suffixes[start]
                .as_ref()
                .map_or(true, |(best, _)| cost < *best)
            {
                let mut path = vec![RleToken { symbol, extra }];
                path.extend_from_slice(suffix);
                suffixes[start] = Some((cost, path));
            }
        }
    }
    suffixes[0].take().map(|(_, path)| path)
}

#[test]
fn shortest_rle_preserves_long_run_windows_missing_codes_and_ties() {
    let mut state = 0x736f_6d65_7073_6575_u64;
    for n in [0, 3, 6, 10, 11, 12, 137, 138, 139, 276, 277, 318] {
        let mut separated = vec![0; n / 2];
        separated.extend([3; 7]);
        separated.resize(n, 0);
        for sequence in [vec![0; n], vec![3; n], separated] {
            for case in 0..24 {
                let mut costs = [1; 19];
                if case != 0 {
                    for cost in &mut costs {
                        state ^= state << 13;
                        state ^= state >> 7;
                        state ^= state << 17;
                        *cost = (state % 8) as u8;
                    }
                }
                assert_eq!(
                    shortest_rle(&sequence, &costs),
                    scanned_shortest_rle(&sequence, &costs),
                    "sequence {sequence:?}, costs {costs:?}",
                );
            }
        }
    }
}

/// `shortest_rle` before it used fixed buffers and counted frequencies.
fn shortest_rle_reference(lengths: &[u8], costs: &[u8; 19]) -> Option<Vec<RleToken>> {
    #[derive(Clone, Copy)]
    struct Step {
        next: usize,
        token: RleToken,
    }

    let mut best = vec![INF; lengths.len() + 1];
    let mut step = vec![None; lengths.len()];
    let mut run = 0;
    let mut zero_repeats = std::collections::VecDeque::<usize>::new();
    best[lengths.len()] = 0;
    for index in (0..lengths.len()).rev() {
        if index + 1 < lengths.len() && lengths[index] == lengths[index + 1] {
            run += 1;
        } else {
            run = 1;
            zero_repeats.clear();
        }

        // Code 18 costs the same for every repeat from 11 to 138, so a
        // monotonic deque keeps the cheapest suffix of that sliding window.
        // Newer ends are smaller, so dropping equal costs keeps the preference
        // for the shortest repeat.
        let zero_repeat = if lengths[index] == 0 && costs[18] != 0 && run >= 11 {
            let first_end = index + 11;
            let last_end = index + run.min(138);
            while zero_repeats.front().is_some_and(|&end| end > last_end) {
                zero_repeats.pop_front();
            }
            while zero_repeats
                .back()
                .is_some_and(|&end| best[end] >= best[first_end])
            {
                zero_repeats.pop_back();
            }
            zero_repeats.push_back(first_end);
            zero_repeats.front().map(|&end| end - index)
        } else {
            None
        };

        let mut consider = |count: usize, symbol: u8, extra: u8| {
            let code = costs[usize::from(symbol)];
            if code == 0 || index + count > lengths.len() {
                return;
            }
            let cost = u64::from(code)
                .saturating_add(rle_extra_bits(symbol))
                .saturating_add(best[index + count]);
            // Replace only on a strict improvement, so a tie keeps the option
            // considered first: an explicit length, then codes 16, 17 and 18.
            if cost < best[index] {
                best[index] = cost;
                step[index] = Some(Step {
                    next: index + count,
                    token: RleToken { symbol, extra },
                });
            }
        };

        consider(1, lengths[index], 0);
        if index > 0 && lengths[index] == lengths[index - 1] {
            for count in 3..=run.min(6) {
                consider(count, 16, (count - 3) as u8);
            }
        }
        if lengths[index] == 0 {
            for count in 3..=run.min(10) {
                consider(count, 17, (count - 3) as u8);
            }
            if let Some(count) = zero_repeat {
                consider(count, 18, (count - 11) as u8);
            }
        }
    }
    if best[0] == INF {
        return None;
    }
    let mut output = Vec::new();
    let mut index = 0;
    while index < lengths.len() {
        let selected = step[index]?;
        output.push(selected.token);
        index = selected.next;
    }
    Some(output)
}

#[test]
fn shortest_rle_matches_the_reference_and_counts_its_symbols() {
    // One scratch for every case, as a header search reuses it.
    let mut scratch = ShortestRleScratch::default();
    let mut random = 0x9e37_79b9_7f4a_7c15_u64;
    let mut next = move || {
        random ^= random << 13;
        random ^= random >> 7;
        random ^= random << 17;
        random
    };
    for case in 0..20_000 {
        let len = 1 + next() as usize % MAX_DYNAMIC_CODE_LENGTH_COUNT;
        // Long runs of zeros and repeats exercise codes 16, 17 and 18.
        let mut lengths = Vec::with_capacity(len);
        while lengths.len() < len {
            let value = if next() % 3 == 0 {
                0
            } else {
                (next() % 16) as u8
            };
            let run = 1 + next() as usize % 150;
            lengths.extend(std::iter::repeat(value).take(run.min(len - lengths.len())));
        }
        let mut costs = [0_u8; 19];
        for cost in &mut costs {
            *cost = (next() % 8) as u8;
        }
        let expected = shortest_rle_reference(&lengths, &costs);
        let actual = shortest_rle_with_frequencies(&lengths, &costs, &mut scratch);
        assert_eq!(
            actual.as_ref().map(|(rle, _)| rle),
            expected.as_ref(),
            "case {case}"
        );
        assert_eq!(shortest_rle(&lengths, &costs), expected, "case {case}");
        if let Some((rle, frequencies)) = actual {
            assert_eq!(frequencies, rle_frequencies(&rle), "case {case}");
        }
    }
}

#[test]
fn kept_run_spellings_match_fresh_searches() {
    // One scratch kept across searches, as a header-plan thread keeps it, so
    // spellings are reused, grown, rebuilt for another key in their slot and
    // dropped at the entry limit. None of that may change a result.
    let mut state = 0x9b05_688c_2b3e_6c1f_u64;
    let mut next = move || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state
    };
    let mut kept = ShortestRleScratch::default();
    let mut dropped = false;
    for case in 0..6_000 {
        let len = 1 + (next() % 318) as usize;
        let mut lengths = Vec::with_capacity(len);
        while lengths.len() < len {
            let value = match next() % 2 {
                0 => 0,
                _ => (next() % 16) as u8,
            };
            let run = 1 + (next() % [4, 40, 320][case % 3]) as usize;
            lengths.extend(std::iter::repeat(value).take(run.min(len - lengths.len())));
        }
        // Costs above 7 share a slot with the key of their low three bits.
        let most = if case % 5 == 0 { 16 } else { 8 };
        let costs: [u8; 19] = std::array::from_fn(|_| (next() % most) as u8);
        let tables = kept.spellings.len();
        let result = shortest_rle_with_frequencies(&lengths, &costs, &mut kept);
        dropped |= kept.spellings.len() < tables;
        assert_eq!(
            result,
            shortest_rle_with_frequencies(&lengths, &costs, &mut ShortestRleScratch::default()),
            "lengths {lengths:?}, costs {costs:?}",
        );
        if let Some((rle, frequencies)) = &result {
            assert_eq!(*frequencies, rle_frequencies(rle));
        }
        if case % 40 == 0 {
            assert_eq!(
                result.map(|(rle, _)| rle),
                scanned_shortest_rle(&lengths, &costs),
                "lengths {lengths:?}, costs {costs:?}",
            );
        }
    }
    assert!(dropped, "the searches never reached the entry limit");
}
