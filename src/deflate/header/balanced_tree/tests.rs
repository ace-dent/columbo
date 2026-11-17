// SPDX-License-Identifier: MIT
// SPDX-FileCopyrightText: 2026 Andrew C.E. Dent <https://github.com/ace-dent>

use super::*;
use crate::deflate::huffman::Huffman;

#[test]
fn columbo_quad_move_preserves_a_complete_tree() {
    let mut tokens = vec![Token::Literal(b'A'); 100];
    tokens.extend(b"bcde".iter().copied().map(Token::Literal));
    let mut literal_frequencies = [0_u32; 286];
    literal_frequencies[usize::from(b'A')] = 100;
    for symbol in b"bcde" {
        literal_frequencies[usize::from(*symbol)] = 1;
    }
    literal_frequencies[256] = 1;
    let distance_frequencies = [0_u32; 30];

    // Two length-two leaves and four length-three leaves form a complete
    // tree. The quad move shortens the frequent leaf and lengthens all
    // four rare leaves while preserving that Kraft sum.
    let mut literal_lengths = vec![0_u8; 257];
    literal_lengths[usize::from(b'A')] = 2;
    literal_lengths[256] = 2;
    for symbol in b"bcde" {
        literal_lengths[usize::from(*symbol)] = 3;
    }
    let seed = DynamicPlan {
        literal_lengths,
        distance_lengths: vec![1],
        code_length_lengths: [0; 19],
        rle: Vec::new(),
        hlit: 257,
        hdist: 1,
        hclen: 4,
        bits: 0,
    };

    let candidate = plan_columbo_balanced_tree_candidate(
        &tokens,
        &literal_frequencies,
        &distance_frequencies,
        &seed,
        true,
        false,
    )
    .expect("the Kraft-preserving quad move is legal");

    assert_eq!(candidate.literal_lengths[usize::from(b'A')], 1);
    for symbol in b"bcde" {
        assert_eq!(candidate.literal_lengths[usize::from(*symbol)], 4);
    }
    assert!(Huffman::build(&candidate.literal_lengths).is_some());
}

#[test]
fn columbo_pair_move_preserves_a_complete_tree() {
    let mut tokens = vec![Token::Literal(b'A'); 100];
    tokens.extend(b"bc".iter().copied().map(Token::Literal));
    let mut literal_frequencies = [0_u32; 286];
    literal_frequencies[usize::from(b'A')] = 100;
    for symbol in b"bc" {
        literal_frequencies[usize::from(*symbol)] = 1;
    }
    literal_frequencies[256] = 1;
    let distance_frequencies = [0_u32; 30];

    // Four length-two leaves form a complete tree. Shortening the frequent
    // leaf and lengthening two rare leaves preserves the Kraft sum.
    let mut literal_lengths = vec![0_u8; 257];
    for symbol in b"Abc" {
        literal_lengths[usize::from(*symbol)] = 2;
    }
    literal_lengths[256] = 2;
    let seed = DynamicPlan {
        literal_lengths,
        distance_lengths: vec![1],
        code_length_lengths: [0; 19],
        rle: Vec::new(),
        hlit: 257,
        hdist: 1,
        hclen: 4,
        bits: 0,
    };

    let candidate = plan_columbo_balanced_tree_candidate(
        &tokens,
        &literal_frequencies,
        &distance_frequencies,
        &seed,
        true,
        true,
    )
    .expect("the Kraft-preserving pair move is legal");

    assert_eq!(candidate.literal_lengths[usize::from(b'A')], 1);
    assert_eq!(candidate.literal_lengths[usize::from(b'b')], 3);
    assert_eq!(candidate.literal_lengths[usize::from(b'c')], 3);
    assert!(Huffman::build(&candidate.literal_lengths).is_some());
}

#[test]
fn columbo_pair_move_also_optimizes_the_distance_tree() {
    let mut tokens = Vec::new();
    for (symbol, count) in [(0_u8, 100_usize), (1, 1), (2, 1), (3, 1)] {
        tokens.extend((0..count).map(|_| Token::Match {
            length: 3,
            distance: u16::from(symbol) + 1,
            length_symbol: 257,
            distance_symbol: symbol,
        }));
    }
    let mut literal_frequencies = [0_u32; 286];
    literal_frequencies[256] = 1;
    literal_frequencies[257] = tokens.len() as u32;
    let mut distance_frequencies = [0_u32; 30];
    distance_frequencies[0] = 100;
    distance_frequencies[1..4].fill(1);
    let mut literal_lengths = vec![0_u8; 258];
    literal_lengths[256] = 1;
    literal_lengths[257] = 1;
    let seed = DynamicPlan {
        literal_lengths,
        distance_lengths: vec![2; 4],
        code_length_lengths: [0; 19],
        rle: Vec::new(),
        hlit: 258,
        hdist: 4,
        hclen: 4,
        bits: 0,
    };

    let candidate = plan_columbo_balanced_tree_candidate(
        &tokens,
        &literal_frequencies,
        &distance_frequencies,
        &seed,
        true,
        false,
    )
    .expect("the distance-side pair move is legal");

    assert_eq!(candidate.distance_lengths[0], 1);
    assert_eq!(
        candidate
            .distance_lengths
            .iter()
            .filter(|&&length| length == 3)
            .count(),
        2
    );
    assert!(Huffman::build(&candidate.distance_lengths).is_some());
}

#[test]
fn columbo_quad_move_also_optimizes_the_distance_tree() {
    let tokens = Vec::new();
    let mut literal_frequencies = [0_u32; 286];
    literal_frequencies[256] = 1;
    let mut distance_frequencies = [0_u32; 30];
    distance_frequencies[0] = 100;
    distance_frequencies[1..6].fill(1);
    let mut literal_lengths = vec![0_u8; 257];
    literal_lengths[256] = 1;
    let seed = DynamicPlan {
        literal_lengths,
        distance_lengths: vec![2, 2, 3, 3, 3, 3],
        code_length_lengths: [0; 19],
        rle: Vec::new(),
        hlit: 257,
        hdist: 6,
        hclen: 4,
        bits: 0,
    };

    let candidate = plan_columbo_balanced_tree_candidate(
        &tokens,
        &literal_frequencies,
        &distance_frequencies,
        &seed,
        true,
        false,
    )
    .expect("the distance-side quad move is legal");

    assert_eq!(candidate.distance_lengths[0], 1);
    assert!(candidate.distance_lengths[2..6]
        .iter()
        .all(|&length| length == 4));
    assert!(Huffman::build(&candidate.distance_lengths).is_some());
}

#[test]
fn columbo_balanced_tree_search_prices_paired_alphabet_moves() {
    let tokens = Vec::new();
    let mut literal_frequencies = [0_u32; 286];
    literal_frequencies[usize::from(b'A')] = 100;
    literal_frequencies[usize::from(b'b')] = 1;
    literal_frequencies[usize::from(b'c')] = 1;
    literal_frequencies[256] = 1;
    let mut distance_frequencies = [0_u32; 30];
    distance_frequencies[0] = 100;
    distance_frequencies[1..4].fill(1);
    let mut literal_lengths = vec![0_u8; 257];
    for symbol in b"Abc" {
        literal_lengths[usize::from(*symbol)] = 2;
    }
    literal_lengths[256] = 2;
    let seed = DynamicPlan {
        literal_lengths,
        distance_lengths: vec![2; 4],
        code_length_lengths: [0; 19],
        rle: Vec::new(),
        hlit: 257,
        hdist: 4,
        hclen: 4,
        bits: 0,
    };

    let opportunities =
        balanced_tree_opportunities(&literal_frequencies, &distance_frequencies, &seed)
            .expect("opportunity counting should remain bounded");
    assert!(opportunities.literal_pair_moves > 0);
    assert!(opportunities.distance_pair_moves > 0);
    assert_eq!(opportunities.paired_prices, 16);

    let candidate = plan_columbo_balanced_tree_candidate(
        &tokens,
        &literal_frequencies,
        &distance_frequencies,
        &seed,
        true,
        true,
    )
    .expect("the paired balanced-tree move is legal");

    assert_eq!(candidate.literal_lengths[usize::from(b'A')], 1);
    assert_eq!(candidate.distance_lengths[0], 1);
    assert!(Huffman::build(&candidate.literal_lengths).is_some());
    assert!(Huffman::build(&candidate.distance_lengths).is_some());
}
