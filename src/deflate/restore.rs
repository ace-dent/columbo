// SPDX-License-Identifier: MIT
// SPDX-FileCopyrightText: 2026 Andrew C.E. Dent <https://github.com/ace-dent>

//! Restore source matches (R1) after the selected tokens and Huffman codes
//! settle.
//!
//! Only source matches prove which intervals repeat at which distance; the
//! pass neither searches history nor changes a Huffman code.

use super::block::{reusable_original_bits, stored_block_bits};
use super::huffman::{FIXED_DISTANCE_CODE_LENGTHS, FIXED_LITERAL_CODE_LENGTHS};
use super::model::{
    canonical_length_encoding, ParsedBlock, PlannedBlock, Representation, SourceBlockType, Token,
    LENGTH_BASE, LENGTH_EXTRA_BITS,
};
use super::stop::SearchStop;

/// Whole-pass size limits, also applied when Max completes its Default
/// comparison floor.
pub(crate) const MAX_RESTORATION_BYTES: usize = 128 * 1024;
pub(crate) const MAX_RESTORATION_BLOCKS: usize = 128;
const MAX_BLOCK_TOKENS: usize = 16 * 1024;
/// Merged same-distance runs can be much longer than one match, so each
/// interval search and the whole pass are bounded.
const MAX_INTERVAL_BYTES: usize = 4 * 1024;
const MAX_SEARCH_EDGES: usize = 1 << 20;

/// Decoded interval covered by consecutive source matches at one distance,
/// which proves that its bytes repeat at that distance. `seed` is the first
/// match.
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
    /// Charge `count` edges, checking the stop every 256; `None` when the
    /// budget or time runs out.
    fn edges(&mut self, mut count: usize) -> Option<()> {
        if count <= (self.remaining & 255) {
            self.remaining -= count;
            return Some(());
        }
        while count != 0 {
            if self.remaining == 0 || (self.remaining & 255 == 0 && self.stop.reached()) {
                self.remaining = 0;
                return None;
            }
            let until_probe = match self.remaining & 255 {
                0 => 256,
                n => n,
            };
            let spent = count.min(until_probe);
            self.remaining -= spent;
            count -= spent;
        }
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

pub(super) fn token_cost(token: Token, literal: &[u8], distances: &[u8]) -> Option<u64> {
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

/// The lengths of one length symbol, with a sliding minimum (at most 32
/// positions wide) of the suffix costs they reach.
#[derive(Clone, Copy, Default)]
struct MatchFamily {
    first: usize,
    last: usize,
    cost: u64,
    positions: [u16; 32],
    head: usize,
    count: usize,
}

impl MatchFamily {
    /// Move one position backward and return the cheapest end position. New
    /// positions win cost ties, so the shortest match wins.
    fn minimum(&mut self, start: usize, costs: &[u64]) -> usize {
        let first = start + self.first;
        if self.first == self.last {
            return first;
        }
        let last = start + self.last;
        while self.count != 0 && usize::from(self.positions[self.head]) > last {
            self.head = (self.head + 1) % self.positions.len();
            self.count -= 1;
        }
        while self.count != 0 {
            let back = (self.head + self.count - 1) % self.positions.len();
            if costs[usize::from(self.positions[back])] < costs[first] {
                break;
            }
            self.count -= 1;
        }
        let next = (self.head + self.count) % self.positions.len();
        self.positions[next] = first as u16;
        self.count += 1;
        usize::from(self.positions[self.head])
    }
}

/// Cheapest tokenization of one certificate interval, clipped to the block,
/// with the block's code lengths fixed. The current token is tried first so
/// it wins cost ties.
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

    let distance_bits = match seed {
        Token::Match {
            distance_symbol,
            distance_extra_bits,
            ..
        } => code_cost(distances, usize::from(distance_symbol))? + u64::from(distance_extra_bits),
        Token::Literal(_) => return None,
    };
    let mut families = [MatchFamily::default(); LENGTH_BASE.len()];
    let mut family_count = 0;
    for (i, &base) in LENGTH_BASE.iter().enumerate() {
        let first = usize::from(base);
        if first > n {
            break;
        }
        let Some(code_bits) = code_cost(literal, 257 + i) else {
            continue;
        };
        let family = &mut families[family_count];
        family.first = first;
        family.last = LENGTH_BASE
            .get(i + 1)
            .map_or(258, |&next| usize::from(next) - 1);
        family.cost = distance_bits + code_bits + u64::from(LENGTH_EXTRA_BITS[i]);
        family_count += 1;
    }
    if family_count == 0 {
        return None;
    }
    let mut costs = filled(n + 1, u64::MAX)?;
    let mut choices = filled(n, None)?;
    costs[n] = 0;
    for start in (0..n).rev() {
        let mut consider = |token: Token, cost: u64| -> Option<()> {
            budget.edges(1)?;
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
        let mut match_length = None;
        for family in &mut families[..family_count] {
            if family.first > n - start {
                break;
            }
            // Charge every length of this symbol, although only the minimum
            // is evaluated. Running out discards the interval.
            budget.edges(family.last.min(n - start) - family.first + 1)?;
            let end = family.minimum(start, &costs);
            let total = family.cost.saturating_add(costs[end]);
            if total < costs[start] {
                costs[start] = total;
                match_length = Some((end - start) as u16);
            }
        }
        if let Some(length) = match_length {
            choices[start] = Some(submatch(seed, length)?);
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
        // A new match counts as restored only if it covers a selected
        // literal; merging contiguous same-distance matches needs no
        // certificate.
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

/// Build an extra candidate from `selected` that restores matches proven by
/// `original`, keeping every Huffman code fixed.
///
/// Both block lists must encode the same decoded data. If stopped, finished
/// rewrites are kept. The caller must compare the complete emitted stream,
/// because later stored-block padding can absorb a local saving.
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
mod tests;
