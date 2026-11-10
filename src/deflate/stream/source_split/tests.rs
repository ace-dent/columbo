// SPDX-License-Identifier: MIT
// SPDX-FileCopyrightText: 2026 Andrew C.E. Dent <https://github.com/ace-dent>

use std::sync::Arc;

use super::*;
use crate::deflate::model::{SourceBlockType, Token};
use crate::deflate::stream::test_support::literal_block;

#[test]
fn adaptive_split_requires_a_material_exact_saving() {
    assert!(!adaptive_split_is_worth_replay(969, 1_000));
    assert!(adaptive_split_is_worth_replay(968, 1_000));
    assert!(!adaptive_split_is_worth_replay(u64::MAX, u64::MAX));
}

#[test]
fn split_first_route_requires_measured_structural_dominance() {
    assert!(!search_split_before_whole_block(true, None, 1_000,));
    assert!(!search_split_before_whole_block(false, Some(999), 1_000,));
    assert!(search_split_before_whole_block(true, Some(999), 1_000,));
    assert!(!search_split_before_whole_block(true, Some(1_000), 1_000,));
}

#[test]
fn midpoint_refinement_keeps_snap_and_adds_exact_inside_match_cut() {
    let mut block = literal_block(&[0; 128], SourceBlockType::Dynamic);
    block.tokens = Arc::new((0..10).map(|_| Token::Literal(0)).collect());
    Arc::make_mut(&mut block.tokens).push(Token::Match {
        length: 100,
        distance: 1,
        length_symbol: 279,
        distance_symbol: 0,
        length_extra: 1,
        distance_extra: 0,
        length_extra_bits: 4,
        distance_extra_bits: 0,
    });
    Arc::make_mut(&mut block.tokens).extend((0..18).map(|_| Token::Literal(0)));
    block.recount_frequencies();

    let blocks = [block];
    let composite = Composite::new(&blocks).unwrap();
    let midpoints = midpoint_cuts(
        &composite,
        Cut { token: 0, plain: 0 },
        Cut {
            token: 29,
            plain: 128,
        },
    );

    // Decoded offset 64 lies inside the 100-byte match. Keep the prior
    // token boundary and add the exact proven-submatch sibling.
    assert_eq!(
        midpoints,
        [
            Some(Cut {
                token: 10,
                plain: 10,
            }),
            Some(Cut {
                token: 10,
                plain: 64,
            }),
        ]
    );
}
