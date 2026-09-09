// SPDX-License-Identifier: MIT

use super::*;
use crate::deflate::model::count_frequencies;

fn literal_block(bytes: &[u8]) -> ParsedBlock {
    let tokens: Vec<_> = bytes.iter().copied().map(Token::Literal).collect();
    let (literal_frequencies, distance_frequencies) = count_frequencies(&tokens);
    ParsedBlock {
        tokens: Arc::new(tokens),
        plain: Arc::new(bytes.to_vec()),
        literal_frequencies,
        distance_frequencies,
        original_literal_lengths: None,
        original_distance_lengths: None,
        original_dynamic: None,
        original: None,
        source_splits: Vec::new(),
        source_type: SourceBlockType::Dynamic,
    }
}

fn assert_same_plan(left: &PlannedBlock, right: &PlannedBlock) {
    assert_eq!(left.bits, right.bits);
    assert_eq!(left.tokens, right.tokens);
    assert_eq!(left.plain, right.plain);
    assert_eq!(left.source_type, right.source_type);
    match (&left.representation, &right.representation) {
        (Representation::Original(left), Representation::Original(right)) => {
            assert_eq!(left, right);
        }
        (Representation::Stored, Representation::Stored)
        | (Representation::Fixed, Representation::Fixed) => {}
        (Representation::Dynamic(left), Representation::Dynamic(right)) => {
            assert_eq!(left, right);
        }
        pair => panic!("different representations: {pair:?}"),
    }
}

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

    let mut singleton_literal = block_with_original(SourceBlockType::Dynamic, 0, Some(vec![1, 1]));
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
fn reusable_huffman_plan_still_selects_stored_per_alignment() {
    let mut block = block_with_original(SourceBlockType::Dynamic, 0, None);
    block.plain = std::sync::Arc::new(vec![0; 26]);

    // A 26-byte stored block costs 243 bits with no padding and 250 bits
    // with seven padding bits. The reusable Huffman kernel must therefore
    // remain a candidate rather than blindly reusing one aligned winner.
    let (no_padding, no_padding_bits) = select_aligned_representation(
        block.plain.len(),
        reusable_original_bits(&block, 5, true),
        5,
        Representation::Fixed,
        244,
    );
    assert!(matches!(no_padding, Representation::Stored));
    assert_eq!(no_padding_bits, 243);

    let (seven_padding, seven_padding_bits) = select_aligned_representation(
        block.plain.len(),
        reusable_original_bits(&block, 6, true),
        6,
        Representation::Fixed,
        244,
    );
    assert!(matches!(seven_padding, Representation::Fixed));
    assert_eq!(seven_padding_bits, 244);
}

#[test]
fn canonical_cache_hits_for_equal_tokens_with_distinct_arcs() {
    let first = literal_block(b"canonical interval");
    let mut second = first.try_clone_shared().unwrap();
    second.tokens = Arc::new(first.tokens.as_ref().clone());
    second.plain = Arc::new(first.plain.as_ref().clone());
    assert!(!Arc::ptr_eq(&first.tokens, &second.tokens));

    let options = Options::default();
    let mut cache = CanonicalPlanCache::new();
    let expected = plan_block(&first, 3, &options, &mut SearchStop::never());
    let first_cached = plan_block_cached(&first, 3, &options, &mut cache);
    let second_cached = plan_block_cached(&second, 3, &options, &mut cache);

    assert_same_plan(&first_cached, &expected);
    assert_same_plan(&second_cached, &expected);
    assert_eq!(
        cache.stats(),
        CanonicalPlanCacheStats {
            lookups: 2,
            hits: 1,
            misses: 1,
            inserts: 1,
            collision_checks: 1,
            saturated: 0,
            retained_token_bytes: first.tokens.capacity() * std::mem::size_of::<Token>(),
        }
    );
}

#[test]
fn header_cache_reuses_trees_across_distinct_token_orders() {
    let first = literal_block(b"header kernel reuse");
    let mut reversed = b"header kernel reuse".to_vec();
    reversed.reverse();
    let second = literal_block(&reversed);
    assert_eq!(first.literal_frequencies, second.literal_frequencies);
    assert_ne!(first.tokens, second.tokens);

    let options = Options::default();
    let mut cache = CanonicalPlanCache::new();
    cache.plan_reusable_complete(&first, &options);
    let first_header_stats = cache.header_cache.stats();
    cache.plan_reusable_complete(&second, &options);
    let second_header_stats = cache.header_cache.stats();

    assert_eq!(cache.stats().hits, 0);
    assert_eq!(cache.stats().misses, 2);
    assert!(second_header_stats.hits > first_header_stats.hits);
    assert!(second_header_stats.inserts >= first_header_stats.inserts);
}

#[test]
fn canonical_cache_verifies_exact_state_after_a_hash_collision() {
    let first = literal_block(b"first collision state");
    let second = literal_block(b"second collision state");
    let options = Options::default();
    let mut cache = CanonicalPlanCache::new();
    cache.plan_reusable_complete(&first, &options);

    // Force the first entry into the second state's bucket. Production
    // hashes are only accelerators, so an exact-state mismatch must still
    // miss and append a separate entry to the collision chain.
    let collision = canonical_plan_fingerprint(&second, &options);
    cache.entries[0].fingerprint = collision;
    cache.first_by_hash.clear();
    cache.first_by_hash.insert(collision, 0);

    let expected = plan_block(&second, 5, &options, &mut SearchStop::never());
    let actual = plan_block_cached(&second, 5, &options, &mut cache);
    assert_same_plan(&actual, &expected);
    assert_eq!(cache.entries.len(), 2);
    assert_eq!(cache.entries[1].next_same_hash, Some(0));
    let stats = cache.stats();
    assert_eq!(stats.hits, 0);
    assert_eq!(stats.misses, 2);
    assert_eq!(stats.inserts, 2);
    assert_eq!(stats.collision_checks, 1);
}

#[test]
fn canonical_cache_isolates_policy_and_source_tree_seed() {
    let block = literal_block(b"policy");
    let relaxed = Options {
        strict: false,
        ..Options::default()
    };
    let exhaustive = Options {
        exhaustive: true,
        ..Options::default()
    };

    let mut cache = CanonicalPlanCache::new();
    cache.plan_reusable_complete(&block, &Options::default());
    cache.plan_reusable_complete(&block, &relaxed);
    cache.plan_reusable_complete(&block, &exhaustive);

    let mut seeded = block.try_clone_shared().unwrap();
    seeded.original_dynamic =
        block_with_original(SourceBlockType::Dynamic, 0, Some(vec![1, 1])).original_dynamic;
    cache.plan_reusable_complete(&seeded, &Options::default());

    let stats = cache.stats();
    assert_eq!(stats.hits, 0);
    assert_eq!(stats.misses, 4);
    assert_eq!(stats.inserts, 4);
}

#[test]
fn cached_kernel_layers_current_original_and_alignment() {
    let mut first = literal_block(b"same generated payload");
    first.source_type = SourceBlockType::Fixed;
    first.original = Some(OriginalBits {
        start: 11,
        len: 1,
        alignment: 0,
        block_type: SourceBlockType::Fixed,
    });
    let mut second = first.try_clone_shared().unwrap();
    second.source_type = SourceBlockType::Dynamic;
    second.original = Some(OriginalBits {
        start: 97,
        len: 2,
        alignment: 7,
        block_type: SourceBlockType::Fixed,
    });

    let options = Options {
        strict: false,
        ..Options::default()
    };
    let mut cache = CanonicalPlanCache::new();
    let first_plan = plan_block_cached(&first, 0, &options, &mut cache);
    let second_plan = plan_block_cached(&second, 5, &options, &mut cache);
    assert!(matches!(
        first_plan.representation,
        Representation::Original(OriginalBits { start: 11, .. })
    ));
    assert!(matches!(
        second_plan.representation,
        Representation::Original(OriginalBits { start: 97, .. })
    ));
    assert_eq!(second_plan.source_type, SourceBlockType::Dynamic);
    assert_eq!(cache.stats().hits, 1);

    let mut stored = second;
    stored.source_type = SourceBlockType::Stored;
    stored.original = Some(OriginalBits {
        start: 123,
        len: 1,
        alignment: 3,
        block_type: SourceBlockType::Stored,
    });
    let aligned = plan_block_cached(&stored, 3, &options, &mut cache);
    let unaligned = plan_block_cached(&stored, 2, &options, &mut cache);
    assert!(matches!(
        aligned.representation,
        Representation::Original(_)
    ));
    assert!(!matches!(
        unaligned.representation,
        Representation::Original(_)
    ));
}

#[test]
fn cache_saturation_recomputes_without_changing_the_plan() {
    let block = literal_block(b"not retained");
    let options = Options::default();
    let mut cache = CanonicalPlanCache::with_limits(1, 0);

    let expected = plan_block(&block, 4, &options, &mut SearchStop::never());
    let first = plan_block_cached(&block, 4, &options, &mut cache);
    let second = plan_block_cached(&block, 4, &options, &mut cache);
    assert_same_plan(&first, &expected);
    assert_same_plan(&second, &expected);

    let stats = cache.stats();
    assert_eq!(stats.hits, 0);
    assert_eq!(stats.misses, 2);
    assert_eq!(stats.inserts, 0);
    assert_eq!(stats.saturated, 2);
    assert_eq!(stats.retained_token_bytes, 0);
}

#[test]
fn empty_fixed_block_has_only_a_header_and_end_code() {
    assert_eq!(fixed_block_bits(&[]), Some(10));
}
