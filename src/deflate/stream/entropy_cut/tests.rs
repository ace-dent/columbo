// SPDX-License-Identifier: MIT
// SPDX-FileCopyrightText: 2026 Andrew C.E. Dent <https://github.com/ace-dent>

use super::*;
use crate::deflate::bitstream::BitWriter;
use crate::deflate::block::emit_block;
use crate::deflate::parse::parse_stream;

fn literal_parent(plain: &[u8], strict: bool) -> ParsedBlock {
    let tokens: Vec<_> = plain.iter().copied().map(Token::Literal).collect();
    let (literal_frequencies, distance_frequencies) = count_frequencies(&tokens);
    let block = ParsedBlock {
        tokens: tokens.into(),
        plain: plain.to_vec().into(),
        literal_frequencies,
        distance_frequencies,
        original_literal_lengths: None,
        original_distance_lengths: None,
        original_dynamic: None,
        original: None,
        source_splits: Vec::new(),
        source_type: SourceBlockType::Dynamic,
    };
    let options = Options {
        strict,
        ..Options::default()
    };
    let plan = plan_block(&block, 0, &options, &mut SearchStop::never());
    let mut writer = BitWriter::default();
    emit_block(&mut writer, &[], &plan, true).unwrap();
    let data = writer.into_bytes();
    let mut parsed = parse_stream(&data, 1 << 20).unwrap();
    parsed.blocks.remove(0)
}

/// Three regimes with disjoint literal alphabets, one table each.
fn three_regime_plain() -> Vec<u8> {
    let mut plain: Vec<_> = (0..400).map(|i| (i % 2) as u8).collect();
    plain.extend((0..400).map(|i| 160 + (i % 16) as u8));
    plain.extend((0..400).map(|i| 40 + (i % 4) as u8));
    plain
}

fn xorshift(mut state: u64) -> impl FnMut() -> u64 {
    move || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state
    }
}

fn table() -> Vec<u64> {
    let mut table = Vec::new();
    weighted_log_table(&mut table).unwrap();
    table
}

#[test]
fn fixed_point_logarithm_matches_log2() {
    assert_eq!(log2_fixed(1), 0);
    for shift in 0..64 {
        assert_eq!(log2_fixed(1 << shift), shift << LOG_FRACTION_BITS);
    }
    let mut next = xorshift(0x9e37_79b9_7f4a_7c15);
    let mut previous = 0;
    for value in 1..100_000_u64 {
        let log = log2_fixed(value);
        assert!(log >= previous, "{value}");
        previous = log;
    }
    for _ in 0..10_000 {
        let value = (next() >> (next() % 64)).max(1);
        let exact = (value as f64).log2() * f64::from(1 << LOG_FRACTION_BITS);
        // Truncation and the squared mantissa's rounding stay within two units.
        let log = log2_fixed(value) as f64;
        assert!(log <= exact + 1e-6 && exact - log < 2.0, "{value}");
    }
}

/// The two sides' entropy recomputed from their histograms.
fn direct_entropy(table: &[u64], tokens: &[Token]) -> u64 {
    let mut literal = [0_u64; 286];
    let mut distance = [0_u64; 30];
    literal[256] = 1;
    for token in tokens {
        let (l, d) = token_symbols(token);
        literal[l] += 1;
        if let Some(d) = d {
            distance[d] += 1;
        }
    }
    let alphabet = |counts: &[u64]| {
        let total: u64 = counts.iter().sum();
        weighted_log(table, total) - counts.iter().map(|&c| weighted_log(table, c)).sum::<u64>()
    };
    alphabet(&literal) + alphabet(&distance)
}

fn random_tokens(next: &mut impl FnMut() -> u64, count: usize) -> Vec<Token> {
    (0..count)
        .map(|_| {
            // Mostly one literal, so counts pass the table's end.
            if next() % 4 != 0 {
                Token::Literal(if next() % 3 == 0 {
                    (next() % 256) as u8
                } else {
                    7
                })
            } else {
                let length_symbol = 257 + (next() % 29) as u16;
                let distance_symbol = (next() % 30) as u8;
                Token::Match {
                    length: 3,
                    distance: 1,
                    length_symbol,
                    distance_symbol,
                    length_extra: 0,
                    distance_extra: 0,
                    length_extra_bits: 0,
                    distance_extra_bits: 0,
                }
            }
        })
        .collect()
}

#[test]
fn moving_cut_entropy_matches_direct_histograms() {
    let table = table();
    let mut next = xorshift(0x2545_f491_4f6c_dd1d);
    let tokens = random_tokens(&mut next, 12_000);
    let mut left = Side::end_of_block(&table);
    let mut right = Side::end_of_block(&table);
    for token in &tokens {
        right.add(&table, token);
    }
    assert!(right
        .literal
        .iter()
        .any(|&count| count as usize > WEIGHTED_LOG_TABLE_LEN));
    for cut in 0..=tokens.len() {
        if cut % 97 == 0 || cut == tokens.len() {
            assert_eq!(left.entropy(&table), direct_entropy(&table, &tokens[..cut]));
            assert_eq!(
                right.entropy(&table),
                direct_entropy(&table, &tokens[cut..])
            );
        }
        if let Some(token) = tokens.get(cut) {
            left.add(&table, token);
            right.remove(&table, token);
        }
    }
}

#[test]
fn best_entropy_cut_finds_a_regime_change_and_ignores_one_regime() {
    let table = table();
    let regime = |range: std::ops::Range<u8>, count: usize| {
        range
            .cycle()
            .take(count)
            .map(Token::Literal)
            .collect::<Vec<_>>()
    };
    let mut tokens = regime(0..2, 300);
    tokens.extend(regime(200..216, 300));
    let mut budget = AlphabetBudget::with_limits(1 << 20, 16);
    let mut never = SearchStop::never();
    assert_eq!(
        best_entropy_cut(&tokens, 0, tokens.len(), &table, &mut budget, &mut never),
        Some(Some(300))
    );
    // A range is scanned from its own start.
    assert_eq!(
        best_entropy_cut(&tokens, 100, tokens.len(), &table, &mut budget, &mut never),
        Some(Some(300))
    );
    let uniform = regime(0..8, 600);
    assert_eq!(
        best_entropy_cut(&uniform, 0, uniform.len(), &table, &mut budget, &mut never),
        Some(None)
    );
    assert_eq!(
        best_entropy_cut(
            &tokens,
            0,
            2 * MIN_SIDE_TOKENS - 1,
            &table,
            &mut budget,
            &mut never
        ),
        Some(None)
    );
    let mut exhausted = AlphabetBudget::with_limits(2 * tokens.len() - 1, 16);
    assert_eq!(
        best_entropy_cut(&tokens, 0, tokens.len(), &table, &mut exhausted, &mut never),
        None
    );
    assert_eq!(
        best_entropy_cut(
            &tokens,
            0,
            tokens.len(),
            &table,
            &mut budget,
            &mut SearchStop::always()
        ),
        None
    );
}

#[test]
fn entropy_cuts_split_each_regime_into_its_own_block() {
    let plain = three_regime_plain();
    for strict in [false, true] {
        let block = literal_parent(&plain, strict);
        let options = Options {
            strict,
            ..Options::default()
        };
        let parent_bits = reusable_original_bits(&block, 0, strict).unwrap().len;
        let plans = plan_entropy_cuts(
            &block,
            0,
            &options,
            &mut EntropyCutBudget::for_stream(&[]),
            &mut SearchStop::never(),
        )
        .unwrap();
        // The second cut comes from cutting a kept half again.
        let ends: Vec<_> = plans
            .iter()
            .scan(0, |end, plan| {
                *end += plan.plain.len();
                Some(*end)
            })
            .collect();
        assert_eq!(ends, [400, 800, 1200]);
        assert!(total_bits(&plans) < parent_bits);
        let decoded: Vec<u8> = plans
            .iter()
            .flat_map(|plan| plan.plain.iter().copied())
            .collect();
        assert_eq!(decoded, plain);
        if strict {
            for plan in &plans {
                if let Representation::Dynamic(dynamic) = &plan.representation {
                    assert!(dynamic.has_strictly_compatible_huffman_codes());
                }
            }
        }
        assert!(plan_entropy_cuts(
            &block,
            0,
            &options,
            &mut EntropyCutBudget::for_stream(&[]),
            &mut SearchStop::always(),
        )
        .is_none());
    }
}

#[test]
fn entropy_cuts_keep_completed_cuts_when_prices_run_out() {
    let block = literal_parent(&three_regime_plain(), true);
    let options = Options::default();
    // Room for the first cut's two ranges only.
    let mut budget = EntropyCutBudget::for_stream(&[]);
    budget.ranges = AlphabetBudget::with_limits(1 << 28, 2);
    let plans =
        plan_entropy_cuts(&block, 0, &options, &mut budget, &mut SearchStop::never()).unwrap();
    assert_eq!(plans.len(), 2);
    assert!(total_bits(&plans) < reusable_original_bits(&block, 0, true).unwrap().len);
}

#[test]
fn entropy_cuts_leave_a_single_regime_block_alone() {
    let plain: Vec<u8> = (0..1200).map(|i| (i % 8) as u8).collect();
    let block = literal_parent(&plain, true);
    assert!(plan_entropy_cuts(
        &block,
        0,
        &Options::default(),
        &mut EntropyCutBudget::for_stream(&[]),
        &mut SearchStop::never(),
    )
    .is_none());
}
