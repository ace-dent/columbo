// SPDX-License-Identifier: MIT

use super::*;
use crate::deflate::header::test_support::rotation_test_block;
use crate::deflate::header::{plan_header_tree, token_bits, HeaderTreeBudget};

#[test]
fn cycle_beats_every_fully_repriced_pair_on_generated_parent() {
    let block = rotation_test_block(&[1, 1]);
    let parent = block.original_dynamic.as_ref().unwrap();
    let parent_bits = block.original.unwrap().len;
    assert_eq!(parent_bits, 1261);
    let mut literal = parent.literal_lengths.clone();
    let symbols: Vec<_> = literal
        .iter()
        .enumerate()
        .filter_map(|(s, &length)| (length != 0).then_some(s))
        .collect();
    for (i, &a) in symbols.iter().enumerate() {
        for &b in symbols.iter().skip(i + 1) {
            literal.swap(a, b);
            let data = token_bits(&block.tokens, &literal, &parent.distance_lengths).unwrap();
            let pair =
                plan_for_advertised_lengths(&literal, &parent.distance_lengths, data).unwrap();
            assert!(pair.bits >= parent_bits, "pair {a}, {b}");
            literal.swap(a, b);
        }
    }
    for strict in [false, true] {
        let candidate = plan_code_length_rotations(
            &block,
            strict,
            &mut RotationBudget::new(),
            &mut SearchStop::never(),
        )
        .unwrap();
        assert_eq!(candidate.bits, 1260);
        assert_eq!(candidate.distance_lengths, parent.distance_lengths);
        assert_eq!(
            (candidate.hlit, candidate.hdist),
            (parent.hlit, parent.hdist)
        );
        assert_eq!(candidate.literal_lengths[4], 5);
        assert_eq!(candidate.literal_lengths[13], 7);
        assert_eq!(candidate.literal_lengths[18], 6);
        assert!(candidate.has_strictly_compatible_huffman_codes());
        let data = token_bits(
            &block.tokens,
            &candidate.literal_lengths,
            &candidate.distance_lengths,
        )
        .unwrap();
        assert_eq!(dynamic_bits(data, &candidate), Some(candidate.bits));
    }
    // Check whether the existing exact CL-tree search can remove this miss
    // without changing any payload lengths.
    assert!(plan_header_tree(
        &block,
        true,
        &mut HeaderTreeBudget::new(),
        &mut SearchStop::never(),
    )
    .is_none());
}

#[test]
fn local_transition_delta_matches_full_sequence_and_rotation_is_invertible() {
    let mut seed = 42u64;
    for _ in 0..100 {
        let mut lengths = [0; 12];
        for length in &mut lengths {
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
            *length = ((seed >> 32) % 8) as u8;
        }
        for a in 0..10 {
            for b in a + 1..11 {
                for c in b + 1..12 {
                    for reverse in [false, true] {
                        let rotation = Rotation {
                            rank: 0,
                            tax: 0,
                            positions: [a, b, c],
                            reverse,
                        };
                        let mut changed = lengths;
                        rotation.apply(&mut changed, 0);
                        let before = lengths.windows(2).filter(|p| p[0] != p[1]).count() as i64;
                        let after = changed.windows(2).filter(|p| p[0] != p[1]).count() as i64;
                        assert_eq!(
                            transitions_removed(
                                &lengths,
                                [a, b, c],
                                [changed[a], changed[b], changed[c]]
                            ),
                            before - after
                        );
                        rotation.undo(&mut changed, 0);
                        assert_eq!(changed, lengths);
                    }
                }
            }
        }
    }
}

#[test]
fn proposal_menu_matches_independent_full_rotation_ranking() {
    let lengths = [0, 6, 4, 3, 3, 3, 6, 5, 2, 4, 2, 5, 4, 5, 0, 0, 6, 5];
    let frequencies = [0, 2, 3, 5, 8, 13, 2, 4, 8, 3, 7, 2, 1, 4];
    let before = lengths.windows(2).filter(|p| p[0] != p[1]).count() as i64;
    let mut expected = Vec::new();
    for a in 0..frequencies.len() {
        for b in a + 1..frequencies.len() {
            for c in b + 1..frequencies.len() {
                let old = [lengths[a], lengths[b], lengths[c]];
                if old.contains(&0) || old[0] == old[1] || old[0] == old[2] || old[1] == old[2] {
                    continue;
                }
                for reverse in [false, true] {
                    let mut rotation = Rotation {
                        rank: 0,
                        tax: 0,
                        positions: [a, b, c],
                        reverse,
                    };
                    let mut changed = lengths;
                    rotation.apply(&mut changed, 0);
                    let tax = frequencies
                        .iter()
                        .enumerate()
                        .map(|(s, &n)| {
                            i64::from(n) * (i64::from(changed[s]) - i64::from(lengths[s]))
                        })
                        .sum::<i64>();
                    let removed =
                        before - changed.windows(2).filter(|p| p[0] != p[1]).count() as i64;
                    if !(-32..=32).contains(&tax) || removed <= 0 {
                        continue;
                    }
                    rotation.tax = tax;
                    rotation.rank = tax - 3 * removed;
                    expected.push(rotation);
                }
            }
        }
    }
    expected.sort();
    assert!(expected.len() > MENU_SIZE);
    expected.truncate(MENU_SIZE);
    let actual = proposals(
        &lengths,
        0,
        &frequencies,
        &mut RotationBudget::new(),
        &mut SearchStop::never(),
    )
    .unwrap();
    assert_eq!(actual, expected);
}

#[test]
fn work_and_price_limits_preserve_completed_improvements() {
    let block = rotation_test_block(&[1, 1]);
    for (work_left, prices_left) in [(0, STREAM_PRICES), (STREAM_WORK, 0)] {
        let mut budget = RotationBudget {
            work_left,
            prices_left,
        };
        assert!(
            plan_code_length_rotations(&block, true, &mut budget, &mut SearchStop::never())
                .is_none()
        );
    }
    let mut previous = block.original.unwrap().len;
    let mut first_winning_price = None;
    for prices in 1..=MENU_SIZE {
        let mut budget = RotationBudget {
            work_left: STREAM_WORK,
            prices_left: prices,
        };
        if let Some(plan) =
            plan_code_length_rotations(&block, true, &mut budget, &mut SearchStop::never())
        {
            first_winning_price.get_or_insert(prices);
            assert!(plan.bits <= previous);
            previous = plan.bits;
        }
        assert!(budget.prices_left <= prices);
        assert!(budget.work_left <= STREAM_WORK);
    }
    assert!(first_winning_price.is_some());
    assert_eq!(previous, 1260);
    let mut budget = RotationBudget::new();
    assert!(
        plan_code_length_rotations(&block, true, &mut budget, &mut SearchStop::always()).is_none()
    );
    assert_eq!(
        (budget.work_left, budget.prices_left),
        (STREAM_WORK, STREAM_PRICES)
    );
}

#[test]
fn relaxed_distance_exceptions_preserve_support() {
    for distance in [&[0][..], &[1][..]] {
        let mut block = rotation_test_block(distance);
        // A valid but deliberately expensive CL tree ensures this test emits
        // a completed contender for both relaxed distance exceptions.
        let parent = block.original_dynamic.as_mut().unwrap();
        parent.code_length_lengths = [5; 19];
        parent.code_length_lengths[..13].fill(4);
        parent.hclen = 19;
        let data = token_bits(&block.tokens, &parent.literal_lengths, distance).unwrap();
        block.original.as_mut().unwrap().len = dynamic_bits(data, parent).unwrap();
        assert!(plan_code_length_rotations(
            &block,
            true,
            &mut RotationBudget::new(),
            &mut SearchStop::never()
        )
        .is_none());
        let plan = plan_code_length_rotations(
            &block,
            false,
            &mut RotationBudget::new(),
            &mut SearchStop::never(),
        )
        .unwrap();
        assert_eq!(plan.distance_lengths, distance);
        assert!(plan.bits < block.original.unwrap().len);
        use crate::deflate::{
            bitstream::BitWriter,
            block::emit_block,
            model::{PlannedBlock, Representation},
            parse::parse_stream,
        };
        let bits = plan.bits;
        let planned = PlannedBlock {
            tokens: block.tokens.clone(),
            plain: block.plain.clone(),
            representation: Representation::Dynamic(plan),
            bits,
            source_type: block.source_type,
        };
        let mut writer = BitWriter::default();
        emit_block(&mut writer, &[], &planned, true).unwrap();
        assert_eq!(writer.bit_position(), bits);
        let parsed = parse_stream(&writer.into_bytes(), 4096).unwrap();
        assert_eq!(parsed.blocks[0].tokens, block.tokens);
        assert_eq!(parsed.blocks[0].plain, block.plain);
        assert_eq!(
            parsed.blocks[0]
                .original_dynamic
                .as_ref()
                .unwrap()
                .distance_lengths,
            distance
        );
    }
}

#[test]
fn late_deadline_and_second_alphabet_work_stop_keep_the_completed_winner() {
    let block = rotation_test_block(&[1, 1]);
    let mut polls = 0;
    let full = plan_code_length_rotations(
        &block,
        true,
        &mut RotationBudget::new(),
        &mut SearchStop::callback(&mut || {
            polls += 1;
            false
        }),
    )
    .unwrap();
    let cutoff = polls - 1;
    polls = 0;
    let stopped = plan_code_length_rotations(
        &block,
        true,
        &mut RotationBudget::new(),
        &mut SearchStop::callback(&mut || {
            polls += 1;
            polls >= cutoff
        }),
    )
    .unwrap();
    assert_eq!(polls, cutoff);
    assert_eq!(stopped.bits, full.bits);

    let parent = block.original_dynamic.as_ref().unwrap();
    let sequence: Vec<_> = parent
        .literal_lengths
        .iter()
        .chain(&parent.distance_lengths)
        .copied()
        .collect();
    let mut budget = RotationBudget::new();
    proposals(
        &sequence,
        0,
        &block.literal_frequencies[..parent.hlit],
        &mut budget,
        &mut SearchStop::never(),
    )
    .unwrap();
    let used = STREAM_WORK - budget.work_left;
    let mut limited = RotationBudget {
        work_left: used,
        prices_left: STREAM_PRICES,
    };
    let completed =
        plan_code_length_rotations(&block, true, &mut limited, &mut SearchStop::never()).unwrap();
    assert_eq!(limited.work_left, 0);
    assert_eq!(completed.bits, full.bits);
}
