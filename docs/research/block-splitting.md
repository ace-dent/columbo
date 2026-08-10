# Block splitting: research & design

## Motivation

ace-dent's columbo binary (`--inspect`) reveals a technique absent from all three
reference tools (DeflOpt, defluff, deft4j): **block splitting**. A large dynamic
block is partitioned into multiple smaller dynamic blocks, each with its own
locally-optimised Huffman tree. This yields compound savings: the per-section
trees are better adapted to local symbol distributions, and the header overhead
is amortised by payload reductions.

Example: `deflopt-methods.md.gz` (1 dynamic block, 8672 tokens, 117355 bits).
ace-dent splits it into multiple blocks, saving 753 bits (94 bytes). Our v1.1
(no splitting) saves 2 bits on the same file — the 751-bit gap is entirely
attributable to block splitting.

## Problem statement

Given a sequence of N Deflate tokens with known symbol frequencies, find a
partition into K blocks (K ≥ 1) that minimises the total encoded size:

```
total_cost(partition) = sum(header_cost(block_i) + payload_cost(block_i))
```

Where:
- `header_cost` = dynamic header size for the block's local Huffman tree + RLE
- `payload_cost` = sum of Huffman code lengths for tokens in the block
- The block's Huffman tree is built from **local** symbol frequencies within
  that block only.

## Algorithm

### Dynamic programming

Standard DP over token positions. Let `cost[i][j]` = cost of encoding tokens
`[i, j)` as a single dynamic block. Let `dp[k][j]` = minimum cost to partition
tokens `[0, j)` into k blocks.

```
dp[1][j] = cost[0][j]   for all j
dp[k][j] = min(dp[k-1][i] + cost[i][j])   for i < j
```

Answer: `min_k dp[k][N]`.

Complexity: O(K · N²) where N is token count and K is max blocks.

### Optimisations

1. **Prefix-sum frequencies**: precompute cumulative frequencies for all symbol
   positions. `cost[i][j]` reads as O(alphabet) from prefix sums, not O(N).

2. **Heuristic K bound**: cap K at `min(N/256, 16)` — splitting beyond
   ~16 blocks per original stream yields diminishing returns (each block adds
   ~50-300 bits of header overhead).

3. **Candidate split positions**: only consider splits at positions where
   frequency distributions DIVERGE (change-point detection). Simple heuristic:
   split when the top-5 symbols by frequency rotate by >20%.

4. **Greedy forward pass** (simpler than full DP): scan left to right, emit a
   block when the cost of adding one more token exceeds the cost of starting a
   new block. O(N) with constant factors. Less optimal than DP but fast enough
   for interactive use.

### Recommended approach for v1

**Greedy with look-ahead**:

```text
position = 0
while position < N:
    best_split = position + 1
    best_cost = INF
    for look in [position+1 .. min(position + MAX_BLOCK_TOKENS, N)]:
        block_cost = cost[position][look]
        // Estimate: would splitting at `look` be better than continuing?
        if block_cost + estimate_remaining(look) < best_cost:
            best_split = look
            best_cost = block_cost
    emit_block(position, best_split)
    position = best_split
```

Where `estimate_remaining` uses a conservative per-token average cost.
`MAX_BLOCK_TOKENS` is bounded by the Deflate stored-block limit (65535 bytes
decoded) and by practical consideration (blocks >10000 tokens rarely benefit
from splitting).

This is O(N · W) where W is the look-ahead window (~1000-5000 tokens).

## Expected savings

- Single-block streams with N > 2000 tokens: 0.3-1.5% reduction (observed in
  ace-dent: 753/117355 = 0.64%).
- Already multi-block streams: minimal (<0.1%).
- Very small blocks (N < 500): no splitting benefit (header overhead dominates).

## Implementation plan

1. **Frequency prefix sums** (`block::freq_prefix`): precompute cumulative
   literal/length and distance frequency arrays for O(1) block cost estimation.

2. **Split cost function** (`block::split_cost`): given token range `[i, j)`,
   compute local Huffman tree + RLE header + payload cost.

3. **Greedy splitter** (`block::split_greedy`): scan with look-ahead window,
   emit optimal split points.

4. **Integration**: in the per-block optimizer, after parsing tokens for a
   block, check if `token_count > SPLIT_THRESHOLD` (e.g. 2000). If so, run
   the splitter and submit the multi-block candidate alongside the 5 existing
   candidates.

5. **Winner selection**: the split candidate competes with stored/fixed/dynamic/
   fallback/pruned. After splitting, each sub-block may itself be optimised
   (stored/fixed/dynamic selection per sub-block).

## Interaction with existing candidates

After splitting, each sub-block enters the normal candidate ladder:
- stored (if decoded bytes ≤ 65535)
- fixed
- dynamic-rebuilt
- dynamic-fallback
- pruned (≤2 iterations)

The total cost of the split candidate is `sum(sub_block_costs) + BFINAL handling`.
The winning representation per sub-block may differ (e.g. first block dynamic,
second block stored if incompressible).

BFINAL is set only on the last sub-block. All preceding sub-blocks have BFINAL=0.

## Open questions

1. **Optimal K**: what's the diminishing-returns point? Ace-dent's behavior
   suggests splitting until each block has ~500-2000 tokens.

2. **Split heuristics**: is greedy + look-ahead good enough vs full DP?
   Test on real corpus to quantify the gap.

3. **Distance symbol locality**: distance symbols reference a 32 KiB history
   window that spans blocks. Splitting may force re-emission of distance codes
   that were already "learned" in the previous block's Huffman tree. This
   trade-off is modelled by the cost function (local tree rebuilt from scratch).

4. **Min block size**: blocks with <50 tokens should never be split — the
   25-token fixed-emission gate plus 3-byte dynamic header makes tiny blocks
   always lose vs keeping them merged.

## References

- `research/defluff-methods.md` — Section "Methods not present": "block splitting"
  (explicitly absent in defluff 0.3.2)
- `research/design-v1.md` — Current architecture, block engine design
- ace-dent/columbo issue #1 — Reference implementation with splitting behaviour
