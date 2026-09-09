// SPDX-License-Identifier: MIT

//! Discover coupled block cuts from the spatial support of payload symbols.
//! A symbol or a group may occupy a short interval whose own tables cost less
//! than carrying those symbols in both surrounding regions. New anchors lie
//! between tokens; existing graph anchors may also split proven matches.

use std::collections::HashMap;

use super::super::block::plan_reusable_block_with_header_cache;
use super::super::header::HeaderPlanCache;
use super::*;

const MAX_BLOCK_TOKENS: usize = 32_768;
const MAX_PAIRS: usize = 8;
const MAX_MENU: usize = (286 + 30) * 5;
const MAX_CUTS: usize = 80;
const ESTIMATE_WINDOW: u64 = 64;

pub(crate) struct AlphabetBudget {
    work_left: usize,
    prices_left: usize,
}

impl AlphabetBudget {
    pub(crate) fn new() -> Self {
        Self {
            work_left: 1 << 26,
            prices_left: 4096,
        }
    }

    fn spend(&mut self, work: usize) -> Option<()> {
        self.work_left = self.work_left.checked_sub(work)?;
        Some(())
    }
}

#[derive(Clone, Copy)]
struct Span {
    start: usize,
    end: usize,
}

impl Span {
    fn empty() -> Self {
        Self {
            start: usize::MAX,
            end: 0,
        }
    }
    fn union(self, other: Self) -> Self {
        Self {
            start: self.start.min(other.start),
            end: self.end.max(other.end),
        }
    }
}

fn support_intervals(
    composite: &Composite,
    stop: &mut SearchStop<'_>,
) -> Option<Vec<(usize, usize)>> {
    let mut literal = [Span::empty(); 286];
    let mut distance = [Span::empty(); 30];
    for (i, &token) in composite.tokens.iter().enumerate() {
        if i & 255 == 0 && stop.reached() {
            return None;
        }
        let span = Span {
            start: i,
            end: i + 1,
        };
        match token {
            Token::Literal(value) => {
                let slot = &mut literal[usize::from(value)];
                *slot = slot.union(span);
            }
            Token::Match {
                length_symbol,
                distance_symbol,
                ..
            } => {
                let slot = &mut literal[usize::from(length_symbol)];
                *slot = slot.union(span);
                let slot = &mut distance[usize::from(distance_symbol)];
                *slot = slot.union(span);
            }
        }
    }
    let mut pairs = Vec::new();
    pairs.try_reserve_exact(MAX_MENU).ok()?;
    for alphabet in [&literal[..], &distance[..]] {
        let mut used = [Span::empty(); 286];
        let mut count = 0;
        for &span in alphabet.iter().filter(|span| span.start < span.end) {
            used[count] = span;
            count += 1;
        }
        let mut tail = Span::empty();
        for &span in used[..count].iter().rev() {
            tail = tail.union(span);
            pairs.push((span.start, span.end));
            pairs.push((tail.start, tail.end));
        }
        for width in 2..=4 {
            for group in used[..count].windows(width) {
                let span = group.iter().fold(Span::empty(), |a, &b| a.union(b));
                pairs.push((span.start, span.end));
            }
        }
    }
    pairs.sort_unstable();
    pairs.dedup();
    pairs.retain(|&(start, end)| start != 0 || end != composite.tokens.len());
    Some(pairs)
}

fn cut(composite: &Composite, token: usize) -> Cut {
    Cut {
        token,
        plain: composite.token_plain_offsets[token],
    }
}

/// The estimate ranks candidates; the window is a search heuristic, not a
/// lower bound proving that omitted pairs cannot improve the stream.
fn estimate(composite: &Composite, start: usize, end: usize, strict: bool) -> Option<u64> {
    if start == end {
        return Some(0);
    }
    let frequencies = composite.range_frequencies(start, end)?;
    let fixed = 3
        + frequencies.extra_bits
        + frequencies
            .distance
            .iter()
            .map(|&n| u64::from(n) * 5)
            .sum::<u64>()
        + frequencies
            .literal
            .iter()
            .enumerate()
            .map(|(i, &n)| {
                u64::from(n)
                    * if i <= 143 {
                        8
                    } else if i <= 255 {
                        9
                    } else if i <= 279 {
                        7
                    } else {
                        8
                    }
            })
            .sum::<u64>();
    Some(fixed.min(estimate_histogram_range_bits(
        &frequencies,
        composite.token_plain_offsets[end] - composite.token_plain_offsets[start],
        strict,
    )))
}

struct EdgeCache<'a> {
    edges: HashMap<(usize, usize), PreparedEdge>,
    headers: HeaderPlanCache,
    budget: &'a mut AlphabetBudget,
}

impl EdgeCache<'_> {
    fn get(
        &mut self,
        block: &ParsedBlock,
        composite: &Composite,
        start: Cut,
        end: Cut,
        options: &Options,
        stop: &mut SearchStop<'_>,
    ) -> Option<&PreparedEdge> {
        let key = (start.plain, end.plain);
        if !self.edges.contains_key(&key) {
            if stop.reached() || self.budget.prices_left == 0 {
                return None;
            }
            let work = composite
                .range_token_count_upper_bound(start, end)?
                .checked_mul(std::mem::size_of::<Token>())?
                .checked_add(end.plain.checked_sub(start.plain)?)?;
            self.budget.spend(work)?;
            self.budget.prices_left -= 1;
            self.edges.try_reserve(1).ok()?;
            // This cache owns exactly one source block. Reuse header kernels
            // across its neighboring ranges: different histograms often build
            // the same length lists, whose RLE description can be shared while
            // each range still pays its own exact payload and extra bits.
            let materialized;
            let range = if start.plain == 0 && end.plain == block.plain.len() {
                block
            } else {
                materialized = make_range(composite, start, end)?;
                &materialized
            };
            let reusable =
                plan_reusable_block_with_header_cache(range, options, stop, &mut self.headers);
            let edge = PreparedEdge {
                plain_len: range.plain.len(),
                source_type: range.source_type,
                original: usable_original(range, options.strict),
                reusable,
                shared_dynamic: None,
            };
            self.edges.insert(key, edge);
        }
        self.edges.get(&key)
    }
}

#[allow(clippy::too_many_arguments)]
fn price_pair(
    block: &ParsedBlock,
    composite: &Composite,
    pair: (usize, usize),
    alignment: u8,
    options: &Options,
    cache: &mut EdgeCache,
    stop: &mut SearchStop<'_>,
) -> Option<Vec<PlannedBlock>> {
    let mut points = [0, pair.0, pair.1, composite.tokens.len()];
    points.sort_unstable();
    let mut plans = Vec::new();
    plans.try_reserve_exact(3).ok()?;
    let mut bits = u64::from(alignment);
    for range in points.windows(2).filter(|p| p[0] != p[1]) {
        let (start, end) = (cut(composite, range[0]), cut(composite, range[1]));
        let plan = cache
            .get(block, composite, start, end, options, stop)?
            .plan((bits & 7) as u8);
        bits = bits.checked_add(plan.bits)?;
        plans.push(plan.instantiate(composite, start, end)?);
    }
    Some(plans)
}

#[allow(clippy::too_many_arguments)]
fn refine(
    block: &ParsedBlock,
    composite: &Composite,
    alignment: u8,
    options: &Options,
    cache: &mut EdgeCache,
    stop: &mut SearchStop<'_>,
    best: &mut Option<Vec<PlannedBlock>>,
) -> Option<()> {
    let pairs = support_intervals(composite, stop)?;
    let mut estimates = HashMap::new();
    estimates
        .try_reserve(pairs.len().checked_mul(3)?.checked_add(1)?)
        .ok()?;
    let mut score = |start, end, stop: &mut SearchStop<'_>| -> Option<u64> {
        if stop.reached() {
            return None;
        }
        if let Some(&value) = estimates.get(&(start, end)) {
            return Some(value);
        }
        // Two partial checkpoints and the complete LL/DD frequency arrays.
        cache.budget.spend(2 * RANGE_HISTOGRAM_INTERVAL + 316)?;
        let value = estimate(composite, start, end, options.strict)?;
        estimates.insert((start, end), value);
        Some(value)
    };
    let n = composite.tokens.len();
    let threshold = score(0, n, stop)?.saturating_add(ESTIMATE_WINDOW);
    let mut ranked = Vec::new();
    ranked.try_reserve_exact(pairs.len()).ok()?;
    for (start, end) in pairs {
        let value = score(0, start, stop)?
            .checked_add(score(start, end, stop)?)?
            .checked_add(score(end, n, stop)?)?;
        if value <= threshold {
            ranked.push((value, start, end));
        }
    }
    ranked.sort_unstable();
    ranked.truncate(MAX_PAIRS);
    let parent_bits = reusable_original_bits(block, alignment, options.strict)?.len;
    let mut best_bits = parent_bits;
    for &(_, start, end) in &ranked {
        let plans = price_pair(
            block,
            composite,
            (start, end),
            alignment,
            options,
            cache,
            stop,
        )?;
        let bits = total_bits(&plans);
        if bits < best_bits {
            best_bits = bits;
            *best = Some(plans);
        }
    }
    // A completed direct win pays for the denser coupled graph. The direct
    // winner survives cancellation or exhaustion during the graph expansion.
    if best.is_none() {
        return Some(());
    }
    let mut cuts = choose_cuts(composite, true, true)?;
    if let Some(extra) = entropy_state_boundary_cuts(composite, options.strict, stop) {
        for point in extra {
            push_optional_cut(&mut cuts, composite, point)?;
        }
    }
    if let Some((_, search)) = adaptive_histogram_split_search(
        composite,
        cut(composite, 0),
        cut(composite, n),
        options.strict,
        stop,
    ) {
        add_cut(&mut cuts, composite, search.best.token)?;
        if let Some(point) = search.secondary {
            add_cut(&mut cuts, composite, point.token)?;
        }
    }
    for &(_, start, end) in &ranked {
        add_cut(&mut cuts, composite, start)?;
        add_cut(&mut cuts, composite, end)?;
    }
    cuts.sort_unstable_by_key(|cut| cut.plain);
    if cuts.len() > MAX_CUTS {
        return None;
    }
    let mut graph = BoundaryGraph::new(cuts.len(), alignment)?;
    for start in 0..cuts.len() - 1 {
        for end in start + 1..cuts.len() {
            if stop.reached() {
                return None;
            }
            // This one-source graph deliberately admits middle slices so
            // both sides of an interval can be selected in the same path.
            let edge = cache.get(block, composite, cuts[start], cuts[end], options, stop)?;
            graph.consider(start, end, edge, stop)?;
        }
    }
    let plans = graph.resolve(composite, &cuts)?;
    if total_bits(&plans) < best_bits {
        *best = Some(plans);
    }
    Some(())
}

pub(crate) fn plan_alphabet_boundaries(
    block: &ParsedBlock,
    alignment: u8,
    options: &Options,
    budget: &mut AlphabetBudget,
    stop: &mut SearchStop<'_>,
) -> Option<Vec<PlannedBlock>> {
    if stop.reached()
        || block.source_type == SourceBlockType::Stored
        || !(16..=MAX_BLOCK_TOKENS).contains(&block.tokens.len())
        || budget.prices_left == 0
    {
        return None;
    }
    budget.spend(block.tokens.len())?;
    let composite = Composite::new(std::slice::from_ref(block))?;
    // Use the same bounded table grid in both modes. Geometry and table-search
    // effort are independent: admitting coupled cuts does not start a Max
    // token search at every edge.
    let bounded = Options {
        exhaustive: false,
        ..options.clone()
    };
    let mut cache = EdgeCache {
        edges: HashMap::new(),
        headers: HeaderPlanCache::new(),
        budget,
    };
    let mut best = None;
    let _ = refine(
        block, &composite, alignment, &bounded, &mut cache, stop, &mut best,
    );
    best
}

#[cfg(test)]
mod tests;
