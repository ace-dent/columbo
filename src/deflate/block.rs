// SPDX-License-Identifier: MIT

//! Planning and emission for one structural Deflate block.

use crate::{Error, Options, Result};

use super::bitstream::BitWriter;
use super::header::{best_dynamic_plan, token_bits};
use super::huffman::{
    fixed_trees, Huffman, FIXED_DISTANCE_CODE_LENGTHS, FIXED_LITERAL_CODE_LENGTHS,
};
use super::model::{
    DynamicPlan, OriginalBits, ParsedBlock, PlannedBlock, Representation, SourceBlockType, Token,
    CODE_LENGTH_ORDER,
};

/// Return original block bits that remain safe at the requested alignment.
///
/// This answers only wire-format and compatibility questions, not whether the
/// original is the cheapest representation. Stored blocks contain alignment
/// padding, while strict dynamic originals must use complete Huffman codes.
pub(crate) fn reusable_original_bits(
    block: &ParsedBlock,
    alignment: u8,
    strict: bool,
) -> Option<OriginalBits> {
    let original = block.original?;
    let alignment_is_usable =
        original.block_type != SourceBlockType::Stored || original.alignment == alignment;
    let huffman_alphabets_are_usable = !strict
        || original.block_type != SourceBlockType::Dynamic
        || block
            .original_dynamic
            .as_ref()
            .is_some_and(DynamicPlan::has_strictly_compatible_huffman_codes);
    (alignment_is_usable && huffman_alphabets_are_usable).then_some(original)
}

pub(crate) fn plan_block(
    block: &ParsedBlock,
    alignment: u8,
    options: &Options,
    expired: impl FnMut() -> bool,
) -> PlannedBlock {
    let (representation, bits) = plan_representation(block, alignment, options, expired);

    PlannedBlock {
        tokens: block.tokens.clone(),
        plain: block.plain.clone(),
        representation,
        bits,
        source_type: block.source_type,
    }
}

/// Price an owned candidate without cloning its potentially large token and
/// decoded-byte vectors into the returned plan.
///
/// Structural search builds those vectors with fallible allocation. Moving
/// them here preserves that safety boundary; the ordinary borrowed planner
/// above remains convenient for persistent parsed blocks.
pub(crate) fn plan_owned_block(
    block: ParsedBlock,
    alignment: u8,
    options: &Options,
    expired: impl FnMut() -> bool,
) -> PlannedBlock {
    let (representation, bits) = plan_representation(&block, alignment, options, expired);
    PlannedBlock {
        tokens: block.tokens,
        plain: block.plain,
        representation,
        bits,
        source_type: block.source_type,
    }
}

fn plan_representation(
    block: &ParsedBlock,
    alignment: u8,
    options: &Options,
    expired: impl FnMut() -> bool,
) -> (Representation, u64) {
    let stored_bits = stored_block_bits(alignment, block.plain.len());
    let fixed_bits = fixed_block_bits(&block.tokens).unwrap_or(u64::MAX);
    let dynamic = best_dynamic_plan(
        &block.tokens,
        &block.literal_frequencies,
        &block.distance_frequencies,
        block.original_dynamic.as_ref(),
        options.strict,
        options.exhaustive,
        expired,
    );

    // Rewritten candidates intentionally use the original Columbo C tie order:
    // stored before fixed before dynamic. A usable exact-original candidate is
    // considered afterward and wins an equal-bit tie, avoiding pointless churn.
    let (mut representation, mut bits) = if stored_bits <= fixed_bits
        && dynamic
            .as_ref()
            .map_or(true, |candidate| stored_bits <= candidate.bits)
    {
        (Representation::Stored, stored_bits)
    } else if dynamic
        .as_ref()
        .map_or(true, |candidate| fixed_bits <= candidate.bits)
    {
        (Representation::Fixed, fixed_bits)
    } else {
        let dynamic = dynamic.expect("the dynamic branch requires a plan");
        let bits = dynamic.bits;
        (Representation::Dynamic(dynamic), bits)
    };

    if let Some(original) = reusable_original_bits(block, alignment, options.strict) {
        if original.len <= bits {
            representation = Representation::Original(original);
            bits = original.len;
        }
    }

    (representation, bits)
}

pub(crate) fn fixed_block_bits(tokens: &[Token]) -> Option<u64> {
    token_bits(
        tokens,
        &FIXED_LITERAL_CODE_LENGTHS,
        &FIXED_DISTANCE_CODE_LENGTHS,
    )?
    .checked_add(3)
}

pub(crate) fn stored_block_bits(mut alignment: u8, plain_size: usize) -> u64 {
    let mut remaining = plain_size;
    let mut bits = 0_u64;
    loop {
        let chunk = remaining.min(65_535);
        let after_header = (alignment + 3) & 7;
        let padding = if after_header == 0 {
            0
        } else {
            8 - after_header
        };
        bits += 3 + u64::from(padding) + 32 + (chunk as u64) * 8;
        remaining -= chunk;
        if remaining == 0 {
            break;
        }
        alignment = 0;
    }
    bits
}

pub(crate) fn emit_block(
    writer: &mut BitWriter,
    input: &[u8],
    block: &PlannedBlock,
    final_block: bool,
) -> Result<()> {
    match &block.representation {
        Representation::Original(original) => {
            writer.write(u32::from(final_block), 1)?;
            // BFINAL is the only bit whose meaning depends on the new stream
            // layout. Everything after it can be copied verbatim.
            for offset in 1..original.len {
                let position = original.start + offset;
                let byte = *input
                    .get((position / 8) as usize)
                    .ok_or_else(|| Error::new("original Deflate bit range is out of bounds"))?;
                writer.write(u32::from((byte >> (position & 7)) & 1), 1)?;
            }
            Ok(())
        }
        Representation::Stored => emit_stored(writer, final_block, &block.plain),
        Representation::Fixed => emit_fixed(writer, final_block, &block.tokens),
        Representation::Dynamic(dynamic) => {
            emit_dynamic(writer, final_block, &block.tokens, dynamic)
        }
    }
}

fn emit_stored(writer: &mut BitWriter, final_block: bool, plain: &[u8]) -> Result<()> {
    let mut offset = 0;
    loop {
        let chunk = (plain.len() - offset).min(65_535);
        let is_last = offset + chunk == plain.len();
        writer.write(u32::from(final_block && is_last), 1)?;
        writer.write(0, 2)?;
        writer.align_to_byte()?;
        let length = chunk as u16;
        writer.write(u32::from(length), 16)?;
        writer.write(u32::from(!length), 16)?;
        writer.write_aligned_bytes(&plain[offset..offset + chunk])?;
        offset += chunk;
        if is_last {
            break;
        }
    }
    Ok(())
}

fn emit_fixed(writer: &mut BitWriter, final_block: bool, tokens: &[Token]) -> Result<()> {
    let (literal, distance) = fixed_trees();
    writer.write(u32::from(final_block), 1)?;
    writer.write(1, 2)?;
    emit_tokens(writer, tokens, literal, distance)
}

fn emit_dynamic(
    writer: &mut BitWriter,
    final_block: bool,
    tokens: &[Token],
    plan: &DynamicPlan,
) -> Result<()> {
    let literal = Huffman::build(&plan.literal_lengths)
        .ok_or_else(|| Error::new("internal invalid literal/length plan"))?;
    let distance = Huffman::build(&plan.distance_lengths)
        .ok_or_else(|| Error::new("internal invalid distance plan"))?;
    let code_length = Huffman::build(&plan.code_length_lengths)
        .ok_or_else(|| Error::new("internal invalid code-length plan"))?;

    writer.write(u32::from(final_block), 1)?;
    writer.write(2, 2)?;
    writer.write((plan.hlit - 257) as u32, 5)?;
    writer.write((plan.hdist - 1) as u32, 5)?;
    writer.write((plan.hclen - 4) as u32, 4)?;
    for &symbol in &CODE_LENGTH_ORDER[..plan.hclen] {
        writer.write(u32::from(plan.code_length_lengths[symbol]), 3)?;
    }
    for rle in &plan.rle {
        emit_symbol(writer, &code_length, usize::from(rle.symbol))?;
        let extra_bits = match rle.symbol {
            16 => 2,
            17 => 3,
            18 => 7,
            _ => 0,
        };
        writer.write(u32::from(rle.extra), extra_bits)?;
    }
    emit_tokens(writer, tokens, &literal, &distance)
}

fn emit_tokens(
    writer: &mut BitWriter,
    tokens: &[Token],
    literal: &Huffman,
    distance: &Huffman,
) -> Result<()> {
    for token in tokens {
        match *token {
            Token::Literal(value) => emit_symbol(writer, literal, usize::from(value))?,
            Token::Match {
                length_symbol,
                distance_symbol,
                length_extra,
                distance_extra,
                length_extra_bits,
                distance_extra_bits,
                ..
            } => {
                emit_symbol(writer, literal, usize::from(length_symbol))?;
                writer.write(u32::from(length_extra), length_extra_bits)?;
                emit_symbol(writer, distance, usize::from(distance_symbol))?;
                writer.write(u32::from(distance_extra), distance_extra_bits)?;
            }
        }
    }
    emit_symbol(writer, literal, 256)
}

fn emit_symbol(writer: &mut BitWriter, tree: &Huffman, symbol: usize) -> Result<()> {
    let code = tree
        .code(symbol)
        .ok_or_else(|| Error::new("internal Huffman plan does not cover token"))?;
    writer.write(u32::from(code.code), code.length)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn block_with_original(
        block_type: SourceBlockType,
        alignment: u8,
        distance_lengths: Option<Vec<u8>>,
    ) -> ParsedBlock {
        let mut literal_lengths = vec![0; 257];
        literal_lengths[0] = 1;
        literal_lengths[256] = 1;
        let mut code_length_lengths = [0; 19];
        code_length_lengths[0] = 1;
        code_length_lengths[1] = 1;
        ParsedBlock {
            tokens: std::sync::Arc::new(Vec::new()),
            plain: std::sync::Arc::new(Vec::new()),
            literal_frequencies: [0; 286],
            distance_frequencies: [0; 30],
            original_literal_lengths: None,
            original_distance_lengths: None,
            original_dynamic: distance_lengths.map(|distance_lengths| DynamicPlan {
                literal_lengths,
                distance_lengths,
                code_length_lengths,
                rle: Vec::new(),
                hlit: 0,
                hdist: 0,
                hclen: 0,
                bits: 0,
            }),
            original: Some(OriginalBits {
                start: 0,
                len: 10,
                alignment,
                block_type,
            }),
            source_splits: Vec::new(),
            source_type: block_type,
        }
    }

    #[test]
    fn original_reuse_enforces_alignment_and_distance_policy() {
        let stored = block_with_original(SourceBlockType::Stored, 3, None);
        assert!(reusable_original_bits(&stored, 3, false).is_some());
        assert!(reusable_original_bits(&stored, 2, false).is_none());

        let fixed = block_with_original(SourceBlockType::Fixed, 3, None);
        assert!(reusable_original_bits(&fixed, 7, true).is_some());

        let no_dynamic_header = block_with_original(SourceBlockType::Dynamic, 0, None);
        assert!(reusable_original_bits(&no_dynamic_header, 0, false).is_some());
        assert!(reusable_original_bits(&no_dynamic_header, 0, true).is_none());

        let one_code = block_with_original(SourceBlockType::Dynamic, 0, Some(vec![1]));
        assert!(reusable_original_bits(&one_code, 0, true).is_none());
        let two_codes = block_with_original(SourceBlockType::Dynamic, 0, Some(vec![1, 1]));
        assert!(reusable_original_bits(&two_codes, 0, true).is_some());

        let mut singleton_literal =
            block_with_original(SourceBlockType::Dynamic, 0, Some(vec![1, 1]));
        let dynamic = singleton_literal.original_dynamic.as_mut().unwrap();
        dynamic.literal_lengths.fill(0);
        dynamic.literal_lengths[256] = 1;
        assert!(reusable_original_bits(&singleton_literal, 0, false).is_some());
        assert!(reusable_original_bits(&singleton_literal, 0, true).is_none());

        let mut reserved_only = vec![0; 32];
        reserved_only[30] = 1;
        reserved_only[31] = 1;
        let reserved_only = block_with_original(SourceBlockType::Dynamic, 0, Some(reserved_only));
        assert!(reusable_original_bits(&reserved_only, 0, true).is_none());
    }

    #[test]
    fn stored_cost_includes_alignment_and_chunks() {
        assert_eq!(stored_block_bits(0, 0), 40);
        assert_eq!(stored_block_bits(5, 0), 35);
        assert_eq!(
            stored_block_bits(0, 65_536),
            3 + 5 + 32 + 65_535 * 8 + 3 + 5 + 32 + 8
        );
    }

    #[test]
    fn empty_fixed_block_has_only_a_header_and_end_code() {
        assert_eq!(fixed_block_bits(&[]), Some(10));
    }
}
