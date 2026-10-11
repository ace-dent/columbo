# deft4j beta 17: source-verified Deflate optimization methods

This document describes how deft4j optimizes an existing Deflate stream, for projects that
reproduce, compare with, or learn from its post-processing methods. The Java source is the
ground truth, so the document keeps the Java candidate order and intermediate states
rather than treating deft4j as an unordered set of ideas.

## Reference source and evidence policy

The analyzed snapshot is [NeRdTheNed/deft4j at commit
`fe818cd22540c53350b3ed3e13f50947dd23d194`](https://github.com/NeRdTheNed/deft4j/tree/fe818cd22540c53350b3ed3e13f50947dd23d194).

| Property | Value |
|---|---|
| Project version | `1.0.0-beta-17` |
| Upstream commit | `fe818cd22540c53350b3ed3e13f50947dd23d194` |
| Java compatibility target | Java 8 |
| Main author/copyright | Ned Loynd, 2023 |
| Original-code license | BSD Zero Clause |
| Huffman-derived code license | MIT, Ridge Shrubsall |

Line references and names apply to that exact snapshot, and executable Java takes
precedence over comments and README statements. Higher-level algorithms combine several
Java methods and were checked against the snapshot's complete call graph.
**Implementation advice** is guidance for ports, not a claim about the Java code.

## Scope and summary

This document covers deft4j's optimization of an already encoded Deflate stream. The
optional `deft4j-compress` module, which can run the JVM Deflater, JZLib, JZopfli, and
CafeUndZopfli first, is out of scope: none of those compressors is needed to reimplement
the post-processor. Containers are covered only far enough to show which Deflate streams
reach the shared optimizer.

deft4j is a token-preserving Deflate optimizer with a large, ordered candidate graph. For
each source block it can:

1. optimize the existing token and header representation;
2. compare a stored representation;
3. rebuild dynamic Huffman tables from the current tokens;
4. replace strictly more expensive matches with literals;
5. keep no-larger match removals as seeds for later tree rebuilding;
6. remove one whole match-length-symbol family as an exploratory seed;
7. regenerate the dynamic header under many RLE strategies;
8. repeat prune/rebuild operations to a strict fixed point; and
9. select the strictly smallest complete block at its current alignment.

The winning block goes through the same ladder again until no more bits are saved. An
optional second phase merges adjacent blocks where the type rules allow, runs the full
block optimizer on each merge, and accepts only a strict local saving. A quirk in
empty-block removal can end either walk early (see
[Empty-block removal and early termination](#empty-block-removal-and-early-termination)).

Intermediate states matter: a no-larger rewrite that is not the current winner can lead to
a better table and header later, so keeping only the best state after each operation does
not reproduce deft4j
([details](#strict-winner-and-non-winning-states)).

## Source model and bit accounting

### Linked block list

`DeflateStream` stores blocks as a doubly linked list of `DeflateBlock`.
`DeflateStream.parse()` reads each 3-bit `BFINAL/BTYPE` block header, constructs a stored,
fixed, or dynamic block object, and links it to its predecessor
(`DeflateStream.java:61-126`; `DeflateBlock.java:64-144`). Through these links,
`DeflateBlock.readSlice()` retrieves match bytes from the current or earlier blocks,
including overlapping matches (`DeflateBlock.java:146-236`), so each parsed match keeps its
exact decoded bytes for later match-to-literal methods.

### Token representation

`DeflateBlockHuffman` stores a `List<LitLen>` (`DeflateBlockHuffman.java:20-46`). A
`LitLen` is a literal or end-of-block when `dist == 0`, or a match when `dist > 0`, with
`litlen` holding the decoded match length. Each item also carries `decodedVal`, the bytes
it produces.

A match keeps an `edgecase` flag when length 258 was encoded with literal/length symbol 284
rather than 285 (`LitLen.java:29-58`; `DeflateBlockHuffman.java:839-844`), and re-emission
preserves that choice through `Constants.len2litlen(len, edgecase)`. This is deft4j
behavior, not a position on the disputed reading of symbol 284; a parity implementation
must keep the source symbol or an equivalent flag.

### Cached sizes

Huffman blocks maintain:

- `sizeBits`: block body size excluding the 3-bit block header;
- `litlenSizeBits`: token payload size, including match extra bits; and
- `dynamicHeaderSizeBits`: dynamic header after `BTYPE`, including the 5-, 5-, and 4-bit
  count fields.

`DeflateStream.getSizeBits()` adds three bits per block, then calls
`block.getSizeBits(position_after_header)` (`DeflateStream.java:171-182`). Huffman block
size does not depend on alignment; stored size includes the padding to the next byte
boundary (`DeflateBlockUncompressed.java:69-74`).

Every candidate in `DeflateStream.optimiseBlock()` is scored as a complete block body at
the current post-header bit position. The winner callback updates only when
`newSizeBits < currentSmallest`, so ties keep the earlier block
(`DeflateStream.java:343-368`).

### Copy-on-write state

`DeflateBlockHuffman.copy()` initially shares token lists, decoded data, and header
arrays; `ensureDidCopyLitLens()` and `ensureDidCopyRLEPairs()` make private lists before
mutation (`DeflateBlockHuffman.java:298-310,1174-1205`). However a port shares state,
candidates must behave as independent states.

## Parsing and writing an existing Deflate stream

`DeflateStream.parse()` parses the three defined block types in the standard way and
rejects reserved `BTYPE` 3 (`DeflateStream.java:72-125`; stored blocks in
`DeflateBlockUncompressed.java:22-35`). Dynamic headers keep repeat symbols 16, 17, and 18
as explicit `rlePairs`, including their decoded length bytes
(`DeflateBlockHuffman.java:892-1009`). Symbol 16 with no previous length, or any repeat
that overruns the advertised `HLIT + HDIST` count, is rejected.

`decodeStream()` decodes one canonical Huffman symbol at a time and records literals,
end-of-block, and matches with their extra bits; match bytes are materialized with
`readSlice()` and appended to the block's decoded buffer
(`DeflateBlockHuffman.java:778-890`). Only literal/length symbols through 285 and distance
symbols through 29 are accepted as data, even though dynamic length arrays may be as wide
as 288 and 32 entries.

`Huffman.buildCodes()` assigns canonical codes by increasing length, then increasing
symbol (`Huffman.java:30-64`). The output method reverses each code with `Util.rev()`
because Deflate packs Huffman codes starting with their most-significant bit, while the
bit writer fills bytes from the least-significant bit (`Huffman.java:207-213`).

`DeflateStream.write()` sets `BFINAL` only on the last surviving block, then byte-aligns
the stream (`DeflateStream.java:128-145`).

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

When the token's `edgecase` flag is set, the original length-258 symbol 284 is used.

### Strict match-to-literal replacement

The shared implementation is `replaceWithLiteralsIfSmaller()`.
`DeflateBlockHuffman.optimise()` first calls
`replaceBackrefsWithLiteralsIfSmaller(false, true)`
(`DeflateBlockHuffman.java:460-468`). For each match:

```text
match_cost    = current encoded match cost
literal_cost  = sum(current literal code length for each decoded byte)
```

The match is replaced only when every required literal has a non-zero code and
`literal_cost < match_cost`. The loop gives up as soon as the running literal cost reaches
the match cost (`DeflateBlockHuffman.java:222-296`). Replacement inserts one literal token
per decoded byte and updates the cached payload and block sizes. It never searches for a
different match or splits the match.

### No-larger match pruning

With `prune == true`, the same helper replaces a match when `literal_cost <= match_cost`.
`recodeHuffmanLessMatches()` uses this mode before rebuilding the Huffman tables
(`DeflateBlockHuffman.java:655-657`). An equal-cost expansion does not shrink the block
immediately, but it changes the symbol frequencies, so the next tree or header may be
smaller.

| Path | Local condition | Immediate tree rebuild |
|---|---|---|
| `optimise()` | literals strictly cheaper | no |
| `recodeHuffmanLessMatches()` | literals no larger | yes |

A port that applies only strict expansion misses deft4j states; a no-larger expansion is a
seed, not a winner, until a complete block is scored.

### Missing literal codes

Both modes reject an expansion when any decoded byte has code length zero in the current
literal table, log a possible missed optimization, and leave the match unchanged
(`DeflateBlockHuffman.java:238-250`). A TODO comment proposes adding missing codes, but
this snapshot does not.

## Length-symbol-family removal

`removeDistLitLeastExpensive(int mode)` creates a broader exploratory state
(`DeflateBlockHuffman.java:372-458`). Despite its name, it groups matches by
**literal/length symbol**, not by distance symbol. For each family:

```text
family_delta = sum(literal replay cost - current match cost)
family_count = number of matches in the family
```

A family is ineligible if any of its matches needs a literal absent from the current
table. Among eligible families, mode 0 selects the smallest `family_delta`
(`leastExpPruned()`) and mode 1 the smallest `family_count` (`leastSeenPruned()`). Ties
keep the first family, the lower length-symbol index. All matches of the chosen family are
expanded to literals, even if `family_delta` is positive: a later Huffman rebuild and
header rewrite decide whether the complete block becomes smaller.

Each call removes one family; the candidate ladder and repeated block passes can chain
least-expensive and least-seen removals.

## Rebuilding literal/length and distance trees

`recodeHuffman()` counts symbols over 286 literal/length and 30 distance entries and trims
trailing zero frequencies (`DeflateBlockHuffman.java:670-743`). The literal/length tree is
`new HuffmanTree(litFreq, 15)`. The distance table has two special cases:

- no used distance symbol: a one-entry table whose only length is zero; and
- exactly one used distance symbol: that symbol gets code 0 of length 1.

Otherwise the distance tree is an ordinary depth-limited tree. `MIN_LIT_CODES` and
`MIN_DIST_CODES` are both zero in this snapshot, so no dummy codes are forced beyond the
builder's own two-leaf rule.

`recodeToHuffmanInternal()` then installs the tables and recomputes every token cost; its
wrapper `recodeToHuffman()` regenerates a dynamic result's header with the default packing
strategy (`DeflateBlockHuffman.java:745-770`).

## deft4j's Huffman tree builder

`HuffmanTree` (`HuffmanTree.java:31-191`) builds literal/length, distance, and code-length
trees.

### Initial leaves and dummy leaves

Every positive frequency becomes a `LeafNode`. The builder then makes sure the priority
queue holds at least two leaves:

```text
index = 0
while queue size < 2:
    if index is beyond the alphabet or frequency[index] == 0:
        add synthetic leaf(index, frequency 1)
    index++
```

With one used symbol, this picks the earliest zero-frequency symbol. A synthetic leaf whose
index lies beyond the table is dropped when codes are copied into the fixed-size result.

### Heap ordering

Nodes compare by `this.weight - other.weight` only: no symbol, height, insertion-sequence,
or node-type tie key (`HuffmanTree.java:194-221`). The builder repeatedly polls the two
least-weight nodes and inserts their sum.

Equal-weight order therefore comes from `java.util.PriorityQueue`'s internal heap
operations, and the Java collection contract does not promise a portable order among
equal elements. A port that only needs equivalent compression can use any deterministic
tie order; a bit-for-bit port must record the runtime used for the reference build or
reproduce its queue operations.

### Source-specific maximum-depth repair

While `maxDepth > limit`, deft4j repairs the tree (`HuffmanTree.java:74-127`):

1. take the first leaf at the deepest depth as `leafA`;
2. take its sibling leaf `leafB`;
3. remove their parent by moving `leafB` up to the grandparent;
4. scan depths from `maxDepth - 2` toward the root (depth 1); at the first depth that
   holds a leaf, take its first stored leaf as `leafC`;
5. replace `leafC` with a new internal node containing `leafA` and `leafC`; and
6. traverse the whole tree again to rebuild the depth map.

This rewrites the tree structure; it is not the histogram overflow repair of zlib or
DeflOpt.

### Canonical table generation

`getTable()` walks depths in increasing order, sorts each depth's leaves by symbol, and
assigns consecutive canonical codes (`HuffmanTree.java:160-191`). Child orientation
affects the depth repair, but not the canonical codes at a finished depth.

## Dynamic-header methods

### Header cost

For counts `numLitlenLens`, `numDistLens`, and `numCodelenLens`, the dynamic header body
is:

```text
5 + 5 + 4
+ 3 * numCodelenLens
+ sum(code_length_code_bits[RLE symbol] + RLE extra bits)
```

Repeat extra-bit widths are 2 for symbol 16, 3 for 17, and 7 for 18
(`DeflateBlockHuffman.java:133-163,892-1009`). The 3-bit block header is counted by
`DeflateStream`, not `dynamicHeaderSizeBits`.

### Remove trailing HCLEN entries

`removeDynHeaderTrailingZeroLenCodelens()` removes zero entries from the end of the
19-symbol code-length permutation, saving three bits each
(`DeflateBlockHuffman.java:334-369`). It recurses until the last transmitted entry is
non-zero. The writer asserts a final count between 4 and 19; the trimming routine has no
lower bound of its own, and source construction and valid input keep the count legal.
`removeTrailingHeaderCodes()` subtracts the saved bits from the block and dynamic-header
totals.

### Strict RLE-token replacement

`optimiseHeader()` trims the code-length-code span, then calls
`replaceRLERunsWithLiteralsIfSmaller(false, true)` (`DeflateBlockHuffman.java:471-475`).
For a repeat token:

```text
repeat_cost   = code-length code for 16/17/18 + its extra bits
explicit_cost = sum(code-length code for each represented length value)
```

It replaces only when the explicit form is strictly smaller and every explicit value has a
code in the current code-length table. It does not rebuild the code-length tree
afterward.

### Recode the existing RLE stream

`recodeHeader()` keeps the current RLE tokenization, counts its symbols, builds a new
19-symbol code-length tree through `Huffman.ofRLEPacked()`, trims the code-length-code
span, and rescores the header (`DeflateBlockHuffman.java:579-629`; `Huffman.java:117-134`).

### No-larger RLE pruning followed by recoding

`recodeHeaderToLessRLEMatches()` expands repeat tokens whose explicit form is no larger,
then calls `recodeHeader()` (`DeflateBlockHuffman.java:631-635`). As with payload pruning,
equal-cost changes give the next tree a different frequency distribution. The order is
part of the method:

```text
rewrite/retain RLE tokens
-> optional no-larger repeat removal
-> rebuild code-length tree
-> final strict header optimization
```

Applying the no-larger rewrite under an unrelated code-length tree does not reproduce the
Java state.

## Regenerating the dynamic-header RLE

`HuffmanTable.packCodeLengths()` concatenates the literal/length and distance length
arrays and packs each equal-value run (`HuffmanTable.java:32-159`). `rewriteHeader()` then
builds the code-length tree for that token stream and updates the block
(`DeflateBlockHuffman.java:480-577`).

### Zero runs

Unless disabled, a zero run is packed as:

1. symbol 18 with the largest legal count from 138 down to 11, repeated as needed;
2. symbol 17 with the largest legal count from 10 down to 3, repeated as needed;
3. the general symbol-16 stage for the remaining zeros after one explicit zero, unless
   repeat-16-for-zeros is disabled; and
4. any remaining zeros explicitly.

The controls are `noZRep2` (no symbol 18), `noZRep` (no symbol 17), `noRep` (no symbol
16), and `noRepZeros` (no symbol 16 for zero runs).

### Non-zero runs and ordinary repeat 16

Unless `noRep` is set, the packer emits the length once, then covers the rest of the run
with the largest legal symbol-16 counts from 6 down to 3, and writes any remainder
explicitly.

### OHH 7- and 8-value alternatives

With `ohh` enabled, two cases can replace the greedy six-plus-explicit shape:

- exactly eight remaining copies can use `4 + 4` symbol-16 runs when `use8` is enabled;
  and
- exactly seven remaining copies can use `4 + 3` symbol-16 runs when `use7` is enabled.

An alternate eight split of `5 + 3` exists behind `alt8`, but `DeflateStream.TRY_ALT_8` is
false, so beta 17 searches `4 + 4`, not `5 + 3` (`DeflateStream.java:243-263`;
`HuffmanTable.java:48-57,110-145`).

## The 56-way header option grid

`DeflateStream.addOptimisedRecoded()` enumerates a nested boolean grid for every
applicable dynamic state (`DeflateStream.java:265-317`). With `TRY_ALT_8 == false`, it
produces 56 parameter combinations per input state. The loop order is:

1. `noRepZeros`: false, true;
2. RLE `prune`: false, true;
3. `noRep`: false, true, except forced false with `noRepZeros`;
4. `noZRep`: false, true, except forced true with `noRepZeros`;
5. `noZRep2`: false, true;
6. `ohh`: true, false;
7. for `ohh`, `use8`: true, false; and
8. for `ohh`, `use7`: true, false.

The OHH branch skips the all-false `use8/use7` combination and any combination with
`noRep`. `alt8` takes only its default false value.

For each combination, `optimiseBlockDynBlock()` does (`DeflateStream.java:184-198`):

```text
copy the input dynamic block
rewriteHeader(selected packing flags)
if RLE prune is enabled:
    recodeHeaderToLessRLEMatches()
optimiseHeader()
submit the complete block candidate
```

## Recode/prune state families

### One-shot Huffman recoding

`recodedHuffman(block, false)` copies a block and calls `recodeHuffman()`;
`recodedHuffman(block, true)` copies it and calls `recodeHuffmanLessMatches()`, keeping
no-larger match expansions before building new payload tables
(`DeflateStream.java:200-210`).

### Fixed-point prune/recode

`recodedHuffmanFull()` repeats the pruned recode (`DeflateStream.java:212-229`):

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

Only a strict complete-block improvement advances the loop. This method discards the
equal-size token changes of its final, losing iteration; the separate one-shot pruned
state still exists elsewhere in the ladder.

### `addOptimisedRecoded()` base states

Before running the header grid, `addOptimisedRecoded()` builds an insertion-ordered
`LinkedHashMap` of:

1. an optimized copy of the input state;
2. an ordinary Huffman-recoded state, then normally optimized;
3. a one-shot no-larger-pruned Huffman-recoded state, then normally optimized; and
4. when the fixed-point loop made at least one strict improvement, the full pruned state,
   then normally optimized.

The wrappers `optimiseBlockCopyHelper()` and `optimiseBlockHelper()` distinguish "copy
then optimize" from "optimize this already-private state". `DeflateBlockHuffman` does not
override `equals()` or `hashCode()`, so the map keys use object identity and equivalent
states are not de-duplicated (`DeflateStream.java:265-277`). Each base state gets the full
grid, so one call can submit 168 candidates with three base states or 224 with four.

## Complete per-block candidate ladder

`DeflateStream.optimiseBlock()` (`DeflateStream.java:343-490`) starts with the original
block as the incumbent and submits every generated candidate to one strict complete-block
winner callback.

### Common candidates

For every block it first tries:

- `optimiseBlockNormal()`: copy, run the block's ordinary `optimise()`, and keep the copy
  only if it reports a positive saving; and
- for every non-stored block of at most 65,535 decoded bytes, a stored representation.

A stored source block has no Huffman ladder, because it is not a `DeflateBlockHuffman`
state.

### Establishing a dynamic Huffman seed

For a dynamic source, the original and normally optimized dynamic states become seeds.
For a fixed source, deft4j copies the block and calls `recodeHuffman()` to make a dynamic
seed, and also normally optimizes that seed, so fixed input enters the dynamic ladder
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

The direct fixed-Huffman code inside this callback is commented out.

### `runOptimisationsCallbackMulti`

The outer callback submits and runs the inner callback on the seed itself, an ordinary
Huffman-recoded copy, a one-shot no-larger-pruned Huffman-recoded copy, and, when it
strictly improves, the fixed-point pruned/recode result (`DeflateStream.java:443-463`).

### Top-level seed order

The multi-callback runs on the default dynamic seed, the normally optimized dynamic seed
(when it exists), a least-expensive-family seed, and a least-seen-family seed. For an
originally dynamic block, `toFixedHuffman()` also copies the default state, calls
`recodeToFixedHuffman()`, runs the ordinary strict optimization, and submits the fixed
candidate (`DeflateStream.java:464-487`). A fixed source already has a fixed incumbent.

A non-trivial Huffman block therefore gets thousands of candidates. The ladder has no
timeout, token-count gate, decoded-size gate, or filename rule; runtime limits in a port
are scheduling policy, not deft4j semantics.

### Strict winner and non-winning states

Equal-cost intermediate states inside no-larger-prune branches never replace the
incumbent but can produce later winners. A faithful implementation therefore tracks both
the ordered work queue of states that still have descendants and the strictly smallest
complete block so far; collapsing the queue to the current winner loses real deft4j
paths.

## Repeated optimization of each block

Unless it removes an empty block, `DeflateStream.optimise()` walks the list left to right
(`DeflateStream.java:496-566`). For a non-empty block, or the only remaining block, it runs
the full `optimiseBlock()` ladder. When the result differs and is strictly smaller, it
replaces the list node and runs the same position again; it advances only after a pass
with no strict saving. A winner from least-seen family removal, for example, can become
the next pass's default seed and receive another family removal or prune/recode sequence.

### Empty-block removal and early termination

The source keeps an empty block only when it is the first and only remaining block:

```text
optimize block if decoded length > 0
or if it is the first and only remaining block
otherwise remove it
```

In beta 17, `DeflateBlock.remove()` calls the virtual `discard()` method, and both concrete
block types clear the removed node's `nextBlock` link (`DeflateBlock.java:36-51,134-144`).
`DeflateStream.optimise()` then executes `currentBlock = currentBlock.getNext()`, receives
null, and ends the per-block walk at the first empty block it removes
(`DeflateStream.java:533-559`). This follows the code, not the README's general statement
that empty blocks are removed.

With merging enabled, `mergeBlocks()` starts a new walk from `firstBlock` and may remove
one more empty block, but it has the same remove-then-`getNext()` shape and stops after
that removal (`DeflateStream.java:620-646`). One optimizer call therefore does not, in
general, reduce an all-empty list to one block, and may not optimize every block after a
removed empty block.

Implementation advice: saving the successor before `remove()` and continuing is the natural
fix, and the sole-block condition still prevents an empty stream. It is not exact beta-17
parity; tests should assert whichever behavior is intended.

### Position-accounting quirk

The loop adds the block's size to `pos` before deciding whether to repeat the same node
and does not rewind `pos` when a winning replacement triggers another pass. `mergeBlocks()`
has the same shape after an accepted merge (`DeflateStream.java:504-559,576-646`). Huffman
body sizes ignore alignment, so this is usually invisible, but stored padding can depend on
it. An exact clone reproduces this progression; a port that recomputes the true position
differs from the source and should be tested separately.

## Stored-block candidates and merging

### Stored representation

`DeflateBlock.asUncompressed()` converts any Huffman block to a stored block of its decoded
bytes (`DeflateBlock.java:53-62`); `optimiseBlock()` submits it when the decoded length is
at most 65,535 bytes. For post-header alignment `a`,
`DeflateBlockUncompressed.getSizeBits(a)` returns:

```text
padding from a to the next byte boundary
+ 16 + 16                  LEN and NLEN
+ 8 * decoded_size
```

`DeflateStream` adds the 3-bit block header. The candidate wins only on a strict saving.

### Asymmetric stored merge

`DeflateBlockUncompressed.canMerge()` lets a **stored current block** absorb a next block
of any type when the combined decoded data is at most 65,535 bytes; the result is one
stored block (`DeflateBlockUncompressed.java:98-117`). A Huffman block followed by a stored
block cannot merge through the Huffman merge method. Ports that require both blocks to be
stored do not match the source.

## Huffman-block merging

`DeflateBlockHuffman.canMerge()` lets a fixed or dynamic current block merge with a fixed
or dynamic next block (`DeflateBlockHuffman.java:1226-1271`). The merge:

1. converts each dynamic child to fixed Huffman independently;
2. copies the first fixed child;
3. removes its end-of-block token;
4. appends the second child's token list, including its end-of-block;
5. combines the decoded byte arrays; and
6. recomputes the fixed payload size.

The merged object is fixed. `mergeBlocks()` then runs it through the full `optimiseBlock()`
ladder, where it may become dynamic or stored again or stay fixed. This is broader than
DeflOpt's exact fixed/fixed boundary deletion.

The beta-17 README lists stored/Huffman and same-tree dynamic merging as future work, but
the Java already implements the stored absorption and fixed-converting merge above; only a
direct same-tree dynamic merge is missing. For parity, the Java predicates and merge bodies
take precedence over the README.

## Adjacent merge walk

After per-block optimization, `DeflateStream.optimise(true)` calls `mergeBlocks()`
(`DeflateStream.java:562-565,568-650`), which walks the optimized list left to right.
Merging is on by default: the no-argument library method delegates to `optimise(true)`,
and the command-line `--merge-blocks` option defaults to true; it can be disabled
(`DeflateStream.java:492-496`; `Optimise.java:33-34`).

For a mergeable current/next pair:

```text
unmerged = current_body_bits_at_position
         + 3
         + next_body_bits_at_position_after_current

merged_candidate = optimiseBlock(current.merge(next), current_position)
saving = unmerged - merged_candidate_body_bits
```

The 3 bits are the second block's header. A merge is accepted only when `saving > 0`. The
merged node replaces both inputs and stays current, so it can merge again with its new
neighbor; the walk advances only when no strict merge wins. This is a greedy left-to-right
walk, not an optimization over all partitions or non-adjacent pairs, so an early merge can
change all later opportunities.

## Raw-stream keep-original rule

`Deft.optimiseDeflateStream()` parses a raw byte array and runs the optimizer with merging
enabled (`Deft.java:15-34`). It serializes the result only when `DeflateStream.optimise()`
reports a positive bit saving. If parsing fails, an `IOException` occurs, or there is no
saving, it returns the original array object; other unchecked failures are not caught.

The gate is in bits, not whole bytes, so a result that saves bits but has the same byte
length can replace the original. Container commands parse and write their containers
separately, so their outer behavior should not be inferred from this helper.

## Complete reconstructed algorithm

This pseudocode follows the source control flow, abstracting Java object ownership:

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
position; it does not stop other queued states from generating descendants.

## Complexity

The nesting multiplies: 56 header configurations for each of three or four base states,
four helper calls per inner callback, three or four states per multi-callback, several
top-level seeds, repeated passes for winning blocks, and the full ladder for every merge
candidate. One dynamic block can cause thousands of candidate builds. The source has no
time budget and describes itself as very inefficient; the search is finite but slow on
large or many-block streams.

Implementation advice: a port can share immutable tokens, frequency vectors, canonical
tables, and equivalent header scores, and can structurally de-duplicate states after
fixing the Java insertion and tie order.

## Container stream discovery

Every container exposes a `List<DeflateStream>`, and `DeflateFilesContainer.optimise()`
runs `DeflateStream.optimise(mergeBlocks)` on each in order
(`DeflateFilesContainer.java:14-43,65-73`). No container has its own Deflate search.

- **Raw Deflate and zlib:** `RawDeflateFile` holds one raw stream. `ZLibFile` checks
  compression method 8, FCHECK, and the absence of a preset dictionary, parses one raw
  stream, keeps the Adler-32 trailer, and recalculates Adler-32 on output
  (`RawDeflateFile.java:11-35`; `ZLibFile.java:14-109`).
- **GZIP:** `GZFile` parses one method-8 member, including `FEXTRA`, `FNAME`, `FCOMMENT`,
  and `FHCRC`, and recalculates CRC-32 and ISIZE on output (`GZFile.java:13-87,181-186`).
  The class has no loop over concatenated members.
- **ZIP:** `ZipFile` loads the archive with lljzip and creates a `DeflateStream` for each
  local entry whose method is Deflate; other entries are left to the archive library
  (`ZipFile.java:27-140`). New bytes and compressed sizes go into the local and linked
  central headers.
- **PNG and APNG:** `PNGFile` exposes separate streams for the concatenated `IDAT` zlib
  stream, each APNG frame's concatenated `fdAT` stream, and zlib-compressed `zTXt`,
  `iCCP`, and compressed `iTXt` chunks. It validates chunk CRCs and APNG sequence/order,
  rebuilds `IDAT`/`fdAT` chunks, renumbers APNG sequence fields, reinserts optimized
  metadata streams, and recalculates CRCs (`PNGFile.java:19-215,262-369,377-605`).

## Out of scope or not implemented

The beta-17 existing-stream optimizer does not include the following; some appear as
TODOs, README future work, optional compressor modules, or methods of other projects.
Label them as extensions, not deft4j parity.

- fresh LZ77 match discovery, or replacing one match with one or more different matches;
- adding a missing literal code to expand a match;
- arbitrary block splitting, global optimization over block partitions, or merging
  non-adjacent blocks;
- direct merging of dynamic blocks under an identical tree, as the README proposes;
- `TRY_ALT_8`'s dormant `5 + 3` header split;
- DeflOpt's four height-aware/frequency-only heap variants or histogram depth repair;
- Defluff package-merge or shortest-path header algorithms;
- finished-tree, block-splitting, or recursive searches added by another implementation;
- filename-, file-size-, token-count-, or timeout-based semantic gates; and
- JVM Deflater, JZLib, JZopfli, CafeUndZopfli, libdeflate, or any other high-level
  recompression engine.

## Suggested validation

Decode every output with several independent decoders, and validate raw Deflate before
containers (container byte identity does not prove token/tree/header parity). Beyond the
ties, missing codes, and boundaries named above, test:

- matches across block boundaries, overlapping matches, maximum distance and length, and
  length 258 as both symbol 284 and 285;
- tied and locally costly family selections, and depth-overflow trees;
- each RLE control flag, the exact seven- and eight-repeat OHH cases, and the count of 56
  active configurations;
- candidate order, by tracing candidate names from a Java build with fine logging and
  comparing the winner after each `optimiseBlock()` pass, including a case where an
  equal-cost pruned state leads to a later strict win; and
- empty first, middle, last, interspersed, and all-empty lists; stored absorption of each
  following type; every fixed/dynamic merge pair; and a first merge that changes the next
  opportunity.

## Source index

The links point to the upstream commit used as ground truth.

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

The candidate graph was traced method by method through the exact snapshot, and the base
module was compiled with `javac`. The map covers the active post-processing paths reached
through `optimiseBlock()`, repeated stream optimization, and the adjacent-block merge pass.
The main reproducibility caveat is `PriorityQueue` equal-weight order (see
[Heap ordering](#heap-ordering)). A production port may add stronger validation, corrected
alignment tracking, state de-duplication, and bounded scheduling; label those as
departures from beta-17 parity and verify them against the source-defined candidate graph.

## Attribution

deft4j's original code is Copyright (c) 2023 Ned Loynd and licensed under the BSD Zero
Clause License. Its Huffman encoding and decoding code is largely derived from Ridge
Shrubsall's `deflate-impl` work under the MIT License. The Deflate parser/writer also credits
Hans Wennborg's public-domain `hwzip` work. This document is an independent description for
interoperability, research, and comparative implementation.
