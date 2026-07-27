// SPDX-License-Identifier: MIT

//! Stream-level Deflate block planning.
//!
//! Deflate block boundaries do not affect the decoded bytes or the 32 KiB
//! history window. A boundary may therefore be removed or moved to any token
//! boundary without changing a match. Columbo uses that freedom to reduce the
//! number of headers and to give locally different data separate Huffman
//! tables. It never discovers new LZ77 matches here.
//!
//! Stream-level attribution is intentionally narrow: DeflOpt contributes only
//! the exact fixed/fixed 10-bit coalescing rule. Columbo contributes arbitrary
//! merging, regrouping, eighth probes, boundary dynamic programming, exact
//! candidate acceptance, and timed routing. Max mode also contains Columbo's
//! independent, speed-bounded implementation of a coarse boundary-search
//! concept described by Turtledeflate. The deft4j-derived greedy merge lives
//! in `deft4j`.

use std::borrow::Cow;
use std::sync::Arc;
use std::thread;

use crate::progress::RouteProgress;
use crate::Options;

use super::block::{
    lookup_block_cached, plan_block, plan_block_cached, plan_reusable_block,
    reusable_original_bits, stored_block_bits, CanonicalPlanCache, ReusableBlockPlan,
};
use super::deft4j::plan_source_block;
use super::header::{estimate_boundary_block_bits, score_existing_dynamic};
use super::model::{
    count_frequencies, token_extra_bits, DynamicPlan, OriginalBits, ParsedBlock, PlannedBlock,
    Representation, SourceBlockType, Token,
};
use super::search::{
    improve_plan_with_deft4j_tree_floor, improve_plan_with_floor,
    improve_plan_with_same_distance_floor, improve_plan_with_short_family_floor,
    plan_block_with_complete_base_search, plan_block_with_narrow_search, plan_block_with_search,
    plan_block_with_seeded_narrow_search, replay_extended_floor, replay_table_ladder,
    score_short_family_frequencies, tighten_terminal_plan, try_clone_planned_block,
    ShortFamilyStats,
};

// The original Columbo C implementation tries its default long-merge route in
// this encoded-size range. The gate avoids quadratic work on very large streams
// while covering the common encoder-flush case.
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
// Columbo's inexpensive long-run floor collects a bounded Huffman prefix
// before planning it. This covers encoder-flush streams without feeding a
// quadratic number of source-pair cuts into the general boundary DP.
const COLLECTED_RUN_MAX_TOKENS: usize = 8_192;
const COLLECTED_RUN_MAX_PLAIN: usize = 512 * 1_024;
const FRAGMENTED_COLLECT_MAX_TOKENS: usize = 4_096;
const FRAGMENTED_COLLECT_MIN_SOURCE_BLOCKS: usize = 64;
const WIDE_COLLECT_MIN_SOURCE_BLOCKS: usize = 128;
// The original Columbo C block-list pass is a linear adjacent walk. Keep this
// port bounded to ordinary encoder block counts; extremely fragmented streams
// use the 8,192-token collection floor above instead.
const MAX_GREEDY_SOURCE_BLOCKS: usize = 128;
const MAX_BOUNDED_GROUP_SPAN: usize = 16;
// The bounded-grouping score rows are independent. Four workers cover the
// useful wall-clock gain without letting this optional route consume every
// hardware thread or multiply its small row table without bound.
const MAX_BOUNDED_GROUP_WORKERS: usize = 4;
// Optional structural routes may copy payloads while the parsed source is
// still live. Two 48 MiB retained-payload partitions leave 32 MiB of a 128 MiB
// envelope for the bounded DP table and Huffman metadata. Boundary DP itself
// keeps the source token parse unchanged; token-spelling searches run in the
// sequential/replay passes rather than once per cut and alignment.
const MAX_GROUPED_MODEL_BYTES: usize = 48 * 1024 * 1024;
const MAX_COMPOSITE_MODEL_BYTES: usize = 48 * 1024 * 1024;
// Concept inspired by Turtledeflate's `turtledeflate_create_global_histogram`
// and `turtledeflate_get_partial_histogram` at commit 756f844. Both use
// 256-token cumulative checkpoints. This is an independent Rust
// implementation: Columbo reconstructs and subtracts two prefixes instead of
// subtracting an aligned middle and scanning both edges.
const RANGE_HISTOGRAM_INTERVAL: usize = 256;
// Columbo independently implements the sample/smooth/narrow concept used by
// Turtledeflate's `turtledeflate_best_block_split`, with a much smaller fixed
// budget before handing one cut to its exact planner.
const ADAPTIVE_SPLIT_MIN_TOKENS: usize = 513;
const ADAPTIVE_SPLIT_INTERVALS: usize = 7;
const ADAPTIVE_SPLIT_CENTER_RADIUS: usize = 1;
const ADAPTIVE_SPLIT_FINAL_WIDTH: usize = 16;
const ADAPTIVE_SPLIT_MAX_PROBES: usize = 128;
// A marginal adaptive split can unlock another whole-stream replay whose cost
// dwarfs the saving. Require four bytes at the Deflate level before allowing
// this optional route to alter the established max result.
const ADAPTIVE_SPLIT_MIN_EXACT_SAVINGS_BITS: u64 = 32;
// Boundary DP is an optional max route. Capping its cut count keeps the dense
// eight-alignment state table small on adversarially fragmented streams.
const MAX_BOUNDARY_DP_CUTS: usize = 2_048;
// Only near-tied first splits justify an extra child refinement in default
// mode; a wider gap is very unlikely to be recovered by one added header.
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

/// Which per-source search is composed by the sequential stream walk.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SourceBlockSearch {
    /// Ordinary planning, including the independent source-split family.
    Full,
    /// One whole-block ladder, leaving split and iterative work to siblings.
    Narrow { allow_individual_prune: bool },
    /// Deterministic table floors only, for a selected terminal seed.
    Floor,
}

/// Identifies the cheap pre-grouped layout selected for the first replay.
///
/// The kind is retained alongside the slice so a completed collected-layout
/// replay is not repeated later in the stream route.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum GroupedLayout {
    Greedy,
    Bounded,
    Collected,
}

fn cheapest_grouped_layout(
    candidates: [Option<(GroupedLayout, &[ParsedBlock], u64)>; 3],
) -> Option<(GroupedLayout, &[ParsedBlock])> {
    candidates
        .into_iter()
        .flatten()
        .min_by_key(|(_, _, bits)| *bits)
        .map(|(kind, blocks, _)| (kind, blocks))
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
    let progress = RouteProgress::disabled();
    plan_stream_with_progress(blocks, start_alignment, options, expired, &progress)
}

/// Plan a stream while exposing coarse progress for one long max route.
///
/// The ordinary entry point above supplies a disabled reporter, keeping all
/// standard and non-verbose callers on the same search path.
pub(crate) fn plan_stream_with_progress<F>(
    blocks: &[ParsedBlock],
    start_alignment: u8,
    options: &Options,
    expired: &mut F,
    progress: &RouteProgress,
) -> Option<Vec<PlannedBlock>>
where
    F: FnMut() -> bool,
{
    let detailed_progress = progress.enabled().then_some(progress);
    let mut plan_cache = CanonicalPlanCache::new();
    progress.phase(
        "Establishing complete comparison floor",
        blocks.len(),
        "source block",
        "source blocks",
    );
    // Stored-only streams have a linear structural floor: adjacent
    // chunks can be repacked up to RFC 1951's 65,535-byte limit without any
    // Huffman or token search. Secure that result before consulting a shared
    // container deadline so a large ZIP cannot optimize only its first member.
    let stored_floor = repack_all_stored_blocks(blocks, start_alignment);

    // A container can call us after its file-wide budget is already spent so
    // the stream is still parsed and validated. Retain its exact source bytes
    // through `build_candidate`'s fallback unless the stored repack above is
    // available; starting token recodes for dozens of later APNG frames would
    // turn a bounded timeout into unbounded work. Strict mode must still
    // rewrite incompatible dynamic alphabets.
    let floor_time_available = !expired();
    if !floor_time_available && !options.strict {
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
        .then(|| source_aligned_huffman_floor(blocks, start_alignment, options, &mut plan_cache))
        .flatten();
    let greedy_blocks = if allow_regroup
        && blocks.len() > MAX_REGROUP_SOURCE_BLOCKS
        && blocks.len() <= MAX_GREEDY_SOURCE_BLOCKS
    {
        greedy_huffman_blocklist(blocks, start_alignment, options, &mut plan_cache)
            .filter(|grouped| grouped.len() < blocks.len())
    } else {
        None
    };
    let greedy_floor = greedy_blocks.as_deref().and_then(|grouped| {
        direct_structural_plan(grouped, start_alignment, options, &mut plan_cache)
    });
    let bounded_group_blocks = if allow_regroup
        && blocks.len() > MAX_REGROUP_SOURCE_BLOCKS
        && blocks.len() <= MAX_GREEDY_SOURCE_BLOCKS
    {
        bounded_huffman_grouping(
            blocks,
            options,
            BoundedRangePricing::Serial,
            &mut plan_cache,
        )
        .filter(|grouped| grouped.len() < blocks.len())
    } else {
        None
    };
    let bounded_group_floor = bounded_group_blocks.as_deref().and_then(|grouped| {
        direct_structural_plan(grouped, start_alignment, options, &mut plan_cache)
    });
    let collected_blocks = if allow_regroup && blocks.len() > MAX_REGROUP_SOURCE_BLOCKS {
        collect_huffman_runs(blocks, false).filter(|collected| collected.len() < blocks.len())
    } else {
        None
    };
    let collected_floor = collected_blocks.as_deref().and_then(|collected| {
        direct_structural_plan(collected, start_alignment, options, &mut plan_cache)
    });
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
        .and_then(|wide| direct_structural_plan(wide, start_alignment, options, &mut plan_cache));

    // Search whichever cheap grouping has the best complete direct price.
    // Keeping this choice separate from the emitted fallback means all other
    // layouts remain available as strict no-growth candidates.
    let selected_grouping = cheapest_grouped_layout([
        greedy_blocks
            .as_deref()
            .zip(greedy_floor.as_ref().map(|floor| total_bits(floor)))
            .map(|(blocks, bits)| (GroupedLayout::Greedy, blocks, bits)),
        bounded_group_blocks
            .as_deref()
            .zip(bounded_group_floor.as_ref().map(|floor| total_bits(floor)))
            .map(|(blocks, bits)| (GroupedLayout::Bounded, blocks, bits)),
        collected_blocks
            .as_deref()
            .zip(collected_floor.as_ref().map(|floor| total_bits(floor)))
            .map(|(blocks, bits)| (GroupedLayout::Collected, blocks, bits)),
    ]);

    // Secure a complete deadline-independent path before token-spelling or
    // split searches. On a shared container deadline this also guarantees
    // that every stream receives useful structural optimization.
    let mut fallback = if floor_time_available && !expired() {
        mandatory_token_floor_plan(blocks, start_alignment, options, &mut plan_cache)?
    } else {
        direct_structural_plan(blocks, start_alignment, options, &mut plan_cache)?
    };
    let mut fallback_bits = total_bits(&fallback);
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
        let floor_bits = total_bits(&floor);
        if floor_bits < fallback_bits {
            fallback = floor;
            fallback_bits = floor_bits;
        }
    }
    progress.checkpoint("Comparison floor", fallback_bits, fallback.len());

    // Search the most promising pre-grouped list before the original long
    // list can consume the deadline. Columbo's original C block-list route
    // does the same: it first commits cheap adjacent structure, then replays
    // the selected groups.
    let mut collected_search_completed = false;
    if let Some((kind, grouped)) = selected_grouping {
        progress.phase(
            "Searching selected grouped layout",
            grouped.len(),
            "grouped block",
            "grouped blocks",
        );
        if !expired() {
            if let Some(candidate) = sequential_plan(
                grouped,
                start_alignment,
                options,
                AdjacentMergeSearch::Disabled,
                &mut plan_cache,
                expired,
                detailed_progress,
            ) {
                progress.advance(grouped.len());
                collected_search_completed = kind == GroupedLayout::Collected;
                let candidate_bits = total_bits(&candidate);
                progress.checkpoint("Grouped-layout result", candidate_bits, candidate.len());
                if candidate_bits < fallback_bits {
                    fallback = candidate;
                    fallback_bits = candidate_bits;
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
    progress.phase(
        "Searching original source order",
        blocks.len(),
        "source block",
        "source blocks",
    );
    if !expired() {
        if let Some(candidate) = sequential_plan(
            blocks,
            start_alignment,
            options,
            fallback_merge_search,
            &mut plan_cache,
            expired,
            detailed_progress,
        ) {
            progress.advance(blocks.len());
            let candidate_bits = total_bits(&candidate);
            progress.checkpoint("Source-order result", candidate_bits, candidate.len());
            if candidate_bits < fallback_bits {
                fallback = candidate;
                fallback_bits = candidate_bits;
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
        if !collected_search_completed {
            if let Some(collected_blocks) = collected_blocks.as_deref() {
                progress.phase(
                    "Replaying collected block layout",
                    collected_blocks.len(),
                    "collected block",
                    "collected blocks",
                );
                let collected = sequential_plan(
                    collected_blocks,
                    start_alignment,
                    options,
                    AdjacentMergeSearch::Disabled,
                    &mut plan_cache,
                    expired,
                    detailed_progress,
                );
                if let Some(collected) = collected {
                    progress.advance(collected_blocks.len());
                    let collected_bits = total_bits(&collected);
                    progress.checkpoint("Collected-layout result", collected_bits, collected.len());
                    if collected_bits < fallback_bits {
                        fallback = collected;
                        fallback_bits = collected_bits;
                    }
                }
            }
        }
        if expired() {
            return Some(finish_plan(fallback, options));
        }

        // Default-mode cross-source DP edges are deliberately limited to a
        // short run. Above that limit the remaining cut set can only revisit
        // individual source blocks at different alignments; it cannot improve
        // on either complete plan above. Returning here avoids quadratic work
        // over hundreds of encoder-flush blocks. Max mode still admits
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
    progress.phase(
        "Building global boundary graph",
        blocks.len(),
        "source block",
        "source blocks",
    );
    let Some(composite) = Composite::new(blocks) else {
        return Some(finish_plan(fallback, options));
    };
    let Some(cuts) = choose_cuts(&composite, options.exhaustive, allow_regroup) else {
        return Some(finish_plan(fallback, options));
    };
    if cuts.len() <= 2 {
        return Some(finish_plan(fallback, options));
    }

    progress.phase(
        "Pricing block boundaries",
        cuts.len().saturating_sub(1),
        "cut anchor",
        "cut anchors",
    );
    let Some(candidate) = boundary_dp(
        blocks,
        &composite,
        &cuts,
        start_alignment,
        options,
        allow_regroup,
        &mut plan_cache,
        expired,
        detailed_progress,
    ) else {
        return Some(finish_plan(fallback, options));
    };
    progress.advance(cuts.len().saturating_sub(1));
    let candidate_bits = total_bits(&candidate);
    progress.checkpoint("Boundary-DP result", candidate_bits, candidate.len());
    if candidate_bits < fallback_bits {
        Some(finish_plan(candidate, options))
    } else {
        Some(finish_plan(fallback, options))
    }
}

/// Run the direct source-order route used when broad split search is a poor
/// use of a bounded max budget.
///
/// Every original Huffman block receives one narrow whole-block search and
/// profitable adjacent pairs are retried greedily. The route is additive to
/// [`plan_stream`]: it deliberately omits grouping, split probes, boundary DP,
/// and iterative state queues so a long regular chain can finish within its
/// own wall-clock slice.
pub(crate) fn plan_source_no_split_route<F>(
    blocks: &[ParsedBlock],
    start_alignment: u8,
    options: &Options,
    allow_individual_prune: bool,
    expired: &mut F,
) -> Option<Vec<PlannedBlock>>
where
    F: FnMut() -> bool,
{
    let prepared = prepare_blocks(blocks);
    let blocks = prepared.as_deref().unwrap_or(blocks);
    let mut plan_cache = CanonicalPlanCache::new();
    let fallback = direct_structural_plan(blocks, start_alignment, options, &mut plan_cache)?;
    if expired() {
        return Some(finish_plan(fallback, options));
    }

    let searched = sequential_plan_with_source_search(
        blocks,
        start_alignment,
        options,
        AdjacentMergeSearch::LongRun,
        SourceBlockSearch::Narrow {
            allow_individual_prune,
        },
        &mut plan_cache,
        expired,
        None,
    );
    match searched {
        Some(candidate) if total_bits(&candidate) < total_bits(&fallback) => {
            Some(finish_plan(candidate, options))
        }
        _ => Some(finish_plan(fallback, options)),
    }
}

/// Greedily merge an already-selected Huffman seed using deterministic floors.
///
/// Unlike the timed no-split route, this cleanup performs no byte-seeking
/// search and has no recursive replay. It is safe to finish after the main
/// deadline because its work is linear in the selected block list and every
/// accepted merge strictly reduces the complete candidate's bit count.
pub(crate) fn plan_terminal_merge_route<F>(
    blocks: &[ParsedBlock],
    start_alignment: u8,
    options: &Options,
    expired: &mut F,
) -> Option<Vec<PlannedBlock>>
where
    F: FnMut() -> bool,
{
    if blocks.len() < 2 || blocks.len() > MAX_GREEDY_SOURCE_BLOCKS || expired() {
        return None;
    }
    let mut plan_cache = CanonicalPlanCache::new();
    let fallback = direct_structural_plan(blocks, start_alignment, options, &mut plan_cache)?;
    let candidate = sequential_plan_with_source_search(
        blocks,
        start_alignment,
        options,
        AdjacentMergeSearch::LongRun,
        SourceBlockSearch::Floor,
        &mut plan_cache,
        expired,
        None,
    )?;
    (total_bits(&candidate) < total_bits(&fallback)).then(|| finish_plan(candidate, options))
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
    plan_cache: &mut CanonicalPlanCache,
) -> Option<Vec<PlannedBlock>> {
    let mut structural_options = options.clone();
    structural_options.exhaustive = false;
    let mut plans = Vec::new();
    plans.try_reserve_exact(blocks.len()).ok()?;
    let mut output_bits = 0_u64;

    for block in blocks {
        let alignment = ((u64::from(start_alignment) + output_bits) & 7) as u8;
        let plan = plan_block_cached(block, alignment, &structural_options, plan_cache);
        append_output_plan(&mut plans, &mut output_bits, plan, true)?;
    }
    Some(plans)
}

/// Apply one bounded token-preserving pass to each ordinary source block.
///
/// Very fragmented streams are handled by collection first; running even a
/// small token pass hundreds of times would spend the container deadline on
/// bookkeeping. For normal block counts this Columbo floor gives every
/// ZIP/APNG member strict source/fixed, exact-Defluff-tree, and hybrid-tree
/// feedback candidates before optional search.
fn mandatory_token_floor_plan(
    blocks: &[ParsedBlock],
    start_alignment: u8,
    options: &Options,
    plan_cache: &mut CanonicalPlanCache,
) -> Option<Vec<PlannedBlock>> {
    if blocks.len() > MAX_GREEDY_SOURCE_BLOCKS {
        return direct_structural_plan(blocks, start_alignment, options, plan_cache);
    }
    let mut plans = Vec::new();
    plans.try_reserve_exact(blocks.len()).ok()?;
    let mut output_bits = 0_u64;
    let extended = blocks.len() <= MAX_REGROUP_SOURCE_BLOCKS;
    let mut floor_options = options.clone();
    floor_options.exhaustive = false;
    for block in blocks {
        let alignment = ((u64::from(start_alignment) + output_bits) & 7) as u8;
        let base = plan_block_cached(block, alignment, &floor_options, plan_cache);
        let mut plan = improve_plan_with_floor(block, alignment, &floor_options, extended, base);
        // Columbo's five cumulative symbol-260..264 bands are inspired by
        // repeated deft4j least-family pruning, but are not deft4j states.
        // Price them before a container deadline can divert optional splitting
        // toward a locally attractive two-block layout.
        plan = improve_plan_with_short_family_floor(block, &floor_options, plan);
        append_output_plan(&mut plans, &mut output_bits, plan, true)?;
    }
    Some(plans)
}

fn plan_block_with_floor_cached(
    block: &ParsedBlock,
    alignment: u8,
    options: &Options,
    extended: bool,
    plan_cache: &mut CanonicalPlanCache,
) -> PlannedBlock {
    let mut floor_options = options.clone();
    floor_options.exhaustive = false;
    let base = plan_block_cached(block, alignment, &floor_options, plan_cache);
    improve_plan_with_floor(block, alignment, &floor_options, extended, base)
}

fn plan_block_with_short_family_floor_cached(
    block: &ParsedBlock,
    alignment: u8,
    options: &Options,
    plan_cache: &mut CanonicalPlanCache,
) -> PlannedBlock {
    let mut floor_options = options.clone();
    floor_options.exhaustive = false;
    let base = plan_block_cached(block, alignment, &floor_options, plan_cache);
    let base = improve_plan_with_same_distance_floor(block, alignment, &floor_options, base);
    improve_plan_with_short_family_floor(block, &floor_options, base)
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
    plan_cache: &mut CanonicalPlanCache,
) -> Option<Vec<PlannedBlock>> {
    source_aligned_huffman_floor_with_limit(
        blocks,
        start_alignment,
        options,
        MAX_REGROUP_SOURCE_BLOCKS,
        plan_cache,
    )
}

fn source_aligned_huffman_floor_with_limit(
    blocks: &[ParsedBlock],
    start_alignment: u8,
    options: &Options,
    max_blocks: usize,
    plan_cache: &mut CanonicalPlanCache,
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
            // their original fixed/dynamic bits for the same reason. A merged
            // range also feeds the additive recode families below, so retain
            // its ordinary plan instead of rebuilding it in every helper.
            let mut recode_base = None;
            let mut singleton_same_distance = None;
            let template = if end_index - start_index > 1 {
                let range = make_range(&composite, start, end)?;
                let base =
                    plan_block_cached(&range, start_alignment, &structural_options, plan_cache);
                let mut selected = PlanTemplate::try_from_planned(&base)?;
                if let Some(shared) = shared_dynamic_plan(
                    &blocks[start_index..end_index],
                    &range.tokens,
                    structural_options.strict,
                ) {
                    if shared.bits < selected.bits {
                        selected.bits = shared.bits;
                        selected.representation = Representation::Dynamic(shared);
                    }
                }
                recode_base = Some((range, base));
                selected
            } else {
                let block = &blocks[start_index];
                let base =
                    plan_block_cached(block, start_alignment, &structural_options, plan_cache);
                let selected = PlanTemplate::try_from_planned(&base)?;
                singleton_same_distance = Some(improve_plan_with_same_distance_floor(
                    block,
                    start_alignment,
                    &structural_options,
                    base,
                ));
                selected
            };
            let mut candidate_bits = template.bits;
            let mut candidate_is_stored = matches!(template.representation, Representation::Stored);
            let mut candidate_plan = SourceAlignedPlan::Template(template);

            // A singleton edge can participate in the globally best source
            // segmentation, so carry its normalized token spelling in this
            // DP instead of relying on the separate all-singleton floor.
            if let Some(recode) = singleton_same_distance {
                if !matches!(recode.representation, Representation::Stored)
                    && recode.bits < candidate_bits
                {
                    candidate_bits = recode.bits;
                    candidate_is_stored = false;
                    candidate_plan = SourceAlignedPlan::Recode(recode);
                }
            }

            // Same-distance runs can cross an erased source boundary, while
            // deft4j rebuilds a merged range before pruning matches. Price
            // those candidates, and Columbo's separate cumulative-family
            // bands, only after forming the range: per-source tables cannot
            // reproduce the merged state. Every helper enforces model limits.
            if let Some((range, base)) = recode_base {
                let is_large_whole_stream = start_index == 0
                    && end_index == blocks.len()
                    && range.plain.len() >= WHOLE_STREAM_RECODE_MIN_PLAIN;
                // All four recode families start from the same ordinary
                // stored/fixed/dynamic price. Build it once and clone only
                // the small representation metadata; tokens and decoded
                // bytes remain shared through their `Arc`s. Each helper
                // still receives an independent base, preserving the old
                // candidate and tie order below.
                // The complete large-stream floor starts with this same
                // normalization, so do not build and replay an identical
                // standalone seed for that one range.
                let same_distance = (!is_large_whole_stream)
                    .then(|| {
                        try_clone_planned_block(&base).map(|base| {
                            improve_plan_with_same_distance_floor(
                                &range,
                                start_alignment,
                                &structural_options,
                                base,
                            )
                        })
                    })
                    .flatten();
                let deft4j = try_clone_planned_block(&base).map(|base| {
                    improve_plan_with_deft4j_tree_floor(
                        &range,
                        start_alignment,
                        &structural_options,
                        base,
                    )
                });
                // A container shares one deadline across all of its
                // streams. Give a large complete merged stream one
                // extended token-preserving pass now, so an earlier frame
                // cannot prevent its best whole-stream spelling from
                // being seen.
                // Limiting this to the full range avoids multiplying that
                // work across every possible source-aligned subrange.
                let (short_family, whole_stream) = if is_large_whole_stream {
                    let short_family = try_clone_planned_block(&base).map(|base| {
                        improve_plan_with_short_family_floor(&range, &structural_options, base)
                    });
                    let whole_stream = Some(improve_plan_with_floor(
                        &range,
                        start_alignment,
                        &structural_options,
                        true,
                        base,
                    ));
                    (short_family, whole_stream)
                } else {
                    let short_family = Some(improve_plan_with_short_family_floor(
                        &range,
                        &structural_options,
                        base,
                    ));
                    (short_family, None)
                };
                let replay_seed_bits = [
                    same_distance.as_ref().map(|plan| plan.bits),
                    deft4j.as_ref().map(|plan| plan.bits),
                    short_family.as_ref().map(|plan| plan.bits),
                    whole_stream.as_ref().map(|plan| plan.bits),
                ]
                .into_iter()
                .flatten()
                .fold(candidate_bits, u64::min);
                for mut recode in [same_distance, deft4j, short_family, whole_stream]
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

    let mut plan_cache = CanonicalPlanCache::new();
    let mut best = mandatory_token_floor_plan(blocks, start_alignment, options, &mut plan_cache)?;
    if let Some(grouped) = source_aligned_huffman_floor_with_limit(
        blocks,
        start_alignment,
        options,
        MAX_FRAGMENTED_REPLAY_BLOCKS,
        &mut plan_cache,
    ) {
        if total_bits(&grouped) < total_bits(&best) {
            best = grouped;
        }
    }
    if let Some(split) =
        plan_compact_source_split_floor_cached(blocks, start_alignment, options, &mut plan_cache)
    {
        if split.len() <= MAX_FRAGMENTED_REPLAY_BLOCKS && total_bits(&split) < total_bits(&best) {
            best = split;
        }
    }
    Some(best)
}

/// Price one direct decoded-eighth split per block without deep token search.
///
/// Fragmented replay and the compact post-deft4j route share this exact
/// structural floor so neither rebuilds or subtly reimplements its cut logic.
/// Accepted children become ordinary source blocks after emission, so a later
/// bounded replay can split them again or merge either child with a neighbour.
pub(crate) fn plan_compact_source_split_floor(
    blocks: &[ParsedBlock],
    start_alignment: u8,
    options: &Options,
) -> Option<Vec<PlannedBlock>> {
    let mut plan_cache = CanonicalPlanCache::new();
    plan_compact_source_split_floor_cached(blocks, start_alignment, options, &mut plan_cache)
}

fn plan_compact_source_split_floor_cached(
    blocks: &[ParsedBlock],
    start_alignment: u8,
    options: &Options,
    plan_cache: &mut CanonicalPlanCache,
) -> Option<Vec<PlannedBlock>> {
    let mut output = Vec::new();
    output.try_reserve_exact(blocks.len()).ok()?;
    let mut output_bits = 0_u64;

    for block in blocks {
        let alignment = ((u64::from(start_alignment) + output_bits) & 7) as u8;
        let base = plan_block_cached(block, alignment, options, plan_cache);
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
                let left_plan = plan_block_cached(&left, alignment, options, plan_cache);
                let right_alignment = ((u64::from(alignment) + left_plan.bits) & 7) as u8;
                let right = make_range(
                    &composite,
                    split,
                    Cut {
                        token: block.tokens.len(),
                        plain: block.plain.len(),
                    },
                )?;
                let right_plan = plan_block_cached(&right, right_alignment, options, plan_cache);
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

/// Model the greedy Huffman-only shape of Columbo's original C block-list pass.
///
/// Each source block and adjacent merged pair is priced with the ordinary
/// token-preserving planner. A strictly cheaper merge replaces the pending
/// pair and is immediately compared with its next neighbour. This greedy
/// shape matters: photographic streams commonly settle into groups of four to
/// eleven source blocks, while collecting the entire run under one table is
/// measurably worse. Unlike the complete original pass, this structural floor
/// excludes stored-block accumulation and its additional fixed/shared-tree
/// candidates.
fn greedy_huffman_blocklist(
    blocks: &[ParsedBlock],
    start_alignment: u8,
    options: &Options,
    plan_cache: &mut CanonicalPlanCache,
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

    // This is a structural floor, not a second max search. The exhaustive
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
            _ => plan_block_with_floor_cached(
                pending.as_block(),
                alignment,
                &structural_options,
                false,
                plan_cache,
            ),
        };
        let current_alignment = ((u64::from(alignment) + pending_plan.bits) & 7) as u8;
        let current_plan = plan_block_with_floor_cached(
            current,
            current_alignment,
            &structural_options,
            false,
            plan_cache,
        );
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
                let merged_plan = plan_block_with_floor_cached(
                    &merged,
                    alignment,
                    &structural_options,
                    false,
                    plan_cache,
                );
                if merged_plan.bits < separate_bits {
                    let mut merged = merged;
                    // Carry a strict intermediate token winner into the next
                    // adjacent comparison, matching Columbo's original C list.
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

/// Replan an already-rewritten Columbo floor with bounded Huffman grouping.
///
/// The general stream planner evaluates several unrelated layouts and token
/// searches. A selected floor needs only this grouping pass: its independent
/// range-score rows run on at most four workers, then the unchanged ordered
/// suffix DP selects and emits one deterministic complete candidate.
pub(crate) fn plan_columbo_floor_seeded_bounded_grouping(
    blocks: &[ParsedBlock],
    start_alignment: u8,
    options: &Options,
) -> Option<Vec<PlannedBlock>> {
    let prepared = prepare_blocks(blocks);
    let blocks = prepared.as_deref().unwrap_or(blocks);
    let mut plan_cache = CanonicalPlanCache::new();
    let grouped = bounded_huffman_grouping(
        blocks,
        options,
        BoundedRangePricing::Parallel,
        &mut plan_cache,
    )?;
    let plans = direct_structural_plan(&grouped, start_alignment, options, &mut plan_cache)?;
    Some(finish_plan(plans, options))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BoundedRangePricing {
    Serial,
    Parallel,
}

/// Find useful source-boundary groups with a small lookahead.
///
/// Greedy adjacent merging models Columbo's original C block list, but a first
/// pair can be neutral even when three or more neighbours profit from one
/// shared table. At each source position, price at most sixteen complete
/// groups, commit the strict best saving, and continue after it. This keeps the
/// pass linear in practical group count while retaining the important
/// lookahead.
fn bounded_huffman_grouping(
    blocks: &[ParsedBlock],
    options: &Options,
    pricing: BoundedRangePricing,
    plan_cache: &mut CanonicalPlanCache,
) -> Option<Vec<ParsedBlock>> {
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
        let plan = plan_block_with_floor_cached(block, 0, &structural_options, false, plan_cache);
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
    let mut source_extra_bits = Vec::new();
    source_extra_bits.try_reserve_exact(blocks.len()).ok()?;
    for block in blocks {
        source_extra_bits.push(token_extra_bits(&block.tokens));
    }

    // Price every bounded range once. Range costs are independent of their
    // neighbours because all candidates here are Huffman blocks, so a small
    // suffix DP can choose the best complete segmentation instead of making
    // an irreversible greedy choice at each source boundary.
    let range_bits = match pricing {
        BoundedRangePricing::Serial => bounded_range_prices(
            blocks,
            &source_plans,
            &source_stats,
            &source_extra_bits,
            structural_options.strict,
            0,
            blocks.len(),
        )?,
        BoundedRangePricing::Parallel => parallel_bounded_range_prices(
            blocks,
            &source_plans,
            &source_stats,
            &source_extra_bits,
            structural_options.strict,
        )?,
    };

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
            let plan = plan_block_with_short_family_floor_cached(
                &winner,
                0,
                &structural_options,
                plan_cache,
            );
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

type BoundedRangePrices = [Option<u64>; MAX_BOUNDED_GROUP_SPAN + 1];

/// Price independent start rows concurrently, preserving source-row order.
///
/// Thread creation is optional: a failed spawn is evaluated immediately on
/// the caller. Once any worker starts, every handle is joined before an
/// allocation failure is returned or the first worker panic is resumed.
fn parallel_bounded_range_prices(
    blocks: &[ParsedBlock],
    source_plans: &[PlannedBlock],
    source_stats: &[ShortFamilyStats],
    source_extra_bits: &[u64],
    min_distance_codes: bool,
) -> Option<Vec<BoundedRangePrices>> {
    let worker_count = thread::available_parallelism()
        .map(usize::from)
        .unwrap_or(1)
        .min(MAX_BOUNDED_GROUP_WORKERS)
        .min(blocks.len());
    if worker_count <= 1 {
        return bounded_range_prices(
            blocks,
            source_plans,
            source_stats,
            source_extra_bits,
            min_distance_codes,
            0,
            blocks.len(),
        );
    }

    let rows_per_worker = blocks.len().div_ceil(worker_count);
    let chunk_count = blocks.len().div_ceil(rows_per_worker);
    thread::scope(|scope| {
        let mut completed = Vec::new();
        completed.try_reserve_exact(chunk_count).ok()?;
        completed.resize_with(chunk_count, || None);

        let mut handles = Vec::new();
        if handles.try_reserve_exact(chunk_count).is_err() {
            return bounded_range_prices(
                blocks,
                source_plans,
                source_stats,
                source_extra_bits,
                min_distance_codes,
                0,
                blocks.len(),
            );
        }

        let mut pricing_failed = false;
        for (chunk_index, start) in (0..blocks.len()).step_by(rows_per_worker).enumerate() {
            let end = blocks.len().min(start + rows_per_worker);
            let worker = thread::Builder::new().spawn_scoped(scope, move || {
                bounded_range_prices(
                    blocks,
                    source_plans,
                    source_stats,
                    source_extra_bits,
                    min_distance_codes,
                    start,
                    end,
                )
            });
            match worker {
                Ok(handle) => handles.push((chunk_index, handle)),
                Err(_) => match bounded_range_prices(
                    blocks,
                    source_plans,
                    source_stats,
                    source_extra_bits,
                    min_distance_codes,
                    start,
                    end,
                ) {
                    Some(rows) => completed[chunk_index] = Some(rows),
                    None => pricing_failed = true,
                },
            }
        }

        let mut first_panic = None;
        for (chunk_index, handle) in handles {
            match handle.join() {
                Ok(Some(rows)) => completed[chunk_index] = Some(rows),
                Ok(None) => pricing_failed = true,
                Err(payload) => {
                    if first_panic.is_none() {
                        first_panic = Some(payload);
                    }
                }
            }
        }
        if let Some(payload) = first_panic {
            std::panic::resume_unwind(payload);
        }
        if pricing_failed {
            return None;
        }

        let mut prices = Vec::new();
        prices.try_reserve_exact(blocks.len()).ok()?;
        for chunk in completed {
            let rows = chunk?;
            let combined_len = prices.len().checked_add(rows.len())?;
            if combined_len > blocks.len() {
                return None;
            }
            prices.extend(rows);
        }
        (prices.len() == blocks.len()).then_some(prices)
    })
}

#[allow(clippy::too_many_arguments)]
fn bounded_range_prices(
    blocks: &[ParsedBlock],
    source_plans: &[PlannedBlock],
    source_stats: &[ShortFamilyStats],
    source_extra_bits: &[u64],
    min_distance_codes: bool,
    start_index: usize,
    end_index: usize,
) -> Option<Vec<BoundedRangePrices>> {
    let mut range_bits = Vec::new();
    range_bits
        .try_reserve_exact(end_index.checked_sub(start_index)?)
        .ok()?;
    for start in start_index..end_index {
        let mut prices = [None; MAX_BOUNDED_GROUP_SPAN + 1];
        prices[1] = Some(source_plans.get(start)?.bits);
        let mut literal_frequencies = [0_u32; 286];
        let mut distance_frequencies = [0_u32; 30];
        let mut range_extra_bits = 0_u64;
        let mut range_tokens = 0_usize;
        let mut range_plain = 0_usize;
        let mut range_stats = source_stats.get(start)?.clone();

        for end in start + 1..=blocks.len().min(start + MAX_BOUNDED_GROUP_SPAN) {
            let block = &blocks[end - 1];
            range_tokens = range_tokens.checked_add(block.tokens.len())?;
            range_plain = range_plain.checked_add(block.plain.len())?;
            range_extra_bits = range_extra_bits.checked_add(*source_extra_bits.get(end - 1)?)?;
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
                range_stats.add_assign(source_stats.get(end - 1)?)?;
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
            prices[end - start] = score_short_family_frequencies(
                &literal_frequencies,
                &distance_frequencies,
                range_extra_bits,
                &range_stats,
                min_distance_codes,
            );
        }
        range_bits.push(prices);
    }
    Some(range_bits)
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
/// blocks break a run so this candidate retains the original Columbo C
/// planner's Huffman-run grouping; stored accumulation is handled separately
/// by [`prepare_blocks`].
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
    let mut plan_cache = CanonicalPlanCache::new();
    direct_structural_plan(&collected, start_alignment, options, &mut plan_cache)
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
    plan_cache: &mut CanonicalPlanCache,
    expired: &mut F,
    progress: Option<&RouteProgress>,
) -> Option<Vec<PlannedBlock>>
where
    F: FnMut() -> bool,
{
    sequential_plan_with_source_search(
        blocks,
        start_alignment,
        options,
        merge_search,
        SourceBlockSearch::Full,
        plan_cache,
        expired,
        progress,
    )
}

#[allow(clippy::too_many_arguments)]
fn sequential_plan_with_source_search<F>(
    blocks: &[ParsedBlock],
    start_alignment: u8,
    options: &Options,
    merge_search: AdjacentMergeSearch,
    source_search: SourceBlockSearch,
    plan_cache: &mut CanonicalPlanCache,
    expired: &mut F,
    progress: Option<&RouteProgress>,
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

    for (index, current) in rest.iter().enumerate() {
        let pending_block = pending.as_block();
        let alignment = ((u64::from(start_alignment) + output_bits) & 7) as u8;
        if let Some(progress) = progress {
            progress.item(index + 1, pending_block.tokens.len());
            progress.activity("Columbo token and split search");
        }
        let pending_plans = match pending_cache.take() {
            Some((cached_alignment, plans)) if cached_alignment == alignment => plans,
            _ => plan_source_with_search(
                pending_block,
                alignment,
                options,
                source_search,
                plan_cache,
                expired,
            ),
        };
        let pending_bits = total_bits(&pending_plans);
        let current_alignment = ((u64::from(alignment) + pending_bits) & 7) as u8;
        if let Some(progress) = progress {
            progress.item(index + 2, current.tokens.len());
            progress.activity("Columbo token and split search");
        }
        let current_plans = plan_source_with_search(
            current,
            current_alignment,
            options,
            source_search,
            plan_cache,
            expired,
        );
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
            if let Some(progress) = progress {
                progress.item(index + 2, merged_block.tokens.len());
                progress.activity("Testing adjacent block merge");
            }
            let candidate = plan_source_with_search(
                merged_block,
                alignment,
                options,
                source_search,
                plan_cache,
                expired,
            );
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
                score_existing_dynamic(&merged_block.tokens, source, options.strict)
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
            // merge, just as the original Columbo C pending-block loop does.
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
    if let Some(progress) = progress {
        progress.item(blocks.len(), pending.as_block().tokens.len());
        progress.activity("Columbo token and split search");
    }
    let pending_plans = match pending_cache {
        Some((cached_alignment, plans)) if cached_alignment == alignment => plans,
        _ => plan_source_with_search(
            pending.as_block(),
            alignment,
            options,
            source_search,
            plan_cache,
            expired,
        ),
    };
    append_output_plans(&mut output, &mut output_bits, pending_plans)?;
    Some(output)
}

fn plan_source_with_search<F>(
    block: &ParsedBlock,
    alignment: u8,
    options: &Options,
    search: SourceBlockSearch,
    plan_cache: &mut CanonicalPlanCache,
    expired: &mut F,
) -> Vec<PlannedBlock>
where
    F: FnMut() -> bool,
{
    match search {
        SourceBlockSearch::Full => {
            plan_source_with_splits(block, alignment, options, plan_cache, expired)
        }
        SourceBlockSearch::Narrow {
            allow_individual_prune,
        } => {
            let plan = match plan_source_block(block, alignment, options, expired) {
                Some(seed) if !expired() => plan_block_with_seeded_narrow_search(
                    block,
                    alignment,
                    options,
                    allow_individual_prune,
                    seed,
                    expired,
                ),
                Some(seed) => seed,
                None => plan_block_with_narrow_search(
                    block,
                    alignment,
                    options,
                    allow_individual_prune,
                    expired,
                ),
            };
            vec![plan]
        }
        SourceBlockSearch::Floor => {
            // Once the terminal route spends its allowance, finish the
            // complete candidate with the ordinary table selector instead of
            // starting another extended floor on every remaining block.
            let plan = if expired() {
                lookup_block_cached(block, alignment, options, plan_cache)
                    .unwrap_or_else(|| plan_block(block, alignment, options, || true))
            } else {
                let mut floor_options = options.clone();
                floor_options.exhaustive = false;
                let base = plan_block_cached(block, alignment, &floor_options, plan_cache);
                improve_plan_with_floor(block, alignment, &floor_options, true, base)
            };
            vec![plan]
        }
    }
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

/// Test Columbo's bounded one-boundary split route for one source block.
///
/// Default mode uses Columbo's seven decoded eighths. `--max` also retains
/// Columbo's compact 32-token probes and tries one Turtledeflate-inspired
/// adaptive probe before exact Columbo replanning. Children use the direct
/// block planner in default mode because they are not pending merge candidates.
fn plan_source_with_splits<F>(
    block: &ParsedBlock,
    alignment: u8,
    options: &Options,
    plan_cache: &mut CanonicalPlanCache,
    expired: &mut F,
) -> Vec<PlannedBlock>
where
    F: FnMut() -> bool,
{
    if block.tokens.len() < 16
        || block.plain.len() < 128
        || (!options.exhaustive && block.plain.len() < 32_768)
    {
        let plan = match lookup_block_cached(block, alignment, options, plan_cache) {
            Some(base) => {
                plan_block_with_complete_base_search(block, alignment, options, base, expired)
            }
            None => plan_block_with_search(block, alignment, options, expired),
        };
        return vec![plan];
    }

    let base = if options.exhaustive {
        lookup_block_cached(block, alignment, options, plan_cache)
            .unwrap_or_else(|| plan_block(block, alignment, options, &mut *expired))
    } else {
        // Default mode retains its established whole-block token search before
        // the seven inexpensive eighth probes.
        match lookup_block_cached(block, alignment, options, plan_cache) {
            Some(base) => {
                plan_block_with_complete_base_search(block, alignment, options, base, expired)
            }
            None => plan_block_with_search(block, alignment, options, expired),
        }
    };
    let complete_base = options
        .exhaustive
        .then(|| try_clone_planned_block(&base))
        .flatten();
    let mut best = vec![base];
    let mut best_bits = total_bits(&best);
    if expired() {
        return best;
    }

    let Some(composite) = Composite::new(std::slice::from_ref(block)) else {
        return best;
    };
    let source = composite.sources[0];
    let start = Cut { token: 0, plain: 0 };
    let end = Cut {
        token: block.tokens.len(),
        plain: block.plain.len(),
    };
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
    let mut ranked_splits = Vec::new();
    if ranked_splits.try_reserve_exact(cuts.len()).is_err() {
        return best;
    }
    for &split in &cuts {
        if expired() {
            break;
        }
        let boundaries = [start, split, end];
        let Some(candidate) = plan_structural_ranges(
            &composite,
            &boundaries,
            alignment,
            options,
            plan_cache,
            expired,
        ) else {
            continue;
        };
        let candidate_bits = total_bits(&candidate);
        ranked_splits.push((candidate_bits, split));
        if candidate_bits < best_bits {
            best_bits = candidate_bits;
            best = candidate;
        }
    }

    // Secure all established exact candidates before spending remaining max
    // time on adaptive discovery. If the new search reaches the deadline, the
    // eighth/32-token floor above remains available.
    if options.exhaustive && !expired() {
        let searched_base = match complete_base {
            Some(base) => {
                plan_block_with_complete_base_search(block, alignment, options, base, expired)
            }
            None => plan_block_with_search(block, alignment, options, expired),
        };
        if searched_base.bits < best_bits {
            best_bits = searched_base.bits;
            best = vec![searched_base];
        }
    }

    if options.exhaustive && !expired() {
        let mut adaptive_cut = Vec::new();
        if add_adaptive_split_cut(
            &mut adaptive_cut,
            &composite,
            start,
            end,
            options.strict,
            expired,
        )
        .is_some()
        {
            if let Some(&split) = adaptive_cut.first() {
                if !cuts.contains(&split) {
                    let boundaries = [start, split, end];
                    let candidate = plan_structural_ranges(
                        &composite,
                        &boundaries,
                        alignment,
                        options,
                        plan_cache,
                        expired,
                    );
                    if let Some(candidate) = candidate {
                        // The histogram route already discovered this cut and
                        // exact structural planning has validated it. Feeding
                        // it into the older child token-search ladder repeats
                        // expensive work for negligible observed gain.
                        let candidate_bits = total_bits(&candidate);
                        if adaptive_split_is_worth_replay(candidate_bits, best_bits) {
                            best_bits = candidate_bits;
                            best = candidate;
                        }
                    }
                }
            }
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
                let Some(candidate) = plan_structural_ranges(
                    &composite,
                    &boundaries,
                    alignment,
                    options,
                    plan_cache,
                    expired,
                ) else {
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

    // With the complete structural and whole-block floors secured, refine
    // promising split children while time remains. Ranking by direct cost
    // makes max mode deterministic and gives plausible boundaries the first
    // search slots.
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

fn adaptive_split_is_worth_replay(candidate_bits: u64, established_bits: u64) -> bool {
    candidate_bits
        .checked_add(ADAPTIVE_SPLIT_MIN_EXACT_SAVINGS_BITS)
        .is_some_and(|required_bits| required_bits <= established_bits)
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
    plan_cache: &mut CanonicalPlanCache,
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
        let plan = lookup_block_cached(&range, alignment, options, plan_cache)
            .unwrap_or_else(|| plan_block(&range, alignment, options, &mut *expired));
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

#[derive(Clone)]
struct FrequencyCheckpoint {
    literal: [u32; 286],
    distance: [u32; 30],
    extra_bits: u64,
}

impl FrequencyCheckpoint {
    fn zero() -> Self {
        Self {
            literal: [0; 286],
            distance: [0; 30],
            extra_bits: 0,
        }
    }

    fn add_token(&mut self, token: &Token) -> Option<()> {
        match *token {
            Token::Literal(value) => {
                let frequency = &mut self.literal[usize::from(value)];
                *frequency = frequency.checked_add(1)?;
            }
            Token::Match {
                length_symbol,
                distance_symbol,
                length_extra_bits,
                distance_extra_bits,
                ..
            } => {
                let literal = &mut self.literal[usize::from(length_symbol)];
                *literal = literal.checked_add(1)?;
                let distance = &mut self.distance[usize::from(distance_symbol)];
                *distance = distance.checked_add(1)?;
                self.extra_bits = self
                    .extra_bits
                    .checked_add(u64::from(length_extra_bits) + u64::from(distance_extra_bits))?;
            }
        }
        Some(())
    }

    fn checked_sub(&self, earlier: &Self) -> Option<Self> {
        let mut difference = Self::zero();
        for ((result, &after), &before) in difference
            .literal
            .iter_mut()
            .zip(&self.literal)
            .zip(&earlier.literal)
        {
            *result = after.checked_sub(before)?;
        }
        for ((result, &after), &before) in difference
            .distance
            .iter_mut()
            .zip(&self.distance)
            .zip(&earlier.distance)
        {
            *result = after.checked_sub(before)?;
        }
        difference.extra_bits = self.extra_bits.checked_sub(earlier.extra_bits)?;
        Some(difference)
    }
}

/// Concatenated view used only while evaluating boundary positions.
struct Composite<'a> {
    tokens: Cow<'a, [Token]>,
    plain: Cow<'a, [u8]>,
    /// Decoded offset at every token boundary, including the final boundary.
    token_plain_offsets: Vec<usize>,
    // Strided cumulative counts over parsed symbols and payload extra bits.
    frequency_checkpoints: Option<Vec<FrequencyCheckpoint>>,
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
        let checkpoint_count = token_count
            .checked_div(RANGE_HISTOGRAM_INTERVAL)?
            .checked_add(1)?;
        let base_optional_bytes = payload_bytes
            .checked_mul(3)?
            .checked_add(offset_bytes)?
            .checked_add(source_bytes)?;
        if base_optional_bytes > MAX_COMPOSITE_MODEL_BYTES {
            return None;
        }
        let index_fits = checkpoint_count
            .checked_mul(std::mem::size_of::<FrequencyCheckpoint>())
            .and_then(|bytes| base_optional_bytes.checked_add(bytes))
            .is_some_and(|bytes| bytes <= MAX_COMPOSITE_MODEL_BYTES);

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

        let frequency_checkpoints = if index_fits {
            let mut checkpoints = Vec::new();
            if checkpoints.try_reserve_exact(checkpoint_count).is_ok() {
                let mut frequencies = FrequencyCheckpoint::zero();
                checkpoints.push(frequencies.clone());
                for (index, token) in tokens.iter().enumerate() {
                    frequencies.add_token(token)?;
                    if (index + 1) % RANGE_HISTOGRAM_INTERVAL == 0 {
                        checkpoints.push(frequencies.clone());
                    }
                }
                debug_assert_eq!(checkpoints.len(), checkpoint_count);
                Some(checkpoints)
            } else {
                None
            }
        } else {
            None
        };

        Some(Self {
            tokens,
            plain,
            token_plain_offsets,
            frequency_checkpoints,
            sources,
        })
    }

    /// Reconstruct cumulative counts at an arbitrary token boundary.
    ///
    /// A stored checkpoint supplies the long prefix; at most 255 following
    /// tokens need to be counted directly.
    fn prefix_frequencies(&self, end: usize) -> Option<FrequencyCheckpoint> {
        if end > self.tokens.len() {
            return None;
        }
        let checkpoint = end / RANGE_HISTOGRAM_INTERVAL;
        let mut frequencies = self
            .frequency_checkpoints
            .as_ref()?
            .get(checkpoint)?
            .clone();
        let checkpoint_token = checkpoint.checked_mul(RANGE_HISTOGRAM_INTERVAL)?;
        for token in &self.tokens[checkpoint_token..end] {
            frequencies.add_token(token)?;
        }
        Some(frequencies)
    }

    /// Count one token range by subtracting independently reconstructed
    /// prefixes. The optional index makes work independent of the range's
    /// interior length; near the memory ceiling, a direct scan preserves the
    /// established structural route without exceeding its model budget.
    fn range_frequencies(&self, start: usize, end: usize) -> Option<FrequencyCheckpoint> {
        if start > end || end > self.tokens.len() {
            return None;
        }
        if self.frequency_checkpoints.is_none() {
            let mut frequencies = FrequencyCheckpoint::zero();
            for token in &self.tokens[start..end] {
                frequencies.add_token(token)?;
            }
            frequencies.literal[256] = frequencies.literal[256].checked_add(1)?;
            return Some(frequencies);
        }
        let start_prefix = self.prefix_frequencies(start)?;
        let end_prefix = self.prefix_frequencies(end)?;
        let mut frequencies = end_prefix.checked_sub(&start_prefix)?;
        frequencies.literal[256] = frequencies.literal[256].checked_add(1)?;
        Some(frequencies)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Cut {
    token: usize,
    plain: usize,
}

/// Candidate boundaries include Columbo's seven eighth-position probes.
///
/// DeflOpt 2.07 does not split blocks. Columbo adds these probes and snaps a
/// target inside a match to the preceding token boundary so the match remains
/// intact.
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
        // Columbo's original C probe chooses the boundary before the token
        // containing the target. Its inclusive end test also keeps the
        // strictly preceding boundary for an exact token endpoint.
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct AdaptiveSplit {
    token: usize,
    bits: u64,
}

/// Add one max-only cut using Columbo's independent, bounded implementation of
/// the coarse-to-fine concept in Turtledeflate's
/// `turtledeflate_best_block_split` at commit 756f844.
///
/// Both methods sample evenly spaced costs, smooth neighbouring samples,
/// narrow around the best basin, and scan the terminal interval. Columbo uses
/// eight samples, a three-point filter, a 16-token terminal window, and hard
/// probe/deadline caps. It omits Turtledeflate's alternate edge-basin stack;
/// the caller accepts the cut only after exact token-preserving replanning and
/// a material complete-plan win.
fn add_adaptive_split_cut<F>(
    cuts: &mut Vec<Cut>,
    composite: &Composite,
    start: Cut,
    end: Cut,
    min_distance_codes: bool,
    expired: &mut F,
) -> Option<()>
where
    F: FnMut() -> bool,
{
    let token_count = end.token.checked_sub(start.token)?;
    let plain_count = end.plain.checked_sub(start.plain)?;
    if !(ADAPTIVE_SPLIT_MIN_TOKENS..=MAX_MERGED_TOKENS).contains(&token_count)
        || plain_count < 128
        || composite.frequency_checkpoints.is_none()
    {
        return Some(());
    }

    let whole = composite.range_frequencies(start.token, end.token)?;
    let unsplit_bits = estimate_histogram_range_bits(&whole, plain_count, min_distance_codes);
    let mut score_split = |token: usize| {
        let split_plain = *composite.token_plain_offsets.get(token)?;
        if split_plain <= start.plain || split_plain >= end.plain {
            return None;
        }

        let left = composite.range_frequencies(start.token, token)?;
        let mut right = whole.checked_sub(&left)?;
        // Both complete range histograms contain one end-of-block count. Their
        // difference contains none, while the prospective right child needs
        // exactly one.
        right.literal[256] = 1;
        let left_bits = estimate_histogram_range_bits(
            &left,
            split_plain.checked_sub(start.plain)?,
            min_distance_codes,
        );
        let right_bits = estimate_histogram_range_bits(
            &right,
            end.plain.checked_sub(split_plain)?,
            min_distance_codes,
        );
        left_bits.checked_add(right_bits)
    };

    let Some(candidate) = coarse_to_fine_split(start.token, end.token, &mut score_split, expired)
    else {
        return Some(());
    };
    if candidate.bits < unsplit_bits {
        add_cut(cuts, composite, candidate.token)?;
    }
    Some(())
}

fn estimate_histogram_range_bits(
    frequencies: &FrequencyCheckpoint,
    plain_len: usize,
    min_distance_codes: bool,
) -> u64 {
    let stored = stored_block_bits(0, plain_len);
    estimate_boundary_block_bits(
        &frequencies.literal,
        &frequencies.distance,
        frequencies.extra_bits,
        min_distance_codes,
    )
    .map_or(stored, |huffman| huffman.min(stored))
}

/// Search kernel for `add_adaptive_split_cut`.
///
/// This independent Columbo implementation is substantially different from
/// Turtledeflate's `turtledeflate_best_block_split`; it is not a translation
/// or exact recreation.
fn coarse_to_fine_split<S, F>(
    start: usize,
    end: usize,
    score: &mut S,
    expired: &mut F,
) -> Option<AdaptiveSplit>
where
    S: FnMut(usize) -> Option<u64>,
    F: FnMut() -> bool,
{
    if end <= start.checked_add(2)? {
        return None;
    }
    let original_midpoint = start + (end - start) / 2;
    let mut range_start = start + 1;
    let mut range_end = end - 1;
    let mut probes = 0_usize;
    let mut cache = Vec::<AdaptiveSplit>::new();
    cache.try_reserve_exact(ADAPTIVE_SPLIT_MAX_PROBES).ok()?;
    // Always retain the original midpoint. Besides being a useful probe, it
    // makes a completely flat score choose two balanced children.
    cached_adaptive_split_score(original_midpoint, &mut probes, &mut cache, score, expired)?;

    while range_end - range_start > ADAPTIVE_SPLIT_FINAL_WIDTH {
        if expired() {
            return None;
        }
        let span = range_end - range_start;
        let mut samples = Vec::new();
        samples
            .try_reserve_exact(ADAPTIVE_SPLIT_INTERVALS + 1)
            .ok()?;
        for interval in 0..=ADAPTIVE_SPLIT_INTERVALS {
            let token = range_start + span.checked_mul(interval)? / ADAPTIVE_SPLIT_INTERVALS;
            if samples
                .last()
                .is_some_and(|sample: &AdaptiveSplit| sample.token == token)
            {
                continue;
            }
            let bits = cached_adaptive_split_score(token, &mut probes, &mut cache, score, expired)?;
            samples.push(AdaptiveSplit { token, bits });
        }
        if samples.len() < 2 {
            return None;
        }

        let interval_midpoint = range_start + span / 2;
        let mut selected = 0_usize;
        let mut selected_key = (u128::MAX, usize::MAX, usize::MAX);
        for (index, sample) in samples.iter().enumerate() {
            let left = samples[index.saturating_sub(1)].bits;
            let right = samples[(index + 1).min(samples.len() - 1)].bits;
            let filtered = u128::from(left)
                .checked_add(u128::from(sample.bits))?
                .checked_add(u128::from(right))?;
            let key = (
                filtered,
                sample.token.abs_diff(interval_midpoint),
                sample.token,
            );
            if key < selected_key {
                selected = index;
                selected_key = key;
            }
        }

        let left_index = selected.saturating_sub(ADAPTIVE_SPLIT_CENTER_RADIUS);
        let right_index = (selected + ADAPTIVE_SPLIT_CENTER_RADIUS).min(samples.len() - 1);
        let next_start = samples[left_index].token;
        let next_end = samples[right_index].token;
        if next_start == range_start && next_end == range_end {
            break;
        }
        range_start = next_start;
        range_end = next_end;
    }

    if expired() {
        return None;
    }
    for token in range_start..=range_end {
        cached_adaptive_split_score(token, &mut probes, &mut cache, score, expired)?;
    }

    cache.into_iter().min_by_key(|candidate| {
        (
            candidate.bits,
            candidate.token.abs_diff(original_midpoint),
            candidate.token,
        )
    })
}

fn cached_adaptive_split_score<S, F>(
    token: usize,
    probes: &mut usize,
    cache: &mut Vec<AdaptiveSplit>,
    score: &mut S,
    expired: &mut F,
) -> Option<u64>
where
    S: FnMut(usize) -> Option<u64>,
    F: FnMut() -> bool,
{
    if let Some(candidate) = cache.iter().find(|candidate| candidate.token == token) {
        return Some(candidate.bits);
    }
    if *probes >= ADAPTIVE_SPLIT_MAX_PROBES || expired() {
        return None;
    }
    let bits = score(token)?;
    *probes += 1;
    cache.push(AdaptiveSplit { token, bits });
    Some(bits)
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
    /// Retain a lightweight structural template while another candidate
    /// family takes ownership of the complete base plan.
    fn try_from_planned(plan: &PlannedBlock) -> Option<Self> {
        Some(Self {
            representation: plan.representation.try_clone()?,
            bits: plan.bits,
            source_type: plan.source_type,
        })
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
    plan_cache: &mut CanonicalPlanCache,
    expired: &mut F,
    progress: Option<&RouteProgress>,
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
        if let Some(progress) = progress {
            progress.advance(start_index);
        }
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
            if expired() {
                return None;
            }
            let Some(edge) = prepare_edge(
                blocks,
                composite,
                start,
                end,
                options,
                plan_cache,
                &mut *expired,
            ) else {
                continue;
            };

            for (reachable_index, &(alignment, prefix_bits)) in
                reachable[..reachable_len].iter().enumerate()
            {
                // Match the former per-alignment loop: a plan that reaches the
                // deadline is still allowed to update its first DP state.
                if reachable_index != 0 && expired() {
                    return None;
                }
                let template = edge.plan(alignment);
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
        // The original Columbo C default route tests one boundary at a time:
        // a candidate is a prefix or suffix, not an arbitrary middle slice.
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
        // Default mode follows the original Columbo C implementation's
        // single-split merge route. At least one end of a cross-source segment
        // stays anchored to an original boundary; free-floating middle ranges
        // belong to the broader --max DP.
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

/// One boundary-DP edge with its alignment-independent Huffman work cached.
///
/// The old loop rebuilt and recopied the same range for every reachable bit
/// alignment. Only stored padding and exact stored-source reuse vary across
/// those eight states.
struct PreparedEdge {
    plain_len: usize,
    source_type: SourceBlockType,
    original: Option<OriginalBits>,
    reusable: ReusableBlockPlan,
    shared_dynamic: Option<DynamicPlan>,
}

impl PreparedEdge {
    fn plan(&self, alignment: u8) -> PlanTemplate {
        let original = self.original.filter(|original| {
            original.block_type != SourceBlockType::Stored || original.alignment == alignment
        });

        // Prefer a strictly cheaper shared tree without first copying a
        // dynamic base table. If that optional allocation fails, the complete
        // ordinary plan below remains available.
        let mut shared_clone_failed = false;
        if let Some(shared) = self.shared_dynamic.as_ref() {
            let theoretical_base_bits =
                self.reusable
                    .bits_at_alignment(self.plain_len, alignment, original);
            if shared.bits < theoretical_base_bits {
                if let Some(shared) = shared.try_clone() {
                    return PlanTemplate {
                        bits: shared.bits,
                        representation: Representation::Dynamic(shared),
                        source_type: self.source_type,
                    };
                }
                shared_clone_failed = true;
            }
        }

        let (representation, bits) =
            self.reusable
                .at_alignment(self.plain_len, alignment, original);
        // A failed base-table clone can select a dearer allocation-free
        // fallback. Give the shared table one chance against that fallback.
        if !shared_clone_failed {
            if let Some(shared) = self
                .shared_dynamic
                .as_ref()
                .filter(|shared| shared.bits < bits)
                .and_then(DynamicPlan::try_clone)
            {
                return PlanTemplate {
                    bits: shared.bits,
                    representation: Representation::Dynamic(shared),
                    source_type: self.source_type,
                };
            }
        }
        PlanTemplate {
            representation,
            bits,
            source_type: self.source_type,
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn prepare_edge<F>(
    blocks: &[ParsedBlock],
    composite: &Composite,
    start: Cut,
    end: Cut,
    options: &Options,
    plan_cache: &mut CanonicalPlanCache,
    expired: &mut F,
) -> Option<PreparedEdge>
where
    F: FnMut() -> bool,
{
    // Boundary DP is deliberately token-preserving. Match-to-literal spelling
    // searches run in the complete sequential pass and again after every
    // accepted structural replay; retaining them in each DP state would make
    // memory proportional to cuts × alignments rather than to the input.
    if let Some(source_index) = exact_source(composite, start, end) {
        let block = &blocks[source_index];
        let reusable = plan_cache
            .lookup_reusable(block, options)
            .unwrap_or_else(|| plan_reusable_block(block, options, expired));
        return Some(PreparedEdge {
            plain_len: block.plain.len(),
            source_type: block.source_type,
            original: usable_original(block, options.strict),
            reusable,
            shared_dynamic: None,
        });
    }

    let range = make_range(composite, start, end)?;
    // Keep ordinary fixed/dynamic pricing ahead of the optional shared-table
    // probe, preserving the established deadline priority.
    let reusable = plan_cache
        .lookup_reusable(&range, options)
        .unwrap_or_else(|| plan_reusable_block(&range, options, &mut *expired));
    let source_range = overlapping_source_range(composite, start, end);
    let source_spans = &composite.sources[source_range.clone()];
    let whole_sources = source_spans.first().is_some_and(|first| {
        start.token == first.token_start
            && end.token == source_spans.last().expect("first was present").token_end
    });
    let shared_dynamic = if whole_sources && source_spans.len() > 1 {
        shared_dynamic_plan(&blocks[source_range], &range.tokens, options.strict)
    } else {
        None
    };
    Some(PreparedEdge {
        plain_len: range.plain.len(),
        source_type: range.source_type,
        original: usable_original(&range, options.strict),
        reusable,
        shared_dynamic,
    })
}

/// Retain only exact-source metadata needed by the eight aligned selectors.
fn usable_original(block: &ParsedBlock, strict: bool) -> Option<OriginalBits> {
    let original = block.original?;
    reusable_original_bits(block, original.alignment, strict)
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
    let frequencies = composite.range_frequencies(start.token, end.token)?;
    let literal_frequencies = frequencies.literal;
    let distance_frequencies = frequencies.distance;
    let source_range = overlapping_source_range(composite, start, end);
    let sources = &composite.sources[source_range];
    let source_type = sources
        .iter()
        .map(|source| source.source_type)
        .reduce(merged_source_type)
        .unwrap_or(SourceBlockType::Dynamic);
    let mut source_splits = Vec::new();
    source_splits
        .try_reserve_exact(sources.len().saturating_sub(1))
        .ok()?;
    for source in sources {
        if source.plain_end > start.plain && source.plain_end < end.plain {
            source_splits.push(source.plain_end.checked_sub(start.plain)?);
        }
    }

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
    strict: bool,
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
    score_existing_dynamic(tokens, first, strict)
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
    } else {
        // Dynamic is the common provenance for mixed source types. The DP does
        // not currently admit stored/Huffman pairs, but this remains the least
        // surprising fallback if that policy changes.
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

    #[test]
    fn cheapest_grouped_layout_retains_the_selected_route_kind() {
        let blocks: &[ParsedBlock] = &[];
        let selected = cheapest_grouped_layout([
            Some((GroupedLayout::Greedy, blocks, 30)),
            Some((GroupedLayout::Bounded, blocks, 20)),
            Some((GroupedLayout::Collected, blocks, 10)),
        ]);

        let (kind, selected_blocks) = selected.expect("one grouped layout is available");
        assert_eq!(kind, GroupedLayout::Collected);
        assert!(selected_blocks.is_empty());
    }

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

    fn short_match_block() -> ParsedBlock {
        let mut tokens = vec![Token::Literal(b'a')];
        tokens.extend((0..100).map(|_| Token::Match {
            length: 6,
            distance: 1,
            length_symbol: 260,
            distance_symbol: 0,
            length_extra: 0,
            distance_extra: 0,
            length_extra_bits: 0,
            distance_extra_bits: 0,
        }));
        let (literal_frequencies, distance_frequencies) = count_frequencies(&tokens);
        ParsedBlock {
            tokens: tokens.into(),
            plain: vec![b'a'; 601].into(),
            literal_frequencies,
            distance_frequencies,
            original_literal_lengths: None,
            original_distance_lengths: None,
            original_dynamic: None,
            original: None,
            source_splits: Vec::new(),
            source_type: SourceBlockType::Dynamic,
        }
    }

    #[test]
    fn strided_range_histograms_match_direct_recounting() {
        let tokens: Vec<_> = (0..700)
            .map(|index| {
                if index % 7 == 0 {
                    Token::Match {
                        length: 11,
                        distance: 5,
                        length_symbol: 265,
                        distance_symbol: 4,
                        length_extra: 0,
                        distance_extra: 0,
                        length_extra_bits: 1,
                        distance_extra_bits: 1,
                    }
                } else {
                    Token::Literal((index % 251) as u8)
                }
            })
            .collect();
        let plain_len = tokens.iter().map(|token| token.decoded_len()).sum();
        let (literal_frequencies, distance_frequencies) = count_frequencies(&tokens);
        let block = ParsedBlock {
            tokens: tokens.into(),
            plain: vec![0; plain_len].into(),
            literal_frequencies,
            distance_frequencies,
            original_literal_lengths: None,
            original_distance_lengths: None,
            original_dynamic: None,
            original: None,
            source_splits: Vec::new(),
            source_type: SourceBlockType::Dynamic,
        };
        let blocks = [block];
        let mut composite = Composite::new(&blocks).unwrap();

        for (start, end) in [
            (0, 0),
            (0, 1),
            (1, 50),
            (10, 20),
            (0, 255),
            (0, 256),
            (0, 257),
            (17, 257),
            (250, 520),
            (256, 512),
            (255, 511),
            (256, 512),
            (257, 513),
            (257, 699),
            (699, 700),
            (0, 700),
        ] {
            let indexed = composite.range_frequencies(start, end).unwrap();
            let direct = count_frequencies(&composite.tokens[start..end]);
            assert_eq!(indexed.literal, direct.0, "{start}..{end}");
            assert_eq!(indexed.distance, direct.1, "{start}..{end}");
            assert_eq!(
                indexed.extra_bits,
                token_extra_bits(&composite.tokens[start..end]),
                "{start}..{end}"
            );
        }

        // Near the optional-model ceiling, Columbo preserves the established
        // structural route without the index and falls back to a direct scan.
        composite.frequency_checkpoints = None;
        let fallback = composite.range_frequencies(257, 699).unwrap();
        let direct = count_frequencies(&composite.tokens[257..699]);
        assert_eq!(fallback.literal, direct.0);
        assert_eq!(fallback.distance, direct.1);
        assert_eq!(
            fallback.extra_bits,
            token_extra_bits(&composite.tokens[257..699])
        );
    }

    #[test]
    fn coarse_to_fine_split_finds_an_off_grid_minimum_under_its_probe_cap() {
        let mut probes = 0_usize;
        let mut score = |token: usize| {
            probes += 1;
            let distance = token.abs_diff(733) as u64;
            Some(distance * distance)
        };
        let candidate =
            coarse_to_fine_split(0, 2_048, &mut score, &mut || false).expect("a legal cut");

        assert_eq!(candidate.token, 733);
        assert_eq!(candidate.bits, 0);
        assert!(probes <= ADAPTIVE_SPLIT_MAX_PROBES);
    }

    #[test]
    fn coarse_to_fine_split_centres_flat_ties() {
        let candidate =
            coarse_to_fine_split(0, 2_048, &mut |_| Some(1), &mut || false).expect("a legal cut");

        assert_eq!(candidate.token, 1_024);
    }

    #[test]
    fn adaptive_histogram_cut_finds_a_literal_transition() {
        let mut bytes = vec![b'a'; 733];
        bytes.extend(std::iter::repeat(b'z').take(1_315));
        let block = literal_block(&bytes, SourceBlockType::Dynamic);
        let blocks = [block];
        let composite = Composite::new(&blocks).unwrap();
        let mut cuts = Vec::new();
        let start = Cut { token: 0, plain: 0 };
        let end = Cut {
            token: bytes.len(),
            plain: bytes.len(),
        };

        add_adaptive_split_cut(&mut cuts, &composite, start, end, false, &mut || false).unwrap();

        assert_eq!(
            cuts,
            [Cut {
                token: 733,
                plain: 733,
            }]
        );
    }

    #[test]
    fn adaptive_split_skips_small_ranges_and_respects_an_expired_deadline() {
        let small = literal_block(&vec![b'x'; 512], SourceBlockType::Dynamic);
        let small_blocks = [small];
        let small_composite = Composite::new(&small_blocks).unwrap();
        let mut cuts = Vec::new();
        add_adaptive_split_cut(
            &mut cuts,
            &small_composite,
            Cut { token: 0, plain: 0 },
            Cut {
                token: 512,
                plain: 512,
            },
            false,
            &mut || false,
        )
        .unwrap();
        assert!(cuts.is_empty());

        let mut scorer_called = false;
        let candidate = coarse_to_fine_split(
            0,
            2_048,
            &mut |_| {
                scorer_called = true;
                Some(0)
            },
            &mut || true,
        );
        assert!(candidate.is_none());
        assert!(!scorer_called);
    }

    #[test]
    fn adaptive_split_requires_a_material_exact_saving() {
        assert!(!adaptive_split_is_worth_replay(969, 1_000));
        assert!(adaptive_split_is_worth_replay(968, 1_000));
        assert!(!adaptive_split_is_worth_replay(u64::MAX, u64::MAX));
    }

    fn assert_same_plan(left: &PlannedBlock, right: &PlannedBlock) {
        assert_eq!(left.bits, right.bits);
        assert_eq!(left.tokens, right.tokens);
        assert_eq!(left.plain, right.plain);
        assert_eq!(left.source_type, right.source_type);
        match (&left.representation, &right.representation) {
            (Representation::Original(left), Representation::Original(right)) => {
                assert_eq!(left, right);
            }
            (Representation::Stored, Representation::Stored)
            | (Representation::Fixed, Representation::Fixed) => {}
            (Representation::Dynamic(left), Representation::Dynamic(right)) => {
                assert_eq!(left, right);
            }
            pair => panic!("different representations: {pair:?}"),
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
    fn mandatory_floor_reuse_matches_independent_candidate_order() {
        let block = short_match_block();
        let options = Options {
            exhaustive: true,
            ..Options::default()
        };
        let mut expected_cache = CanonicalPlanCache::new();
        let floor = plan_block_with_floor_cached(&block, 3, &options, true, &mut expected_cache);
        let short =
            plan_block_with_short_family_floor_cached(&block, 3, &options, &mut expected_cache);
        let expected = if short.bits < floor.bits {
            short
        } else {
            floor
        };

        let mut actual_cache = CanonicalPlanCache::new();
        let actual = mandatory_token_floor_plan(
            std::slice::from_ref(&block),
            3,
            &options,
            &mut actual_cache,
        )
        .expect("one valid block always has a complete floor")
        .pop()
        .expect("one source block produces one plan");
        assert_same_plan(&actual, &expected);
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
        let relaxed = Options {
            strict: false,
            ..Options::default()
        };
        let after_deadline = plan_stream(&blocks, 0, &relaxed, &mut || true).unwrap();
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
        let mut plan_cache = CanonicalPlanCache::new();
        let plans = source_aligned_huffman_floor(&blocks, 0, &options, &mut plan_cache).unwrap();

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
    fn canonical_cache_reuses_source_intervals_across_routes() {
        let blocks = [
            literal_block(&vec![b'a'; 800], SourceBlockType::Dynamic),
            literal_block(&vec![b'b'; 800], SourceBlockType::Dynamic),
            literal_block(&vec![b'c'; 800], SourceBlockType::Dynamic),
        ];
        let options = Options::default();
        let mut plan_cache = CanonicalPlanCache::new();

        source_aligned_huffman_floor(&blocks, 0, &options, &mut plan_cache).unwrap();
        let after_source_aligned = plan_cache.stats();
        let mandatory = mandatory_token_floor_plan(&blocks, 0, &options, &mut plan_cache).unwrap();
        let after_mandatory = plan_cache.stats();

        assert_eq!(
            after_mandatory.hits - after_source_aligned.hits,
            blocks.len()
        );
        assert_eq!(after_mandatory.inserts, after_source_aligned.inserts);
        let decoded: Vec<_> = mandatory
            .iter()
            .flat_map(|plan| plan.plain.iter().copied())
            .collect();
        assert_eq!(
            decoded,
            blocks
                .iter()
                .flat_map(|block| block.plain.iter().copied())
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn floor_search_caches_only_the_bounded_header_policy() {
        let block = literal_block(&vec![b'a'; 800], SourceBlockType::Dynamic);
        let exhaustive = Options {
            exhaustive: true,
            ..Options::default()
        };
        let mut plan_cache = CanonicalPlanCache::new();
        let plans = plan_source_with_search(
            &block,
            0,
            &exhaustive,
            SourceBlockSearch::Floor,
            &mut plan_cache,
            &mut || false,
        );
        assert_eq!(plans.len(), 1);

        let bounded = Options::default();
        let before = plan_cache.stats();
        assert!(lookup_block_cached(&block, 0, &bounded, &mut plan_cache).is_some());
        let after = plan_cache.stats();
        assert_eq!(after.hits, before.hits + 1);
        assert_eq!(after.inserts, before.inserts);
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
    fn parallel_bounded_grouping_matches_the_serial_candidate() {
        let blocks: Vec<_> = (0..20)
            .map(|_| {
                let mut tokens = vec![Token::Literal(b'a')];
                tokens.extend((0..100).map(|_| Token::Match {
                    length: 11,
                    distance: 1,
                    length_symbol: 265,
                    distance_symbol: 0,
                    length_extra: 0,
                    distance_extra: 0,
                    length_extra_bits: 1,
                    distance_extra_bits: 0,
                }));
                let (literal_frequencies, distance_frequencies) = count_frequencies(&tokens);
                ParsedBlock {
                    tokens: tokens.into(),
                    plain: vec![b'a'; 1_101].into(),
                    literal_frequencies,
                    distance_frequencies,
                    original_literal_lengths: None,
                    original_distance_lengths: None,
                    original_dynamic: None,
                    original: None,
                    source_splits: Vec::new(),
                    source_type: SourceBlockType::Dynamic,
                }
            })
            .collect();
        let options = Options::default();
        let mut serial_cache = CanonicalPlanCache::new();
        let serial = bounded_huffman_grouping(
            &blocks,
            &options,
            BoundedRangePricing::Serial,
            &mut serial_cache,
        )
        .unwrap();
        let mut parallel_cache = CanonicalPlanCache::new();
        let parallel = bounded_huffman_grouping(
            &blocks,
            &options,
            BoundedRangePricing::Parallel,
            &mut parallel_cache,
        )
        .unwrap();

        // The same-distance floor can make each synthetic source block as
        // cheap as its grouped form. This test requires identical serial and
        // parallel decisions, whether or not a merge remains profitable.
        assert_eq!(parallel.len(), serial.len());
        for (parallel, serial) in parallel.iter().zip(&serial) {
            assert_eq!(parallel.tokens, serial.tokens);
            assert_eq!(parallel.plain, serial.plain);
            assert_eq!(parallel.literal_frequencies, serial.literal_frequencies);
            assert_eq!(parallel.distance_frequencies, serial.distance_frequencies);
            assert_eq!(parallel.source_splits, serial.source_splits);
            assert_eq!(parallel.source_type, serial.source_type);
        }

        let serial_plans = finish_plan(
            direct_structural_plan(&serial, 0, &options, &mut serial_cache).unwrap(),
            &options,
        );
        let parallel_plans =
            plan_columbo_floor_seeded_bounded_grouping(&blocks, 0, &options).unwrap();
        assert_eq!(parallel_plans.len(), serial_plans.len());
        for (parallel, serial) in parallel_plans.iter().zip(&serial_plans) {
            assert_same_plan(parallel, serial);
        }
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
        let mut plan_cache = CanonicalPlanCache::new();
        let mandatory =
            mandatory_token_floor_plan(&blocks, 0, &Options::default(), &mut plan_cache).unwrap();
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
        let mut plan_cache = CanonicalPlanCache::new();
        let plans =
            direct_structural_plan(&exhaustive, 0, &Options::default(), &mut plan_cache).unwrap();
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

        let mut plan_cache = CanonicalPlanCache::new();
        let plans = sequential_plan(
            &blocks,
            0,
            &options,
            AdjacentMergeSearch::Local,
            &mut plan_cache,
            &mut || false,
            None,
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

        let mut plan_cache = CanonicalPlanCache::new();
        let adjacent = sequential_plan(
            &blocks,
            0,
            &options,
            AdjacentMergeSearch::LongRun,
            &mut plan_cache,
            &mut || false,
            None,
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
            &mut plan_cache,
            &mut || false,
            None,
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
        let mut plan_cache = CanonicalPlanCache::new();
        let separate = sequential_plan(
            &blocks,
            0,
            &options,
            AdjacentMergeSearch::Disabled,
            &mut plan_cache,
            &mut || false,
            None,
        )
        .unwrap();
        let merged = sequential_plan(
            &blocks,
            0,
            &options,
            AdjacentMergeSearch::LongRun,
            &mut plan_cache,
            &mut || false,
            None,
        )
        .unwrap();

        assert!(total_bits(&merged) < total_bits(&separate));
        assert_eq!(merged.len(), 1);
        assert_eq!(merged[0].plain.len(), 2_000);
    }

    #[test]
    fn direct_no_split_route_is_additive_and_preserves_decoded_bytes() {
        let options = Options {
            exhaustive: true,
            ..Options::default()
        };
        let blocks = [
            literal_block(&vec![b'a'; 800], SourceBlockType::Dynamic),
            literal_block(&vec![b'a'; 800], SourceBlockType::Dynamic),
            literal_block(&vec![b'a'; 800], SourceBlockType::Dynamic),
        ];
        let mut plan_cache = CanonicalPlanCache::new();
        let fallback = direct_structural_plan(&blocks, 0, &options, &mut plan_cache).unwrap();
        let route = plan_source_no_split_route(&blocks, 0, &options, true, &mut || false)
            .expect("the direct route retains a complete fallback");

        assert!(total_bits(&route) <= total_bits(&fallback));
        let decoded: Vec<_> = route
            .iter()
            .flat_map(|plan| plan.plain.iter().copied())
            .collect();
        assert_eq!(decoded, vec![b'a'; 2_400]);
    }

    #[test]
    fn terminal_merge_returns_only_a_strict_complete_stream_win() {
        let options = Options::default();
        let blocks = [
            literal_block(&vec![b'a'; 800], SourceBlockType::Dynamic),
            literal_block(&vec![b'a'; 800], SourceBlockType::Dynamic),
        ];
        let mut plan_cache = CanonicalPlanCache::new();
        let fallback = direct_structural_plan(&blocks, 0, &options, &mut plan_cache).unwrap();
        let merged = plan_terminal_merge_route(&blocks, 0, &options, &mut || false)
            .expect("equal neighbours have a profitable deterministic merge");

        assert!(total_bits(&merged) < total_bits(&fallback));
        assert_eq!(merged.len(), 1);
        assert_eq!(merged[0].plain.as_slice(), &[b'a'; 1_600]);

        assert!(plan_terminal_merge_route(&blocks, 0, &options, &mut || true).is_none());
    }

    #[test]
    fn long_huffman_route_does_not_join_large_stored_neighbours() {
        let options = Options::default();
        let blocks = [
            literal_block(&vec![b'a'; 1_000], SourceBlockType::Stored),
            literal_block(&vec![b'a'; 1_000], SourceBlockType::Stored),
        ];
        let mut plan_cache = CanonicalPlanCache::new();
        let separate = sequential_plan(
            &blocks,
            0,
            &options,
            AdjacentMergeSearch::Disabled,
            &mut plan_cache,
            &mut || false,
            None,
        )
        .unwrap();
        let long_route = sequential_plan(
            &blocks,
            0,
            &options,
            AdjacentMergeSearch::LongRun,
            &mut plan_cache,
            &mut || false,
            None,
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

        // The original Columbo C probe treats decoded offset 16 as the
        // inclusive end of the token beginning at offset 15, so the split
        // stays before it.
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
