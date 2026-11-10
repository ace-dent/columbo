// SPDX-License-Identifier: MIT
// SPDX-FileCopyrightText: 2026 Andrew C.E. Dent <https://github.com/ace-dent>

use super::*;
use crate::deflate::model::SourceBlockType;
use crate::deflate::stream::test_support::literal_block;

/// Return only the best split from `coarse_to_fine_split_search`.
fn coarse_to_fine_split<S>(
    start: usize,
    end: usize,
    score: &mut S,
    stop: &mut SearchStop<'_>,
) -> Option<AdaptiveSplit>
where
    S: FnMut(usize) -> Option<u64>,
{
    Some(coarse_to_fine_split_search(start, end, score, stop)?.best)
}

#[test]
fn coarse_to_fine_split_finds_an_off_grid_minimum_under_its_probe_cap() {
    let mut probes = 0_usize;
    let mut score = |token: usize| {
        probes += 1;
        let distance = token.abs_diff(733) as u64;
        Some(distance * distance)
    };
    let candidate =
        coarse_to_fine_split(0, 2_048, &mut score, &mut SearchStop::never()).expect("a legal cut");

    assert_eq!(candidate.token, 733);
    assert_eq!(candidate.bits, 0);
    assert!(probes <= ADAPTIVE_SPLIT_MAX_PROBES);
}

#[test]
fn coarse_to_fine_split_centers_flat_ties() {
    let candidate = coarse_to_fine_split(0, 2_048, &mut |_| Some(1), &mut SearchStop::never())
        .expect("a legal cut");

    assert_eq!(candidate.token, 1_024);
}

#[test]
fn coarse_to_fine_split_retains_one_well_separated_secondary_basin() {
    let mut score = |token: usize| {
        let primary = token.abs_diff(420) as u64;
        let secondary = token.abs_diff(1_600) as u64;
        Some((primary * primary).min(100 + secondary * secondary))
    };
    let search = coarse_to_fine_split_search(0, 2_048, &mut score, &mut SearchStop::never())
        .expect("two sampled basins");

    assert_eq!(search.best.token, 420);
    let secondary = search
        .secondary
        .expect("the distant local minimum is retained");
    assert!(secondary.token > 1_000);
    assert!(secondary.token.abs_diff(search.best.token) >= 2_048 / ADAPTIVE_SPLIT_INTERVALS);
}

#[test]
fn adaptive_histogram_cut_finds_a_literal_transition() {
    let mut bytes = vec![b'a'; 733];
    bytes.extend(std::iter::repeat(b'z').take(1_315));
    let block = literal_block(&bytes, SourceBlockType::Dynamic);
    let blocks = [block];
    let composite = Composite::new(&blocks).unwrap();
    let mut cuts = Vec::new();
    let start = Cut { token: 0, plain: 0 };
    let end = Cut {
        token: bytes.len(),
        plain: bytes.len(),
    };

    add_adaptive_split_cut(
        &mut cuts,
        &composite,
        start,
        end,
        false,
        &mut SearchStop::never(),
    )
    .unwrap();

    assert_eq!(
        cuts,
        [Cut {
            token: 733,
            plain: 733,
        }]
    );
}

#[test]
fn adaptive_split_skips_small_ranges_and_respects_an_expired_deadline() {
    let small = literal_block(&vec![b'x'; 512], SourceBlockType::Dynamic);
    let small_blocks = [small];
    let small_composite = Composite::new(&small_blocks).unwrap();
    let mut cuts = Vec::new();
    add_adaptive_split_cut(
        &mut cuts,
        &small_composite,
        Cut { token: 0, plain: 0 },
        Cut {
            token: 512,
            plain: 512,
        },
        false,
        &mut SearchStop::never(),
    )
    .unwrap();
    assert!(cuts.is_empty());

    let mut scorer_called = false;
    let candidate = coarse_to_fine_split(
        0,
        2_048,
        &mut |_| {
            scorer_called = true;
            Some(0)
        },
        &mut SearchStop::always(),
    );
    assert!(candidate.is_none());
    assert!(!scorer_called);
}
