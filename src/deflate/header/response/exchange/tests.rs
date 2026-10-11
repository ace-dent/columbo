// SPDX-License-Identifier: MIT
// SPDX-FileCopyrightText: 2026 Andrew C.E. Dent <https://github.com/ace-dent>

use super::super::{plan_header_response, spell, submatch, STREAM_PRICES, STREAM_WORK};
use super::*;

fn seed(length: usize) -> Token {
    submatch(
        Token::Match {
            length: 3,
            distance: 1,
            length_symbol: 257,
            length_extra: 0,
            length_extra_bits: 0,
            distance_symbol: 0,
            distance_extra: 0,
            distance_extra_bits: 0,
        },
        length,
    )
    .unwrap()
}

/// Oracle for `interval_bound` that tries every canonical length, not just
/// each family's longest.
fn scanning_interval_bound(interval: &Interval, literal: &[u8], distance: &[u8]) -> Option<u64> {
    let n = interval.seed.decoded_len();
    let mut ratio = interval.literal_min.map(|bits| (bits, 1_u64));
    let mut consider = |bits: u64, width: usize| {
        let width = width as u64;
        if ratio.map_or(true, |(b, w)| bits * w < b * width) {
            ratio = Some((bits, width));
        }
    };
    if let Some(bits) = match_price(interval.seed, literal, distance) {
        consider(bits, n);
    }
    for width in 3..=n {
        if let Some(bits) = match_price(submatch(interval.seed, width)?, literal, distance) {
            consider(bits, width);
        }
    }
    let (bits, width) = ratio?;
    Some((n as u64 * bits).div_ceil(width))
}

#[test]
fn interval_bound_never_exceeds_exact_cost_with_absent_source_symbols() {
    let mut state = 193_u64;
    for n in 3..=258 {
        let mut literal = [0; 286];
        for l in &mut literal {
            state = state.wrapping_mul(6364136223846793005).wrapping_add(1);
            *l = ((state >> 32) % 9) as u8;
        }
        literal[256] = 1;
        let plain: Vec<_> = (0..n).map(|i| (i % 3) as u8).collect();
        let interval = Interval {
            seed: seed(n),
            literal_min: plain
                .iter()
                .filter_map(|&b| price(&literal, b as usize))
                .min(),
        };
        for absent in [false, true] {
            if absent {
                let Token::Match { length_symbol, .. } = interval.seed else {
                    unreachable!()
                };
                literal[length_symbol as usize] = 0;
            }
            let bound = interval_bound(&interval, &literal, &[2]);
            assert_eq!(bound, scanning_interval_bound(&interval, &literal, &[2]));
            let mut tokens = Vec::new();
            if spell(
                interval.seed,
                &plain,
                &literal,
                &[2],
                &mut ResponseBudget::new(),
                &mut SearchStop::never(),
                &mut tokens,
            )
            .is_some()
            {
                assert!(bound.unwrap() < token_bits(&tokens, &literal, &[2]).unwrap());
            }
        }
    }
}

#[test]
fn bound_includes_the_exact_relaxed_258_edge_and_detects_no_edges() {
    let mut literal = [0; 286];
    literal[284] = 1;
    let source = Token::Match {
        length: 258,
        length_symbol: 284,
        length_extra: 31,
        length_extra_bits: 5,
        distance: 1,
        distance_symbol: 0,
        distance_extra: 0,
        distance_extra_bits: 0,
    };
    let interval = Interval {
        seed: source,
        literal_min: None,
    };
    assert_eq!(interval_bound(&interval, &literal, &[1]), Some(7));
    let mut tokens = Vec::new();
    spell(
        source,
        &[42; 258],
        &literal,
        &[1],
        &mut ResponseBudget::new(),
        &mut SearchStop::never(),
        &mut tokens,
    )
    .unwrap();
    assert_eq!(tokens, vec![source]);
    assert_eq!(interval_bound(&interval, &[0; 286], &[1]), None);
}

fn witness() -> ParsedBlock {
    crate::deflate::parse::parse_stream(
        &crate::deflate::header::test_support::length_exchange_test_stream(),
        1024,
    )
    .unwrap()
    .blocks
    .remove(0)
}

fn verify(block: &ParsedBlock, plan: &PlannedBlock) {
    crate::deflate::symbol_set::test_support::assert_proven_rewrite(block, &plan.tokens);
    let old = block.original_dynamic.as_ref().unwrap();
    let Representation::Dynamic(new) = &plan.representation else {
        panic!("expected dynamic")
    };
    assert!(new.has_strictly_compatible_huffman_codes());
    assert_eq!(&old.literal_lengths[..257], &new.literal_lengths[..257]);
    assert_eq!(old.distance_lengths, new.distance_lengths);
    let mut a = [0; 286];
    let mut b = [0; 286];
    a[..old.literal_lengths.len()].copy_from_slice(&old.literal_lengths);
    b[..new.literal_lengths.len()].copy_from_slice(&new.literal_lengths);
    let removed: Vec<_> = (257..286).filter(|&i| a[i] > 0 && b[i] == 0).collect();
    let added: Vec<_> = (257..286).filter(|&i| a[i] == 0 && b[i] > 0).collect();
    assert_eq!(removed.len(), 1);
    assert_eq!(added.len(), 1);
    assert!(block.literal_frequencies[removed[0]] > 0);
    let (literal, _) = crate::deflate::model::count_frequencies(&plan.tokens);
    assert_eq!(literal[removed[0]], 0);
    assert_eq!(a[removed[0]], b[added[0]]);
    a.sort_unstable();
    b.sort_unstable();
    assert_eq!(a, b);
    let payload = token_bits(&plan.tokens, &new.literal_lengths, &new.distance_lengths).unwrap();
    assert_eq!(
        plan_for_advertised_lengths(&new.literal_lengths, &new.distance_lengths, payload)
            .unwrap()
            .bits,
        plan.bits
    );
    let mut w = crate::deflate::bitstream::BitWriter::default();
    crate::deflate::block::emit_block(&mut w, &[], plan, true).unwrap();
    assert_eq!(w.bit_position(), plan.bits);
    let parsed = crate::deflate::parse::parse_stream(&w.into_bytes(), 1024).unwrap();
    assert_eq!(parsed.blocks[0].plain, block.plain);
}

#[test]
fn exchange_crosses_fixed_support_barrier_and_preserves_proofs() {
    let block = witness();
    assert_eq!(block.original.unwrap().len, 352);
    assert!(plan_header_response(
        &block,
        true,
        &mut ResponseBudget::new(),
        &mut SearchStop::never()
    )
    .is_none());
    for strict in [false, true] {
        let plan = plan_length_exchange(
            &block,
            strict,
            &mut ResponseBudget::new(),
            &mut SearchStop::never(),
        )
        .unwrap();
        assert_eq!(plan.bits, 351);
        verify(&block, &plan);
    }
}

#[test]
fn bounded_search_matches_unpruned_exchange_enumeration() {
    let block = witness();
    let parent = block.original_dynamic.as_ref().unwrap();
    let mut literal = [0; 286];
    literal[..parent.literal_lengths.len()].copy_from_slice(&parent.literal_lengths);
    let mut best = block.original.unwrap().len;
    for from in 257..286 {
        if literal[from] == 0 || block.literal_frequencies[from] == 0 {
            continue;
        }
        for to in 257..286 {
            if literal[to] != 0 {
                continue;
            }
            literal.swap(from, to);
            let mut unlimited = ResponseBudget {
                work_left: usize::MAX,
                prices_left: usize::MAX,
            };
            if let Some(tokens) = response(
                &block,
                &literal,
                &parent.distance_lengths,
                &mut unlimited,
                &mut SearchStop::never(),
            ) {
                let payload = token_bits(&tokens, &literal, &parent.distance_lengths).unwrap();
                let trimmed = literal.iter().rposition(|&l| l > 0).unwrap().max(256) + 1;
                for n in [trimmed, trimmed.max(parent.literal_lengths.len())] {
                    let plan = plan_for_advertised_lengths(
                        &literal[..n],
                        &parent.distance_lengths,
                        payload,
                    )
                    .unwrap();
                    best = best.min(plan.bits);
                }
            }
            literal.swap(from, to);
        }
    }
    let mut budget = ResponseBudget::new();
    let plan = plan_length_exchange(&block, true, &mut budget, &mut SearchStop::never()).unwrap();
    assert!(budget.work_left > 0 && budget.prices_left > 0);
    assert_eq!(plan.bits, best);
}

#[test]
fn exchange_cutoffs_and_shared_budgets_keep_only_complete_winners() {
    let block = witness();
    for prices in [0, 1, 8, 32, 128, STREAM_PRICES] {
        for work in [0, 1, 1000, 10000, 100000, 1000000, STREAM_WORK] {
            let mut budget = ResponseBudget {
                work_left: work,
                prices_left: prices,
            };
            let plan = plan_length_exchange(&block, true, &mut budget, &mut SearchStop::never());
            assert!(budget.work_left <= work && budget.prices_left <= prices);
            if let Some(plan) = plan {
                assert!(plan.bits < 352);
                verify(&block, &plan);
            }
            if prices == 0 || work == 0 {
                assert_eq!(budget.work_left, work);
            }
        }
    }
    let mut calls = 0;
    let best = plan_length_exchange(
        &block,
        true,
        &mut ResponseBudget::new(),
        &mut SearchStop::callback(&mut || {
            calls += 1;
            false
        }),
    )
    .unwrap();
    let mut previous = 352;
    for cap in [0, calls / 4, calls / 2, 3 * calls / 4, calls + 1] {
        let mut seen = 0;
        let plan = plan_length_exchange(
            &block,
            true,
            &mut ResponseBudget::new(),
            &mut SearchStop::callback(&mut || {
                seen += 1;
                seen > cap
            }),
        );
        let bits = plan.as_ref().map_or(352, |p| p.bits);
        assert!(bits <= previous);
        previous = bits;
        if let Some(plan) = plan {
            verify(&block, &plan);
        }
    }
    assert_eq!(previous, best.bits);
    let mut budget = ResponseBudget::new();
    for _ in 0..8 {
        let before = (budget.work_left, budget.prices_left);
        if let Some(plan) =
            plan_length_exchange(&block, true, &mut budget, &mut SearchStop::never())
        {
            verify(&block, &plan);
        }
        assert!(budget.work_left <= before.0 && budget.prices_left <= before.1);
    }
    assert!(budget.work_left < STREAM_WORK && budget.prices_left < STREAM_PRICES);
}

#[test]
fn exchange_rejects_oversized_or_incompatible_parents_without_work() {
    let original = witness();
    for kind in 0..5 {
        let mut block = original.clone();
        match kind {
            0 => block.plain = vec![0; MAX_PLAIN + 1].into(),
            1 => block.tokens = vec![Token::Literal(0); MAX_TOKENS + 1].into(),
            2 => block.tokens = vec![seed(3); MAX_MATCHES + 1].into(),
            3 => block.tokens = vec![Token::Literal(0)].into(),
            _ => block.original_dynamic.as_mut().unwrap().distance_lengths = vec![1],
        }
        let mut budget = ResponseBudget::new();
        assert!(
            plan_length_exchange(&block, true, &mut budget, &mut SearchStop::never()).is_none()
        );
        assert_eq!(
            (budget.work_left, budget.prices_left),
            (STREAM_WORK, STREAM_PRICES)
        );
    }
}
