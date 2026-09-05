// SPDX-License-Identifier: MIT

//! Restore original match choices after the selected tokens and trees settle.
//!
//! The original parse is the authority for each interval and distance. A
//! literal rewrite never creates a certificate, and this pass neither searches
//! history nor changes a Huffman tree. All plans refer to the selected parent.

use super::block::{reusable_original_bits, stored_block_bits};
use super::huffman::{FIXED_DISTANCE_CODE_LENGTHS, FIXED_LITERAL_CODE_LENGTHS};
use super::model::{
    canonical_length_encoding, ParsedBlock, PlannedBlock, Representation, SourceBlockType, Token,
};
use super::stop::SearchStop;

// Bound the complete reparse/model walk, not just each individual DP. These
// limits also apply when Max finishes its mandatory Default comparison floor.
pub(crate) const MAX_RESTORATION_BYTES: usize = 128 * 1024;
pub(crate) const MAX_RESTORATION_BLOCKS: usize = 128;
const MAX_BLOCK_TOKENS: usize = 16 * 1024;
// Coalesced same-distance runs can be much longer than a Deflate match. Bound
// the DP to 4 KiB of decoded positions and the stream to 2^20 edge evaluations, so a
// long literalized run cannot multiply the fixed-tree pass's work unboundedly.
const MAX_INTERVAL_BYTES: usize = 4 * 1024;
const MAX_SEARCH_EDGES: usize = 1 << 20;

#[derive(Clone, Copy)]
struct Certificate {
    start: usize,
    end: usize,
    seed: Token,
}

fn distance(token: Token) -> Option<u16> {
    match token {
        Token::Match { distance, .. } => Some(distance),
        Token::Literal(_) => None,
    }
}

fn certificates(blocks: &[ParsedBlock], stop: &mut SearchStop<'_>) -> Option<Vec<Certificate>> {
    let mut result: Vec<Certificate> = Vec::new();
    let mut offset = 0_usize;
    for block in blocks {
        if stop.reached() {
            return None;
        }
        let block_end = offset.checked_add(block.plain.len())?;
        if block.source_type == SourceBlockType::Stored {
            offset = block_end;
            continue;
        }
        for (index, &token) in block.tokens.iter().enumerate() {
            if index & 255 == 0 && stop.reached() {
                return None;
            }
            let end = offset.checked_add(token.decoded_len())?;
            if distance(token).is_some() {
                if let Some(last) = result
                    .last_mut()
                    .filter(|last| last.end == offset && distance(last.seed) == distance(token))
                {
                    last.end = end;
                } else {
                    result.try_reserve(1).ok()?;
                    result.push(Certificate {
                        start: offset,
                        end,
                        seed: token,
                    });
                }
            }
            offset = end;
        }
        if offset != block_end {
            return None;
        }
    }
    Some(result)
}

struct Budget<'a, 'b> {
    remaining: usize,
    stop: &'a mut SearchStop<'b>,
}

impl Budget<'_, '_> {
    fn edge(&mut self) -> Option<()> {
        if self.remaining == 0 || (self.remaining & 255 == 0 && self.stop.reached()) {
            self.remaining = 0;
            return None;
        }
        self.remaining -= 1;
        Some(())
    }

    fn closed(&mut self) -> bool {
        self.remaining == 0 || self.stop.reached()
    }
}

fn filled<T: Clone>(len: usize, value: T) -> Option<Vec<T>> {
    let mut values = Vec::new();
    values.try_reserve_exact(len).ok()?;
    values.resize(len, value);
    Some(values)
}

fn code_cost(lengths: &[u8], symbol: usize) -> Option<u64> {
    lengths
        .get(symbol)
        .copied()
        .filter(|&n| n != 0)
        .map(u64::from)
}

fn token_cost(token: Token, literal: &[u8], distances: &[u8]) -> Option<u64> {
    match token {
        Token::Literal(value) => code_cost(literal, usize::from(value)),
        Token::Match {
            length_symbol,
            distance_symbol,
            length_extra_bits,
            distance_extra_bits,
            ..
        } => Some(
            code_cost(literal, usize::from(length_symbol))?
                + code_cost(distances, usize::from(distance_symbol))?
                + u64::from(length_extra_bits)
                + u64::from(distance_extra_bits),
        ),
    }
}

fn submatch(seed: Token, length: u16) -> Option<Token> {
    let Token::Match {
        distance,
        distance_symbol,
        distance_extra,
        distance_extra_bits,
        ..
    } = seed
    else {
        return None;
    };
    let (length_symbol, length_extra, length_extra_bits) = canonical_length_encoding(length)?;
    Some(Token::Match {
        length,
        distance,
        length_symbol,
        length_extra,
        length_extra_bits,
        distance_symbol,
        distance_extra,
        distance_extra_bits,
    })
}

/// Exact shortest path inside one clipped certificate under fixed code prices.
/// The current edge is considered first so payload ties retain its spelling.
fn restore_interval(
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
            budget.edge()?;
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
        // A newly chosen match is unavailable from the selected proofs iff it
        // crosses a literal gap. Continuous same-distance matches were already
        // coalescible without the original certificate.
        restored |= distance(token).is_some()
            && existing[at..end]
                .iter()
                .any(|t| matches!(t, Some(Token::Literal(_))));
        result.push(token);
        at = end;
    }
    restored.then_some((result, old_cost - costs[0]))
}

fn restore_block(
    block: &ParsedBlock,
    offset: usize,
    certificates: &[Certificate],
    budget: &mut Budget<'_, '_>,
) -> Option<(Vec<Token>, u64)> {
    if block.tokens.len() > MAX_BLOCK_TOKENS || block.source_type == SourceBlockType::Stored {
        return None;
    }
    let (literal, distances): (&[u8], &[u8]) = match &block.original_dynamic {
        Some(dynamic) => (&dynamic.literal_lengths, &dynamic.distance_lengths),
        None if block.source_type == SourceBlockType::Fixed => {
            (&FIXED_LITERAL_CODE_LENGTHS, &FIXED_DISTANCE_CODE_LENGTHS)
        }
        None => return None,
    };
    let mut bounds = Vec::new();
    bounds.try_reserve_exact(block.tokens.len() + 1).ok()?;
    bounds.push(offset);
    for token in block.tokens.iter() {
        bounds.push(bounds.last()?.checked_add(token.decoded_len())?);
    }
    let block_end = offset.checked_add(block.plain.len())?;
    if bounds.last().copied()? != block_end {
        return None;
    }
    let mut result = Vec::new();
    let mut copied = 0;
    let mut saving = 0_u64;
    let first = certificates.partition_point(|cert| cert.end <= offset);
    for cert in &certificates[first..] {
        if cert.start >= block_end || budget.closed() {
            break;
        }
        let start = cert.start.max(offset);
        let end = cert.end.min(block_end);
        if !(3..=MAX_INTERVAL_BYTES).contains(&(end - start)) {
            continue;
        }
        let (Ok(a), Ok(b)) = (bounds.binary_search(&start), bounds.binary_search(&end)) else {
            continue;
        };
        if !block.tokens[a..b]
            .iter()
            .any(|t| matches!(t, Token::Literal(_)))
        {
            continue;
        }
        let Some((replacement, bits)) = restore_interval(
            &block.plain[start - offset..end - offset],
            &block.tokens[a..b],
            cert.seed,
            literal,
            distances,
            budget,
        ) else {
            continue;
        };
        result.try_reserve(a - copied + replacement.len()).ok()?;
        result.extend_from_slice(&block.tokens[copied..a]);
        result.extend(replacement);
        copied = b;
        saving = saving.checked_add(bits)?;
    }
    if saving == 0 {
        return None;
    }
    result.try_reserve(block.tokens.len() - copied).ok()?;
    result.extend_from_slice(&block.tokens[copied..]);
    Some((result, saving))
}

/// Build an additive, fixed-tree candidate using only original match proofs.
/// Both block lists must be validated encodings of the same decoded stream.
/// On interruption, completed interval repairs survive and the remaining
/// blocks retain their parent encoding. The caller must compare the complete
/// emission because later stored padding can absorb a local payload saving.
pub(crate) fn plan_original_match_restoration(
    original: &[ParsedBlock],
    selected: &[ParsedBlock],
    strict: bool,
    stop: &mut SearchStop<'_>,
) -> Option<Vec<PlannedBlock>> {
    if original.len() > MAX_RESTORATION_BLOCKS || selected.len() > MAX_RESTORATION_BLOCKS {
        return None;
    }
    for blocks in [original, selected] {
        let bytes = blocks
            .iter()
            .try_fold(0_usize, |n, b| n.checked_add(b.plain.len()))?;
        if bytes > MAX_RESTORATION_BYTES {
            return None;
        }
    }
    let certificates = certificates(original, stop)?;
    if certificates.is_empty() {
        return None;
    }
    let mut budget = Budget {
        remaining: MAX_SEARCH_EDGES,
        stop,
    };
    let mut plans = Vec::new();
    plans.try_reserve_exact(selected.len()).ok()?;
    let mut offset = 0_usize;
    let mut alignment = 0_u8;
    let mut changed = false;
    for block in selected {
        let mut plan = if let Some(original) = reusable_original_bits(block, alignment, strict) {
            PlannedBlock {
                tokens: block.tokens.clone(),
                plain: block.plain.clone(),
                representation: Representation::Original(original),
                bits: original.len,
                source_type: block.source_type,
            }
        } else if block.source_type == SourceBlockType::Stored {
            PlannedBlock {
                tokens: block.tokens.clone(),
                plain: block.plain.clone(),
                representation: Representation::Stored,
                bits: stored_block_bits(alignment, block.plain.len()),
                source_type: block.source_type,
            }
        } else {
            return None;
        };
        if !budget.closed() {
            if let Some((tokens, saving)) = restore_block(block, offset, &certificates, &mut budget)
            {
                plan.bits = plan.bits.checked_sub(saving)?;
                plan.representation = if let Some(dynamic) = &block.original_dynamic {
                    let mut dynamic = dynamic.try_clone()?;
                    dynamic.bits = plan.bits;
                    Representation::Dynamic(dynamic)
                } else {
                    Representation::Fixed
                };
                plan.tokens = tokens.into();
                changed = true;
            }
        }
        offset += block.plain.len();
        alignment = ((u64::from(alignment) + plan.bits) & 7) as u8;
        plans.push(plan);
    }
    changed.then_some(plans)
}

#[cfg(test)]
mod tests {
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
}
