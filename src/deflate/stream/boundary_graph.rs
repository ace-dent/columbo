// SPDX-License-Identifier: MIT
// SPDX-FileCopyrightText: 2026 Andrew C.E. Dent <https://github.com/ace-dent>

//! Choose block layouts by dynamic programming over cut positions.
//!
//! The source-aligned floor chooses which runs of consecutive source blocks to
//! merge, keeping only original boundaries; Huffman prices do not depend on bit
//! alignment, so one price per range keeps that program exact. The global
//! boundary graph adds new cuts, inside matches and from the entropy-state
//! scout among them, and keeps one state per bit residue, so stored and Huffman
//! ranges compete at their real alignment. Each edge is priced once and rebuilt
//! from its template when the shortest path is resolved.

use crate::deflate::block::{
    plan_block_cached, plan_reusable_block, reusable_original_bits, CanonicalPlanCache,
    ReusableBlockPlan,
};
use crate::deflate::header::score_existing_dynamic;
use crate::deflate::model::{
    DynamicPlan, OriginalBits, ParsedBlock, PlannedBlock, Representation, SourceBlockType, Token,
};
use crate::deflate::search::{
    improve_plan_with_deft4j_tree_floor, improve_plan_with_floor,
    improve_plan_with_same_distance_floor, improve_plan_with_short_family_floor,
    replay_extended_floor, replay_table_ladder, same_distance_opportunities,
    try_clone_planned_block,
};
use crate::deflate::stop::SearchStop;
use crate::progress::RouteProgress;
use crate::Options;

use super::entropy_scout::entropy_state_boundary_cuts;
use super::{
    add_cut, add_eighth_cuts, add_exact_eighth_cuts, include_inside_match_cuts, make_range,
    overlapping_source_range, push_cut, push_optional_cut, sort_cuts_by_plain, total_bits,
    try_shared_slice, Composite, Cut, SourceSpan, COLLECTED_RUN_MAX_TOKENS, MAX_BOUNDARY_DP_CUTS,
    MAX_MERGED_PLAIN, MAX_MERGED_TOKENS, MAX_REGROUP_SOURCE_BLOCKS,
};

/// Minimum decoded size for the deadline-independent whole-stream replay;
/// smaller streams reach its result within their normal search slice.
const WHOLE_STREAM_RECODE_MIN_PLAIN: usize = 100_000;
const WHOLE_STREAM_REPLAY_MARGIN_BITS: u64 = 256;

/// Whether to run the global boundary graph before the source-order search once
/// a complete incumbent exists: with several same-distance repartition runs,
/// distant cuts can pay off together.
pub(super) fn established_boundary_graph_priority(blocks: &[ParsedBlock]) -> bool {
    if blocks.is_empty() || blocks.len() > 2 {
        return false;
    }
    let Some(token_count) = blocks.iter().try_fold(0_usize, |total, block| {
        total.checked_add(block.tokens.len())
    }) else {
        return false;
    };
    let Some(plain_count) = blocks
        .iter()
        .try_fold(0_usize, |total, block| total.checked_add(block.plain.len()))
    else {
        return false;
    };
    token_count <= COLLECTED_RUN_MAX_TOKENS
        && plain_count >= WHOLE_STREAM_RECODE_MIN_PLAIN
        && same_distance_opportunities(blocks).repartition_runs >= 2
}

#[allow(clippy::too_many_arguments)]
pub(super) fn plan_global_boundary_graph(
    blocks: &[ParsedBlock],
    start_alignment: u8,
    options: &Options,
    allow_regroup: bool,
    plan_cache: &mut CanonicalPlanCache,
    stop: &mut SearchStop<'_>,
    progress: &RouteProgress,
    detailed_progress: Option<&RouteProgress>,
) -> Option<Vec<PlannedBlock>> {
    progress.phase(
        "Building global boundary graph",
        blocks.len(),
        "source block",
        "source blocks",
    );
    let composite = Composite::new(blocks)?;
    let mut cuts = choose_cuts(&composite, options.exhaustive, allow_regroup)?;
    if options.exhaustive && cuts.len() < MAX_BOUNDARY_DP_CUTS && !stop.reached() {
        progress.activity("Scouting entropy-state boundaries");
        if let Some(scouted) = entropy_state_boundary_cuts(&composite, options.strict, stop) {
            for cut in scouted {
                if push_optional_cut(&mut cuts, &composite, cut).is_none() {
                    break;
                }
            }
            sort_cuts_by_plain(&mut cuts);
        }
    }
    if cuts.len() <= 2 {
        return None;
    }

    progress.phase(
        "Pricing block boundaries",
        cuts.len().saturating_sub(1),
        "cut anchor",
        "cut anchors",
    );
    let candidate = boundary_dp(
        blocks,
        &composite,
        &cuts,
        start_alignment,
        options,
        allow_regroup,
        plan_cache,
        stop,
        detailed_progress,
    )?;
    progress.advance(cuts.len().saturating_sub(1));
    progress.checkpoint("Boundary result", total_bits(&candidate), candidate.len());
    Some(candidate)
}

/// Find the cheapest grouping of Huffman source blocks at their original
/// boundaries by pricing every contiguous range (36 for eight blocks), before
/// deep search can use up the deadline.
pub(super) fn source_aligned_huffman_floor(
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

pub(super) fn source_aligned_huffman_floor_with_limit(
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
    // Deterministic and deadline-independent; Max keeps the result as an upper
    // bound for deeper searches.
    structural_options.exhaustive = false;

    struct Node {
        bits: u64,
        previous: usize,
        plan: SourceAlignedPlan,
    }

    /// A template rebuilds its payload from the composite; a recode has its own
    /// spelling, so it keeps its complete plan.
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

            // Huffman blocks have no alignment padding, so one price (including
            // an exact source block's original bits) is valid at every bit
            // offset.
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

            // A singleton range can be part of the best segmentation, so carry
            // its same-distance spelling in this DP, not only in the
            // all-singleton floor.
            if let Some(recode) = singleton_same_distance {
                if !matches!(recode.representation, Representation::Stored)
                    && recode.bits < candidate_bits
                {
                    candidate_bits = recode.bits;
                    candidate_is_stored = false;
                    candidate_plan = SourceAlignedPlan::Recode(recode);
                }
            }

            // Same-distance runs can cross an erased source boundary, and
            // deft4j rebuilds a merged range before pruning matches, so price
            // those candidates and the short-length bands on the merged range.
            // Each helper enforces its own model limits.
            if let Some((range, base)) = recode_base {
                let is_large_whole_stream = start_index == 0
                    && end_index == blocks.len()
                    && range.plain.len() >= WHOLE_STREAM_RECODE_MIN_PLAIN;
                // The whole-stream floor already starts with same-distance
                // normalization.
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
                // A container shares one deadline across its streams, so a
                // large whole-stream range gets one deadline-independent
                // extended floor pass; an earlier frame then cannot hide its
                // best whole-stream spelling.
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
                    // A changed tree can expose another spelling win, so replay
                    // every near-tied whole-stream seed: a costlier table can
                    // be the intermediate state the ladder needs. The margin
                    // keeps weak seeds from using later frames' shared time.
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
                // Stored output's price depends on the prefix alignment; only
                // the eight-state boundary DP handles that.
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

/// Choose the cut anchors for the boundary graph.
pub(super) fn choose_cuts(
    composite: &Composite,
    exhaustive: bool,
    allow_regroup: bool,
) -> Option<Vec<Cut>> {
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
                false,
            )?;
        }
        if exhaustive && token_count <= 512 {
            for token in (source.token_start + 32..source.token_end.saturating_sub(32)).step_by(32)
            {
                add_cut(&mut cuts, composite, token)?;
            }
        }
    }

    // Eighth cuts over each joined Huffman run let a boundary move away from
    // every original encoder flush point.
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
                false,
            )?;

            // Pair and group cuts help when the winning stream keeps some
            // source boundaries but regroups their neighbors.
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
                        false,
                    )?;
                }
            }
        }
    }

    // Collect every token-boundary anchor first, so inside-match cuts cannot
    // crowd them out of the cut budget.
    sort_cuts_by_plain(&mut cuts);
    cuts.dedup_by_key(|cut| cut.plain);

    // Inside-match cuts enlarge the boundary graph, so Default adds them only
    // for compact composites.
    let mut inside_cuts = Vec::new();
    if include_inside_match_cuts(exhaustive, composite.tokens.len()) {
        for source in &composite.sources {
            let token_count = source.token_end - source.token_start;
            let plain_count = source.plain_end - source.plain_start;
            if token_count < 16 || plain_count < 128 || (!exhaustive && plain_count < 32_768) {
                continue;
            }
            add_exact_eighth_cuts(
                &mut inside_cuts,
                composite,
                source.token_start,
                source.token_end,
                source.plain_start,
                source.plain_end,
            )?;
        }
        if allow_regroup && composite.sources.len() <= MAX_REGROUP_SOURCE_BLOCKS {
            for (run_start, run_end) in huffman_runs(&composite.sources)? {
                let first = composite.sources[run_start];
                let last = composite.sources[run_end - 1];
                add_exact_eighth_cuts(
                    &mut inside_cuts,
                    composite,
                    first.token_start,
                    last.token_end,
                    first.plain_start,
                    last.plain_end,
                )?;
                for start in run_start..run_end {
                    for end in start + 2..=run_end {
                        let first = composite.sources[start];
                        let last = composite.sources[end - 1];
                        add_exact_eighth_cuts(
                            &mut inside_cuts,
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
    }
    sort_cuts_by_plain(&mut inside_cuts);
    inside_cuts.dedup_by_key(|cut| cut.plain);
    for cut in inside_cuts {
        if cuts.len() >= MAX_BOUNDARY_DP_CUTS {
            break;
        }
        push_cut(&mut cuts, composite, cut)?;
    }
    sort_cuts_by_plain(&mut cuts);
    Some(cuts)
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

/// A block's representation and bits without its payload; `instantiate`
/// rebuilds the block from a composite range.
#[derive(Clone)]
pub(super) struct PlanTemplate {
    representation: Representation,
    pub(super) bits: u64,
    source_type: SourceBlockType,
}

impl PlanTemplate {
    fn try_from_planned(plan: &PlannedBlock) -> Option<Self> {
        Some(Self {
            representation: plan.representation.try_clone()?,
            bits: plan.bits,
            source_type: plan.source_type,
        })
    }

    pub(super) fn instantiate(
        &self,
        composite: &Composite,
        start: Cut,
        end: Cut,
    ) -> Option<PlannedBlock> {
        Some(PlannedBlock {
            tokens: composite.materialize_range_tokens(start, end)?,
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
pub(super) fn boundary_dp(
    blocks: &[ParsedBlock],
    composite: &Composite,
    cuts: &[Cut],
    start_alignment: u8,
    options: &Options,
    allow_regroup: bool,
    plan_cache: &mut CanonicalPlanCache,
    stop: &mut SearchStop<'_>,
    progress: Option<&RouteProgress>,
) -> Option<Vec<PlannedBlock>> {
    if cuts.len() > MAX_BOUNDARY_DP_CUTS {
        return None;
    }
    let mut graph = BoundaryGraph::new(cuts.len(), start_alignment)?;

    for start_index in 0..cuts.len() - 1 {
        if let Some(progress) = progress {
            progress.advance(start_index);
        }
        if stop.reached() {
            return None;
        }
        if graph.states[start_index].iter().all(Option::is_none) {
            continue;
        }

        for end_index in start_index + 1..cuts.len() {
            let start = cuts[start_index];
            let end = cuts[end_index];
            if !edge_allowed(composite, start, end, options.exhaustive, allow_regroup) {
                continue;
            }
            if stop.reached() {
                return None;
            }
            let Some(edge) = prepare_edge(
                blocks, composite, start, end, options, plan_cache, &mut *stop,
            ) else {
                continue;
            };

            graph.consider(start_index, end_index, &edge, stop)?;
        }
    }

    graph.resolve(composite, cuts)
}

/// Shortest-path search over cut anchors, with one state per bit alignment.
pub(super) struct BoundaryGraph {
    states: Vec<[Option<DpNode>; 8]>,
    start_alignment: u8,
}

impl BoundaryGraph {
    pub(super) fn new(cut_count: usize, start_alignment: u8) -> Option<Self> {
        if !(2..=MAX_BOUNDARY_DP_CUTS).contains(&cut_count) || start_alignment >= 8 {
            return None;
        }
        let mut states = Vec::new();
        states.try_reserve_exact(cut_count).ok()?;
        for _ in 0..cut_count {
            states.push(std::array::from_fn(|_| None));
        }
        states[0][usize::from(start_alignment)] = Some(DpNode {
            bits: 0,
            previous: None,
        });
        Some(Self {
            states,
            start_alignment,
        })
    }

    pub(super) fn consider(
        &mut self,
        start: usize,
        end: usize,
        edge: &PreparedEdge,
        stop: &mut SearchStop<'_>,
    ) -> Option<()> {
        let mut visited = false;
        for alignment in 0..8 {
            let Some(prefix_bits) = self.states[start][alignment].as_ref().map(|node| node.bits)
            else {
                continue;
            };
            // A finished edge may update its first reachable state even if
            // pricing used up the last of the allowance.
            if visited && stop.reached() {
                return None;
            }
            visited = true;
            let template = edge.plan(alignment as u8);
            let Some(bits) = prefix_bits.checked_add(template.bits) else {
                continue;
            };
            let next = ((u64::from(self.start_alignment) + bits) & 7) as usize;
            let destination = &mut self.states[end][next];
            if destination.as_ref().map_or(true, |old| bits < old.bits) {
                *destination = Some(DpNode {
                    bits,
                    previous: Some(Previous {
                        cut: start,
                        alignment: alignment as u8,
                        plan: template,
                    }),
                });
            }
        }
        Some(())
    }

    pub(super) fn resolve(&self, composite: &Composite, cuts: &[Cut]) -> Option<Vec<PlannedBlock>> {
        let end_index = cuts.len() - 1;
        let (mut alignment, _) = self.states[end_index]
            .iter()
            .enumerate()
            .filter_map(|(alignment, node)| node.as_ref().map(|node| (alignment as u8, node.bits)))
            .min_by_key(|&(_, bits)| bits)?;

        let mut at = end_index;
        let mut plans = Vec::new();
        plans.try_reserve_exact(cuts.len().saturating_sub(1)).ok()?;
        while at != 0 {
            let previous = self.states[at][usize::from(alignment)]
                .as_ref()?
                .previous
                .as_ref()?;
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
}

fn edge_allowed(
    composite: &Composite,
    start: Cut,
    end: Cut,
    exhaustive: bool,
    allow_regroup: bool,
) -> bool {
    if !composite.cut_is_valid(start) || !composite.cut_is_valid(end) || start.plain >= end.plain {
        return false;
    }
    let source_range = overlapping_source_range(composite, start, end);
    let sources = &composite.sources[source_range];
    let Some((first, rest)) = sources.split_first() else {
        return false;
    };
    if rest.is_empty() {
        // Default allows one new boundary per source block: a prefix or suffix.
        return exhaustive || start.plain == first.plain_start || end.plain == first.plain_end;
    }

    let Some(token_count) = composite.range_token_count_upper_bound(start, end) else {
        return false;
    };
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
    let whole_sources = start.plain == first.plain_start
        && end.plain == sources.last().expect("the range is nonempty").plain_end;

    if all_stored {
        return whole_sources && plain_count <= 65_535;
    }
    if !all_huffman {
        return false;
    }

    // Cross-source partial ranges are the regroup search. Default builds the
    // graph only with regroup on short source lists, and keeps one end of each
    // edge on a source boundary.
    exhaustive
        || (allow_regroup
            && composite.sources.len() <= MAX_REGROUP_SOURCE_BLOCKS
            && sources
                .iter()
                .any(|source| start.plain == source.plain_start || end.plain == source.plain_end))
}

/// One boundary-DP edge, priced once. Only stored padding and reuse of a
/// stored source block vary with alignment.
pub(super) struct PreparedEdge {
    pub(super) plain_len: usize,
    pub(super) source_type: SourceBlockType,
    pub(super) original: Option<OriginalBits>,
    pub(super) reusable: ReusableBlockPlan,
    pub(super) shared_dynamic: Option<DynamicPlan>,
}

impl PreparedEdge {
    pub(super) fn plan(&self, alignment: u8) -> PlanTemplate {
        let original = self.original.filter(|original| {
            original.block_type != SourceBlockType::Stored || original.alignment == alignment
        });

        // Prefer a strictly cheaper shared tree before copying the base table.
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
        // A failed base-table clone can fall back to a costlier plan; give the
        // shared table one more chance against it.
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
fn prepare_edge(
    blocks: &[ParsedBlock],
    composite: &Composite,
    start: Cut,
    end: Cut,
    options: &Options,
    plan_cache: &mut CanonicalPlanCache,
    stop: &mut SearchStop<'_>,
) -> Option<PreparedEdge> {
    // Edges split only a match that an edge end cuts. Match-to-literal searches
    // stay in the sequential and replay passes, so memory does not grow with
    // cuts × alignments.
    if let Some(source_index) = exact_source(composite, start, end) {
        let block = &blocks[source_index];
        let reusable = plan_cache
            .lookup_reusable(block, options)
            .unwrap_or_else(|| plan_reusable_block(block, options, stop));
        return Some(PreparedEdge {
            plain_len: block.plain.len(),
            source_type: block.source_type,
            original: usable_original(block, options.strict),
            reusable,
            shared_dynamic: None,
        });
    }

    let range = make_range(composite, start, end)?;
    // Price the ordinary fixed/dynamic plans before the optional shared-table
    // probe, so they keep deadline priority.
    let reusable = plan_cache
        .lookup_reusable(&range, options)
        .unwrap_or_else(|| plan_reusable_block(&range, options, &mut *stop));
    let source_range = overlapping_source_range(composite, start, end);
    let source_spans = &composite.sources[source_range.clone()];
    let whole_sources = source_spans.first().is_some_and(|first| {
        start.plain == first.plain_start
            && end.plain == source_spans.last().expect("first was present").plain_end
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

/// Return the block's original bits when reusable at their own alignment under
/// the strict policy; [`PreparedEdge::plan`] checks stored originals'
/// alignment.
pub(super) fn usable_original(block: &ParsedBlock, strict: bool) -> Option<OriginalBits> {
    let original = block.original?;
    reusable_original_bits(block, original.alignment, strict)
}

pub(super) fn exact_source(composite: &Composite, start: Cut, end: Cut) -> Option<usize> {
    let range = overlapping_source_range(composite, start, end);
    if range.len() != 1 {
        return None;
    }
    let index = range.start;
    let source = composite.sources[index];
    (composite.cut_is_token_boundary(start)
        && composite.cut_is_token_boundary(end)
        && source.plain_start == start.plain
        && source.plain_end == end.plain)
        .then_some(index)
}

/// When every block transmits the same dynamic trees, price `tokens` with the
/// first block's header.
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

#[cfg(test)]
mod tests;
