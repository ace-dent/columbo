// SPDX-License-Identifier: MIT
// SPDX-FileCopyrightText: 2026 Andrew C.E. Dent <https://github.com/ace-dent>

use super::test_support::{assert_proven_rewrite, symbol_set_test_block};
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
            &BlockMatches::new(&block).unwrap(),
            Ban {
                lengths: 0,
                distances: 1,
            },
            &super::super::huffman::FIXED_LITERAL_CODE_LENGTHS,
            &super::super::huffman::FIXED_DISTANCE_CODE_LENGTHS,
            None,
            &mut budget,
            &mut SearchStop::callback(&mut counter),
        )
        .and_then(|rewritten| rewritten.spell(&block));
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
    // Candidate generation makes most of the stop checks on this fixture.
    // Also sample the tail densely to exercise completed pricing and feedback.
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

#[test]
fn remembered_submatch_spellings_match_fresh_rewrites() {
    let mut seed = 0x6c07_8965u32;
    let mut next = move || {
        seed ^= seed << 13;
        seed ^= seed >> 17;
        seed ^= seed << 5;
        seed
    };
    for case in 0..12 {
        // Matches at distance 1 inside one run, so every submatch is proven.
        let mut tokens = vec![Token::Literal(b'a')];
        for _ in 0..120 {
            if next() % 6 == 0 {
                tokens.push(Token::Literal(b'a'));
                continue;
            }
            let length = 3 + (next() % if case % 2 == 0 { 256 } else { 40 }) as u16;
            let (length_symbol, length_extra, length_extra_bits) =
                super::super::model::canonical_length_encoding(length).unwrap();
            tokens.push(Token::Match {
                length,
                distance: 1,
                length_symbol,
                distance_symbol: 0,
                length_extra,
                distance_extra: 0,
                length_extra_bits,
                distance_extra_bits: 0,
            });
        }
        let plain_len = tokens.iter().map(|token| token.decoded_len()).sum();
        let (literal_frequencies, distance_frequencies) = count_frequencies(&tokens);
        let block = ParsedBlock {
            tokens: tokens.into(),
            plain: vec![b'a'; plain_len].into(),
            literal_frequencies,
            distance_frequencies,
            original_literal_lengths: None,
            original_distance_lengths: None,
            original_dynamic: None,
            original: None,
            source_splits: Vec::new(),
            source_type: super::super::model::SourceBlockType::Fixed,
        };
        let weights: Vec<u32> = (0..286).map(|_| 1 + next() % 1000).collect();
        let literal_lengths = super::super::huffman::make_lengths(&weights, 15, 0);
        let distance_lengths = [5_u8; 30];
        let mut bans = masks(&block).unwrap();
        let crosses: Vec<_> = bans
            .iter()
            .filter(|literal| literal.distances == 0)
            .flat_map(|literal| {
                bans.iter()
                    .filter(|distance| distance.lengths == 0)
                    .map(|distance| Ban {
                        lengths: literal.lengths,
                        distances: distance.distances,
                    })
            })
            .collect();
        bans.extend(crosses);
        let matches = BlockMatches::new(&block).unwrap();
        let mut spellings = SubmatchSpellings::new(&block).unwrap();
        let mut remembered = 0;
        for _ in 0..2 {
            for &ban in &bans {
                let mut fresh_budget = SymbolSetBudget::new();
                let mut kept_budget = SymbolSetBudget::new();
                let fresh = rewrite(
                    &block,
                    &matches,
                    ban,
                    &literal_lengths,
                    &distance_lengths,
                    None,
                    &mut fresh_budget,
                    &mut SearchStop::never(),
                )
                .and_then(|rewritten| rewritten.spell(&block));
                let kept = rewrite(
                    &block,
                    &matches,
                    ban,
                    &literal_lengths,
                    &distance_lengths,
                    Some(&mut spellings),
                    &mut kept_budget,
                    &mut SearchStop::never(),
                )
                .and_then(|rewritten| rewritten.spell(&block));
                assert_eq!(kept, fresh, "case {case}: {ban:?}");
                assert_eq!(kept_budget.work_left, fresh_budget.work_left);
                if let Some(tokens) = kept {
                    assert_proven_rewrite(&block, &tokens);
                    remembered += 1;
                }
            }
        }
        assert!(remembered > 0, "case {case}");
    }
}

/// The rewrite before it visited only matches: every token copied and the
/// result counted afresh.
fn reference_rewrite(
    block: &ParsedBlock,
    ban: Ban,
    literal_lengths: &[u8],
    distance_lengths: &[u8],
    mut spellings: Option<&mut SubmatchSpellings>,
    budget: &mut SymbolSetBudget,
    stop: &mut SearchStop<'_>,
) -> Option<Vec<Token>> {
    if stop.reached() || !budget.spend(block.tokens.len()) {
        return None;
    }
    let mut affected = 0;
    let mut bytes = 0;
    let mut edges = 0;
    for &token in block.tokens.iter() {
        if !ban.affects(token) {
            continue;
        }
        affected += 1;
        let length = token.decoded_len();
        bytes += length;
        if affected > MAX_AFFECTED_MATCHES || bytes > MAX_REWRITTEN_BYTES {
            return None;
        }
        if let Token::Match {
            distance_symbol, ..
        } = token
        {
            if ban.distances & (1 << distance_symbol) == 0 {
                // The restricted submatch solver visits this many match edges,
                // including banned lengths. Charge the whole search up front.
                edges += (length - 1) * (length - 2) / 2;
            }
        }
    }
    if affected == 0 || !budget.spend(edges + bytes + block.tokens.len()) {
        return None;
    }
    let mut result = Vec::new();
    result.try_reserve_exact(block.tokens.len() + bytes).ok()?;
    let mut position = 0;
    for (i, &token) in block.tokens.iter().enumerate() {
        if i & 255 == 0 && stop.reached() {
            return None;
        }
        let end = position + token.decoded_len();
        let decoded = block.plain.get(position..end)?;
        match token {
            Token::Match {
                distance_symbol, ..
            } if ban.distances & (1 << distance_symbol) != 0 => {
                result.extend(decoded.iter().copied().map(Token::Literal))
            }
            Token::Match { length_symbol, .. }
                if ban.lengths & (1 << (length_symbol - 257)) != 0 =>
            {
                let solve = |stop: &mut SearchStop<'_>| {
                    solve_proven_submatch_avoiding(
                        token,
                        decoded,
                        literal_lengths,
                        distance_lengths,
                        ban.lengths,
                        stop,
                    )
                };
                let spelled = match spellings.as_deref_mut() {
                    Some(spellings) => {
                        let relevant = ban.lengths & ((2 << (length_symbol - 257)) - 1);
                        spellings.spell(i, relevant, stop, solve)
                    }
                    None => solve(stop),
                };
                result.extend(spelled?);
            }
            _ => result.push(token),
        }
        position = end;
    }
    (position == block.plain.len()).then_some(result)
}

#[test]
fn match_only_rewrites_match_full_token_reference() {
    let mut seed = 0x1b87_3a95u32;
    let mut next = move || {
        seed ^= seed << 13;
        seed ^= seed >> 17;
        seed ^= seed << 5;
        seed
    };
    let mut accepted = 0;
    for case in 0..8 {
        // Literals and matches at distance 1 inside one run, so every
        // submatch is proven; more than 256 tokens, so the stop is checked
        // several times.
        let mut tokens = vec![Token::Literal(b'a')];
        while tokens.len() < 700 {
            if next() % 3 != 0 {
                tokens.push(Token::Literal(b'a'));
                continue;
            }
            let length = 3 + (next() % if case % 2 == 0 { 120 } else { 20 }) as u16;
            let (length_symbol, length_extra, length_extra_bits) =
                super::super::model::canonical_length_encoding(length).unwrap();
            // Distances 1-4 are codes 0-3, without extra bits.
            let distance_symbol = (next() % 4) as u8;
            let (distance, distance_extra, distance_extra_bits) =
                (u16::from(distance_symbol) + 1, 0, 0);
            tokens.push(Token::Match {
                length,
                distance,
                length_symbol,
                distance_symbol,
                length_extra,
                distance_extra,
                length_extra_bits,
                distance_extra_bits,
            });
        }
        let plain_len = tokens.iter().map(|token| token.decoded_len()).sum();
        let (literal_frequencies, distance_frequencies) = count_frequencies(&tokens);
        let block = ParsedBlock {
            tokens: tokens.into(),
            plain: vec![b'a'; plain_len].into(),
            literal_frequencies,
            distance_frequencies,
            original_literal_lengths: None,
            original_distance_lengths: None,
            original_dynamic: None,
            original: None,
            source_splits: Vec::new(),
            source_type: super::super::model::SourceBlockType::Fixed,
        };
        let weights: Vec<u32> = (0..286).map(|_| 1 + next() % 1000).collect();
        let literal_lengths = super::super::huffman::make_lengths(&weights, 15, 0);
        let distance_lengths = [5_u8; 30];
        let matches = BlockMatches::new(&block).unwrap();
        let mut bans = masks(&block).unwrap();
        let crosses: Vec<_> = bans
            .iter()
            .filter(|literal| literal.distances == 0)
            .flat_map(|literal| {
                bans.iter()
                    .filter(|distance| distance.lengths == 0)
                    .map(|distance| Ban {
                        lengths: literal.lengths,
                        distances: distance.distances,
                    })
            })
            .collect();
        bans.extend(crosses);
        for (index, &ban) in bans.iter().enumerate() {
            // Stop after a varying number of checks, or never.
            let stop_after = (index % 3 == 0).then(|| 1 + next() % 8);
            let run = |rewritten: bool| {
                let mut budget = SymbolSetBudget::new();
                let mut calls = 0;
                let mut should_stop = || {
                    calls += 1;
                    stop_after.is_some_and(|after| calls >= after)
                };
                let mut stop = SearchStop::callback(&mut should_stop);
                let tokens = if rewritten {
                    rewrite(
                        &block,
                        &matches,
                        ban,
                        &literal_lengths,
                        &distance_lengths,
                        None,
                        &mut budget,
                        &mut stop,
                    )
                    .map(|rewritten| {
                        let tokens = rewritten.spell(&block).unwrap();
                        let (literal, distance) = count_frequencies(&tokens);
                        assert_eq!(rewritten.literal_frequencies, literal);
                        assert_eq!(rewritten.distance_frequencies, distance);
                        assert_eq!(rewritten.extra_bits, token_extra_bits(&tokens));
                        tokens
                    })
                } else {
                    reference_rewrite(
                        &block,
                        ban,
                        &literal_lengths,
                        &distance_lengths,
                        None,
                        &mut budget,
                        &mut stop,
                    )
                };
                (tokens, budget.work_left, calls)
            };
            let expected = run(false);
            accepted += usize::from(expected.0.is_some());
            assert_eq!(run(true), expected, "case {case}: {ban:?}");
        }
    }
    assert!(accepted > 100);
}
