// SPDX-License-Identifier: MIT
// SPDX-FileCopyrightText: 2026 Andrew C.E. Dent <https://github.com/ace-dent>

//! Merge adjacent blocks after the terminal finish (R16).
//!
//! The main search merges blocks only in bounded forms: pairs that decode to
//! at most 512 bytes together, long runs, short source-aligned ranges and
//! boundary-graph edges. The boundary slide (R1c) moves a boundary but never
//! removes one. Once the finish has settled, two neighbors may cost more apart
//! than as one block, which saves a header and its end-of-block code. One
//! left-to-right pass prices each pair merged, at its real bit alignment, and
//! keeps the merged block, which may absorb its next neighbor too, only when
//! it is strictly smaller than the pair as encoded.

use super::super::block::{plan_reusable_block, stored_block_bits};
use super::*;

/// A block of the pass and, once merged, its plan at the bit alignment where
/// it starts, which no later merge changes.
struct Pending {
    block: ParsedBlock,
    plan: Option<(Representation, u64)>,
}

/// An unchanged block's own encoding at `alignment`: its original bits, or a
/// stored block's padding. `None` when a non-stored block cannot keep them.
fn encoded_price(
    block: &ParsedBlock,
    alignment: u8,
    strict: bool,
) -> Option<(Representation, u64)> {
    match reusable_original_bits(block, alignment, strict) {
        Some(original) => Some((Representation::Original(original), original.len)),
        None if block.source_type == SourceBlockType::Stored => Some((
            Representation::Stored,
            stored_block_bits(alignment, block.plain.len()),
        )),
        None => None,
    }
}

fn pending_bits(pending: &Pending, alignment: u8, strict: bool) -> Option<u64> {
    match &pending.plan {
        Some((_, bits)) => Some(*bits),
        None => Some(encoded_price(&pending.block, alignment, strict)?.1),
    }
}

/// The two blocks as one, or `None` when they cannot be modeled together.
fn merge(left: &ParsedBlock, right: &ParsedBlock) -> Option<ParsedBlock> {
    let pair = [left.try_clone_shared()?, right.try_clone_shared()?];
    let composite = Composite::new(&pair)?;
    let end = Cut {
        token: composite.tokens.len(),
        plain: composite.plain.len(),
    };
    make_range(&composite, Cut { token: 0, plain: 0 }, end)
}

fn finish(pending: Pending, alignment: u8, strict: bool) -> Option<PlannedBlock> {
    let (representation, bits) = match pending.plan {
        Some(plan) => plan,
        None => encoded_price(&pending.block, alignment, strict)?,
    };
    Some(PlannedBlock {
        tokens: pending.block.tokens,
        plain: pending.block.plain,
        representation,
        bits,
        source_type: pending.block.source_type,
    })
}

/// R16: merge adjacent non-stored blocks left to right while each merge is
/// strictly smaller than the pair as encoded. Returns the plans of every
/// block when at least one merge was kept; when the stop or the work budget
/// ([`terminal_layout_work`] units of planned token and decoded-byte volume)
/// ends the pass, completed merges are kept and later blocks keep their
/// encoding.
pub(crate) fn plan_terminal_merges(
    blocks: &[ParsedBlock],
    options: &Options,
    stop: &mut SearchStop<'_>,
) -> Option<Vec<PlannedBlock>> {
    // Consecutive merges re-plan a growing block.
    let work = terminal_layout_work(blocks);
    plan_terminal_merges_with_work(blocks, options, work, stop)
}

fn plan_terminal_merges_with_work(
    blocks: &[ParsedBlock],
    options: &Options,
    work: usize,
    stop: &mut SearchStop<'_>,
) -> Option<Vec<PlannedBlock>> {
    // Merged blocks are priced with the bounded table search in both modes,
    // like R6 and R15.
    let bounded = Options {
        exhaustive: false,
        ..options.clone()
    };
    let strict = options.strict;
    let mut plans = Vec::new();
    plans.try_reserve_exact(blocks.len()).ok()?;
    let mut bits = 0_u64;
    let mut work_left = work;
    let mut changed = false;
    let mut current: Option<Pending> = None;
    for block in blocks {
        let next = Pending {
            block: block.try_clone_shared()?,
            plan: None,
        };
        let Some(left) = current.take() else {
            current = Some(next);
            continue;
        };
        let alignment = (bits % 8) as u8;
        let left_bits = pending_bits(&left, alignment, strict)?;
        let pair_work = left
            .block
            .tokens
            .len()
            .checked_add(next.block.tokens.len())?
            .checked_mul(std::mem::size_of::<Token>())?
            .checked_add(left.block.plain.len())?
            .checked_add(next.block.plain.len())?;
        if left.block.source_type != SourceBlockType::Stored
            && next.block.source_type != SourceBlockType::Stored
            && pair_work <= work_left
            && !stop.reached()
        {
            work_left -= pair_work;
            let right_alignment = ((u64::from(alignment) + left_bits) % 8) as u8;
            let right_bits = pending_bits(&next, right_alignment, strict)?;
            if let Some(block) = merge(&left.block, &next.block) {
                let plan = plan_reusable_block(&block, &bounded, stop).at_alignment(
                    block.plain.len(),
                    alignment,
                    reusable_original_bits(&block, alignment, strict),
                );
                if plan.1 < left_bits.checked_add(right_bits)? {
                    changed = true;
                    current = Some(Pending {
                        block,
                        plan: Some(plan),
                    });
                    continue;
                }
            }
        }
        bits = bits.checked_add(left_bits)?;
        plans.push(finish(left, alignment, strict)?);
        current = Some(next);
    }
    if let Some(last) = current {
        plans.push(finish(last, (bits % 8) as u8, strict)?);
    }
    changed.then_some(plans)
}

#[cfg(test)]
mod tests;
