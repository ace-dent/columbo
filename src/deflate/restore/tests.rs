// SPDX-License-Identifier: MIT
// SPDX-FileCopyrightText: 2026 Andrew C.E. Dent <https://github.com/ace-dent>

use super::*;
use crate::deflate::bitstream::BitWriter;
use crate::deflate::block::emit_block;
use crate::deflate::model::{ParsedStream, DISTANCE_BASE, DISTANCE_EXTRA_BITS};
use crate::deflate::parse::parse_stream;

fn matched(length: u16, distance: u16) -> Token {
    let symbol = DISTANCE_BASE
        .iter()
        .rposition(|&base| base <= distance)
        .unwrap();
    submatch(
        Token::Match {
            length,
            distance,
            length_symbol: 0,
            length_extra: 0,
            length_extra_bits: 0,
            distance_symbol: symbol as u8,
            distance_extra: distance - DISTANCE_BASE[symbol],
            distance_extra_bits: DISTANCE_EXTRA_BITS[symbol],
        },
        length,
    )
    .unwrap()
}

fn fixed(tokens: Vec<Token>) -> PlannedBlock {
    PlannedBlock {
        plain: vec![b'X'; tokens.iter().map(|t| t.decoded_len()).sum()].into(),
        tokens: tokens.into(),
        representation: Representation::Fixed,
        bits: 0,
        source_type: SourceBlockType::Fixed,
    }
}

fn stored(n: usize) -> PlannedBlock {
    PlannedBlock {
        plain: vec![b'X'; n].into(),
        tokens: Vec::new().into(),
        representation: Representation::Stored,
        bits: 0,
        source_type: SourceBlockType::Stored,
    }
}

fn emit(parent: &[u8], plans: &[PlannedBlock]) -> (Vec<u8>, ParsedStream) {
    let mut writer = BitWriter::default();
    for (i, plan) in plans.iter().enumerate() {
        emit_block(&mut writer, parent, plan, i + 1 == plans.len()).unwrap();
    }
    let data = writer.into_bytes();
    let parsed = parse_stream(&data, 1 << 20).unwrap();
    (data, parsed)
}

fn plain(stream: &ParsedStream) -> Vec<u8> {
    stream
        .blocks
        .iter()
        .flat_map(|b| b.plain.iter().copied())
        .collect()
}

fn assert_certified(source: &ParsedStream, output: &ParsedStream) {
    let cert = certificates(&source.blocks, &mut SearchStop::never()).unwrap();
    let mut at = 0;
    for block in &output.blocks {
        for &token in block.tokens.iter() {
            if distance(token).is_some() {
                assert!(cert.iter().any(|p| p.start <= at
                    && at + token.decoded_len() <= p.end
                    && distance(p.seed) == distance(token)));
            }
            at += token.decoded_len();
        }
    }
    assert_eq!(plain(source), plain(output));
}

/// Reference: charge one edge at a time, to check the batched `Budget::edges`.
fn reference_edge(budget: &mut Budget<'_, '_>) -> Option<()> {
    if budget.remaining == 0 || (budget.remaining & 255 == 0 && budget.stop.reached()) {
        budget.remaining = 0;
        return None;
    }
    budget.remaining -= 1;
    Some(())
}

/// Direct per-length recurrence used to verify choices and budget behavior.
fn reference_restore_interval(
    plain: &[u8],
    current: &[Token],
    seed: Token,
    literal: &[u8],
    distances: &[u8],
    budget: &mut Budget<'_, '_>,
) -> Option<(Vec<Token>, u64)> {
    let n = plain.len();
    if !(3..=MAX_INTERVAL_BYTES).contains(&n) || budget.closed() {
        return None;
    }
    let mut existing = filled(n, None)?;
    let mut at = 0_usize;
    let mut old_cost = 0_u64;
    let mut has_literals = false;
    for &token in current {
        if distance(token).is_some() && distance(token) != distance(seed) {
            return None;
        }
        has_literals |= matches!(token, Token::Literal(_));
        *existing.get_mut(at)? = Some(token);
        at = at.checked_add(token.decoded_len())?;
        old_cost = old_cost.checked_add(token_cost(token, literal, distances)?)?;
    }
    if at != n || !has_literals {
        return None;
    }

    let mut matches = [None; 259];
    for (length, slot) in matches.iter_mut().enumerate().take(n.min(258) + 1).skip(3) {
        let token = submatch(seed, length as u16)?;
        *slot = token_cost(token, literal, distances).map(|cost| (token, cost));
    }
    if matches.iter().all(Option::is_none) {
        return None;
    }
    let mut costs = filled(n + 1, u64::MAX)?;
    let mut choices = filled(n, None)?;
    costs[n] = 0;
    for start in (0..n).rev() {
        let mut consider = |token: Token, cost: u64| -> Option<()> {
            reference_edge(budget)?;
            let end = start + token.decoded_len();
            let total = cost.saturating_add(*costs.get(end)?);
            if total < costs[start] {
                costs[start] = total;
                choices[start] = Some(token);
            }
            Some(())
        };
        if let Some(token) = existing[start] {
            consider(token, token_cost(token, literal, distances)?)?;
        }
        if let Some(cost) = code_cost(literal, usize::from(plain[start])) {
            consider(Token::Literal(plain[start]), cost)?;
        }
        for &(token, cost) in matches
            .iter()
            .take((n - start).min(258) + 1)
            .skip(3)
            .flatten()
        {
            consider(token, cost)?;
        }
    }
    if costs[0] >= old_cost || budget.stop.reached() {
        return None;
    }

    let mut result = Vec::new();
    result.try_reserve_exact(n).ok()?;
    let mut restored = false;
    at = 0;
    while at < n {
        let token = choices[at]?;
        let end = at + token.decoded_len();
        // Same restored-match rule as in `restore_interval`.
        restored |= distance(token).is_some()
            && existing[at..end]
                .iter()
                .any(|t| matches!(t, Some(Token::Literal(_))));
        result.push(token);
        at = end;
    }
    restored.then_some((result, old_cost - costs[0]))
}

fn compare_interval(n: usize, mixed: bool, literal: &[u8], remaining: usize, cutoff: usize) {
    let plain = vec![b'X'; n];
    let seed = matched(n.min(258) as u16, 1);
    let mut current = vec![Token::Literal(b'X'); n];
    if mixed && n > 8 {
        let length = (n / 2).min(258);
        let mut token = matched(length as u16, 1);
        if length == 258 {
            if let Token::Match {
                length_symbol,
                length_extra,
                length_extra_bits,
                ..
            } = &mut token
            {
                *length_symbol = 284;
                *length_extra = 31;
                *length_extra_bits = 5;
            }
        }
        current.splice(1..=length, [token]);
    }
    let results: Vec<_> = [reference_restore_interval, restore_interval]
        .into_iter()
        .map(|solve| {
            let mut polls = 0;
            let mut expired = || {
                polls += 1;
                polls >= cutoff
            };
            let mut stop = SearchStop::callback(&mut expired);
            let mut budget = Budget {
                remaining,
                stop: &mut stop,
            };
            let result = solve(&plain, &current, seed, literal, &[1], &mut budget);
            let left = budget.remaining;
            (result, left, polls)
        })
        .collect();
    assert_eq!(
        results[0], results[1],
        "n={n}, mixed={mixed}, budget={remaining}, cutoff={cutoff}"
    );
}

#[test]
fn restoration_clips_original_proofs_and_never_matches_original_literals() {
    let (_, source) = emit(&[], &[fixed(vec![Token::Literal(b'X'), matched(12, 1)])]);
    let (raw, parent) = emit(
        &[],
        &[
            fixed(vec![Token::Literal(b'X'); 7]),
            fixed(vec![Token::Literal(b'X'); 6]),
        ],
    );
    let plans = plan_original_match_restoration(
        &source.blocks,
        &parent.blocks,
        true,
        &mut SearchStop::never(),
    )
    .unwrap();
    let (_, output) = emit(&raw, &plans);
    assert_certified(&source, &output);
    assert_eq!(
        output.blocks[0].tokens.as_slice(),
        &[Token::Literal(b'X'), matched(6, 1)]
    );
    assert_eq!(output.blocks[1].tokens.as_slice(), &[matched(6, 1)]);
    assert!(output.meaningful_bits < parent.meaningful_bits);
    assert!(plan_original_match_restoration(
        &parent.blocks,
        &parent.blocks,
        true,
        &mut SearchStop::never(),
    )
    .is_none());

    // One- and two-byte pieces cannot form Deflate matches, even when
    // there is a longer source certificate on the other side of the cut.
    let (raw, parent) = emit(
        &[],
        &[
            fixed(vec![Token::Literal(b'X'); 2]),
            fixed(vec![Token::Literal(b'X'); 2]),
            fixed(vec![Token::Literal(b'X'); 9]),
        ],
    );
    let plans = plan_original_match_restoration(
        &source.blocks,
        &parent.blocks,
        true,
        &mut SearchStop::never(),
    )
    .unwrap();
    let (_, output) = emit(&raw, &plans);
    assert_certified(&source, &output);
    assert_eq!(output.blocks[0].tokens, parent.blocks[0].tokens);
    assert_eq!(output.blocks[1].tokens, parent.blocks[1].tokens);
}

#[test]
fn restoration_coalesces_only_adjacent_original_same_distance_matches() {
    for (gap, second_distance) in [(false, 1), (true, 1), (false, 2)] {
        let mut tokens = vec![Token::Literal(b'X'), matched(3, 1)];
        if gap {
            tokens.push(Token::Literal(b'X'));
        }
        tokens.push(matched(3, second_distance));
        let (_, source) = emit(&[], &[fixed(tokens)]);
        let (raw, parent) = emit(
            &[],
            &[fixed(vec![
                Token::Literal(b'X');
                source.decoded_size as usize
            ])],
        );
        let plans = plan_original_match_restoration(
            &source.blocks,
            &parent.blocks,
            true,
            &mut SearchStop::never(),
        )
        .unwrap();
        let (_, output) = emit(&raw, &plans);
        assert_certified(&source, &output);
        if gap {
            assert_eq!(
                output.blocks[0].tokens.as_slice(),
                &[
                    Token::Literal(b'X'),
                    matched(3, 1),
                    Token::Literal(b'X'),
                    matched(3, 1)
                ]
            );
        } else if second_distance != 1 {
            assert_eq!(
                output.blocks[0].tokens.as_slice(),
                &[
                    Token::Literal(b'X'),
                    matched(3, 1),
                    matched(3, second_distance)
                ]
            );
        } else {
            assert_eq!(
                output.blocks[0].tokens.as_slice(),
                &[Token::Literal(b'X'), matched(6, 1)]
            );
        }
    }
}

#[test]
fn restoration_retains_completed_intervals_when_work_expires() {
    let (_, source) = emit(
        &[],
        &[fixed(vec![
            Token::Literal(b'X'),
            matched(3, 1),
            Token::Literal(b'X'),
            matched(258, 1),
        ])],
    );
    let (_, parent) = emit(&[], &[fixed(vec![Token::Literal(b'X'); 263])]);
    let mut stop = SearchStop::never();
    let cert = certificates(&source.blocks, &mut stop).unwrap();
    let mut budget = Budget {
        remaining: 20,
        stop: &mut stop,
    };
    let (tokens, saving) = restore_block(&parent.blocks[0], 0, &cert, &mut budget).unwrap();
    assert_eq!(&tokens[..2], &[Token::Literal(b'X'), matched(3, 1)]);
    assert!(tokens[2..].iter().all(|t| matches!(t, Token::Literal(_))));
    assert!(saving > 0);
    assert_eq!(budget.remaining, 0);
    assert!(plan_original_match_restoration(
        &source.blocks,
        &parent.blocks,
        true,
        &mut SearchStop::always(),
    )
    .is_none());

    let mut probes = 0;
    let mut expired = || {
        probes += 1;
        probes >= 3
    };
    let mut stop = SearchStop::callback(&mut expired);
    let mut budget = Budget {
        remaining: MAX_SEARCH_EDGES,
        stop: &mut stop,
    };
    assert!(restore_interval(
        &vec![b'X'; 258],
        &vec![Token::Literal(b'X'); 258],
        matched(258, 1),
        &FIXED_LITERAL_CODE_LENGTHS,
        &FIXED_DISTANCE_CODE_LENGTHS,
        &mut budget,
    )
    .is_none());
    assert_eq!(budget.remaining, 0);
}

#[test]
fn restoration_respects_absent_codes_distances_and_arena_limit() {
    let literals = vec![Token::Literal(b'X'); 6];
    let mut stop = SearchStop::never();
    let mut budget = Budget {
        remaining: MAX_SEARCH_EDGES,
        stop: &mut stop,
    };
    assert!(restore_interval(
        b"XXXXXX",
        &literals,
        matched(6, 1),
        &FIXED_LITERAL_CODE_LENGTHS,
        &[0; 32],
        &mut budget
    )
    .is_none());
    let mut ll = FIXED_LITERAL_CODE_LENGTHS;
    ll[257..].fill(0);
    assert!(restore_interval(
        b"XXXXXX",
        &literals,
        matched(6, 1),
        &ll,
        &FIXED_DISTANCE_CODE_LENGTHS,
        &mut budget
    )
    .is_none());
    assert!(restore_interval(
        b"XXXXXX",
        &[Token::Literal(b'X'), matched(5, 2)],
        matched(6, 1),
        &FIXED_LITERAL_CODE_LENGTHS,
        &FIXED_DISTANCE_CODE_LENGTHS,
        &mut budget
    )
    .is_none());
    assert!(restore_interval(
        &vec![b'X'; MAX_INTERVAL_BYTES + 1],
        &[],
        matched(258, 1),
        &FIXED_LITERAL_CODE_LENGTHS,
        &FIXED_DISTANCE_CODE_LENGTHS,
        &mut budget
    )
    .is_none());
}

#[test]
fn restoration_shortest_path_matches_exhaustive_spellings() {
    fn enumerate(plain: &[u8], at: usize, prefix: &mut Vec<Token>, all: &mut Vec<Vec<Token>>) {
        if at == plain.len() {
            all.push(prefix.clone());
            return;
        }
        prefix.push(Token::Literal(plain[at]));
        enumerate(plain, at + 1, prefix, all);
        prefix.pop();
        for length in 3..=plain.len() - at {
            prefix.push(matched(length as u16, 1));
            enumerate(plain, at + length, prefix, all);
            prefix.pop();
        }
    }
    let bytes = b"XYXYXYXYX";
    for n in 3..=9 {
        let mut all = Vec::new();
        enumerate(&bytes[..n], 0, &mut Vec::new(), &mut all);
        for profile in 0..64_usize {
            // Abstract prices exercise the oracle, including absent
            // length symbols; independent stream tests use valid trees.
            let mut ll = [0; 286];
            ll[b'X' as usize] = 1 + (profile % 8) as u8;
            ll[b'Y' as usize] = 1 + (profile / 8) as u8;
            for (i, length) in ll[257..264].iter_mut().enumerate() {
                *length = ((profile + i * i) % 9) as u8;
            }
            let dd = [1 + (profile % 5) as u8, 1];
            let cost = |v: &[Token]| {
                v.iter()
                    .try_fold(0_u64, |n, &t| Some(n + token_cost(t, &ll, &dd)?))
            };
            let current: Vec<_> = bytes[..n].iter().copied().map(Token::Literal).collect();
            let mut stop = SearchStop::never();
            let mut budget = Budget {
                remaining: MAX_SEARCH_EDGES,
                stop: &mut stop,
            };
            let restored = restore_interval(
                &bytes[..n],
                &current,
                matched(n as u16, 1),
                &ll,
                &dd,
                &mut budget,
            );
            let selected = restored
                .as_ref()
                .map_or(current.as_slice(), |r| r.0.as_slice());
            assert_eq!(
                cost(selected).unwrap(),
                all.iter().filter_map(|v| cost(v)).min().unwrap()
            );
            if let Some((_, saving)) = restored {
                assert_eq!(saving, cost(&current).unwrap() - cost(selected).unwrap());
            }
        }
    }
}

#[test]
fn restoration_handles_window_extremes_and_stored_alignment() {
    for d in [1, 2, 3, 256, 513, 32768] {
        for length in [3, 258] {
            let (_, source) = emit(
                &[],
                &[
                    stored(d as usize),
                    fixed(vec![matched(length, d)]),
                    stored(3),
                ],
            );
            let (raw, parent) = emit(
                &[],
                &[
                    stored(d as usize),
                    fixed(vec![Token::Literal(b'X'); length as usize]),
                    stored(3),
                ],
            );
            for strict in [false, true] {
                let Some(plans) = plan_original_match_restoration(
                    &source.blocks,
                    &parent.blocks,
                    strict,
                    &mut SearchStop::never(),
                ) else {
                    let literal_cost = token_cost(
                        Token::Literal(b'X'),
                        &FIXED_LITERAL_CODE_LENGTHS,
                        &FIXED_DISTANCE_CODE_LENGTHS,
                    )
                    .unwrap();
                    assert!(
                        token_cost(
                            matched(length, d),
                            &FIXED_LITERAL_CODE_LENGTHS,
                            &FIXED_DISTANCE_CODE_LENGTHS
                        )
                        .unwrap()
                            >= literal_cost * u64::from(length)
                    );
                    continue;
                };
                let (_, output) = emit(&raw, &plans);
                assert_certified(&source, &output);
                assert_eq!(output.max_distance, d);
                assert_eq!(output.meaningful_bits, plans.iter().map(|p| p.bits).sum());
                assert!(output.meaningful_bits <= parent.meaningful_bits);
                if d == 513 && length == 3 {
                    // Stored-block padding absorbs the four saved payload
                    // bits, so the complete streams tie.
                    assert_eq!(output.meaningful_bits, parent.meaningful_bits);
                }
            }
        }
    }
}

#[test]
fn restoration_family_minima_match_direct_choices_and_budgets() {
    let mut seed = 0x93fe_48bd_u32;
    for profile in 0..16 {
        let mut literal = FIXED_LITERAL_CODE_LENGTHS.to_vec();
        for length in &mut literal[257..] {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            *length = match profile {
                0 => *length,
                1 => 0,
                2 => 1,
                3 => u8::MAX,
                _ => (seed % 16) as u8,
            };
        }
        if profile == 1 {
            literal[285] = 1;
        }
        for n in [
            3, 10, 31, 32, 33, 63, 64, 127, 128, 257, 258, 259, 511, 512, 1024, 4096,
        ] {
            for mixed in [false, true] {
                compare_interval(n, mixed, &literal, MAX_SEARCH_EDGES, usize::MAX);
            }
        }
    }
    for remaining in [
        0,
        1,
        2,
        255,
        256,
        257,
        511,
        512,
        513,
        1024,
        4096,
        MAX_SEARCH_EDGES,
    ] {
        for cutoff in [1, 2, 3, 5, usize::MAX] {
            for mixed in [false, true] {
                compare_interval(259, mixed, &FIXED_LITERAL_CODE_LENGTHS, remaining, cutoff);
            }
        }
    }
}

#[test]
fn restoration_batch_budget_matches_individual_edges() {
    for remaining in [0, 1, 255, 256, 257, 511, 512, 513, 1024] {
        for count in 0..=513 {
            for cutoff in [1, 2, 3, usize::MAX] {
                let results: Vec<_> = [false, true]
                    .into_iter()
                    .map(|batched| {
                        let mut polls = 0;
                        let mut expired = || {
                            polls += 1;
                            polls >= cutoff
                        };
                        let mut stop = SearchStop::callback(&mut expired);
                        let mut budget = Budget {
                            remaining,
                            stop: &mut stop,
                        };
                        let result = if batched {
                            budget.edges(count)
                        } else {
                            (0..count).try_for_each(|_| reference_edge(&mut budget))
                        };
                        let left = budget.remaining;
                        (result, left, polls)
                    })
                    .collect();
                assert_eq!(
                    results[0], results[1],
                    "budget={remaining}, edges={count}, cutoff={cutoff}"
                );
            }
        }
    }
}

#[test]
fn restoration_rebuilds_long_certificates_and_preserves_alignment() {
    let mut source_tokens = vec![Token::Literal(b'X')];
    source_tokens.extend(std::iter::repeat(matched(258, 1)).take(15));
    source_tokens.push(matched(225, 1));
    let (_, source) = emit(&[], &[fixed(source_tokens), stored(3)]);
    let (raw, parent) = emit(&[], &[fixed(vec![Token::Literal(b'X'); 4096]), stored(3)]);
    let plans = plan_original_match_restoration(
        &source.blocks,
        &parent.blocks,
        true,
        &mut SearchStop::never(),
    )
    .unwrap();
    let (_, output) = emit(&raw, &plans);
    assert_certified(&source, &output);
    assert_eq!(
        output.meaningful_bits,
        plans.iter().map(|plan| plan.bits).sum()
    );
    assert!(output.meaningful_bits < parent.meaningful_bits);
    assert_eq!(output.blocks[0].tokens[0], Token::Literal(b'X'));
    assert!(output.blocks[0].tokens[1..]
        .iter()
        .all(|token| matches!(token, Token::Match { .. })));
    assert_eq!(output.blocks[1].source_type, SourceBlockType::Stored);
}
