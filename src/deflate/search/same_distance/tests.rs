// SPDX-License-Identifier: MIT
// SPDX-FileCopyrightText: 2026 Andrew C.E. Dent <https://github.com/ace-dent>

use super::*;
use crate::deflate::model::{canonical_length_encoding, SourceBlockType};
use crate::deflate::search::test_support::{short_family_test_block, test_match};

fn test_repack(
    tokens: &[Token],
    decoded_bytes: usize,
    literal_lengths: &[u8],
    exhaustive: bool,
) -> Option<Vec<Token>> {
    repack_same_distance_runs(
        tokens,
        decoded_bytes,
        literal_lengths,
        exhaustive,
        &mut SearchStop::never(),
    )
}

fn test_partitioner(
    literal_lengths: &[u8],
    max_active: usize,
    max_deficit: usize,
) -> Option<SameDistancePartitioner> {
    SameDistancePartitioner::new(
        literal_lengths,
        max_active,
        max_deficit,
        &mut SearchStop::never(),
    )
}

/// Direct recurrence retained as an oracle for family range minima.
fn reference_partition_choices(
    literal_lengths: &[u8],
    max_active: usize,
    max_deficit: usize,
    stop: &mut SearchStop<'_>,
) -> Option<Vec<u16>> {
    let width = max_deficit.checked_add(1)?;
    let choice_count = max_active.checked_add(1)?.checked_mul(width)?;
    let mut choices = Vec::new();
    choices.try_reserve_exact(choice_count).ok()?;
    choices.resize(choice_count, u16::MAX);

    let mut cost_by_deficit = [0_u32; 256];
    for (deficit, cost) in cost_by_deficit.iter_mut().enumerate() {
        let length = 258_u16.checked_sub(deficit as u16)?;
        let (symbol, _, extra_bits) = canonical_length_encoding(length)?;
        *cost = estimated_length(literal_lengths, usize::from(symbol))
            .checked_add(u64::from(extra_bits))?
            .try_into()
            .ok()?;
    }

    let mut previous = [u32::MAX; 258];
    let mut current = [u32::MAX; 258];
    previous[0] = 0;
    for slot in 1..=max_active {
        if stop.reached() {
            return None;
        }
        current[..width].fill(u32::MAX);
        for used_deficit in 0..=max_deficit {
            if used_deficit & 31 == 0 && stop.reached() {
                return None;
            }
            let mut best_cost = u32::MAX;
            let mut best_deficit = u16::MAX;
            for token_deficit in 0..=used_deficit.min(255) {
                let prefix = previous[used_deficit - token_deficit];
                if prefix == u32::MAX {
                    continue;
                }
                let candidate = prefix.checked_add(cost_by_deficit[token_deficit])?;
                if candidate < best_cost {
                    best_cost = candidate;
                    best_deficit = token_deficit as u16;
                }
            }
            current[used_deficit] = best_cost;
            choices[slot * width + used_deficit] = best_deficit;
        }
        previous = current;
    }
    Some(choices)
}

#[test]
fn same_distance_stats_count_maximal_runs() {
    let tokens = vec![
        Token::Literal(b'x'),
        test_match(3, 1, 0, 0, 0),
        test_match(4, 1, 0, 0, 0),
        test_match(5, 2, 1, 0, 0),
        Token::Literal(b'y'),
        test_match(130, 3, 2, 0, 0),
        test_match(130, 3, 2, 0, 0),
        // Distances five and six share symbol four, but are not one run.
        test_match(3, 5, 4, 0, 1),
        test_match(3, 6, 4, 1, 1),
    ];
    let plain_len: usize = tokens.iter().map(|token| token.decoded_len()).sum();
    let block = short_family_test_block(tokens, vec![0; plain_len]);

    assert_eq!(
        same_distance_opportunities(&[block]),
        SameDistanceOpportunities {
            runs: 2,
            matches: 4,
            decoded_bytes: 267,
            coalescible_runs: 1,
            repartition_runs: 1,
            tokens_removable: 1,
        }
    );
}

#[test]
fn same_distance_stats_join_source_blocks_but_skip_unchanged_minimum_runs() {
    let first = short_family_test_block(vec![test_match(100, 1, 0, 0, 0)], vec![0; 100]);
    let empty = short_family_test_block(Vec::new(), Vec::new());
    let last = short_family_test_block(
        vec![
            test_match(158, 1, 0, 0, 0),
            Token::Literal(b'x'),
            test_match(258, 2, 1, 0, 0),
            test_match(258, 2, 1, 0, 0),
        ],
        vec![0; 675],
    );

    assert_eq!(
        same_distance_opportunities(&[first, empty, last]),
        SameDistanceOpportunities {
            runs: 2,
            matches: 4,
            decoded_bytes: 774,
            coalescible_runs: 1,
            repartition_runs: 0,
            tokens_removable: 1,
        }
    );
}

#[test]
fn same_distance_repacking_coalesces_a_short_run() {
    let source = [test_match(3, 5, 4, 0, 1), test_match(4, 5, 4, 0, 1)];
    let repacked = test_repack(&source, 7, &FIXED_LITERAL_CODE_LENGTHS, false).unwrap();
    assert_eq!(
        repacked,
        [Token::Match {
            length: 7,
            distance: 5,
            length_symbol: 261,
            distance_symbol: 4,
            length_extra: 0,
            distance_extra: 0,
            length_extra_bits: 0,
            distance_extra_bits: 1,
        }]
    );

    let canonical_258 = [test_match(100, 1, 0, 0, 0), test_match(158, 1, 0, 0, 0)];
    assert!(matches!(
        test_repack(&canonical_258, 258, &FIXED_LITERAL_CODE_LENGTHS, false,)
            .unwrap()
            .as_slice(),
        [Token::Match {
            length: 258,
            length_symbol: 285,
            length_extra: 0,
            length_extra_bits: 0,
            ..
        }]
    ));
}

#[test]
fn direct_same_distance_coalescing_does_not_enter_the_dp() {
    let source = [test_match(100, 1, 0, 0, 0), test_match(158, 1, 0, 0, 0)];
    let mut dp_must_not_poll = || -> bool { panic!("direct coalescing entered the DP") };
    let mut stop = SearchStop::callback(&mut dp_must_not_poll);
    let repacked =
        repack_same_distance_runs(&source, 258, &FIXED_LITERAL_CODE_LENGTHS, false, &mut stop)
            .unwrap();
    assert_eq!(repacked.len(), 1);
}

#[test]
fn default_repacking_bounds_pathological_dp_depth_and_max_polls_deadline() {
    let source = vec![test_match(257, 1, 0, 0, 0); 257];
    let decoded_bytes = 257 * 257;
    let mut default_must_not_poll = || -> bool { panic!("default fallback entered the DP") };
    let mut default_stop = SearchStop::callback(&mut default_must_not_poll);
    let repacked = repack_same_distance_runs(
        &source,
        decoded_bytes,
        &FIXED_LITERAL_CODE_LENGTHS,
        false,
        &mut default_stop,
    )
    .unwrap();
    assert_eq!(repacked.len(), 257);
    assert_eq!(
        repacked
            .iter()
            .map(|token| token.decoded_len())
            .sum::<usize>(),
        decoded_bytes
    );

    let mut polls = 0_usize;
    let mut expires_during_dp = || {
        polls += 1;
        polls > 3
    };
    let mut expiring_stop = SearchStop::callback(&mut expires_during_dp);
    assert!(repack_same_distance_runs(
        &source,
        decoded_bytes,
        &FIXED_LITERAL_CODE_LENGTHS,
        true,
        &mut expiring_stop,
    )
    .is_none());
    assert!(polls > 3);
}

#[test]
fn same_distance_partitioning_matches_direct_recurrence() {
    let mut seed = 0x1f8b_4321_u32;
    for profile in 0..24 {
        let mut lengths = FIXED_LITERAL_CODE_LENGTHS.to_vec();
        match profile {
            0 => lengths.clear(),
            1 => lengths.truncate(258),
            2 => {}
            3 => lengths.fill(u8::MAX),
            _ => {
                for length in &mut lengths {
                    seed ^= seed << 13;
                    seed ^= seed >> 17;
                    seed ^= seed << 5;
                    *length = (seed % if profile % 2 == 0 { 4 } else { 16 }) as u8;
                }
            }
        }
        let cases = [(2, 6), (16, 31), (16, 257), (257, 257)];
        for &(active, deficit) in &cases[..if profile < 4 { 4 } else { 3 }] {
            let expected =
                reference_partition_choices(&lengths, active, deficit, &mut SearchStop::never())
                    .unwrap();
            let actual = test_partitioner(&lengths, active, deficit).unwrap();
            assert_eq!(
                actual.choices, expected,
                "profile {profile}, {active}/{deficit}"
            );
        }
    }
}

#[test]
fn same_distance_partitioning_preserves_stop_probes() {
    for cutoff in 1..=31 {
        let mut expected_polls = 0;
        let mut expected_stop = || {
            expected_polls += 1;
            expected_polls >= cutoff
        };
        let expected = reference_partition_choices(
            &FIXED_LITERAL_CODE_LENGTHS,
            3,
            257,
            &mut SearchStop::callback(&mut expected_stop),
        );
        let mut actual_polls = 0;
        let mut actual_stop = || {
            actual_polls += 1;
            actual_polls >= cutoff
        };
        let actual = SameDistancePartitioner::new(
            &FIXED_LITERAL_CODE_LENGTHS,
            3,
            257,
            &mut SearchStop::callback(&mut actual_stop),
        )
        .map(|partitioner| partitioner.choices);
        assert_eq!(actual, expected, "cutoff {cutoff}");
        assert_eq!(actual_polls, expected_polls, "cutoff {cutoff}");
    }
}

#[test]
fn same_distance_partitioning_handles_small_remainders() {
    for total in [259_usize, 260] {
        let left = total / 2;
        let source = [
            test_match(left as u16, 1, 0, 0, 0),
            test_match((total - left) as u16, 1, 0, 0, 0),
        ];
        let repacked = test_repack(&source, total, &FIXED_LITERAL_CODE_LENGTHS, false).unwrap();
        assert_eq!(repacked.len(), 2);
        assert!(repacked
            .iter()
            .all(|token| (3..=258).contains(&token.decoded_len())));
        assert_eq!(
            repacked
                .iter()
                .map(|token| token.decoded_len())
                .sum::<usize>(),
            total
        );
    }
}

#[test]
fn same_distance_partitioning_is_legal_for_all_small_totals() {
    let partitioner = test_partitioner(&FIXED_LITERAL_CODE_LENGTHS, 16, 257).unwrap();
    for total in 6_usize..=4_096 {
        let matches = total.div_ceil(258);
        let deficit = 258 * matches - total;
        let active = matches.min(deficit);
        let active_lengths = partitioner.active_lengths(active, deficit).unwrap();
        let lengths = std::iter::repeat(258_u16)
            .take(matches - active)
            .chain(active_lengths)
            .collect::<Vec<_>>();
        assert_eq!(lengths.len(), matches, "total {total}");
        assert!(
            lengths.iter().all(|length| (3..=258).contains(length)),
            "total {total}: {lengths:?}"
        );
        assert_eq!(
            lengths.iter().copied().map(usize::from).sum::<usize>(),
            total
        );
    }
}

#[test]
fn same_distance_partitioning_uses_huffman_costs() {
    let mut prefer_258 = [15_u8; 286];
    prefer_258[284] = 15;
    prefer_258[285] = 1;
    let first = test_partitioner(&prefer_258, 2, 6)
        .unwrap()
        .active_lengths(2, 6)
        .unwrap();

    let mut avoid_258 = [15_u8; 286];
    avoid_258[284] = 1;
    avoid_258[285] = 15;
    let second = test_partitioner(&avoid_258, 2, 6)
        .unwrap()
        .active_lengths(2, 6)
        .unwrap();

    assert_eq!(first, [258, 252]);
    assert_eq!(second, [257, 253]);
}

#[test]
fn same_distance_repacking_respects_barriers_and_strict_comparison() {
    let tokens = vec![
        Token::Literal(b'a'),
        test_match(3, 1, 0, 0, 0),
        test_match(4, 2, 1, 0, 0),
        Token::Literal(b'b'),
        test_match(3, 1, 0, 0, 0),
    ];
    assert!(test_repack(&tokens, 12, &FIXED_LITERAL_CODE_LENGTHS, false).is_none());

    let already_minimal = [test_match(258, 1, 0, 0, 0); 2];
    assert!(test_repack(&already_minimal, 516, &FIXED_LITERAL_CODE_LENGTHS, false,).is_none());

    let redundant_exact_multiple = [
        test_match(100, 1, 0, 0, 0),
        test_match(158, 1, 0, 0, 0),
        test_match(258, 1, 0, 0, 0),
    ];
    assert_eq!(
        test_repack(
            &redundant_exact_multiple,
            516,
            &FIXED_LITERAL_CODE_LENGTHS,
            false,
        )
        .unwrap(),
        [test_match(258, 1, 0, 0, 0); 2]
    );

    let source = vec![
        Token::Literal(b'a'),
        test_match(3, 1, 0, 0, 0),
        test_match(4, 1, 0, 0, 0),
    ];
    let block = short_family_test_block(source.clone(), vec![b'a'; 8]);
    let incumbent = PlannedBlock {
        tokens: source.into(),
        plain: block.plain.clone(),
        representation: Representation::Fixed,
        bits: 0,
        source_type: SourceBlockType::Fixed,
    };
    let retained = improve_plan_with_same_distance_floor(&block, 0, &Options::default(), incumbent);
    assert_eq!(retained.bits, 0);
    assert_eq!(retained.tokens, block.tokens);
}
