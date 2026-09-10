// SPDX-License-Identifier: MIT

use super::*;
use crate::deflate::header::rle_cost;
use crate::deflate::header::test_support::header_tree_test_block;

fn enumerate(seq: &[u8]) -> u64 {
    let mut allowed = [false; 19];
    let mut mandatory = [false; 19];
    let mut run = 0;
    for (i, &v) in seq.iter().enumerate() {
        allowed[usize::from(v)] = true;
        if v != 0 {
            mandatory[usize::from(v)] = true;
        }
        run = if i > 0 && seq[i - 1] == v { run + 1 } else { 1 };
        allowed[16] |= run >= 4;
        allowed[17] |= v == 0 && run >= 3;
        allowed[18] |= v == 0 && run >= 11;
    }
    let symbols: Vec<_> = (0..19).filter(|&s| allowed[s]).collect();
    assert!(symbols.len() <= 7);
    fn go(
        i: usize,
        capacity: usize,
        symbols: &[usize],
        mandatory: &[bool; 19],
        tree: &mut [u8; 19],
        seq: &[u8],
        best: &mut u64,
    ) {
        if i == symbols.len() {
            if capacity != 128 {
                return;
            }
            if let Some(rle) = shortest_rle(seq, tree) {
                let cost = 17 + 3 * trim_code_lengths(tree) as u64 + rle_cost(&rle, tree);
                *best = (*best).min(cost);
            }
            return;
        }
        let n = symbols.len() - i;
        if capacity > 128 || capacity + 64 * n < 128 {
            return;
        }
        let symbol = symbols[i];
        for length in u8::from(mandatory[symbol])..=7 {
            let next = capacity + weight(length);
            if next > 128 {
                continue;
            }
            tree[symbol] = length;
            go(i + 1, next, symbols, mandatory, tree, seq, best);
        }
        tree[symbol] = 0;
    }
    let mut best = u64::MAX;
    go(0, 0, &symbols, &mandatory, &mut [0; 19], seq, &mut best);
    best
}
#[test]
fn exact_search_matches_exhaustive_trees() {
    let mut seed = 0xa1b2c3d4u64;
    for trial in 0..80 {
        let mut seq = vec![1, 0, 2];
        for _ in 0..(trial % 8 + 2) {
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
            let value = ((seed >> 32) % 4) as u8;
            let count = ((seed >> 40) % 15 + 1) as usize;
            seq.extend(std::iter::repeat(value).take(count));
        }
        let runs = Runs::new(&seq).unwrap();
        let (tree, cost) = search(
            &runs,
            u64::MAX,
            &mut HeaderTreeBudget {
                work_left: usize::MAX,
            },
            &mut SearchStop::never(),
        )
        .unwrap();
        let rle = shortest_rle(&seq, &tree).unwrap();
        assert_eq!(
            cost,
            17 + 3 * trim_code_lengths(&tree) as u64 + rle_cost(&rle, &tree)
        );
        assert_eq!(cost, enumerate(&seq), "trial {trial} {seq:?}");
    }
}

#[test]
fn zero_run_prices_match_full_rle_paths() {
    for count in [1, 2, 3, 4, 6, 7, 10, 11, 12, 137, 138, 139, 140, 276, 318] {
        let mut runs = Runs::new(&[1, 0]).unwrap();
        runs.counts[0] = [0; LENGTHS];
        runs.counts[0][count] = 1;
        runs.longest[0] = count;
        let sequence = vec![0; count];
        let mut scratch = ZeroScratch::new();
        for a in [0, 1, 4, 7] {
            for b in [0, 1, 3, 7] {
                for c in [0, 2, 7] {
                    for d in [0, 1, 5, 7] {
                        let mut tree = [0; 19];
                        tree[0] = a;
                        tree[16] = b;
                        tree[17] = c;
                        tree[18] = d;
                        assert_eq!(
                            scratch.price(&runs, &tree).map(u64::from),
                            shortest_rle(&sequence, &tree).map(|rle| rle_cost(&rle, &tree)),
                            "run {count}, tree {tree:?}"
                        );
                    }
                }
            }
        }
    }
}

#[test]
fn completed_prices_survive_budget_and_deadline_stops() {
    let seq: Vec<_> = [1, 0, 2]
        .into_iter()
        .chain(std::iter::repeat(3).take(15))
        .chain(std::iter::repeat(0).take(30))
        .collect();
    let runs = Runs::new(&seq).unwrap();
    let mut budget = HeaderTreeBudget { work_left: 0 };
    assert!(search(&runs, u64::MAX, &mut budget, &mut SearchStop::never()).is_none());
    let mut best = None;
    let mut first_budget = None;
    for work in (100..50_000).step_by(100) {
        let result = search(
            &runs,
            u64::MAX,
            &mut HeaderTreeBudget { work_left: work },
            &mut SearchStop::never(),
        );
        if let Some((tree, bits)) = result {
            assert_eq!(
                bits,
                17 + 3 * trim_code_lengths(&tree) as u64
                    + rle_cost(&shortest_rle(&seq, &tree).unwrap(), &tree)
            );
            if first_budget.is_none() {
                first_budget = Some(work);
            }
            if let Some(previous) = best {
                assert!(bits <= previous);
            }
            best = Some(bits);
        }
    }
    assert!(first_budget.is_some());
    let mut polls = 0;
    let mut cutoff = || {
        polls += 1;
        polls >= 128
    };
    let result = search(
        &runs,
        u64::MAX,
        &mut HeaderTreeBudget::new(),
        &mut SearchStop::callback(&mut cutoff),
    );
    assert!(result.is_some());
    assert!(polls >= 128);
}

#[test]
fn complete_plan_beats_full_header_repricing_without_changing_payload_codes() {
    use crate::deflate::header::{plan_for_advertised_lengths, token_bits};
    let block = header_tree_test_block();
    let parent = block.original_dynamic.as_ref().unwrap();
    let data = token_bits(
        &block.tokens,
        &parent.literal_lengths,
        &parent.distance_lengths,
    )
    .unwrap();
    let control =
        plan_for_advertised_lengths(&parent.literal_lengths, &parent.distance_lengths, data)
            .unwrap();
    assert_eq!(control.bits, block.original.unwrap().len);
    for strict in [false, true] {
        let plan = plan_header_tree(
            &block,
            strict,
            &mut HeaderTreeBudget::new(),
            &mut SearchStop::never(),
        )
        .unwrap();
        assert_eq!(plan.bits + 3, control.bits);
        assert_eq!(plan.literal_lengths, parent.literal_lengths);
        assert_eq!(plan.distance_lengths, parent.distance_lengths);
        assert_eq!((plan.hlit, plan.hdist), (parent.hlit, parent.hdist));
        assert!(plan.has_strictly_compatible_huffman_codes());
        assert_eq!(dynamic_bits(data, &plan), Some(plan.bits));
    }
    let mut budget = HeaderTreeBudget::new();
    let before = budget.work_left;
    assert!(plan_header_tree(&block, true, &mut budget, &mut SearchStop::always()).is_none());
    assert_eq!(before, budget.work_left);
}

#[test]
fn kraft_search_covers_all_literal_values_and_seven_bit_trees() {
    let sequence: Vec<_> = (1..=15).collect();
    let runs = Runs::new(&sequence).unwrap();
    let (_, bits) = search(
        &runs,
        u64::MAX,
        &mut HeaderTreeBudget::new(),
        &mut SearchStop::never(),
    )
    .unwrap();
    assert_eq!(bits, 17 + 19 * 3 + 3 + 14 * 4);

    let mut sequence = Vec::new();
    for symbol in 2..=8 {
        for _ in 0..(1 << (8 - symbol)) {
            sequence.extend([1, symbol]);
        }
    }
    sequence.push(1);
    let runs = Runs::new(&sequence).unwrap();
    let (tree, bits) = search(
        &runs,
        u64::MAX,
        &mut HeaderTreeBudget::new(),
        &mut SearchStop::never(),
    )
    .unwrap();
    assert_eq!(&tree[1..=8], &[1, 2, 3, 4, 5, 6, 7, 7]);
    assert_eq!(bits, 17 + 18 * 3 + 501);
}

#[test]
fn relaxed_distance_exceptions_still_emit_valid_headers() {
    use crate::deflate::{
        bitstream::BitWriter,
        block::emit_block,
        header::{plan_for_advertised_lengths, token_bits},
        model::{count_frequencies, PlannedBlock, Representation, SourceBlockType, Token},
        parse::parse_stream,
    };
    let fixture = header_tree_test_block();
    let literal = fixture.original_dynamic.unwrap().literal_lengths;
    let symbol = (0..256).find(|&i| literal[i] != 0).unwrap() as u8;
    let tokens = vec![Token::Literal(symbol); 7];
    let (literal_frequencies, distance_frequencies) = count_frequencies(&tokens);
    for distance in [vec![0], vec![1]] {
        let data = token_bits(&tokens, &literal, &distance).unwrap();
        let mut original = plan_for_advertised_lengths(&literal, &distance, data).unwrap();
        // A valid, deliberately redundant CL tree guarantees that both
        // relaxed distance exceptions exercise an emitted improvement.
        original.code_length_lengths = [4; 19];
        original.code_length_lengths[13..].fill(5);
        original.hclen = 19;
        original.bits = dynamic_bits(data, &original).unwrap();
        let mut writer = BitWriter::default();
        let plan = PlannedBlock {
            tokens: tokens.clone().into(),
            plain: vec![symbol; 7].into(),
            bits: original.bits,
            representation: Representation::Dynamic(original),
            source_type: SourceBlockType::Dynamic,
        };
        emit_block(&mut writer, &[], &plan, true).unwrap();
        let input = writer.into_bytes();
        let parsed = parse_stream(&input, 32).unwrap();
        let block = &parsed.blocks[0];
        assert_eq!(block.literal_frequencies, literal_frequencies);
        assert_eq!(block.distance_frequencies, distance_frequencies);
        assert!(plan_header_tree(
            block,
            true,
            &mut HeaderTreeBudget::new(),
            &mut SearchStop::never()
        )
        .is_none());
        {
            let candidate = plan_header_tree(
                block,
                false,
                &mut HeaderTreeBudget::new(),
                &mut SearchStop::never(),
            )
            .expect("a redundant CL tree must improve for both relaxed exceptions");
            assert!(candidate.bits < block.original.unwrap().len);
            assert_eq!(candidate.distance_lengths, distance);
            let bits = candidate.bits;
            let mut writer = BitWriter::default();
            let mut plan = plan.clone();
            plan.representation = Representation::Dynamic(candidate);
            plan.bits = bits;
            emit_block(&mut writer, &[], &plan, true).unwrap();
            assert_eq!(writer.bit_position(), bits);
            let result = parse_stream(&writer.into_bytes(), 32).unwrap();
            assert_eq!(result.blocks[0].plain, block.plain);
        }
    }
}
