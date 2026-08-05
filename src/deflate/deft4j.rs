// SPDX-License-Identifier: MIT

//! Source-ordered deft4j beta-17 candidate optimization.
//!
//! deft4j does not rank a generic beam of promising token streams. It walks a
//! named, insertion-ordered graph of block objects carrying both tokens and
//! data-code tables, then feeds optimized objects into a greedy left-to-right
//! merge pass. Keeping that route isolated makes its ordering and tie rules
//! visible instead of hiding them inside the broader Columbo search.
//!
//! This is deliberately labelled deft4j-derived rather than exact parity.
//! Columbo adds structural state de-duplication, safety budgets, deadlines, an
//! alternate table seed, a legacy two-leaf distance rebuild, corrected
//! empty-block traversal, and corrected alignment accounting. The recovered
//! deft4j operations and their order stay visible inside those Columbo
//! safeguards.

use std::array;
use std::collections::HashMap;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::mem::size_of;
use std::sync::Arc;

use crate::Options;

use super::block::{fixed_block_bits, reusable_original_bits, stored_block_bits};
use super::header::{plan_for_deft4j_lengths_with_cost, Deft4jHeaderPolicy};
use super::huffman::{
    make_lengths_deft4j_java_heap, FIXED_DISTANCE_CODE_LENGTHS, FIXED_LITERAL_CODE_LENGTHS,
};
use super::model::{
    count_frequencies, token_extra_bits, DynamicPlan, ParsedBlock, PlannedBlock, Representation,
    SourceBlockType, Token,
};
use super::parse::{parsed_model_bytes, MAX_PARSED_MODEL_BYTES};
use super::stop::SearchStop;

type StateId = usize;

/// The parsed stream already owns up to `MAX_PARSED_MODEL_BYTES`. This
/// deft4j-derived route shares that source model but can materialize expanded
/// token states and merged payloads. Columbo keeps every additional live
/// payload under one ceiling so many blocks cannot multiply memory use.
const MAX_DEFT4J_ROUTE_BYTES: usize = MAX_PARSED_MODEL_BYTES / 2;
const MAX_DEFT4J_ARENA_BYTES: usize = MAX_DEFT4J_ROUTE_BYTES;
const MAX_DEFT4J_STATES: usize = 4_096;

fn token_payload_bytes(token_count: usize) -> Option<usize> {
    token_count.checked_mul(size_of::<Token>())
}

/// Additional memory retained by one bounded deft4j source-list candidate.
///
/// The parser's source buffers are deliberately not charged again: source
/// blocks share those `Arc`s. Expanded token vectors and merged token/plain
/// buffers are charged before allocation and remain charged until the owning
/// working block is replaced or discarded.
struct Deft4jRouteBudget {
    limit_bytes: usize,
    live_bytes: usize,
}

impl Deft4jRouteBudget {
    fn new(limit_bytes: usize) -> Self {
        Self {
            limit_bytes,
            live_bytes: 0,
        }
    }

    fn remaining(&self) -> usize {
        self.limit_bytes.saturating_sub(self.live_bytes)
    }

    fn reserve(&mut self, bytes: usize) -> Option<()> {
        let live_bytes = self.live_bytes.checked_add(bytes)?;
        if live_bytes > self.limit_bytes {
            return None;
        }
        self.live_bytes = live_bytes;
        Some(())
    }

    fn release(&mut self, bytes: usize) -> Option<()> {
        self.live_bytes = self.live_bytes.checked_sub(bytes)?;
        Some(())
    }
}

#[derive(Debug, Clone, Copy)]
enum ScoreKind {
    CompleteHeader,
    DefaultHeaderFixedPoint,
}

impl ScoreKind {
    fn index(self) -> usize {
        match self {
            Self::CompleteHeader => 0,
            Self::DefaultHeaderFixedPoint => 1,
        }
    }

    fn header_policy(self) -> Deft4jHeaderPolicy {
        match self {
            Self::CompleteHeader => Deft4jHeaderPolicy::Complete,
            Self::DefaultHeaderFixedPoint => Deft4jHeaderPolicy::DefaultRecode,
        }
    }
}

enum ScoreMemo {
    Unscored,
    Invalid,
    Scored(DynamicPlan),
}

#[derive(Debug, Clone, Copy)]
enum TransformKind {
    Strict,
    Recode,
    Pruned,
    LeastExpensive,
    LeastSeen,
}

impl TransformKind {
    fn index(self) -> usize {
        match self {
            Self::Strict => 0,
            Self::Recode => 1,
            Self::Pruned => 2,
            Self::LeastExpensive => 3,
            Self::LeastSeen => 4,
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct TransformMemo {
    state: StateId,
    changed: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct LeastFamilyChoices {
    least_expensive: Option<u16>,
    least_seen: Option<u16>,
}

#[derive(Debug, Clone, Copy)]
enum LeastFamilyMemo {
    Unscored,
    Scored(LeastFamilyChoices),
}

struct Deft4jState {
    tokens: Arc<Vec<Token>>,
    /// Content hash shared by every table-only state for this token vector.
    /// Keeping it beside the `Arc` avoids hashing a 16K-token image block for
    /// every recode edge in the deft4j queue.
    token_hash: u64,
    literal_lengths: [u8; 286],
    distance_lengths: [u8; 30],
    literal_frequencies: [u32; 286],
    distance_frequencies: [u32; 30],
    extra_bits: u64,
    depth: usize,
    scores: [ScoreMemo; 2],
    transforms: [Option<TransformMemo>; 5],
    /// Both least-family selectors use the same per-match costs and counts.
    /// Cache their choices together so asking for the second route does not
    /// rescan the complete decoded block under the same Huffman tables.
    least_families: LeastFamilyMemo,
}

#[derive(Clone, Copy)]
enum TokenPayloadCharge {
    /// The token allocation is already owned by the parser or another state.
    Shared,
    /// This state retains a newly expanded token allocation in the arena.
    NewlyAllocated,
}

/// Fully computed state waiting to enter the memoized deft4j queue.
///
/// Named fields make the two `u64` values and four Huffman arrays unambiguous at
/// call sites. Callers still supply already-computed statistics so insertion
/// never rescans a potentially large token vector.
struct PendingState {
    tokens: Arc<Vec<Token>>,
    token_hash: u64,
    literal_lengths: [u8; 286],
    distance_lengths: [u8; 30],
    literal_frequencies: [u32; 286],
    distance_frequencies: [u32; 30],
    extra_bits: u64,
    depth: usize,
    token_payload_charge: TokenPayloadCharge,
}

/// Columbo's memoized execution queue for deft4j's ordered candidate graph.
///
/// deft4j beta-17's key objects retain `Object`'s identity-based equality and
/// hashing, while `LinkedHashMap` preserves their insertion order. Columbo
/// hashes equivalent token/table content to avoid duplicate work while
/// preserving first-insertion order.
struct Deft4jQueue {
    states: Vec<Deft4jState>,
    /// A hash maps to the first state with that fingerprint. A vanishingly
    /// unlikely collision falls back to a full scan, retaining correctness
    /// without allocating a second `Vec` for every hash bucket.
    first_by_hash: HashMap<u64, StateId>,
    accounted_bytes: usize,
    limit_bytes: usize,
    saturated: bool,
}

impl Deft4jQueue {
    fn new(limit_bytes: usize) -> Self {
        Self {
            states: Vec::new(),
            first_by_hash: HashMap::new(),
            accounted_bytes: 0,
            limit_bytes: limit_bytes.min(MAX_DEFT4J_ARENA_BYTES),
            saturated: false,
        }
    }

    fn remaining_bytes(&self) -> usize {
        self.limit_bytes.saturating_sub(self.accounted_bytes)
    }

    fn push(&mut self, pending: PendingState) -> Option<StateId> {
        let PendingState {
            tokens,
            token_hash,
            literal_lengths,
            distance_lengths,
            literal_frequencies,
            distance_frequencies,
            extra_bits,
            depth,
            token_payload_charge,
        } = pending;
        let hash = state_hash(token_hash, &literal_lengths, &distance_lengths);
        if let Some(&candidate) = self.first_by_hash.get(&hash) {
            if state_matches(
                &self.states[candidate],
                &tokens,
                &literal_lengths,
                &distance_lengths,
            ) {
                return Some(candidate);
            }
            // Preserve correctness under a real hash collision. The normal
            // path examines one state; only colliding fingerprints scan all.
            if let Some(index) = self.states.iter().position(|state| {
                state_matches(state, &tokens, &literal_lengths, &distance_lengths)
            }) {
                return Some(index);
            }
        }

        if self.states.len() >= MAX_DEFT4J_STATES {
            self.saturated = true;
            return None;
        }
        let payload_bytes = match token_payload_charge {
            TokenPayloadCharge::Shared => 0,
            TokenPayloadCharge::NewlyAllocated => tokens.len().checked_mul(size_of::<Token>())?,
        };
        let added = size_of::<Deft4jState>().checked_add(payload_bytes)?;
        let new_total = self.accounted_bytes.checked_add(added)?;
        if new_total > self.limit_bytes {
            self.saturated = true;
            return None;
        }

        self.states.try_reserve(1).ok()?;
        self.first_by_hash.try_reserve(1).ok()?;
        let index = self.states.len();
        self.states.push(Deft4jState {
            tokens,
            token_hash,
            literal_lengths,
            distance_lengths,
            literal_frequencies,
            distance_frequencies,
            extra_bits,
            depth,
            scores: array::from_fn(|_| ScoreMemo::Unscored),
            transforms: [None; 5],
            least_families: LeastFamilyMemo::Unscored,
        });
        self.first_by_hash.entry(hash).or_insert(index);
        self.accounted_bytes = new_total;
        Some(index)
    }

    fn score(&mut self, state: StateId, kind: ScoreKind, strict: bool) -> Option<&DynamicPlan> {
        let state = &mut self.states[state];
        let slot = kind.index();
        if matches!(state.scores[slot], ScoreMemo::Unscored) {
            let plan = plan_for_deft4j_lengths_with_cost(
                &state.literal_frequencies,
                &state.distance_frequencies,
                state.extra_bits,
                &state.literal_lengths,
                &state.distance_lengths,
                strict,
                kind.header_policy(),
            );
            state.scores[slot] = match plan {
                Some(plan) => ScoreMemo::Scored(plan),
                None => ScoreMemo::Invalid,
            };
        }
        match &state.scores[slot] {
            ScoreMemo::Scored(plan) => Some(plan),
            ScoreMemo::Unscored | ScoreMemo::Invalid => None,
        }
    }

    fn transform_strict(&mut self, source: StateId, plain: &[u8]) -> Option<TransformMemo> {
        self.transform_marked(source, plain, TransformKind::Strict, false)
    }

    fn transform_recode(&mut self, source: StateId) -> Option<TransformMemo> {
        let kind = TransformKind::Recode;
        if let Some(result) = self.states[source].transforms[kind.index()] {
            return Some(result);
        }
        let state = &self.states[source];
        let tokens = Arc::clone(&state.tokens);
        let token_hash = state.token_hash;
        let literal_frequencies = state.literal_frequencies;
        let distance_frequencies = state.distance_frequencies;
        let extra_bits = state.extra_bits;
        let depth = state.depth.checked_add(1)?;
        let (literal_lengths, distance_lengths) =
            columbo_deft4j_lengths(&literal_frequencies, &distance_frequencies)?;
        let result = self.push(PendingState {
            tokens,
            token_hash,
            literal_lengths,
            distance_lengths,
            literal_frequencies,
            distance_frequencies,
            extra_bits,
            depth,
            token_payload_charge: TokenPayloadCharge::Shared,
        })?;
        let memo = TransformMemo {
            state: result,
            changed: result != source,
        };
        self.states[source].transforms[kind.index()] = Some(memo);
        Some(memo)
    }

    fn transform_pruned(&mut self, source: StateId, plain: &[u8]) -> Option<TransformMemo> {
        self.transform_marked(source, plain, TransformKind::Pruned, true)
    }

    fn transform_least(
        &mut self,
        source: StateId,
        plain: &[u8],
        least_seen: bool,
    ) -> Option<TransformMemo> {
        let kind = if least_seen {
            TransformKind::LeastSeen
        } else {
            TransformKind::LeastExpensive
        };
        if let Some(result) = self.states[source].transforms[kind.index()] {
            return Some(result);
        }

        // The least-family route will need a byte-per-token mark vector and a
        // new queued state. Bail out before its full decoded-data scan when the
        // remaining arena cannot hold even those fixed costs.
        let expansion_budget = self
            .remaining_bytes()
            .checked_sub(size_of::<Deft4jState>())?;
        if self.states[source].tokens.len() > expansion_budget {
            return None;
        }
        let choices = match self.states[source].least_families {
            LeastFamilyMemo::Unscored => {
                let state = &self.states[source];
                let choices = least_length_families(
                    &state.tokens,
                    plain,
                    &state.literal_lengths,
                    &state.distance_lengths,
                )
                // The old single-selector helper treated an inconsistent
                // internal token/plain pair as "no family". Retain that safe
                // no-op behavior while caching the result for both selectors.
                .unwrap_or(LeastFamilyChoices {
                    least_expensive: None,
                    least_seen: None,
                });
                self.states[source].least_families = LeastFamilyMemo::Scored(choices);
                choices
            }
            LeastFamilyMemo::Scored(choices) => choices,
        };
        let chosen = if least_seen {
            choices.least_seen
        } else {
            choices.least_expensive
        };
        let other_kind = if least_seen {
            TransformKind::LeastExpensive
        } else {
            TransformKind::LeastSeen
        };
        let Some(symbol) = chosen else {
            let memo = TransformMemo {
                state: source,
                changed: false,
            };
            self.states[source].transforms[kind.index()] = Some(memo);
            if choices.least_expensive == choices.least_seen {
                self.states[source].transforms[other_kind.index()] = Some(memo);
            }
            return Some(memo);
        };
        // `expand_where` temporarily owns both its byte-per-token mark vector
        // and the expanded token payload. Leave room for the queued state's
        // inline metadata as well, so rejection happens before either large
        // allocation rather than after `push` observes the arena limit.
        let state = &self.states[source];
        let expanded = expand_where(
            &state.tokens,
            plain,
            |token| matches!(token, Token::Match { length_symbol, .. } if length_symbol == symbol),
            expansion_budget,
        )?;
        let literal_lengths = state.literal_lengths;
        let distance_lengths = state.distance_lengths;
        let depth = state.depth.checked_add(1)?;
        let result = self.push(PendingState {
            tokens: expanded.tokens,
            token_hash: expanded.token_hash,
            literal_lengths,
            distance_lengths,
            literal_frequencies: expanded.literal_frequencies,
            distance_frequencies: expanded.distance_frequencies,
            extra_bits: expanded.extra_bits,
            depth,
            token_payload_charge: TokenPayloadCharge::NewlyAllocated,
        })?;
        let memo = TransformMemo {
            state: result,
            changed: true,
        };
        self.states[source].transforms[kind.index()] = Some(memo);
        if choices.least_expensive == choices.least_seen {
            // Identical family choices have identical token output and retain
            // the same data-code tables. Share the queued state as well as the
            // analysis without changing which named route is visited first.
            self.states[source].transforms[other_kind.index()] = Some(memo);
        }
        Some(memo)
    }

    fn transform_marked(
        &mut self,
        source: StateId,
        plain: &[u8],
        kind: TransformKind,
        rebuild_deft4j_lengths: bool,
    ) -> Option<TransformMemo> {
        if let Some(result) = self.states[source].transforms[kind.index()] {
            return Some(result);
        }
        let state = &self.states[source];
        let include_equal = matches!(kind, TransformKind::Pruned);
        let transform_budget = self
            .remaining_bytes()
            .checked_sub(size_of::<Deft4jState>())?;
        let marks = mark_cost_expansions(
            &state.tokens,
            plain,
            &state.literal_lengths,
            &state.distance_lengths,
            include_equal,
            transform_budget,
        )?;
        let changed = marks.iter().any(|&mark| mark != 0);
        if !changed && !rebuild_deft4j_lengths {
            let memo = TransformMemo {
                state: source,
                changed: false,
            };
            self.states[source].transforms[kind.index()] = Some(memo);
            return Some(memo);
        }

        let state = &self.states[source];
        let (
            tokens,
            token_hash,
            literal_frequencies,
            distance_frequencies,
            extra_bits,
            token_payload_charge,
        ) = if changed {
            let payload_budget = transform_budget.checked_sub(marks.len())?;
            let expanded = expand_marked(&state.tokens, plain, &marks, payload_budget)?;
            (
                expanded.tokens,
                expanded.token_hash,
                expanded.literal_frequencies,
                expanded.distance_frequencies,
                expanded.extra_bits,
                TokenPayloadCharge::NewlyAllocated,
            )
        } else {
            (
                Arc::clone(&state.tokens),
                state.token_hash,
                state.literal_frequencies,
                state.distance_frequencies,
                state.extra_bits,
                TokenPayloadCharge::Shared,
            )
        };
        // The transform decision is fully represented by `tokens` now. Drop
        // the byte-per-token marks before building deft4j tables or inserting
        // the state so they do not overlap those later allocations.
        drop(marks);
        let (literal_lengths, distance_lengths) = if rebuild_deft4j_lengths {
            columbo_deft4j_lengths(&literal_frequencies, &distance_frequencies)?
        } else {
            (state.literal_lengths, state.distance_lengths)
        };
        let depth = state.depth.checked_add(1)?;
        let result = self.push(PendingState {
            tokens,
            token_hash,
            literal_lengths,
            distance_lengths,
            literal_frequencies,
            distance_frequencies,
            extra_bits,
            depth,
            token_payload_charge,
        })?;
        let memo = TransformMemo {
            state: result,
            changed,
        };
        self.states[source].transforms[kind.index()] = Some(memo);
        Some(memo)
    }
}

fn state_hash(token_hash: u64, literal_lengths: &[u8; 286], distance_lengths: &[u8; 30]) -> u64 {
    let mut hasher = DefaultHasher::new();
    token_hash.hash(&mut hasher);
    literal_lengths.hash(&mut hasher);
    distance_lengths.hash(&mut hasher);
    hasher.finish()
}

fn token_hash(tokens: &[Token]) -> u64 {
    let mut hasher = DefaultHasher::new();
    tokens.len().hash(&mut hasher);
    for token in tokens {
        token.hash(&mut hasher);
    }
    hasher.finish()
}

fn state_matches(
    state: &Deft4jState,
    tokens: &Arc<Vec<Token>>,
    literal_lengths: &[u8; 286],
    distance_lengths: &[u8; 30],
) -> bool {
    state.literal_lengths == *literal_lengths
        && state.distance_lengths == *distance_lengths
        && (Arc::ptr_eq(&state.tokens, tokens) || state.tokens.as_slice() == tokens.as_slice())
}

/// Build Columbo's deft4j-derived payload trees.
///
/// Both alphabets use the recovered `HuffmanTree`/`PriorityQueue` path. The
/// beta-17 payload caller bypasses that path for zero or one used distance
/// symbol; retaining the original Columbo C implementation's two-leaf spelling
/// is a deliberate size-preserving extension rather than deft4j parity.
fn columbo_deft4j_lengths(
    literal_frequencies: &[u32; 286],
    distance_frequencies: &[u32; 30],
) -> Option<([u8; 286], [u8; 30])> {
    let literal = make_lengths_deft4j_java_heap(literal_frequencies, 15);
    let distance = make_lengths_deft4j_java_heap(distance_frequencies, 15);
    if literal.len() != 286 || distance.len() != 30 {
        return None;
    }
    let mut literal_lengths = [0_u8; 286];
    let mut distance_lengths = [0_u8; 30];
    literal_lengths.copy_from_slice(&literal);
    distance_lengths.copy_from_slice(&distance);
    // Frequencies produced by the parser already include EOB. Keep the guard
    // explicit because queue states are also constructed from transformations.
    if literal_lengths[256] == 0 {
        literal_lengths[256] = 1;
    }
    Some((literal_lengths, distance_lengths))
}

struct ExpandedState {
    tokens: Arc<Vec<Token>>,
    token_hash: u64,
    literal_frequencies: [u32; 286],
    distance_frequencies: [u32; 30],
    extra_bits: u64,
}

fn mark_cost_expansions(
    tokens: &[Token],
    plain: &[u8],
    literal_lengths: &[u8; 286],
    distance_lengths: &[u8; 30],
    include_equal: bool,
    max_mark_bytes: usize,
) -> Option<Vec<u8>> {
    if tokens.len() > max_mark_bytes {
        return None;
    }
    let mut marks = Vec::new();
    marks.try_reserve_exact(tokens.len()).ok()?;
    let mut offset = 0_usize;
    for &token in tokens {
        let end = offset.checked_add(token.decoded_len())?;
        let decoded = plain.get(offset..end)?;
        let mark = match token {
            Token::Literal(_) => false,
            Token::Match { .. } => {
                match_expansion_delta(token, decoded, literal_lengths, distance_lengths)
                    .is_some_and(|delta| delta < 0 || (include_equal && delta == 0))
            }
        };
        marks.push(u8::from(mark));
        offset = end;
    }
    (offset == plain.len()).then_some(marks)
}

fn match_expansion_delta(
    token: Token,
    decoded: &[u8],
    literal_lengths: &[u8; 286],
    distance_lengths: &[u8; 30],
) -> Option<i64> {
    let Token::Match {
        length_symbol,
        distance_symbol,
        length_extra_bits,
        distance_extra_bits,
        ..
    } = token
    else {
        return None;
    };
    let match_literal = literal_lengths.get(usize::from(length_symbol)).copied()?;
    let match_distance = distance_lengths
        .get(usize::from(distance_symbol))
        .copied()?;
    if match_literal == 0 || match_distance == 0 {
        return None;
    }
    let mut literal_bits = 0_u64;
    for &byte in decoded {
        let length = literal_lengths[usize::from(byte)];
        if length == 0 {
            return None;
        }
        literal_bits = literal_bits.checked_add(u64::from(length))?;
    }
    let match_bits = u64::from(match_literal)
        .checked_add(u64::from(match_distance))?
        .checked_add(u64::from(length_extra_bits))?
        .checked_add(u64::from(distance_extra_bits))?;
    Some(literal_bits as i64 - match_bits as i64)
}

fn least_length_families(
    tokens: &[Token],
    plain: &[u8],
    literal_lengths: &[u8; 286],
    distance_lengths: &[u8; 30],
) -> Option<LeastFamilyChoices> {
    let mut deltas = [0_i64; 29];
    let mut counts = [0_u32; 29];
    let mut invalid = [false; 29];
    let mut offset = 0_usize;
    for &token in tokens {
        let end = offset.checked_add(token.decoded_len())?;
        let decoded = plain.get(offset..end)?;
        if let Token::Match { length_symbol, .. } = token {
            if (257..=285).contains(&length_symbol) {
                let family = usize::from(length_symbol - 257);
                counts[family] = counts[family].checked_add(1)?;
                match match_expansion_delta(token, decoded, literal_lengths, distance_lengths) {
                    Some(delta) => deltas[family] = deltas[family].checked_add(delta)?,
                    None => invalid[family] = true,
                }
            }
        }
        offset = end;
    }
    if offset != plain.len() {
        return None;
    }

    let mut least_expensive = None;
    let mut least_seen = None;
    for family in 0..29 {
        if counts[family] == 0 || invalid[family] {
            continue;
        }
        let improves_expense = match least_expensive {
            Some(current) => deltas[family] < deltas[current],
            None => true,
        };
        if improves_expense {
            least_expensive = Some(family);
        }
        let improves_count = match least_seen {
            Some(current) => counts[family] < counts[current],
            None => true,
        };
        if improves_count {
            least_seen = Some(family);
        }
    }
    Some(LeastFamilyChoices {
        least_expensive: least_expensive.map(|family| family as u16 + 257),
        least_seen: least_seen.map(|family| family as u16 + 257),
    })
}

fn expand_where(
    tokens: &[Token],
    plain: &[u8],
    should_expand: impl Fn(Token) -> bool,
    max_additional_bytes: usize,
) -> Option<ExpandedState> {
    // The mark vector stays live while `expand_marked` allocates its output.
    // Debit it first so their combined peak remains within the caller's arena.
    if tokens.len() > max_additional_bytes {
        return None;
    }
    let mut marks = Vec::new();
    marks.try_reserve_exact(tokens.len()).ok()?;
    marks.extend(
        tokens
            .iter()
            .copied()
            .map(|token| u8::from(matches!(token, Token::Match { .. }) && should_expand(token))),
    );
    expand_marked(
        tokens,
        plain,
        &marks,
        max_additional_bytes.checked_sub(marks.len())?,
    )
}

fn expand_marked(
    tokens: &[Token],
    plain: &[u8],
    marks: &[u8],
    max_token_payload_bytes: usize,
) -> Option<ExpandedState> {
    if marks.len() != tokens.len() {
        return None;
    }
    let mut output_count = 0_usize;
    let mut decoded_count = 0_usize;
    for (&token, &mark) in tokens.iter().zip(marks) {
        let decoded = token.decoded_len();
        decoded_count = decoded_count.checked_add(decoded)?;
        output_count = output_count.checked_add(if mark != 0 { decoded } else { 1 })?;
    }
    let output_bytes = token_payload_bytes(output_count)?;
    if output_bytes > max_token_payload_bytes
        || decoded_count != plain.len()
        || parsed_model_bytes(plain.len(), output_count, 1)? > MAX_PARSED_MODEL_BYTES
    {
        return None;
    }

    let mut output = Vec::new();
    output.try_reserve_exact(output_count).ok()?;
    let mut literal_frequencies = [0_u32; 286];
    let mut distance_frequencies = [0_u32; 30];
    let mut extra_bits = 0_u64;
    let mut hasher = DefaultHasher::new();
    output_count.hash(&mut hasher);
    let mut offset = 0_usize;
    for (&token, &mark) in tokens.iter().zip(marks) {
        let end = offset.checked_add(token.decoded_len())?;
        let decoded = plain.get(offset..end)?;
        if mark != 0 {
            for &byte in decoded {
                let literal = Token::Literal(byte);
                literal.hash(&mut hasher);
                literal_frequencies[usize::from(byte)] =
                    literal_frequencies[usize::from(byte)].checked_add(1)?;
                output.push(literal);
            }
        } else {
            token.hash(&mut hasher);
            match token {
                Token::Literal(byte) => {
                    literal_frequencies[usize::from(byte)] =
                        literal_frequencies[usize::from(byte)].checked_add(1)?;
                }
                Token::Match {
                    length_symbol,
                    distance_symbol,
                    length_extra_bits,
                    distance_extra_bits,
                    ..
                } => {
                    literal_frequencies[usize::from(length_symbol)] =
                        literal_frequencies[usize::from(length_symbol)].checked_add(1)?;
                    distance_frequencies[usize::from(distance_symbol)] =
                        distance_frequencies[usize::from(distance_symbol)].checked_add(1)?;
                    extra_bits = extra_bits
                        .checked_add(u64::from(length_extra_bits))?
                        .checked_add(u64::from(distance_extra_bits))?;
                }
            }
            output.push(token);
        }
        offset = end;
    }
    literal_frequencies[256] = literal_frequencies[256].checked_add(1)?;
    Some(ExpandedState {
        tokens: Arc::new(output),
        token_hash: hasher.finish(),
        literal_frequencies,
        distance_frequencies,
        extra_bits,
    })
}

struct Deft4jBest {
    plan: PlannedBlock,
    improved: bool,
}

struct Deft4jPipeline {
    queue: Deft4jQueue,
    plain: Arc<Vec<u8>>,
    strict: bool,
}

impl Deft4jPipeline {
    fn submit(&mut self, state: StateId, best: &mut Deft4jBest) {
        let Some(candidate) = self
            .queue
            .score(state, ScoreKind::CompleteHeader, self.strict)
        else {
            return;
        };
        if candidate.bits >= best.plan.bits {
            return;
        }
        let Some(dynamic) = candidate.try_clone() else {
            return;
        };
        best.plan = PlannedBlock {
            tokens: Arc::clone(&self.queue.states[state].tokens),
            plain: Arc::clone(&self.plain),
            bits: dynamic.bits,
            representation: Representation::Dynamic(dynamic),
            source_type: SourceBlockType::Dynamic,
        };
        best.improved = true;
    }

    fn pruned_fixed_point(
        &mut self,
        source: StateId,
        stop: &mut SearchStop<'_>,
    ) -> Option<TransformMemo> {
        let mut current = source;
        let mut current_bits = self
            .queue
            .score(current, ScoreKind::DefaultHeaderFixedPoint, self.strict)?
            .bits;
        let mut changed = false;
        while !stop.reached() && !self.queue.saturated {
            let next = self.queue.transform_pruned(current, &self.plain)?;
            if next.state == current {
                break;
            }
            let Some(plan) =
                self.queue
                    .score(next.state, ScoreKind::DefaultHeaderFixedPoint, self.strict)
            else {
                break;
            };
            if plan.bits >= current_bits {
                break;
            }
            current = next.state;
            current_bits = plan.bits;
            changed = true;
        }
        Some(TransformMemo {
            state: current,
            changed,
        })
    }

    fn add_optimized_recoded(
        &mut self,
        source: StateId,
        best: &mut Deft4jBest,
        stop: &mut SearchStop<'_>,
    ) -> bool {
        let Some(strict) = self.queue.transform_strict(source, &self.plain) else {
            return false;
        };
        self.submit(strict.state, best);
        if stop.reached() || self.queue.saturated {
            return false;
        }

        let Some(recoded) = self.queue.transform_recode(source) else {
            return false;
        };
        let Some(strict_recoded) = self.queue.transform_strict(recoded.state, &self.plain) else {
            return false;
        };
        self.submit(strict_recoded.state, best);
        if stop.reached() || self.queue.saturated {
            return false;
        }

        let Some(pruned) = self.queue.transform_pruned(source, &self.plain) else {
            return false;
        };
        let Some(strict_pruned) = self.queue.transform_strict(pruned.state, &self.plain) else {
            return false;
        };
        self.submit(strict_pruned.state, best);
        if stop.reached() || self.queue.saturated {
            return false;
        }

        let Some(full) = self.pruned_fixed_point(pruned.state, stop) else {
            return false;
        };
        if full.changed {
            let Some(strict_full) = self.queue.transform_strict(full.state, &self.plain) else {
                return false;
            };
            self.submit(strict_full.state, best);
        }
        true
    }

    fn run_optimizations(
        &mut self,
        source: StateId,
        best: &mut Deft4jBest,
        stop: &mut SearchStop<'_>,
    ) -> bool {
        if !self.add_optimized_recoded(source, best, stop) {
            return false;
        }
        if stop.reached() || self.queue.saturated {
            return false;
        }
        let Some(least) = self.queue.transform_least(source, &self.plain, false) else {
            return false;
        };
        if !self.add_optimized_recoded(least.state, best, stop) {
            return false;
        }
        if stop.reached() || self.queue.saturated {
            return false;
        }
        let Some(least_seen) = self.queue.transform_least(source, &self.plain, true) else {
            return false;
        };
        self.add_optimized_recoded(least_seen.state, best, stop)
    }

    fn run_multi(
        &mut self,
        source: StateId,
        best: &mut Deft4jBest,
        stop: &mut SearchStop<'_>,
    ) -> bool {
        self.submit(source, best);
        if !self.run_optimizations(source, best, stop) {
            return false;
        }
        if stop.reached() || self.queue.saturated {
            return false;
        }

        let Some(recoded) = self.queue.transform_recode(source) else {
            return false;
        };
        self.submit(recoded.state, best);
        if !self.run_optimizations(recoded.state, best, stop) {
            return false;
        }
        if stop.reached() || self.queue.saturated {
            return false;
        }

        let Some(pruned) = self.queue.transform_pruned(source, &self.plain) else {
            return false;
        };
        self.submit(pruned.state, best);
        if !self.run_optimizations(pruned.state, best, stop) {
            return false;
        }
        if stop.reached() || self.queue.saturated {
            return false;
        }

        let Some(full) = self.pruned_fixed_point(pruned.state, stop) else {
            return false;
        };
        if full.changed {
            self.submit(full.state, best);
            if !self.run_optimizations(full.state, best, stop) {
                return false;
            }
        }
        true
    }

    fn run_ordered(&mut self, base: StateId, best: &mut Deft4jBest, stop: &mut SearchStop<'_>) {
        if !self.run_multi(base, best, stop) || stop.reached() || self.queue.saturated {
            return;
        }
        let Some(strict) = self.queue.transform_strict(base, &self.plain) else {
            return;
        };
        if strict.changed
            && (!self.run_multi(strict.state, best, stop) || stop.reached() || self.queue.saturated)
        {
            return;
        }
        let Some(least) = self.queue.transform_least(base, &self.plain, false) else {
            return;
        };
        if !self.run_multi(least.state, best, stop) || stop.reached() || self.queue.saturated {
            return;
        }
        let Some(least_seen) = self.queue.transform_least(base, &self.plain, true) else {
            return;
        };
        let _ = self.run_multi(least_seen.state, best, stop);
    }
}

struct BlockOutcome {
    plan: PlannedBlock,
    improved: bool,
}

fn plan_source_block_once(
    block: &ParsedBlock,
    alignment: u8,
    options: &Options,
    available_bytes: usize,
    stop: &mut SearchStop<'_>,
) -> BlockOutcome {
    let mut best = Deft4jBest {
        plan: initial_plan(block, alignment, options),
        improved: false,
    };

    if block.source_type != SourceBlockType::Stored {
        let bits = stored_block_bits(alignment, block.plain.len());
        if bits < best.plan.bits {
            best.plan = PlannedBlock {
                tokens: Arc::clone(&block.tokens),
                plain: Arc::clone(&block.plain),
                representation: Representation::Stored,
                bits,
                source_type: SourceBlockType::Stored,
            };
            best.improved = true;
        }
    }
    if block.source_type == SourceBlockType::Dynamic {
        if let Some(bits) = fixed_block_bits(&block.tokens) {
            if bits < best.plan.bits {
                best.plan = PlannedBlock {
                    tokens: Arc::clone(&block.tokens),
                    plain: Arc::clone(&block.plain),
                    representation: Representation::Fixed,
                    bits,
                    source_type: SourceBlockType::Fixed,
                };
                best.improved = true;
            }
        }
    }

    // Both deft4j and Defluff price a fixed block after expanding every match
    // that is strictly dearer than its decoded literals in the fixed table.
    if block.source_type != SourceBlockType::Stored {
        let (fixed_literal, fixed_distance) = fixed_lengths();
        if let Some(marks) = mark_cost_expansions(
            &block.tokens,
            &block.plain,
            &fixed_literal,
            &fixed_distance,
            false,
            available_bytes,
        ) {
            if marks.iter().any(|&mark| mark != 0) {
                let payload_budget = available_bytes.saturating_sub(marks.len());
                if let Some(expanded) =
                    expand_marked(&block.tokens, &block.plain, &marks, payload_budget)
                {
                    if let Some(bits) = fixed_block_bits(&expanded.tokens) {
                        if bits < best.plan.bits {
                            best.plan = PlannedBlock {
                                tokens: expanded.tokens,
                                plain: Arc::clone(&block.plain),
                                representation: Representation::Fixed,
                                bits,
                                source_type: SourceBlockType::Fixed,
                            };
                            best.improved = true;
                        }
                    }
                }
            }
        }
    }

    if matches!(
        block.source_type,
        SourceBlockType::Fixed | SourceBlockType::Dynamic
    ) && !stop.reached()
    {
        // A fixed-strict winner above remains alive while the deft4j state queue
        // runs. Reserve its payload from the queue's arena so both cannot each
        // consume the full remaining route budget at the same time.
        let retained_best_bytes = if Arc::ptr_eq(&best.plan.tokens, &block.tokens) {
            0
        } else {
            token_payload_bytes(best.plan.tokens.len()).unwrap_or(available_bytes)
        };
        let queue_limit = available_bytes.saturating_sub(retained_best_bytes);
        let seed = if block.source_type == SourceBlockType::Dynamic
            && block.original.is_some()
            && block.original_dynamic.is_some()
        {
            block
                .original_dynamic
                .as_ref()
                .and_then(dynamic_length_arrays)
        } else {
            columbo_deft4j_lengths(&block.literal_frequencies, &block.distance_frequencies)
        };
        if let Some((literal_lengths, distance_lengths)) = seed {
            // Both ordered seeds keep the source token stream. Hash it once;
            // the queue still compares token contents on a hash collision.
            // Their token extra-bit total is likewise identical.
            let source_token_hash = token_hash(&block.tokens);
            let source_extra_bits = token_extra_bits(&block.tokens);
            let mut pipeline = Deft4jPipeline {
                queue: Deft4jQueue::new(queue_limit),
                plain: Arc::clone(&block.plain),
                strict: options.strict,
            };
            if let Some(base) = pipeline.queue.push(PendingState {
                tokens: Arc::clone(&block.tokens),
                token_hash: source_token_hash,
                literal_lengths,
                distance_lengths,
                literal_frequencies: block.literal_frequencies,
                distance_frequencies: block.distance_frequencies,
                extra_bits: source_extra_bits,
                depth: 0,
                token_payload_charge: TokenPayloadCharge::Shared,
            }) {
                pipeline.run_ordered(base, &mut best, stop);

                // The source table and deft4j recodes form the source-derived
                // path. Columbo's no-split sibling also keeps the original
                // token stream and starts the same ordered graph from the
                // independently best completed table. This is a genuinely
                // different state: using the winning tokens here would merely
                // repeat the fixed-point adoption performed by `WorkingBlock`.
                // Reuse this queue so shared states are deduplicated rather
                // than rescanning them in a second complete route.
                if !stop.reached() && !pipeline.queue.saturated {
                    if let Some((alternate_literal, alternate_distance)) =
                        alternate_seed_lengths(&best.plan, &literal_lengths, &distance_lengths)
                    {
                        if let Some(alternate_base) = pipeline.queue.push(PendingState {
                            tokens: Arc::clone(&block.tokens),
                            token_hash: source_token_hash,
                            literal_lengths: alternate_literal,
                            distance_lengths: alternate_distance,
                            literal_frequencies: block.literal_frequencies,
                            distance_frequencies: block.distance_frequencies,
                            extra_bits: source_extra_bits,
                            depth: 0,
                            token_payload_charge: TokenPayloadCharge::Shared,
                        }) {
                            pipeline.run_ordered(alternate_base, &mut best, stop);
                        }
                    }
                }
            }
        }
    }

    BlockOutcome {
        plan: best.plan,
        improved: best.improved,
    }
}

fn alternate_seed_lengths(
    best: &PlannedBlock,
    source_literal: &[u8; 286],
    source_distance: &[u8; 30],
) -> Option<([u8; 286], [u8; 30])> {
    let alternate = match &best.representation {
        Representation::Dynamic(dynamic) => dynamic_length_arrays(dynamic),
        Representation::Fixed => Some(fixed_lengths()),
        Representation::Original(_) | Representation::Stored => None,
    }?;
    (alternate.0 != *source_literal || alternate.1 != *source_distance).then_some(alternate)
}

fn initial_plan(block: &ParsedBlock, alignment: u8, options: &Options) -> PlannedBlock {
    if let Some(original) = reusable_original_bits(block, alignment, options.strict) {
        return PlannedBlock {
            tokens: Arc::clone(&block.tokens),
            plain: Arc::clone(&block.plain),
            representation: Representation::Original(original),
            bits: original.len,
            source_type: block.source_type,
        };
    }

    let representation = match block.source_type {
        SourceBlockType::Stored => Representation::Stored,
        SourceBlockType::Fixed => Representation::Fixed,
        SourceBlockType::Dynamic => block
            .original_dynamic
            .as_ref()
            .and_then(dynamic_length_arrays)
            .and_then(|(literal, distance)| {
                plan_for_deft4j_lengths_with_cost(
                    &block.literal_frequencies,
                    &block.distance_frequencies,
                    token_extra_bits(&block.tokens),
                    &literal,
                    &distance,
                    options.strict,
                    Deft4jHeaderPolicy::Complete,
                )
            })
            .map(Representation::Dynamic)
            .unwrap_or(Representation::Fixed),
    };
    let bits = representation_bits(&representation, block, alignment).unwrap_or(u64::MAX);
    PlannedBlock {
        tokens: Arc::clone(&block.tokens),
        plain: Arc::clone(&block.plain),
        representation,
        bits,
        source_type: block.source_type,
    }
}

/// Return a valid spelling of the block's current object without running any
/// match-expansion or header-search work.
///
/// This is used only to finish a source-route candidate after its deadline.
/// Original bits are preferred whenever they remain safe for alignment and
/// compatibility. An adopted dynamic object already owns its completed header;
/// otherwise a fixed spelling is the cheap universal fallback for Huffman
/// tokens.
fn cheap_current_plan(
    block: &ParsedBlock,
    alignment: u8,
    options: &Options,
) -> Option<PlannedBlock> {
    if let Some(original) = reusable_original_bits(block, alignment, options.strict) {
        return Some(PlannedBlock {
            tokens: Arc::clone(&block.tokens),
            plain: Arc::clone(&block.plain),
            representation: Representation::Original(original),
            bits: original.len,
            source_type: block.source_type,
        });
    }

    let representation = match block.source_type {
        SourceBlockType::Stored => Representation::Stored,
        SourceBlockType::Fixed => Representation::Fixed,
        SourceBlockType::Dynamic => {
            let compatible_dynamic = block.original_dynamic.as_ref().filter(|dynamic| {
                !options.strict || dynamic.has_strictly_compatible_huffman_codes()
            });
            match compatible_dynamic {
                Some(dynamic) => Representation::Dynamic(dynamic.try_clone()?),
                None => Representation::Fixed,
            }
        }
    };
    let bits = representation_bits(&representation, block, alignment)?;
    Some(PlannedBlock {
        tokens: Arc::clone(&block.tokens),
        plain: Arc::clone(&block.plain),
        representation,
        bits,
        source_type: block.source_type,
    })
}

fn representation_bits(
    representation: &Representation,
    block: &ParsedBlock,
    alignment: u8,
) -> Option<u64> {
    match representation {
        Representation::Original(original) => Some(original.len),
        Representation::Stored => Some(stored_block_bits(alignment, block.plain.len())),
        Representation::Fixed => fixed_block_bits(&block.tokens),
        Representation::Dynamic(dynamic) => Some(dynamic.bits),
    }
}

fn dynamic_length_arrays(dynamic: &DynamicPlan) -> Option<([u8; 286], [u8; 30])> {
    if dynamic.literal_lengths.len() > 286 || dynamic.distance_lengths.len() > 30 {
        return None;
    }
    let mut literal = [0_u8; 286];
    let mut distance = [0_u8; 30];
    literal[..dynamic.literal_lengths.len()].copy_from_slice(&dynamic.literal_lengths);
    distance[..dynamic.distance_lengths.len()].copy_from_slice(&dynamic.distance_lengths);
    Some((literal, distance))
}

fn fixed_lengths() -> ([u8; 286], [u8; 30]) {
    let mut literal = [0_u8; 286];
    literal.copy_from_slice(&FIXED_LITERAL_CODE_LENGTHS[..286]);
    let mut distance = [0_u8; 30];
    distance.copy_from_slice(&FIXED_DISTANCE_CODE_LENGTHS[..30]);
    (literal, distance)
}

struct WorkingBlock {
    block: ParsedBlock,
    plans: [Option<PlannedBlock>; 8],
    /// deft4j-route allocations owned by this block. Source `Arc`s are shared
    /// with the parser and therefore start at zero; transformed tokens and
    /// merged payloads are charged here until this block is dropped/replaced.
    token_payload_bytes: usize,
    other_model_bytes: usize,
}

impl WorkingBlock {
    fn new(block: ParsedBlock) -> Self {
        Self {
            block,
            plans: array::from_fn(|_| None),
            token_payload_bytes: 0,
            other_model_bytes: 0,
        }
    }

    fn new_owned(block: ParsedBlock, token_payload_bytes: usize, other_model_bytes: usize) -> Self {
        Self {
            block,
            plans: array::from_fn(|_| None),
            token_payload_bytes,
            other_model_bytes,
        }
    }

    fn accounted_bytes(&self) -> Option<usize> {
        self.token_payload_bytes.checked_add(self.other_model_bytes)
    }

    fn plan(
        &mut self,
        alignment: u8,
        options: &Options,
        budget: &mut Deft4jRouteBudget,
        stop: &mut SearchStop<'_>,
    ) -> Option<u64> {
        let slot = usize::from(alignment & 7);
        if let Some(plan) = &self.plans[slot] {
            return Some(plan.bits);
        }

        loop {
            let outcome =
                plan_source_block_once(&self.block, alignment, options, budget.remaining(), stop);
            let bits = outcome.plan.bits;
            if !outcome.improved {
                self.plans[slot] = Some(outcome.plan);
                return Some(bits);
            }

            let replaces_tokens = !Arc::ptr_eq(&self.block.tokens, &outcome.plan.tokens);
            let new_token_bytes = if replaces_tokens {
                token_payload_bytes(outcome.plan.tokens.len())?
            } else {
                0
            };
            if replaces_tokens {
                // The old and new token buffers coexist until adoption. Charge
                // the new allocation first, then release the old one only after
                // every cache/reference owned by this block has been dropped.
                budget.reserve(new_token_bytes)?;
            }
            if adopt_plan(&mut self.block, &outcome.plan).is_none() {
                if replaces_tokens {
                    let _ = budget.release(new_token_bytes);
                }
                return None;
            }
            self.plans = array::from_fn(|_| None);
            if replaces_tokens {
                budget.release(self.token_payload_bytes)?;
                self.token_payload_bytes = new_token_bytes;
            }
            if stop.reached() {
                // The adopted plan is complete and remains a valid emission
                // candidate even though another fixed-point pass cannot start.
                self.plans[slot] = Some(outcome.plan);
                return Some(bits);
            }
        }
    }

    fn take_plan(
        &mut self,
        alignment: u8,
        options: &Options,
        budget: &mut Deft4jRouteBudget,
        stop: &mut SearchStop<'_>,
    ) -> Option<PlannedBlock> {
        let slot = usize::from(alignment & 7);
        if self.plans[slot].is_none() && stop.reached() {
            // An untouched tail must not run fixed-strict expansion after the
            // deadline. Emit the current block object faithfully instead; this
            // keeps all earlier source-route wins without another full model scan.
            return cheap_current_plan(&self.block, alignment, options);
        }
        self.plan(alignment, options, budget, stop)?;
        self.plans[slot].take()
    }
}

fn adopt_plan(block: &mut ParsedBlock, plan: &PlannedBlock) -> Option<()> {
    block.tokens = Arc::clone(&plan.tokens);
    block.plain = Arc::clone(&plan.plain);
    block.recount_frequencies();
    block.original = None;
    match &plan.representation {
        Representation::Original(_) => return Some(()),
        Representation::Stored => {
            block.source_type = SourceBlockType::Stored;
            block.original_literal_lengths = None;
            block.original_distance_lengths = None;
            block.original_dynamic = None;
        }
        Representation::Fixed => {
            block.source_type = SourceBlockType::Fixed;
            block.original_literal_lengths = None;
            block.original_distance_lengths = None;
            block.original_dynamic = None;
        }
        Representation::Dynamic(dynamic) => {
            let (literal, distance) = dynamic_length_arrays(dynamic)?;
            block.source_type = SourceBlockType::Dynamic;
            block.original_literal_lengths = Some(literal);
            block.original_distance_lengths = Some(distance);
            block.original_dynamic = Some(dynamic.try_clone()?);
        }
    }
    Some(())
}

/// Produce Columbo's bounded source-ordered deft4j candidate list.
///
/// The returned blocks are ready for emission in order. A deadline may leave a
/// valid partially explored list, while allocation failure abandons this
/// optional route and leaves the caller's complete fallback untouched.
pub(crate) fn plan_source_blocks(
    source: &[ParsedBlock],
    start_alignment: u8,
    options: &Options,
    stop: &mut SearchStop<'_>,
) -> Option<Vec<PlannedBlock>> {
    plan_source_blocks_with_budget(
        source,
        start_alignment,
        options,
        MAX_DEFT4J_ROUTE_BYTES,
        stop,
    )
}

/// Optimize one source block with the bounded deft4j-derived candidate graph,
/// without attempting stream-level adjacent merges.
///
/// The direct no-split route uses this completed object as an additive seed
/// for its table-feedback ladder. Keeping the helper here reuses the deft4j
/// queue's state cache without invoking the source-list route's separate
/// fixed-point adoption or reimplementing the deft4j graph in the stream
/// planner.
pub(crate) fn plan_source_block(
    source: &ParsedBlock,
    alignment: u8,
    options: &Options,
    stop: &mut SearchStop<'_>,
) -> Option<PlannedBlock> {
    Some(plan_source_block_once(source, alignment, options, MAX_DEFT4J_ROUTE_BYTES, stop).plan)
}

fn plan_source_blocks_with_budget(
    source: &[ParsedBlock],
    start_alignment: u8,
    options: &Options,
    budget_bytes: usize,
    stop: &mut SearchStop<'_>,
) -> Option<Vec<PlannedBlock>> {
    let mut blocks = prepare_source_blocks(source)?;
    if blocks.is_empty() {
        return None;
    }
    let mut budget = Deft4jRouteBudget::new(budget_bytes.min(MAX_DEFT4J_ROUTE_BYTES));

    // deft4j first optimizes every list member in source order. Columbo
    // deliberately recomputes the true alignment after every chosen block,
    // correcting beta-17's repeated-pass position-accounting quirk.
    let mut alignment = start_alignment & 7;
    for block in &mut blocks {
        if stop.reached() {
            break;
        }
        let bits = block.plan(alignment, options, &mut budget, stop)?;
        alignment = ((u64::from(alignment) + bits) & 7) as u8;
    }

    // deft4j's `mergeBlocks()` is deliberately greedy. An accepted pair stays
    // at the same list index and is immediately retried against its neighbour.
    alignment = start_alignment & 7;
    let mut index = 0;
    while index + 1 < blocks.len() && !stop.reached() {
        let current_bits = blocks[index].plan(alignment, options, &mut budget, stop)?;
        let next_alignment = ((u64::from(alignment) + current_bits) & 7) as u8;
        let next_bits = blocks[index + 1].plan(next_alignment, options, &mut budget, stop)?;

        let left_type = blocks[index].block.source_type;
        let right_type = blocks[index + 1].block.source_type;
        let huffman_merge = is_huffman(left_type) && is_huffman(right_type);
        let stored_merge = left_type == SourceBlockType::Stored
            && blocks[index]
                .block
                .plain
                .len()
                .checked_add(blocks[index + 1].block.plain.len())
                .is_some_and(|length| length <= 65_535);

        if huffman_merge || stored_merge {
            if let Some(merged) = merge_blocks(
                &blocks[index].block,
                &blocks[index + 1].block,
                if stored_merge {
                    SourceBlockType::Stored
                } else {
                    SourceBlockType::Fixed
                },
                budget.remaining(),
            ) {
                budget.reserve(merged.accounted_bytes()?)?;
                let mut merged = WorkingBlock::new_owned(
                    merged.block,
                    merged.token_payload_bytes,
                    merged.other_model_bytes,
                );
                let merged_bits = merged.plan(alignment, options, &mut budget, stop)?;
                if merged_bits < current_bits.checked_add(next_bits)? {
                    let replaced_bytes = blocks[index]
                        .accounted_bytes()?
                        .checked_add(blocks[index + 1].accounted_bytes()?)?;
                    blocks[index] = merged;
                    blocks.remove(index + 1);
                    budget.release(replaced_bytes)?;
                    continue;
                }
                let discarded_bytes = merged.accounted_bytes()?;
                drop(merged);
                budget.release(discarded_bytes)?;
            }
        }

        alignment = next_alignment;
        index += 1;
    }

    let mut output = Vec::new();
    output.try_reserve_exact(blocks.len()).ok()?;
    alignment = start_alignment & 7;
    for block in &mut blocks {
        let plan = block.take_plan(alignment, options, &mut budget, stop)?;
        alignment = ((u64::from(alignment) + plan.bits) & 7) as u8;
        output.push(plan);
    }
    Some(output)
}

fn prepare_source_blocks(source: &[ParsedBlock]) -> Option<Vec<WorkingBlock>> {
    // Columbo removes every redundant empty block while retaining one legal
    // block for an all-empty stream. beta-17 intends this behavior but its
    // cleared linked-list successor stops each source walk after one removal.
    let nonempty = source
        .iter()
        .filter(|block| !block.plain.is_empty())
        .count();
    let mut output = Vec::new();
    output.try_reserve_exact(nonempty.max(1)).ok()?;
    if nonempty == 0 {
        if let Some(block) = source.last() {
            output.push(WorkingBlock::new(block.try_clone_shared()?));
        }
        return Some(output);
    }
    for block in source.iter().filter(|block| !block.plain.is_empty()) {
        output.push(WorkingBlock::new(block.try_clone_shared()?));
    }
    Some(output)
}

fn is_huffman(block_type: SourceBlockType) -> bool {
    matches!(
        block_type,
        SourceBlockType::Fixed | SourceBlockType::Dynamic
    )
}

struct MergedBlock {
    block: ParsedBlock,
    token_payload_bytes: usize,
    other_model_bytes: usize,
}

impl MergedBlock {
    fn accounted_bytes(&self) -> Option<usize> {
        self.token_payload_bytes.checked_add(self.other_model_bytes)
    }
}

fn merge_blocks(
    left: &ParsedBlock,
    right: &ParsedBlock,
    merged_type: SourceBlockType,
    max_additional_bytes: usize,
) -> Option<MergedBlock> {
    let token_count = left.tokens.len().checked_add(right.tokens.len())?;
    let plain_count = left.plain.len().checked_add(right.plain.len())?;
    let split_count = left
        .source_splits
        .len()
        .checked_add(right.source_splits.len())?
        .checked_add(usize::from(
            !left.plain.is_empty() && !right.plain.is_empty(),
        ))?;
    let token_payload_bytes = token_payload_bytes(token_count)?;
    let model_bytes = parsed_model_bytes(plain_count, token_count, 1)?;
    if model_bytes > MAX_PARSED_MODEL_BYTES {
        return None;
    }
    let split_bytes = split_count.checked_mul(size_of::<usize>())?;
    let accounted_bytes = model_bytes.checked_add(split_bytes)?;
    if accounted_bytes > max_additional_bytes {
        return None;
    }
    let other_model_bytes = accounted_bytes.checked_sub(token_payload_bytes)?;

    // Every size and route-budget check is complete before the payload
    // allocations. A rejected merge therefore cannot briefly exceed the live
    // deft4j-route ceiling while its two input blocks are still resident.
    let mut tokens = Vec::new();
    tokens.try_reserve_exact(token_count).ok()?;
    tokens.extend_from_slice(&left.tokens);
    tokens.extend_from_slice(&right.tokens);
    let mut plain = Vec::new();
    plain.try_reserve_exact(plain_count).ok()?;
    plain.extend_from_slice(&left.plain);
    plain.extend_from_slice(&right.plain);

    let mut source_splits = Vec::new();
    source_splits.try_reserve_exact(split_count).ok()?;
    source_splits.extend_from_slice(&left.source_splits);
    if !left.plain.is_empty() && !right.plain.is_empty() {
        source_splits.push(left.plain.len());
    }
    for &split in &right.source_splits {
        source_splits.push(left.plain.len().checked_add(split)?);
    }
    let (literal_frequencies, distance_frequencies) = count_frequencies(&tokens);
    Some(MergedBlock {
        block: ParsedBlock {
            tokens: Arc::new(tokens),
            plain: Arc::new(plain),
            literal_frequencies,
            distance_frequencies,
            original_literal_lengths: None,
            original_distance_lengths: None,
            original_dynamic: None,
            original: None,
            source_splits,
            source_type: merged_type,
        },
        token_payload_bytes,
        other_model_bytes,
    })
}

#[cfg(test)]
mod tests {
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

    fn push_fixed_state(queue: &mut Deft4jQueue, tokens: Arc<Vec<Token>>) -> StateId {
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
        assert!(all_empty[0].block.plain.is_empty());
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
    fn stored_merge_limit_is_asymmetric_and_hard() {
        let left = literal_block(&vec![1; 40_000], SourceBlockType::Stored);
        let right = literal_block(&vec![2; 30_000], SourceBlockType::Fixed);
        let mut never = SearchStop::never();
        let plans = plan_source_blocks(&[left, right], 0, &Options::default(), &mut never).unwrap();
        assert_eq!(plans.len(), 2);
    }

    #[test]
    fn completed_plan_is_reused_at_the_same_alignment() {
        let mut block =
            WorkingBlock::new(literal_block(b"alignment cache", SourceBlockType::Fixed));
        let calls = Cell::new(0_usize);
        let mut deadline = || {
            calls.set(calls.get() + 1);
            false
        };
        let mut stop = SearchStop::callback(&mut deadline);
        let mut budget = Deft4jRouteBudget::new(MAX_DEFT4J_ROUTE_BYTES);
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
        // One three-token expansion plus its temporary one-byte mark fits. Once
        // retained, only that mark byte remains and the second block cannot
        // allocate another expanded token vector.
        let budget_bytes = token_payload_bytes(3).unwrap() + 1;
        let mut never = SearchStop::never();
        let plans = plan_source_blocks_with_budget(
            &blocks,
            0,
            &Options::default(),
            budget_bytes,
            &mut never,
        )
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
            columbo_deft4j_lengths(&block.literal_frequencies, &block.distance_frequencies)
                .unwrap();
        let mut second_literal = first_literal;
        second_literal[0] = second_literal[0].saturating_add(1);
        let mut queue = Deft4jQueue::new(MAX_DEFT4J_ARENA_BYTES);
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
        different_literal[usize::from(b'a')] =
            different_literal[usize::from(b'a')].saturating_sub(1);
        assert_eq!(
            alternate_seed_lengths(&plan, &different_literal, &fixed_distance),
            Some((fixed_literal, fixed_distance))
        );
    }

    #[test]
    fn queue_identity_keeps_shared_pointer_and_content_fallbacks() {
        let block = literal_block(b"identity", SourceBlockType::Fixed);
        let mut queue = Deft4jQueue::new(MAX_DEFT4J_ARENA_BYTES);
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
        let mut queue = Deft4jQueue::new(MAX_DEFT4J_ARENA_BYTES);
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
        let mut queue = Deft4jQueue::new(MAX_DEFT4J_ARENA_BYTES);
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
}
