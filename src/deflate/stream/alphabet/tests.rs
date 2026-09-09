// SPDX-License-Identifier: MIT

use super::*;
use crate::deflate::bitstream::BitWriter;
use crate::deflate::block::emit_block;
use crate::deflate::parse::parse_stream;

fn literal_parent(plain: &[u8]) -> (Vec<u8>, ParsedBlock) {
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
    let plan = plan_block(&block, 0, &Options::default(), &mut SearchStop::never());
    let mut writer = BitWriter::default();
    emit_block(&mut writer, &[], &plan, true).unwrap();
    let data = writer.into_bytes();
    let mut parsed = parse_stream(&data, 1 << 20).unwrap();
    (data, parsed.blocks.remove(0))
}

fn coupled_plain() -> Vec<u8> {
    let mut plain: Vec<_> = (0..32).map(|i| (i % 2) as u8).collect();
    plain.extend((0..256).map(|i| 160 + (i % 16) as u8));
    plain.extend((0..35).map(|i| (i % 2) as u8));
    plain
}

#[test]
fn paired_cuts_can_win_when_either_cut_alone_loses() {
    let plain = coupled_plain();
    let (raw, block) = literal_parent(&plain);
    let composite = Composite::new(std::slice::from_ref(&block)).unwrap();
    assert!(support_intervals(&composite, &mut SearchStop::never())
        .unwrap()
        .contains(&(32, 288)));
    let mut budget = AlphabetBudget::new();
    let mut cache = EdgeCache {
        edges: HashMap::new(),
        headers: HeaderPlanCache::new(),
        budget: &mut budget,
    };
    let options = Options::default();
    let pair = price_pair(
        &block,
        &composite,
        (32, 288),
        0,
        &options,
        &mut cache,
        &mut SearchStop::never(),
    )
    .unwrap();
    let left = price_pair(
        &block,
        &composite,
        (0, 32),
        0,
        &options,
        &mut cache,
        &mut SearchStop::never(),
    )
    .unwrap();
    let right = price_pair(
        &block,
        &composite,
        (0, 288),
        0,
        &options,
        &mut cache,
        &mut SearchStop::never(),
    )
    .unwrap();
    assert_eq!(block.original.unwrap().len, 1508);
    assert_eq!(
        (total_bits(&pair), total_bits(&left), total_bits(&right)),
        (1464, 1527, 1513)
    );
    let selected = plan_alphabet_boundaries(
        &block,
        0,
        &options,
        &mut AlphabetBudget::new(),
        &mut SearchStop::never(),
    )
    .unwrap();
    assert!(total_bits(&selected) <= 1464);
    let mut writer = BitWriter::default();
    for (i, plan) in selected.iter().enumerate() {
        emit_block(&mut writer, &raw, plan, i + 1 == selected.len()).unwrap();
    }
    assert_eq!(writer.bit_position(), total_bits(&selected));
    let parsed = parse_stream(&writer.into_bytes(), 1024).unwrap();
    assert_eq!(
        parsed
            .blocks
            .iter()
            .flat_map(|b| b.plain.iter().copied())
            .collect::<Vec<_>>(),
        plain
    );
    assert!(parsed
        .blocks
        .iter()
        .flat_map(|b| b.tokens.iter())
        .all(|t| matches!(t, Token::Literal(_))));
}

#[test]
fn a_complete_pair_survives_price_exhaustion_and_a_mid_search_stop() {
    let (_, block) = literal_parent(&coupled_plain());
    let mut budget = AlphabetBudget {
        work_left: 1 << 26,
        prices_left: 3,
    };
    let mut polls = 0;
    let mut count_polls = || {
        polls += 1;
        false
    };
    let plans = plan_alphabet_boundaries(
        &block,
        0,
        &Options::default(),
        &mut budget,
        &mut SearchStop::callback(&mut count_polls),
    )
    .unwrap();
    // Repeat the same prefix with enough budget, then stop at the poll
    // where the first run ran out of prices. Its completed pair survives.
    let mut stopped_budget = AlphabetBudget::new();
    let mut stopped_polls = 0;
    let mut stop_after_prefix = || {
        stopped_polls += 1;
        stopped_polls >= polls
    };
    let stopped = plan_alphabet_boundaries(
        &block,
        0,
        &Options::default(),
        &mut stopped_budget,
        &mut SearchStop::callback(&mut stop_after_prefix),
    )
    .unwrap();
    assert_eq!(total_bits(&stopped), total_bits(&plans));
    assert!(stopped_budget.prices_left > 0);
    assert!(total_bits(&plans) < block.original.unwrap().len);
    assert_eq!(budget.prices_left, 0);
    assert!(plan_alphabet_boundaries(
        &block,
        0,
        &Options::default(),
        &mut budget,
        &mut SearchStop::never()
    )
    .is_none());
    let mut budget = AlphabetBudget::new();
    assert!(plan_alphabet_boundaries(
        &block,
        0,
        &Options::default(),
        &mut budget,
        &mut SearchStop::always()
    )
    .is_none());
    assert_eq!(budget.work_left, 1 << 26);
    let mut exhausted = AlphabetBudget {
        work_left: 0,
        prices_left: 4096,
    };
    assert!(plan_alphabet_boundaries(
        &block,
        0,
        &Options::default(),
        &mut exhausted,
        &mut SearchStop::never()
    )
    .is_none());
}

#[test]
fn boundary_graph_matches_every_partition_at_all_alignments() {
    let mut plain = vec![0; 49];
    plain.extend(0..=255);
    plain.extend(vec![1; 85]);
    let (_, block) = literal_parent(&plain);
    let composite = Composite::new(std::slice::from_ref(&block)).unwrap();
    let cuts: Vec<_> = [0, 17, 49, 305, 333, 390]
        .into_iter()
        .map(|i| cut(&composite, i))
        .collect();
    let options = Options::default();
    let mut budget = AlphabetBudget::new();
    let mut cache = EdgeCache {
        edges: HashMap::new(),
        headers: HeaderPlanCache::new(),
        budget: &mut budget,
    };
    let mut saw_stored = false;
    for alignment in 0..8 {
        let mut graph = BoundaryGraph::new(cuts.len(), alignment).unwrap();
        for start in 0..cuts.len() - 1 {
            for end in start + 1..cuts.len() {
                let edge = cache
                    .get(
                        &block,
                        &composite,
                        cuts[start],
                        cuts[end],
                        &options,
                        &mut SearchStop::never(),
                    )
                    .unwrap();
                graph
                    .consider(start, end, edge, &mut SearchStop::never())
                    .unwrap();
            }
        }
        let plans = graph.resolve(&composite, &cuts).unwrap();
        let mut minimum = u64::MAX;
        // Independently enumerate every partition, repricing its ranges
        // directly instead of consulting graph states or cached edges.
        for mask in 0..1 << (cuts.len() - 2) {
            let mut bits = 0;
            let mut from = 0;
            for to in 1..cuts.len() {
                if to + 1 != cuts.len() && mask & (1 << (to - 1)) == 0 {
                    continue;
                }
                let range = make_range(&composite, cuts[from], cuts[to]).unwrap();
                let plan = plan_block(
                    &range,
                    ((u64::from(alignment) + bits) & 7) as u8,
                    &options,
                    &mut SearchStop::never(),
                );
                saw_stored |= matches!(plan.representation, Representation::Stored);
                bits += plan.bits;
                from = to;
            }
            minimum = minimum.min(bits);
        }
        assert_eq!(total_bits(&plans), minimum);
    }
    assert!(cache.headers.stats().hits > 0);
    assert!(
        saw_stored,
        "the oracle must exercise alignment-dependent stored edges"
    );
}
