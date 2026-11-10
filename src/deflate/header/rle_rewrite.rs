// SPDX-License-Identifier: MIT
// SPDX-FileCopyrightText: 2026 Andrew C.E. Dent <https://github.com/ace-dent>

//! Local rewrites of a header search's RLE under a fixed code-length code:
//! DeflOpt's rewrite and rebuild loop, and deft4j's pruned header, which
//! expands repeats that cost no more than their explicit lengths and rebuilds
//! the code. Both spell out repeat codes and price each rewrite exactly.

use super::code_length_tree::{CodeLengthBuilder, CodeLengthTree, CodeLengthTrees};
use super::{
    consider_dynamic_header, consider_tree_header, reorder_code_length_lengths, rle_frequencies,
    rle_symbol_cost, CountedRle, HeaderPlanSearch, INF,
};
use crate::deflate::huffman::TreeBuildScratch;
use crate::deflate::model::{DynamicPlan, RleToken, MAX_DYNAMIC_CODE_LENGTH_COUNT};

/// Price a header using deft4j's code-length tree and equal-cost RLE prune,
/// then rebuild the code-length code. A tied rewrite can still shorten the
/// rebuilt header.
#[allow(clippy::too_many_arguments)]
pub(super) fn consider_deft4j_pruned_header(
    data_bits: u64,
    literal_lengths: &[u8],
    distance_lengths: &[u8],
    rle: &[RleToken],
    frequencies: &[u32; 19],
    deft4j: &CodeLengthTree,
    heap_scratch: &mut TreeBuildScratch,
    best: &mut Option<DynamicPlan>,
    [pruned, optimized]: &mut [Vec<RleToken>; 2],
) {
    // A used symbol without a code makes the cost infinite.
    if !deft4j.valid || deft4j.rle_bits() == INF {
        return;
    }

    let Some(frequencies) =
        rewrite_rle_deft4j_literals_into(rle, frequencies, &deft4j.lengths, true, pruned)
    else {
        return;
    };
    let rebuilt = CodeLengthTrees::new(&frequencies)
        .get_tree(CodeLengthBuilder::Deft4jJavaHeap, heap_scratch);
    if !rebuilt.valid || rebuilt.rle_bits() == INF {
        return;
    }
    let code_length_lengths = rebuilt.lengths;

    consider_tree_header(
        data_bits,
        literal_lengths,
        distance_lengths,
        pruned,
        &rebuilt,
        best,
    );

    // deft4j's final `optimiseHeader` step expands only strictly dearer
    // repeats under the rebuilt tree and keeps that tree unchanged.
    if let Some(optimized_frequencies) = rewrite_rle_deft4j_literals_into(
        pruned,
        &frequencies,
        &code_length_lengths,
        false,
        optimized,
    ) {
        consider_dynamic_header(
            data_bits,
            literal_lengths,
            distance_lengths,
            optimized,
            &optimized_frequencies,
            code_length_lengths,
            best,
        );
    }
}

/// Steps the DeflOpt local rewrite has taken from one RLE seed. Each heap
/// variant starts from the seed, and a step depends only on the current RLE
/// and the tree the variant builds there: the prices it offers, whether the
/// loop goes on, and the next RLE. A variant that builds an explored step's
/// tree at the same RLE would offer the same prices again, which cannot
/// change the best plan, so it moves straight to the recorded outcome.
#[derive(Default)]
pub(super) struct LocalRewriteSteps {
    /// RLEs reached from the seed and their symbol frequencies; state 0, the
    /// seed itself, is not stored. Only the first `reached` belong to this
    /// seed; the rest are buffers from earlier seeds, rewritten in place.
    states: Vec<CountedRle>,
    reached: usize,
    steps: Vec<(usize, [u8; 19], LocalRewriteOutcome)>,
}

impl LocalRewriteSteps {
    /// Start a new seed, keeping the buffers.
    pub(super) fn clear(&mut self) {
        self.reached = 0;
        self.steps.clear();
    }
}

#[derive(Clone, Copy)]
enum LocalRewriteOutcome {
    /// The tree gives no valid header.
    Invalid,
    Stop,
    Next(usize),
}

/// One step's result; a next RLE is in the buffer the step was given.
enum LocalRewriteStep {
    Invalid,
    Stop,
    Next([u32; 19]),
}

/// Price DeflOpt-style local rewrite/rebuild candidates from one RLE seed.
/// Each rewrite must strictly lower `repeat_rank`, which bounds the loop.
/// Returns the first tree that gave a valid header, for reuse by the caller.
#[allow(clippy::too_many_arguments)]
pub(super) fn consider_columbo_deflopt_local_rewrite(
    data_bits: u64,
    literal_lengths: &[u8],
    distance_lengths: &[u8],
    initial_rle: &[RleToken],
    initial_frequencies: &[u32; 19],
    variant: u32,
    steps: &mut LocalRewriteSteps,
    search: &mut HeaderPlanSearch,
) -> Option<[u8; 19]> {
    let mut state = 0;
    let mut initial_tree = None;

    loop {
        let frequencies = match state {
            0 => *initial_frequencies,
            _ => steps.states[state - 1].1,
        };
        let tree = CodeLengthTrees::new(&frequencies).get_tree(
            CodeLengthBuilder::DefloptHeap(variant),
            &mut search.heap_scratch,
        );
        if !tree.valid {
            return initial_tree;
        }
        let code_length_lengths = tree.lengths;

        let explored = steps
            .steps
            .iter()
            .find(|(explored_state, tree, _)| {
                *explored_state == state && *tree == code_length_lengths
            })
            .map(|&(_, _, outcome)| outcome);
        let outcome = match explored {
            Some(outcome) => outcome,
            None => {
                if steps.reached == steps.states.len() {
                    steps.states.push((Vec::new(), [0; 19]));
                }
                let (reached, spare) = steps.states.split_at_mut(steps.reached);
                let rle = match state {
                    0 => initial_rle,
                    _ => &reached[state - 1].0,
                };
                let next = &mut spare[0];
                let outcome = match local_rewrite_step(
                    data_bits,
                    literal_lengths,
                    distance_lengths,
                    rle,
                    &frequencies,
                    &tree,
                    search,
                    &mut next.0,
                ) {
                    LocalRewriteStep::Invalid => LocalRewriteOutcome::Invalid,
                    LocalRewriteStep::Stop => LocalRewriteOutcome::Stop,
                    LocalRewriteStep::Next(frequencies) => {
                        next.1 = frequencies;
                        steps.reached += 1;
                        LocalRewriteOutcome::Next(steps.reached)
                    }
                };
                steps.steps.push((state, code_length_lengths, outcome));
                outcome
            }
        };
        match outcome {
            LocalRewriteOutcome::Invalid => return initial_tree,
            LocalRewriteOutcome::Stop => {
                initial_tree.get_or_insert(code_length_lengths);
                return initial_tree;
            }
            LocalRewriteOutcome::Next(next) => {
                initial_tree.get_or_insert(code_length_lengths);
                state = next;
            }
        }
    }
}

/// One step of the DeflOpt local rewrite: price the RLE under the tree (and its
/// frequency-ordered form), then rewrite the RLE under the tree and price the
/// rewrite if it lowers `repeat_rank`. Returns the rewrite to continue from.
#[allow(clippy::too_many_arguments)]
fn local_rewrite_step(
    data_bits: u64,
    literal_lengths: &[u8],
    distance_lengths: &[u8],
    rle: &[RleToken],
    frequencies: &[u32; 19],
    tree: &CodeLengthTree,
    search: &mut HeaderPlanSearch,
    rewritten: &mut Vec<RleToken>,
) -> LocalRewriteStep {
    if consider_tree_header(
        data_bits,
        literal_lengths,
        distance_lengths,
        rle,
        tree,
        &mut search.best,
    )
    .is_none()
    {
        return LocalRewriteStep::Invalid;
    }
    let code_length_lengths = tree.lengths;

    // The rewrite loop follows the unmodified DeflOpt tree, which keeps tie
    // behavior deterministic. Lengths that follow the frequencies never
    // change.
    let mut reordered = code_length_lengths;
    if !tree.follows && reorder_code_length_lengths(&mut reordered, frequencies) {
        consider_dynamic_header(
            data_bits,
            literal_lengths,
            distance_lengths,
            rle,
            frequencies,
            reordered,
            &mut search.best,
        );
    }

    let rank = repeat_rank(frequencies);
    if rank == 0 {
        return LocalRewriteStep::Stop;
    }
    let Some(rewritten_frequencies) =
        rewrite_rle_deflopt_local_into(rle, frequencies, &code_length_lengths, rewritten)
    else {
        return LocalRewriteStep::Stop;
    };
    if repeat_rank(&rewritten_frequencies) >= rank {
        return LocalRewriteStep::Stop;
    }

    // Price the rewrite under the existing tree too: it is a distinct legal
    // header.
    consider_dynamic_header(
        data_bits,
        literal_lengths,
        distance_lengths,
        rewritten,
        &rewritten_frequencies,
        code_length_lengths,
        &mut search.best,
    );
    LocalRewriteStep::Next(rewritten_frequencies)
}

/// [`rewrite_rle_deflopt_local_into`] into a new buffer, returning the rewrite
/// with its symbol frequencies.
#[cfg(test)]
fn rewrite_rle_deflopt_local(
    input: &[RleToken],
    frequencies: &[u32; 19],
    code_length_lengths: &[u8; 19],
) -> Option<CountedRle> {
    let mut output = Vec::new();
    let frequencies =
        rewrite_rle_deflopt_local_into(input, frequencies, code_length_lengths, &mut output)?;
    Some((output, frequencies))
}

/// Expand a repeat code only when its explicit spelling is strictly cheaper
/// under the given tree. A code 17 of 3 to 6 zeros after a zero becomes code
/// 16 when code 16's length is no longer. `frequencies` are the input's symbol
/// frequencies. The rewrite goes to `output`, which this clears; returns its
/// symbol frequencies, the input's adjusted at each change, or `None` when
/// nothing changes, leaving `output` unspecified.
pub(super) fn rewrite_rle_deflopt_local_into(
    input: &[RleToken],
    frequencies: &[u32; 19],
    code_length_lengths: &[u8; 19],
    output: &mut Vec<RleToken>,
) -> Option<[u32; 19]> {
    debug_assert_eq!(*frequencies, rle_frequencies(input));
    // The last rewrite of a search usually changes nothing; this finds most
    // such rewrites without reading the RLE.
    if !some_repeat_expands(frequencies, code_length_lengths, false)
        && !(frequencies[17] != 0 && zero_repeat_takes_code_16(code_length_lengths))
    {
        return None;
    }
    // Built from the first change on: a rewrite that passes the check above
    // can still change nothing.
    output.clear();
    let mut started = false;
    // Input before this index is already in `output`; unchanged runs are
    // copied whole at the next change or the end.
    let mut copied = 0;
    let mut rewritten_frequencies = *frequencies;
    let mut previous = None;

    for (index, &token) in input.iter().enumerate() {
        // The replacement: one token, or `count` explicit copies of a value.
        let (replacement, repeated) = match token.symbol {
            value @ 0..=15 => {
                previous = Some(value);
                (None, None)
            }
            18 => {
                let count = usize::from(token.extra) + 11;
                previous = Some(0);
                let expand = explicit_rle_cost(code_length_lengths, 0, count)
                    < rle_symbol_cost(code_length_lengths, 18);
                (None, expand.then_some((0, count)))
            }
            17 => {
                let count = usize::from(token.extra) + 3;
                let follows_zero = previous == Some(0);
                previous = Some(0);
                if follows_zero
                    && token.extra <= 3
                    && zero_repeat_takes_code_16(code_length_lengths)
                {
                    let repeat = RleToken {
                        symbol: 16,
                        extra: token.extra,
                    };
                    (Some(repeat), None)
                } else {
                    let expand = explicit_rle_cost(code_length_lengths, 0, count)
                        < rle_symbol_cost(code_length_lengths, 17);
                    (None, expand.then_some((0, count)))
                }
            }
            16 => {
                let previous = previous?;
                let count = usize::from(token.extra) + 3;
                let expand = explicit_rle_cost(code_length_lengths, previous, count)
                    < rle_symbol_cost(code_length_lengths, 16);
                (None, expand.then_some((previous, count)))
            }
            _ => return None,
        };

        if replacement.is_none() && repeated.is_none() {
            continue;
        }
        if !started {
            // Expansions can outgrow the input, but not a header's lengths.
            output
                .try_reserve(input.len().max(MAX_DYNAMIC_CODE_LENGTH_COUNT))
                .ok()?;
            started = true;
        }
        output.extend_from_slice(&input[copied..index]);
        copied = index + 1;
        rewritten_frequencies[usize::from(token.symbol)] -= 1;
        if let Some((value, count)) = repeated {
            output.extend(
                std::iter::repeat(RleToken {
                    symbol: value,
                    extra: 0,
                })
                .take(count),
            );
            rewritten_frequencies[usize::from(value)] += count as u32;
        } else if let Some(replacement) = replacement {
            output.push(replacement);
            rewritten_frequencies[usize::from(replacement.symbol)] += 1;
        }
    }

    if !started {
        return None;
    }
    output.extend_from_slice(&input[copied..]);
    debug_assert_eq!(rewritten_frequencies, rle_frequencies(output));
    Some(rewritten_frequencies)
}

/// Whether spelling out some repeat code of an RLE with these symbol
/// frequencies costs less than the code under the tree, or with
/// `include_equal` no more; if not, a rewrite need not read the RLE. A repeat
/// spelled out costs more the longer it is, so each repeat code is checked at
/// its shortest count, and code 16 after every value it can follow.
fn some_repeat_expands(
    frequencies: &[u32; 19],
    code_length_lengths: &[u8; 19],
    include_equal: bool,
) -> bool {
    let expands = |value: u8, symbol: u8, shortest: usize| {
        let explicit = explicit_rle_cost(code_length_lengths, value, shortest);
        let repeat = rle_symbol_cost(code_length_lengths, symbol);
        explicit < repeat || (include_equal && explicit == repeat)
    };
    let zero_repeats = frequencies[17] != 0 || frequencies[18] != 0;
    (frequencies[18] != 0 && expands(0, 18, 11))
        || (frequencies[17] != 0 && expands(0, 17, 3))
        || (frequencies[16] != 0
            && (0..16).any(|value| {
                (frequencies[usize::from(value)] != 0 || (value == 0 && zero_repeats))
                    && expands(value, 16, 3)
            }))
}

/// Whether a code 17 of 3 to 6 zeros after a zero becomes code 16 in
/// [`rewrite_rle_deflopt_local_into`].
fn zero_repeat_takes_code_16(code_length_lengths: &[u8; 19]) -> bool {
    code_length_lengths[16] != 0
        && code_length_lengths[17] != 0
        && code_length_lengths[16] <= code_length_lengths[17]
}

/// Expand repeat codes that cost more than (or, with `include_equal`, the same
/// as) their explicit spelling under a fixed tree. `frequencies` are the
/// input's symbol frequencies. Returns `None` if nothing changes; otherwise
/// the rewrite and its symbol frequencies.
///
/// This mirrors deft4j's header-only prune: `include_equal` is used before a
/// tree rebuild, since a tied rewrite can change the next tree.
pub(super) fn rewrite_rle_deft4j_literals(
    input: &[RleToken],
    frequencies: &[u32; 19],
    code_length_lengths: &[u8; 19],
    include_equal: bool,
) -> Option<CountedRle> {
    let mut output = Vec::new();
    let frequencies = rewrite_rle_deft4j_literals_into(
        input,
        frequencies,
        code_length_lengths,
        include_equal,
        &mut output,
    )?;
    Some((output, frequencies))
}

/// [`rewrite_rle_deft4j_literals`] into `output`, which this clears, as
/// [`rewrite_rle_deflopt_local_into`] writes its rewrite.
fn rewrite_rle_deft4j_literals_into(
    input: &[RleToken],
    frequencies: &[u32; 19],
    code_length_lengths: &[u8; 19],
    include_equal: bool,
    output: &mut Vec<RleToken>,
) -> Option<[u32; 19]> {
    debug_assert_eq!(*frequencies, rle_frequencies(input));
    if !some_repeat_expands(frequencies, code_length_lengths, include_equal) {
        return None;
    }
    let mut rewritten_frequencies = *frequencies;
    // Built from the first change on, like the DeflOpt rewrite's, with
    // unchanged runs copied whole.
    output.clear();
    let mut started = false;
    let mut copied = 0;
    let mut previous = None;

    for (index, &token) in input.iter().enumerate() {
        let (value, count) = match token.symbol {
            value @ 0..=15 => {
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
            if !started {
                output.try_reserve(MAX_DYNAMIC_CODE_LENGTH_COUNT).ok()?;
                started = true;
            }
            output.extend_from_slice(&input[copied..index]);
            copied = index + 1;
            if output.len().checked_add(count)? > MAX_DYNAMIC_CODE_LENGTH_COUNT {
                return None;
            }
            output.extend(
                std::iter::repeat(RleToken {
                    symbol: value,
                    extra: 0,
                })
                .take(count),
            );
            rewritten_frequencies[usize::from(token.symbol)] -= 1;
            rewritten_frequencies[usize::from(value)] += count as u32;
        }
        previous = Some(value);
    }
    if !started {
        return None;
    }
    output.extend_from_slice(&input[copied..]);
    Some(rewritten_frequencies)
}

fn explicit_rle_cost(lengths: &[u8; 19], symbol: u8, count: usize) -> u64 {
    let length = lengths[usize::from(symbol)];
    if length == 0 {
        INF
    } else {
        u64::from(length).saturating_mul(count as u64).min(INF)
    }
}

/// Two for each code 17 and one for each code 16 or 18, from the RLE's symbol
/// frequencies.
fn repeat_rank(frequencies: &[u32; 19]) -> u64 {
    2 * u64::from(frequencies[17]) + u64::from(frequencies[16]) + u64::from(frequencies[18])
}

#[cfg(test)]
mod tests;
