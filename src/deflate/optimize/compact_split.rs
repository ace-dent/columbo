// SPDX-License-Identifier: MIT
// SPDX-FileCopyrightText: 2026 Andrew C.E. Dent <https://github.com/ace-dent>

//! The compact split floor (M10c) and the deferred source-max split (M10d).
//! Each reparses a compact complete parent and cuts its blocks without token
//! search, merge search or replay, so the split can finish after the deadline.

use crate::sort::SortShared;
use crate::{Error, Options, Result};

use super::{
    build_candidate_from_plans, parse_validated_rewrite, replace_optional_keeping_parent,
    Candidate, CandidateInput, ReplayPlanner, StreamIdentity, COMPACT_SPLIT_FLOOR_MAX_BLOCKS,
    COMPACT_SPLIT_FLOOR_MAX_COMPRESSED, COMPACT_SPLIT_FLOOR_MAX_DECODED,
    COMPACT_SPLIT_FLOOR_MAX_TOKENS,
};
use crate::deflate::model::{
    ParsedBlock, ParsedStream, PlannedBlock, Representation, SourceBlockType,
};
use crate::deflate::stop::{Deadline, SearchStop};
use crate::deflate::stream::{
    plan_compact_source_split_floor, plan_compact_source_split_floor_until,
};

pub(super) const TERMINAL_SOURCE_SPLIT_MAX_DECODED: u64 = 256 * 1024;

/// A compact completed parent, reparsed for the compact split floor (M10c).
/// The split runs no token search, merge search or replay, so its limits make
/// it safe to finish after the deadline.
pub(super) struct CompactSplitSeed {
    pub(super) data: Vec<u8>,
    pub(super) bits: u64,
    pub(super) stream: ParsedStream,
}

pub(super) fn prepare_compact_source_split_seed(
    candidate: &Candidate,
    decoded_limit: u64,
    identity: StreamIdentity,
) -> Result<Option<CompactSplitSeed>> {
    prepare_compact_source_split_seed_with_limits(
        candidate,
        decoded_limit,
        identity,
        2,
        COMPACT_SPLIT_FLOOR_MAX_DECODED,
    )
}

fn prepare_compact_source_split_seed_with_limits(
    candidate: &Candidate,
    decoded_limit: u64,
    identity: StreamIdentity,
    minimum_blocks: usize,
    maximum_decoded: u64,
) -> Result<Option<CompactSplitSeed>> {
    if candidate.data.len() > COMPACT_SPLIT_FLOOR_MAX_COMPRESSED {
        return Ok(None);
    }

    let mut data = Vec::new();
    data.try_reserve_exact(candidate.data.len())
        .map_err(|_| Error::allocation("compact route seed"))?;
    data.extend_from_slice(&candidate.data);
    let stream = parse_validated_rewrite(&data, decoded_limit, identity, &candidate.plans)?;
    if !compact_source_split_floor_eligible_with_limits(
        identity.decoded_size,
        &stream.blocks,
        minimum_blocks,
        maximum_decoded,
    ) {
        return Ok(None);
    }
    Ok(Some(CompactSplitSeed {
        data,
        bits: candidate.bits,
        stream,
    }))
}

pub(super) fn build_prepared_compact_source_split_floor(
    seed: &CompactSplitSeed,
    options: &Options,
    decoded_limit: u64,
    identity: StreamIdentity,
) -> Result<Option<Candidate>> {
    let Some(plans) = plan_compact_source_split_floor(&seed.stream.blocks, 0, options) else {
        return Ok(None);
    };
    build_prepared_compact_source_split_floor_from_plans(
        seed,
        options,
        decoded_limit,
        identity,
        plans,
    )
}

fn build_prepared_compact_source_split_floor_until(
    seed: &CompactSplitSeed,
    options: &Options,
    decoded_limit: u64,
    identity: StreamIdentity,
    expired: &mut SearchStop<'_>,
) -> Result<Option<Candidate>> {
    let Some(plans) =
        plan_compact_source_split_floor_until(&seed.stream.blocks, 0, options, expired)
    else {
        return Ok(None);
    };
    build_prepared_compact_source_split_floor_from_plans(
        seed,
        options,
        decoded_limit,
        identity,
        plans,
    )
}

fn build_prepared_compact_source_split_floor_from_plans(
    seed: &CompactSplitSeed,
    options: &Options,
    decoded_limit: u64,
    identity: StreamIdentity,
    plans: Vec<PlannedBlock>,
) -> Result<Option<Candidate>> {
    let source = CandidateInput {
        compressed: &seed.data,
        blocks: &seed.stream.blocks,
        meaningful_bits: seed.bits,
        decoded_limit,
        identity,
    };
    let mut never_expires = SearchStop::never();
    build_candidate_from_plans(
        source,
        plans,
        options,
        0,
        ReplayPlanner::Full,
        &mut never_expires,
    )
    .map(|candidate| Some(candidate.named("Columbo compact split floor")))
}

pub(super) fn build_prepared_compact_source_split_floors(
    seeds: [Option<&CompactSplitSeed>; 3],
    options: &Options,
    decoded_limit: u64,
    identity: StreamIdentity,
    deadline: &Deadline,
) -> Result<Option<Candidate>> {
    let seeds = ordered_compact_split_seeds(seeds);
    let mut candidate = None;
    let mut attempted = false;
    for seed in seeds {
        // The first parent always runs and polls the hard stop, keeping every
        // completed cut. Later parents start only before the soft deadline,
        // so Max keeps a split that a later parent's split replaces.
        if attempted && !deadline.can_start_route() {
            break;
        }
        attempted = true;
        if let Some(contender) = build_prepared_compact_source_split_floor_until(
            seed,
            options,
            decoded_limit,
            identity,
            &mut deadline.hard_stop(),
        )? {
            replace_optional_keeping_parent(&mut candidate, contender, options.exhaustive);
        }
    }
    Ok(candidate)
}

fn ordered_compact_split_seeds(seeds: [Option<&CompactSplitSeed>; 3]) -> Vec<&CompactSplitSeed> {
    // Smaller parents carry the best known spellings and tables, so try them
    // first. Split pricing is not monotone, so every distinct parent still
    // runs while the deadline allows.
    let mut seeds: Vec<_> = seeds.into_iter().flatten().collect();
    seeds.sort_shared_by_key(|seed| (seed.data.len(), seed.bits));
    seeds
}

pub(super) fn compact_split_parent_is_completed(
    candidate: &Candidate,
    completed: Option<&[u8]>,
) -> bool {
    completed.is_some_and(|data| data == candidate.data)
}

pub(super) fn compact_split_preserves_source_blocks(
    blocks: &[ParsedBlock],
    plans: &[PlannedBlock],
) -> bool {
    blocks.len() == plans.len()
        && blocks.iter().zip(plans).all(|(block, plan)| {
            std::sync::Arc::ptr_eq(&block.tokens, &plan.tokens)
                && std::sync::Arc::ptr_eq(&block.plain, &plan.plain)
                && match &plan.representation {
                    Representation::Original(original) => original.block_type == block.source_type,
                    Representation::Stored => block.source_type == SourceBlockType::Stored,
                    Representation::Fixed => block.source_type == SourceBlockType::Fixed,
                    Representation::Dynamic(_) => block.source_type == SourceBlockType::Dynamic,
                }
        })
}

pub(super) fn refine_with_compact_source_split_floor(
    candidate: &Candidate,
    options: &Options,
    decoded_limit: u64,
    identity: StreamIdentity,
) -> Result<Option<Candidate>> {
    let Some(seed) = prepare_compact_source_split_seed(candidate, decoded_limit, identity)? else {
        return Ok(None);
    };
    build_prepared_compact_source_split_floor(&seed, options, decoded_limit, identity)
}

pub(super) fn refine_with_compact_source_split_floor_until(
    candidate: &Candidate,
    options: &Options,
    decoded_limit: u64,
    identity: StreamIdentity,
    expired: &mut SearchStop<'_>,
) -> Result<Option<Candidate>> {
    let Some(seed) = prepare_compact_source_split_seed(candidate, decoded_limit, identity)? else {
        return Ok(None);
    };
    build_prepared_compact_source_split_floor_until(
        &seed,
        options,
        decoded_limit,
        identity,
        expired,
    )
}

/// Split a compact source-max parent after the ordinary timed routes (M10d).
/// Unlike M10c, it accepts a one-block parent, whose split can find a second
/// Huffman regime, and up to 256 KiB decoded.
pub(super) fn refine_with_terminal_source_split_floor_until(
    candidate: &Candidate,
    options: &Options,
    decoded_limit: u64,
    identity: StreamIdentity,
    expired: &mut SearchStop<'_>,
) -> Result<Option<Candidate>> {
    let Some(seed) = prepare_compact_source_split_seed_with_limits(
        candidate,
        decoded_limit,
        identity,
        1,
        TERMINAL_SOURCE_SPLIT_MAX_DECODED,
    )?
    else {
        return Ok(None);
    };
    build_prepared_compact_source_split_floor_until(
        &seed,
        options,
        decoded_limit,
        identity,
        expired,
    )
}

pub(super) fn compact_source_split_floor_eligible_with_limits(
    decoded_size: u64,
    blocks: &[ParsedBlock],
    minimum_blocks: usize,
    maximum_decoded: u64,
) -> bool {
    if decoded_size > maximum_decoded
        || !(minimum_blocks..=COMPACT_SPLIT_FLOOR_MAX_BLOCKS).contains(&blocks.len())
        || blocks
            .iter()
            .any(|block| block.plain.is_empty() || block.source_type == SourceBlockType::Stored)
        || !blocks
            .iter()
            .any(|block| block.tokens.len() >= 16 && block.plain.len() >= 128)
    {
        return false;
    }
    blocks
        .iter()
        .try_fold(0_usize, |count, block| {
            count.checked_add(block.tokens.len())
        })
        .is_some_and(|tokens| tokens <= COMPACT_SPLIT_FLOOR_MAX_TOKENS)
}

#[cfg(test)]
mod tests;
