// SPDX-License-Identifier: MIT
// SPDX-FileCopyrightText: 2026 Andrew C.E. Dent <https://github.com/ace-dent>

//! Terminal header methods on a finished block that Default and Max both run:
//! strict distance completion (R1b), its relaxed counterpart (R19), the
//! payload/header tradeoff (R2) and the literal/length span (R3). Each keeps
//! the tokens and prices complete headers.

use super::{
    dynamic_bits, length_swap_delta, plan_for_trimmed_lengths_uncached, token_bits,
    transitions_removed, trim_distance, trim_literal,
};
use crate::deflate::model::{DynamicPlan, MAX_DYNAMIC_CODE_LENGTH_COUNT};
use crate::deflate::stop::SearchStop;

/// Uniform complete distance codes for a strict block that uses at most one
/// distance symbol.
///
/// Any complete code that covers the used symbol decodes the same payload. A
/// uniform code of depth 1 to 4 is one run of one code-length value; the
/// cheapest depth depends on the rest of the header and on that value's
/// position in the RFC 1951 code-length order.
fn strict_distance_completions(frequencies: &[u32; 30]) -> Vec<Vec<u8>> {
    let mut used = frequencies
        .iter()
        .enumerate()
        .filter_map(|(symbol, &frequency)| (frequency != 0).then_some(symbol));
    let only = match (used.next(), used.next()) {
        (None, _) => None,
        (Some(only), None) => Some(only),
        (Some(_), Some(_)) => return Vec::new(),
    };
    (1..=4_u8)
        .map(|depth| {
            let width = 1_usize << depth;
            let start = only.map_or(0, |symbol| (symbol + 1).saturating_sub(width));
            let mut lengths = vec![0_u8; frequencies.len()];
            lengths[start..start + width].fill(depth);
            lengths
        })
        .collect()
}

/// Distance codes for a relaxed block that uses at most one distance symbol:
/// every strict completion, then the forms RFC 1951 allows.
///
/// With one symbol in use, the RFC form is a 1-bit code on it. With none, the
/// RFC forms are one unused 1-bit code on symbol 0 and no codes at all. The
/// first sends code-length symbol 1 and the second symbol 0, and either may
/// already be in the header. A strict completion can still be the cheapest of
/// them, when its code-length value is the cheaper one; listed first, it also
/// wins a tie, which keeps the block strict at no cost.
fn relaxed_distance_codes(frequencies: &[u32; 30]) -> Vec<Vec<u8>> {
    let mut codes = strict_distance_completions(frequencies);
    if codes.is_empty() {
        return codes;
    }
    let only = frequencies.iter().position(|&frequency| frequency != 0);
    let mut single = vec![0_u8; frequencies.len()];
    single[only.unwrap_or(0)] = 1;
    codes.push(single);
    if only.is_none() {
        codes.push(vec![0_u8; frequencies.len()]);
    }
    codes
}

/// Most payload bits a trade-off swap (R2) may add to shorten the header.
const MAX_SWAP_PAYLOAD_TAX: i64 = 18;
const MAX_TAXED_SWAPS_PER_ALPHABET: usize = 32;

fn swap_transitions_removed(lengths: &[u8], a: usize, b: usize) -> i64 {
    transitions_removed(lengths, [a, b], [lengths[b], lengths[a]])
}

/// List up to `MAX_TAXED_SWAPS_PER_ALPHABET` swaps that add 1 to
/// `MAX_SWAP_PAYLOAD_TAX` payload bits and remove run transitions, ranked by
/// tax minus three bits per removed transition (a heuristic, not a bound).
fn payload_taxed_swap_proposals(
    lengths: &[u8],
    offset: usize,
    frequencies: &[u32],
    stop: &mut SearchStop<'_>,
) -> Option<Vec<(i64, i64, usize, usize)>> {
    let count = frequencies.len().min(lengths.len().checked_sub(offset)?);
    let mut proposals = Vec::new();
    proposals
        .try_reserve_exact(MAX_TAXED_SWAPS_PER_ALPHABET + 1)
        .ok()?;
    for a in 0..count {
        if stop.reached() {
            return None;
        }
        let length_a = lengths[offset + a];
        if length_a == 0 {
            continue;
        }
        for b in a + 1..count {
            let length_b = lengths[offset + b];
            if length_b == 0 || length_a == length_b {
                continue;
            }
            let tax = length_swap_delta(frequencies[a], frequencies[b], length_a, length_b);
            if !(1..=MAX_SWAP_PAYLOAD_TAX).contains(&tax) {
                continue;
            }
            let removed = swap_transitions_removed(lengths, offset + a, offset + b);
            if removed <= 0 {
                continue;
            }
            let proposal = (tax - 3 * removed, tax, a, b);
            let at = proposals.binary_search(&proposal).unwrap_or_else(|at| at);
            if at < MAX_TAXED_SWAPS_PER_ALPHABET {
                proposals.insert(at, proposal);
                proposals.truncate(MAX_TAXED_SWAPS_PER_ALPHABET);
            }
        }
    }
    Some(proposals)
}

/// Re-complete a finished strict block's degenerate distance code (R1b).
///
/// Planning completes an empty or singleton distance alphabet with two one-bit
/// codes. That is cheap when code-length symbol 1 is already in the header;
/// otherwise it can force 18 code-length code lengths to be sent. It runs
/// before the other terminal header methods, so they start from the cheaper
/// completion.
pub(crate) fn plan_strict_distance_completion(
    block: &crate::deflate::model::ParsedBlock,
    strict: bool,
    prices_left: &mut usize,
    stop: &mut SearchStop<'_>,
) -> Option<DynamicPlan> {
    if !strict {
        return None;
    }
    plan_degenerate_distance_code(block, true, prices_left, stop)
}

/// Choose a finished relaxed block's degenerate distance code (R19).
///
/// Planning gives an empty distance alphabet no codes and a singleton one
/// 1-bit code, and a block copied from the source keeps the source's code.
/// Which of [`relaxed_distance_codes`] is cheapest depends on the code-length
/// values the rest of the header already sends, so each is priced with the
/// block's literal/length code.
pub(crate) fn plan_relaxed_distance_code(
    block: &crate::deflate::model::ParsedBlock,
    strict: bool,
    prices_left: &mut usize,
    stop: &mut SearchStop<'_>,
) -> Option<DynamicPlan> {
    if strict {
        return None;
    }
    plan_degenerate_distance_code(block, false, prices_left, stop)
}

/// Reprice a finished block that uses at most one distance symbol with each
/// distance code its mode allows, keeping the literal/length code.
// Out of line: both modes' methods share the one copy.
#[inline(never)]
fn plan_degenerate_distance_code(
    block: &crate::deflate::model::ParsedBlock,
    strict: bool,
    prices_left: &mut usize,
    stop: &mut SearchStop<'_>,
) -> Option<DynamicPlan> {
    if *prices_left == 0 || stop.reached() {
        return None;
    }
    let original = block.original_dynamic.as_ref()?;
    let completions = if strict {
        strict_distance_completions(&block.distance_frequencies)
    } else {
        relaxed_distance_codes(&block.distance_frequencies)
    };
    if completions.is_empty() || (strict && !original.has_strictly_compatible_huffman_codes()) {
        return None;
    }
    let literal = &original.literal_lengths[..trim_literal(&original.literal_lengths)];
    let distance = &original.distance_lengths[..trim_distance(&original.distance_lengths)];
    let original_data_bits = token_bits(&block.tokens, literal, distance)?;
    let original_bits = dynamic_bits(original_data_bits, original)?;
    let mut best = None;
    let mut best_bits = original_bits;
    for completion in completions {
        if *prices_left == 0 || stop.reached() {
            break;
        }
        *prices_left -= 1;
        let completion = &completion[..trim_distance(&completion)];
        let Some(data_bits) = completed_distance_data_bits(
            original_data_bits,
            &block.distance_frequencies,
            distance,
            completion,
        ) else {
            continue;
        };
        if let Some(plan) =
            plan_for_trimmed_lengths_uncached(literal, completion, data_bits, true, 0xff)
        {
            if plan.bits < best_bits && (!strict || plan.has_strictly_compatible_huffman_codes()) {
                best_bits = plan.bits;
                best = Some(plan);
            }
        }
    }
    best
}

/// Reprice a block's payload after replacing a degenerate distance code: at
/// most one distance symbol is used, so only its length changes the payload.
fn completed_distance_data_bits(
    original_data_bits: u64,
    distance_frequencies: &[u32; 30],
    original: &[u8],
    completion: &[u8],
) -> Option<u64> {
    let Some(symbol) = distance_frequencies
        .iter()
        .position(|&frequency| frequency != 0)
    else {
        return Some(original_data_bits);
    };
    let frequency = u64::from(distance_frequencies[symbol]);
    let old = u64::from(*original.get(symbol)?);
    let new = u64::from(*completion.get(symbol)?);
    original_data_bits
        .checked_sub(frequency.checked_mul(old)?)?
        .checked_add(frequency.checked_mul(new)?)
}

/// Reprice the unchanged tree, then bounded swaps in each alphabet that add
/// payload bits but may shorten the header, each applied alone to the parent
/// (R2). Swapping two nonzero lengths preserves the set of used symbols, the
/// maximum depth and the Kraft sum.
pub(crate) fn plan_payload_header_tradeoff(
    block: &crate::deflate::model::ParsedBlock,
    strict: bool,
    prices_left: &mut usize,
    stop: &mut SearchStop<'_>,
) -> Option<DynamicPlan> {
    if *prices_left == 0 || stop.reached() {
        return None;
    }
    let original = block.original_dynamic.as_ref()?;
    if strict && !original.has_strictly_compatible_huffman_codes() {
        return None;
    }
    let literal = &original.literal_lengths[..trim_literal(&original.literal_lengths)];
    let distance = &original.distance_lengths[..trim_distance(&original.distance_lengths)];
    let data_bits = token_bits(&block.tokens, literal, distance)?;
    let original_bits = dynamic_bits(data_bits, original)?;
    let mut best = None;
    let mut best_bits = original_bits;
    *prices_left -= 1;
    if let Some(plan) = plan_for_trimmed_lengths_uncached(literal, distance, data_bits, true, 0xff)
    {
        if plan.bits < best_bits && (!strict || plan.has_strictly_compatible_huffman_codes()) {
            best_bits = plan.bits;
            best = Some(plan);
        }
    }
    let mut lengths = [0_u8; MAX_DYNAMIC_CODE_LENGTH_COUNT];
    let count = literal.len().checked_add(distance.len())?;
    let lengths = lengths.get_mut(..count)?;
    lengths[..literal.len()].copy_from_slice(literal);
    lengths[literal.len()..].copy_from_slice(distance);
    for (offset, frequencies) in [
        (0, &block.literal_frequencies[..literal.len()]),
        (literal.len(), block.distance_frequencies.as_slice()),
    ] {
        if *prices_left == 0 || stop.reached() {
            break;
        }
        let Some(proposals) = payload_taxed_swap_proposals(lengths, offset, frequencies, stop)
        else {
            break;
        };
        for (_, tax, a, b) in proposals {
            if *prices_left == 0 || stop.reached() {
                break;
            }
            lengths.swap(offset + a, offset + b);
            *prices_left -= 1;
            let plan = plan_for_trimmed_lengths_uncached(
                &lengths[..literal.len()],
                &lengths[literal.len()..],
                data_bits.checked_add(tax as u64)?,
                true,
                0xff,
            );
            lengths.swap(offset + a, offset + b);
            if let Some(plan) = plan {
                if plan.bits < best_bits
                    && (!strict || plan.has_strictly_compatible_huffman_codes())
                {
                    best_bits = plan.bits;
                    best = Some(plan);
                }
            }
        }
    }
    best
}

/// Try every legal HLIT from the trimmed minimum to 286, padding with zero
/// lengths (R3). The padding changes the zero run that continues into the
/// distance code lengths but no codeword. Each count gets a full header
/// search, since the existing code-length code alone can miss a gain.
pub(crate) fn plan_literal_span(
    block: &crate::deflate::model::ParsedBlock,
    strict: bool,
    prices_left: &mut usize,
    stop: &mut SearchStop<'_>,
) -> Option<DynamicPlan> {
    if *prices_left == 0 || stop.reached() {
        return None;
    }
    let original = block.original_dynamic.as_ref()?;
    if strict && !original.has_strictly_compatible_huffman_codes() {
        return None;
    }
    let minimum = trim_literal(&original.literal_lengths);
    let mut literal = [0_u8; 286];
    if minimum >= literal.len() {
        return None;
    }
    literal[..minimum].copy_from_slice(&original.literal_lengths[..minimum]);
    let distance = &original.distance_lengths;
    let data_bits = token_bits(&block.tokens, &literal[..minimum], distance)?;
    let mut best_bits = dynamic_bits(data_bits, original)?;
    let mut best = None;
    for span in minimum..=literal.len() {
        if *prices_left == 0 || stop.reached() {
            break;
        }
        if span == original.hlit {
            continue;
        }
        *prices_left -= 1;
        // The ordinary explicit-length wrapper would trim away the padding.
        if let Some(plan) =
            plan_for_trimmed_lengths_uncached(&literal[..span], distance, data_bits, true, 0xff)
        {
            if plan.bits < best_bits && (!strict || plan.has_strictly_compatible_huffman_codes()) {
                best_bits = plan.bits;
                best = Some(plan);
            }
        }
    }
    best
}

#[cfg(test)]
mod tests;
