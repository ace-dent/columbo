# deft4j beta 17: source-verified Deflate optimization methods

This document describes the methods used by deft4j to optimize bytes in an existing
Deflate stream. It is intended as an implementation-oriented reference for projects that
want to reproduce, compare with, or learn from deft4j's post-processing methods.

The Java source is the ground truth. In particular, this document preserves the Java
candidate ordering and intermediate states rather than treating deft4j as an unordered
collection of optimization ideas.

## Reference source and evidence policy

The analysed snapshot is [NeRdTheNed/deft4j at commit
`fe818cd22540c53350b3ed3e13f50947dd23d194`](https://github.com/NeRdTheNed/deft4j/tree/fe818cd22540c53350b3ed3e13f50947dd23d194).

| Property | Value |
|---|---|
| Project version | `1.0.0-beta-17` |
| Upstream commit | `fe818cd22540c53350b3ed3e13f50947dd23d194` |
| Java compatibility target | Java 8 |
| Main author/copyright | Ned Loynd, 2023 |
| Original-code licence | BSD Zero Clause |
| Huffman-derived code licence | MIT, Ridge Shrubsall |

Line references below apply to that exact source snapshot. They may move in later releases.

Statements use these evidence classes:

- **Source-defined** means that the method, condition, ordering, constant, or comparison is
  directly expressed in the Java source.
- **Reconstructed** means that the description combines several Java methods into a
  higher-level algorithm. These interpretations were checked against the complete call
  graph and executable control flow in the referenced Java snapshot.
- **Implementation advice** explains how another project can preserve the behavior without
  copying deft4j's object layout. It is not a claim that the Java implementation uses that
  internal representation.

Method and class names are written exactly as in the source. Source ranges are the primary
route back to the evidence; comments and README statements are treated as secondary to
executable Java code.

## Scope

The subject here is deft4j's optimization of an already encoded Deflate stream:

- parsing stored, fixed, and dynamic blocks;
- retaining literals, matches, decoded bytes, and Huffman/header state;
- replacing selected matches with literals;
- rebuilding literal/length, distance, and code-length trees;
- trying multiple dynamic-header serializations;
- choosing stored or fixed representations where beneficial;
- removing empty blocks; and
- merging selected adjacent blocks.

The optional `deft4j-compress` module can run fresh compression engines such as the JVM
Deflater, JZLib, JZopfli, and CafeUndZopfli before applying deft4j. That is deliberately out
of scope. None of those compressors is required to reimplement the existing-stream
post-processor documented here.

Container parsing is covered only far enough to identify which existing Deflate streams
deft4j submits to the shared optimizer. Container-specific recompression policy is not part
of the core method.

## Executive summary

deft4j is a token-preserving Deflate optimizer with a large, ordered candidate graph.
For each source block it can:

1. optimize the existing token and header representation;
2. compare a stored representation;
3. rebuild dynamic Huffman tables from current tokens;
4. replace strictly more expensive matches with literals;
5. deliberately retain no-larger match removals as seeds for later tree rebuilding;
6. remove one complete match-length-symbol family as an exploratory seed;
7. regenerate the dynamic header under many RLE strategies;
8. repeat prune/rebuild operations to a strict fixed point; and
9. select the strictly smallest complete block at its current alignment.

It then repeats the winning block through the same candidate ladder until no further bits
are saved. An optional second phase walks adjacent blocks, constructs a merged block when
the type rules allow it, runs the entire block optimizer on the merge, and accepts only a
strict local saving. A source-level empty-removal link quirk can terminate either walk
early; that behavior is documented separately below.

The important consequence for another implementation is that intermediate states matter.
A no-larger token rewrite may not be the current winner, yet can lead to a better Huffman
table and header later. Keeping only the best state after each primitive operation does not
reproduce deft4j.

## Source model and bit accounting

### Linked block list

`DeflateStream` stores blocks as a doubly linked list through `DeflateBlock`.
`DeflateStream.parse()` reads the 3-bit `BFINAL/BTYPE` prologue, constructs a stored,
fixed, or dynamic block object, and links it to its predecessor
(`DeflateStream.java:61-126`; `DeflateBlock.java:64-144`).

Previous-block links are semantically significant. `DeflateBlock.readSlice()` can retrieve
match bytes from the current or earlier blocks and supports overlapping matches
(`DeflateBlock.java:146-236`). This lets each parsed match retain its exact decoded bytes,
which later match-to-literal methods can insert directly.

### Token representation

`DeflateBlockHuffman` stores a `List<LitLen>` (`DeflateBlockHuffman.java:20-46`). A
`LitLen` represents either:

- a literal or end-of-block when `dist == 0`; or
- a match when `dist > 0`, with `litlen` holding the decoded match length.

Each item also carries `decodedVal`, the bytes produced by that token. A match retains an
`edgecase` flag when length 258 was encoded with literal/length symbol 284 rather than the
ordinary symbol 285 (`LitLen.java:29-58`; `DeflateBlockHuffman.java:839-844`). Re-emission
preserves that choice through `Constants.len2litlen(len, edgecase)`.

This edge case is a deft4j source behavior, not a recommendation about the disputed
interpretation of symbol 284. A parity implementation must either preserve the source
symbol explicitly or retain an equivalent flag.

### Cached sizes

Huffman blocks maintain:

- `sizeBits`: block body size excluding the 3-bit block prologue;
- `litlenSizeBits`: token payload size, including match extra bits; and
- `dynamicHeaderSizeBits`: dynamic header after `BTYPE`, including the 5-, 5-, and 4-bit
  count fields.

`DeflateStream.getSizeBits()` adds three bits for each block, then calls
`block.getSizeBits(position_after_header)` (`DeflateStream.java:171-182`). Huffman block
size is alignment-independent. Stored size includes the padding needed to reach the next
byte boundary (`DeflateBlockUncompressed.java:69-74`).

Every candidate in `DeflateStream.optimiseBlock()` is scored as a complete block body at
the current post-header bit position. The winner callback updates only when
`newSizeBits < currentSmallest`, so ties preserve the earlier block
(`DeflateStream.java:343-368`).

### Copy-on-write state

`DeflateBlockHuffman.copy()` shares token lists, decoded data, and header arrays initially.
`ensureDidCopyLitLens()` and `ensureDidCopyRLEPairs()` create private lists before mutation
(`DeflateBlockHuffman.java:298-310,1174-1205`). The Java object strategy is not required in
a port, but candidates must behave as independent states. Accidentally changing a shared
token or RLE list alters later branches of the candidate graph.

## Parsing and writing an existing Deflate stream

### Block parsing

`DeflateStream.parse()` accepts the three defined block types and rejects reserved `BTYPE`
3 (`DeflateStream.java:72-125`). Stored blocks:

- align to the next byte;
- read `LEN` and `NLEN`;
- verify their one's-complement relation; and
- retain the payload bytes (`DeflateBlockUncompressed.java:22-35`).

Fixed blocks use the standard Deflate tables. Dynamic blocks read:

- `HLIT + 257`;
- `HDIST + 1`;
- `HCLEN + 4`;
- code-length-code lengths in Deflate permutation order; and
- exactly `HLIT + HDIST` expanded literal/length and distance lengths.

Repeat symbols 16, 17, and 18 are retained as explicit `rlePairs`, including their decoded
length bytes (`DeflateBlockHuffman.java:892-1009`). Symbol 16 is rejected when there is no
previous length, and any repeat that exceeds the advertised combined count is rejected.

### Token decoding

`decodeStream()` reads one canonical Huffman symbol at a time. It records literal bytes,
the end-of-block token, and match length/distance including extra bits. Match bytes are
materialized with `readSlice()` and appended to the block's decoded buffer
(`DeflateBlockHuffman.java:778-890`).

Only literal/length symbols through 285 and distance symbols through 29 are accepted for
data. The parser can receive dynamic length arrays as wide as 288 and 32 entries, but the
two reserved literal/length and distance symbols cannot be decoded as ordinary tokens.

### Canonical codes and bit order

`Huffman.buildCodes()` assigns canonical codes by increasing length and then increasing
symbol (`Huffman.java:30-64`). The output method reverses the selected code's bits through
`Util.rev()` because Deflate transmits Huffman codes least-significant bit first
(`Huffman.java:207-213`).

`DeflateStream.write()` regenerates `BFINAL` from list position: only the last surviving
block is written as final. It then byte-aligns the completed stream
(`DeflateStream.java:128-145`).

## Primitive token optimization

### Cost of one token

`DeflateBlockHuffman.getLitLenSize()` defines the comparison cost
(`DeflateBlockHuffman.java:112-131`). A literal or end-of-block costs its current
literal/length code length. A match costs:

```text
literal_length_code_length[length_symbol]
+ length_extra_bits
+ distance_code_length[distance_symbol]
+ distance_extra_bits
```

The original length-258 symbol-284 choice is used when the token's `edgecase` flag is set.

### Strict match-to-literal replacement

The shared implementation is `replaceWithLiteralsIfSmaller()`.
`DeflateBlockHuffman.optimise()` first calls
`replaceBackrefsWithLiteralsIfSmaller(false, true)`
(`DeflateBlockHuffman.java:460-468`). For each match:

```text
match_cost    = current encoded match cost
literal_cost  = sum(current literal code length for each decoded byte)
```

The match is replaced only when every required literal has a non-zero code and:

```text
literal_cost < match_cost
```

The loop abandons the candidate as soon as the running literal cost reaches the match cost
(`DeflateBlockHuffman.java:222-296`). Replacement inserts one literal token per decoded
byte and updates the cached payload and block sizes. It does not search for a different
match or split the match into smaller matches.

### No-larger match pruning

The same helper has a `prune` mode. With `prune == true`, a match is replaced when:

```text
literal_cost <= match_cost
```

This mode is used by `recodeHuffmanLessMatches()` before rebuilding the Huffman tables
(`DeflateBlockHuffman.java:655-657`). Equal-cost expansion is useful because it changes the
symbol frequencies: the current block does not get smaller immediately, but the next tree
or header may.

This distinction is fundamental:

| Path | Local condition | Immediate tree rebuild |
|---|---|---|
| `optimise()` | literals strictly cheaper | no |
| `recodeHuffmanLessMatches()` | literals no larger | yes |

A port that applies only strict expansion misses deft4j states. A port that lets no-larger
expansion overwrite the current winner directly also gets the control flow wrong; it is a
candidate seed until a complete block is scored.

### Missing literal codes

Both modes reject a match expansion when any decoded byte has code length zero in the
current literal table (`DeflateBlockHuffman.java:238-250`). The TODO comment proposes
adding missing codes in future, but no such search is implemented in this snapshot.

## Length-symbol-family removal

`removeDistLitLeastExpensive(int mode)` creates a broader exploratory state
(`DeflateBlockHuffman.java:372-458`). Despite the method name, it groups matches by their
**literal/length symbol**, not by distance symbol.

For each length-symbol family it accumulates:

```text
family_delta = sum(literal replay cost - current match cost)
family_count = number of matches in the family
```

If any match in a family needs a literal absent from the current table, the complete family
is ineligible. Among eligible families:

- mode 0 selects the smallest aggregate `family_delta` (`leastExpPruned()`); and
- mode 1 selects the smallest `family_count` (`leastSeenPruned()`).

Ties retain the first family encountered, which is the lower length-symbol index. The
chosen family's matches are all expanded to literals. Crucially, the method does not
require the immediate delta to be zero or negative. It may deliberately make the current
payload larger so a later Huffman rebuild and dynamic-header rewrite can produce a smaller
complete block.

Each invocation removes one family. The outer candidate ladder and repeated block passes
can apply least-expensive and least-seen removal in longer sequences.

## Rebuilding literal/length and distance trees

`recodeHuffman()` counts symbols in the current token list
(`DeflateBlockHuffman.java:670-743`). Its active arrays cover 286 literal/length symbols
and 30 distance symbols. Trailing zero-frequency symbols are trimmed before building the
new tables.

The literal/length tree is built with `new HuffmanTree(litFreq, 15)`. The distance table has
two special cases:

- no used distance symbol: emit a one-entry table whose only length is zero; or
- exactly one used distance symbol: give that symbol code 0 of length 1.

Otherwise deft4j builds a normal depth-limited distance tree. `MIN_LIT_CODES` and
`MIN_DIST_CODES` are both zero in this snapshot, so no compatibility-only dummy codes are
forced beyond the generic Huffman builder's own two-leaf rule.

After rebuilding the payload tables, `recodeToHuffmanInternal()` installs the tables and
recomputes every token cost. Its `recodeToHuffman()` wrapper then, for a dynamic result,
regenerates the header with the default packing strategy
(`DeflateBlockHuffman.java:745-770`).

## deft4j's Huffman tree builder

`HuffmanTree` is used for literal/length trees, distance trees, and code-length trees.
The source is `HuffmanTree.java:31-191`.

### Initial leaves and dummy leaves

Every positive frequency becomes a `LeafNode`. The builder then ensures the priority queue
contains at least two leaves:

```text
index = 0
while queue size < 2:
    if index is beyond the alphabet or frequency[index] == 0:
        add synthetic leaf(index, frequency 1)
    index++
```

This selects the earliest available zero-frequency symbol when the real alphabet has only
one used symbol. A synthetic leaf whose index lies beyond the table is omitted when codes
are finally copied into the fixed-size result.

### Heap ordering

Nodes implement `Comparable<Node>` as:

```text
this.weight - other.weight
```

There is no secondary comparison by symbol, height, insertion sequence, or node type
(`HuffmanTree.java:194-221`). The tree repeatedly removes the two least-weight nodes and
inserts their sum.

For exact output parity, "frequency-only" is not enough to specify equal-weight behavior:
the source uses `java.util.PriorityQueue`, whose internal heap behavior determines which
equal nodes are polled. An implementation that only needs equivalent compression can
choose any deterministic equal-weight order; a bit-for-bit reimplementation must reproduce
the Java runtime's heap operations for the reference run.

### Source-specific maximum-depth repair

After building an ordinary tree, deft4j repeatedly repairs it while `maxDepth > limit`
(`HuffmanTree.java:74-127`):

1. choose the first leaf at the deepest depth as `leafA`;
2. take its sibling leaf `leafB`;
3. remove their parent by moving `leafB` up to the grandparent;
4. search from depth `maxDepth - 2` upward toward the root for the first available leaf
   `leafC`;
5. replace `leafC` with a new internal node containing `leafA` and `leafC`; and
6. traverse the entire tree again to rebuild the depth map.

The search scans candidate depths downward from `maxDepth - 2` to 1 and takes the first
leaf stored at the chosen depth. This is a structural tree rewrite, not the histogram
overflow repair used by zlib or DeflOpt.

### Canonical table generation

`getTable()` walks depths in increasing order. At each depth it sorts leaves by symbol
value and assigns consecutive canonical codes (`HuffmanTree.java:160-191`). Tree child
orientation therefore affects the depth-repair choices, but canonical codes at a finished
depth are assigned in numeric symbol order.

## Dynamic-header methods

deft4j keeps the decoded literal/length and distance code lengths, their RLE token stream,
and a separate code-length Huffman table. It applies several distinct methods to those
layers.

### Header cost

For counts `numLitlenLens`, `numDistLens`, and `numCodelenLens`, the dynamic header body is
scored as:

```text
5 + 5 + 4
+ 3 * numCodelenLens
+ sum(code_length_code_bits[RLE symbol] + RLE extra bits)
```

Repeat extra-bit widths are 2 for symbol 16, 3 for 17, and 7 for 18
(`DeflateBlockHuffman.java:133-163,892-1009`). The common 3-bit Deflate block prologue is
accounted by `DeflateStream`, not `dynamicHeaderSizeBits`.

### Remove trailing HCLEN entries

`removeDynHeaderTrailingZeroLenCodelens()` removes zero-valued entries from the end of the
19-symbol code-length permutation, saving three bits per removed entry
(`DeflateBlockHuffman.java:334-369`). It recurses until the last transmitted entry is
non-zero. The writer asserts that the eventual count is between 4 and 19; the trimming
routine itself does not contain a separate explicit floor, so source construction and
valid input are responsible for preserving a legal minimum.

`removeTrailingHeaderCodes()` is the accounting wrapper that subtracts those saved bits
from both the complete block and dynamic-header totals.

### Strict RLE-token replacement

`optimiseHeader()` first trims the code-length-code span, then invokes
`replaceRLERunsWithLiteralsIfSmaller(false, true)`
(`DeflateBlockHuffman.java:471-475`). For a repeat token, it compares:

```text
repeat_cost   = code-length code for 16/17/18 + its extra bits
explicit_cost = sum(code-length code for each represented length value)
```

It replaces only when the explicit form is strictly smaller and every explicit value has
a code in the current code-length table. This method changes the RLE stream but does not
rebuild the code-length tree afterward.

### Recode the existing RLE stream

`recodeHeader()` preserves the current RLE tokenization, counts its symbols, constructs a
new 19-symbol code-length tree through `Huffman.ofRLEPacked()`, trims the transmitted
code-length-code span, and rescores the header
(`DeflateBlockHuffman.java:579-629`; `Huffman.java:117-134`).

### No-larger RLE pruning followed by recoding

`recodeHeaderToLessRLEMatches()` first expands repeat tokens whose explicit form is no
larger, then calls `recodeHeader()` (`DeflateBlockHuffman.java:631-635`). As with payload
pruning, accepting equal-cost changes creates a different frequency distribution for the
next tree.

The order is part of the method:

```text
rewrite/retain RLE tokens
-> optional no-larger repeat removal
-> rebuild code-length tree
-> final strict header optimization
```

Applying the no-larger rewrite under an unrelated code-length tree does not reproduce the
Java state.

## Regenerating the dynamic-header RLE

`HuffmanTable.packCodeLengths()` concatenates literal/length and distance length arrays and
packs each equal-value run (`HuffmanTable.java:32-159`). `rewriteHeader()` then builds the
code-length tree for that token stream and updates the block
(`DeflateBlockHuffman.java:480-577`).

### Zero runs

For a zero run, unless disabled:

1. emit symbol 18 using the largest legal count from 138 down to 11, repeating as needed;
2. emit symbol 17 using the largest legal count from 10 down to 3, repeating as needed;
3. allow the general symbol-16 stage to encode remaining zeros after one explicit zero,
   unless repeat-16-for-zeros is disabled; and
4. emit remaining zeros explicitly.

The controls are:

- `noZRep2`: disable symbol 18;
- `noZRep`: disable symbol 17;
- `noRep`: disable symbol 16; and
- `noRepZeros`: prohibit symbol 16 specifically for zero runs.

### Non-zero runs and ordinary repeat 16

Unless `noRep` is set, the packer emits the length once, then consumes the remaining run
with the largest legal symbol-16 counts from 6 down to 3. Any remainder is written as
explicit lengths.

### OHH 7- and 8-value alternatives

When `ohh` is enabled, two special cases can replace the greedy six-plus-literals shape:

- exactly eight remaining copies can use `4 + 4` symbol-16 runs when `use8` is enabled;
- exactly seven remaining copies can use `4 + 3` symbol-16 runs when `use7` is enabled.

The implementation contains an alternate eight split of `5 + 3`, controlled by `alt8`,
but `DeflateStream.TRY_ALT_8` is false. The active beta-17 optimizer therefore searches
`4 + 4`, not `5 + 3` (`DeflateStream.java:243-263`; `HuffmanTable.java:48-57,110-145`).

## The 56-way header option grid

`DeflateStream.addOptimisedRecoded()` enumerates a nested boolean grid for every applicable
dynamic state (`DeflateStream.java:265-317`). With `TRY_ALT_8 == false`, the source emits
56 parameter combinations per input state.

The loop order is:

1. `noRepZeros`: false, true;
2. RLE `prune`: false, true;
3. `noRep`: false, true, except forced false with `noRepZeros`;
4. `noZRep`: false, true, except forced true with `noRepZeros`;
5. `noZRep2`: false, true;
6. `ohh`: true, false;
7. for `ohh`, `use8`: true, false; and
8. for `ohh`, `use7`: true, false.

The all-false `use8/use7` combination is skipped in the OHH branch, and OHH combinations
with `noRep` are skipped. `alt8` has only its default false value.

For each combination, `optimiseBlockDynBlock()`:

```text
copy the input dynamic block
rewriteHeader(selected packing flags)
if RLE prune is enabled:
    recodeHeaderToLessRLEMatches()
optimiseHeader()
submit the complete block candidate
```

Thus a grid entry is not merely an RLE string. It includes a code-length-tree build,
optional no-larger RLE pruning and rebuild, and the final strict header pass
(`DeflateStream.java:184-198`).

## Recode/prune state families

### One-shot Huffman recoding

`recodedHuffman(block, false)` copies a block and calls `recodeHuffman()`.
`recodedHuffman(block, true)` copies it and calls `recodeHuffmanLessMatches()`
(`DeflateStream.java:200-210`). The second state retains no-larger match expansions before
building new payload tables.

### Fixed-point prune/recode

`recodedHuffmanFull()` repeatedly performs the pruned recode
(`DeflateStream.java:212-229`):

```text
previous_size = current complete block body size
loop:
    candidate = copy current
    expand matches whose literals are no larger under current tables
    rebuild payload tables and dynamic header
    if candidate_size >= previous_size:
        return current
    current = candidate
    previous_size = candidate_size
```

Only strict complete-block improvement advances the loop. Equal-size token changes from
the losing iteration are discarded by this particular fixed-point method, although the
separate one-shot pruned state remains available elsewhere in the candidate ladder.

### `addOptimisedRecoded()` base states

Before running the 56-way header grid, `addOptimisedRecoded()` constructs an insertion-
ordered `LinkedHashMap` containing:

1. an optimized copy of the input state;
2. an ordinary Huffman-recoded state, then normally optimized;
3. a one-shot no-larger-pruned Huffman-recoded state, then normally optimized; and
4. when the fixed-point loop made at least one strict improvement, the full pruned state,
   then normally optimized.

The small wrappers `optimiseBlockCopyHelper()` and `optimiseBlockHelper()` distinguish
"copy then optimize" from "optimize this already-private state" in this construction.

`DeflateBlockHuffman` does not override `equals()` or `hashCode()`, so these map keys use
object identity. The map preserves insertion order but does not structurally de-duplicate
equivalent states (`DeflateStream.java:265-277`). Each retained base state receives the
full header grid.

This one helper can therefore submit 168 candidates with three base states or 224 with
four, before the outer ladder's other candidates are counted.

## Complete per-block candidate ladder

`DeflateStream.optimiseBlock()` is the central method
(`DeflateStream.java:343-490`). It starts with the original block as the incumbent and
submits every generated candidate to one strict complete-block winner callback.

### Common candidates

For every block it first tries:

- `optimiseBlockNormal()`: copy, run the block's ordinary `optimise()`, and retain the copy
  only if that method reports a positive saving; and
- for every non-stored block of at most 65,535 decoded bytes, a stored representation.

A stored source block has no Huffman candidate ladder because it is not a
`DeflateBlockHuffman` state.

### Establishing a dynamic Huffman seed

For a dynamic source, the original and normally optimized dynamic states become seeds.

For a fixed source, deft4j copies it and calls `recodeHuffman()`, producing a dynamic seed.
It also normally optimizes that seed. This means fixed input can enter the large dynamic
candidate ladder even though the original block has no dynamic header
(`DeflateStream.java:385-397`).

### `runOptimisationsCallback`

For one dynamic seed, the inner callback generates four branches
(`DeflateStream.java:399-442`):

1. **post-recoded:** copy the seed, call `recodeHeader()`, submit it, normally optimize it,
   and call `addOptimisedRecoded()` on it;
2. **pruned header:** copy the seed, call `recodeHeaderToLessRLEMatches()`, submit it,
   normally optimize it, and call `addOptimisedRecoded()` on it;
3. **least-expensive family:** remove the selected length-symbol family and call
   `addOptimisedRecoded()`; and
4. **least-seen family:** remove the selected length-symbol family and call
   `addOptimisedRecoded()`.

The direct fixed-Huffman code inside this callback is commented out and is not active
beta-17 behavior.

### `runOptimisationsCallbackMulti`

The outer callback submits and runs the inner callback on:

1. the seed itself;
2. an ordinary Huffman-recoded copy;
3. a one-shot no-larger-pruned Huffman-recoded copy; and
4. when it strictly improves, the fixed-point pruned/recode result.

This ordering is at `DeflateStream.java:443-463`.

### Top-level seed order

The multi-callback is invoked for:

1. the default dynamic seed;
2. the normally optimized dynamic seed, when it exists;
3. a least-expensive-family seed; and
4. a least-seen-family seed.

For an originally dynamic block, `toFixedHuffman()` separately copies the default state and
calls `recodeToFixedHuffman()`. deft4j runs the result's ordinary strict optimization and
submits that fixed candidate
(`DeflateStream.java:464-487`). Fixed source does not repeat that conversion because it
already has a fixed incumbent.

The result is an order-of-thousands candidate search for a non-trivial Huffman block.
There is no source-level timeout, token-count gate, decoded-size gate, or filename rule in
this candidate ladder. Runtime limits in another project are scheduling policy, not deft4j
method semantics.

### Strict winner and non-winning states

Every complete candidate is compared with `<`. Equal-size candidates do not replace the
incumbent. However, intermediate equal-cost states are still constructed inside
no-larger-prune branches and can produce later winning descendants.

This is why a faithful implementation needs two concepts:

- the ordered work queue of states that still require descendants; and
- the strictly smallest complete block seen so far.

Collapsing the work queue to the current winner after each stage loses real deft4j paths.

## Repeated optimization of each block

In the absence of a removable empty block, `DeflateStream.optimise()` walks the linked list from left to right
(`DeflateStream.java:496-566`). For a non-empty block—or the only remaining block—it calls
the complete `optimiseBlock()` ladder. When the returned block is different and strictly
smaller, it replaces the list node and immediately runs the same list position through
another pass. It advances to the next block only when a pass produces no strict saving.

This permits longer transformation sequences than one invocation of `optimiseBlock()`.
For example, a winner derived from least-seen family removal can become the default seed of
the next pass and receive another family removal or prune/recode sequence.

### Empty-block removal and early termination

The source intends to remove an empty block when another block remains. Its condition keeps
an empty block when it is the first and only remaining block:

```text
optimize block if decoded length > 0
or if it is the first and only remaining block
otherwise remove it
```

The exact beta-17 control flow has an additional consequence. `DeflateBlock.remove()`
calls the virtual `discard()` method, and both concrete block implementations ultimately
clear the removed node's `nextBlock` link (`DeflateBlock.java:36-51,134-144`). After that
call, `DeflateStream.optimise()` executes `currentBlock = currentBlock.getNext()`. It
therefore receives null and ends the per-block walk at the first empty block it removes
(`DeflateStream.java:533-559`).

With merging enabled, `mergeBlocks()` starts a new walk from `firstBlock` and may remove one
further empty block, but it has the same remove-then-`getNext()` shape and also stops after
that removal (`DeflateStream.java:620-646`). Consequently, one optimizer invocation does
not generally reduce an arbitrary all-empty list to one block, nor does it necessarily
optimize all blocks following a removed empty block.

Implementation advice: saving the successor before `remove()` and continuing the walk is
the natural correctness fix, and the retained sole-block condition still prevents an empty
stream representation. It is nevertheless a deliberate difference from the beta-17 Java
source and should not be described as exact parity.

### Position-accounting quirk

The Java loop adds the block's size to `pos` before deciding whether to repeat the same
list node. It does not rewind `pos` when a winning replacement causes another pass.
`mergeBlocks()` has the same shape after an accepted merge
(`DeflateStream.java:504-559,576-646`). Huffman body sizes ignore alignment, so this is
usually invisible; stored-candidate padding can depend on it.

An exact-output clone should reproduce the source's position progression. A new optimizer
designed for ideal alignment accounting may instead recompute the true list position, but
that is a deliberate behavioral difference and should be tested separately.

## Stored-block candidates and merging

### Stored representation

`DeflateBlock.asUncompressed()` converts any Huffman block to a stored block containing its
decoded bytes (`DeflateBlock.java:53-62`). `optimiseBlock()` submits this candidate when the
decoded length is at most 65,535 bytes.

For post-header alignment `a`, `DeflateBlockUncompressed.getSizeBits(a)` returns:

```text
padding from a to the next byte boundary
+ 16 + 16                  LEN and NLEN
+ 8 * decoded_size
```

The common 3-bit block prologue is added by `DeflateStream`. The candidate wins only on a
strict complete-body saving.

### Asymmetric stored merge

`DeflateBlockUncompressed.canMerge()` allows a **stored current block** to absorb any next
block type when their combined decoded data is at most 65,535 bytes
(`DeflateBlockUncompressed.java:98-117`). The result is one stored block.

This rule is asymmetric:

- stored followed by fixed/dynamic/stored may merge within the size limit;
- Huffman followed by stored cannot merge through the Huffman merge method.

Ports that require both blocks to be stored before this merge do not match the source.

## Huffman-block merging

`DeflateBlockHuffman.canMerge()` permits a fixed or dynamic current block to merge with a
fixed or dynamic next block (`DeflateBlockHuffman.java:1226-1271`). It does not implement a
same-dynamic-tree merge.

The merge method:

1. converts each dynamic child to fixed Huffman independently;
2. copies the first fixed child;
3. removes its end-of-block token;
4. appends the second child's token list, including its end-of-block;
5. combines the decoded byte arrays; and
6. recomputes the fixed payload size.

The immediate merged object is therefore fixed. `mergeBlocks()` then passes it through the
complete `optimiseBlock()` ladder, where it may become dynamic again, become stored, or
remain fixed.

This is broader than DeflOpt's exact deletion of a fixed/fixed boundary: dynamic children
are first recoded as fixed so their token lists can form a common merged state, and that
state is fully reoptimized.

The beta-17 README still lists stored/Huffman and same-tree dynamic merging as future
work. The executable Java is more advanced in some directions and narrower in another:
it implements the stored absorption and fixed-converting Huffman merge described above,
but not a direct identical-dynamic-tree merge. For source parity, the Java predicates and
merge bodies take precedence over that README list.

## Adjacent merge walk

After per-block optimization, `DeflateStream.optimise(true)` calls `mergeBlocks()`
(`DeflateStream.java:562-565,568-650`). The method walks left to right over the optimized
linked list.

Merging is enabled by default: the no-argument library method delegates to
`optimise(true)`, and the command-line `--merge-blocks` option also defaults to true. It can
be disabled explicitly (`DeflateStream.java:492-496`; `Optimise.java:33-34`).

For a mergeable current/next pair it computes:

```text
unmerged = current_body_bits_at_position
         + 3
         + next_body_bits_at_position_after_current

merged_candidate = optimiseBlock(current.merge(next), current_position)
saving = unmerged - merged_candidate_body_bits
```

The extra three bits are the second block's prologue. A merge is accepted only when
`saving > 0`. The merged node replaces both inputs and remains the current node, so it can
be considered again with its new next neighbor. The walk advances only when no strict merge
wins at that position.

The order matters. This is not global dynamic programming over all block partitions, and
it does not consider arbitrary non-adjacent pairs. Changing one early local merge can alter
all later opportunities.

## Raw-stream keep-original rule

The convenience entry `Deft.optimiseDeflateStream()` parses a raw byte array and runs the
optimizer with merging enabled by default (`Deft.java:15-34`). It serializes the parsed
list only when `DeflateStream.optimise()` reports a positive bit saving. If parsing fails,
an `IOException` occurs, or optimization reports no saving, it returns the original byte
array object. Other unchecked failures are not caught by this helper.

This is a bit-level gate, not a whole-byte comparison. A result that saves bits but occupies
the same number of bytes can replace the original. Container command paths parse and write
their containers separately, so their outer serialization behavior should not be inferred
from this raw helper alone.

## Complete reconstructed algorithm

The following pseudocode expresses the source control flow while abstracting Java object
ownership:

```text
function optimize_stream(parsed_block_list, merge_enabled):
    current = first block
    first = true

    while current exists:
        if current is empty and not (first and current is only remaining block):
            remove current
            end this source-defined per-block walk

        winner = optimize_block(current, current_post_header_position)

        if winner is strictly smaller than current:
            replace current with winner
            current = winner
            repeat this list position
        else:
            advance to next block
            first = false

    if merge_enabled:
        merge_blocks_left_to_right()

    regenerate BFINAL from the surviving list and write the stream


function optimize_block(source, position):
    best = source

    submit_strict(normal_optimized_copy(source))

    if source is not stored and decoded_size <= 65535:
        submit_strict(stored_copy(source))

    if source is dynamic:
        default_dynamic = source
        optimized_dynamic = normal_optimized_copy(source), if improving
    else if source is fixed:
        default_dynamic = rebuild_dynamic_huffman(copy(source))
        optimized_dynamic = normal_optimized_copy(default_dynamic), if improving
    else:
        return best

    run_multi(default_dynamic)
    if optimized_dynamic exists:
        run_multi(optimized_dynamic)

    if source was dynamic:
        fixed = convert_to_fixed(default_dynamic)
        normal_optimize(fixed)
        submit_strict(fixed)

    run_multi(remove_least_expensive_length_family(default_dynamic))
    run_multi(remove_least_seen_length_family(default_dynamic))

    return best


function run_multi(seed):
    for state in [
        seed,
        huffman_recoded(seed),
        no_larger_matches_then_huffman_recoded(seed),
        strict_fixed_point_of_no_larger_prune_and_recode(seed), if improving
    ]:
        submit_strict(state)
        run_inner(state)


function run_inner(seed):
    post = recode_current_header_RLE_tree(seed)
    submit_strict(post)
    submit_strict(normal_optimized_copy(post), if improving)
    add_optimized_recoded(post)

    rle_pruned = remove_no_larger_RLE_repeats_then_recode(seed)
    submit_strict(rle_pruned)
    submit_strict(normal_optimized_copy(rle_pruned), if improving)
    add_optimized_recoded(rle_pruned)

    add_optimized_recoded(remove_least_expensive_length_family(seed))
    add_optimized_recoded(remove_least_seen_length_family(seed))


function add_optimized_recoded(seed):
    ordered_states = [
        normal_optimized_copy(seed),
        normal_optimize(huffman_recoded(seed)),
        normal_optimized_copy(no_larger_matches_then_huffman_recoded(seed)),
        normal_optimize(strict_fixed_point_pruned_recode(seed)), if improving
    ]

    for state in ordered_states:
        for header_configuration in the 56 source configurations:
            candidate = rewrite_header(state, header_configuration)
            if configuration requests RLE pruning:
                candidate = remove_no_larger_RLE_repeats_then_recode(candidate)
            candidate = strict_header_optimize(candidate)
            submit_strict(candidate)
```

`submit_strict` updates the complete-block winner only for a smaller body at the supplied
position. It does not prevent other queued states from generating descendants.

## Complexity and compute characteristics

The source favors breadth over efficiency:

- one `addOptimisedRecoded()` invocation generates 56 header configurations for each of
  three or four base states;
- the inner callback invokes that helper four times;
- the multi-callback applies the inner callback to three or four recode/prune states;
- the top level applies the multi-callback to several seeds;
- a winning block is processed again; and
- every merge candidate runs the same block ladder.

Consequently, a single dynamic block can cause thousands of complete candidate
constructions and scores. The source has no time budget and describes itself as very
inefficient. Its search is finite, but runtime can be high on large or highly blocked
streams.

An efficient port can share immutable tokens, frequency vectors, canonical tables, and
equivalent header scores. It can also structurally de-duplicate states after preserving the
Java insertion/tie order. It must not prune a state solely because it is not the current
winner if that state still has source-defined descendants.

## Container stream discovery

All supported containers ultimately expose a `List<DeflateStream>`.
`DeflateFilesContainer.optimise()` processes that list in order and calls the same
`DeflateStream.optimise(mergeBlocks)` method for every stream
(`DeflateFilesContainer.java:14-43,65-73`). There is no container-specific Deflate search
algorithm.

### Raw Deflate and zlib

`RawDeflateFile` contains one raw stream. `ZLibFile` validates compression method 8, FCHECK,
and the absence of a preset dictionary, then parses one raw Deflate stream and retains the
Adler-32 trailer (`RawDeflateFile.java:11-35`; `ZLibFile.java:14-109`). On output it
recalculates Adler-32 from decoded bytes.

### GZIP

`GZFile` parses one method-8 member, including `FEXTRA`, `FNAME`, `FCOMMENT`, and `FHCRC`,
then exposes its raw Deflate stream (`GZFile.java:13-87,181-186`). Output recalculates CRC-32
and ISIZE from the decoded data. No loop over concatenated GZIP members appears in this
class.

### ZIP

`ZipFile` uses lljzip to load the archive and creates a `DeflateStream` for every local
entry whose compression method is Deflate. Non-Deflate entries are left to the archive
library and are not submitted to the optimizer (`ZipFile.java:27-140`). Rewritten Deflate
bytes and compressed sizes are synchronized into local and linked central headers before
the archive is serialized.

### PNG and APNG

`PNGFile` exposes independent streams for:

- the concatenated PNG `IDAT` zlib stream;
- each APNG frame's concatenated `fdAT` zlib stream; and
- supported zlib-compressed `zTXt`, `iCCP`, and compressed `iTXt` chunks.

It validates chunk CRCs and APNG sequence/order constraints, rebuilds `IDAT`/`fdAT` chunks,
renumbers APNG sequence fields, reinserts optimized metadata streams, and recalculates PNG
CRCs (`PNGFile.java:19-215,262-369,377-605`). Unlike DeflOpt 2.07, this source explicitly
handles APNG `fdAT` streams.

These wrapper methods are useful for locating streams but do not change the raw candidate
ladder.

## Explicitly out of scope or not implemented

The following are not methods of the beta-17 existing-stream optimizer:

- fresh LZ77 match discovery;
- replacing one match with one or more different matches;
- adding a missing literal code in order to expand a match;
- arbitrary block splitting;
- global optimization over all block partitions;
- merging non-adjacent blocks;
- direct merging of dynamic blocks under an identical tree;
- the README's proposed same-Huffman-tree dynamic merge;
- `TRY_ALT_8`'s dormant `5 + 3` header split;
- DeflOpt's four height-aware/frequency-only heap variants;
- DeflOpt's histogram-based maximum-depth repair;
- Defluff package-merge or shortest-path header algorithms;
- finished-tree, block-splitting, or recursive searches added by another implementation;
- filename-, file-size-, token-count-, or timeout-based semantic gates; and
- JVM Deflater, JZLib, JZopfli, CafeUndZopfli, libdeflate, or any other high-level
  recompression engine.

Some of these appear as TODOs, README future work, optional compressor modules, or methods
in other projects. They should be labelled extensions rather than deft4j post-processor
parity.

## Implementation requirements and common pitfalls

An independent implementation should preserve these details when parity is the goal:

1. **Keep token, payload-tree, and header state together.** A header-only candidate and a
   token-pruned candidate have different descendants.
2. **Separate the work queue from the current winner.** Non-winning no-larger states may
   later win after recoding.
3. **Preserve source ordering.** `LinkedHashMap` insertion order and strict `<` winner tests
   make order observable.
4. **Do not treat the map as structural de-duplication.** Java uses object identity for
   `DeflateBlockHuffman` keys.
5. **Distinguish strict and no-larger expansion.** `optimise()` uses `<`; prune/recode paths
   use `<=` locally but still require a later strict complete-block win.
6. **Reject expansions with missing literal codes.** The source logs a possible missed
   optimization and leaves the match unchanged.
7. **Group family pruning by length symbol.** `removeDistLitLeastExpensive()` does not
   select a distance-code family.
8. **Allow a locally costly family removal.** The source relies on later table/header
   rebuilds to decide the complete candidate.
9. **Use the Java Huffman depth repair.** A conventional length-limited Huffman algorithm
   can produce different trees.
10. **Account for `PriorityQueue` equal-weight behavior.** A secondary symbol tie breaker
    is not present in the source.
11. **Preserve symbol 284 for parsed length-258 matches.** Canonicalizing every 258 to 285
    changes frequencies and output.
12. **Run all 56 active header configurations.** `alt8` remains false in this snapshot.
13. **Keep the exact header operation order.** Pack, optional no-larger RLE prune/recode,
    then strict header optimization.
14. **Repeat a winning block through the full ladder.** One pass is not equivalent.
15. **Treat empty-block traversal as an explicit compatibility choice.** The linked-list
    condition preserves a sole empty block, but clearing `nextBlock` during removal makes
    beta-17 stop that phase after the first removal. Saving the successor first is a
    correctness fix, not exact source behavior.
16. **Respect asymmetric merge predicates.** Stored-current and Huffman-current merges have
    different eligible successors.
17. **Fixed-convert Huffman children before merging.** The merged state is then fully
    optimized again.
18. **Accept block and merge winners strictly.** Equal complete sizes retain the incumbent.
19. **Do not invent source gates.** Work budgets can limit a port, but they are not deft4j
    behavior.
20. **Treat source quirks deliberately.** The repeated-pass position progression can affect
    stored padding; document whether exact parity or corrected accounting is intended.

## Suggested validation strategy

### 1. Parser and writer validity

- Round-trip stored, fixed, and dynamic blocks.
- Include matches that cross a source-block boundary and overlapping matches.
- Test maximum distance and match length.
- Include length 258 encoded by both symbols 284 and 285.
- Decompress every output with multiple independent Deflate decoders.

### 2. Primitive method parity

- Test strict match replacement at cheaper, equal, and more-expensive costs.
- Repeat with one required literal absent.
- Test no-larger pruning followed by a Huffman rebuild.
- Test least-expensive and least-seen family selection, including tied families and a
  locally costly selected family.
- Test zero-, one-, and many-symbol Huffman alphabets plus depth-overflow cases.

### 3. Header parity

- Test symbols 16, 17, and 18 at every legal boundary.
- Test zero runs with each of `noRep`, `noZRep`, `noZRep2`, and `noRepZeros`.
- Test exact seven- and eight-repeat OHH cases.
- Confirm that active source enumeration produces 56 configurations.
- Distinguish strict `optimiseHeader()` from no-larger
  `recodeHeaderToLessRLEMatches()`.

### 4. Candidate-order parity

- Trace candidate names from a Java build with fine logging enabled.
- Compare the winner after each `optimiseBlock()` pass, not only the final stream.
- Retain a case where an equal-cost pruned state leads to a later strict win.
- Compare token lists, literal/distance lengths, header RLE, and complete bit count.

### 5. Block-list parity

- Test an empty first/middle/last block, interspersed empty blocks, and an all-empty stream.
  For exact beta-17 parity, assert that each removal ends the current phase; for corrected
  traversal, assert the intentionally different result.
- Test stored-current absorption of each possible following type.
- Test fixed/fixed, fixed/dynamic, dynamic/fixed, and dynamic/dynamic Huffman merges.
- Confirm every accepted merge is re-run through the complete per-block ladder.
- Test a sequence where the first local merge changes the next merge opportunity.

### 6. Container parity

Validate raw Deflate first. Then verify zlib checksums, GZIP CRC/ISIZE, ZIP entry data and
sizes, PNG chunk CRCs, APNG sequence numbers, and independent compressed metadata streams.
Container byte identity is not a substitute for raw token/tree/header parity.

## Source index

The links below point to the exact upstream commit used as ground truth.

| Source location | Role |
|---|---|
| [`Deft.java:15-53`](https://github.com/NeRdTheNed/deft4j/blob/fe818cd22540c53350b3ed3e13f50947dd23d194/deft4j-base/src/main/java/com/github/NeRdTheNed/deft4j/Deft.java#L15-L53) | raw optimizer entry and keep-original fallback |
| [`DeflateStream.java:61-182`](https://github.com/NeRdTheNed/deft4j/blob/fe818cd22540c53350b3ed3e13f50947dd23d194/deft4j-base/src/main/java/com/github/NeRdTheNed/deft4j/deflate/DeflateStream.java#L61-L182) | block-list parse, write, decoded data, and bit count |
| [`DeflateStream.java:184-229`](https://github.com/NeRdTheNed/deft4j/blob/fe818cd22540c53350b3ed3e13f50947dd23d194/deft4j-base/src/main/java/com/github/NeRdTheNed/deft4j/deflate/DeflateStream.java#L184-L229) | dynamic-header wrapper and recode/prune fixed point |
| [`DeflateStream.java:231-317`](https://github.com/NeRdTheNed/deft4j/blob/fe818cd22540c53350b3ed3e13f50947dd23d194/deft4j-base/src/main/java/com/github/NeRdTheNed/deft4j/deflate/DeflateStream.java#L231-L317) | length-family seeds and 56-way header grid |
| [`DeflateStream.java:319-490`](https://github.com/NeRdTheNed/deft4j/blob/fe818cd22540c53350b3ed3e13f50947dd23d194/deft4j-base/src/main/java/com/github/NeRdTheNed/deft4j/deflate/DeflateStream.java#L319-L490) | complete per-block candidate ladder |
| [`DeflateStream.java:492-566`](https://github.com/NeRdTheNed/deft4j/blob/fe818cd22540c53350b3ed3e13f50947dd23d194/deft4j-base/src/main/java/com/github/NeRdTheNed/deft4j/deflate/DeflateStream.java#L492-L566) | repeated block passes and empty-block removal |
| [`DeflateStream.java:568-650`](https://github.com/NeRdTheNed/deft4j/blob/fe818cd22540c53350b3ed3e13f50947dd23d194/deft4j-base/src/main/java/com/github/NeRdTheNed/deft4j/deflate/DeflateStream.java#L568-L650) | adjacent optimized-block merge walk |
| [`DeflateBlock.java:36-62`](https://github.com/NeRdTheNed/deft4j/blob/fe818cd22540c53350b3ed3e13f50947dd23d194/deft4j-base/src/main/java/com/github/NeRdTheNed/deft4j/deflate/DeflateBlock.java#L36-L62), [`92-236`](https://github.com/NeRdTheNed/deft4j/blob/fe818cd22540c53350b3ed3e13f50947dd23d194/deft4j-base/src/main/java/com/github/NeRdTheNed/deft4j/deflate/DeflateBlock.java#L92-L236) | link mutation, stored conversion, replacement/removal, and cross-block/overlapping match materialization |
| [`DeflateBlockUncompressed.java:22-117`](https://github.com/NeRdTheNed/deft4j/blob/fe818cd22540c53350b3ed3e13f50947dd23d194/deft4j-base/src/main/java/com/github/NeRdTheNed/deft4j/deflate/DeflateBlockUncompressed.java#L22-L117) | stored parse/write/cost and asymmetric merge |
| [`DeflateBlockHuffman.java:112-191`](https://github.com/NeRdTheNed/deft4j/blob/fe818cd22540c53350b3ed3e13f50947dd23d194/deft4j-base/src/main/java/com/github/NeRdTheNed/deft4j/deflate/DeflateBlockHuffman.java#L112-L191) | token and header-RLE cost helpers |
| [`DeflateBlockHuffman.java:213-332`](https://github.com/NeRdTheNed/deft4j/blob/fe818cd22540c53350b3ed3e13f50947dd23d194/deft4j-base/src/main/java/com/github/NeRdTheNed/deft4j/deflate/DeflateBlockHuffman.java#L213-L332) | strict/no-larger token replacement |
| [`DeflateBlockHuffman.java:334-458`](https://github.com/NeRdTheNed/deft4j/blob/fe818cd22540c53350b3ed3e13f50947dd23d194/deft4j-base/src/main/java/com/github/NeRdTheNed/deft4j/deflate/DeflateBlockHuffman.java#L334-L458) | HCLEN trimming and length-family removal |
| [`DeflateBlockHuffman.java:460-635`](https://github.com/NeRdTheNed/deft4j/blob/fe818cd22540c53350b3ed3e13f50947dd23d194/deft4j-base/src/main/java/com/github/NeRdTheNed/deft4j/deflate/DeflateBlockHuffman.java#L460-L635) | ordinary/header optimization, header rewrite, and recode |
| [`DeflateBlockHuffman.java:637-770`](https://github.com/NeRdTheNed/deft4j/blob/fe818cd22540c53350b3ed3e13f50947dd23d194/deft4j-base/src/main/java/com/github/NeRdTheNed/deft4j/deflate/DeflateBlockHuffman.java#L637-L770) | fixed conversion and payload Huffman rebuild |
| [`DeflateBlockHuffman.java:778-1155`](https://github.com/NeRdTheNed/deft4j/blob/fe818cd22540c53350b3ed3e13f50947dd23d194/deft4j-base/src/main/java/com/github/NeRdTheNed/deft4j/deflate/DeflateBlockHuffman.java#L778-L1155) | dynamic/fixed token parse and write |
| [`DeflateBlockHuffman.java:1226-1271`](https://github.com/NeRdTheNed/deft4j/blob/fe818cd22540c53350b3ed3e13f50947dd23d194/deft4j-base/src/main/java/com/github/NeRdTheNed/deft4j/deflate/DeflateBlockHuffman.java#L1226-L1271) | fixed-converting Huffman merge |
| [`HuffmanTable.java:32-159`](https://github.com/NeRdTheNed/deft4j/blob/fe818cd22540c53350b3ed3e13f50947dd23d194/deft4j-base/src/main/java/com/github/NeRdTheNed/deft4j/huffman/HuffmanTable.java#L32-L159) | dynamic-header RLE strategies |
| [`HuffmanTree.java:31-191`](https://github.com/NeRdTheNed/deft4j/blob/fe818cd22540c53350b3ed3e13f50947dd23d194/deft4j-base/src/main/java/com/github/NeRdTheNed/deft4j/huffman/HuffmanTree.java#L31-L191) | Java priority-queue tree and depth repair |
| [`Huffman.java:30-141`](https://github.com/NeRdTheNed/deft4j/blob/fe818cd22540c53350b3ed3e13f50947dd23d194/deft4j-base/src/main/java/com/github/NeRdTheNed/deft4j/huffman/Huffman.java#L30-L141) | canonical tables and code-length tree construction |
| [`PNGFile.java:19-605`](https://github.com/NeRdTheNed/deft4j/blob/fe818cd22540c53350b3ed3e13f50947dd23d194/deft4j-container/src/main/java/com/github/NeRdTheNed/deft4j/container/PNGFile.java#L19-L605) | PNG/APNG and compressed ancillary streams |
| [`GZFile.java:13-186`](https://github.com/NeRdTheNed/deft4j/blob/fe818cd22540c53350b3ed3e13f50947dd23d194/deft4j-container/src/main/java/com/github/NeRdTheNed/deft4j/container/GZFile.java#L13-L186) | GZIP stream discovery and checksum rewrite |
| [`ZLibFile.java:14-109`](https://github.com/NeRdTheNed/deft4j/blob/fe818cd22540c53350b3ed3e13f50947dd23d194/deft4j-container/src/main/java/com/github/NeRdTheNed/deft4j/container/ZLibFile.java#L14-L109) | zlib validation and Adler-32 rewrite |
| [`ZipFile.java:27-140`](https://github.com/NeRdTheNed/deft4j/blob/fe818cd22540c53350b3ed3e13f50947dd23d194/deft4j-container/src/main/java/com/github/NeRdTheNed/deft4j/container/ZipFile.java#L27-L140) | ZIP method-8 stream discovery and update |

## Confidence and remaining limits

The candidate graph was traced method by method through the exact Java snapshot, and the
base module was compiled directly with `javac`. The method map covers the active
post-processing paths reached through `optimiseBlock()`, repeated stream optimization, and
the adjacent-block merge pass.

The empty-block account follows executable code rather than the README's general statement
that empty blocks are removed. `remove()` calls `discard()`, which clears the removed
node's successor before the loop asks that node for its successor; the current phase
therefore ends after that removal.

The strongest remaining reproducibility caveat is equal-weight ordering in Java's
`PriorityQueue`: the source comparator intentionally supplies no tie key, while the Java
collection contract does not promise a portable order among equal elements. Exact output
should therefore record the runtime used for a reference build or emulate its queue
operations explicitly.

This document describes source behavior, including expensive breadth and the position-
accounting quirk. A production implementation may choose stronger validation, corrected
alignment tracking, state de-duplication, and bounded scheduling. Those changes should be
distinguished from deft4j beta-17 parity and verified against the source-defined candidate
graph.

## Attribution

deft4j's original code is Copyright (c) 2023 Ned Loynd and licensed under the BSD Zero
Clause License. Its Huffman encoding and decoding code is largely derived from Ridge
Shrubsall's `deflate-impl` work under the MIT License. The Deflate parser/writer also credits
Hans Wennborg's public-domain `hwzip` work. This document is an independent description for
interoperability, research, and comparative implementation.
