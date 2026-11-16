// SPDX-License-Identifier: MIT
// SPDX-FileCopyrightText: 2026 Andrew C.E. Dent <https://github.com/ace-dent>

//! Generated Deflate streams shared by the parser and container tests.

use crate::deflate::bitstream::BitWriter;
use crate::deflate::block::{emit_block, plan_block};
use crate::deflate::distance_band::test_support::matched;
use crate::deflate::model::{count_frequencies, ParsedBlock, SourceBlockType, Token};
use crate::deflate::stop::SearchStop;
use crate::Options;

/// Tokens in each block of [`counting_stream`]; the reported stream averaged
/// 184,000.
const COUNTING_BLOCK_TOKENS: usize = 1 << 17;

/// The decimal integers `0..count` written end to end, and a Deflate stream
/// of them in dynamic blocks, returned as `(stream, decoded)`.
///
/// A reported 9.9 MB ZIP held `0..10_000_000` (68.9 MB decoded) in 109
/// dynamic blocks of 20.0 million tokens, almost one match and one literal for
/// each integer. These tokens take the same shape: an integer that shares at
/// least three leading digits with the one before copies them from it, at the
/// distance of its own length, and the rest of its digits are literals.
pub(crate) fn counting_stream(count: u32) -> (Vec<u8>, Vec<u8>) {
    let mut decoded = Vec::new();
    let mut tokens = Vec::new();
    let mut digits = vec![b'0'];
    for value in 0..count {
        let shared = if value == 0 {
            0
        } else {
            increment(&mut digits)
        };
        // Deflate's shortest match is three bytes.
        let copied = if shared >= 3 {
            tokens.push(matched(shared as u16, digits.len() as u16));
            shared
        } else {
            0
        };
        tokens.extend(digits[copied..].iter().copied().map(Token::Literal));
        decoded.extend_from_slice(&digits);
    }

    let mut writer = BitWriter::default();
    let mut start = 0_usize;
    let mut chunks = tokens.chunks(COUNTING_BLOCK_TOKENS).peekable();
    while let Some(chunk) = chunks.next() {
        let length = chunk
            .iter()
            .map(|token| match *token {
                Token::Literal(_) => 1,
                Token::Match { length, .. } => usize::from(length),
            })
            .sum::<usize>();
        let (literal_frequencies, distance_frequencies) = count_frequencies(chunk);
        let block = ParsedBlock {
            tokens: chunk.to_vec().into(),
            plain: decoded[start..start + length].to_vec().into(),
            literal_frequencies,
            distance_frequencies,
            original_literal_lengths: None,
            original_distance_lengths: None,
            original_dynamic: None,
            original: None,
            source_splits: Vec::new(),
            source_type: SourceBlockType::Dynamic,
        };
        let alignment = (writer.bit_position() % 8) as u8;
        let plan = plan_block(
            &block,
            alignment,
            &Options::default(),
            &mut SearchStop::never(),
        );
        emit_block(&mut writer, &[], &plan, chunks.peek().is_none()).unwrap();
        start += length;
    }
    (writer.into_bytes(), decoded)
}

/// Add one to the decimal `digits` and return how many leading digits are
/// unchanged.
fn increment(digits: &mut Vec<u8>) -> usize {
    for index in (0..digits.len()).rev() {
        if digits[index] < b'9' {
            digits[index] += 1;
            return index;
        }
        digits[index] = b'0';
    }
    digits.insert(0, b'1');
    0
}
