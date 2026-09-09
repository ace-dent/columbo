// SPDX-License-Identifier: MIT

use super::*;

#[test]
fn rewrite_limits_are_inclusive_and_rejections_keep_the_scan_charge() {
    for (count, length, accepted) in [
        (64, 3, true),
        (65, 3, false),
        (32, 256, true),
        (32, 257, false),
        (0, 3, false),
    ] {
        let (length_symbol, length_extra, length_extra_bits) =
            super::super::model::canonical_length_encoding(length).unwrap();
        let token = Token::Match {
            length,
            distance: 1,
            length_symbol,
            distance_symbol: 0,
            length_extra,
            distance_extra: 0,
            length_extra_bits,
            distance_extra_bits: 0,
        };
        let mut tokens = vec![Token::Literal(b'a')];
        tokens.extend(std::iter::repeat(token).take(count));
        tokens.resize(MAX_BLOCK_TOKENS, Token::Literal(b'a'));
        let bytes = count * usize::from(length);
        let plain = vec![b'a'; MAX_BLOCK_TOKENS - count + bytes];
        let (literal_frequencies, distance_frequencies) = count_frequencies(&tokens);
        let block = ParsedBlock {
            tokens: tokens.into(),
            plain: plain.into(),
            literal_frequencies,
            distance_frequencies,
            original_literal_lengths: None,
            original_distance_lengths: None,
            original_dynamic: None,
            original: None,
            source_splits: Vec::new(),
            source_type: super::super::model::SourceBlockType::Fixed,
        };
        let mut budget = SymbolSetBudget::new();
        let initial_work = budget.work_left;
        let mut calls = 0;
        let mut counter = || {
            calls += 1;
            false
        };
        let result = rewrite(
            &block,
            Ban {
                lengths: 0,
                distances: 1,
            },
            &super::super::huffman::FIXED_LITERAL_CODE_LENGTHS,
            &super::super::huffman::FIXED_DISTANCE_CODE_LENGTHS,
            &mut budget,
            &mut SearchStop::callback(&mut counter),
        );
        assert_eq!(result.is_some(), accepted);
        let rewrite_work = if accepted {
            bytes + block.tokens.len()
        } else {
            0
        };
        assert_eq!(
            budget.work_left,
            initial_work - block.tokens.len() - rewrite_work
        );
        assert_eq!(budget.prices_left, 128);
        if let Some(result) = result {
            assert_eq!(result, vec![Token::Literal(b'a'); block.plain.len()]);
            assert_proven_rewrite(&block, &result);
            assert_eq!(calls, 1 + MAX_BLOCK_TOKENS / 256);
        } else {
            assert_eq!(calls, 1);
        }
    }
}

#[test]
fn grouped_removal_keeps_certificates_and_strict_codes() {
    let block = symbol_set_test_block();
    let mut budget = SymbolSetBudget::new();
    let selected = plan_symbol_sets(
        &block,
        0,
        &Options::default(),
        &mut budget,
        &mut SearchStop::never(),
    )
    .unwrap();
    assert_eq!(block.original.unwrap().len, 731);
    assert_eq!(selected.bits, 727);
    assert_ne!(selected.tokens, block.tokens);
    assert_proven_rewrite(&block, &selected.tokens);
    let Representation::Dynamic(plan) = &selected.representation else {
        panic!("expected dynamic witness")
    };
    assert!(plan.has_strictly_compatible_huffman_codes());
    assert!(budget.work_left < 1 << 25);
    assert!(budget.prices_left < 128);
    assert!(budget.prices_left >= 112);
}

#[test]
fn interval_masks_keep_holes_forbidden_and_exclude_reserved_distances() {
    let mut block = symbol_set_test_block();
    block.literal_frequencies.fill(0);
    block.distance_frequencies.fill(0);
    for i in [257, 260, 266, 285] {
        block.literal_frequencies[i] = 1;
    }
    for i in [0, 7, 20, 29] {
        block.distance_frequencies[i] = 1;
    }
    let masks = masks(&block).unwrap();
    assert_eq!(masks.len(), 20);
    assert!(masks
        .iter()
        .all(|b| b.distances >> 30 == 0 && b.lengths >> 29 == 0));
    assert!(masks.contains(&Ban {
        lengths: 15,
        distances: 0
    }));
    assert!(masks.contains(&Ban {
        lengths: 0,
        distances: 255
    }));
    assert!(masks
        .iter()
        .all(|&b| (1..=4).contains(&b.targeted_symbols(&block))));
}

#[test]
fn budgets_and_stops_preserve_completed_parents() {
    let block = symbol_set_test_block();
    for (work_left, prices_left) in [(0, 128), (1 << 25, 0)] {
        let mut budget = SymbolSetBudget {
            work_left,
            prices_left,
        };
        assert!(plan_symbol_sets(
            &block,
            0,
            &Options::default(),
            &mut budget,
            &mut SearchStop::never()
        )
        .is_none());
        assert!(budget.work_left <= work_left && budget.prices_left <= prices_left);
    }
    assert!(plan_symbol_sets(
        &block,
        0,
        &Options::default(),
        &mut SymbolSetBudget::new(),
        &mut SearchStop::always()
    )
    .is_none());
    let mut calls = 0;
    let mut counter = || {
        calls += 1;
        false
    };
    let result = plan_symbol_sets(
        &block,
        0,
        &Options::default(),
        &mut SymbolSetBudget::new(),
        &mut SearchStop::callback(&mut counter),
    )
    .unwrap();
    let minimum = result.bits;
    // Stop at different points in generation, pricing and feedback. Any
    // returned plan must be complete, proven and strictly below the parent.
    let mut retained_after_stop = false;
    let mut cutoffs: Vec<_> = (1..16).map(|step| calls * step / 16).collect();
    // Candidate generation dominates the stop probes on this witness.
    // Also sample the tail densely to exercise completed prices/feedback.
    let mut tail = 1;
    while tail < calls {
        cutoffs.push(calls - tail);
        tail *= 2;
    }
    cutoffs.sort_unstable();
    cutoffs.dedup();
    for cutoff in cutoffs {
        let mut observed = 0;
        let mut stop = || {
            observed += 1;
            observed >= cutoff
        };
        if let Some(plan) = plan_symbol_sets(
            &block,
            0,
            &Options::default(),
            &mut SymbolSetBudget::new(),
            &mut SearchStop::callback(&mut stop),
        ) {
            assert!(plan.bits >= minimum && plan.bits < block.original.unwrap().len);
            assert_proven_rewrite(&block, &plan.tokens);
            retained_after_stop |= observed >= cutoff;
        }
    }
    assert!(retained_after_stop);
}
