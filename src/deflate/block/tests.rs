// SPDX-License-Identifier: MIT
// SPDX-FileCopyrightText: 2026 Andrew C.E. Dent <https://github.com/ace-dent>

use super::*;
use crate::deflate::distance_band::test_support::matched;
use crate::deflate::model::count_frequencies;

fn literal_block(bytes: &[u8]) -> ParsedBlock {
    let tokens: Vec<_> = bytes.iter().copied().map(Token::Literal).collect();
    let mut block = counted_block(tokens);
    block.plain = Arc::new(bytes.to_vec());
    block
}

/// A block of `tokens` with their counts; planning does not read `plain`.
fn counted_block(tokens: Vec<Token>) -> ParsedBlock {
    let (literal_frequencies, distance_frequencies) = count_frequencies(&tokens);
    ParsedBlock {
        tokens: Arc::new(tokens),
        plain: Arc::new(Vec::new()),
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
    // with seven padding bits. The reusable Huffman plan must therefore stay
    // a candidate rather than reusing the winner from one alignment.
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
    // The thread's block plans would answer the reordered block first.
    release_thread_block_plans();
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

    // Force the first entry into the second block's chain; the mismatched block
    // must still miss and append a separate entry.
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

fn assert_same_reusable(left: &ReusableBlockPlan, right: &ReusableBlockPlan) {
    assert_eq!(left.bits, right.bits);
    match (&left.representation, &right.representation) {
        (Representation::Fixed, Representation::Fixed) => {}
        (Representation::Dynamic(left), Representation::Dynamic(right)) => {
            assert_eq!(left, right);
        }
        pair => panic!("different representations: {pair:?}"),
    }
}

/// Random literals and matches of a few lengths and distances.
fn random_tokens(next: &mut impl FnMut() -> u64) -> Vec<Token> {
    let count = 1 + (next() % 400) as usize;
    let alphabet = 2 + next() % 60;
    (0..count)
        .map(|_| {
            if next() % 3 == 0 {
                let length = 3 + (next() % 60) as u16;
                let distance = 1 + (next() % 3_000) as u16;
                matched(length, distance)
            } else {
                Token::Literal((next() % alphabet) as u8)
            }
        })
        .collect()
}

fn xorshift(seed: u64) -> impl FnMut() -> u64 {
    let mut state = seed;
    move || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state
    }
}

#[test]
fn stored_block_plans_match_uncached_planning_in_any_token_order() {
    let mut next = xorshift(0x243f_6a88_85a3_08d3);
    release_thread_block_plans();
    for case in 0..48 {
        let tokens = random_tokens(&mut next);
        let mut reversed = tokens.clone();
        reversed.reverse();
        let mut block = counted_block(tokens);
        let mut reordered = counted_block(reversed);
        if case % 4 == 3 {
            let original = plan_reusable_block_uncached(
                &block,
                &Options::default(),
                &mut SearchStop::never(),
                &mut HeaderPlanCache::new(),
            );
            if let Representation::Dynamic(original) = original.representation {
                block.original_dynamic = original.try_clone();
                reordered.original_dynamic = Some(original);
            }
        }
        let options = Options {
            strict: case % 3 != 1,
            exhaustive: case % 8 == 0,
            ..Options::default()
        };
        let expected = plan_reusable_block_uncached(
            &block,
            &options,
            &mut SearchStop::never(),
            &mut HeaderPlanCache::new(),
        );
        for planned in [&block, &block, &reordered] {
            let plan = plan_reusable_block(planned, &options, &mut SearchStop::never());
            assert_same_reusable(&plan, &expected);
        }
    }
    let stored = BLOCK_PLAN_STORE.with(|store| store.borrow().current.plans.len());
    assert!(stored > 0);
    release_thread_block_plans();
    assert!(BLOCK_PLAN_STORE.with(|store| store.borrow().current.plans.is_empty()));
}

#[test]
fn stored_block_plan_rejects_a_fingerprint_collision() {
    let mut next = xorshift(0x1319_8a2e_0370_7344);
    let block = counted_block(random_tokens(&mut next));
    let options = Options::default();
    let key = BlockPlanKey::new(&block, &options);
    let plan = plan_reusable_block_uncached(
        &block,
        &options,
        &mut SearchStop::never(),
        &mut HeaderPlanCache::new(),
    );

    let mut other_count = block.try_clone_shared().unwrap();
    other_count.literal_frequencies[256] += 1;
    let mut seeded = block.try_clone_shared().unwrap();
    seeded.original_dynamic = match &plan.representation {
        Representation::Dynamic(dynamic) => dynamic.try_clone(),
        _ => block_with_original(SourceBlockType::Dynamic, 0, Some(vec![1, 1])).original_dynamic,
    };
    let relaxed = Options {
        strict: false,
        ..Options::default()
    };

    let mut store = BlockPlanStore::with_budget(usize::MAX);
    store.insert(7, StoredBlockPlan::pack(&key, &block, &plan).unwrap());
    let forced = |block: &ParsedBlock, options: &Options| BlockPlanKey {
        fingerprint: 7,
        ..BlockPlanKey::new(block, options)
    };
    for (other, other_options) in [
        (&other_count, &options),
        (&seeded, &options),
        (&block, &relaxed),
    ] {
        assert!(store.get(&forced(other, other_options), other).is_none());
    }
    let stored = store.get(&forced(&block, &options), &block).unwrap();
    assert_same_reusable(&stored, &plan);
}

#[test]
fn reached_stops_neither_use_nor_fill_stored_block_plans() {
    let mut next = xorshift(0xa409_3822_299f_31d0);
    let block = counted_block(random_tokens(&mut next));
    let options = Options::default();
    release_thread_block_plans();

    let truncated = plan_reusable_block(&block, &options, &mut SearchStop::always());
    assert!(BLOCK_PLAN_STORE.with(|store| store.borrow().current.plans.is_empty()));

    let complete = plan_reusable_block(&block, &options, &mut SearchStop::never());
    assert_eq!(
        BLOCK_PLAN_STORE.with(|store| store.borrow().current.plans.len()),
        1
    );
    let expected = plan_reusable_block_uncached(
        &block,
        &options,
        &mut SearchStop::always(),
        &mut HeaderPlanCache::new(),
    );
    assert_same_reusable(&truncated, &expected);
    assert_same_reusable(
        &plan_reusable_block(&block, &options, &mut SearchStop::always()),
        &expected,
    );
    assert_same_reusable(
        &plan_reusable_block(&block, &options, &mut SearchStop::never()),
        &complete,
    );
    release_thread_block_plans();
}
