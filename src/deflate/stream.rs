// SPDX-License-Identifier: MIT
// SPDX-FileCopyrightText: 2026 Andrew C.E. Dent <https://github.com/ace-dent>

//! Stream-level Deflate block planning.
//!
//! Block boundaries affect neither the decoded bytes nor the 32 KiB history
//! window, so a boundary may be removed or moved freely. A cut inside a match
//! splits it into fragments at the original distance; a fragment shorter than
//! three bytes becomes literals. This removes block headers and gives locally
//! different data separate Huffman codes; no new LZ77 matches are found.
//!
//! Attribution: from DeflOpt come the fixed/fixed coalescing rule and the
//! heap code-length builder that the entropy-state scout uses. Max
//! mode also runs an independent, bounded form of Turtledeflate's coarse
//! boundary search. The deft4j-derived greedy merge is in `source_recode`.

mod adaptive_split;
mod alphabet;
mod boundary_graph;
mod compact_split;
mod entropy_cut;
mod entropy_scout;
mod grouping;
mod merge;
mod routes;
mod selected_plan;
mod sequential_walk;
mod source_split;
pub(crate) use alphabet::{plan_alphabet_boundaries, AlphabetBudget};
pub(crate) use compact_split::{
    plan_compact_source_split_floor, plan_compact_source_split_floor_until,
};
pub(crate) use entropy_cut::{plan_entropy_cuts, EntropyCutBudget};
pub(crate) use grouping::plan_columbo_floor_seeded_bounded_grouping;
pub(crate) use merge::{plan_boundary_merges, plan_terminal_merges};
pub(crate) use routes::{
    fragmented_collect_seed, plan_fragmented_replay, plan_integrated_proven_source_route,
    plan_proven_submatch_route, plan_source_individual_no_split_route, plan_source_no_split_route,
    plan_terminal_merge_route,
};

use adaptive_split::{adaptive_histogram_split_search, estimate_histogram_range_bits};
use boundary_graph::{
    choose_cuts, established_boundary_graph_priority, plan_global_boundary_graph,
    source_aligned_huffman_floor, usable_original, BoundaryGraph, PreparedEdge,
};
use entropy_scout::entropy_state_boundary_cuts;
use grouping::{
    bounded_huffman_grouping, collect_huffman_runs, greedy_huffman_blocklist, BoundedRangePricing,
};
use selected_plan::{
    plan_forced_split_boundary_escape, plan_selected_huffman_boundary_reseat,
    plan_selected_huffman_merge_floor,
};
use sequential_walk::sequential_plan_with_compact_policy;

/// Smallest work budget for one R15 or R16 pass.
const MIN_TERMINAL_LAYOUT_WORK: usize = 1 << 28;
/// Work units per token and decoded byte for one R15 or R16 pass.
const TERMINAL_LAYOUT_WORK_PER_UNIT: usize = 64;

/// Work allowed for one R15 or R16 pass over `blocks`. Each recursion level or
/// merge chain costs work in proportion to the stream, so the budget scales
/// with its tokens and decoded bytes. A bounded pass leaves time for the
/// slides and merges that follow it; in Max, R12 repeats a pass that changed
/// the stream, so further cuts and merges remain reachable with more time.
fn terminal_layout_work(blocks: &[ParsedBlock]) -> usize {
    blocks
        .iter()
        .fold(0_usize, |volume, block| {
            volume
                .saturating_add(block.tokens.len())
                .saturating_add(block.plain.len())
        })
        .saturating_mul(TERMINAL_LAYOUT_WORK_PER_UNIT)
        .max(MIN_TERMINAL_LAYOUT_WORK)
}

use std::borrow::Cow;
use std::sync::Arc;

use crate::progress::RouteProgress;
use crate::sort::SortShared;
use crate::Options;

use super::block::{
    lookup_block_cached, plan_block, plan_block_cached, reusable_original_bits, stored_block_bits,
    CanonicalPlanCache,
};
use super::model::{
    canonical_length_encoding, count_frequencies, ParsedBlock, PlannedBlock, Representation,
    SourceBlockType, Token,
};
use super::search::{
    improve_plan_with_floor, improve_plan_with_integrated_proven_floor,
    improve_plan_with_same_distance_floor, improve_plan_with_short_family_floor,
    same_distance_opportunities, tighten_terminal_plan,
};
use super::stop::SearchStop;

/// Default mode regroups source blocks only for encoded streams of
/// `DEFAULT_LONG_MERGE_MIN..=DEFAULT_LONG_MERGE_MAX` bytes, the common
/// encoder-flush case. The upper bound avoids quadratic work.
const DEFAULT_LONG_MERGE_MIN: u64 = 16_000;
const DEFAULT_LONG_MERGE_MAX: u64 = 100_000;
const MAX_REGROUP_SOURCE_BLOCKS: usize = 8;
const MAX_FRAGMENTED_REPLAY_BLOCKS: usize = 12;
const MAX_MERGED_TOKENS: usize = 250_000;
const MAX_MERGED_PLAIN: usize = 64_000_000;
/// Limits for one collected run of adjacent Huffman blocks, so the source-pair
/// cuts given to the boundary DP do not grow quadratically.
const COLLECTED_RUN_MAX_TOKENS: usize = 8_192;
const COLLECTED_RUN_MAX_PLAIN: usize = 512 * 1_024;
const FRAGMENTED_COLLECT_MAX_TOKENS: usize = 4_096;
const FRAGMENTED_COLLECT_MIN_SOURCE_BLOCKS: usize = 64;
const WIDE_COLLECT_MIN_SOURCE_BLOCKS: usize = 128;
/// Largest source-block count for the greedy walk and other per-block passes;
/// more fragmented streams rely on the collected-run floor.
const MAX_GREEDY_SOURCE_BLOCKS: usize = 128;
/// Payload-copy limits for optional structural routes, which copy while the
/// parsed source is live. Two 48 MiB copies leave 32 MiB of a 128 MiB envelope
/// for the bounded DP table and Huffman metadata.
const MAX_GROUPED_MODEL_BYTES: usize = 48 * 1024 * 1024;
const MAX_COMPOSITE_MODEL_BYTES: usize = 48 * 1024 * 1024;
/// Token interval between cumulative histogram checkpoints, as in
/// Turtledeflate's `turtledeflate_create_global_histogram` (independent
/// implementation; see `docs/research/turtledeflate-methods.md`).
const RANGE_HISTOGRAM_INTERVAL: usize = 256;
const DEFAULT_INSIDE_MATCH_TOKEN_WORK: usize = 32 * 1_024;
const SOURCE_EIGHTH_SPLITS: usize = 7;
/// Keeps the boundary DP's dense eight-alignment state table small on
/// adversarially fragmented streams.
const MAX_BOUNDARY_DP_CUTS: usize = 2_048;

/// How much adjacent-source work the linear pending-block planner may do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AdjacentMergeSearch {
    /// Keep only exact fixed-block joins.
    Disabled,
    /// Search merges with small neighbors; the cheap ordinary fallback.
    Local,
    /// Rebuild every eligible adjacent Huffman pair in a long source run.
    LongRun,
}

/// Per-source search used by the sequential stream walk.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SourceBlockSearch {
    /// Ordinary planning, including source splits.
    Full {
        /// The completed compact proven lineage beat the normal floor.
        integrated_compact_proven: bool,
    },
    /// One whole-block search per source block, with or without individual
    /// match pruning; no splits or iterative work.
    Narrow { individual_prune: bool },
    /// Deterministic table floors only, for a selected terminal seed.
    Floor,
}

fn include_inside_match_cuts(exhaustive: bool, token_count: usize) -> bool {
    exhaustive
        || token_count
            .checked_mul(SOURCE_EIGHTH_SPLITS)
            .is_some_and(|work| work <= DEFAULT_INSIDE_MATCH_TOKEN_WORK)
}

/// The cheap pre-grouped layout selected for the first replay; its kind
/// prevents repeating a completed collected-layout replay.
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

/// Plan all blocks in a raw Deflate stream of `stream_bytes` encoded bytes,
/// beginning at `start_alignment`.
///
/// Splits and merges become ordinary [`PlannedBlock`] values, so the emitter
/// needs no stream-level logic.
pub(crate) fn plan_stream(
    blocks: &[ParsedBlock],
    stream_bytes: usize,
    start_alignment: u8,
    options: &Options,
    stop: &mut SearchStop<'_>,
) -> Option<Vec<PlannedBlock>> {
    let progress = RouteProgress::disabled();
    plan_stream_with_progress(
        blocks,
        stream_bytes,
        start_alignment,
        options,
        false,
        false,
        stop,
        &progress,
    )
}

/// Plan a Max candidate after the caller has kept a complete ordinary
/// candidate. Skips the full token-preserving floor that the incumbent already
/// covers, but still builds a structural fallback.
pub(crate) fn plan_stream_from_established_floor(
    blocks: &[ParsedBlock],
    stream_bytes: usize,
    start_alignment: u8,
    options: &Options,
    integrated_compact_proven: bool,
    stop: &mut SearchStop<'_>,
) -> Option<Vec<PlannedBlock>> {
    let progress = RouteProgress::disabled();
    plan_stream_with_progress(
        blocks,
        stream_bytes,
        start_alignment,
        options,
        true,
        integrated_compact_proven,
        stop,
        &progress,
    )
}

/// Plan a stream while reporting coarse progress for one long Max route.
/// Progress reporting does not change the search path.
#[allow(clippy::too_many_arguments)]
pub(crate) fn plan_stream_with_progress(
    blocks: &[ParsedBlock],
    stream_bytes: usize,
    start_alignment: u8,
    options: &Options,
    comparison_floor_secured: bool,
    integrated_compact_proven: bool,
    stop: &mut SearchStop<'_>,
    progress: &RouteProgress,
) -> Option<Vec<PlannedBlock>> {
    let detailed_progress = progress.enabled().then_some(progress);
    let mut plan_cache = CanonicalPlanCache::new();
    progress.phase(
        "Establishing complete comparison floor",
        blocks.len(),
        "source block",
        "source blocks",
    );
    // Build the linear stored-only floor before consulting a shared container
    // deadline, so a large ZIP cannot optimize only its first member.
    let stored_floor = repack_all_stored_blocks(blocks, start_alignment);

    // Once a container's file-wide budget is spent, only the stored repack may
    // apply; `None` makes `build_candidate` keep the source bytes. Strict mode
    // must still rewrite incompatible dynamic alphabets.
    let floor_time_available = !stop.reached();
    if !floor_time_available && !options.strict {
        return stored_floor.map(|floor| finish_plan(floor, options));
    }
    let source_bytes = encoded_source_bytes(blocks, stream_bytes);
    let reuse_established_floor = comparison_floor_secured;
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
    // A wide collection suits uniform runs; the bounded one above keeps several
    // local tables, as row-flushed PNG data needs.
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

    // Every layout still competes as a floor below.
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

    // Secure a complete deadline-independent floor before spelling or split
    // searches, so every stream under a shared container deadline gets
    // structural optimization.
    let mut fallback = if floor_time_available && !reuse_established_floor && !stop.reached() {
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
    if options.exhaustive && !stop.reached() {
        if let Some(candidate) = plan_selected_huffman_boundary_reseat(
            &fallback,
            start_alignment,
            options,
            &mut plan_cache,
            stop,
        ) {
            let candidate_bits = total_bits(&candidate);
            progress.checkpoint(
                "Comparison-floor boundary reseat",
                candidate_bits,
                candidate.len(),
            );
            if candidate_bits < fallback_bits {
                fallback = candidate;
                fallback_bits = candidate_bits;
            }
        }
    }
    if options.exhaustive && !stop.reached() {
        if let Some(candidate) = plan_forced_split_boundary_escape(
            &fallback,
            start_alignment,
            options,
            &mut plan_cache,
            stop,
        ) {
            let candidate_bits = total_bits(&candidate);
            progress.checkpoint(
                "Forced-split boundary escape",
                candidate_bits,
                candidate.len(),
            );
            if candidate_bits < fallback_bits {
                fallback = candidate;
                fallback_bits = candidate_bits;
            }
        }
    }

    // Search the selected grouped layout before the long source list can use
    // up the deadline.
    let mut collected_search_completed = false;
    if let Some((kind, grouped)) = selected_grouping {
        progress.phase(
            "Searching selected grouped layout",
            grouped.len(),
            "grouped block",
            "grouped blocks",
        );
        if !stop.reached() {
            if let Some(candidate) = sequential_plan_with_compact_policy(
                grouped,
                start_alignment,
                options,
                AdjacentMergeSearch::Disabled,
                integrated_compact_proven,
                &mut plan_cache,
                stop,
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

    // Only the boundary DP can combine distant cut anchors, so run it before
    // the full token beam can use up the Max deadline.
    let prioritize_boundary_graph = options.exhaustive
        && comparison_floor_secured
        && allow_regroup
        && established_boundary_graph_priority(blocks);
    let mut boundary_search_completed = false;
    if prioritize_boundary_graph && !stop.reached() {
        boundary_search_completed = true;
        if let Some(candidate) = plan_global_boundary_graph(
            blocks,
            start_alignment,
            options,
            allow_regroup,
            &mut plan_cache,
            stop,
            progress,
            detailed_progress,
        ) {
            let candidate_bits = total_bits(&candidate);
            if candidate_bits < fallback_bits {
                fallback = candidate;
                fallback_bits = candidate_bits;
            }
        }
    }

    // The source-order route stays a candidate: it suits unfragmented streams
    // and keeps source boundaries where the cheap grouping misleads.
    let fallback_merge_search = if default_long_run {
        // On a long encoder-flush chain, this fold subsumes the ordinary
        // sequential plan: a merge is kept only if it beats the exact separate
        // cost.
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
    let mut source_search_selected = false;
    if !stop.reached() {
        if let Some(candidate) = sequential_plan_with_compact_policy(
            blocks,
            start_alignment,
            options,
            fallback_merge_search,
            integrated_compact_proven,
            &mut plan_cache,
            stop,
            detailed_progress,
        ) {
            progress.advance(blocks.len());
            let candidate_bits = total_bits(&candidate);
            progress.checkpoint("Source-order result", candidate_bits, candidate.len());
            if candidate_bits < fallback_bits {
                fallback = candidate;
                fallback_bits = candidate_bits;
                source_search_selected = true;
            }
        }
    }
    // One greedy merge pass over the selected plan captures the common
    // first-replay win before the boundary graph uses the remaining Max budget.
    if options.exhaustive && source_search_selected {
        if let Some(candidate) =
            plan_selected_huffman_merge_floor(&fallback, start_alignment, options, &mut plan_cache)
        {
            let candidate_bits = total_bits(&candidate);
            progress.checkpoint(
                "Selected-plan merge cleanup",
                candidate_bits,
                candidate.len(),
            );
            if candidate_bits < fallback_bits {
                fallback = candidate;
                fallback_bits = candidate_bits;
            }
        }
    }
    if blocks.len() <= 1 && blocks.first().map_or(true, |block| block.plain.len() < 128) {
        return Some(finish_plan(fallback, options));
    }
    if stop.reached() {
        return Some(finish_plan(fallback, options));
    }

    // Also replay the collected layout: an encoder-flush chain can need several
    // source blocks under one table before any pairwise merge pays off.
    if allow_regroup && blocks.len() > MAX_REGROUP_SOURCE_BLOCKS {
        if !collected_search_completed {
            if let Some(collected_blocks) = collected_blocks.as_deref() {
                progress.phase(
                    "Replaying collected block layout",
                    collected_blocks.len(),
                    "collected block",
                    "collected blocks",
                );
                let collected = sequential_plan_with_compact_policy(
                    collected_blocks,
                    start_alignment,
                    options,
                    AdjacentMergeSearch::Disabled,
                    integrated_compact_proven,
                    &mut plan_cache,
                    stop,
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
        if stop.reached() {
            return Some(finish_plan(fallback, options));
        }

        // Default allows cross-source partial edges only on short source lists,
        // so here the remaining cuts cannot improve on the plans above; stop to
        // avoid quadratic work.
        if !options.exhaustive {
            return Some(finish_plan(fallback, options));
        }
    }

    if !allow_regroup {
        // Without regrouping there are no cross-source DP candidates, and the
        // linear fallback already has the per-source split probes.
        return Some(finish_plan(fallback, options));
    }
    if boundary_search_completed {
        return Some(finish_plan(fallback, options));
    }
    let Some(candidate) = plan_global_boundary_graph(
        blocks,
        start_alignment,
        options,
        allow_regroup,
        &mut plan_cache,
        stop,
        progress,
        detailed_progress,
    ) else {
        return Some(finish_plan(fallback, options));
    };
    let candidate_bits = total_bits(&candidate);
    if candidate_bits < fallback_bits {
        Some(finish_plan(candidate, options))
    } else {
        Some(finish_plan(fallback, options))
    }
}

/// Apply [`tighten_terminal_plan`] after the last alignment-dependent block: a
/// shorter Huffman block could shift a later stored block's padding.
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

/// Price a prepared block list without spelling or split searches. This linear
/// pass ignores the deadline.
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

/// Apply the cheap per-block floor (see `search::plan_block_with_floor`) to
/// each source block.
///
/// Lists over [`MAX_GREEDY_SOURCE_BLOCKS`] blocks get only the structural plan:
/// a token pass over hundreds of blocks would spend the container deadline on
/// bookkeeping.
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
    // A same-distance repartition changes the token boundaries that
    // proven-submatch resegmentation works on, so the floor composes the two.
    // Otherwise proven resegmentation stays in its own lineage.
    let compose_repartition_proven = same_distance_opportunities(blocks).repartition_runs != 0;
    let mut floor_options = options.clone();
    floor_options.exhaustive = false;
    for block in blocks {
        let alignment = ((u64::from(start_alignment) + output_bits) & 7) as u8;
        let base = plan_block_cached(block, alignment, &floor_options, plan_cache);
        let mut plan = if compose_repartition_proven {
            improve_plan_with_integrated_proven_floor(
                block,
                alignment,
                &floor_options,
                extended,
                base,
            )
        } else {
            improve_plan_with_floor(block, alignment, &floor_options, extended, base)
        };
        // Price the cumulative length-symbol bands (260..=264) before a
        // deadline can steer optional splitting to a locally attractive
        // two-block layout.
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

/// Repack an all-stored stream into the largest legal stored blocks (65,535
/// bytes each, except the last).
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

/// Remove empty blocks (keeping one if all are empty) and join adjacent stored
/// blocks while they fit one RFC 1951 stored block. `None` means use the
/// original blocks.
fn prepare_blocks(blocks: &[ParsedBlock]) -> Option<Vec<ParsedBlock>> {
    // Prepared blocks, the sequential winner and a collected alternative can
    // be live at once, so budget three payload copies.
    if payload_storage_bytes(blocks)?.checked_mul(3)? > MAX_GROUPED_MODEL_BYTES {
        return None;
    }
    let keep_one_empty = !blocks.iter().any(|block| !block.plain.is_empty());

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

fn same_block_layout(left: &[ParsedBlock], right: &[ParsedBlock]) -> bool {
    left.len() == right.len()
        && left.iter().zip(right).all(|(left, right)| {
            left.tokens.len() == right.tokens.len() && left.plain.len() == right.plain.len()
        })
}

/// The owned variant is large but short-lived; keeping it inline avoids an
/// extra infallible heap allocation.
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
        // A fixed join removes 10 bits and shifts every later block, so never
        // join while a stored plan, whose padding depends on alignment,
        // follows.
        let suffix_is_alignment_independent =
            last_alignment_sensitive.map_or(true, |last| last <= index);
        append_output_plan(output, output_bits, plan, suffix_is_alignment_independent)?;
    }
    Some(())
}

/// Append one plan, joining it to a fixed predecessor if `allow_fixed_join`.
/// Space is reserved first, so a failure leaves both plans unchanged.
fn append_output_plan(
    output: &mut Vec<PlannedBlock>,
    output_bits: &mut u64,
    plan: PlannedBlock,
    allow_fixed_join: bool,
) -> Option<()> {
    output.try_reserve(1).ok()?;
    if allow_fixed_join && is_fixed_plan(&plan) {
        if let Some(left) = output.last_mut().filter(|left| is_fixed_plan(left)) {
            let left_bits = left.bits;
            if try_join_fixed_plans(left, &plan).is_some() {
                *output_bits = *output_bits - left_bits + left.bits;
                return Some(());
            }
        }
    }
    *output_bits += plan.bits;
    output.push(plan);
    Some(())
}

fn plan_is_alignment_independent(plan: &PlannedBlock) -> bool {
    match plan.representation {
        Representation::Stored => false,
        Representation::Original(original) => original.block_type != SourceBlockType::Stored,
        Representation::Fixed | Representation::Dynamic(_) => true,
    }
}

fn plan_structural_ranges(
    composite: &Composite,
    boundaries: &[Cut],
    mut alignment: u8,
    options: &Options,
    plan_cache: &mut CanonicalPlanCache,
    stop: &mut SearchStop<'_>,
) -> Option<Vec<PlannedBlock>> {
    let mut plans = Vec::new();
    plans
        .try_reserve_exact(boundaries.len().saturating_sub(1))
        .ok()?;
    for pair in boundaries.windows(2) {
        if stop.reached() {
            return None;
        }
        let range = make_range(composite, pair[0], pair[1])?;
        let plan = lookup_block_cached(&range, alignment, options, plan_cache)
            .unwrap_or_else(|| plan_block(&range, alignment, options, &mut *stop));
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

/// Join a fixed plan onto a fixed predecessor, dropping one 7-bit end-of-block
/// code and one 3-bit block header (10 bits).
fn try_join_fixed_plans(left: &mut PlannedBlock, right: &PlannedBlock) -> Option<()> {
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
    Some(())
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
    /// Top-level and inherited source boundaries in decoded coordinates.
    source_plain_splits: Vec<usize>,
    /// Cumulative symbol counts and extra bits every
    /// [`RANGE_HISTOGRAM_INTERVAL`] tokens; `None` if over the model memory
    /// limit or unallocatable.
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

        // Budget three payload copies: the composite, the DP edge and range
        // plans, and the reconstructed winner.
        let payload_bytes = token_count
            .checked_mul(std::mem::size_of::<Token>())?
            .checked_add(plain_count)?;
        let offset_bytes = token_count
            .checked_add(1)?
            .checked_mul(std::mem::size_of::<usize>())?;
        let source_bytes = blocks
            .len()
            .checked_mul(std::mem::size_of::<SourceSpan>())?;
        let source_split_count = blocks
            .iter()
            .try_fold(blocks.len().saturating_sub(1), |total, block| {
                total.checked_add(block.source_splits.len())
            })?;
        let source_split_bytes = source_split_count.checked_mul(std::mem::size_of::<usize>())?;
        let checkpoint_count = token_count
            .checked_div(RANGE_HISTOGRAM_INTERVAL)?
            .checked_add(1)?;
        let base_optional_bytes = payload_bytes
            .checked_mul(3)?
            .checked_add(offset_bytes)?
            .checked_add(source_bytes)?
            .checked_add(source_split_bytes)?;
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
        let mut source_plain_splits = Vec::new();
        source_plain_splits
            .try_reserve_exact(source_split_count)
            .ok()?;
        let mut token_end = 0_usize;
        let mut plain_end = 0_usize;

        for (block_index, block) in blocks.iter().enumerate() {
            let token_start = token_end;
            let plain_start = plain_end;
            token_end += block.tokens.len();
            plain_end += block.plain.len();
            for &split in &block.source_splits {
                if split == 0 || split >= block.plain.len() {
                    return None;
                }
                source_plain_splits.push(plain_start.checked_add(split)?);
            }
            if block_index + 1 < blocks.len() {
                source_plain_splits.push(plain_end);
            }
            sources.push(SourceSpan {
                token_start,
                token_end,
                plain_start,
                plain_end,
                source_type: block.source_type,
            });
        }
        source_plain_splits.sort_shared_by(Ord::cmp);
        source_plain_splits.dedup();

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
            source_plain_splits,
            frequency_checkpoints,
            sources,
        })
    }

    /// Cumulative counts before token `end`, from the nearest checkpoint plus
    /// at most 255 counted tokens.
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

    /// Count a token range's symbols, plus one end-of-block symbol.
    fn range_frequencies(&self, start: usize, end: usize) -> Option<FrequencyCheckpoint> {
        if start > end || end > self.tokens.len() {
            return None;
        }
        let prefix_tokens = start % RANGE_HISTOGRAM_INTERVAL + end % RANGE_HISTOGRAM_INTERVAL;
        if self.frequency_checkpoints.is_none() || end - start <= prefix_tokens {
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

    /// Return the [`Cut`] at one decoded offset. An offset inside a token is
    /// valid only inside a match.
    fn cut_at_plain(&self, plain: usize) -> Option<Cut> {
        if plain > self.plain.len() {
            return None;
        }
        match self.token_plain_offsets.binary_search(&plain) {
            Ok(token) => Some(Cut { token, plain }),
            Err(next) => {
                let token = next.checked_sub(1)?;
                let start = *self.token_plain_offsets.get(token)?;
                let end = *self.token_plain_offsets.get(token.checked_add(1)?)?;
                matches!(self.tokens.get(token), Some(Token::Match { .. }))
                    .then_some(Cut { token, plain })
                    .filter(|_| start < plain && plain < end)
            }
        }
    }

    fn cut_is_token_boundary(&self, cut: Cut) -> bool {
        self.token_plain_offsets.get(cut.token).copied() == Some(cut.plain)
    }

    fn cut_is_valid(&self, cut: Cut) -> bool {
        if self.cut_is_token_boundary(cut) {
            return true;
        }
        let Some(&start) = self.token_plain_offsets.get(cut.token) else {
            return false;
        };
        let Some(next) = cut.token.checked_add(1) else {
            return false;
        };
        let Some(&end) = self.token_plain_offsets.get(next) else {
            return false;
        };
        start < cut.plain
            && cut.plain < end
            && matches!(self.tokens.get(cut.token), Some(Token::Match { .. }))
    }

    /// Token index immediately after every source token touched by `cut`.
    fn cut_end_token(&self, cut: Cut) -> Option<usize> {
        self.cut_is_valid(cut).then_some(())?;
        if self.cut_is_token_boundary(cut) {
            Some(cut.token)
        } else {
            cut.token.checked_add(1)
        }
    }

    /// Upper bound on a materialized range's token count, for work limits.
    fn range_token_count_upper_bound(&self, start: Cut, end: Cut) -> Option<usize> {
        if !self.cut_is_valid(start) || !self.cut_is_valid(end) || start.plain >= end.plain {
            return None;
        }
        let past_end = self.cut_end_token(end)?;
        let base = past_end.checked_sub(start.token)?;
        base.checked_add(usize::from(!self.cut_is_token_boundary(start)))?
            .checked_add(usize::from(!self.cut_is_token_boundary(end)))
    }

    /// Build the tokens of one decoded range. Complete tokens keep their
    /// spelling; partial matches are split as described in the module docs.
    fn materialize_range_tokens(&self, start: Cut, end: Cut) -> Option<Arc<Vec<Token>>> {
        if !self.cut_is_valid(start) || !self.cut_is_valid(end) || start.plain >= end.plain {
            return None;
        }
        if self.cut_is_token_boundary(start) && self.cut_is_token_boundary(end) {
            return try_shared_slice(self.tokens.get(start.token..end.token)?);
        }

        let past_end = self.cut_end_token(end)?;
        let source_count = past_end.checked_sub(start.token)?;
        let capacity = source_count.checked_add(2)?;
        let mut tokens = Vec::new();
        tokens.try_reserve_exact(capacity).ok()?;

        for token_index in start.token..past_end {
            let token = *self.tokens.get(token_index)?;
            let token_start = *self.token_plain_offsets.get(token_index)?;
            let token_end = *self.token_plain_offsets.get(token_index.checked_add(1)?)?;
            let fragment_start = token_start.max(start.plain);
            let fragment_end = token_end.min(end.plain);
            if fragment_start >= fragment_end {
                continue;
            }
            if fragment_start == token_start && fragment_end == token_end {
                tokens.push(token);
                continue;
            }
            let decoded = self.plain.get(fragment_start..fragment_end)?;
            append_proven_match_fragment(&mut tokens, token, decoded)?;
        }

        let decoded_len = tokens.iter().try_fold(0_usize, |total, token| {
            total.checked_add(token.decoded_len())
        })?;
        (decoded_len == end.plain.checked_sub(start.plain)?).then(|| Arc::new(tokens))
    }
}

/// A cut at decoded offset `plain`. `token` is the index of the next token, or
/// of the match that contains the offset.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Cut {
    token: usize,
    plain: usize,
}

/// Append a fragment of a source match at the same distance, or as literals
/// when it is shorter than three bytes.
fn append_proven_match_fragment(
    output: &mut Vec<Token>,
    source: Token,
    decoded: &[u8],
) -> Option<()> {
    let Token::Match {
        distance,
        distance_symbol,
        distance_extra,
        distance_extra_bits,
        ..
    } = source
    else {
        return None;
    };
    if decoded.len() < 3 {
        output.extend(decoded.iter().copied().map(Token::Literal));
        return Some(());
    }
    let length: u16 = decoded.len().try_into().ok()?;
    let (length_symbol, length_extra, length_extra_bits) = canonical_length_encoding(length)?;
    output.push(Token::Match {
        length,
        distance,
        length_symbol,
        distance_symbol,
        length_extra,
        distance_extra,
        length_extra_bits,
        distance_extra_bits,
    });
    Some(())
}

fn add_eighth_cuts(
    cuts: &mut Vec<Cut>,
    composite: &Composite,
    token_start: usize,
    token_end: usize,
    plain_start: usize,
    plain_end: usize,
    include_inside_matches: bool,
) -> Option<()> {
    if token_end - token_start < 16 || plain_end - plain_start < 128 {
        return Some(());
    }
    let plain_len = plain_end - plain_start;
    for eighth in 1..8 {
        let target = plain_start + plain_len * eighth / 8;
        // Cut before the token containing the target; a target on a token
        // boundary counts as the inclusive end of the earlier token.
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
    if include_inside_matches {
        add_exact_eighth_cuts(
            cuts,
            composite,
            token_start,
            token_end,
            plain_start,
            plain_end,
        )?;
    }
    Some(())
}

fn add_exact_eighth_cuts(
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
        if let Some(exact) = composite
            .cut_at_plain(target)
            .filter(|&cut| !composite.cut_is_token_boundary(cut))
        {
            push_optional_cut(cuts, composite, exact)?;
        }
    }
    Some(())
}

// Out of line: each call scans the existing cuts.
#[inline(never)]
fn add_cut(cuts: &mut Vec<Cut>, composite: &Composite, token: usize) -> Option<()> {
    if let Some(&plain) = composite.token_plain_offsets.get(token) {
        push_cut(cuts, composite, Cut { token, plain })?;
    }
    Some(())
}

fn push_cut(cuts: &mut Vec<Cut>, composite: &Composite, cut: Cut) -> Option<()> {
    if !composite.cut_is_valid(cut) || cuts.iter().any(|existing| existing.plain == cut.plain) {
        return Some(());
    }
    // `None` abandons only this optional boundary search.
    if cuts.len() >= MAX_BOUNDARY_DP_CUTS {
        return None;
    }
    cuts.try_reserve(1).ok()?;
    cuts.push(cut);
    Some(())
}

fn push_optional_cut(cuts: &mut Vec<Cut>, composite: &Composite, cut: Cut) -> Option<()> {
    if cuts.len() >= MAX_BOUNDARY_DP_CUTS {
        return Some(());
    }
    push_cut(cuts, composite, cut)
}

/// Order cuts by decoded offset. Every caller shares this one sort.
#[inline(never)]
fn sort_cuts_by_plain(cuts: &mut [Cut]) {
    cuts.sort_unstable_by_key(|cut| cut.plain);
}

fn overlapping_source_range(composite: &Composite, start: Cut, end: Cut) -> std::ops::Range<usize> {
    let first = composite
        .sources
        .partition_point(|source| source.plain_end <= start.plain);
    let past_last = composite
        .sources
        .partition_point(|source| source.plain_start < end.plain);
    first..past_last
}

fn make_range(composite: &Composite, start: Cut, end: Cut) -> Option<ParsedBlock> {
    let token_boundary_range =
        composite.cut_is_token_boundary(start) && composite.cut_is_token_boundary(end);
    let tokens = composite.materialize_range_tokens(start, end)?;
    let (literal_frequencies, distance_frequencies) = if token_boundary_range {
        let frequencies = composite.range_frequencies(start.token, end.token)?;
        (frequencies.literal, frequencies.distance)
    } else {
        count_frequencies(&tokens)
    };
    let source_range = overlapping_source_range(composite, start, end);
    let sources = &composite.sources[source_range];
    let source_type = sources
        .iter()
        .map(|source| source.source_type)
        .reduce(merged_source_type)
        .unwrap_or(SourceBlockType::Dynamic);
    let first_split = composite
        .source_plain_splits
        .partition_point(|&split| split <= start.plain);
    let past_last_split = composite
        .source_plain_splits
        .partition_point(|&split| split < end.plain);
    let mut source_splits = Vec::new();
    source_splits
        .try_reserve_exact(past_last_split.checked_sub(first_split)?)
        .ok()?;
    for &split in &composite.source_plain_splits[first_split..past_last_split] {
        source_splits.push(split.checked_sub(start.plain)?);
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

fn try_combine_frequencies(
    left_literal: &[u32; 286],
    left_distance: &[u32; 30],
    right_literal: &[u32; 286],
    right_distance: &[u32; 30],
) -> Option<([u32; 286], [u32; 30])> {
    let mut literal = *left_literal;
    for (combined, &right) in literal.iter_mut().zip(right_literal) {
        *combined = combined.checked_add(right)?;
    }
    // The joined block has one end-of-block symbol, not two.
    literal[256] = literal[256].checked_sub(1)?;

    let mut distance = *left_distance;
    for (combined, &right) in distance.iter_mut().zip(right_distance) {
        *combined = combined.checked_add(right)?;
    }
    Some((literal, distance))
}

/// Append `right` to `left` in place. Returns `false`, leaving `left`'s
/// contents unchanged, on overflow or allocation failure.
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

    let Some((literal_frequencies, distance_frequencies)) = try_combine_frequencies(
        &left.literal_frequencies,
        &left.distance_frequencies,
        &right.literal_frequencies,
        &right.distance_frequencies,
    ) else {
        return false;
    };

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

/// Fallible `Arc::make_mut` plus reserve: make a shared vector uniquely mutable
/// with room for `additional` more elements. Failure leaves its contents
/// unchanged.
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
    let (literal_frequencies, distance_frequencies) = try_combine_frequencies(
        &left.literal_frequencies,
        &left.distance_frequencies,
        &right.literal_frequencies,
        &right.distance_frequencies,
    )?;
    let tokens = try_concat_shared(&left.tokens, &right.tokens)?;
    let plain = try_concat_shared(&left.plain, &right.plain)?;

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
    source_splits.sort_shared_by(Ord::cmp);
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
        // Mixed source types count as Dynamic; in the boundary DP, which never
        // joins stored and Huffman blocks, that means fixed/dynamic mixes.
        SourceBlockType::Dynamic
    }
}

/// The source bits of `blocks`, rounded up to bytes. Strict length-258
/// normalization clears a block's source bits, so once any block has lost them,
/// `stream_bytes`, the encoded length of their stream, is used instead.
fn encoded_source_bytes(blocks: &[ParsedBlock], stream_bytes: usize) -> u64 {
    let mut bits = 0_u64;
    for block in blocks {
        let Some(original) = block.original else {
            return stream_bytes as u64;
        };
        bits += original.len;
    }
    bits.div_ceil(8)
}

// Out of line: each call sums a whole plan list.
#[inline(never)]
fn total_bits(plans: &[PlannedBlock]) -> u64 {
    plans.iter().map(|plan| plan.bits).sum()
}

#[cfg(test)]
pub(crate) mod test_support;
#[cfg(test)]
mod tests;
