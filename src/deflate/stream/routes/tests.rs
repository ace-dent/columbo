// SPDX-License-Identifier: MIT
// SPDX-FileCopyrightText: 2026 Andrew C.E. Dent <https://github.com/ace-dent>

use super::*;
use crate::deflate::model::OriginalBits;
use crate::deflate::stream::test_support::literal_block;

#[test]
fn fragmented_collection_uses_the_independent_4096_token_limit() {
    let mut source_start = 0_u64;
    let blocks: Vec<_> = (0..FRAGMENTED_COLLECT_MIN_SOURCE_BLOCKS)
        .map(|index| {
            let mut block = literal_block(&[index as u8; 100], SourceBlockType::Dynamic);
            block.original = Some(OriginalBits {
                start: source_start,
                len: 2_048,
                alignment: (source_start & 7) as u8,
                block_type: SourceBlockType::Dynamic,
            });
            source_start += 2_048;
            block
        })
        .collect();
    let options = Options {
        exhaustive: true,
        ..Options::default()
    };

    let seed = fragmented_collect_seed(&blocks, 16_384, 0, &options).unwrap();
    assert_eq!(seed.len(), 2);
    assert_eq!(seed[0].tokens.len(), 4_000);
    assert_eq!(seed[1].tokens.len(), 2_400);
    let decoded: Vec<_> = seed
        .iter()
        .flat_map(|plan| plan.plain.iter().copied())
        .collect();
    let source: Vec<_> = blocks
        .iter()
        .flat_map(|block| block.plain.iter().copied())
        .collect();
    assert_eq!(decoded, source);
}

#[test]
fn encoded_source_size_survives_strict_alias_normalization() {
    // 64 source blocks of 2,008 bits encode 16,064 bytes, just inside the
    // regroup window; the stream adds 5 bytes of dropped empty blocks.
    let mut source_start = 0_u64;
    let mut blocks: Vec<_> = (0..FRAGMENTED_COLLECT_MIN_SOURCE_BLOCKS)
        .map(|index| {
            let mut block = literal_block(&[index as u8; 100], SourceBlockType::Dynamic);
            block.original = Some(OriginalBits {
                start: source_start,
                len: 2_008,
                alignment: (source_start & 7) as u8,
                block_type: SourceBlockType::Dynamic,
            });
            source_start += 2_008;
            block
        })
        .collect();
    let stream_bytes = 16_069;
    // While every block keeps its source bits, they alone decide.
    assert_eq!(encoded_source_bytes(&blocks, stream_bytes), 16_064);

    // Strict length-258 normalization clears a block's source bits. The rest
    // encode 15,813 bytes, below the window, so the stream length stands in.
    blocks[0].original = None;
    assert_eq!(encoded_source_bytes(&blocks, stream_bytes), 16_069);
    let options = Options {
        exhaustive: true,
        ..Options::default()
    };
    assert!(fragmented_collect_seed(&blocks, stream_bytes, 0, &options).is_some());
}

#[test]
fn fragmented_replay_is_bounded_and_preserves_decoded_bytes() {
    let blocks: Vec<_> = (0..4)
        .map(|_| literal_block(&vec![b'a'; 512], SourceBlockType::Dynamic))
        .collect();
    let mut plan_cache = CanonicalPlanCache::new();
    let mandatory =
        mandatory_token_floor_plan(&blocks, 0, &Options::default(), &mut plan_cache).unwrap();
    let plans = plan_fragmented_replay(&blocks, 0, &Options::default()).unwrap();

    assert_eq!(plans.len(), 1);
    assert!(total_bits(&plans) < total_bits(&mandatory));
    let decoded: Vec<_> = plans
        .iter()
        .flat_map(|plan| plan.plain.iter().copied())
        .collect();
    let source: Vec<_> = blocks
        .iter()
        .flat_map(|block| block.plain.iter().copied())
        .collect();
    assert_eq!(decoded, source);

    let too_many = vec![blocks[0].clone(); MAX_FRAGMENTED_REPLAY_BLOCKS + 1];
    assert!(plan_fragmented_replay(&too_many, 0, &Options::default()).is_none());
}

#[test]
fn direct_no_split_route_is_additive_and_preserves_decoded_bytes() {
    let options = Options {
        exhaustive: true,
        ..Options::default()
    };
    let blocks = [
        literal_block(&vec![b'a'; 800], SourceBlockType::Dynamic),
        literal_block(&vec![b'a'; 800], SourceBlockType::Dynamic),
        literal_block(&vec![b'a'; 800], SourceBlockType::Dynamic),
    ];
    let mut plan_cache = CanonicalPlanCache::new();
    let fallback = direct_structural_plan(&blocks, 0, &options, &mut plan_cache).unwrap();
    let route = plan_source_no_split_route(&blocks, 0, &options, &mut SearchStop::never())
        .expect("the direct route retains a complete fallback");

    assert!(total_bits(&route) <= total_bits(&fallback));
    let decoded: Vec<_> = route
        .iter()
        .flat_map(|plan| plan.plain.iter().copied())
        .collect();
    assert_eq!(decoded, vec![b'a'; 2_400]);
}

#[test]
fn terminal_merge_returns_only_a_strict_complete_stream_win() {
    let options = Options::default();
    let blocks = [
        literal_block(&vec![b'a'; 800], SourceBlockType::Dynamic),
        literal_block(&vec![b'a'; 800], SourceBlockType::Dynamic),
    ];
    let mut plan_cache = CanonicalPlanCache::new();
    let fallback = direct_structural_plan(&blocks, 0, &options, &mut plan_cache).unwrap();
    let merged = plan_terminal_merge_route(&blocks, 0, &options, &mut SearchStop::never())
        .expect("equal neighbors have a profitable deterministic merge");

    assert!(total_bits(&merged) < total_bits(&fallback));
    assert_eq!(merged.len(), 1);
    assert_eq!(merged[0].plain.as_slice(), &[b'a'; 1_600]);

    assert!(plan_terminal_merge_route(&blocks, 0, &options, &mut SearchStop::always()).is_none());
}
