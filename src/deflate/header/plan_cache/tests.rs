// SPDX-License-Identifier: MIT
// SPDX-FileCopyrightText: 2026 Andrew C.E. Dent <https://github.com/ace-dent>

use super::*;
use crate::deflate::header::test_support::best_dynamic_plan;
use crate::deflate::header::{
    plan_for_explicit_lengths_with_cost_cached, plan_for_trimmed_lengths,
    release_thread_header_plans, trim_distance, trim_literal, HEADER_PLAN_SEARCH,
};
use crate::deflate::huffman::make_lengths;
use crate::deflate::model::{count_frequencies, Token};
use crate::deflate::stop::SearchStop;

impl HeaderPlanCache {
    pub(super) fn with_limit(max_entries: usize) -> Self {
        Self {
            max_entries,
            ..Self::new()
        }
    }
}

#[test]
fn header_plan_cache_reuses_only_the_header_kernel() {
    let mut literal_lengths = [0_u8; 257];
    literal_lengths[0] = 1;
    literal_lengths[256] = 1;
    let distance_lengths = [1_u8];
    let mut cache = HeaderPlanCache::with_limit(2);

    let first = plan_for_explicit_lengths_with_cost_cached(
        &literal_lengths,
        &distance_lengths,
        10,
        false,
        &mut cache,
    )
    .expect("the first header kernel is valid");
    let second = plan_for_explicit_lengths_with_cost_cached(
        &literal_lengths,
        &distance_lengths,
        25,
        false,
        &mut cache,
    )
    .expect("the cached header kernel is valid");

    assert_eq!(second.bits, first.bits + 15);
    assert_eq!(
        cache.stats(),
        HeaderPlanCacheStats {
            lookups: 2,
            hits: 1,
            misses: 1,
            inserts: 1,
            collision_checks: 1,
            saturated: 0,
        }
    );

    let exhaustive = plan_for_explicit_lengths_with_cost_cached(
        &literal_lengths,
        &distance_lengths,
        25,
        true,
        &mut cache,
    )
    .expect("header policy produces an independent valid kernel");
    assert_eq!(exhaustive.bits, second.bits);
    assert_eq!(cache.stats().hits, 1);
    assert_eq!(cache.stats().misses, 2);
    assert_eq!(cache.stats().inserts, 2);
}

#[test]
fn saturated_header_cache_returns_exact_plans_without_retaining_misses() {
    let mut literal = [0_u8; 257];
    literal[0] = 1;
    literal[256] = 1;
    let distance = [1_u8];
    for limit in [0, 1] {
        let mut cache = HeaderPlanCache::with_limit(limit);
        for data_bits in [0, 10, 25] {
            let actual = cache.price(&literal, &distance, data_bits, false, 0xff);
            let expected =
                plan_for_trimmed_lengths_uncached(&literal, &distance, data_bits, false, 0xff);
            assert_eq!(actual, expected);
        }
        // An exhaustive miss has a different cache key even if its plan ties.
        let actual = cache.price(&literal, &distance, 17, true, 0xff);
        let expected = plan_for_trimmed_lengths_uncached(&literal, &distance, 17, true, 0xff);
        assert_eq!(actual, expected);
        assert_eq!(cache.entries.len(), limit);
        assert_eq!(cache.stats().inserts, limit);
        assert_eq!(cache.stats().hits, 2 * limit);
        assert_eq!(cache.stats().saturated, 4 - 3 * limit);
        let before = cache.stats();
        assert!(cache
            .price(&literal, &distance, u64::MAX, true, 0xff)
            .is_none());
        assert_eq!(cache.stats().inserts, before.inserts);
        assert_eq!(cache.stats().saturated, before.saturated);
    }
}

#[test]
fn header_plan_cache_verifies_lengths_after_a_hash_collision() {
    let mut first_literal = [0_u8; 257];
    first_literal[0] = 1;
    first_literal[256] = 1;
    let mut second_literal = [0_u8; 257];
    second_literal[0] = 2;
    second_literal[1] = 2;
    second_literal[2] = 2;
    second_literal[256] = 2;
    let distance = [1_u8];
    let mut cache = HeaderPlanCache::new();
    plan_for_explicit_lengths_with_cost_cached(&first_literal, &distance, 10, false, &mut cache)
        .expect("the first header kernel is valid");

    let fingerprint = header_plan_fingerprint(&second_literal, &distance, false, 0xff);
    cache.entries[0].fingerprint = fingerprint;
    cache.first_by_hash.clear();
    cache.first_by_hash.insert(fingerprint, 0);
    let second = plan_for_explicit_lengths_with_cost_cached(
        &second_literal,
        &distance,
        20,
        false,
        &mut cache,
    )
    .expect("the colliding header kernel is independently planned");

    assert_eq!(second.literal_lengths, second_literal);
    assert_eq!(cache.entries.len(), 2);
    assert_eq!(cache.entries[1].next_same_hash, Some(0));
    assert_eq!(cache.stats().hits, 0);
    assert_eq!(cache.stats().misses, 2);
    assert_eq!(cache.stats().collision_checks, 1);
}

/// Trimmed literal/length and distance code lengths from random Huffman trees,
/// with zero runs long enough for codes 17 and 18.
fn random_trimmed_lengths(next: &mut impl FnMut() -> u64) -> (Vec<u8>, Vec<u8>) {
    let mut literal_frequencies = [0_u32; 286];
    for (symbol, frequency) in literal_frequencies.iter_mut().enumerate() {
        let gap = (symbol / 8) % 3 == usize::try_from(next() % 3).unwrap();
        if !gap && next() % 3 != 0 {
            *frequency = u32::try_from(next() % 200).unwrap();
        }
    }
    literal_frequencies[256] = 1;
    let mut distance_frequencies = [0_u32; 30];
    for frequency in distance_frequencies
        .iter_mut()
        .take(1 + (next() % 30) as usize)
    {
        *frequency = u32::try_from(next() % 50).unwrap();
    }
    let literal = make_lengths(&literal_frequencies, 15, 0);
    let distance = make_lengths(&distance_frequencies, 15, 0);
    let literal = literal[..trim_literal(&literal)].to_vec();
    let distance = distance[..trim_distance(&distance)].to_vec();
    (literal, distance)
}

/// The packing before lengths were packed in pairs: one length at a time, then
/// each RLE token's two bytes.
fn packed_bytes_reference(plan: &DynamicPlan) -> Option<Vec<u8>> {
    let mut packed = Vec::new();
    let mut pending = None;
    for &length in plan.literal_lengths.iter().chain(&plan.distance_lengths) {
        if length > 15 {
            return None;
        }
        match pending.take() {
            None => pending = Some(length),
            Some(low) => packed.push(low | (length << 4)),
        }
    }
    packed.extend(pending);
    for token in &plan.rle {
        packed.extend([token.symbol, token.extra]);
    }
    Some(packed)
}

#[test]
fn packed_plans_match_length_by_length_packing() {
    let mut state = 0x3c6e_f372_fe94_f82b_u64;
    let mut next = move || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state
    };
    for case in 0..2_000 {
        // Odd and even counts on both sides, empty sides and 32 distances.
        let hlit = (next() % 290) as usize;
        let hdist = (next() % 34) as usize;
        let mut lengths: Vec<u8> = (0..hlit + hdist).map(|_| (next() % 16) as u8).collect();
        if case % 10 == 0 && !lengths.is_empty() {
            let at = (next() as usize) % lengths.len();
            lengths[at] = 16;
        }
        let rle = (0..next() % 80)
            .map(|_| RleToken {
                symbol: (next() % 19) as u8,
                extra: (next() % 128) as u8,
            })
            .collect();
        let plan = DynamicPlan {
            literal_lengths: lengths[..hlit].to_vec(),
            distance_lengths: lengths[hlit..].to_vec(),
            code_length_lengths: std::array::from_fn(|_| (next() % 8) as u8),
            rle,
            hlit,
            hdist,
            hclen: (next() % 16) as usize,
            bits: next() % 100_000,
        };
        let packed = PackedDynamicPlan::pack(&plan);
        assert_eq!(
            packed.as_ref().map(|packed| packed.packed.to_vec()),
            packed_bytes_reference(&plan),
            "hlit {hlit}, hdist {hdist}",
        );
        if let Some(packed) = packed {
            assert!(packed.matches(&plan));
            assert_eq!(packed.unpack(), Some(plan));
        }
    }
}

#[test]
fn stored_header_plans_round_trip_through_generation_turnover() {
    let mut state = 0x9e37_79b9_7f4a_7c15_u64;
    let mut next = move || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state
    };
    let cases: Vec<_> = (0..48).map(|_| random_trimmed_lengths(&mut next)).collect();
    let plans: Vec<_> = cases
        .iter()
        .map(|(literal, distance)| {
            plan_for_trimmed_lengths_uncached(literal, distance, 0, false, 0xff)
                .expect("random trees give a valid header")
        })
        .collect();
    // About six plans per generation, so lookups cross both generations and
    // evicted plans.
    let mut store = HeaderPlanStore::with_budget(8 << 10);
    let mut inserted = [false; 48];
    let (mut hits, mut evictions) = (0, 0);
    for round in 0..400 {
        let index = (next() % if round % 2 == 0 { 8 } else { 48 }) as usize;
        let (literal, distance) = &cases[index];
        let fingerprint = header_plan_fingerprint(literal, distance, false, 0xff);
        match store.get(fingerprint, literal, distance, false, 0xff) {
            Some(plan) => {
                assert_eq!(plan, plans[index]);
                hits += 1;
            }
            None => {
                evictions += usize::from(inserted[index]);
                inserted[index] = true;
                store.insert(fingerprint, false, 0xff, &plans[index]);
            }
        }
        assert!(store.current.charge() < store.max_bytes / 2);
    }
    assert!(
        hits > 0 && evictions > 0,
        "{hits} hits, {evictions} evictions"
    );
}

#[test]
fn stored_rle_tokens_round_trip_through_one_byte() {
    let mut encoded = 0;
    for symbol in 0..=u8::MAX {
        for extra in 0..=u8::MAX {
            let token = RleToken { symbol, extra };
            let valid = match symbol {
                0..=15 => extra == 0,
                16 => extra <= 3,
                17 => extra <= 7,
                18 => extra <= 127,
                _ => false,
            };
            match rle_token_byte(token) {
                Some(byte) => {
                    assert!(valid, "{token:?}");
                    assert_eq!(rle_token_from_byte(byte), token);
                    encoded += 1;
                }
                None => assert!(!valid, "{token:?}"),
            }
        }
    }
    assert_eq!(encoded, 16 + 4 + 8 + 128);
}

#[test]
fn stored_header_plan_rejects_a_fingerprint_collision() {
    let mut state = 0x2545_f491_4f6c_dd1d_u64;
    let mut next = move || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state
    };
    let (literal, distance) = random_trimmed_lengths(&mut next);
    let (other_literal, other_distance) = random_trimmed_lengths(&mut next);
    let plan = plan_for_trimmed_lengths_uncached(&literal, &distance, 0, false, 0xff).unwrap();
    for move_to_previous in [false, true] {
        let mut store = HeaderPlanStore::with_budget(usize::MAX);
        store.insert(7, false, 0xff, &plan);
        if move_to_previous {
            store.previous = std::mem::take(&mut store.current);
        }
        assert_eq!(
            store.get(7, &other_literal, &other_distance, false, 0xff),
            None
        );
        assert_eq!(store.get(7, &literal, &distance, true, 0xff), None);
        let expected = (!move_to_previous).then(|| plan.clone());
        // A rejected entry in the previous generation is dropped.
        assert_eq!(store.get(7, &literal, &distance, false, 0xff), expected);
    }
}

#[test]
fn thread_header_plans_match_uncached_planning() {
    let mut state = 0x6a09_e667_f3bc_c909_u64;
    let mut next = move || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state
    };
    release_thread_header_plans();
    for case in 0..40 {
        let (literal, distance) = random_trimmed_lengths(&mut next);
        let exhaustive = case % 8 == 0;
        let rle_mask = [0xff, 0x01, 0x11][case % 3];
        let data_bits = next() % 100_000;
        let expected =
            plan_for_trimmed_lengths_uncached(&literal, &distance, data_bits, exhaustive, rle_mask);
        for _ in 0..2 {
            let planned =
                plan_for_trimmed_lengths(&literal, &distance, data_bits, exhaustive, rle_mask);
            assert_eq!(planned, expected);
        }
    }
    release_thread_header_plans();
    assert!(HEADER_PLAN_STORE.with(|store| store.borrow().current.offsets.is_empty()));
    assert!(HEADER_PLAN_SEARCH.with(|search| search.borrow().is_none()));
}

#[test]
fn max_block_planning_keeps_more_header_plans_until_released() {
    let budget = || HEADER_PLAN_STORE.with(|store| store.borrow().max_bytes);
    let tokens: Vec<Token> = (0..40_u8).map(|byte| Token::Literal(byte % 7)).collect();
    let (literal, distance) = count_frequencies(&tokens);
    release_thread_header_plans();
    for (exhaustive, expected) in [
        (false, MAX_THREAD_HEADER_PLAN_BYTES),
        (true, MAX_THREAD_EXHAUSTIVE_HEADER_PLAN_BYTES),
        (false, MAX_THREAD_EXHAUSTIVE_HEADER_PLAN_BYTES),
    ] {
        best_dynamic_plan(
            &tokens,
            &literal,
            &distance,
            None,
            false,
            exhaustive,
            &mut SearchStop::never(),
        )
        .unwrap();
        assert_eq!(budget(), expected);
    }
    release_thread_header_plans();
    assert_eq!(budget(), MAX_THREAD_HEADER_PLAN_BYTES);
}
