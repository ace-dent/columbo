// SPDX-License-Identifier: MIT
// SPDX-FileCopyrightText: 2026 Andrew C.E. Dent <https://github.com/ace-dent>

//! deft4j beta-17's header for fixed data trees: its code-length run packer,
//! its Java-heap code-length code and its ordered header option grid. The
//! deft4j-derived source route (M1) prices headers with these instead of
//! Columbo's wider header search.

use super::code_length_tree::{code_length_tree, CodeLengthBuilder};
use super::rle_rewrite::rewrite_rle_deft4j_literals;
use super::{
    dynamic_bits, keep_better, rle_frequencies, token_bits_from_frequencies, trim_code_lengths,
    trim_distance, trim_literal,
};
use crate::deflate::huffman::{
    code_length_tree_shape_is_valid, huffman_code_lengths_are_valid, payload_tree_shape_is_valid,
    TreeBuildScratch,
};
use crate::deflate::model::{
    try_clone_slice, DynamicPlan, RleToken, MAX_DYNAMIC_CODE_LENGTH_COUNT,
};

/// `Complete` is the 56-way header option grid of deft4j's
/// `addOptimisedRecoded`. `DefaultRecode` only decides whether deft4j's
/// repeated individual-prune step reached a smaller fixed point.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Deft4jHeaderPolicy {
    Complete,
    DefaultRecode,
}

/// Switches for deft4j's code-length run packer. `special_repeat` is deft4j's
/// `ohh` flag, which enables the 4+4 (`use_eight`) and 4+3 (`use_seven`)
/// code-16 splits.
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

/// Score a dynamic header with deft4j beta-17's ordered header grid, keeping
/// the data trees fixed and trimming HLIT and HDIST (beta-17 can keep the
/// source HLIT and HDIST for its source-dynamic base). Columbo's wider header
/// search would change which intermediate states the deft4j-derived route
/// keeps.
pub(crate) fn plan_for_deft4j_lengths_with_cost(
    literal_frequencies: &[u32; 286],
    distance_frequencies: &[u32; 30],
    extra_bits: u64,
    literal_lengths: &[u8; 286],
    distance_lengths: &[u8; 30],
    strict: bool,
    policy: Deft4jHeaderPolicy,
) -> Option<DynamicPlan> {
    let mut literal_lengths = *literal_lengths;
    apply_min_code_lengths(&mut literal_lengths, literal_frequencies, strict);
    let mut distance_lengths = *distance_lengths;
    apply_min_code_lengths(&mut distance_lengths, distance_frequencies, strict);

    let data_bits = token_bits_from_frequencies(
        literal_frequencies,
        distance_frequencies,
        &literal_lengths,
        &distance_lengths,
        extra_bits,
    )?;
    let hlit = trim_literal(&literal_lengths);
    let hdist = trim_distance(&distance_lengths);
    let literal = try_clone_slice(&literal_lengths[..hlit])?;
    let distance = try_clone_slice(&distance_lengths[..hdist])?;
    if !huffman_code_lengths_are_valid(&literal)
        || !huffman_code_lengths_are_valid(&distance)
        || !payload_tree_shape_is_valid(&literal, false)
        || !payload_tree_shape_is_valid(&distance, true)
    {
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
    // Keep deft4j's loop order: of equal-sized headers the first spelling
    // wins, and it becomes the block used by later transformations and merges.
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

fn apply_min_code_lengths(lengths: &mut [u8], frequencies: &[u32], enabled: bool) {
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
        if let Some((pruned, _)) =
            rewrite_rle_deft4j_literals(&rle, &rle_frequencies(&rle), &code_length_lengths, true)
        {
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
        if let Some((optimized, _)) = rewrite_rle_deft4j_literals(
            &plan.rle,
            &rle_frequencies(&plan.rle),
            &plan.code_length_lengths,
            false,
        ) {
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
    let lengths = code_length_tree(
        CodeLengthBuilder::Deft4jJavaHeap,
        &frequencies,
        &mut TreeBuildScratch::default(),
    );
    if !code_length_tree_shape_is_valid(&lengths) {
        return None;
    }
    rle.iter()
        .all(|token| lengths[usize::from(token.symbol)] != 0)
        .then_some(lengths)
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
        literal_lengths: try_clone_slice(literal_lengths)?,
        distance_lengths: try_clone_slice(distance_lengths)?,
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

fn deft4j_pack_code_lengths(lengths: &[u8], options: Deft4jPackOptions) -> Option<Vec<RleToken>> {
    let mut output = Vec::new();
    output
        .try_reserve_exact(MAX_DYNAMIC_CODE_LENGTH_COUNT)
        .ok()?;
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
                while run >= 11 {
                    let count = run.min(138);
                    output.push(RleToken {
                        symbol: 18,
                        extra: (count - 11) as u8,
                    });
                    run -= count;
                }
            }
            if !options.no_zero_repeat {
                while run >= 3 {
                    let count = run.min(10);
                    output.push(RleToken {
                        symbol: 17,
                        extra: (count - 3) as u8,
                    });
                    run -= count;
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
    (output.len() <= MAX_DYNAMIC_CODE_LENGTH_COUNT).then_some(output)
}

#[cfg(test)]
mod tests;
