// SPDX-License-Identifier: MIT
// SPDX-FileCopyrightText: 2026 Andrew C.E. Dent <https://github.com/ace-dent>

use super::*;
use crate::deflate::model::SourceBlockType;
use crate::deflate::stream::direct_structural_plan;
use crate::deflate::stream::test_support::literal_block;

#[test]
fn selected_plan_merge_cleanup_reuses_finished_token_spellings() {
    let options = Options::default();
    let blocks = [
        literal_block(&vec![b'a'; 800], SourceBlockType::Dynamic),
        literal_block(&vec![b'a'; 800], SourceBlockType::Dynamic),
    ];
    let mut plan_cache = CanonicalPlanCache::new();
    let selected = direct_structural_plan(&blocks, 0, &options, &mut plan_cache).unwrap();
    let merged = plan_selected_huffman_merge_floor(&selected, 0, &options, &mut plan_cache)
        .expect("the selected token spellings admit a cheaper adjacent merge");

    assert!(total_bits(&merged) < total_bits(&selected));
    assert_eq!(merged.len(), 1);
    assert_eq!(merged[0].plain.as_slice(), &[b'a'; 1_600]);
}

#[test]
fn selected_plan_boundary_reseat_finds_a_better_transition() {
    let options = Options {
        exhaustive: true,
        ..Options::default()
    };
    let mut left_bytes = vec![b'a'; 733];
    left_bytes.extend(std::iter::repeat(b'z').take(291));
    let blocks = [
        literal_block(&left_bytes, SourceBlockType::Dynamic),
        literal_block(&vec![b'z'; 1_024], SourceBlockType::Dynamic),
    ];
    let mut plan_cache = CanonicalPlanCache::new();
    let selected = direct_structural_plan(&blocks, 0, &options, &mut plan_cache).unwrap();
    let reseated = plan_selected_huffman_boundary_reseat(
        &selected,
        0,
        &options,
        &mut plan_cache,
        &mut SearchStop::never(),
    )
    .expect("the literal transition is a cheaper boundary");

    assert!(total_bits(&reseated) < total_bits(&selected));
    assert_eq!(reseated.len(), 2);
    assert_eq!(reseated[0].plain.len(), 733);
    let decoded: Vec<_> = reseated
        .iter()
        .flat_map(|plan| plan.plain.iter().copied())
        .collect();
    assert_eq!(decoded, [vec![b'a'; 733], vec![b'z'; 1_315]].concat());

    assert!(plan_selected_huffman_boundary_reseat(
        &selected,
        0,
        &Options::default(),
        &mut plan_cache,
        &mut SearchStop::never(),
    )
    .is_none());
    assert!(plan_selected_huffman_boundary_reseat(
        &selected,
        0,
        &options,
        &mut plan_cache,
        &mut SearchStop::always(),
    )
    .is_none());
}

#[test]
fn forced_split_escape_is_max_only_and_deadline_bounded() {
    let blocks = [
        literal_block(&vec![b'a'; 1_024], SourceBlockType::Dynamic),
        literal_block(&vec![b'z'; 1_024], SourceBlockType::Dynamic),
    ];
    let mut plan_cache = CanonicalPlanCache::new();
    let selected =
        direct_structural_plan(&blocks, 0, &Options::default(), &mut plan_cache).unwrap();

    assert!(plan_forced_split_boundary_escape(
        &selected,
        0,
        &Options::default(),
        &mut plan_cache,
        &mut SearchStop::never(),
    )
    .is_none());
    assert!(plan_forced_split_boundary_escape(
        &selected,
        0,
        &Options {
            exhaustive: true,
            ..Options::default()
        },
        &mut plan_cache,
        &mut SearchStop::always(),
    )
    .is_none());
}
