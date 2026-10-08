// SPDX-License-Identifier: MIT
// SPDX-FileCopyrightText: 2026 Andrew C.E. Dent <https://github.com/ace-dent>

//! Slide block boundaries under the transmitted Huffman trees.
//!
//! A boundary changes neither the decoded bytes nor the shared history window,
//! so a token may join its neighbouring block whenever that block's trees can
//! code it. Holding both trees fixed leaves every header unchanged: a cheaper
//! cut saves payload bits outright. Boundary searches elsewhere plan fresh
//! trees for a few sampled cuts; this pass prices every token cut against the
//! trees actually selected, so it finds the shifts those samples pass over.
//! A joining match the neighbour cannot code, or codes expensively, may be
//! spelled as its decoded literals instead; equal bytes need no match proof.
//! Each moved block is then re-planned with fresh trees, keeping whichever of
//! its transmitted and re-planned codes is cheaper. New trees can make further
//! cuts profitable, so the slide repeats under them until no block moves.

use std::sync::Arc;

use crate::Options;

use super::block::{plan_block, reusable_original_bits, stored_block_bits};
use super::huffman::{FIXED_DISTANCE_CODE_LENGTHS, FIXED_LITERAL_CODE_LENGTHS};
use super::model::{
    count_frequencies, DynamicPlan, ParsedBlock, PlannedBlock, Representation, SourceBlockType,
    Token,
};
use super::restore::token_cost;
use super::stop::SearchStop;

/// Every accepted move strictly lowers the payload, so sweeps terminate; the
/// cap only bounds work on a long chain of interacting boundaries.
const MAX_SWEEPS: usize = 16;
/// Every round after the first follows a strictly cheaper re-plan, so rounds
/// terminate; the cap only bounds work.
const MAX_ROUNDS: usize = 16;
/// Poll the stop between boundaries and within long block pairs.
const STOP_POLL_TOKENS: usize = 1 << 16;

type Trees<'a> = (&'a [u8], &'a [u8]);

/// How a block is coded while the slide runs.
enum Code<'a> {
    /// Stored, or re-planned as stored: its boundaries stay fixed.
    Stored,
    /// The parsed block's own trees, dynamic or fixed.
    Transmitted(Trees<'a>),
    /// The fixed trees, chosen by re-planning a moved block.
    Fixed,
    /// A dynamic header chosen by re-planning a moved block.
    Dynamic(DynamicPlan),
}

impl Code<'_> {
    fn trees(&self) -> Option<Trees<'_>> {
        match self {
            Self::Stored => None,
            Self::Transmitted(trees) => Some(*trees),
            Self::Fixed => Some((
                &FIXED_LITERAL_CODE_LENGTHS[..],
                &FIXED_DISTANCE_CODE_LENGTHS[..],
            )),
            Self::Dynamic(dynamic) => Some((&dynamic.literal_lengths, &dynamic.distance_lengths)),
        }
    }
}

struct Slot<'a> {
    tokens: Arc<Vec<Token>>,
    plain: Arc<Vec<u8>>,
    code: Code<'a>,
    payload: u64,
    /// The block's bits other than its payload under `code`: block header,
    /// trees and end-of-block code.
    overhead: u64,
    /// Whether a sweep moved the block since it was last planned.
    moved: bool,
}

impl Slot<'_> {
    /// The block coded as it stands, at `alignment`.
    fn planned(&self, block: &ParsedBlock, alignment: u8) -> Option<(Representation, u64)> {
        if matches!(self.code, Code::Stored) {
            let bits = stored_block_bits(alignment, self.plain.len());
            return Some((Representation::Stored, bits));
        }
        let bits = self.overhead.checked_add(self.payload)?;
        let dynamic = match &self.code {
            Code::Transmitted(_) => block.original_dynamic.as_ref(),
            Code::Dynamic(dynamic) => Some(dynamic),
            Code::Stored | Code::Fixed => None,
        };
        let representation = match dynamic {
            Some(dynamic) => {
                let mut dynamic = dynamic.try_clone()?;
                dynamic.bits = bits;
                Representation::Dynamic(dynamic)
            }
            None => Representation::Fixed,
        };
        Some((representation, bits))
    }

    /// Code the block from now on as `replanned` chose.
    fn adopt(&mut self, replanned: &PlannedBlock) -> Option<()> {
        self.code = match &replanned.representation {
            Representation::Stored => Code::Stored,
            Representation::Fixed => Code::Fixed,
            Representation::Dynamic(dynamic) => Code::Dynamic(dynamic.try_clone()?),
            Representation::Original(_) => return None,
        };
        if let Some(trees) = self.code.trees() {
            self.payload = payload_bits(&self.tokens, trees)?;
            self.overhead = replanned.bits.checked_sub(self.payload)?;
        }
        Some(())
    }
}

fn payload_bits(tokens: &[Token], (literal, distances): Trees<'_>) -> Option<u64> {
    tokens.iter().try_fold(0_u64, |bits, &token| {
        bits.checked_add(token_cost(token, literal, distances)?)
    })
}

/// A token's payload bits in one block, and whether it is spelled as literals.
///
/// A token keeps its own spelling in its own block. A joining match takes the
/// cheaper of its own spelling and its decoded bytes as literals, when the
/// block's trees code either; an equal price keeps the match.
fn price(token: Token, bytes: &[u8], trees: Trees<'_>, joining: bool) -> Option<(u64, bool)> {
    let own = token_cost(token, trees.0, trees.1);
    if !joining || matches!(token, Token::Literal(_)) {
        return own.map(|bits| (bits, false));
    }
    let literals = bytes.iter().try_fold(0_u64, |bits, &byte| {
        bits.checked_add(token_cost(Token::Literal(byte), trees.0, trees.1)?)
    });
    match (own, literals) {
        (Some(own), Some(literals)) if literals < own => Some((literals, true)),
        (Some(own), _) => Some((own, false)),
        (None, literals) => literals.map(|bits| (bits, true)),
    }
}

/// Two adjacent Huffman blocks with the trees each transmits.
struct Pair<'a> {
    left: &'a [Token],
    left_plain: &'a [u8],
    right: &'a [Token],
    right_plain: &'a [u8],
    left_trees: Trees<'a>,
    right_trees: Trees<'a>,
}

impl<'a> Pair<'a> {
    /// Every token in order with its decoded bytes and whether it starts in
    /// the left block. An item is `None` if a token overruns its block.
    fn walk(&self) -> impl Iterator<Item = Option<(Token, &'a [u8], bool)>> + 'a {
        let side = |tokens: &'a [Token], plain: &'a [u8], from_left: bool| {
            tokens.iter().scan(0_usize, move |at, &token| {
                let start = *at;
                *at = start.saturating_add(token.decoded_len());
                Some(plain.get(start..*at).map(|bytes| (token, bytes, from_left)))
            })
        };
        side(self.left, self.left_plain, true).chain(side(self.right, self.right_plain, false))
    }

    fn left_price(&self, token: Token, bytes: &[u8], from_left: bool) -> Option<(u64, bool)> {
        price(token, bytes, self.left_trees, !from_left)
    }

    fn right_price(&self, token: Token, bytes: &[u8], from_left: bool) -> Option<(u64, bool)> {
        price(token, bytes, self.right_trees, from_left)
    }

    /// Rebuild both token lists for `cut`, spelling each joining token as its
    /// price chose.
    fn split(&self, cut: usize) -> Option<(Vec<Token>, Vec<Token>)> {
        let mut first = Vec::new();
        let mut second = Vec::new();
        for (index, item) in self.walk().enumerate() {
            let (token, bytes, from_left) = item?;
            let (side, (_, literals)) = if index < cut {
                (&mut first, self.left_price(token, bytes, from_left)?)
            } else {
                (&mut second, self.right_price(token, bytes, from_left)?)
            };
            if literals {
                side.try_reserve(bytes.len()).ok()?;
                side.extend(bytes.iter().copied().map(Token::Literal));
            } else {
                side.try_reserve(1).ok()?;
                side.push(token);
            }
        }
        Some((first, second))
    }
}

/// Find the cheapest cut of two adjacent blocks under their fixed trees.
///
/// A feasible cut leaves both blocks nonempty and every token on a side that
/// can price it. Returns the new left token count and both payloads when that
/// cut is strictly cheaper; ties keep the cut nearest the current one.
fn cheaper_cut(pair: &Pair<'_>, stop: &mut SearchStop<'_>) -> Option<(usize, u64, u64)> {
    let current = pair.left.len();
    // The current cut must lie inside the scanned range to be compared.
    if current == 0 || pair.right.is_empty() {
        return None;
    }
    let n = current.checked_add(pair.right.len())?;
    let (mut lo, mut hi) = (1, n - 1);
    for (index, item) in pair.walk().enumerate() {
        let (token, bytes, from_left) = item?;
        if from_left {
            if pair.right_price(token, bytes, true).is_none() {
                lo = index + 1;
            }
        } else if pair.left_price(token, bytes, false).is_none() {
            hi = hi.min(index);
            break;
        }
    }
    if lo >= hi {
        return None;
    }

    let (mut prefix, mut suffix) = (0_u64, 0_u64);
    for (index, item) in pair.walk().enumerate() {
        let (token, bytes, from_left) = item?;
        if index < lo {
            prefix = prefix.checked_add(pair.left_price(token, bytes, from_left)?.0)?;
        } else {
            suffix = suffix.checked_add(pair.right_price(token, bytes, from_left)?.0)?;
        }
    }
    let mut walk = pair.walk().skip(lo);
    let mut best: Option<(u64, usize, u64, u64)> = None;
    for cut in lo..=hi {
        if cut % STOP_POLL_TOKENS == 0 && stop.reached() {
            return None;
        }
        let total = prefix.checked_add(suffix)?;
        let better = best.map_or(true, |(bits, best_cut, _, _)| {
            (total, cut.abs_diff(current)) < (bits, best_cut.abs_diff(current))
        });
        if better {
            best = Some((total, cut, prefix, suffix));
        }
        if cut < hi {
            let (token, bytes, from_left) = walk.next()??;
            prefix = prefix.checked_add(pair.left_price(token, bytes, from_left)?.0)?;
            suffix = suffix.checked_sub(pair.right_price(token, bytes, from_left)?.0)?;
        }
    }
    let (_, cut, left_bits, right_bits) = best?;
    (cut != current).then_some((cut, left_bits, right_bits))
}

/// Plan a moved block's new tokens as Columbo plans any block, with fresh
/// trees and header.
fn replan(
    block: &ParsedBlock,
    slot: &Slot<'_>,
    alignment: u8,
    options: &Options,
    stop: &mut SearchStop<'_>,
) -> Option<PlannedBlock> {
    let (literal_frequencies, distance_frequencies) = count_frequencies(&slot.tokens);
    let moved = ParsedBlock {
        tokens: Arc::clone(&slot.tokens),
        plain: Arc::clone(&slot.plain),
        literal_frequencies,
        distance_frequencies,
        original_literal_lengths: None,
        original_distance_lengths: None,
        original_dynamic: None,
        original: None,
        source_splits: Vec::new(),
        source_type: block.source_type,
    };
    (!stop.reached()).then(|| plan_block(&moved, alignment, options, stop))
}

/// Split the concatenation of two slices at `cut` without infallible growth.
fn split_pair<T: Copy>(left: &[T], right: &[T], cut: usize) -> Option<(Vec<T>, Vec<T>)> {
    let total = left.len().checked_add(right.len())?;
    let mut first = Vec::new();
    let mut second = Vec::new();
    first.try_reserve_exact(cut).ok()?;
    second.try_reserve_exact(total.checked_sub(cut)?).ok()?;
    if cut <= left.len() {
        first.extend_from_slice(&left[..cut]);
        second.extend_from_slice(&left[cut..]);
        second.extend_from_slice(right);
    } else {
        first.extend_from_slice(left);
        first.extend_from_slice(right.get(..cut - left.len())?);
        second.extend_from_slice(&right[cut - left.len()..]);
    }
    Some((first, second))
}

/// Move Huffman block boundaries to cheaper token cuts under fixed trees,
/// re-plan each moved block, and repeat under the new trees until no block
/// moves.
///
/// Stored blocks and their boundaries stay in place. Every Huffman block keeps
/// at least one token, so the parse that follows retains the block layout
/// later terminal methods require. The caller must compare the complete
/// emission because stored padding can absorb a saving.
pub(crate) fn plan_boundary_slide(
    blocks: &[ParsedBlock],
    options: &Options,
    stop: &mut SearchStop<'_>,
) -> Option<Vec<PlannedBlock>> {
    plan_slide(blocks, options, true, MAX_ROUNDS, stop)
}

/// The slide itself. Without `refit`, every moved block keeps its transmitted
/// trees, so the result is exactly the fixed-tree optimum the tests check and
/// one round reaches it.
fn plan_slide(
    blocks: &[ParsedBlock],
    options: &Options,
    refit: bool,
    max_rounds: usize,
    stop: &mut SearchStop<'_>,
) -> Option<Vec<PlannedBlock>> {
    let strict = options.strict;
    if blocks.len() < 2 {
        return None;
    }
    let mut slots = Vec::new();
    slots.try_reserve_exact(blocks.len()).ok()?;
    for block in blocks {
        let code = match block.source_type {
            SourceBlockType::Stored => Code::Stored,
            // Fixed trees cannot change, so the strict policy is decided here.
            _ if reusable_original_bits(block, 0, strict).is_none() => return None,
            SourceBlockType::Fixed => Code::Transmitted((
                &FIXED_LITERAL_CODE_LENGTHS[..],
                &FIXED_DISTANCE_CODE_LENGTHS[..],
            )),
            SourceBlockType::Dynamic => {
                let dynamic = block.original_dynamic.as_ref()?;
                Code::Transmitted((&dynamic.literal_lengths[..], &dynamic.distance_lengths[..]))
            }
        };
        let (payload, overhead) = match code.trees() {
            Some(trees) => {
                let payload = payload_bits(&block.tokens, trees)?;
                let original = reusable_original_bits(block, 0, strict)?;
                (payload, original.len.checked_sub(payload)?)
            }
            None => (0, 0),
        };
        slots.push(Slot {
            tokens: Arc::clone(&block.tokens),
            plain: Arc::clone(&block.plain),
            code,
            payload,
            overhead,
            moved: false,
        });
    }

    // Each round fits the boundaries to the current trees, then re-plans the
    // blocks it moved. A strictly cheaper re-plan changes some trees, which
    // can make further cuts profitable; otherwise the cuts are already
    // optimal for the trees that stay.
    let mut plans = None;
    for _ in 0..max_rounds {
        if !sweep(&mut slots, stop)? {
            break;
        }
        let (round, refitted) = assemble(blocks, &mut slots, options, refit, stop)?;
        plans = Some(round);
        if !refitted {
            break;
        }
    }
    plans
}

/// Sweep every boundary between two Huffman blocks, repeating while a sweep
/// moves one, at most `MAX_SWEEPS` times. Returns whether any block moved.
fn sweep(slots: &mut [Slot<'_>], stop: &mut SearchStop<'_>) -> Option<bool> {
    let mut changed = false;
    'sweeps: for _ in 0..MAX_SWEEPS {
        let mut moved = false;
        for k in 0..slots.len() - 1 {
            if stop.reached() {
                break 'sweeps;
            }
            let (Some(left_trees), Some(right_trees)) =
                (slots[k].code.trees(), slots[k + 1].code.trees())
            else {
                continue;
            };
            let pair = Pair {
                left: &slots[k].tokens,
                left_plain: &slots[k].plain,
                right: &slots[k + 1].tokens,
                right_plain: &slots[k + 1].plain,
                left_trees,
                right_trees,
            };
            let Some((cut, left_bits, right_bits)) = cheaper_cut(&pair, stop) else {
                continue;
            };
            let (left_tokens, right_tokens) = pair.split(cut)?;
            let left_plain_len = left_tokens.iter().try_fold(0_usize, |total, token| {
                total.checked_add(token.decoded_len())
            })?;
            let (left_plain, right_plain) =
                split_pair(pair.left_plain, pair.right_plain, left_plain_len)?;
            for (slot, tokens, plain, payload) in [
                (k, left_tokens, left_plain, left_bits),
                (k + 1, right_tokens, right_plain, right_bits),
            ] {
                let slot = &mut slots[slot];
                slot.tokens = Arc::new(tokens);
                slot.plain = Arc::new(plain);
                slot.payload = payload;
                slot.moved = true;
            }
            moved = true;
            changed = true;
        }
        if !moved {
            break;
        }
    }
    Some(changed)
}

/// Plan every block at its actual alignment. Unchanged blocks keep their
/// original bits; moved blocks keep their code with an adjusted bit count.
/// With `refit`, each block moved since it was last planned is re-planned,
/// and a strictly cheaper plan becomes its code; a tie keeps the code. Also
/// returns whether any block took a re-planned code.
fn assemble(
    blocks: &[ParsedBlock],
    slots: &mut [Slot<'_>],
    options: &Options,
    refit: bool,
    stop: &mut SearchStop<'_>,
) -> Option<(Vec<PlannedBlock>, bool)> {
    let strict = options.strict;
    let mut plans = Vec::new();
    plans.try_reserve_exact(blocks.len()).ok()?;
    let mut alignment = 0_u8;
    let mut refitted = false;
    for (block, slot) in blocks.iter().zip(slots.iter_mut()) {
        let original = reusable_original_bits(block, alignment, strict);
        let (representation, bits) = if block.source_type == SourceBlockType::Stored {
            // Earlier savings can shift a stored block's padding.
            match original {
                Some(original) => (Representation::Original(original), original.len),
                None => (
                    Representation::Stored,
                    stored_block_bits(alignment, block.plain.len()),
                ),
            }
        } else if Arc::ptr_eq(&slot.tokens, &block.tokens) {
            let original = original?;
            (Representation::Original(original), original.len)
        } else {
            let current = slot.planned(block, alignment)?;
            // The block's trees were fitted to its old contents. Re-plan it
            // and keep the cheaper; a tie keeps its trees.
            let moved = std::mem::take(&mut slot.moved);
            match (refit && moved).then(|| replan(block, slot, alignment, options, stop)) {
                Some(Some(replanned)) if replanned.bits < current.1 => {
                    slot.adopt(&replanned)?;
                    refitted = true;
                    (replanned.representation, replanned.bits)
                }
                _ => current,
            }
        };
        alignment = ((u64::from(alignment) + bits) & 7) as u8;
        plans.push(PlannedBlock {
            tokens: Arc::clone(&slot.tokens),
            plain: Arc::clone(&slot.plain),
            representation,
            bits,
            source_type: block.source_type,
        });
    }
    Some((plans, refitted))
}

#[cfg(test)]
mod tests;
