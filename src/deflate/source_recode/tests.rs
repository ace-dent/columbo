// SPDX-License-Identifier: MIT

use std::cell::Cell;

use super::*;

fn literal_block(bytes: &[u8], source_type: SourceBlockType) -> ParsedBlock {
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
        source_type,
    }
}

fn costly_match_block() -> ParsedBlock {
    // Under fixed tables this length-three, longest-family distance match
    // costs one bit more than spelling its three decoded `a` bytes. It is a
    // compact fixture for proving whether fixed-strict expansion ran.
    let tokens = vec![Token::Match {
        length: 3,
        distance: 24_577,
        length_symbol: 257,
        distance_symbol: 29,
        length_extra: 0,
        distance_extra: 0,
        length_extra_bits: 0,
        distance_extra_bits: 13,
    }];
    let (literal_frequencies, distance_frequencies) = count_frequencies(&tokens);
    ParsedBlock {
        tokens: Arc::new(tokens),
        plain: Arc::new(b"aaa".to_vec()),
        literal_frequencies,
        distance_frequencies,
        original_literal_lengths: None,
        original_distance_lengths: None,
        original_dynamic: None,
        original: None,
        source_splits: Vec::new(),
        source_type: SourceBlockType::Fixed,
    }
}

fn match_token(length: u16, length_symbol: u16, distance_symbol: u8) -> Token {
    Token::Match {
        length,
        distance: if distance_symbol == 29 { 24_577 } else { 1 },
        length_symbol,
        distance_symbol,
        length_extra: 0,
        distance_extra: 0,
        length_extra_bits: 0,
        distance_extra_bits: if distance_symbol == 29 { 13 } else { 0 },
    }
}

fn push_fixed_state(queue: &mut SourceStateQueue, tokens: Arc<Vec<Token>>) -> StateId {
    let (literal_frequencies, distance_frequencies) = count_frequencies(&tokens);
    let (literal_lengths, distance_lengths) = fixed_lengths();
    queue
        .push(PendingState {
            tokens: Arc::clone(&tokens),
            token_hash: token_hash(&tokens),
            literal_lengths,
            distance_lengths,
            literal_frequencies,
            distance_frequencies,
            extra_bits: token_extra_bits(&tokens),
            depth: 0,
            token_payload_charge: TokenPayloadCharge::Shared,
        })
        .unwrap()
}

#[test]
fn source_ordered_list_removes_empty_blocks_but_keeps_one_empty_block() {
    let empty = literal_block(&[], SourceBlockType::Fixed);
    let content = literal_block(b"content", SourceBlockType::Fixed);
    assert_eq!(
        prepare_source_blocks(&[empty.clone(), content, empty.clone()])
            .unwrap()
            .len(),
        1
    );
    let all_empty = prepare_source_blocks(&[empty.clone(), empty]).unwrap();
    assert_eq!(all_empty.len(), 1);
    assert!(all_empty[0].as_ref().unwrap().block.plain.is_empty());
}

#[test]
fn accepted_merge_retries_the_same_index() {
    let blocks = [
        literal_block(b"aaaaaaaa", SourceBlockType::Fixed),
        literal_block(b"aaaaaaaa", SourceBlockType::Fixed),
        literal_block(b"aaaaaaaa", SourceBlockType::Fixed),
    ];
    let mut never = SearchStop::never();
    let plans = plan_source_blocks(&blocks, 0, &Options::default(), &mut never).unwrap();
    assert_eq!(
        plans.len(),
        1,
        "the accepted pair must absorb its third neighbour"
    );
    assert_eq!(plans[0].plain.len(), 24);
}

#[test]
fn source_route_runs_past_the_historical_128_block_threshold() {
    let blocks = (0..129)
        .map(|_| literal_block(b"a", SourceBlockType::Stored))
        .collect::<Vec<_>>();
    let mut never = SearchStop::never();
    let plans = plan_source_blocks(&blocks, 0, &Options::default(), &mut never).unwrap();

    assert_eq!(plans.len(), 1);
    assert_eq!(plans[0].plain.len(), 129);
}

#[test]
fn retired_merge_slots_preserve_order_alignment_and_the_unvisited_tail() {
    let blocks: Vec<_> = [2, 3, 65_531, 4, 5]
        .into_iter()
        .enumerate()
        .map(|(index, length)| {
            let mut block = literal_block(&[], SourceBlockType::Stored);
            block.plain = Arc::new(vec![index as u8; length]);
            block
        })
        .collect();
    let expected: Vec<_> = blocks
        .iter()
        .flat_map(|block| block.plain.iter().copied())
        .collect();
    for alignment in 0..8 {
        for cutoff in [0, 1, 10, 20, 40, usize::MAX] {
            let mut calls = 0;
            let mut expired = || {
                calls += 1;
                calls > cutoff
            };
            let plans = plan_source_blocks(
                &blocks,
                alignment,
                &Options::default(),
                &mut SearchStop::callback(&mut expired),
            )
            .unwrap();
            let plain: Vec<_> = plans
                .iter()
                .flat_map(|plan| plan.plain.iter().copied())
                .collect();
            assert_eq!(plain, expected, "alignment {alignment}, cutoff {cutoff}");
            let mut at = alignment;
            for plan in &plans {
                assert!(matches!(plan.representation, Representation::Stored));
                assert_eq!(plan.bits, stored_block_bits(at, plan.plain.len()));
                at = ((u64::from(at) + plan.bits) & 7) as u8;
            }
            if cutoff == usize::MAX {
                // Accept the first pair, reject its oversized neighbour,
                // accept a later pair, then retain the final untouched block.
                assert_eq!(
                    plans
                        .iter()
                        .map(|plan| plan.plain.len())
                        .collect::<Vec<_>>(),
                    [5, 65_535, 5],
                );
            }
        }
    }
}

#[test]
fn stored_merge_limit_is_asymmetric_and_hard() {
    let left = literal_block(&vec![1; 40_000], SourceBlockType::Stored);
    let right = literal_block(&vec![2; 30_000], SourceBlockType::Fixed);
    let mut never = SearchStop::never();
    let plans = plan_source_blocks(&[left, right], 0, &Options::default(), &mut never).unwrap();
    assert_eq!(plans.len(), 2);
}

#[test]
fn completed_plan_is_reused_at_the_same_alignment() {
    let mut block = WorkingBlock::new(literal_block(b"alignment cache", SourceBlockType::Fixed));
    let calls = Cell::new(0_usize);
    let mut deadline = || {
        calls.set(calls.get() + 1);
        false
    };
    let mut stop = SearchStop::callback(&mut deadline);
    let mut budget = SourceRouteBudget::new(MAX_SOURCE_ROUTE_BYTES);
    let first = block
        .plan(3, &Options::default(), &mut budget, &mut stop)
        .unwrap();
    let after_first = calls.get();
    let second = block
        .plan(3, &Options::default(), &mut budget, &mut stop)
        .unwrap();
    assert_eq!(first, second);
    assert_eq!(calls.get(), after_first);
}

#[test]
fn expired_tail_uses_current_tokens_without_fixed_strict_expansion() {
    let blocks = [costly_match_block(), costly_match_block()];
    let source_tokens = [Arc::clone(&blocks[0].tokens), Arc::clone(&blocks[1].tokens)];
    let mut stop = SearchStop::always();
    let plans = plan_source_blocks(&blocks, 0, &Options::default(), &mut stop).unwrap();

    assert_eq!(plans.len(), 2);
    for (plan, source) in plans.iter().zip(source_tokens) {
        assert!(Arc::ptr_eq(&plan.tokens, &source));
        assert_eq!(plan.tokens.len(), 1);
        assert!(matches!(plan.representation, Representation::Fixed));
    }
}

#[test]
fn cumulative_budget_limits_retained_expansions_across_blocks() {
    let blocks = [costly_match_block(), costly_match_block()];
    let working_bytes = prepared_source_blocks_bytes(&blocks).unwrap();
    // One three-token expansion plus its temporary one-byte mark fits. Once
    // retained, only that mark byte remains and the second block cannot
    // allocate another expanded token vector. The route-local working-block
    // metadata is now charged independently of transformed payloads.
    let budget_bytes = working_bytes + token_payload_bytes(3).unwrap() + 1;
    let mut never = SearchStop::never();
    let plans =
        plan_source_blocks_with_budget(&blocks, 0, &Options::default(), budget_bytes, &mut never)
            .unwrap();

    assert_eq!(plans.len(), 2);
    assert_eq!(plans[0].tokens.len(), 3);
    assert_eq!(plans[1].tokens.len(), 1);
}

#[test]
fn expansion_payload_is_rejected_before_reserve_when_over_budget() {
    let block = costly_match_block();
    let marks = [1];
    let one_byte_short = token_payload_bytes(3).unwrap() - 1;
    assert!(expand_marked(&block.tokens, &block.plain, &marks, one_byte_short).is_none());
}

#[test]
fn expanded_state_fingerprint_matches_a_complete_token_scan() {
    let block = costly_match_block();
    let expanded = expand_marked(
        &block.tokens,
        &block.plain,
        &[1],
        token_payload_bytes(3).unwrap(),
    )
    .unwrap();

    assert_eq!(expanded.token_hash, token_hash(&expanded.tokens));
}

#[test]
fn merged_model_is_preflighted_against_remaining_route_budget() {
    let left = literal_block(b"left", SourceBlockType::Fixed);
    let right = literal_block(b"right", SourceBlockType::Fixed);
    let token_count = left.tokens.len() + right.tokens.len();
    let plain_count = left.plain.len() + right.plain.len();
    let required = parsed_model_bytes(plain_count, token_count, 1)
        .unwrap()
        .checked_add(size_of::<usize>())
        .unwrap();

    assert!(merge_blocks(&left, &right, SourceBlockType::Fixed, required - 1,).is_none());
    assert!(merge_blocks(&left, &right, SourceBlockType::Fixed, required).is_some());
}

#[test]
fn queue_identity_includes_huffman_tables() {
    let block = literal_block(b"same tokens", SourceBlockType::Fixed);
    let (first_literal, first_distance) =
        source_recode_lengths(&block.literal_frequencies, &block.distance_frequencies).unwrap();
    let mut second_literal = first_literal;
    second_literal[0] = second_literal[0].saturating_add(1);
    let mut queue = SourceStateQueue::new(MAX_SOURCE_ARENA_BYTES);
    let first = queue
        .push(PendingState {
            tokens: Arc::clone(&block.tokens),
            token_hash: token_hash(&block.tokens),
            literal_lengths: first_literal,
            distance_lengths: first_distance,
            literal_frequencies: block.literal_frequencies,
            distance_frequencies: block.distance_frequencies,
            extra_bits: 0,
            depth: 0,
            token_payload_charge: TokenPayloadCharge::Shared,
        })
        .unwrap();
    let second = queue
        .push(PendingState {
            tokens: Arc::clone(&block.tokens),
            token_hash: token_hash(&block.tokens),
            literal_lengths: second_literal,
            distance_lengths: first_distance,
            literal_frequencies: block.literal_frequencies,
            distance_frequencies: block.distance_frequencies,
            extra_bits: 0,
            depth: 0,
            token_payload_charge: TokenPayloadCharge::Shared,
        })
        .unwrap();
    assert_ne!(first, second);
}

#[test]
fn queue_state_count_is_limited_by_arena_bytes_not_an_iteration_cap() {
    let tokens = Arc::new(vec![Token::Literal(b'a')]);
    let (literal_frequencies, distance_frequencies) = count_frequencies(&tokens);
    let (_, distance_lengths) = fixed_lengths();
    let mut queue = SourceStateQueue::new(MAX_SOURCE_ARENA_BYTES);

    for index in 0_u32..=4_096 {
        let mut literal_lengths = [0_u8; 286];
        literal_lengths[0] = index as u8;
        literal_lengths[1] = (index >> 8) as u8;
        literal_lengths[2] = (index >> 16) as u8;
        assert!(
            queue
                .push(PendingState {
                    tokens: Arc::clone(&tokens),
                    token_hash: token_hash(&tokens),
                    literal_lengths,
                    distance_lengths,
                    literal_frequencies,
                    distance_frequencies,
                    extra_bits: 0,
                    depth: usize::try_from(index).unwrap(),
                    token_payload_charge: TokenPayloadCharge::Shared,
                })
                .is_some(),
            "state {index} should fit the configured byte arena"
        );
    }
    assert_eq!(queue.states.len(), 4_097);
    assert!(!queue.saturated);
}

#[test]
fn alternate_seed_is_distinct_from_the_source_table() {
    let block = literal_block(b"alternate", SourceBlockType::Fixed);
    let plan = PlannedBlock {
        tokens: Arc::clone(&block.tokens),
        plain: Arc::clone(&block.plain),
        representation: Representation::Fixed,
        bits: 0,
        source_type: SourceBlockType::Fixed,
    };
    let (fixed_literal, fixed_distance) = fixed_lengths();

    assert!(alternate_seed_lengths(&plan, &fixed_literal, &fixed_distance).is_none());

    let mut different_literal = fixed_literal;
    different_literal[usize::from(b'a')] = different_literal[usize::from(b'a')].saturating_sub(1);
    assert_eq!(
        alternate_seed_lengths(&plan, &different_literal, &fixed_distance),
        Some((fixed_literal, fixed_distance))
    );
}

#[test]
fn queue_identity_keeps_shared_pointer_and_content_fallbacks() {
    let block = literal_block(b"identity", SourceBlockType::Fixed);
    let mut queue = SourceStateQueue::new(MAX_SOURCE_ARENA_BYTES);
    let first = push_fixed_state(&mut queue, Arc::clone(&block.tokens));

    // The common table-only path shares the allocation. A separately
    // allocated but equal token vector still exercises the collision-safe
    // content fallback and must resolve to the same state. Duplicate
    // detection happens before arena charging, even when the caller just
    // allocated that equivalent payload.
    let shared = push_fixed_state(&mut queue, Arc::clone(&block.tokens));
    let accounted_before_duplicate = queue.accounted_bytes;
    let copied_tokens = Arc::new(block.tokens.as_ref().clone());
    let (literal_frequencies, distance_frequencies) = count_frequencies(&copied_tokens);
    let (literal_lengths, distance_lengths) = fixed_lengths();
    let copied = queue
        .push(PendingState {
            tokens: Arc::clone(&copied_tokens),
            token_hash: token_hash(&copied_tokens),
            literal_lengths,
            distance_lengths,
            literal_frequencies,
            distance_frequencies,
            extra_bits: token_extra_bits(&copied_tokens),
            depth: 0,
            token_payload_charge: TokenPayloadCharge::NewlyAllocated,
        })
        .unwrap();

    assert_eq!(shared, first);
    assert_eq!(copied, first);
    assert_eq!(queue.states.len(), 1);
    assert_eq!(queue.accounted_bytes, accounted_before_duplicate);
}

#[test]
fn identical_least_family_routes_share_the_transformed_state() {
    let block = costly_match_block();
    let mut queue = SourceStateQueue::new(MAX_SOURCE_ARENA_BYTES);
    let base = push_fixed_state(&mut queue, Arc::clone(&block.tokens));

    let least_expensive = queue.transform_least(base, &block.plain, false).unwrap();
    let states_after_first = queue.states.len();
    let least_seen = queue.transform_least(base, &block.plain, true).unwrap();

    assert!(matches!(
        queue.states[base].least_families,
        LeastFamilyMemo::Scored(LeastFamilyChoices {
            least_expensive: Some(257),
            least_seen: Some(257),
        })
    ));
    assert_eq!(least_expensive.state, least_seen.state);
    assert!(least_expensive.changed && least_seen.changed);
    assert_eq!(queue.states.len(), states_after_first);
    assert!(queue.states[least_seen.state]
        .tokens
        .iter()
        .all(|token| matches!(token, Token::Literal(b'a'))));
}

#[test]
fn distinct_least_family_choices_reuse_analysis_but_keep_route_order() {
    // Family 257 appears twice and saves one fixed-table bit per match;
    // family 258 appears once but is much dearer to expand. The two named
    // selectors must therefore retain their distinct source-order choices.
    let tokens = Arc::new(vec![
        match_token(3, 257, 29),
        match_token(3, 257, 29),
        match_token(4, 258, 0),
    ]);
    let plain = b"aaaaaabbbb";
    let mut queue = SourceStateQueue::new(MAX_SOURCE_ARENA_BYTES);
    let base = push_fixed_state(&mut queue, Arc::clone(&tokens));

    let least_expensive = queue.transform_least(base, plain, false).unwrap();
    let least_seen = queue.transform_least(base, plain, true).unwrap();

    assert!(matches!(
        queue.states[base].least_families,
        LeastFamilyMemo::Scored(LeastFamilyChoices {
            least_expensive: Some(257),
            least_seen: Some(258),
        })
    ));
    assert_ne!(least_expensive.state, least_seen.state);
    assert_eq!(queue.states[least_expensive.state].tokens.len(), 7);
    assert_eq!(queue.states[least_seen.state].tokens.len(), 6);
}
