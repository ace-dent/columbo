// SPDX-License-Identifier: MIT

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
                    // Four saved payload bits are absorbed by stored
                    // padding; the caller must retain the complete tie.
                    assert_eq!(output.meaningful_bits, parent.meaningful_bits);
                }
            }
        }
    }
}
