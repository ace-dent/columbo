// SPDX-License-Identifier: MIT
// SPDX-FileCopyrightText: 2026 Andrew C.E. Dent <https://github.com/ace-dent>

use super::*;
use crate::deflate::model::SourceBlockType;
use crate::deflate::stream::direct_structural_plan;
use crate::deflate::stream::test_support::literal_block;

#[test]
fn compact_split_coarse_to_fine_never_loses_the_unlimited_exact_route() {
    let options = Options {
        exhaustive: true,
        ..Options::default()
    };
    let mut first = vec![b'a'; 512];
    first.extend(std::iter::repeat(b'b').take(512));
    let mut second = vec![b'c'; 512];
    second.extend(std::iter::repeat(b'd').take(512));
    let blocks = [
        literal_block(&first, SourceBlockType::Dynamic),
        literal_block(&second, SourceBlockType::Dynamic),
    ];

    let exact = plan_compact_source_split_floor(&blocks, 0, &options).unwrap();
    let timed =
        plan_compact_source_split_floor_until(&blocks, 0, &options, &mut SearchStop::never())
            .unwrap();
    assert!(total_bits(&timed) <= total_bits(&exact));

    let stopped =
        plan_compact_source_split_floor_until(&blocks, 0, &options, &mut SearchStop::always())
            .expect("an expired route still forwards a complete structural parent");
    let decoded: Vec<_> = stopped
        .iter()
        .flat_map(|plan| plan.plain.iter().copied())
        .collect();
    assert_eq!(decoded, [first, second].concat());
}

#[test]
fn expired_compact_split_rescue_keeps_a_complete_parent_or_better() {
    let options = Options {
        exhaustive: true,
        ..Options::default()
    };
    let mut plain = vec![b'a'; 8_192];
    plain.extend((0..8_192).map(|index| (index % 251) as u8));
    let blocks = [literal_block(&plain, SourceBlockType::Dynamic)];
    let mut plan_cache = CanonicalPlanCache::new();
    let fallback = direct_structural_plan(&blocks, 0, &options, &mut plan_cache).unwrap();
    let rescued =
        plan_compact_source_split_floor_until(&blocks, 0, &options, &mut SearchStop::always())
            .expect("the bounded rescue retains a complete parent");

    assert!(total_bits(&rescued) <= total_bits(&fallback));
    let decoded: Vec<_> = rescued
        .iter()
        .flat_map(|plan| plan.plain.iter().copied())
        .collect();
    assert_eq!(decoded, plain);
}
