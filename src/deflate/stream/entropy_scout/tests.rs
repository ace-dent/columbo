// SPDX-License-Identifier: MIT
// SPDX-FileCopyrightText: 2026 Andrew C.E. Dent <https://github.com/ace-dent>

use super::*;
use crate::deflate::stream::test_support::literal_block;

#[test]
fn entropy_state_assignment_matches_an_exhaustive_oracle() {
    let left_tokens = [Token::Literal(b'a'); 32];
    let right_tokens = [Token::Literal(b'b'); 32];
    let mut left = EntropyScoutHistogram::zero();
    let mut right = EntropyScoutHistogram::zero();
    for token in left_tokens {
        left.add_token(token);
    }
    for token in right_tokens {
        right.add_token(token);
    }
    let codes = [
        EntropyScoutCode::from_histogram(&left, true),
        EntropyScoutCode::from_histogram(&right, true),
    ];
    let tokens = [
        Token::Literal(b'a'),
        Token::Literal(b'a'),
        Token::Literal(b'b'),
        Token::Literal(b'b'),
        Token::Literal(b'a'),
    ];
    let switch_bits = 2;
    let states =
        assign_entropy_states(&tokens, &codes, switch_bits, &mut SearchStop::never()).unwrap();
    let path_cost = |states: &[u8]| {
        let payload: u64 = tokens
            .iter()
            .zip(states)
            .map(|(&token, &state)| codes[usize::from(state)].token_bits(token))
            .sum();
        let switches = states.windows(2).filter(|pair| pair[0] != pair[1]).count() as u64;
        payload + switches * switch_bits
    };
    let oracle = (0_u8..1 << tokens.len())
        .map(|mask| {
            let candidate: Vec<_> = (0..tokens.len())
                .map(|position| (mask >> position) & 1)
                .collect();
            path_cost(&candidate)
        })
        .min()
        .unwrap();

    assert_eq!(path_cost(&states), oracle);
}

#[test]
fn entropy_state_scout_finds_multiple_off_grid_transitions_deterministically() {
    let mut bytes = vec![b'a'; 700];
    bytes.extend(std::iter::repeat(b'b').take(700));
    bytes.extend(std::iter::repeat(b'c').take(700));
    let block = literal_block(&bytes, SourceBlockType::Dynamic);
    let blocks = [block];
    let composite = Composite::new(&blocks).unwrap();

    let first = entropy_state_boundary_cuts(&composite, true, &mut SearchStop::never())
        .expect("the learned states expose both literal transitions");
    let second = entropy_state_boundary_cuts(&composite, true, &mut SearchStop::never())
        .expect("the scout is deterministic");

    assert_eq!(first, second);
    assert!(first.contains(&Cut {
        token: 700,
        plain: 700,
    }));
    assert!(first.contains(&Cut {
        token: 1_400,
        plain: 1_400,
    }));
}

#[test]
fn entropy_state_scout_is_bounded_and_skips_ineligible_work() {
    let small = literal_block(&vec![b'a'; 512], SourceBlockType::Dynamic);
    let small_blocks = [small];
    let small_composite = Composite::new(&small_blocks).unwrap();
    assert!(
        entropy_state_boundary_cuts(&small_composite, true, &mut SearchStop::never(),).is_none()
    );

    let stored = literal_block(&vec![b'a'; 1_024], SourceBlockType::Stored);
    let stored_blocks = [stored];
    let stored_composite = Composite::new(&stored_blocks).unwrap();
    assert!(
        entropy_state_boundary_cuts(&stored_composite, true, &mut SearchStop::never(),).is_none()
    );

    let eligible = literal_block(&vec![b'a'; 1_024], SourceBlockType::Dynamic);
    let eligible_blocks = [eligible];
    let eligible_composite = Composite::new(&eligible_blocks).unwrap();
    assert!(
        entropy_state_boundary_cuts(&eligible_composite, true, &mut SearchStop::always(),)
            .is_none()
    );

    let mut bytes = Vec::new();
    for chunk in 0..40 {
        bytes.extend(std::iter::repeat(if chunk % 2 == 0 { b'a' } else { b'b' }).take(32));
    }
    let block = literal_block(&bytes, SourceBlockType::Dynamic);
    let blocks = [block];
    let composite = Composite::new(&blocks).unwrap();
    let mut left = EntropyScoutHistogram::zero();
    let mut right = EntropyScoutHistogram::zero();
    for _ in 0..32 {
        left.add_token(Token::Literal(b'a'));
        right.add_token(Token::Literal(b'b'));
    }
    let codes = [
        EntropyScoutCode::from_histogram(&left, true),
        EntropyScoutCode::from_histogram(&right, true),
    ];
    let states: Vec<_> = (0..bytes.len())
        .map(|position| (position / 32 % 2) as u8)
        .collect();
    let cuts = ranked_entropy_state_cuts(&composite, &codes, &states).unwrap();
    assert_eq!(cuts.len(), ENTROPY_SCOUT_MAX_CUTS);
    assert!(cuts.windows(2).all(|pair| pair[0].plain < pair[1].plain));
}
