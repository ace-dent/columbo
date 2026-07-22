// SPDX-License-Identifier: MIT

//! Stream-level Deflate block planning.
//!
//! Deflate block boundaries do not affect the decoded bytes or the 32 KiB
//! history window. A boundary may therefore be removed or moved to any token
//! boundary without changing a match. Columbo uses that freedom to reduce the
//! number of headers and to give locally different data separate Huffman
//! tables. It never discovers new LZ77 matches here.

use std::borrow::Cow;
use std::sync::Arc;

use crate::Options;

use super::block::{plan_block, stored_block_bits};
use super::header::score_existing_dynamic;
use super::model::{
    count_frequencies, token_extra_bits, DynamicPlan, ParsedBlock, PlannedBlock, Representation,
    SourceBlockType, Token,
};
use super::search::{
    plan_block_with_floor, plan_block_with_java_floor, plan_block_with_search,
    plan_block_with_short_family_floor, replay_extended_floor, replay_table_ladder,
    score_short_family_frequencies, tighten_terminal_plan, ShortFamilyStats,
};

// The C default candidate tries its long-merge route for raw streams in this
// encoded-size range. Keeping the same broad gate avoids quadratic range
// searches on very large streams while covering the common encoder-flush case.
const DEFAULT_LONG_MERGE_MIN: u64 = 16_000;
const DEFAULT_LONG_MERGE_MAX: u64 = 100_000;
const MAX_REGROUP_SOURCE_BLOCKS: usize = 8;
const MAX_FRAGMENTED_REPLAY_BLOCKS: usize = 12;
const MAX_MERGED_TOKENS: usize = 250_000;
const MAX_MERGED_PLAIN: usize = 64_000_000;
// Small container streams can reach the same fixed point inside their normal
// proportional search slice. Reserve the deadline-independent full-range
// replay for larger joined payloads, where that slice is otherwise too short.
const WHOLE_STREAM_RECODE_MIN_PLAIN: usize = 100_000;
const WHOLE_STREAM_REPLAY_MARGIN_BITS: u64 = 256;
// DeflOpt's inexpensive long-run floor collects a bounded Huffman prefix
// before planning it.  This covers encoder flush streams without feeding a
// quadratic number of source-pair cuts into the general boundary DP.
const COLLECTED_RUN_MAX_TOKENS: usize = 8_192;
const COLLECTED_RUN_MAX_PLAIN: usize = 512 * 1_024;
const FRAGMENTED_COLLECT_MAX_TOKENS: usize = 4_096;
const FRAGMENTED_COLLECT_MIN_SOURCE_BLOCKS: usize = 64;
const WIDE_COLLECT_MIN_SOURCE_BLOCKS: usize = 128;
// The C block-list pass is a linear adjacent walk.  Keep the mandatory Rust
// equivalent bounded to ordinary encoder block counts; extremely fragmented
// streams use the 8,192-token collection floor above instead.
const MAX_GREEDY_SOURCE_BLOCKS: usize = 128;
const MAX_BOUNDED_GROUP_SPAN: usize = 16;
// Optional structural routes may copy payloads while the parsed source is
// still live. Two 48 MiB retained-payload partitions leave 32 MiB of a 128 MiB
// envelope for the bounded DP table and Huffman metadata. Boundary DP itself
// keeps the source token parse unchanged; token-spelling searches run in the
// sequential/replay passes rather than once per cut and alignment.
const MAX_GROUPED_MODEL_BYTES: usize = 48 * 1024 * 1024;
const MAX_COMPOSITE_MODEL_BYTES: usize = 48 * 1024 * 1024;
// Boundary DP is an optional max route. Capping its cut count keeps the dense
// eight-alignment state table small on adversarially fragmented streams.
const MAX_BOUNDARY_DP_CUTS: usize = 2_048;
/// Only near-tied first splits justify an extra child refinement in default
/// mode; a wider gap is very unlikely to be recovered by one added header.
const NESTED_RUNNER_UP_MARGIN_BITS: u64 = 64;

/// How much adjacent-source work the linear pending-block planner may do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AdjacentMergeSearch {
    /// Retain only exact fixed-block joins.
    Disabled,
    /// Search tiny neighbours; this is the inexpensive ordinary fallback.
    Local,
    /// Rebuild every eligible adjacent Huffman pair in a long source run.
    LongRun,
}

/// Plan all blocks in a raw Deflate stream, beginning at `start_alignment`.
///
/// Splits and merges are flattened into ordinary [`PlannedBlock`] values. This
/// keeps stream search out of the emitter and makes every returned block useful
/// on its own. `expired` is deliberately borrowed from the caller so all block,
/// header, and stream searches share one deadline.
pub(crate) fn plan_stream<F>(
    blocks: &[ParsedBlock],
    start_alignment: u8,
    options: &Options,
    expired: &mut F,
) -> Option<Vec<PlannedBlock>>
where
    F: FnMut() -> bool,
{
    // Stored-only streams have a linear structural floor: adjacent
    // chunks can be repacked up to RFC 1951's 65,535-byte limit without any
    // Huffman or token search. Secure that result before consulting a shared
    // container deadline so a large ZIP cannot optimize only its first member.
    let stored_floor = repack_all_stored_blocks(blocks, start_alignment);

    // A container can call us after its file-wide budget is already spent so
    // the stream is still parsed and validated. Retain its exact source bytes
    // through `build_candidate`'s fallback unless the stored repack above is
    // available; starting token recodes for dozens of later APNG frames would
    // turn a bounded timeout into unbounded work. Compatibility mode must still
    // rewrite its distance alphabet.
    let floor_time_available = !expired();
    if !floor_time_available && !options.min_distance_codes {
        return stored_floor.map(|floor| finish_plan(floor, options));
    }
    let source_bytes = encoded_source_bytes(blocks);
    // Stored-only streams deserve an inexpensive floor before any Huffman
    // search. General-purpose encoders often flush incompressible input in
    // 16 KiB stored chunks; repacking those bytes into RFC 1951's 65,535-byte
    // maximum removes headers without changing or rediscovering any match.
    // Build this first so it remains available even if pricing the literal
    // alternatives consumes the caller's search deadline.
    let prepared = prepare_blocks(blocks);
    let blocks = prepared.as_deref().unwrap_or(blocks);
    let allow_regroup = floor_time_available
        && (options.exhaustive
            || (DEFAULT_LONG_MERGE_MIN..=DEFAULT_LONG_MERGE_MAX).contains(&source_bytes));
    let default_long_run =
        !options.exhaustive && allow_regroup && blocks.len() > MAX_REGROUP_SOURCE_BLOCKS;
    let source_merge_floor = allow_regroup
        .then(|| source_aligned_huffman_floor(blocks, start_alignment, options))
        .flatten();
    let greedy_blocks = if allow_regroup
        && blocks.len() > MAX_REGROUP_SOURCE_BLOCKS
        && blocks.len() <= MAX_GREEDY_SOURCE_BLOCKS
    {
        greedy_huffman_blocklist(blocks, start_alignment, options)
            .filter(|grouped| grouped.len() < blocks.len())
    } else {
        None
    };
    let greedy_floor = greedy_blocks
        .as_deref()
        .and_then(|grouped| direct_structural_plan(grouped, start_alignment, options));
    let bounded_group_blocks = if allow_regroup
        && blocks.len() > MAX_REGROUP_SOURCE_BLOCKS
        && blocks.len() <= MAX_GREEDY_SOURCE_BLOCKS
    {
        bounded_huffman_grouping(blocks, options).filter(|grouped| grouped.len() < blocks.len())
    } else {
        None
    };
    let bounded_group_floor = bounded_group_blocks
        .as_deref()
        .and_then(|grouped| direct_structural_plan(grouped, start_alignment, options));
    let collected_blocks = if allow_regroup && blocks.len() > MAX_REGROUP_SOURCE_BLOCKS {
        collect_huffman_runs(blocks, false).filter(|collected| collected.len() < blocks.len())
    } else {
        None
    };
    let collected_floor = collected_blocks
        .as_deref()
        .and_then(|collected| direct_structural_plan(collected, start_alignment, options));
    // Price a wide collection as an additive candidate. It is effective for
    // uniform runs, while the bounded collection above keeps several local
    // tables for streams such as row-flushed PNG data.
    let wide_collected_blocks = if allow_regroup
        && blocks.len() > MAX_REGROUP_SOURCE_BLOCKS
        && (options.exhaustive || blocks.len() >= WIDE_COLLECT_MIN_SOURCE_BLOCKS)
    {
        collect_huffman_runs(blocks, true).filter(|wide| {
            wide.len() < blocks.len()
                && collected_blocks
                    .as_ref()
                    .map_or(true, |bounded| !same_block_layout(wide, bounded))
        })
    } else {
        None
    };
    let wide_collected_floor = wide_collected_blocks
        .as_deref()
        .and_then(|wide| direct_structural_plan(wide, start_alignment, options));

    // Search whichever cheap grouping has the best complete direct price.
    // Keeping this choice separate from the emitted fallback means all other
    // layouts remain available as strict no-growth candidates.
    let grouped_search = [
        greedy_blocks
            .as_deref()
            .zip(greedy_floor.as_ref().map(|floor| total_bits(floor))),
        bounded_group_blocks
            .as_deref()
            .zip(bounded_group_floor.as_ref().map(|floor| total_bits(floor))),
        collected_blocks
            .as_deref()
            .zip(collected_floor.as_ref().map(|floor| total_bits(floor))),
    ]
    .into_iter()
    .flatten()
    .min_by_key(|(_, bits)| *bits)
    .map(|(grouped, _)| grouped);

    // Secure a complete deadline-independent path before token-spelling or
    // split searches.  On a shared container deadline this also guarantees
    // that every stream receives useful structural optimization.
    let mut fallback = if floor_time_available && !expired() {
        mandatory_token_floor_plan(blocks, start_alignment, options)?
    } else {
        direct_structural_plan(blocks, start_alignment, options)?
    };
    for floor in [
        stored_floor,
        source_merge_floor,
        greedy_floor,
        bounded_group_floor,
        collected_floor,
        wide_collected_floor,
    ]
    .into_iter()
    .flatten()
    {
        if total_bits(&floor) < total_bits(&fallback) {
            fallback = floor;
        }
    }

    // Search the most promising pre-grouped list before the original long
    // list can consume the deadline.  C's block-list route does the same: it
    // first commits cheap adjacent structure, then replays the selected groups.
    if let Some(grouped) = grouped_search {
        if !expired() {
            if let Some(candidate) = sequential_plan(
                grouped,
                start_alignment,
                options,
                AdjacentMergeSearch::Disabled,
                expired,
            ) {
                if total_bits(&candidate) < total_bits(&fallback) {
                    fallback = candidate;
                }
            }
        }
    }

    // The ordinary source-order route remains an additive candidate. It is
    // both the best path for unfragmented streams and a way to retain source
    // boundaries when the cheap grouping was locally misleading.
    let fallback_merge_search = if default_long_run {
        // For a long encoder-flush chain, this pending-block fold subsumes the
        // ordinary sequential plan: every merge is optional and must beat the
        // exact separate cost before it is retained.
        AdjacentMergeSearch::LongRun
    } else if allow_regroup {
        AdjacentMergeSearch::Disabled
    } else {
        AdjacentMergeSearch::Local
    };
    if !expired() {
        if let Some(candidate) = sequential_plan(
            blocks,
            start_alignment,
            options,
            fallback_merge_search,
            expired,
        ) {
            if total_bits(&candidate) < total_bits(&fallback) {
                fallback = candidate;
            }
        }
    }
    if blocks.len() <= 1 && blocks.first().map_or(true, |block| block.plain.len() < 128) {
        return Some(finish_plan(fallback, options));
    }
    if expired() {
        return Some(finish_plan(fallback, options));
    }

    // While the shared deadline still permits optional work, always price the
    // bounded collect-before-plan floor. A regular encoder-flush chain can need
    // several source blocks under one table before its first pairwise merge pays
    // for itself. Savings found by the greedy adjacent walk do not dominate that
    // different grouping, even when those savings happen to be large elsewhere.
    if allow_regroup && blocks.len() > MAX_REGROUP_SOURCE_BLOCKS {
        if let Some(collected_blocks) = collected_blocks.as_deref() {
            let collected = sequential_plan(
                collected_blocks,
                start_alignment,
                options,
                AdjacentMergeSearch::Disabled,
                expired,
            );
            if let Some(collected) = collected {
                if total_bits(&collected) < total_bits(&fallback) {
                    fallback = collected;
                }
            }
        }
        if expired() {
            return Some(finish_plan(fallback, options));
        }

        // Default-mode cross-source DP edges are deliberately limited to a
        // short run.  Above that limit the remaining cut set can only revisit
        // individual source blocks at different alignments; it cannot improve
        // on either complete plan above.  Returning here avoids quadratic
        // work over hundreds of encoder-flush blocks.  Max mode still admits
        // arbitrary cross-source edges and therefore keeps the broader DP.
        if !options.exhaustive {
            return Some(finish_plan(fallback, options));
        }
    }

    if !allow_regroup {
        // Per-source split probes are already part of the linear fallback.
        // Outside the long-merge range there are no default cross-source DP
        // candidates, so avoid multiplying those probes by alignment states.
        return Some(finish_plan(fallback, options));
    }
    let Some(composite) = Composite::new(blocks) else {
        return Some(finish_plan(fallback, options));
    };
    let Some(cuts) = choose_cuts(&composite, options.exhaustive, allow_regroup) else {
        return Some(finish_plan(fallback, options));
    };
    if cuts.len() <= 2 {
        return Some(finish_plan(fallback, options));
    }

    let Some(candidate) = boundary_dp(
        blocks,
        &composite,
        &cuts,
        start_alignment,
        options,
        allow_regroup,
        expired,
    ) else {
        return Some(finish_plan(fallback, options));
    };
    if total_bits(&candidate) < total_bits(&fallback) {
        Some(finish_plan(candidate, options))
    } else {
        Some(finish_plan(fallback, options))
    }
}

/// Compose the deterministic per-block tree floor with the selected layout.
///
/// A shorter Huffman block can shift padding in a later stored block. Tighten
/// only the alignment-independent suffix after the last such block; all-Huffman
/// streams naturally admit the complete plan list.
fn finish_plan(mut plans: Vec<PlannedBlock>, options: &Options) -> Vec<PlannedBlock> {
    let first_safe = plans
        .iter()
        .rposition(|plan| !plan_is_alignment_independent(plan))
        .map_or(0, |index| index + 1);
    for plan in &mut plans[first_safe..] {
        tighten_terminal_plan(plan, options);
    }
    plans
}

/// Price a prepared block list without token-spelling or split searches.
///
/// This complete linear pass is intentionally deadline-independent. It is the
/// inexpensive comparison floor that makes a useful collected grouping
/// available before optional byte-seeking work begins.
fn direct_structural_plan(
    blocks: &[ParsedBlock],
    start_alignment: u8,
    options: &Options,
) -> Option<Vec<PlannedBlock>> {
    let mut structural_options = options.clone();
    structural_options.exhaustive = false;
    let mut plans = Vec::new();
    plans.try_reserve_exact(blocks.len()).ok()?;
    let mut output_bits = 0_u64;

    for block in blocks {
        let alignment = ((u64::from(start_alignment) + output_bits) & 7) as u8;
        let plan = plan_block(block, alignment, &structural_options, || false);
        append_output_plan(&mut plans, &mut output_bits, plan, true)?;
    }
    Some(plans)
}

/// Apply one bounded token-preserving pass to each ordinary source block.
///
/// Very fragmented streams are handled by collection first; running even a
/// small token pass hundreds of times would spend the container deadline on
/// bookkeeping. For normal block counts this floor gives every ZIP/APNG
/// member strict source/fixed/Defluff match expansions before optional search.
fn mandatory_token_floor_plan(
    blocks: &[ParsedBlock],
    start_alignment: u8,
    options: &Options,
) -> Option<Vec<PlannedBlock>> {
    if blocks.len() > MAX_GREEDY_SOURCE_BLOCKS {
        return direct_structural_plan(blocks, start_alignment, options);
    }
    let mut plans = Vec::new();
    plans.try_reserve_exact(blocks.len()).ok()?;
    let mut output_bits = 0_u64;
    let extended = blocks.len() <= MAX_REGROUP_SOURCE_BLOCKS;
    for block in blocks {
        let alignment = ((u64::from(start_alignment) + output_bits) & 7) as u8;
        let mut plan = plan_block_with_floor(block, alignment, options, extended);
        // deft4j's five cumulative 6..10-length states are cheap enough to be
        // a true per-source floor: their frequency effects are fixed-size and
        // each materialized candidate only expands matches already present in
        // this block. Price them before a container deadline can divert the
        // optional split search toward a locally attractive two-block layout.
        let short_family = plan_block_with_short_family_floor(block, alignment, options);
        if short_family.bits < plan.bits {
            plan = short_family;
        }
        append_output_plan(&mut plans, &mut output_bits, plan, true)?;
    }
    Some(plans)
}

/// Repacketize an all-stored stream using the largest legal stored payloads.
///
/// A stored block carries no Huffman or LZ77 decisions, so its boundary is
/// pure serialization overhead. Keeping this as a separate linear floor also
/// avoids cloning one `Token::Literal` per byte merely to join large random
/// inputs; only the bytes required by the winning stored plans are copied.
fn repack_all_stored_blocks(
    blocks: &[ParsedBlock],
    start_alignment: u8,
) -> Option<Vec<PlannedBlock>> {
    if blocks.len() < 2
        || blocks
            .iter()
            .any(|block| block.source_type != SourceBlockType::Stored)
    {
        return None;
    }
    let plain_size = blocks
        .iter()
        .try_fold(0_usize, |total, block| total.checked_add(block.plain.len()))?;
    if plain_size > MAX_MERGED_PLAIN {
        return None;
    }

    let plan_count = plain_size.checked_add(65_534)?.checked_div(65_535)?.max(1);
    let mut plans = Vec::new();
    plans.try_reserve_exact(plan_count).ok()?;
    let empty_tokens = Arc::new(Vec::new());
    let mut output_bits = 0_u64;
    let mut block_index = 0_usize;
    let mut byte_index = 0_usize;
    let mut bytes_written = 0_usize;

    for _ in 0..plan_count {
        let remaining = plain_size.checked_sub(bytes_written)?;
        let chunk_len = remaining.min(65_535);
        let mut plain = Vec::new();
        plain.try_reserve_exact(chunk_len).ok()?;

        while plain.len() < chunk_len {
            let block = blocks.get(block_index)?;
            let available = block.plain.len().saturating_sub(byte_index);
            let take = available.min(chunk_len - plain.len());
            plain.extend_from_slice(&block.plain[byte_index..byte_index + take]);
            byte_index += take;
            if byte_index == block.plain.len() {
                block_index += 1;
                byte_index = 0;
            }
        }

        let alignment = ((u64::from(start_alignment) + output_bits) & 7) as u8;
        let bits = stored_block_bits(alignment, chunk_len);
        output_bits = output_bits.checked_add(bits)?;
        bytes_written = bytes_written.checked_add(chunk_len)?;
        plans.push(PlannedBlock {
            tokens: Arc::clone(&empty_tokens),
            plain: Arc::new(plain),
            representation: Representation::Stored,
            bits,
            source_type: SourceBlockType::Stored,
        });
    }
    Some(plans)
}

/// Find the best bounded grouping at original block boundaries.
///
/// Range templates are priced directly, while merged ranges add a small set
/// of bounded token spellings. With at most eight source blocks there are only
/// 36 contiguous ranges. Pricing them first prevents a byte-seeking search on
/// the first large block from consuming the deadline before a profitable
/// three-block merge is seen.
fn source_aligned_huffman_floor(
    blocks: &[ParsedBlock],
    start_alignment: u8,
    options: &Options,
) -> Option<Vec<PlannedBlock>> {
    source_aligned_huffman_floor_with_limit(
        blocks,
        start_alignment,
        options,
        MAX_REGROUP_SOURCE_BLOCKS,
    )
}

fn source_aligned_huffman_floor_with_limit(
    blocks: &[ParsedBlock],
    start_alignment: u8,
    options: &Options,
    max_blocks: usize,
) -> Option<Vec<PlannedBlock>> {
    if !(2..=max_blocks).contains(&blocks.len())
        || blocks
            .iter()
            .any(|block| block.source_type == SourceBlockType::Stored)
    {
        return None;
    }

    let composite = Composite::new(blocks)?;
    let mut structural_options = options.clone();
    // Keep every candidate non-exhaustive. The merged recodes below are
    // deterministic and deadline-independent; max mode retains their result
    // as an upper bound before beginning deeper searches.
    structural_options.exhaustive = false;

    struct Node {
        bits: u64,
        previous: usize,
        plan: SourceAlignedPlan,
    }

    // Most range candidates can keep only their representation and borrow the
    // original composite payload during reconstruction. A token recode owns a
    // different spelling, so retain that complete plan only when it wins.
    enum SourceAlignedPlan {
        Template(PlanTemplate),
        Recode(PlannedBlock),
    }

    let mut best: Vec<Option<Node>> = Vec::new();
    best.try_reserve_exact(blocks.len().checked_add(1)?).ok()?;
    best.resize_with(blocks.len() + 1, || None);
    let mut prefix_bits = Vec::new();
    prefix_bits
        .try_reserve_exact(blocks.len().checked_add(1)?)
        .ok()?;
    prefix_bits.resize(blocks.len() + 1, u64::MAX);
    prefix_bits[0] = 0;

    for end_index in 1..=blocks.len() {
        for start_index in 0..end_index {
            let prefix = prefix_bits[start_index];
            if prefix == u64::MAX {
                continue;
            }
            let first = composite.sources[start_index];
            let last = composite.sources[end_index - 1];
            let start = Cut {
                token: first.token_start,
                plain: first.plain_start,
            };
            let end = Cut {
                token: last.token_end,
                plain: last.plain_end,
            };
            if end.token - start.token > MAX_MERGED_TOKENS
                || end.plain - start.plain > MAX_MERGED_PLAIN
            {
                continue;
            }

            // Huffman blocks have no alignment padding, so one direct price is
            // valid at every incoming bit offset. Exact source blocks may keep
            // their original fixed/dynamic bits for the same reason.
            let template = plan_edge(
                blocks,
                &composite,
                start,
                end,
                start_alignment,
                &structural_options,
                &mut || false,
            )?;
            let mut candidate_bits = template.bits;
            let mut candidate_is_stored = matches!(template.representation, Representation::Stored);
            let mut candidate_plan = SourceAlignedPlan::Template(template);

            // deft4j rebuilds a merged range before pruning matches. Pricing
            // the Java and cumulative short-family states only after the range
            // has been formed is essential: separate per-source trees cannot
            // reproduce the merged frequency table. The ordinary eight-block
            // path has at most 28 merged ranges; the twelve-block fragmented
            // replay path has 66. Every helper enforces model limits.
            if end_index - start_index > 1 {
                if let Some(range) = make_range(&composite, start, end) {
                    let java =
                        plan_block_with_java_floor(&range, start_alignment, &structural_options);
                    let short_family = plan_block_with_short_family_floor(
                        &range,
                        start_alignment,
                        &structural_options,
                    );
                    // A container shares one deadline across all of its
                    // streams. Give a large complete merged stream one
                    // extended token-preserving pass now, so an earlier frame
                    // cannot prevent its best whole-stream spelling from
                    // being seen.
                    // Limiting this to the full range avoids multiplying that
                    // work across every possible source-aligned subrange.
                    let is_large_whole_stream = start_index == 0
                        && end_index == blocks.len()
                        && range.plain.len() >= WHOLE_STREAM_RECODE_MIN_PLAIN;
                    let whole_stream = is_large_whole_stream.then(|| {
                        plan_block_with_floor(&range, start_alignment, &structural_options, true)
                    });
                    let replay_seed_bits = [
                        Some(java.bits),
                        Some(short_family.bits),
                        whole_stream.as_ref().map(|plan| plan.bits),
                    ]
                    .into_iter()
                    .flatten()
                    .fold(candidate_bits, u64::min);
                    for mut recode in [Some(java), Some(short_family), whole_stream]
                        .into_iter()
                        .flatten()
                    {
                        // A changed tree can expose another strict spelling
                        // win. Complete every near-tied full-stream seed before
                        // comparing it: a temporarily dearer table can be the
                        // intermediate state needed by the bounded ladder. The
                        // same 256-bit beam used by the state queue prevents a
                        // weak seed from consuming later frames' shared time.
                        if is_large_whole_stream
                            && recode.bits
                                <= replay_seed_bits.saturating_add(WHOLE_STREAM_REPLAY_MARGIN_BITS)
                        {
                            if let Some(replay) =
                                replay_extended_floor(&recode, start_alignment, &structural_options)
                            {
                                recode = replay;
                            }
                            if let Some(ladder) =
                                replay_table_ladder(&recode, start_alignment, &structural_options)
                            {
                                recode = ladder;
                            }
                        }
                        if !matches!(recode.representation, Representation::Stored)
                            && recode.bits < candidate_bits
                        {
                            candidate_bits = recode.bits;
                            candidate_is_stored = false;
                            candidate_plan = SourceAlignedPlan::Recode(recode);
                        }
                    }
                }
            }

            if candidate_is_stored {
                // Stored output pads to a byte boundary, so its price depends
                // on the prefix alignment. The general eight-state DP handles
                // that case; this one-dimensional Huffman floor does not.
                continue;
            }
            let bits = prefix.checked_add(candidate_bits)?;
            if bits < prefix_bits[end_index] {
                prefix_bits[end_index] = bits;
                best[end_index] = Some(Node {
                    bits,
                    previous: start_index,
                    plan: candidate_plan,
                });
            }
        }
    }

    let mut at = blocks.len();
    let mut plans = Vec::new();
    plans.try_reserve_exact(blocks.len()).ok()?;
    while at != 0 {
        let node = best[at].take()?;
        debug_assert_eq!(node.bits, prefix_bits[at]);
        let first = composite.sources[node.previous];
        let last = composite.sources[at - 1];
        plans.push(match node.plan {
            SourceAlignedPlan::Template(template) => template.instantiate(
                &composite,
                Cut {
                    token: first.token_start,
                    plain: first.plain_start,
                },
                Cut {
                    token: last.token_end,
                    plain: last.plain_end,
                },
            )?,
            SourceAlignedPlan::Recode(plan) => plan,
        });
        at = node.previous;
    }
    plans.reverse();
    Some(plans)
}

/// Bounded replay planner for an alternate fragmented-stream seed.
///
/// The ordinary planner deliberately explores broad token and boundary state.
/// Once a 4,096-token seed has exposed useful coarse boundaries, replay needs a
/// different profile: repack/transform each complete block, price every source
/// boundary merge, and test the seven decoded eighths. Repeating these exact,
/// linear-sized stages after emission makes nested cuts available without
/// spending the seed's deadline on the original hundreds of source blocks.
pub(crate) fn plan_fragmented_replay(
    blocks: &[ParsedBlock],
    start_alignment: u8,
    options: &Options,
) -> Option<Vec<PlannedBlock>> {
    if blocks.is_empty()
        || blocks.len() > MAX_FRAGMENTED_REPLAY_BLOCKS
        || blocks
            .iter()
            .any(|block| block.source_type == SourceBlockType::Stored)
    {
        return None;
    }

    let mut best = mandatory_token_floor_plan(blocks, start_alignment, options)?;
    if let Some(grouped) = source_aligned_huffman_floor_with_limit(
        blocks,
        start_alignment,
        options,
        MAX_FRAGMENTED_REPLAY_BLOCKS,
    ) {
        if total_bits(&grouped) < total_bits(&best) {
            best = grouped;
        }
    }
    if let Some(split) = direct_eighth_split_floor(blocks, start_alignment, options) {
        if split.len() <= MAX_FRAGMENTED_REPLAY_BLOCKS && total_bits(&split) < total_bits(&best) {
            best = split;
        }
    }
    Some(best)
}

/// Price one direct decoded-eighth split per block without deep token search.
/// Accepted children become ordinary source blocks after emission, so a later
/// bounded replay can split them again or merge either child with a neighbour.
fn direct_eighth_split_floor(
    blocks: &[ParsedBlock],
    start_alignment: u8,
    options: &Options,
) -> Option<Vec<PlannedBlock>> {
    let mut structural_options = options.clone();
    structural_options.exhaustive = false;
    let mut output = Vec::new();
    output.try_reserve_exact(blocks.len()).ok()?;
    let mut output_bits = 0_u64;

    for block in blocks {
        let alignment = ((u64::from(start_alignment) + output_bits) & 7) as u8;
        let base = plan_block(block, alignment, &structural_options, || false);
        // Each source block contributes either itself or two children. Reserve
        // that tiny upper bound fallibly so even optional replay work respects
        // the optimizer's allocation-failure contract.
        let mut winner = Vec::new();
        winner.try_reserve_exact(2).ok()?;
        winner.push(base);
        let mut winner_bits = total_bits(&winner);

        if block.tokens.len() >= 16 && block.plain.len() >= 128 {
            let composite = Composite::new(std::slice::from_ref(block))?;
            let source = composite.sources[0];
            let mut cuts = Vec::new();
            add_eighth_cuts(
                &mut cuts,
                &composite,
                source.token_start,
                source.token_end,
                source.plain_start,
                source.plain_end,
            )?;
            cuts.sort_unstable_by_key(|cut| cut.token);
            cuts.dedup_by_key(|cut| cut.token);
            for split in cuts {
                let left = make_range(&composite, Cut { token: 0, plain: 0 }, split)?;
                let left_plan = plan_block(&left, alignment, &structural_options, || false);
                let right_alignment = ((u64::from(alignment) + left_plan.bits) & 7) as u8;
                let right = make_range(
                    &composite,
                    split,
                    Cut {
                        token: block.tokens.len(),
                        plain: block.plain.len(),
                    },
                )?;
                let right_plan = plan_block(&right, right_alignment, &structural_options, || false);
                let bits = left_plan.bits.checked_add(right_plan.bits)?;
                if bits < winner_bits {
                    winner_bits = bits;
                    winner.clear();
                    winner.push(left_plan);
                    winner.push(right_plan);
                }
            }
        }

        if output.len().checked_add(winner.len())? > MAX_FRAGMENTED_REPLAY_BLOCKS {
            return None;
        }
        append_output_plans(&mut output, &mut output_bits, winner)?;
    }
    Some(output)
}

/// Remove semantic no-ops and collect adjacent stored chunks while their
/// combined payload still fits a single RFC 1951 stored block.
///
/// `None` means either that no preparation is useful or that this optional
/// route could not allocate its metadata; both cases use the original blocks.
fn prepare_blocks(blocks: &[ParsedBlock]) -> Option<Vec<ParsedBlock>> {
    // Prepared blocks, the sequential winner, and a collected alternative can
    // all be live briefly. Budget three conservative complete payload views.
    if payload_storage_bytes(blocks)?.checked_mul(3)? > MAX_GROUPED_MODEL_BYTES {
        return None;
    }
    let keep_one_empty = !blocks.iter().any(|block| !block.plain.is_empty());

    // Most streams need no preparation at all. Avoid duplicating the fairly
    // rich per-block metadata unless an empty block can actually be removed or
    // a neighbouring stored pair can actually be joined.
    let removes_empty = if keep_one_empty {
        blocks.len() > 1
    } else {
        blocks.iter().any(|block| block.plain.is_empty())
    };
    let joins_stored = blocks.windows(2).any(|pair| {
        pair[0].source_type == SourceBlockType::Stored
            && pair[1].source_type == SourceBlockType::Stored
            && pair[0]
                .plain
                .len()
                .checked_add(pair[1].plain.len())
                .is_some_and(|plain| plain <= 65_535)
    });
    if !removes_empty && !joins_stored {
        return None;
    }

    let mut prepared = Vec::<ParsedBlock>::new();

    for block in blocks {
        if block.plain.is_empty() && (!keep_one_empty || !prepared.is_empty()) {
            continue;
        }

        if let Some(previous) = prepared.last_mut() {
            if previous.source_type == SourceBlockType::Stored
                && block.source_type == SourceBlockType::Stored
                && previous
                    .plain
                    .len()
                    .checked_add(block.plain.len())
                    .is_some_and(|plain| plain <= 65_535)
                && try_append_parsed_block(previous, block)
            {
                continue;
            }
        }
        prepared.try_reserve(1).ok()?;
        prepared.push(block.try_clone_shared()?);
    }

    Some(prepared)
}

fn payload_storage_bytes(blocks: &[ParsedBlock]) -> Option<usize> {
    blocks.iter().try_fold(0_usize, |total, block| {
        let tokens = block
            .tokens
            .capacity()
            .checked_mul(std::mem::size_of::<Token>())?;
        total
            .checked_add(tokens)?
            .checked_add(block.plain.capacity())
    })
}

/// Recreate C's inexpensive source-order block-list merge pass.
///
/// Each source block and adjacent merged pair is priced with the ordinary
/// token-preserving planner.  A strictly cheaper merge replaces the pending
/// pair and is immediately compared with its next neighbour.  This greedy
/// shape matters: photographic streams commonly settle into groups of four to
/// eleven source blocks, while collecting the entire run under one table is
/// measurably worse.
fn greedy_huffman_blocklist(
    blocks: &[ParsedBlock],
    start_alignment: u8,
    options: &Options,
) -> Option<Vec<ParsedBlock>> {
    let (first, rest) = blocks.split_first()?;
    if rest.is_empty()
        || blocks
            .iter()
            .any(|block| block.source_type == SourceBlockType::Stored)
        // The linear walk retains the parsed source and at most one complete
        // grouped view. Candidate pairs are bounded subsets of that view.
        || payload_storage_bytes(blocks)?.checked_mul(2)? > MAX_GROUPED_MODEL_BYTES
    {
        return None;
    }

    // This is a structural floor, not a second max search.  The exhaustive
    // header families remain available when the chosen groups are replayed.
    let mut structural_options = options.clone();
    structural_options.exhaustive = false;

    let mut grouped = Vec::new();
    grouped.try_reserve_exact(blocks.len()).ok()?;
    let mut output_bits = 0_u64;
    let mut pending = PendingBlock::Borrowed(first);
    let mut pending_cache: Option<(u8, PlannedBlock)> = None;

    for current in rest {
        let alignment = ((u64::from(start_alignment) + output_bits) & 7) as u8;
        let pending_plan = match pending_cache.take() {
            Some((cached_alignment, plan)) if cached_alignment == alignment => plan,
            _ => plan_block_with_floor(pending.as_block(), alignment, &structural_options, false),
        };
        let current_alignment = ((u64::from(alignment) + pending_plan.bits) & 7) as u8;
        let current_plan =
            plan_block_with_floor(current, current_alignment, &structural_options, false);
        let mut separate_bits = pending_plan.bits.checked_add(current_plan.bits)?;
        // Two fixed plans are emitted as one fixed run elsewhere, so compare a
        // rebuilt dynamic block with that exact ten-bit-cheaper floor.
        if is_fixed_plan(&pending_plan) && is_fixed_plan(&current_plan) {
            separate_bits = separate_bits.checked_sub(10)?;
        }

        let pending_block = pending.as_block();
        let can_merge = pending_block
            .tokens
            .len()
            .checked_add(current.tokens.len())
            .is_some_and(|tokens| tokens <= MAX_MERGED_TOKENS)
            && pending_block
                .plain
                .len()
                .checked_add(current.plain.len())
                .is_some_and(|plain| plain <= MAX_MERGED_PLAIN);
        if can_merge {
            if let Some(merged) = try_merge_parsed_blocks(pending_block, current) {
                let merged_plan =
                    plan_block_with_floor(&merged, alignment, &structural_options, false);
                if merged_plan.bits < separate_bits {
                    let mut merged = merged;
                    // Carry a strict intermediate token winner into the next
                    // adjacent comparison, matching C's retained block list.
                    merged.tokens = merged_plan.tokens.clone();
                    merged.recount_frequencies();
                    pending = PendingBlock::Owned(merged);
                    pending_cache = Some((alignment, merged_plan));
                    continue;
                }
            }
        }

        grouped.try_reserve(1).ok()?;
        grouped.push(pending_block.try_clone_shared()?);
        output_bits = output_bits.checked_add(pending_plan.bits)?;
        pending = PendingBlock::Borrowed(current);
        pending_cache = Some((current_alignment, current_plan));
    }

    grouped.try_reserve(1).ok()?;
    grouped.push(pending.as_block().try_clone_shared()?);
    Some(grouped)
}

/// Find useful source-boundary groups with a small lookahead.
///
/// Greedy adjacent merging faithfully models C's block list, but a first pair
/// can be neutral even when three or more neighbours profit from one shared
/// table. At each source position, price at most sixteen complete groups,
/// commit the strict best saving, and continue after it. This keeps the pass
/// linear in practical group count while retaining the important lookahead.
fn bounded_huffman_grouping(blocks: &[ParsedBlock], options: &Options) -> Option<Vec<ParsedBlock>> {
    if blocks.len() < 2
        || blocks.len() > MAX_GREEDY_SOURCE_BLOCKS
        || blocks
            .iter()
            .any(|block| block.source_type == SourceBlockType::Stored)
        // Range pricing is transient; reconstruction retains only the source
        // plus one complete winning grouped view.
        || payload_storage_bytes(blocks)?.checked_mul(2)? > MAX_GROUPED_MODEL_BYTES
    {
        return None;
    }

    let mut structural_options = options.clone();
    structural_options.exhaustive = false;
    let mut source_plans = Vec::new();
    source_plans.try_reserve_exact(blocks.len()).ok()?;
    for block in blocks {
        let plan = plan_block_with_floor(block, 0, &structural_options, false);
        if matches!(plan.representation, Representation::Stored) {
            return None;
        }
        source_plans.push(plan);
    }

    let mut source_stats = Vec::new();
    source_stats.try_reserve_exact(blocks.len()).ok()?;
    for block in blocks {
        source_stats.push(ShortFamilyStats::from_block(block)?);
    }

    // Price every bounded range once. Range costs are independent of their
    // neighbours because all candidates here are Huffman blocks, so a small
    // suffix DP can choose the best complete segmentation instead of making
    // an irreversible greedy choice at each source boundary.
    let mut range_bits = Vec::new();
    range_bits.try_reserve_exact(blocks.len()).ok()?;
    for start in 0..blocks.len() {
        let mut prices = [None; MAX_BOUNDED_GROUP_SPAN + 1];
        prices[1] = Some(source_plans[start].bits);
        let mut literal_frequencies = [0_u32; 286];
        let mut distance_frequencies = [0_u32; 30];
        let mut range_extra_bits = 0_u64;
        let mut range_tokens = 0_usize;
        let mut range_plain = 0_usize;
        let mut range_stats = source_stats[start].clone();

        for end in start + 1..=blocks.len().min(start + MAX_BOUNDED_GROUP_SPAN) {
            let block = &blocks[end - 1];
            range_tokens = range_tokens.checked_add(block.tokens.len())?;
            range_plain = range_plain.checked_add(block.plain.len())?;
            range_extra_bits = range_extra_bits.checked_add(token_extra_bits(&block.tokens))?;
            for (total, &frequency) in literal_frequencies
                .iter_mut()
                .zip(&block.literal_frequencies)
            {
                *total = total.checked_add(frequency)?;
            }
            for (total, &frequency) in distance_frequencies
                .iter_mut()
                .zip(&block.distance_frequencies)
            {
                *total = total.checked_add(frequency)?;
            }

            if end > start + 1 {
                // Every parsed source block owns an end-of-block symbol. A
                // grouped range emits only one, at the end of the range.
                literal_frequencies[256] = literal_frequencies[256].checked_sub(1)?;
                range_stats.add_assign(&source_stats[end - 1])?;
            }

            if range_tokens > MAX_MERGED_TOKENS || range_plain > MAX_MERGED_PLAIN {
                break;
            }
            if end == start + 1 {
                continue;
            }

            // Short-family token changes have additive frequency effects.
            // Score each possible range from these fixed-size summaries, then
            // materialize and fully plan only the range that is selected.
            let group_bits = score_short_family_frequencies(
                &literal_frequencies,
                &distance_frequencies,
                range_extra_bits,
                &range_stats,
                structural_options.min_distance_codes,
            );
            prices[end - start] = group_bits;
        }
        range_bits.push(prices);
    }

    let next_boundary = choose_bounded_boundaries(&range_bits)?;

    let mut grouped = Vec::new();
    grouped.try_reserve_exact(blocks.len()).ok()?;
    let mut start = 0_usize;
    while start < blocks.len() {
        let end = next_boundary[start];
        if end > start + 1 {
            let mut winner = blocks[start].try_clone_shared()?;
            for block in &blocks[start + 1..end] {
                if !try_append_parsed_block(&mut winner, block) {
                    return None;
                }
            }
            let plan = plan_block_with_short_family_floor(&winner, 0, &structural_options);
            let predicted = range_bits[start][end - start]?;
            // The frequency score prices a concrete paired-tree candidate.
            // If materializing that token spelling failed under memory
            // pressure, abandon this optional layout instead of letting its
            // missing range hide a later profitable group.
            if plan.bits > predicted {
                return None;
            }
            winner.tokens = plan.tokens;
            winner.recount_frequencies();
            grouped.push(winner);
            start = end;
        } else {
            let mut single = blocks[start].try_clone_shared()?;
            single.tokens = source_plans[start].tokens.clone();
            single.recount_frequencies();
            grouped.push(single);
            start += 1;
        }
    }
    Some(grouped)
}

/// Choose the least-cost complete segmentation from bounded range prices.
///
/// Entry `prices[start][span]` is the cost of one block covering that source
/// range. Span one is always the complete per-source fallback; absent longer
/// entries simply are not legal edges. Working backward makes the otherwise
/// local range choices globally optimal within the fixed lookahead window.
fn choose_bounded_boundaries(
    prices: &[[Option<u64>; MAX_BOUNDED_GROUP_SPAN + 1]],
) -> Option<Vec<usize>> {
    let mut suffix_bits = Vec::new();
    suffix_bits
        .try_reserve_exact(prices.len().checked_add(1)?)
        .ok()?;
    suffix_bits.resize(prices.len() + 1, u64::MAX);
    suffix_bits[prices.len()] = 0;
    let mut next_boundary = Vec::new();
    next_boundary.try_reserve_exact(prices.len()).ok()?;
    next_boundary.resize(prices.len(), 0_usize);

    for start in (0..prices.len()).rev() {
        let mut best_bits = prices[start][1]?.checked_add(suffix_bits[start + 1])?;
        let mut best_end = start + 1;
        for span in 2..=MAX_BOUNDED_GROUP_SPAN.min(prices.len() - start) {
            let Some(bits) = prices[start][span] else {
                continue;
            };
            let total = bits.checked_add(suffix_bits[start + span])?;
            if total < best_bits {
                best_bits = total;
                best_end = start + span;
            }
        }
        suffix_bits[start] = best_bits;
        next_boundary[start] = best_end;
    }
    Some(next_boundary)
}

fn same_block_layout(left: &[ParsedBlock], right: &[ParsedBlock]) -> bool {
    left.len() == right.len()
        && left.iter().zip(right).all(|(left, right)| {
            left.tokens.len() == right.tokens.len() && left.plain.len() == right.plain.len()
        })
}

/// Collect adjacent Huffman blocks into bounded runs before rebuilding them.
///
/// This is deliberately a source-order fold, not a search over combinations:
/// a block is appended only while both cost guards remain satisfied. Stored
/// blocks break a run so this candidate retains the C planner's Huffman-run
/// grouping; stored accumulation is handled separately by [`prepare_blocks`].
fn collect_huffman_runs(blocks: &[ParsedBlock], wide: bool) -> Option<Vec<ParsedBlock>> {
    // Very long encoder-flush chains benefit from one broad source-order
    // collection before replay. It remains bounded by the same 250k-token /
    // 64MB limits as an ordinary adjacent merge, and the result still has to
    // beat the complete sequential fallback.
    let (token_limit, plain_limit) = if wide {
        (MAX_MERGED_TOKENS, MAX_MERGED_PLAIN)
    } else {
        (COLLECTED_RUN_MAX_TOKENS, COLLECTED_RUN_MAX_PLAIN)
    };
    collect_huffman_runs_with_limits(blocks, token_limit, plain_limit)
}

fn collect_huffman_runs_with_limits(
    blocks: &[ParsedBlock],
    token_limit: usize,
    plain_limit: usize,
) -> Option<Vec<ParsedBlock>> {
    if token_limit == 0
        || plain_limit == 0
        || payload_storage_bytes(blocks)?.checked_mul(3)? > MAX_GROUPED_MODEL_BYTES
    {
        return None;
    }
    let mut collected = Vec::<ParsedBlock>::new();

    for block in blocks {
        let can_append = collected.last().is_some_and(|pending| {
            pending.source_type != SourceBlockType::Stored
                && block.source_type != SourceBlockType::Stored
                && pending
                    .tokens
                    .len()
                    .checked_add(block.tokens.len())
                    .is_some_and(|tokens| tokens <= token_limit)
                && pending
                    .plain
                    .len()
                    .checked_add(block.plain.len())
                    .is_some_and(|plain| plain <= plain_limit)
        });
        if can_append {
            let pending = collected
                .last_mut()
                .expect("the append predicate observed a pending block");
            if !try_append_parsed_block(pending, block) {
                // Allocation failure only disables this optional grouping. The
                // unmerged source block remains a complete, lossless fallback.
                if collected.len() >= MAX_BOUNDARY_DP_CUTS {
                    return None;
                }
                collected.try_reserve(1).ok()?;
                collected.push(block.try_clone_shared()?);
            }
        } else {
            if collected.len() >= MAX_BOUNDARY_DP_CUTS {
                return None;
            }
            collected.try_reserve(1).ok()?;
            collected.push(block.try_clone_shared()?);
        }
    }

    Some(collected)
}

/// Build the independent 4,096-token candidate used by max-mode replay.
///
/// On some highly fragmented encoder streams, the ordinary 8,192-token floor
/// wins immediately while a slightly larger 4,096-token layout exposes match
/// pruning and split choices that win after several strict replays. Returning
/// its complete initial plan lets `optimize` replay it independently.
pub(crate) fn fragmented_collect_seed(
    blocks: &[ParsedBlock],
    start_alignment: u8,
    options: &Options,
) -> Option<Vec<PlannedBlock>> {
    if !options.exhaustive
        || blocks.len() < FRAGMENTED_COLLECT_MIN_SOURCE_BLOCKS
        || !(DEFAULT_LONG_MERGE_MIN..=DEFAULT_LONG_MERGE_MAX)
            .contains(&encoded_source_bytes(blocks))
        || blocks
            .iter()
            .any(|block| block.source_type == SourceBlockType::Stored)
    {
        return None;
    }

    let collected = collect_huffman_runs_with_limits(
        blocks,
        FRAGMENTED_COLLECT_MAX_TOKENS,
        COLLECTED_RUN_MAX_PLAIN,
    )?;
    if collected.len() >= blocks.len()
        || !(2..=MAX_REGROUP_SOURCE_BLOCKS).contains(&collected.len())
    {
        return None;
    }
    direct_structural_plan(&collected, start_alignment, options)
}

// One owned merged block is large but short-lived. Keeping it inline avoids an
// additional infallible heap allocation solely to represent borrowed-or-owned.
#[allow(clippy::large_enum_variant)]
enum PendingBlock<'a> {
    Borrowed(&'a ParsedBlock),
    Owned(ParsedBlock),
}

impl PendingBlock<'_> {
    fn as_block(&self) -> &ParsedBlock {
        match self {
            Self::Borrowed(block) => block,
            Self::Owned(block) => block,
        }
    }
}

/// Build the ordinary per-block result, joining consecutive fixed output plans
/// as DeflOpt does. Removing the first block's seven-bit end code and the next
/// block's three-bit header saves exactly ten bits.
fn sequential_plan<F>(
    blocks: &[ParsedBlock],
    start_alignment: u8,
    options: &Options,
    merge_search: AdjacentMergeSearch,
    expired: &mut F,
) -> Option<Vec<PlannedBlock>>
where
    F: FnMut() -> bool,
{
    let Some((first, rest)) = blocks.split_first() else {
        return Some(Vec::new());
    };
    let mut output = Vec::<PlannedBlock>::new();
    output.try_reserve_exact(blocks.len()).ok()?;
    let mut output_bits = 0_u64;
    let mut pending = PendingBlock::Borrowed(first);
    let mut pending_cache: Option<(u8, Vec<PlannedBlock>)> = None;

    for current in rest {
        let pending_block = pending.as_block();
        let alignment = ((u64::from(start_alignment) + output_bits) & 7) as u8;
        let pending_plans = match pending_cache.take() {
            Some((cached_alignment, plans)) if cached_alignment == alignment => plans,
            _ => plan_source_with_splits(pending_block, alignment, options, expired),
        };
        let pending_bits = total_bits(&pending_plans);
        let current_alignment = ((u64::from(alignment) + pending_bits) & 7) as u8;
        let current_plans = plan_source_with_splits(current, current_alignment, options, expired);
        let separate_bits = pending_bits + total_bits(&current_plans);

        let shared_tree = blocks_share_dynamic_tree(pending_block, current);
        let small_merge = pending_block.plain.len() + current.plain.len() <= 512;
        let merge_search_enabled = merge_search != AdjacentMergeSearch::Disabled;
        let long_huffman_merge = merge_search == AdjacentMergeSearch::LongRun
            && pending_block.source_type != SourceBlockType::Stored
            && current.source_type != SourceBlockType::Stored;
        let can_search_merge = merge_search_enabled
            && (small_merge || options.exhaustive || long_huffman_merge)
            && pending_block.tokens.len() + current.tokens.len() <= MAX_MERGED_TOKENS
            && pending_block.plain.len() + current.plain.len() <= MAX_MERGED_PLAIN
            && !expired();

        // A fixed/fixed join is exact and needs no Huffman search. Keep it as a
        // candidate even on streams outside the long-merge range.
        let fixed_join_eligible = pending_plans.len() == 1
            && current_plans.len() == 1
            && is_fixed_plan(&pending_plans[0])
            && is_fixed_plan(&current_plans[0]);
        let need_merged_block =
            can_search_merge || (merge_search_enabled && shared_tree) || fixed_join_eligible;
        let mut merged = need_merged_block
            .then(|| try_merge_parsed_blocks(pending_block, current))
            .flatten();
        let mut merged_winner = fixed_join_eligible
            .then(|| {
                let merged = merged.as_ref()?;
                Some(vec![PlannedBlock {
                    tokens: Arc::clone(&merged.tokens),
                    plain: Arc::clone(&merged.plain),
                    representation: Representation::Fixed,
                    bits: pending_plans[0]
                        .bits
                        .checked_add(current_plans[0].bits)?
                        .checked_sub(10)?,
                    source_type: merged.source_type,
                }])
            })
            .flatten();

        if let Some(merged_block) = can_search_merge.then_some(()).and(merged.as_ref()) {
            let candidate = plan_source_with_splits(merged_block, alignment, options, expired);
            if total_bits(&candidate) < separate_bits
                && merged_winner
                    .as_ref()
                    .map_or(true, |winner| total_bits(&candidate) < total_bits(winner))
            {
                merged_winner = Some(candidate);
            }
        }

        // Identical decoded trees need no rebuild. Reuse the first header and
        // score its codes over the concatenated tokens, removing one complete
        // dynamic header in a single inexpensive candidate.
        if let Some(merged_block) = (merge_search_enabled && shared_tree)
            .then_some(())
            .and(merged.as_ref())
        {
            if let Some(dynamic) = pending_block.original_dynamic.as_ref().and_then(|source| {
                score_existing_dynamic(&merged_block.tokens, source, options.min_distance_codes)
            }) {
                if dynamic.bits < separate_bits
                    && merged_winner
                        .as_ref()
                        .map_or(true, |winner| dynamic.bits < total_bits(winner))
                {
                    merged_winner = Some(vec![PlannedBlock {
                        tokens: merged_block.tokens.clone(),
                        plain: merged_block.plain.clone(),
                        bits: dynamic.bits,
                        representation: Representation::Dynamic(dynamic),
                        source_type: merged_block.source_type,
                    }]);
                }
            }
        }

        if let Some(winner) = merged_winner {
            let mut merged = merged
                .take()
                .expect("every accepted adjacent candidate owns a merged block");
            // Carry a winning single-block token replay into the next adjacent
            // merge, just as the C pending-block loop does.
            if winner.len() == 1 {
                merged.tokens = winner[0].tokens.clone();
                merged.recount_frequencies();
            }
            pending = PendingBlock::Owned(merged);
            pending_cache = Some((alignment, winner));
        } else {
            append_output_plans(&mut output, &mut output_bits, pending_plans)?;
            pending = PendingBlock::Borrowed(current);
            pending_cache = Some((current_alignment, current_plans));
        }
    }

    let alignment = ((u64::from(start_alignment) + output_bits) & 7) as u8;
    let pending_plans = match pending_cache {
        Some((cached_alignment, plans)) if cached_alignment == alignment => plans,
        _ => plan_source_with_splits(pending.as_block(), alignment, options, expired),
    };
    append_output_plans(&mut output, &mut output_bits, pending_plans)?;
    Some(output)
}

fn append_output_plans(
    output: &mut Vec<PlannedBlock>,
    output_bits: &mut u64,
    plans: Vec<PlannedBlock>,
) -> Option<()> {
    output.try_reserve(plans.len()).ok()?;
    let last_alignment_sensitive = plans
        .iter()
        .rposition(|plan| !plan_is_alignment_independent(plan));
    for (index, plan) in plans.into_iter().enumerate() {
        // Removing a fixed EOB/header shifts every following block by two bits.
        // Fixed and dynamic payloads are alignment-independent, but a stored
        // block's padding (including copied original stored bits) is not. Never
        // invalidate the alignment at which a later stored plan was priced.
        let suffix_is_alignment_independent =
            last_alignment_sensitive.map_or(true, |last| last <= index);
        append_output_plan(output, output_bits, plan, suffix_is_alignment_independent)?;
    }
    Some(())
}

/// Append one complete plan, optionally joining it to a fixed predecessor.
///
/// The caller decides whether later plans were priced at an alignment that a
/// ten-bit fixed join would invalidate. Reserving before removing the left
/// plan also leaves room to restore both plans if their optional payload copy
/// cannot be allocated.
fn append_output_plan(
    output: &mut Vec<PlannedBlock>,
    output_bits: &mut u64,
    plan: PlannedBlock,
    allow_fixed_join: bool,
) -> Option<()> {
    output.try_reserve(1).ok()?;
    if allow_fixed_join && output.last().is_some_and(is_fixed_plan) && is_fixed_plan(&plan) {
        let left = output.pop().expect("the fixed predecessor was just tested");
        *output_bits -= left.bits;
        if let Some(joined) = try_join_fixed_plans(left.clone(), plan.clone()) {
            *output_bits += joined.bits;
            output.push(joined);
        } else {
            // Joining is optional. Restore two independently valid blocks when
            // the combined payload cannot be allocated.
            *output_bits += left.bits + plan.bits;
            output.push(left);
            output.push(plan);
        }
    } else {
        *output_bits += plan.bits;
        output.push(plan);
    }
    Some(())
}

fn plan_is_alignment_independent(plan: &PlannedBlock) -> bool {
    match plan.representation {
        Representation::Stored => false,
        Representation::Original(original) => original.block_type != SourceBlockType::Stored,
        Representation::Fixed | Representation::Dynamic(_) => true,
    }
}

fn blocks_share_dynamic_tree(left: &ParsedBlock, right: &ParsedBlock) -> bool {
    match (&left.original_dynamic, &right.original_dynamic) {
        (Some(left), Some(right)) => {
            left.literal_lengths == right.literal_lengths
                && left.distance_lengths == right.distance_lengths
        }
        _ => false,
    }
}

/// Test the C planner's bounded one-boundary split route for one source block.
/// Default mode uses seven decoded eighths; `--max` also retains its compact
/// 32-token probes. Children use the direct block planner in default mode, as
/// they have not yet become pending merge candidates.
fn plan_source_with_splits<F>(
    block: &ParsedBlock,
    alignment: u8,
    options: &Options,
    expired: &mut F,
) -> Vec<PlannedBlock>
where
    F: FnMut() -> bool,
{
    if block.tokens.len() < 16
        || block.plain.len() < 128
        || (!options.exhaustive && block.plain.len() < 32_768)
    {
        return vec![plan_block_with_search(block, alignment, options, expired)];
    }

    let base = if options.exhaustive {
        plan_block(block, alignment, options, &mut *expired)
    } else {
        // Default mode retains its established whole-block token search before
        // the seven inexpensive eighth probes.
        plan_block_with_search(block, alignment, options, expired)
    };
    let mut best = vec![base];
    let mut best_bits = total_bits(&best);

    let Some(composite) = Composite::new(std::slice::from_ref(block)) else {
        return best;
    };
    let source = composite.sources[0];
    let mut cuts = Vec::new();
    if add_eighth_cuts(
        &mut cuts,
        &composite,
        source.token_start,
        source.token_end,
        source.plain_start,
        source.plain_end,
    )
    .is_none()
    {
        return best;
    }
    if options.exhaustive && block.tokens.len() <= 512 {
        for token in (32..block.tokens.len().saturating_sub(32)).step_by(32) {
            if add_cut(&mut cuts, &composite, token).is_none() {
                return best;
            }
        }
    }
    cuts.sort_unstable_by_key(|cut| cut.token);
    cuts.dedup_by_key(|cut| cut.token);
    cuts.truncate(24);

    // Whole-block max search includes several match-state beams. On compact
    // blocks it can consume the complete deadline before a useful late split
    // (the 10th or 11th 32-token probe is common in sprite data) is reached.
    // Price every bounded boundary with direct structural planning first.
    let start = Cut { token: 0, plain: 0 };
    let end = Cut {
        token: block.tokens.len(),
        plain: block.plain.len(),
    };
    let mut ranked_splits = Vec::with_capacity(cuts.len());
    for &split in &cuts {
        if expired() {
            break;
        }
        let Some(left) = make_range(&composite, start, split) else {
            continue;
        };
        let left_plan = plan_block(&left, alignment, options, &mut *expired);
        if expired() {
            break;
        }
        let right_alignment = ((u64::from(alignment) + left_plan.bits) & 7) as u8;
        let Some(right) = make_range(&composite, split, end) else {
            continue;
        };
        let right_plan = plan_block(&right, right_alignment, options, &mut *expired);
        let candidate_bits = left_plan.bits + right_plan.bits;
        ranked_splits.push((candidate_bits, split));
        if candidate_bits < best_bits {
            best_bits = candidate_bits;
            best = vec![left_plan, right_plan];
        }
    }

    ranked_splits.sort_unstable_by_key(|&(bits, split)| (bits, split.token));
    if !options.exhaustive {
        // A locally second-best first split can expose the best stream after
        // its larger child is split again. The winning first split already gets
        // this opportunity through whole-stream replay, so following only its
        // runner-up avoids repeating that work in default mode.
        if ranked_splits.len() >= 2
            && ranked_splits[1].0
                <= ranked_splits[0]
                    .0
                    .saturating_add(NESTED_RUNNER_UP_MARGIN_BITS)
        {
            let outer = ranked_splits[1].1;
            let left_plain = outer.plain - start.plain;
            let right_plain = end.plain - outer.plain;
            let (child_start, child_end) = if left_plain >= right_plain {
                (start, outer)
            } else {
                (outer, end)
            };
            if expired() {
                return best;
            }
            if let Some(inner) = midpoint_cut(&composite, child_start, child_end) {
                let boundaries = if inner.token < outer.token {
                    [start, inner, outer, end]
                } else {
                    [start, outer, inner, end]
                };
                let Some(candidate) =
                    plan_structural_ranges(&composite, &boundaries, alignment, options, expired)
                else {
                    return best;
                };
                let candidate_bits = total_bits(&candidate);
                if candidate_bits < best_bits {
                    best = candidate;
                }
            }
        }
        return best;
    }
    if expired() {
        return best;
    }

    // With the complete structural floor secured, search the whole block and
    // refine promising split children while time remains. Ranking by direct
    // cost makes max mode deterministic and gives plausible boundaries the
    // first search slots.
    let searched_base = plan_block_with_search(block, alignment, options, expired);
    if searched_base.bits < best_bits {
        best_bits = searched_base.bits;
        best = vec![searched_base];
    }
    for (_, split) in ranked_splits {
        if expired() {
            break;
        }
        let Some(left) = make_range(&composite, start, split) else {
            continue;
        };
        let left_plan = plan_block_with_search(&left, alignment, options, expired);
        if expired() {
            break;
        }
        let right_alignment = ((u64::from(alignment) + left_plan.bits) & 7) as u8;
        let Some(right) = make_range(&composite, split, end) else {
            continue;
        };
        let right_plan = plan_block_with_search(&right, right_alignment, options, expired);
        let candidate_bits = left_plan.bits + right_plan.bits;
        if candidate_bits < best_bits {
            best_bits = candidate_bits;
            best = vec![left_plan, right_plan];
        }
    }
    best
}

/// Return the legal token boundary immediately before a decoded midpoint.
fn midpoint_cut(composite: &Composite, start: Cut, end: Cut) -> Option<Cut> {
    if end.token <= start.token + 1 || end.plain <= start.plain + 1 {
        return None;
    }
    let target = start.plain + (end.plain - start.plain) / 2;
    let insertion = composite.token_plain_offsets[start.token..=end.token]
        .partition_point(|&offset| offset <= target);
    let token = start.token + insertion.saturating_sub(1);
    (token > start.token && token < end.token).then(|| Cut {
        token,
        plain: composite.token_plain_offsets[token],
    })
}

/// Directly plan consecutive ranges while carrying their exact bit alignment.
fn plan_structural_ranges<F>(
    composite: &Composite,
    boundaries: &[Cut],
    mut alignment: u8,
    options: &Options,
    expired: &mut F,
) -> Option<Vec<PlannedBlock>>
where
    F: FnMut() -> bool,
{
    let mut plans = Vec::new();
    plans
        .try_reserve_exact(boundaries.len().saturating_sub(1))
        .ok()?;
    for pair in boundaries.windows(2) {
        if expired() {
            return None;
        }
        let range = make_range(composite, pair[0], pair[1])?;
        let plan = plan_block(&range, alignment, options, &mut *expired);
        alignment = ((u64::from(alignment) + plan.bits) & 7) as u8;
        plans.push(plan);
    }
    Some(plans)
}

fn is_fixed_plan(plan: &PlannedBlock) -> bool {
    match plan.representation {
        Representation::Fixed => true,
        Representation::Original(original) => original.block_type == SourceBlockType::Fixed,
        _ => false,
    }
}

fn try_join_fixed_plans(mut left: PlannedBlock, right: PlannedBlock) -> Option<PlannedBlock> {
    if !try_prepare_shared_append(&mut left.tokens, right.tokens.len())
        || !try_prepare_shared_append(&mut left.plain, right.plain.len())
    {
        return None;
    }
    Arc::get_mut(&mut left.tokens)?.extend_from_slice(&right.tokens);
    Arc::get_mut(&mut left.plain)?.extend_from_slice(&right.plain);
    left.bits = left
        .bits
        .checked_add(right.bits)
        .and_then(|bits| bits.checked_sub(10))
        .expect("two complete fixed blocks contain a header and end code");
    left.representation = Representation::Fixed;
    left.source_type = merged_source_type(left.source_type, right.source_type);
    Some(left)
}

#[derive(Debug, Clone, Copy)]
struct SourceSpan {
    token_start: usize,
    token_end: usize,
    plain_start: usize,
    plain_end: usize,
    source_type: SourceBlockType,
}

/// Concatenated view used only while evaluating boundary positions.
struct Composite<'a> {
    tokens: Cow<'a, [Token]>,
    plain: Cow<'a, [u8]>,
    /// Decoded offset at every token boundary, including the final boundary.
    token_plain_offsets: Vec<usize>,
    sources: Vec<SourceSpan>,
}

impl<'a> Composite<'a> {
    fn new(blocks: &'a [ParsedBlock]) -> Option<Self> {
        let token_count = blocks.iter().try_fold(0_usize, |total, block| {
            total.checked_add(block.tokens.len())
        })?;
        let plain_count = blocks
            .iter()
            .try_fold(0_usize, |total, block| total.checked_add(block.plain.len()))?;

        // At this point the sequential fallback may already retain a grouped
        // payload. Conservatively budget three complete payload views for the
        // concatenated composite, DP edge/range plans, and reconstructed winner.
        let payload_bytes = token_count
            .checked_mul(std::mem::size_of::<Token>())?
            .checked_add(plain_count)?;
        let offset_bytes = token_count
            .checked_add(1)?
            .checked_mul(std::mem::size_of::<usize>())?;
        let source_bytes = blocks
            .len()
            .checked_mul(std::mem::size_of::<SourceSpan>())?;
        let optional_bytes = payload_bytes
            .checked_mul(3)?
            .checked_add(offset_bytes)?
            .checked_add(source_bytes)?;
        if optional_bytes > MAX_COMPOSITE_MODEL_BYTES {
            return None;
        }

        let (tokens, plain) = if let [block] = blocks {
            (
                Cow::Borrowed(block.tokens.as_slice()),
                Cow::Borrowed(block.plain.as_slice()),
            )
        } else {
            let mut tokens = Vec::new();
            let mut plain = Vec::new();
            tokens.try_reserve_exact(token_count).ok()?;
            plain.try_reserve_exact(plain_count).ok()?;
            for block in blocks {
                tokens.extend_from_slice(&block.tokens);
                plain.extend_from_slice(&block.plain);
            }
            (Cow::Owned(tokens), Cow::Owned(plain))
        };

        let mut sources = Vec::new();
        sources.try_reserve_exact(blocks.len()).ok()?;
        let mut token_end = 0_usize;
        let mut plain_end = 0_usize;

        for block in blocks {
            let token_start = token_end;
            let plain_start = plain_end;
            token_end += block.tokens.len();
            plain_end += block.plain.len();
            sources.push(SourceSpan {
                token_start,
                token_end,
                plain_start,
                plain_end,
                source_type: block.source_type,
            });
        }

        let mut token_plain_offsets = Vec::new();
        token_plain_offsets
            .try_reserve_exact(tokens.len().checked_add(1)?)
            .ok()?;
        token_plain_offsets.push(0);
        for token in tokens.iter() {
            let next = token_plain_offsets
                .last()
                .copied()
                .expect("the initial boundary exists")
                + token.decoded_len();
            token_plain_offsets.push(next);
        }
        debug_assert_eq!(token_plain_offsets.last().copied(), Some(plain.len()));

        Some(Self {
            tokens,
            plain,
            token_plain_offsets,
            sources,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Cut {
    token: usize,
    plain: usize,
}

/// Candidate boundaries are source boundaries plus DeflOpt's seven eighth
/// probes. An eighth that falls inside a match snaps to the preceding token
/// boundary: this is important because the match itself must remain intact.
fn choose_cuts(composite: &Composite, exhaustive: bool, allow_regroup: bool) -> Option<Vec<Cut>> {
    let mut cuts = Vec::new();
    add_cut(&mut cuts, composite, 0)?;
    add_cut(&mut cuts, composite, composite.tokens.len())?;

    for source in &composite.sources {
        add_cut(&mut cuts, composite, source.token_start)?;
        add_cut(&mut cuts, composite, source.token_end)?;
        let token_count = source.token_end - source.token_start;
        let plain_count = source.plain_end - source.plain_start;
        if token_count >= 16 && plain_count >= 128 && (exhaustive || plain_count >= 32_768) {
            add_eighth_cuts(
                &mut cuts,
                composite,
                source.token_start,
                source.token_end,
                source.plain_start,
                source.plain_end,
            )?;
        }
        if exhaustive && token_count <= 512 {
            for token in (source.token_start + 32..source.token_end.saturating_sub(32)).step_by(32)
            {
                add_cut(&mut cuts, composite, token)?;
            }
        }
    }

    // The default long-merge candidate first joins a Huffman run and then
    // applies the same eighth probes to that combined token stream. Computing
    // cuts over the combined decoded range allows a profitable boundary to
    // move away from every original encoder flush point.
    if allow_regroup && composite.sources.len() <= MAX_REGROUP_SOURCE_BLOCKS {
        for (run_start, run_end) in huffman_runs(&composite.sources)? {
            let first = composite.sources[run_start];
            let last = composite.sources[run_end - 1];
            add_eighth_cuts(
                &mut cuts,
                composite,
                first.token_start,
                last.token_end,
                first.plain_start,
                last.plain_end,
            )?;

            // Pair/group cuts make the union useful when the winning stream
            // retains some source boundaries but regroups their neighbours.
            for start in run_start..run_end {
                for end in start + 2..=run_end {
                    let first = composite.sources[start];
                    let last = composite.sources[end - 1];
                    add_eighth_cuts(
                        &mut cuts,
                        composite,
                        first.token_start,
                        last.token_end,
                        first.plain_start,
                        last.plain_end,
                    )?;
                }
            }
        }
    }

    cuts.sort_unstable_by_key(|cut| cut.token);
    cuts.dedup_by_key(|cut| cut.token);
    Some(cuts)
}

fn add_eighth_cuts(
    cuts: &mut Vec<Cut>,
    composite: &Composite,
    token_start: usize,
    token_end: usize,
    plain_start: usize,
    plain_end: usize,
) -> Option<()> {
    if token_end - token_start < 16 || plain_end - plain_start < 128 {
        return Some(());
    }
    let plain_len = plain_end - plain_start;
    for eighth in 1..8 {
        let target = plain_start + plain_len * eighth / 8;
        // DeflOpt chooses the boundary before the token containing the target.
        // Its interval test is inclusive at the token's decoded end, so even
        // an exact endpoint retains the strictly preceding boundary.
        let insertion = composite.token_plain_offsets[token_start..=token_end]
            .partition_point(|&offset| offset < target);
        if insertion == 0 {
            continue;
        }
        let token = token_start + insertion - 1;
        if token > token_start && token < token_end {
            add_cut(cuts, composite, token)?;
        }
    }
    Some(())
}

fn add_cut(cuts: &mut Vec<Cut>, composite: &Composite, token: usize) -> Option<()> {
    if let Some(&plain) = composite.token_plain_offsets.get(token) {
        // Returning `None` only abandons this optional boundary search. The
        // complete sequential plan built before it remains available.
        if cuts.len() >= MAX_BOUNDARY_DP_CUTS {
            return None;
        }
        cuts.try_reserve(1).ok()?;
        cuts.push(Cut { token, plain });
    }
    Some(())
}

fn huffman_runs(sources: &[SourceSpan]) -> Option<Vec<(usize, usize)>> {
    let mut runs = Vec::new();
    runs.try_reserve(sources.len()).ok()?;
    let mut start = 0;
    while start < sources.len() {
        if sources[start].source_type == SourceBlockType::Stored {
            start += 1;
            continue;
        }
        let mut end = start + 1;
        while end < sources.len() && sources[end].source_type != SourceBlockType::Stored {
            end += 1;
        }
        if end - start >= 2 {
            runs.push((start, end));
        }
        start = end;
    }
    Some(runs)
}

#[derive(Clone)]
struct PlanTemplate {
    representation: Representation,
    bits: u64,
    source_type: SourceBlockType,
}

impl PlanTemplate {
    fn from_planned(plan: PlannedBlock) -> Self {
        Self {
            representation: plan.representation,
            bits: plan.bits,
            source_type: plan.source_type,
        }
    }

    fn instantiate(&self, composite: &Composite, start: Cut, end: Cut) -> Option<PlannedBlock> {
        Some(PlannedBlock {
            tokens: try_shared_slice(&composite.tokens[start.token..end.token])?,
            plain: try_shared_slice(&composite.plain[start.plain..end.plain])?,
            representation: self.representation.try_clone()?,
            bits: self.bits,
            source_type: self.source_type,
        })
    }
}

#[derive(Clone)]
struct Previous {
    cut: usize,
    alignment: u8,
    plan: PlanTemplate,
}

#[derive(Clone)]
struct DpNode {
    bits: u64,
    previous: Option<Previous>,
}

#[allow(clippy::too_many_arguments)]
fn boundary_dp<F>(
    blocks: &[ParsedBlock],
    composite: &Composite,
    cuts: &[Cut],
    start_alignment: u8,
    options: &Options,
    allow_regroup: bool,
    expired: &mut F,
) -> Option<Vec<PlannedBlock>>
where
    F: FnMut() -> bool,
{
    if cuts.len() > MAX_BOUNDARY_DP_CUTS {
        return None;
    }
    let mut dp = Vec::<[Option<DpNode>; 8]>::new();
    dp.try_reserve_exact(cuts.len()).ok()?;
    for _ in cuts {
        dp.push(std::array::from_fn(|_| None));
    }
    dp[0][usize::from(start_alignment)] = Some(DpNode {
        bits: 0,
        previous: None,
    });

    for start_index in 0..cuts.len() - 1 {
        if expired() {
            return None;
        }
        // There are exactly eight possible starting bit alignments, so a
        // fixed array avoids a small heap allocation at every cut.
        let mut reachable = [(0_u8, 0_u64); 8];
        let mut reachable_len = 0;
        for (alignment, node) in dp[start_index].iter().enumerate() {
            if let Some(node) = node {
                reachable[reachable_len] = (alignment as u8, node.bits);
                reachable_len += 1;
            }
        }
        if reachable_len == 0 {
            continue;
        }

        for end_index in start_index + 1..cuts.len() {
            let start = cuts[start_index];
            let end = cuts[end_index];
            if !edge_allowed(composite, start, end, options.exhaustive, allow_regroup) {
                continue;
            }

            for &(alignment, prefix_bits) in &reachable[..reachable_len] {
                if expired() {
                    return None;
                }
                let Some(template) =
                    plan_edge(blocks, composite, start, end, alignment, options, expired)
                else {
                    continue;
                };
                let Some(bits) = prefix_bits.checked_add(template.bits) else {
                    continue;
                };
                let next_alignment = ((u64::from(start_alignment) + bits) & 7) as u8;
                let destination = &mut dp[end_index][usize::from(next_alignment)];
                if destination.as_ref().map_or(true, |old| bits < old.bits) {
                    *destination = Some(DpNode {
                        bits,
                        previous: Some(Previous {
                            cut: start_index,
                            alignment,
                            plan: template,
                        }),
                    });
                }
            }
        }
    }

    let end_index = cuts.len() - 1;
    let (mut alignment, _) = dp[end_index]
        .iter()
        .enumerate()
        .filter_map(|(alignment, node)| node.as_ref().map(|node| (alignment as u8, node.bits)))
        .min_by_key(|&(_, bits)| bits)?;

    let mut at = end_index;
    let mut plans = Vec::new();
    plans.try_reserve_exact(cuts.len().saturating_sub(1)).ok()?;
    while at != 0 {
        let previous = dp[at][usize::from(alignment)].as_ref()?.previous.as_ref()?;
        plans.push(
            previous
                .plan
                .instantiate(composite, cuts[previous.cut], cuts[at])?,
        );
        at = previous.cut;
        alignment = previous.alignment;
    }
    plans.reverse();
    Some(plans)
}

fn edge_allowed(
    composite: &Composite,
    start: Cut,
    end: Cut,
    exhaustive: bool,
    allow_regroup: bool,
) -> bool {
    if start.token >= end.token || start.plain >= end.plain {
        return false;
    }
    let source_range = overlapping_source_range(composite, start, end);
    let sources = &composite.sources[source_range];
    let Some((first, rest)) = sources.split_first() else {
        return false;
    };
    if rest.is_empty() {
        // The default C route tests one boundary at a time: a candidate is a
        // prefix or suffix of its source block, not an arbitrary middle slice.
        // --max may combine several remembered cuts through the full DP.
        return exhaustive || start.token == first.token_start || end.token == first.token_end;
    }

    let token_count = end.token - start.token;
    let plain_count = end.plain - start.plain;
    if token_count > MAX_MERGED_TOKENS || plain_count > MAX_MERGED_PLAIN {
        return false;
    }
    let all_stored = sources
        .iter()
        .all(|source| source.source_type == SourceBlockType::Stored);
    let all_huffman = sources
        .iter()
        .all(|source| source.source_type != SourceBlockType::Stored);
    let whole_sources = start.token == first.token_start
        && end.token == sources.last().expect("the range is nonempty").token_end;

    if all_stored {
        return whole_sources && plain_count <= 65_535;
    }
    if !all_huffman {
        return false;
    }

    // Cross-boundary partial ranges are the regroup search. Bound that search
    // to short source runs; source-aligned fixed/shared-tree joins remain cheap.
    if exhaustive || (allow_regroup && composite.sources.len() <= MAX_REGROUP_SOURCE_BLOCKS) {
        if exhaustive {
            return true;
        }
        // Default mode follows C's single-split merge route. At least one end
        // of a cross-source segment stays anchored to an original boundary;
        // free-floating middle ranges belong to the broader --max DP.
        return sources
            .iter()
            .any(|source| start.token == source.token_start || end.token == source.token_end);
    }
    if whole_sources && plain_count <= 512 {
        return true;
    }
    whole_sources
        && sources
            .iter()
            .all(|source| source.source_type == SourceBlockType::Fixed)
}

/// Return the contiguous source-span range touched by a token interval.
///
/// Source spans are ordered and non-overlapping, so two binary searches avoid
/// allocating and filtering a fresh index vector for every boundary-DP edge.
fn overlapping_source_range(composite: &Composite, start: Cut, end: Cut) -> std::ops::Range<usize> {
    let first = composite
        .sources
        .partition_point(|source| source.token_end <= start.token);
    let past_last = composite
        .sources
        .partition_point(|source| source.token_start < end.token);
    first..past_last
}

#[allow(clippy::too_many_arguments)]
fn plan_edge<F>(
    blocks: &[ParsedBlock],
    composite: &Composite,
    start: Cut,
    end: Cut,
    alignment: u8,
    options: &Options,
    expired: &mut F,
) -> Option<PlanTemplate>
where
    F: FnMut() -> bool,
{
    if let Some(source_index) = exact_source(composite, start, end) {
        let plan = plan_block(&blocks[source_index], alignment, options, expired);
        return Some(PlanTemplate::from_planned(plan));
    }

    let range = make_range(composite, start, end)?;
    let mut planned = plan_block(&range, alignment, options, &mut *expired);

    let source_range = overlapping_source_range(composite, start, end);
    let source_spans = &composite.sources[source_range.clone()];
    let whole_sources = source_spans.first().is_some_and(|first| {
        start.token == first.token_start
            && end.token == source_spans.last().expect("first was present").token_end
    });

    // Boundary DP is deliberately token-preserving. Match-to-literal spelling
    // searches run in the complete sequential pass and again after every
    // accepted structural replay; retaining them in each DP state would make
    // memory proportional to cuts × alignments rather than to the input.

    if whole_sources && source_spans.len() > 1 {
        if let Some(shared) = shared_dynamic_plan(
            &blocks[source_range],
            &range.tokens,
            options.min_distance_codes,
        ) {
            if shared.bits < planned.bits {
                let bits = shared.bits;
                planned = PlannedBlock {
                    tokens: range.tokens.clone(),
                    plain: range.plain.clone(),
                    representation: Representation::Dynamic(shared),
                    bits,
                    source_type: range.source_type,
                };
            }
        }
    }

    Some(PlanTemplate::from_planned(planned))
}

fn exact_source(composite: &Composite, start: Cut, end: Cut) -> Option<usize> {
    let range = overlapping_source_range(composite, start, end);
    if range.len() != 1 {
        return None;
    }
    let index = range.start;
    let source = composite.sources[index];
    (source.token_start == start.token
        && source.token_end == end.token
        && source.plain_start == start.plain
        && source.plain_end == end.plain)
        .then_some(index)
}

fn make_range(composite: &Composite, start: Cut, end: Cut) -> Option<ParsedBlock> {
    let tokens = try_shared_slice(&composite.tokens[start.token..end.token])?;
    let (literal_frequencies, distance_frequencies) = count_frequencies(&tokens);
    let source_range = overlapping_source_range(composite, start, end);
    let source_type = composite.sources[source_range]
        .iter()
        .map(|source| source.source_type)
        .reduce(merged_source_type)
        .unwrap_or(SourceBlockType::Dynamic);
    let split_count = composite
        .sources
        .iter()
        .filter(|source| source.plain_end > start.plain && source.plain_end < end.plain)
        .count();
    let mut source_splits = Vec::new();
    source_splits.try_reserve_exact(split_count).ok()?;
    // Filter before subtracting: eagerly forming the offset for an earlier
    // source would underflow even though it is outside this range.
    source_splits.extend(
        composite
            .sources
            .iter()
            .filter(|source| source.plain_end > start.plain && source.plain_end < end.plain)
            .map(|source| source.plain_end - start.plain),
    );

    Some(ParsedBlock {
        tokens,
        plain: try_shared_slice(&composite.plain[start.plain..end.plain])?,
        literal_frequencies,
        distance_frequencies,
        original_literal_lengths: None,
        original_distance_lengths: None,
        original_dynamic: None,
        original: None,
        source_splits,
        source_type,
    })
}

fn shared_dynamic_plan(
    blocks: &[ParsedBlock],
    tokens: &[Token],
    min_distance_codes: bool,
) -> Option<DynamicPlan> {
    let (first_block, rest) = blocks.split_first()?;
    let first = first_block.original_dynamic.as_ref()?;
    if !rest.iter().all(|block| {
        block.original_dynamic.as_ref().is_some_and(|plan| {
            plan.literal_lengths == first.literal_lengths
                && plan.distance_lengths == first.distance_lengths
        })
    }) {
        return None;
    }
    score_existing_dynamic(tokens, first, min_distance_codes)
}

/// Append one parsed block without repeatedly copying the accumulated prefix.
///
/// Preparation and collection are optional planning routes, so an allocation
/// failure simply leaves the blocks separate. All length/frequency arithmetic
/// and reservations are completed before the model itself is changed.
fn try_append_parsed_block(left: &mut ParsedBlock, right: &ParsedBlock) -> bool {
    debug_assert!(left.source_splits.windows(2).all(|pair| pair[0] < pair[1]));
    debug_assert!(right.source_splits.windows(2).all(|pair| pair[0] < pair[1]));
    let Some(token_len) = left.tokens.len().checked_add(right.tokens.len()) else {
        return false;
    };
    let Some(plain_len) = left.plain.len().checked_add(right.plain.len()) else {
        return false;
    };
    let add_boundary = usize::from(!left.plain.is_empty() && !right.plain.is_empty());
    let Some(additional_splits) = right.source_splits.len().checked_add(add_boundary) else {
        return false;
    };
    if left
        .source_splits
        .len()
        .checked_add(additional_splits)
        .is_none()
        || right
            .source_splits
            .iter()
            .any(|&split| left.plain.len().checked_add(split).is_none())
    {
        return false;
    }

    // A merged block keeps the left EOB and adds only the right payload
    // symbols. Computing this before mutation also makes overflow a clean
    // reason to abandon the optional merge.
    let mut literal_frequencies = left.literal_frequencies;
    let mut distance_frequencies = left.distance_frequencies;
    for &token in right.tokens.iter() {
        let frequency = match token {
            Token::Literal(value) => &mut literal_frequencies[usize::from(value)],
            Token::Match {
                length_symbol,
                distance_symbol,
                ..
            } => {
                let distance = &mut distance_frequencies[usize::from(distance_symbol)];
                let Some(updated) = distance.checked_add(1) else {
                    return false;
                };
                *distance = updated;
                &mut literal_frequencies[usize::from(length_symbol)]
            }
        };
        let Some(updated) = frequency.checked_add(1) else {
            return false;
        };
        *frequency = updated;
    }

    let additional_tokens = token_len - left.tokens.len();
    let additional_plain = plain_len - left.plain.len();
    if !try_prepare_shared_append(&mut left.tokens, additional_tokens)
        || !try_prepare_shared_append(&mut left.plain, additional_plain)
        || left.source_splits.try_reserve(additional_splits).is_err()
    {
        return false;
    }

    let left_plain_len = left.plain.len();
    let keeps_shared_dynamic = match (&left.original_dynamic, &right.original_dynamic) {
        (Some(left_plan), Some(right_plan)) => {
            left_plan.literal_lengths == right_plan.literal_lengths
                && left_plan.distance_lengths == right_plan.distance_lengths
        }
        _ => false,
    };
    Arc::get_mut(&mut left.tokens)
        .expect("the payload was made unique before append")
        .extend_from_slice(&right.tokens);
    Arc::get_mut(&mut left.plain)
        .expect("the payload was made unique before append")
        .extend_from_slice(&right.plain);
    if add_boundary != 0 {
        left.source_splits.push(left_plain_len);
    }
    left.source_splits.extend(
        right
            .source_splits
            .iter()
            .map(|&split| left_plain_len + split),
    );
    debug_assert!(left.source_splits.windows(2).all(|pair| pair[0] < pair[1]));
    left.literal_frequencies = literal_frequencies;
    left.distance_frequencies = distance_frequencies;
    left.original = None;
    if !keeps_shared_dynamic {
        left.original_literal_lengths = None;
        left.original_distance_lengths = None;
        left.original_dynamic = None;
    }
    left.source_type = merged_source_type(left.source_type, right.source_type);
    true
}

/// Make a shared immutable vector uniquely mutable and reserve an append.
///
/// `Arc::make_mut` performs an infallible clone. This explicit equivalent uses
/// `try_reserve_exact`, allowing optional merge routes to fail closed while
/// preserving the source payload unchanged.
fn try_prepare_shared_append<T: Clone>(shared: &mut Arc<Vec<T>>, additional: usize) -> bool {
    let Some(required) = shared.len().checked_add(additional) else {
        return false;
    };
    if Arc::get_mut(shared).is_none() {
        let mut owned = Vec::new();
        if owned.try_reserve_exact(required).is_err() {
            return false;
        }
        owned.extend_from_slice(shared);
        *shared = Arc::new(owned);
    }
    Arc::get_mut(shared)
        .expect("the shared vector was made unique")
        .try_reserve(additional)
        .is_ok()
}

fn try_shared_slice<T: Clone>(source: &[T]) -> Option<Arc<Vec<T>>> {
    let mut owned = Vec::new();
    owned.try_reserve_exact(source.len()).ok()?;
    owned.extend_from_slice(source);
    Some(Arc::new(owned))
}

fn try_concat_shared<T: Clone>(left: &[T], right: &[T]) -> Option<Arc<Vec<T>>> {
    let total = left.len().checked_add(right.len())?;
    let mut joined = Vec::new();
    joined.try_reserve_exact(total).ok()?;
    joined.extend_from_slice(left);
    joined.extend_from_slice(right);
    Some(Arc::new(joined))
}

fn try_merge_parsed_blocks(left: &ParsedBlock, right: &ParsedBlock) -> Option<ParsedBlock> {
    let tokens = try_concat_shared(&left.tokens, &right.tokens)?;
    let plain = try_concat_shared(&left.plain, &right.plain)?;
    let (literal_frequencies, distance_frequencies) = count_frequencies(&tokens);

    let add_boundary = usize::from(!left.plain.is_empty() && !right.plain.is_empty());
    let split_count = left
        .source_splits
        .len()
        .checked_add(right.source_splits.len())?
        .checked_add(add_boundary)?;
    let mut source_splits = Vec::new();
    source_splits.try_reserve_exact(split_count).ok()?;
    source_splits.extend_from_slice(&left.source_splits);
    if !left.plain.is_empty() && !right.plain.is_empty() {
        source_splits.push(left.plain.len());
    }
    for &split in &right.source_splits {
        source_splits.push(left.plain.len().checked_add(split)?);
    }
    source_splits.sort_unstable();
    source_splits.dedup();

    let shared_dynamic = match (&left.original_dynamic, &right.original_dynamic) {
        (Some(left_plan), Some(right_plan))
            if left_plan.literal_lengths == right_plan.literal_lengths
                && left_plan.distance_lengths == right_plan.distance_lengths =>
        {
            Some(left_plan.try_clone()?)
        }
        _ => None,
    };
    let (original_literal_lengths, original_distance_lengths) = if shared_dynamic.is_some() {
        (
            left.original_literal_lengths,
            left.original_distance_lengths,
        )
    } else {
        (None, None)
    };

    Some(ParsedBlock {
        tokens,
        plain,
        literal_frequencies,
        distance_frequencies,
        original_literal_lengths,
        original_distance_lengths,
        original_dynamic: shared_dynamic,
        original: None,
        source_splits,
        source_type: merged_source_type(left.source_type, right.source_type),
    })
}

fn merged_source_type(left: SourceBlockType, right: SourceBlockType) -> SourceBlockType {
    if left == right {
        left
    } else if left != SourceBlockType::Stored && right != SourceBlockType::Stored {
        SourceBlockType::Dynamic
    } else {
        // Mixed stored/Huffman ranges are not currently admitted by the DP.
        // Dynamic is the least surprising provenance if this helper is reused.
        SourceBlockType::Dynamic
    }
}

fn encoded_source_bytes(blocks: &[ParsedBlock]) -> u64 {
    let bits: u64 = blocks
        .iter()
        .filter_map(|block| block.original.map(|original| original.len))
        .sum();
    bits.div_ceil(8)
}

fn total_bits(plans: &[PlannedBlock]) -> u64 {
    plans.iter().map(|plan| plan.bits).sum()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::deflate::bitstream::BitWriter;
    use crate::deflate::block::{emit_block, stored_block_bits};
    use crate::deflate::model::OriginalBits;
    use crate::deflate::parse::parse_stream;

    fn literal_block(bytes: &[u8], source_type: SourceBlockType) -> ParsedBlock {
        let tokens: Vec<_> = bytes.iter().copied().map(Token::Literal).collect();
        let (literal_frequencies, distance_frequencies) = count_frequencies(&tokens);
        ParsedBlock {
            tokens: tokens.into(),
            plain: bytes.to_vec().into(),
            literal_frequencies,
            distance_frequencies,
            original_literal_lengths: None,
            original_distance_lengths: None,
            original_dynamic: None,
            original: None,
            source_splits: Vec::new(),
            source_type,
        }
    }

    #[test]
    fn redundant_empty_blocks_are_removed() {
        let empty = literal_block(&[], SourceBlockType::Fixed);
        let content = literal_block(b"one more thing", SourceBlockType::Fixed);
        let prepared = prepare_blocks(&[empty.clone(), content.clone(), empty]).unwrap();
        assert_eq!(prepared.len(), 1);
        assert_eq!(prepared[0].plain, content.plain);
    }

    #[test]
    fn an_empty_stream_keeps_one_legal_block() {
        let empty = literal_block(&[], SourceBlockType::Fixed);
        let prepared = prepare_blocks(&[empty.clone(), empty]).unwrap();
        assert_eq!(prepared.len(), 1);
        assert!(prepared[0].plain.is_empty());
    }

    #[test]
    fn adjacent_stored_chunks_are_accumulated() {
        let left = literal_block(&vec![1; 20_000], SourceBlockType::Stored);
        let right = literal_block(&vec![2; 30_000], SourceBlockType::Stored);
        let prepared = prepare_blocks(&[left, right]).unwrap();
        assert_eq!(prepared.len(), 1);
        assert_eq!(prepared[0].plain.len(), 50_000);
        assert_eq!(prepared[0].source_splits, [20_000]);
    }

    #[test]
    fn ordinary_planning_shares_unchanged_payload_buffers() {
        let source = literal_block(b"immutable payload", SourceBlockType::Dynamic);
        assert!(prepare_blocks(std::slice::from_ref(&source)).is_none());

        let plan = plan_block(&source, 0, &Options::default(), || false);
        assert!(Arc::ptr_eq(&plan.tokens, &source.tokens));
        assert!(Arc::ptr_eq(&plan.plain, &source.plain));
    }

    #[test]
    fn stored_accumulation_respects_the_wire_limit() {
        let left = literal_block(&vec![1; 40_000], SourceBlockType::Stored);
        let right = literal_block(&vec![2; 30_000], SourceBlockType::Stored);
        assert!(prepare_blocks(&[left, right]).is_none());
    }

    #[test]
    fn stored_only_floor_repacks_without_copying_literal_tokens() {
        let blocks: Vec<_> = (0..5)
            .map(|value| literal_block(&vec![value; 16_000], SourceBlockType::Stored))
            .collect();
        let plans = repack_all_stored_blocks(&blocks, 0).unwrap();

        assert_eq!(plans.len(), 2);
        assert_eq!(plans[0].plain.len(), 65_535);
        assert_eq!(plans[1].plain.len(), 14_465);
        assert!(plans.iter().all(|plan| plan.tokens.is_empty()));
        let decoded: Vec<_> = plans
            .iter()
            .flat_map(|plan| plan.plain.iter().copied())
            .collect();
        let source: Vec<_> = blocks
            .iter()
            .flat_map(|block| block.plain.iter().copied())
            .collect();
        assert_eq!(decoded, source);

        let source_bits: u64 = blocks
            .iter()
            .map(|block| stored_block_bits(0, block.plain.len()))
            .sum();
        assert!(total_bits(&plans) < source_bits);

        // Repacking does not consult Huffman tables or alter LZ77 tokens, so
        // it remains safe after a ZIP/APNG file-wide search budget is spent.
        let after_deadline = plan_stream(&blocks, 0, &Options::default(), &mut || true).unwrap();
        assert_eq!(total_bits(&after_deadline), total_bits(&plans));
        assert_eq!(after_deadline.len(), plans.len());
    }

    #[test]
    fn source_aligned_floor_prices_short_huffman_groups_before_search() {
        let blocks = [
            literal_block(&vec![b'a'; 800], SourceBlockType::Dynamic),
            literal_block(&vec![b'a'; 800], SourceBlockType::Dynamic),
            literal_block(&vec![b'a'; 800], SourceBlockType::Dynamic),
        ];
        let options = Options::default();
        let plans = source_aligned_huffman_floor(&blocks, 0, &options).unwrap();

        assert_eq!(plans.len(), 1);
        assert_eq!(plans[0].plain.len(), 2_400);
        let separate_bits: u64 = blocks
            .iter()
            .map(|block| plan_block(block, 0, &options, || false).bits)
            .sum();
        assert!(total_bits(&plans) < separate_bits);

        // Exhaustive mode admits regrouping regardless of encoded source size.
        // Once admitted at call entry, the complete structural floor remains
        // available even if the next deadline check expires optional search.
        let exhaustive = Options {
            exhaustive: true,
            ..Options::default()
        };
        let mut deadline_checks = 0;
        let deadline_safe = plan_stream(&blocks, 0, &exhaustive, &mut || {
            deadline_checks += 1;
            deadline_checks > 1
        })
        .unwrap();
        assert_eq!(deadline_safe.len(), 1);
        assert_eq!(deadline_safe[0].plain.len(), 2_400);
    }

    #[test]
    fn bounded_segmentation_keeps_a_later_combined_win() {
        let mut prices = vec![[None; MAX_BOUNDED_GROUP_SPAN + 1]; 4];
        for price in &mut prices {
            price[1] = Some(10);
        }
        // The largest immediate saving at source zero is the three-block
        // range (30 -> 18). Taking it would hide the much stronger range at
        // source two. The complete optimum is [0..2] + [2..4] = 12 bits.
        prices[0][2] = Some(11);
        prices[0][3] = Some(18);
        prices[2][2] = Some(1);

        let boundaries = choose_bounded_boundaries(&prices).unwrap();
        assert_eq!(boundaries, [2, 2, 4, 4]);
    }

    #[test]
    fn long_huffman_collection_is_linear_and_bounded() {
        let blocks: Vec<_> = (0..10)
            .map(|value| literal_block(&vec![value; 1_000], SourceBlockType::Dynamic))
            .collect();
        let collected = collect_huffman_runs(&blocks, false).unwrap();

        assert_eq!(collected.len(), 2);
        assert_eq!(collected[0].tokens.len(), 8_000);
        assert_eq!(collected[0].plain.len(), 8_000);
        assert_eq!(collected[0].source_splits.len(), 7);
        assert_eq!(collected[1].tokens.len(), 2_000);
        let (literal, distance) = count_frequencies(&collected[0].tokens);
        assert_eq!(collected[0].literal_frequencies, literal);
        assert_eq!(collected[0].distance_frequencies, distance);
    }

    #[test]
    fn fragmented_collection_uses_the_independent_4096_token_limit() {
        let mut source_start = 0_u64;
        let blocks: Vec<_> = (0..FRAGMENTED_COLLECT_MIN_SOURCE_BLOCKS)
            .map(|index| {
                let mut block = literal_block(&[index as u8; 100], SourceBlockType::Dynamic);
                block.original = Some(OriginalBits {
                    start: source_start,
                    len: 2_048,
                    alignment: (source_start & 7) as u8,
                    block_type: SourceBlockType::Dynamic,
                });
                source_start += 2_048;
                block
            })
            .collect();
        let options = Options {
            exhaustive: true,
            ..Options::default()
        };

        let seed = fragmented_collect_seed(&blocks, 0, &options).unwrap();
        assert_eq!(seed.len(), 2);
        assert_eq!(seed[0].tokens.len(), 4_000);
        assert_eq!(seed[1].tokens.len(), 2_400);
        let decoded: Vec<_> = seed
            .iter()
            .flat_map(|plan| plan.plain.iter().copied())
            .collect();
        let source: Vec<_> = blocks
            .iter()
            .flat_map(|block| block.plain.iter().copied())
            .collect();
        assert_eq!(decoded, source);
    }

    #[test]
    fn fragmented_replay_is_bounded_and_preserves_decoded_bytes() {
        let blocks: Vec<_> = (0..4)
            .map(|_| literal_block(&vec![b'a'; 512], SourceBlockType::Dynamic))
            .collect();
        let mandatory = mandatory_token_floor_plan(&blocks, 0, &Options::default()).unwrap();
        let plans = plan_fragmented_replay(&blocks, 0, &Options::default()).unwrap();

        assert_eq!(plans.len(), 1);
        assert!(total_bits(&plans) < total_bits(&mandatory));
        let decoded: Vec<_> = plans
            .iter()
            .flat_map(|plan| plan.plain.iter().copied())
            .collect();
        let source: Vec<_> = blocks
            .iter()
            .flat_map(|block| block.plain.iter().copied())
            .collect();
        assert_eq!(decoded, source);

        let too_many = vec![blocks[0].clone(); MAX_FRAGMENTED_REPLAY_BLOCKS + 1];
        assert!(plan_fragmented_replay(&too_many, 0, &Options::default()).is_none());
    }

    #[test]
    fn wide_collection_remains_an_additive_candidate() {
        let blocks: Vec<_> = (0..10)
            .map(|value| literal_block(&vec![value; 1_000], SourceBlockType::Dynamic))
            .collect();
        let ordinary = collect_huffman_runs(&blocks, false).unwrap();
        let exhaustive = collect_huffman_runs(&blocks, true).unwrap();

        assert_eq!(ordinary.len(), 2);
        assert_eq!(exhaustive.len(), 1);
        let plans = direct_structural_plan(&exhaustive, 0, &Options::default()).unwrap();
        assert_eq!(plans.len(), 1);
    }

    #[test]
    fn very_long_flush_runs_can_use_the_bounded_wide_collection() {
        let blocks: Vec<_> = (0..WIDE_COLLECT_MIN_SOURCE_BLOCKS)
            .map(|value| literal_block(&[value as u8; 100], SourceBlockType::Dynamic))
            .collect();
        let collected = collect_huffman_runs(&blocks, true).unwrap();

        assert_eq!(collected.len(), 1);
        assert_eq!(collected[0].tokens.len(), 12_800);
        assert_eq!(collected[0].plain.len(), 12_800);
    }

    #[test]
    fn stored_block_breaks_huffman_collection() {
        let left = literal_block(&vec![1; 1_000], SourceBlockType::Dynamic);
        let stored = literal_block(&vec![2; 1_000], SourceBlockType::Stored);
        let right = literal_block(&vec![3; 1_000], SourceBlockType::Dynamic);

        let collected = collect_huffman_runs(&[left, stored, right], false).unwrap();
        assert_eq!(collected.len(), 3);
        assert_eq!(collected[1].source_type, SourceBlockType::Stored);
    }

    #[test]
    fn consecutive_fixed_plans_remove_ten_bits() {
        let options = Options::default();
        let blocks = [
            literal_block(b"a", SourceBlockType::Fixed),
            literal_block(b"b", SourceBlockType::Fixed),
        ];
        let separate_left = plan_block(&blocks[0], 0, &options, || false);
        let separate_right = plan_block(&blocks[1], 0, &options, || false);
        assert!(is_fixed_plan(&separate_left));
        assert!(is_fixed_plan(&separate_right));

        let plans = sequential_plan(
            &blocks,
            0,
            &options,
            AdjacentMergeSearch::Local,
            &mut || false,
        )
        .unwrap();
        assert_eq!(plans.len(), 1);
        assert!(matches!(plans[0].representation, Representation::Fixed));
        assert_eq!(plans[0].bits, separate_left.bits + separate_right.bits - 10);
        assert_eq!(plans[0].plain.as_slice(), b"ab");
    }

    #[test]
    fn fixed_join_does_not_shift_a_later_stored_plan() {
        let options = Options::default();
        let left_block = literal_block(b"a", SourceBlockType::Fixed);
        let right_block = literal_block(b"b", SourceBlockType::Fixed);
        let left = plan_block(&left_block, 0, &options, || false);
        let right = plan_block(&right_block, (left.bits & 7) as u8, &options, || false);
        assert!(is_fixed_plan(&left));
        assert!(is_fixed_plan(&right));

        let stored_alignment = ((left.bits + right.bits) & 7) as u8;
        let stored = PlannedBlock {
            tokens: vec![Token::Literal(b'c')].into(),
            plain: vec![b'c'].into(),
            representation: Representation::Stored,
            bits: stored_block_bits(stored_alignment, 1),
            source_type: SourceBlockType::Stored,
        };
        let expected_bits = left.bits + right.bits + stored.bits;
        let mut output_bits = left.bits;
        let mut output = vec![left];
        append_output_plans(&mut output, &mut output_bits, vec![right, stored]).unwrap();

        // Joining the two fixed blocks would save ten bits, but it would also
        // move the stored block away from the alignment used to price its
        // padding. A later copied stored block could then be emitted invalidly.
        assert_eq!(output.len(), 3);
        assert_eq!(output_bits, expected_bits);

        let mut writer = BitWriter::default();
        for (index, plan) in output.iter().enumerate() {
            emit_block(&mut writer, &[], plan, index + 1 == output.len()).unwrap();
        }
        assert_eq!(writer.bit_position(), output_bits);
        let encoded = writer.into_bytes();
        let reparsed = parse_stream(&encoded, 3).unwrap();
        assert_eq!(reparsed.decoded_size, 3);
    }

    #[test]
    fn collected_floor_is_not_hidden_by_an_unrelated_strong_adjacent_win() {
        let options = Options::default();
        let mut source_start = 0_u64;
        let blocks: Vec<_> = (0..9)
            .map(|index| {
                let value = if index & 1 == 0 { b'a' } else { b'b' };
                let mut block = literal_block(&[value; 100], SourceBlockType::Dynamic);
                // Deliberately expensive source serializations make the
                // adjacent route a >512-byte win. That saving is independent of
                // the alternating run's better collect-first grouping.
                block.original = Some(OriginalBits {
                    start: source_start,
                    len: 16_000,
                    alignment: (source_start & 7) as u8,
                    block_type: SourceBlockType::Dynamic,
                });
                source_start += 16_000;
                block
            })
            .collect();

        let adjacent = sequential_plan(
            &blocks,
            0,
            &options,
            AdjacentMergeSearch::LongRun,
            &mut || false,
        )
        .unwrap();
        assert!(
            encoded_source_bytes(&blocks) > total_bits(&adjacent).div_ceil(8).saturating_add(512)
        );
        let collected_blocks = collect_huffman_runs(&blocks, false).unwrap();
        let collected = sequential_plan(
            &collected_blocks,
            0,
            &options,
            AdjacentMergeSearch::Disabled,
            &mut || false,
        )
        .unwrap();
        assert!(total_bits(&collected) < total_bits(&adjacent));

        let planned = plan_stream(&blocks, 0, &options, &mut || false).unwrap();
        assert_eq!(total_bits(&planned), total_bits(&collected));
    }

    #[test]
    fn long_run_search_rebuilds_profitable_dynamic_neighbours() {
        let options = Options::default();
        let blocks = [
            literal_block(&vec![b'a'; 1_000], SourceBlockType::Dynamic),
            literal_block(&vec![b'a'; 1_000], SourceBlockType::Dynamic),
        ];
        let separate = sequential_plan(
            &blocks,
            0,
            &options,
            AdjacentMergeSearch::Disabled,
            &mut || false,
        )
        .unwrap();
        let merged = sequential_plan(
            &blocks,
            0,
            &options,
            AdjacentMergeSearch::LongRun,
            &mut || false,
        )
        .unwrap();

        assert!(total_bits(&merged) < total_bits(&separate));
        assert_eq!(merged.len(), 1);
        assert_eq!(merged[0].plain.len(), 2_000);
    }

    #[test]
    fn long_huffman_route_does_not_join_large_stored_neighbours() {
        let options = Options::default();
        let blocks = [
            literal_block(&vec![b'a'; 1_000], SourceBlockType::Stored),
            literal_block(&vec![b'a'; 1_000], SourceBlockType::Stored),
        ];
        let separate = sequential_plan(
            &blocks,
            0,
            &options,
            AdjacentMergeSearch::Disabled,
            &mut || false,
        )
        .unwrap();
        let long_route = sequential_plan(
            &blocks,
            0,
            &options,
            AdjacentMergeSearch::LongRun,
            &mut || false,
        )
        .unwrap();

        assert_eq!(total_bits(&long_route), total_bits(&separate));
        assert_eq!(long_route.len(), separate.len());
    }

    #[test]
    fn eighth_target_snaps_before_a_crossing_match() {
        let mut block = literal_block(&[0; 128], SourceBlockType::Dynamic);
        block.tokens = vec![
            Token::Literal(0),
            Token::Match {
                length: 60,
                distance: 1,
                length_symbol: 276,
                distance_symbol: 0,
                length_extra: 1,
                distance_extra: 0,
                length_extra_bits: 4,
                distance_extra_bits: 0,
            },
        ]
        .into();
        Arc::make_mut(&mut block.tokens).extend((0..67).map(|_| Token::Literal(0)));
        block.recount_frequencies();
        let blocks = [block];
        let composite = Composite::new(&blocks).unwrap();
        let mut cuts = Vec::new();
        add_eighth_cuts(&mut cuts, &composite, 0, 69, 0, 128).unwrap();

        // The first target is decoded offset 16, inside the 60-byte match. The
        // legal preceding boundary is after the first literal at offset one.
        assert!(cuts.contains(&Cut { token: 1, plain: 1 }));
    }

    #[test]
    fn eighth_target_keeps_the_strictly_preceding_exact_boundary() {
        let block = literal_block(&[0; 128], SourceBlockType::Dynamic);
        let blocks = [block];
        let composite = Composite::new(&blocks).unwrap();
        let mut cuts = Vec::new();
        add_eighth_cuts(&mut cuts, &composite, 0, 128, 0, 128).unwrap();

        // DeflOpt treats decoded offset 16 as the inclusive end of the token
        // beginning at offset 15, so the split stays before that token.
        assert!(cuts.contains(&Cut {
            token: 15,
            plain: 15,
        }));
        assert!(!cuts.contains(&Cut {
            token: 16,
            plain: 16,
        }));
    }

    #[test]
    fn midpoint_refinement_snaps_before_a_crossing_match() {
        let mut block = literal_block(&[0; 128], SourceBlockType::Dynamic);
        block.tokens = Arc::new((0..10).map(|_| Token::Literal(0)).collect());
        Arc::make_mut(&mut block.tokens).push(Token::Match {
            length: 100,
            distance: 1,
            length_symbol: 279,
            distance_symbol: 0,
            length_extra: 1,
            distance_extra: 0,
            length_extra_bits: 4,
            distance_extra_bits: 0,
        });
        Arc::make_mut(&mut block.tokens).extend((0..18).map(|_| Token::Literal(0)));
        block.recount_frequencies();

        let blocks = [block];
        let composite = Composite::new(&blocks).unwrap();
        let midpoint = midpoint_cut(
            &composite,
            Cut { token: 0, plain: 0 },
            Cut {
                token: 29,
                plain: 128,
            },
        );

        // Decoded offset 64 lies inside the 100-byte match, so the last legal
        // boundary before it is the one after the ten leading literals.
        assert_eq!(
            midpoint,
            Some(Cut {
                token: 10,
                plain: 10,
            })
        );
    }

    #[test]
    fn combined_run_adds_eighth_regroup_boundaries() {
        let left = literal_block(&vec![b'a'; 20_000], SourceBlockType::Dynamic);
        let right = literal_block(&vec![b'b'; 20_000], SourceBlockType::Dynamic);
        let blocks = [left, right];
        let composite = Composite::new(&blocks).unwrap();
        let cuts = choose_cuts(&composite, false, true).unwrap();
        assert!(cuts.contains(&Cut {
            token: 4_999,
            plain: 4_999,
        }));
        assert!(cuts.contains(&Cut {
            token: 20_000,
            plain: 20_000,
        }));
    }

    #[test]
    fn compact_max_cut_set_reaches_late_32_token_probes() {
        let block = literal_block(&vec![b'x'; 384], SourceBlockType::Dynamic);
        let blocks = [block];
        let composite = Composite::new(&blocks).unwrap();
        let cuts = choose_cuts(&composite, true, true).unwrap();

        // This boundary is deliberately near the end. Scoring all compact
        // probes before deep whole-block searches keeps it reachable under a
        // finite max-mode deadline.
        assert!(cuts.contains(&Cut {
            token: 320,
            plain: 320,
        }));
    }

    #[test]
    fn identical_dynamic_trees_survive_a_merge() {
        let shared = DynamicPlan {
            literal_lengths: vec![1; 257],
            distance_lengths: vec![1],
            code_length_lengths: [0; 19],
            rle: Vec::new(),
            hlit: 257,
            hdist: 1,
            hclen: 4,
            bits: 100,
        };
        let mut left = literal_block(b"left", SourceBlockType::Dynamic);
        let mut right = literal_block(b"right", SourceBlockType::Dynamic);
        left.original_dynamic = Some(shared.clone());
        right.original_dynamic = Some(shared);

        assert!(blocks_share_dynamic_tree(&left, &right));
        let merged = try_merge_parsed_blocks(&left, &right).unwrap();
        assert!(merged.original_dynamic.is_some());
        assert_eq!(merged.plain.as_slice(), b"leftright");
    }
}
