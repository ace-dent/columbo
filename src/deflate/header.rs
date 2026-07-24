// SPDX-License-Identifier: MIT

//! Dynamic-header construction and exact bit accounting.

use super::huffman::{
    make_lengths, make_lengths_columbo_defluff_limited, make_lengths_deflopt_heap,
    make_lengths_defluff_exact, make_lengths_deft4j_java_heap, make_lengths_order_heap, Huffman,
};
use super::model::{token_extra_bits, DynamicPlan, RleToken, Token, CODE_LENGTH_ORDER};

const INF: u64 = u64::MAX / 4;

/// Which deft4j header spelling governs a source-state decision.
///
/// `Complete` is the full `addOptimisedRecoded` option grid. The deliberately
/// narrower `DefaultRecode` spelling is used only while deciding whether
/// deft4j's repeated individual-prune step reached a smaller fixed point.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Deft4jHeaderPolicy {
    Complete,
    DefaultRecode,
}

/// Switches used by deft4j's code-length run packer.
///
/// Keeping these as named fields makes the deft4j option grid readable
/// without changing its insertion order or adding any dynamic dispatch.
#[derive(Debug, Clone, Copy)]
struct Deft4jPackOptions {
    special_repeat: bool,
    use_eight: bool,
    use_seven: bool,
    no_repeat: bool,
    no_zero_repeat: bool,
    no_long_zero_repeat: bool,
    no_repeat_zeros: bool,
}

/// Header-level choices layered on top of a code-length packing strategy.
#[derive(Debug, Clone, Copy)]
struct Deft4jHeaderOptions {
    pack: Deft4jPackOptions,
    prune: bool,
    optimize_header: bool,
}

const DEFT4J_DEFAULT_RECODE_OPTIONS: Deft4jHeaderOptions = Deft4jHeaderOptions {
    pack: Deft4jPackOptions {
        special_repeat: true,
        use_eight: true,
        use_seven: true,
        no_repeat: false,
        no_zero_repeat: false,
        no_long_zero_repeat: false,
        no_repeat_zeros: false,
    },
    prune: false,
    optimize_header: false,
};

pub(crate) fn token_bits(
    tokens: &[Token],
    literal_lengths: &[u8],
    distance_lengths: &[u8],
) -> Option<u64> {
    let mut bits = token_extra_bits(tokens);
    for token in tokens {
        match *token {
            Token::Literal(value) => {
                bits = bits.checked_add(u64::from(*literal_lengths.get(usize::from(value))?))?;
                if literal_lengths[usize::from(value)] == 0 {
                    return None;
                }
            }
            Token::Match {
                length_symbol,
                distance_symbol,
                ..
            } => {
                let literal = *literal_lengths.get(usize::from(length_symbol))?;
                let distance = *distance_lengths.get(usize::from(distance_symbol))?;
                if literal == 0 || distance == 0 {
                    return None;
                }
                bits = bits.checked_add(u64::from(literal) + u64::from(distance))?;
            }
        }
    }
    let end = *literal_lengths.get(256)?;
    if end == 0 {
        return None;
    }
    bits.checked_add(u64::from(end))
}

pub(crate) fn token_bits_from_frequencies(
    literal_frequencies: &[u32; 286],
    distance_frequencies: &[u32; 30],
    literal_lengths: &[u8],
    distance_lengths: &[u8],
    extra_bits: u64,
) -> Option<u64> {
    let mut bits = extra_bits;
    for (symbol, &frequency) in literal_frequencies.iter().enumerate() {
        if frequency == 0 {
            continue;
        }
        let length = *literal_lengths.get(symbol)?;
        if length == 0 {
            return None;
        }
        bits = bits.checked_add(u64::from(frequency) * u64::from(length))?;
    }
    for (symbol, &frequency) in distance_frequencies.iter().enumerate() {
        if frequency == 0 {
            continue;
        }
        let length = *distance_lengths.get(symbol)?;
        if length == 0 {
            return None;
        }
        bits = bits.checked_add(u64::from(frequency) * u64::from(length))?;
    }
    Some(bits)
}

pub(crate) fn score_existing_dynamic(
    tokens: &[Token],
    source: &DynamicPlan,
    min_distance_codes: bool,
) -> Option<DynamicPlan> {
    if min_distance_codes
        && source
            .distance_lengths
            .iter()
            .filter(|&&length| length != 0)
            .count()
            < 2
    {
        return None;
    }
    if source.hlit < 257
        || source.hlit > 286
        || source.hdist == 0
        || source.hdist > 30
        || source.hclen < 4
        || source.hclen > 19
        || source.literal_lengths.len() != source.hlit
        || source.distance_lengths.len() != source.hdist
        || Huffman::build(&source.literal_lengths).is_none()
        || Huffman::build(&source.distance_lengths).is_none()
        || Huffman::build(&source.code_length_lengths).is_none()
    {
        return None;
    }
    let mut plan = source.clone();
    let data_bits = token_bits(tokens, &plan.literal_lengths, &plan.distance_lengths)?;
    plan.bits = dynamic_bits(data_bits, &plan)?;
    Some(plan)
}

pub(crate) fn best_dynamic_plan(
    tokens: &[Token],
    literal_frequencies: &[u32; 286],
    distance_frequencies: &[u32; 30],
    original: Option<&DynamicPlan>,
    min_distance_codes: bool,
    exhaustive: bool,
    mut expired: impl FnMut() -> bool,
) -> Option<DynamicPlan> {
    let extra_bits = token_extra_bits(tokens);
    let source_exact =
        original.and_then(|plan| score_existing_dynamic(tokens, plan, min_distance_codes));
    let source_is_compatible = source_exact.is_some();
    let mut best = source_exact;

    // Preserve the source trees as a data-code candidate, but repack their
    // header. This is distinct from `score_existing_dynamic`, which retains
    // the source RLE byte-for-byte; DeflOpt explicitly tries both forms.
    if let Some(source) = original {
        if source_is_compatible {
            let data_bits = token_bits_from_frequencies(
                literal_frequencies,
                distance_frequencies,
                &source.literal_lengths,
                &source.distance_lengths,
                extra_bits,
            );
            if let Some(data_bits) = data_bits {
                if let Some(repacked) = plan_for_explicit_lengths_with_cost(
                    &source.literal_lengths,
                    &source.distance_lengths,
                    data_bits,
                    exhaustive,
                ) {
                    keep_better(&mut best, repacked);
                }
            }
        }
    }

    let mut literal_candidates = tree_candidates(literal_frequencies, 15, exhaustive);
    let mut build_distance_frequencies = *distance_frequencies;
    ensure_distance_symbols(&mut build_distance_frequencies, min_distance_codes);
    let mut distance_candidates = tree_candidates(&build_distance_frequencies, 15, exhaustive);

    literal_candidates.retain(|lengths| lengths.get(256).copied().unwrap_or(0) != 0);
    distance_candidates.retain(|lengths| {
        distance_frequencies
            .iter()
            .enumerate()
            .all(|(symbol, &frequency)| frequency == 0 || lengths[symbol] != 0)
    });
    // The original Columbo C planner preserves family/variant insertion order
    // and keeps up to twenty unique trees per alphabet. Payload sorting is
    // incorrect here: a slightly dearer tree can encode a much smaller header.
    literal_candidates.truncate(20);
    distance_candidates.truncate(20);

    'outer: for literal in &literal_candidates {
        for distance in &distance_candidates {
            if expired() {
                break 'outer;
            }
            let data_bits = token_bits_from_frequencies(
                literal_frequencies,
                distance_frequencies,
                literal,
                distance,
                extra_bits,
            );
            if let Some(data_bits) = data_bits {
                if let Some(candidate) =
                    plan_for_explicit_lengths_with_cost(literal, distance, data_bits, exhaustive)
                {
                    keep_better(&mut best, candidate);
                }
            }
        }
    }

    // Symbols with the same frequency may exchange code lengths without
    // changing the payload cost or invalidating either Huffman tree. Their
    // positions do affect the run-length encoded dynamic header, however.
    // Columbo's --max route tries these stable assignments before its more
    // expensive finished-tree searches. DeflOpt 2.07 does not permute a
    // finished tree, so this remains explicitly a Columbo extension.
    if exhaustive && !expired() {
        if let Some(seed) = best.clone() {
            let seed_literal = pad_lengths::<286>(&seed.literal_lengths);
            let seed_distance = pad_lengths::<30>(&seed.distance_lengths);
            for descending in [false, true] {
                // The finished-tree route rearranges literal symbols alone.
                // Keep the distance assignment fixed as an independent
                // candidate; arranging both alphabets can hide a literal-side
                // header win even though neither change affects payload cost.
                let mut literal = seed_literal;
                arrange_equal_frequency_lengths(literal_frequencies, &mut literal, descending);
                if literal != seed_literal && !expired() {
                    if let Some(candidate) =
                        plan_for_explicit_lengths(tokens, &literal, &seed_distance, exhaustive)
                    {
                        keep_better(&mut best, candidate);
                    }
                }

                // The lightweight max route also arranges both alphabets.
                let mut distance = seed_distance;
                arrange_equal_frequency_lengths(distance_frequencies, &mut distance, descending);
                if (literal != seed_literal || distance != seed_distance) && !expired() {
                    if let Some(candidate) =
                        plan_for_explicit_lengths(tokens, &literal, &distance, exhaustive)
                    {
                        keep_better(&mut best, candidate);
                    }
                }
            }
        }
    }

    // Columbo's greedy swap route explores the same finished-tree degree of
    // freedom more locally. It is particularly useful for small, literal-heavy
    // blocks, where rearranging the tree can save a whole byte without changing
    // a single decoded token. This route is not part of DeflOpt 2.07.
    if exhaustive && tokens.len() <= 700 && !expired() {
        if let Some(seed) = best.clone() {
            improve_by_length_swaps(
                tokens,
                literal_frequencies,
                distance_frequencies,
                &seed,
                exhaustive,
                &mut expired,
                &mut best,
            );
        }
    }
    best
}

fn pad_lengths<const N: usize>(lengths: &[u8]) -> [u8; N] {
    let mut padded = [0_u8; N];
    let count = lengths.len().min(N);
    padded[..count].copy_from_slice(&lengths[..count]);
    padded
}

/// Reassign one tree's lengths within equal-frequency symbol groups.
///
/// Sorting only the lengths (and retaining the symbol positions) preserves
/// both the Kraft sum and the exact frequency-weighted payload cost. The
/// stable symbol order makes this a cheap pair of useful header candidates
/// instead of a factorial permutation search.
fn arrange_equal_frequency_lengths<const N: usize>(
    frequencies: &[u32; N],
    lengths: &mut [u8; N],
    descending: bool,
) {
    let mut visited = [false; N];
    for first in 0..N {
        if visited[first] || frequencies[first] == 0 || lengths[first] == 0 {
            continue;
        }

        let frequency = frequencies[first];
        let mut positions = Vec::new();
        let mut assigned_lengths = Vec::new();
        for symbol in first..N {
            if !visited[symbol] && frequencies[symbol] == frequency && lengths[symbol] != 0 {
                visited[symbol] = true;
                positions.push(symbol);
                assigned_lengths.push(lengths[symbol]);
            }
        }
        if positions.len() < 2 {
            continue;
        }

        assigned_lengths.sort_unstable();
        if descending {
            assigned_lengths.reverse();
        }
        for (symbol, length) in positions.into_iter().zip(assigned_lengths) {
            lengths[symbol] = length;
        }
    }
}

fn ensure_distance_symbols(frequencies: &mut [u32; 30], min_distance_codes: bool) {
    if !min_distance_codes {
        return;
    }
    let mut used = frequencies
        .iter()
        .enumerate()
        .filter_map(|(symbol, &frequency)| (frequency != 0).then_some(symbol));
    match (used.next(), used.next()) {
        (None, _) => {
            frequencies[0] = 1;
            frequencies[1] = 1;
        }
        (Some(only), None) => frequencies[usize::from(only == 0)] = 1,
        (Some(_), Some(_)) => {}
    }
}

fn tree_candidates(frequencies: &[u32], max_bits: u8, exhaustive: bool) -> Vec<Vec<u8>> {
    let mut candidates = Vec::new();
    // Family order is observable because equal complete plans retain the
    // earlier candidate. The original Columbo C selector combines the mapped
    // DeflOpt heap with Columbo's legacy order-key heap. Broader Columbo and
    // exact Defluff families belong to max or terminal feedback routes.
    // Keeping that separation caps the ordinary cross product at 64 pairs.
    for variant in 0..4 {
        push_unique(
            &mut candidates,
            make_lengths_deflopt_heap(frequencies, max_bits, variant),
        );
    }
    for variant in 0..4 {
        push_unique(
            &mut candidates,
            make_lengths_order_heap(frequencies, max_bits, variant),
        );
    }

    if exhaustive {
        for variant in 0..4 {
            push_unique(
                &mut candidates,
                make_lengths(frequencies, max_bits, variant),
            );
        }
        push_unique(
            &mut candidates,
            make_lengths_columbo_defluff_limited(frequencies, max_bits, 0),
        );
        push_unique(
            &mut candidates,
            make_lengths_defluff_exact(frequencies, max_bits, 0),
        );
        for variant in 0..4 {
            if variant == 0 {
                push_unique(
                    &mut candidates,
                    make_lengths_deft4j_java_heap(frequencies, max_bits),
                );
            }
        }
    }
    candidates.retain(|lengths| {
        lengths.len() == frequencies.len()
            && frequencies
                .iter()
                .zip(lengths)
                .all(|(&frequency, &length)| frequency == 0 || length != 0)
            && Huffman::build(lengths).is_some()
    });
    candidates
}

fn push_unique(candidates: &mut Vec<Vec<u8>>, candidate: Vec<u8>) {
    if !candidates.iter().any(|current| current == &candidate) {
        candidates.push(candidate);
    }
}

fn trim_literal(lengths: &[u8]) -> usize {
    lengths
        .iter()
        .enumerate()
        .skip(257)
        .rfind(|(_, length)| **length != 0)
        .map_or(257, |(index, _)| index + 1)
}

fn trim_distance(lengths: &[u8]) -> usize {
    lengths
        .iter()
        .enumerate()
        .skip(1)
        .rfind(|(_, length)| **length != 0)
        .map_or(1, |(index, _)| index + 1)
}

/// Score an explicitly assigned literal/length and distance tree.
///
/// Keeping this separate from tree construction lets structural searches
/// rearrange a valid length histogram solely to make its dynamic header less
/// expensive. The represented token stream is never changed here.
pub(crate) fn plan_for_explicit_lengths(
    tokens: &[Token],
    literal_lengths: &[u8],
    distance_lengths: &[u8],
    exhaustive: bool,
) -> Option<DynamicPlan> {
    plan_for_explicit_lengths_masked(tokens, literal_lengths, distance_lengths, exhaustive, 0xff)
}

fn plan_for_explicit_lengths_masked(
    tokens: &[Token],
    literal_lengths: &[u8],
    distance_lengths: &[u8],
    exhaustive: bool,
    rle_mask: u8,
) -> Option<DynamicPlan> {
    let hlit = trim_literal(literal_lengths);
    let hdist = trim_distance(distance_lengths);
    let literal_lengths = literal_lengths[..hlit].to_vec();
    let distance_lengths = distance_lengths[..hdist].to_vec();
    let data_bits = token_bits(tokens, &literal_lengths, &distance_lengths)?;

    plan_for_trimmed_lengths(
        literal_lengths,
        distance_lengths,
        data_bits,
        exhaustive,
        rle_mask,
    )
}

pub(crate) fn plan_for_explicit_lengths_with_cost(
    literal_lengths: &[u8],
    distance_lengths: &[u8],
    data_bits: u64,
    exhaustive: bool,
) -> Option<DynamicPlan> {
    let hlit = trim_literal(literal_lengths);
    let hdist = trim_distance(distance_lengths);
    plan_for_trimmed_lengths(
        literal_lengths[..hlit].to_vec(),
        distance_lengths[..hdist].to_vec(),
        data_bits,
        exhaustive,
        0xff,
    )
}

/// Score a dynamic header with deft4j beta 17's ordered header grid.
///
/// Columbo's ordinary planner intentionally considers a wider family of
/// headers. The deft4j-derived route cannot use that wider score to guide its
/// state graph without changing which intermediate states the source ordering
/// retains. This helper therefore keeps the data trees fixed and reproduces
/// deft4j's option grid and insertion order after Columbo trims HLIT and HDIST.
/// beta 17 can instead retain the source header's advertised spans for its
/// source-dynamic base.
pub(crate) fn plan_for_deft4j_lengths_with_cost(
    literal_frequencies: &[u32; 286],
    distance_frequencies: &[u32; 30],
    extra_bits: u64,
    literal_lengths: &[u8; 286],
    distance_lengths: &[u8; 30],
    min_distance_codes: bool,
    policy: Deft4jHeaderPolicy,
) -> Option<DynamicPlan> {
    let mut distance_lengths = *distance_lengths;
    apply_min_distance_codes(
        &mut distance_lengths,
        distance_frequencies,
        min_distance_codes,
    );

    let data_bits = token_bits_from_frequencies(
        literal_frequencies,
        distance_frequencies,
        literal_lengths,
        &distance_lengths,
        extra_bits,
    )?;
    let hlit = trim_literal(literal_lengths);
    let hdist = trim_distance(&distance_lengths);
    let literal = try_vec_from_slice(&literal_lengths[..hlit])?;
    let distance = try_vec_from_slice(&distance_lengths[..hdist])?;
    if Huffman::build(&literal).is_none() || Huffman::build(&distance).is_none() {
        return None;
    }

    let mut combined = Vec::new();
    combined
        .try_reserve_exact(literal.len().checked_add(distance.len())?)
        .ok()?;
    combined.extend_from_slice(&literal);
    combined.extend_from_slice(&distance);

    if policy == Deft4jHeaderPolicy::DefaultRecode {
        return build_deft4j_header(
            data_bits,
            &literal,
            &distance,
            &combined,
            DEFT4J_DEFAULT_RECODE_OPTIONS,
        );
    }

    let mut best = None;
    // Keep deft4j's loop order. Equal-sized headers deliberately retain the
    // first spelling because it becomes the block object used by later
    // transformations and merges.
    for no_repeat_zeros in [false, true] {
        for prune in [false, true] {
            for no_repeat in [false, true] {
                if no_repeat_zeros && no_repeat {
                    continue;
                }
                for no_zero_repeat in [false, true] {
                    if no_repeat_zeros && !no_zero_repeat {
                        continue;
                    }
                    for no_long_zero_repeat in [false, true] {
                        for special_repeat in [true, false] {
                            if special_repeat {
                                if no_repeat {
                                    continue;
                                }
                                for use_eight in [true, false] {
                                    for use_seven in [true, false] {
                                        if !use_eight && !use_seven {
                                            continue;
                                        }
                                        if let Some(candidate) = build_deft4j_header(
                                            data_bits,
                                            &literal,
                                            &distance,
                                            &combined,
                                            Deft4jHeaderOptions {
                                                pack: Deft4jPackOptions {
                                                    special_repeat: true,
                                                    use_eight,
                                                    use_seven,
                                                    no_repeat,
                                                    no_zero_repeat,
                                                    no_long_zero_repeat,
                                                    no_repeat_zeros,
                                                },
                                                prune,
                                                optimize_header: true,
                                            },
                                        ) {
                                            keep_better(&mut best, candidate);
                                        }
                                    }
                                }
                            } else if let Some(candidate) = build_deft4j_header(
                                data_bits,
                                &literal,
                                &distance,
                                &combined,
                                Deft4jHeaderOptions {
                                    pack: Deft4jPackOptions {
                                        special_repeat: false,
                                        use_eight: false,
                                        use_seven: false,
                                        no_repeat,
                                        no_zero_repeat,
                                        no_long_zero_repeat,
                                        no_repeat_zeros,
                                    },
                                    prune,
                                    optimize_header: true,
                                },
                            ) {
                                keep_better(&mut best, candidate);
                            }
                        }
                    }
                }
            }
        }
    }
    best
}

fn apply_min_distance_codes(lengths: &mut [u8; 30], frequencies: &[u32; 30], enabled: bool) {
    if !enabled || lengths.iter().filter(|&&length| length != 0).count() >= 2 {
        return;
    }
    let used = frequencies.iter().position(|&frequency| frequency != 0);
    match used {
        Some(symbol) => {
            lengths[symbol] = 1;
            lengths[usize::from(symbol == 0)] = 1;
        }
        None => {
            lengths[0] = 1;
            lengths[1] = 1;
        }
    }
}

fn build_deft4j_header(
    data_bits: u64,
    literal_lengths: &[u8],
    distance_lengths: &[u8],
    combined: &[u8],
    options: Deft4jHeaderOptions,
) -> Option<DynamicPlan> {
    let mut rle = deft4j_pack_code_lengths(combined, options.pack)?;
    let mut code_length_lengths = deft4j_code_length_tree(&rle)?;

    if options.prune {
        if let Some(pruned) = rewrite_rle_deft4j_literals(&rle, &code_length_lengths, true) {
            rle = pruned;
            code_length_lengths = deft4j_code_length_tree(&rle)?;
        }
    }

    let mut plan = deft4j_dynamic_plan(
        data_bits,
        literal_lengths,
        distance_lengths,
        rle,
        code_length_lengths,
    )?;
    if options.optimize_header {
        if let Some(optimized) =
            rewrite_rle_deft4j_literals(&plan.rle, &plan.code_length_lengths, false)
        {
            if let Some(candidate) = deft4j_dynamic_plan(
                data_bits,
                literal_lengths,
                distance_lengths,
                optimized,
                plan.code_length_lengths,
            ) {
                if candidate.bits < plan.bits {
                    plan = candidate;
                }
            }
        }
    }
    Some(plan)
}

fn deft4j_code_length_tree(rle: &[RleToken]) -> Option<[u8; 19]> {
    let frequencies = rle_frequencies(rle);
    let lengths = make_lengths_deft4j_java_heap(&frequencies, 7);
    if lengths.len() != 19 || Huffman::build(&lengths).is_none() {
        return None;
    }
    let mut result = [0_u8; 19];
    result.copy_from_slice(&lengths);
    rle.iter()
        .all(|token| result[usize::from(token.symbol)] != 0)
        .then_some(result)
}

fn deft4j_dynamic_plan(
    data_bits: u64,
    literal_lengths: &[u8],
    distance_lengths: &[u8],
    rle: Vec<RleToken>,
    code_length_lengths: [u8; 19],
) -> Option<DynamicPlan> {
    let hclen = trim_code_lengths(&code_length_lengths);
    let mut plan = DynamicPlan {
        literal_lengths: try_vec_from_slice(literal_lengths)?,
        distance_lengths: try_vec_from_slice(distance_lengths)?,
        code_length_lengths,
        rle,
        hlit: literal_lengths.len(),
        hdist: distance_lengths.len(),
        hclen,
        bits: 0,
    };
    plan.bits = dynamic_bits(data_bits, &plan)?;
    Some(plan)
}

fn try_vec_from_slice<T: Copy>(source: &[T]) -> Option<Vec<T>> {
    let mut output = Vec::new();
    output.try_reserve_exact(source.len()).ok()?;
    output.extend_from_slice(source);
    Some(output)
}

fn deft4j_pack_code_lengths(lengths: &[u8], options: Deft4jPackOptions) -> Option<Vec<RleToken>> {
    let mut output = Vec::new();
    output.try_reserve_exact(316).ok()?;
    let mut index = 0;
    while index < lengths.len() {
        let value = lengths[index];
        let mut run = 1;
        while index + run < lengths.len() && lengths[index + run] == value {
            run += 1;
        }
        index += run;

        if value == 0 {
            if !options.no_long_zero_repeat {
                let mut count = 138;
                while count >= 11 {
                    if run >= count {
                        output.push(RleToken {
                            symbol: 18,
                            extra: (count - 11) as u8,
                        });
                        run -= count;
                    } else {
                        count -= 1;
                    }
                }
            }
            if !options.no_zero_repeat {
                let mut count = 10;
                while count >= 3 {
                    if run >= count {
                        output.push(RleToken {
                            symbol: 17,
                            extra: (count - 3) as u8,
                        });
                        run -= count;
                    } else {
                        count -= 1;
                    }
                }
            }
        }

        if !options.no_repeat && run != 0 && (!options.no_repeat_zeros || value != 0) {
            output.push(RleToken {
                symbol: value,
                extra: 0,
            });
            run -= 1;
            let mut count = 6;
            while count >= 3 {
                if options.special_repeat && options.use_eight && run == 8 {
                    output.push(RleToken {
                        symbol: 16,
                        extra: 1,
                    });
                    output.push(RleToken {
                        symbol: 16,
                        extra: 1,
                    });
                    run -= 8;
                    break;
                }
                if options.special_repeat && options.use_seven && run == 7 {
                    output.push(RleToken {
                        symbol: 16,
                        extra: 1,
                    });
                    output.push(RleToken {
                        symbol: 16,
                        extra: 0,
                    });
                    run -= 7;
                    break;
                }
                if run >= count {
                    output.push(RleToken {
                        symbol: 16,
                        extra: (count - 3) as u8,
                    });
                    run -= count;
                } else {
                    count -= 1;
                }
            }
        }

        output.extend(
            std::iter::repeat(RleToken {
                symbol: value,
                extra: 0,
            })
            .take(run),
        );
    }
    (output.len() <= 316).then_some(output)
}

fn plan_for_trimmed_lengths(
    literal_lengths: Vec<u8>,
    distance_lengths: Vec<u8>,
    data_bits: u64,
    exhaustive: bool,
    rle_mask: u8,
) -> Option<DynamicPlan> {
    let mut concatenated = literal_lengths.clone();
    concatenated.extend_from_slice(&distance_lengths);
    let mut best: Option<DynamicPlan> = None;
    for mask in 0..8 {
        if rle_mask & (1 << mask) == 0 {
            continue;
        }
        let no_16 = mask & 1 != 0;
        let no_17 = mask & 2 != 0;
        let no_18 = mask & 4 != 0;
        let rle = greedy_rle(&concatenated, no_16, no_17, no_18);
        consider_rle(
            data_bits,
            &literal_lengths,
            &distance_lengths,
            &rle,
            exhaustive,
            &mut best,
        );

        // A greedy six-length repeat can leave one or two explicit lengths.
        // Columbo's additive packer generalizes deft4j's 4+3 and 4+4 OHH
        // alternatives so those tails can use repeats instead of literals.
        if !no_16 {
            let balanced = balanced_repeat_rle(&concatenated, no_17, no_18);
            if balanced != rle {
                consider_rle(
                    data_bits,
                    &literal_lengths,
                    &distance_lengths,
                    &balanced,
                    exhaustive,
                    &mut best,
                );
            }

            // deft4j also permits symbol 16 to continue a zero left explicit
            // after disabling either zero-repeat code. This source-shaped
            // header is distinct from the ordinary mask grid, which spells
            // every short zero tail literally.
            let zero_repeat = deft4j_zero_repeat_rle(&concatenated, no_17, no_18);
            if zero_repeat != rle && zero_repeat != balanced {
                consider_rle(
                    data_bits,
                    &literal_lengths,
                    &distance_lengths,
                    &zero_repeat,
                    exhaustive,
                    &mut best,
                );
            }
        }
    }
    best
}

fn length_swap_delta(frequency_a: u32, frequency_b: u32, length_a: u8, length_b: u8) -> i64 {
    let frequency_a = i64::from(frequency_a);
    let frequency_b = i64::from(frequency_b);
    let length_a = i64::from(length_a);
    let length_b = i64::from(length_b);
    frequency_a * (length_b - length_a) + frequency_b * (length_a - length_b)
}

#[allow(clippy::too_many_arguments)]
fn improve_by_length_swaps(
    tokens: &[Token],
    literal_frequencies: &[u32; 286],
    distance_frequencies: &[u32; 30],
    seed: &DynamicPlan,
    exhaustive: bool,
    expired: &mut impl FnMut() -> bool,
    best: &mut Option<DynamicPlan>,
) {
    let mut literal_lengths = vec![0_u8; 286];
    let mut distance_lengths = vec![0_u8; 30];
    if seed.literal_lengths.len() > literal_lengths.len()
        || seed.distance_lengths.len() > distance_lengths.len()
    {
        return;
    }
    literal_lengths[..seed.literal_lengths.len()].copy_from_slice(&seed.literal_lengths);
    distance_lengths[..seed.distance_lengths.len()].copy_from_slice(&seed.distance_lengths);

    // Columbo's greedy swap search uses only the ordinary RLE spelling while
    // deciding which length exchange to commit. It then scores the complete
    // header family. DeflOpt 2.07 has no finished-tree swap search.
    let Some(mut current) = plan_for_explicit_lengths_masked(
        tokens,
        &literal_lengths,
        &distance_lengths,
        exhaustive,
        0x01,
    ) else {
        return;
    };
    keep_better(best, current.clone());

    improve_one_tree_by_swaps(
        tokens,
        literal_frequencies,
        &mut literal_lengths,
        &mut distance_lengths,
        true,
        exhaustive,
        expired,
        &mut current,
        best,
    );
    improve_one_tree_by_swaps(
        tokens,
        distance_frequencies,
        &mut literal_lengths,
        &mut distance_lengths,
        false,
        exhaustive,
        expired,
        &mut current,
        best,
    );
}

#[allow(clippy::too_many_arguments)]
fn improve_one_tree_by_swaps(
    tokens: &[Token],
    frequencies: &[u32],
    literal_lengths: &mut [u8],
    distance_lengths: &mut [u8],
    literal_tree: bool,
    exhaustive: bool,
    expired: &mut impl FnMut() -> bool,
    current: &mut DynamicPlan,
    best: &mut Option<DynamicPlan>,
) {
    const MAX_PASSES: usize = 12;

    for _ in 0..MAX_PASSES {
        if expired() {
            break;
        }

        let mut selected_pair = None;
        let mut selected_plan = current.clone();
        for a in 0..frequencies.len() {
            if expired() {
                return;
            }
            let length_a = if literal_tree {
                literal_lengths[a]
            } else {
                distance_lengths[a]
            };
            if length_a == 0 {
                continue;
            }

            for b in (a + 1)..frequencies.len() {
                let length_b = if literal_tree {
                    literal_lengths[b]
                } else {
                    distance_lengths[b]
                };
                if length_b == 0 || length_a == length_b {
                    continue;
                }
                if length_swap_delta(frequencies[a], frequencies[b], length_a, length_b) > 0 {
                    continue;
                }

                if literal_tree {
                    literal_lengths.swap(a, b);
                } else {
                    distance_lengths.swap(a, b);
                }
                let candidate = plan_for_explicit_lengths_masked(
                    tokens,
                    literal_lengths,
                    distance_lengths,
                    exhaustive,
                    0x01,
                );
                if literal_tree {
                    literal_lengths.swap(a, b);
                } else {
                    distance_lengths.swap(a, b);
                }

                if let Some(candidate) = candidate {
                    if candidate.bits < selected_plan.bits {
                        selected_pair = Some((a, b));
                        selected_plan = candidate;
                    }
                }
            }
        }

        let Some((a, b)) = selected_pair else {
            break;
        };
        if literal_tree {
            literal_lengths.swap(a, b);
        } else {
            distance_lengths.swap(a, b);
        }
        *current = selected_plan;
        keep_better(best, current.clone());

        if let Some(full_plan) =
            plan_for_explicit_lengths(tokens, literal_lengths, distance_lengths, exhaustive)
        {
            keep_better(best, full_plan);
        }
    }
}

fn keep_better(best: &mut Option<DynamicPlan>, candidate: DynamicPlan) {
    if best
        .as_ref()
        .map_or(true, |current| candidate.bits < current.bits)
    {
        *best = Some(candidate);
    }
}

fn consider_rle(
    data_bits: u64,
    literal_lengths: &[u8],
    distance_lengths: &[u8],
    initial_rle: &[RleToken],
    exhaustive: bool,
    best: &mut Option<DynamicPlan>,
) {
    let mut rle = initial_rle.to_vec();
    let mut decoded_lengths = Vec::new();
    if decoded_lengths
        .try_reserve_exact(literal_lengths.len() + distance_lengths.len())
        .is_err()
    {
        return;
    }
    decoded_lengths.extend_from_slice(literal_lengths);
    decoded_lengths.extend_from_slice(distance_lengths);

    // From each Columbo RLE-mask seed, price a DeflOpt-derived local candidate:
    // build DeflOpt's height-tied code-length tree, replace repeat tokens that
    // are locally dearer than explicit lengths, then rebuild. Every rewrite
    // strictly reduces the finite repeat rank, so this remains bounded. It is
    // not DeflOpt's complete state-feedback route.
    for variant in 0..4 {
        consider_columbo_deflopt_local_rewrite(
            data_bits,
            literal_lengths,
            distance_lengths,
            &rle,
            variant,
            best,
        );
    }

    // This four-pass, best-intermediate loop is Columbo's composite header
    // route. Its numeric bound is inspired by Defluff, but Defluff always emits
    // its fourth pass and neither stops early nor retains earlier winners.
    let passes = 4;
    for pass in 0..passes {
        let frequencies = rle_frequencies(&rle);
        let variants = 4;
        let mut code_length_candidates = Vec::new();
        for variant in 0..variants {
            push_unique(
                &mut code_length_candidates,
                make_lengths_order_heap(&frequencies, 7, variant),
            );
            // The deft4j tree's PriorityQueue-compatible heap is expensive
            // across the full data alphabets, but the code-length alphabet has
            // only nineteen symbols. Keeping it in Columbo's ordinary header
            // repack closes RLE gaps without broadening token search.
            if variant == 0 {
                let deft4j_lengths = make_lengths_deft4j_java_heap(&frequencies, 7);
                consider_deft4j_pruned_header(
                    data_bits,
                    literal_lengths,
                    distance_lengths,
                    &rle,
                    &deft4j_lengths,
                    best,
                );
                push_unique(&mut code_length_candidates, deft4j_lengths);
                // This is Columbo's generic code-length tree with Defluff's
                // limiter, not Defluff's complete tree builder. Pricing the
                // hybrid once is inexpensive when the data trees are fixed.
                push_unique(
                    &mut code_length_candidates,
                    make_lengths_columbo_defluff_limited(&frequencies, 7, 0),
                );
            }
            if exhaustive {
                push_unique(
                    &mut code_length_candidates,
                    make_lengths_deflopt_heap(&frequencies, 7, variant),
                );
                push_unique(
                    &mut code_length_candidates,
                    make_lengths(&frequencies, 7, variant),
                );
                push_unique(
                    &mut code_length_candidates,
                    make_lengths_defluff_exact(&frequencies, 7, variant),
                );
            }
        }

        let mut feedback_tree = None;
        for candidate_lengths in code_length_candidates {
            if candidate_lengths.len() != 19
                || Huffman::build(&candidate_lengths).is_none()
                || rle
                    .iter()
                    .any(|token| candidate_lengths[usize::from(token.symbol)] == 0)
            {
                continue;
            }
            let mut code_length_lengths = [0_u8; 19];
            code_length_lengths.copy_from_slice(&candidate_lengths);
            let hclen = trim_code_lengths(&code_length_lengths);
            let mut candidate = DynamicPlan {
                literal_lengths: literal_lengths.to_vec(),
                distance_lengths: distance_lengths.to_vec(),
                code_length_lengths,
                rle: rle.clone(),
                hlit: literal_lengths.len(),
                hdist: distance_lengths.len(),
                hclen,
                bits: 0,
            };
            if let Some(bits) = dynamic_bits(data_bits, &candidate) {
                candidate.bits = bits;
                if best
                    .as_ref()
                    .map_or(true, |current| candidate.bits < current.bits)
                {
                    *best = Some(candidate.clone());
                }
                if feedback_tree.as_ref().map_or(true, |current: &[u8; 19]| {
                    rle_cost(&rle, &candidate.code_length_lengths) < rle_cost(&rle, current)
                }) {
                    feedback_tree = Some(candidate.code_length_lengths);
                }

                // Reassigning the same code-length histogram preserves a
                // valid canonical tree. DeflOpt pairs shorter codes with the
                // more frequent RLE symbols; the changed HCLEN tail can make
                // this worthwhile even when weighted symbol cost ties.
                let mut reordered = candidate.clone();
                if reorder_code_length_lengths(&mut reordered.code_length_lengths, &frequencies) {
                    reordered.hclen = trim_code_lengths(&reordered.code_length_lengths);
                    if let Some(bits) = dynamic_bits(data_bits, &reordered) {
                        reordered.bits = bits;
                        keep_better(best, reordered);
                    }
                }
            }
        }

        if pass + 1 == passes {
            break;
        }
        let Some(tree) = feedback_tree else {
            break;
        };
        let mut rewritten = rewrite_rle_deflopt_local(&rle, &tree);
        if exhaustive {
            if let Some(shortest) = shortest_rle(&decoded_lengths, &tree) {
                if rewritten.as_ref().map_or(true, |local| {
                    rle_cost(&shortest, &tree) < rle_cost(local, &tree)
                }) {
                    rewritten = Some(shortest);
                }
            }
        }
        let Some(rewritten) = rewritten else {
            break;
        };
        if rewritten == rle {
            break;
        }
        rle = rewritten;
    }
}

/// Add deft4j's code-length tree and equal-cost RLE prune to a Columbo header.
///
/// The deft4j route first expands repeat tokens whose explicit spelling is no
/// dearer under the current code-length tree, then rebuilds that tiny tree.
/// Although the first rewrite may tie, the changed frequencies can shorten the
/// rebuilt header. Only the dynamic header changes; data symbols and LZ77
/// tokens remain untouched.
fn consider_deft4j_pruned_header(
    data_bits: u64,
    literal_lengths: &[u8],
    distance_lengths: &[u8],
    rle: &[RleToken],
    deft4j_lengths: &[u8],
    best: &mut Option<DynamicPlan>,
) {
    if deft4j_lengths.len() != 19 || Huffman::build(deft4j_lengths).is_none() {
        return;
    }
    let mut code_length_lengths = [0_u8; 19];
    code_length_lengths.copy_from_slice(deft4j_lengths);
    if rle
        .iter()
        .any(|token| code_length_lengths[usize::from(token.symbol)] == 0)
    {
        return;
    }

    let Some(pruned) = rewrite_rle_deft4j_literals(rle, &code_length_lengths, true) else {
        return;
    };
    let frequencies = rle_frequencies(&pruned);
    let rebuilt = make_lengths_deft4j_java_heap(&frequencies, 7);
    if rebuilt.len() != 19 || Huffman::build(&rebuilt).is_none() {
        return;
    }
    code_length_lengths.copy_from_slice(&rebuilt);
    if pruned
        .iter()
        .any(|token| code_length_lengths[usize::from(token.symbol)] == 0)
    {
        return;
    }

    let mut plan = DynamicPlan {
        literal_lengths: literal_lengths.to_vec(),
        distance_lengths: distance_lengths.to_vec(),
        code_length_lengths,
        rle: pruned,
        hlit: literal_lengths.len(),
        hdist: distance_lengths.len(),
        hclen: trim_code_lengths(&code_length_lengths),
        bits: 0,
    };
    if let Some(bits) = dynamic_bits(data_bits, &plan) {
        plan.bits = bits;
        keep_better(best, plan.clone());
    }

    // deft4j's final optimiseHeader step expands only strictly dearer repeats
    // under the rebuilt tree and deliberately keeps that tree unchanged.
    if let Some(optimized) =
        rewrite_rle_deft4j_literals(&plan.rle, &plan.code_length_lengths, false)
    {
        plan.rle = optimized;
        if let Some(bits) = dynamic_bits(data_bits, &plan) {
            plan.bits = bits;
            keep_better(best, plan);
        }
    }
}

/// Price Columbo's bounded DeflOpt-derived local rewrite/rebuild candidate.
///
/// DeflOpt supplies the strict local repeat rewrite and height-tied tree
/// rebuild. Columbo starts from each of its own RLE-mask seeds and also prices
/// a frequency-reassigned tree as an additive candidate, rather than feeding
/// every state through DeflOpt's complete bounded feedback route. The data-code
/// lengths and LZ77 parse remain fixed.
fn consider_columbo_deflopt_local_rewrite(
    data_bits: u64,
    literal_lengths: &[u8],
    distance_lengths: &[u8],
    initial_rle: &[RleToken],
    variant: u32,
    best: &mut Option<DynamicPlan>,
) {
    let mut rle = initial_rle.to_vec();
    let mut rank = repeat_rank(&rle);

    loop {
        let frequencies = rle_frequencies(&rle);
        let candidate_lengths = make_lengths_deflopt_heap(&frequencies, 7, variant);
        if candidate_lengths.len() != 19
            || Huffman::build(&candidate_lengths).is_none()
            || rle
                .iter()
                .any(|token| candidate_lengths[usize::from(token.symbol)] == 0)
        {
            return;
        }

        let mut code_length_lengths = [0_u8; 19];
        code_length_lengths.copy_from_slice(&candidate_lengths);
        let mut plan = DynamicPlan {
            literal_lengths: literal_lengths.to_vec(),
            distance_lengths: distance_lengths.to_vec(),
            code_length_lengths,
            rle: rle.clone(),
            hlit: literal_lengths.len(),
            hdist: distance_lengths.len(),
            hclen: trim_code_lengths(&code_length_lengths),
            bits: 0,
        };
        let Some(bits) = dynamic_bits(data_bits, &plan) else {
            return;
        };
        plan.bits = bits;
        keep_better(best, plan.clone());

        // The pair-swap pass preserves the code-length tree histogram while
        // assigning its shorter codes to more frequent RLE symbols. Retain it
        // as an additive candidate; the local rewrite/rebuild loop follows the
        // unmodified DeflOpt tree, which keeps tie behaviour deterministic.
        let mut reordered = plan.clone();
        if reorder_code_length_lengths(&mut reordered.code_length_lengths, &frequencies) {
            reordered.hclen = trim_code_lengths(&reordered.code_length_lengths);
            if let Some(bits) = dynamic_bits(data_bits, &reordered) {
                reordered.bits = bits;
                keep_better(best, reordered);
            }
        }

        if rank == 0 {
            break;
        }
        let Some(rewritten) = rewrite_rle_deflopt_local(&rle, &plan.code_length_lengths) else {
            break;
        };
        let next_rank = repeat_rank(&rewritten);
        if next_rank >= rank {
            break;
        }

        // Price the rewrite once under the existing tree before rebuilding.
        // This is a distinct legal header and occasionally beats both adjacent
        // fixed-point states by one or two bits.
        let mut same_tree = plan;
        same_tree.rle = rewritten.clone();
        if let Some(bits) = dynamic_bits(data_bits, &same_tree) {
            same_tree.bits = bits;
            keep_better(best, same_tree);
        }

        rle = rewritten;
        rank = next_rank;
    }
}

/// Replace repeat tokens only when their current-tree local cost decreases.
fn rewrite_rle_deflopt_local(
    input: &[RleToken],
    code_length_lengths: &[u8; 19],
) -> Option<Vec<RleToken>> {
    let mut output = Vec::new();
    output.try_reserve(input.len()).ok()?;
    let mut previous = None;
    let mut changed = false;

    for &token in input {
        match token.symbol {
            value @ 0..=15 => {
                output.push(token);
                previous = Some(value);
            }
            18 => {
                let count = usize::from(token.extra) + 11;
                if explicit_rle_cost(code_length_lengths, 0, count)
                    < rle_symbol_cost(code_length_lengths, 18)
                {
                    output.extend(
                        std::iter::repeat(RleToken {
                            symbol: 0,
                            extra: 0,
                        })
                        .take(count),
                    );
                    changed = true;
                } else {
                    output.push(token);
                }
                previous = Some(0);
            }
            17 => {
                let count = usize::from(token.extra) + 3;
                if previous == Some(0)
                    && token.extra <= 3
                    && code_length_lengths[16] != 0
                    && code_length_lengths[17] != 0
                    && code_length_lengths[16] <= code_length_lengths[17]
                {
                    output.push(RleToken {
                        symbol: 16,
                        extra: token.extra,
                    });
                    changed = true;
                } else if explicit_rle_cost(code_length_lengths, 0, count)
                    < rle_symbol_cost(code_length_lengths, 17)
                {
                    output.extend(
                        std::iter::repeat(RleToken {
                            symbol: 0,
                            extra: 0,
                        })
                        .take(count),
                    );
                    changed = true;
                } else {
                    output.push(token);
                }
                previous = Some(0);
            }
            16 => {
                let previous = previous?;
                let count = usize::from(token.extra) + 3;
                if explicit_rle_cost(code_length_lengths, previous, count)
                    < rle_symbol_cost(code_length_lengths, 16)
                {
                    output.extend(
                        std::iter::repeat(RleToken {
                            symbol: previous,
                            extra: 0,
                        })
                        .take(count),
                    );
                    changed = true;
                } else {
                    output.push(token);
                }
            }
            _ => return None,
        }
    }

    changed.then_some(output)
}

/// Expand repeat tokens that are dearer (or tied) under a fixed tree.
///
/// This mirrors deft4j's header-only prune. `include_equal` is used before a
/// deft4j tree rebuild because a tied local rewrite can change the next tree;
/// the final optimize-header pass accepts strict local savings only.
fn rewrite_rle_deft4j_literals(
    input: &[RleToken],
    code_length_lengths: &[u8; 19],
    include_equal: bool,
) -> Option<Vec<RleToken>> {
    let mut output = Vec::new();
    output.try_reserve(316).ok()?;
    let mut previous = None;
    let mut changed = false;

    for &token in input {
        let (value, count) = match token.symbol {
            value @ 0..=15 => {
                output.push(token);
                previous = Some(value);
                continue;
            }
            16 => (previous?, usize::from(token.extra) + 3),
            17 => (0, usize::from(token.extra) + 3),
            18 => (0, usize::from(token.extra) + 11),
            _ => return None,
        };
        let explicit = explicit_rle_cost(code_length_lengths, value, count);
        let repeat = rle_symbol_cost(code_length_lengths, token.symbol);
        if explicit < repeat || (include_equal && explicit == repeat) {
            if output.len().checked_add(count)? > 316 {
                return None;
            }
            output.extend(
                std::iter::repeat(RleToken {
                    symbol: value,
                    extra: 0,
                })
                .take(count),
            );
            changed = true;
        } else {
            output.push(token);
        }
        previous = Some(value);
    }
    changed.then_some(output)
}

fn explicit_rle_cost(lengths: &[u8; 19], symbol: u8, count: usize) -> u64 {
    let length = lengths[usize::from(symbol)];
    if length == 0 {
        INF
    } else {
        u64::from(length).saturating_mul(count as u64).min(INF)
    }
}

fn rle_symbol_cost(lengths: &[u8; 19], symbol: u8) -> u64 {
    let length = lengths[usize::from(symbol)];
    if length == 0 {
        INF
    } else {
        u64::from(length)
            .saturating_add(rle_extra_bits(symbol))
            .min(INF)
    }
}

fn repeat_rank(rle: &[RleToken]) -> usize {
    rle.iter()
        .map(|token| match token.symbol {
            17 => 2,
            16 | 18 => 1,
            _ => 0,
        })
        .sum()
}

fn reorder_code_length_lengths(lengths: &mut [u8; 19], frequencies: &[u32; 19]) -> bool {
    let mut changed = false;
    for left in 0..18 {
        if lengths[left] == 0 {
            continue;
        }
        for right in (left + 1)..19 {
            if lengths[right] == 0 {
                continue;
            }
            let should_swap = if lengths[right] < lengths[left] {
                frequencies[right] < frequencies[left]
            } else if lengths[right] > lengths[left] {
                frequencies[right] > frequencies[left]
            } else {
                false
            };
            if should_swap {
                lengths.swap(left, right);
                changed = true;
            }
        }
    }
    changed
}

fn greedy_rle(lengths: &[u8], no_16: bool, no_17: bool, no_18: bool) -> Vec<RleToken> {
    let mut output = Vec::new();
    let mut index = 0;
    while index < lengths.len() {
        let value = lengths[index];
        let mut run = 1;
        while index + run < lengths.len() && lengths[index + run] == value {
            run += 1;
        }
        index += run;

        if value == 0 {
            while !no_18 && run >= 11 {
                let count = run.min(138);
                output.push(RleToken {
                    symbol: 18,
                    extra: (count - 11) as u8,
                });
                run -= count;
            }
            while !no_17 && run >= 3 {
                let count = run.min(10);
                output.push(RleToken {
                    symbol: 17,
                    extra: (count - 3) as u8,
                });
                run -= count;
            }
        } else if !no_16 && run >= 4 {
            output.push(RleToken {
                symbol: value,
                extra: 0,
            });
            run -= 1;
            while run >= 3 {
                let count = run.min(6);
                output.push(RleToken {
                    symbol: 16,
                    extra: (count - 3) as u8,
                });
                run -= count;
            }
        }
        output.extend(
            std::iter::repeat(RleToken {
                symbol: value,
                extra: 0,
            })
            .take(run),
        );
    }
    output
}

/// Columbo's generalized form of deft4j's balanced repeat-16 alternatives.
///
/// deft4j directly tries 4+3 and 4+4 for seven- and eight-value tails.
/// Columbo applies the same idea whenever greedy six-value chunks would leave
/// one or two explicit values.
fn balanced_repeat_rle(lengths: &[u8], no_17: bool, no_18: bool) -> Vec<RleToken> {
    let mut output = Vec::new();
    let mut index = 0;
    while index < lengths.len() {
        let value = lengths[index];
        let mut run = 1;
        while index + run < lengths.len() && lengths[index + run] == value {
            run += 1;
        }
        index += run;

        if value == 0 {
            while !no_18 && run >= 11 {
                let count = run.min(138);
                output.push(RleToken {
                    symbol: 18,
                    extra: (count - 11) as u8,
                });
                run -= count;
            }
            while !no_17 && run >= 3 {
                let count = run.min(10);
                output.push(RleToken {
                    symbol: 17,
                    extra: (count - 3) as u8,
                });
                run -= count;
            }
        } else if run >= 4 {
            output.push(RleToken {
                symbol: value,
                extra: 0,
            });
            run -= 1;
            while run >= 3 {
                let count = if matches!(run % 6, 1 | 2) && run >= 7 {
                    4
                } else {
                    run.min(6)
                };
                output.push(RleToken {
                    symbol: 16,
                    extra: (count - 3) as u8,
                });
                run -= count;
            }
        }
        output.extend(
            std::iter::repeat(RleToken {
                symbol: value,
                extra: 0,
            })
            .take(run),
        );
    }
    output
}

/// deft4j's greedy packing with repeat-16 enabled for residual zero runs.
fn deft4j_zero_repeat_rle(lengths: &[u8], no_17: bool, no_18: bool) -> Vec<RleToken> {
    let mut output = Vec::new();
    let mut index = 0;
    while index < lengths.len() {
        let value = lengths[index];
        let mut run = 1;
        while index + run < lengths.len() && lengths[index + run] == value {
            run += 1;
        }
        index += run;

        if value == 0 {
            while !no_18 && run >= 11 {
                let count = run.min(138);
                output.push(RleToken {
                    symbol: 18,
                    extra: (count - 11) as u8,
                });
                run -= count;
            }
            while !no_17 && run >= 3 {
                let count = run.min(10);
                output.push(RleToken {
                    symbol: 17,
                    extra: (count - 3) as u8,
                });
                run -= count;
            }
        }

        if run >= 4 {
            output.push(RleToken {
                symbol: value,
                extra: 0,
            });
            run -= 1;
            while run >= 3 {
                let count = if matches!(run % 6, 1 | 2) && run >= 7 {
                    4
                } else {
                    run.min(6)
                };
                output.push(RleToken {
                    symbol: 16,
                    extra: (count - 3) as u8,
                });
                run -= count;
            }
        }
        output.extend(
            std::iter::repeat(RleToken {
                symbol: value,
                extra: 0,
            })
            .take(run),
        );
    }
    output
}

fn rle_frequencies(rle: &[RleToken]) -> [u32; 19] {
    let mut frequencies = [0_u32; 19];
    for token in rle {
        frequencies[usize::from(token.symbol)] += 1;
    }
    frequencies
}

fn trim_code_lengths(lengths: &[u8; 19]) -> usize {
    (4..19)
        .rev()
        .find(|&index| lengths[CODE_LENGTH_ORDER[index]] != 0)
        .map_or(4, |index| index + 1)
}

fn dynamic_bits(data_bits: u64, plan: &DynamicPlan) -> Option<u64> {
    let mut bits = 3_u64 + 5 + 5 + 4 + u64::try_from(plan.hclen).ok()? * 3;
    for token in &plan.rle {
        let code_bits = *plan.code_length_lengths.get(usize::from(token.symbol))?;
        if code_bits == 0 {
            return None;
        }
        bits = bits.checked_add(u64::from(code_bits) + rle_extra_bits(token.symbol))?;
    }
    bits.checked_add(data_bits)
}

fn rle_extra_bits(symbol: u8) -> u64 {
    match symbol {
        16 => 2,
        17 => 3,
        18 => 7,
        _ => 0,
    }
}

fn rle_cost(rle: &[RleToken], lengths: &[u8; 19]) -> u64 {
    rle.iter()
        .map(|token| {
            let code = lengths[usize::from(token.symbol)];
            if code == 0 {
                INF
            } else {
                u64::from(code) + rle_extra_bits(token.symbol)
            }
        })
        .fold(0_u64, |sum, value| sum.saturating_add(value).min(INF))
}

/// Find the cheapest valid RLE stream under a fixed code-length tree.
fn shortest_rle(lengths: &[u8], costs: &[u8; 19]) -> Option<Vec<RleToken>> {
    #[derive(Clone, Copy)]
    struct Step {
        next: usize,
        token: RleToken,
    }

    let mut best = vec![INF; lengths.len() + 1];
    let mut step = vec![None; lengths.len()];
    let mut run_lengths = vec![1_usize; lengths.len()];
    for index in (0..lengths.len().saturating_sub(1)).rev() {
        if lengths[index] == lengths[index + 1] {
            run_lengths[index] = run_lengths[index + 1] + 1;
        }
    }
    best[lengths.len()] = 0;
    for index in (0..lengths.len()).rev() {
        let mut consider = |count: usize, symbol: u8, extra: u8| {
            let code = costs[usize::from(symbol)];
            if code == 0 || index + count > lengths.len() {
                return;
            }
            let cost = u64::from(code)
                .saturating_add(rle_extra_bits(symbol))
                .saturating_add(best[index + count]);
            // Strict replacement retains Columbo's source-like transition
            // order on equal cost, matching the original Columbo C
            // implementation.
            if cost < best[index] {
                best[index] = cost;
                step[index] = Some(Step {
                    next: index + count,
                    token: RleToken { symbol, extra },
                });
            }
        };

        consider(1, lengths[index], 0);
        if index > 0 && lengths[index] == lengths[index - 1] {
            for count in 3..=run_lengths[index].min(6) {
                consider(count, 16, (count - 3) as u8);
            }
        }
        if lengths[index] == 0 {
            for count in 3..=run_lengths[index].min(10) {
                consider(count, 17, (count - 3) as u8);
            }
            for count in 11..=run_lengths[index].min(138) {
                consider(count, 18, (count - 11) as u8);
            }
        }
    }
    if best[0] == INF {
        return None;
    }
    let mut output = Vec::new();
    let mut index = 0;
    while index < lengths.len() {
        let selected = step[index]?;
        output.push(selected.token);
        index = selected.next;
    }
    Some(output)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rle_round_trip_shape() {
        let lengths = [0, 0, 0, 0, 3, 3, 3, 3, 3, 0, 0, 0];
        let rle = greedy_rle(&lengths, false, false, false);
        assert!(rle.iter().any(|token| token.symbol == 16));
        assert!(rle.iter().any(|token| token.symbol == 17));
    }

    #[test]
    fn distance_symbol_one_is_not_trimmed_away() {
        let mut lengths = [0_u8; 30];
        lengths[1] = 1;
        assert_eq!(trim_distance(&lengths), 2);
    }

    #[test]
    fn balanced_repeat_avoids_a_two_literal_tail() {
        let lengths = [5_u8; 9];
        let greedy = greedy_rle(&lengths, false, false, false);
        let balanced = balanced_repeat_rle(&lengths, false, false);

        assert_eq!(greedy.len(), 4);
        assert_eq!(balanced.len(), 3);
        assert_eq!(
            balanced[0],
            RleToken {
                symbol: 5,
                extra: 0
            }
        );
        assert_eq!(
            balanced[1],
            RleToken {
                symbol: 16,
                extra: 1
            }
        );
        assert_eq!(
            balanced[2],
            RleToken {
                symbol: 16,
                extra: 1
            }
        );
    }

    #[test]
    fn deft4j_zero_repeat_can_continue_a_literal_zero() {
        let lengths = [0_u8; 5];
        let ordinary = greedy_rle(&lengths, false, true, false);
        let deft4j = deft4j_zero_repeat_rle(&lengths, true, false);

        assert_eq!(
            ordinary,
            vec![
                RleToken {
                    symbol: 0,
                    extra: 0
                };
                5
            ]
        );
        assert_eq!(
            deft4j,
            [
                RleToken {
                    symbol: 0,
                    extra: 0,
                },
                RleToken {
                    symbol: 16,
                    extra: 1,
                },
            ]
        );
    }

    #[test]
    fn package_merge_can_win_the_code_length_alphabet() {
        // This histogram is a reduced header-only regression for a block where
        // Defluff's package-merge tree saves one bit over every heap tree. Lay
        // the symbols out without adjacent repeats so this test isolates the
        // nineteen-symbol tree choice rather than RLE packing.
        let mut remaining = [134_u32, 2, 0, 1, 1, 6, 5, 39, 33, 21, 74];
        let mut lengths = Vec::new();
        lengths.try_reserve_exact(316).unwrap();
        while lengths.len() < 316 {
            let previous = lengths.last().copied();
            let symbol = remaining
                .iter()
                .enumerate()
                .filter(|&(symbol, &count)| count != 0 && Some(symbol as u8) != previous)
                .max_by_key(|&(symbol, &count)| (count, std::cmp::Reverse(symbol)))
                .map(|(symbol, _)| symbol)
                .unwrap();
            remaining[symbol] -= 1;
            lengths.push(symbol as u8);
        }
        assert!(remaining.iter().all(|&count| count == 0));
        assert!(lengths.windows(2).all(|pair| pair[0] != pair[1]));

        let rle: Vec<_> = lengths
            .iter()
            .copied()
            .map(|symbol| RleToken { symbol, extra: 0 })
            .collect();
        let mut best = None;
        consider_rle(
            4_496,
            &lengths[..286],
            &lengths[286..],
            &rle,
            false,
            &mut best,
        );

        let best = best.unwrap();
        assert_eq!(
            best.code_length_lengths,
            [1, 6, 0, 7, 7, 6, 6, 4, 4, 4, 2, 0, 0, 0, 0, 0, 0, 0, 0]
        );
    }

    #[test]
    fn deflopt_local_rewrite_has_a_strictly_decreasing_repeat_rank() {
        let input = [
            RleToken {
                symbol: 0,
                extra: 0,
            },
            RleToken {
                symbol: 17,
                extra: 1,
            },
        ];
        let mut code_lengths = [0_u8; 19];
        code_lengths[0] = 3;
        code_lengths[16] = 2;
        code_lengths[17] = 2;

        let rewritten = rewrite_rle_deflopt_local(&input, &code_lengths).unwrap();
        assert_eq!(rewritten[1].symbol, 16);
        assert!(repeat_rank(&rewritten) < repeat_rank(&input));
    }

    #[test]
    fn deflopt_local_rewrite_expands_only_a_strictly_dearer_repeat() {
        let input = [RleToken {
            symbol: 18,
            extra: 0, // eleven zero lengths
        }];
        let mut code_lengths = [0_u8; 19];
        code_lengths[0] = 1;
        code_lengths[18] = 7;

        let rewritten = rewrite_rle_deflopt_local(&input, &code_lengths).unwrap();
        assert_eq!(rewritten.len(), 11);
        assert!(rewritten.iter().all(|token| token.symbol == 0));

        // Ties are deliberately retained, matching the source optimizer's
        // deterministic strict-improvement rule.
        code_lengths[0] = 1;
        code_lengths[18] = 4;
        assert!(rewrite_rle_deflopt_local(&input, &code_lengths).is_none());
    }

    #[test]
    fn deft4j_prune_can_rebuild_after_an_equal_cost_rewrite() {
        let input = [RleToken {
            symbol: 17,
            extra: 0, // three zero lengths
        }];
        let mut code_lengths = [0_u8; 19];
        code_lengths[0] = 2;
        code_lengths[17] = 3;

        // Three explicit zero codes and one repeat-17 code plus its three
        // extra bits both cost six. The ordinary strict finalizer retains the
        // repeat; deft4j's pre-rebuild prune deliberately expands the tie.
        assert!(rewrite_rle_deft4j_literals(&input, &code_lengths, false).is_none());
        assert_eq!(
            rewrite_rle_deft4j_literals(&input, &code_lengths, true).unwrap(),
            vec![
                RleToken {
                    symbol: 0,
                    extra: 0
                };
                3
            ]
        );
    }

    #[test]
    fn unused_distance_alphabet_can_remain_empty() {
        let frequencies = [0_u32; 30];
        let candidates = tree_candidates(&frequencies, 15, false);
        assert!(candidates
            .iter()
            .any(|lengths| lengths.iter().all(|&length| length == 0)));
    }

    #[test]
    fn equal_frequency_arrangement_preserves_payload_cost_and_histogram() {
        let frequencies = [7, 2, 7, 7, 2, 1];
        let original = [4, 3, 2, 3, 5, 2];
        let original_cost: u32 = frequencies
            .iter()
            .zip(original)
            .map(|(&frequency, length)| frequency * u32::from(length))
            .sum();

        let mut ascending = original;
        arrange_equal_frequency_lengths(&frequencies, &mut ascending, false);
        assert_eq!(ascending, [2, 3, 3, 4, 5, 2]);

        let mut descending = original;
        arrange_equal_frequency_lengths(&frequencies, &mut descending, true);
        assert_eq!(descending, [4, 5, 3, 2, 3, 2]);

        for arranged in [ascending, descending] {
            let arranged_cost: u32 = frequencies
                .iter()
                .zip(arranged)
                .map(|(&frequency, length)| frequency * u32::from(length))
                .sum();
            let mut histogram = arranged;
            histogram.sort_unstable();
            let mut original_histogram = original;
            original_histogram.sort_unstable();
            assert_eq!(arranged_cost, original_cost);
            assert_eq!(histogram, original_histogram);
        }
    }
}
