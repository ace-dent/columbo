# Defluff 0.3.2: reverse-engineered methods

This document describes the Deflate post-processing methods of Defluff 0.3.2 for projects
that reproduce, compare with, or learn from them. The original executable is the ground
truth: the 32-bit Windows build was disassembled and its optimizer was run directly under
an x86 emulator. Reconstructed pseudocode and independent implementations were used to
test the interpretation but do not override the executable.

## Reference binaries and evidence policy

| Property | Windows specimen | Darwin specimen |
|---|---|---|
| Product version | Defluff 0.3.2 | Defluff 0.3.2 |
| Author shown by the program | Joachim Henke | Joachim Henke |
| Format | PE32 console executable, Intel 80386 | Mach-O executable, Intel i386 |
| SHA-256 | `8847fba3ff5bc23fd4d5178f5383b910a33302266bd2354035a00f4f10e2d54f` | `c647ac35c9d0c173141fc9381a1d74c9660eafcff1ddc7617c13d354862d428c` |
| Local copy | [Windows executable](binaries/defluff/defluff032/defluff-0.3.2-windows-i686.exe) | [Darwin executable](binaries/defluff/defluff032/defluff-0.3.2-darwin-x86) |

**Address convention.** Every hexadecimal code address or range, such as `0x4066f0`, is an
**absolute virtual address in `defluff-0.3.2-windows-i686.exe`**, the principal specimen.
Subtract the preferred PE base `0x00400000` to obtain an RVA. The values are not file
offsets or Darwin addresses (the Darwin build is kept only as a second artifact) and do
not apply to another release, build, or repacked executable. The executable is stripped,
so routine names here were assigned during reverse engineering.

Addresses, constants, and branch conditions are observed in the disassembly. Higher-level
algorithms are reconstructed from several instruction sequences and checked against
executable behavior wherever practical; for the raw optimizer this includes running the
original instructions under Unicorn
(see [Exact emulation with Unicorn](#exact-emulation-with-unicorn)). **Implementation
advice** is guidance for ports, not a claim about Defluff's source.

## Scope

Defluff optimizes an existing Deflate stream. It parses each source block, keeps its
decoded literals and matches, and chooses a cheaper serialization from a small
deterministic family. It runs no high-level compressor and searches for no new LZ77
matches. Its methods are:

1. choose stored, fixed, or dynamic output for an existing block;
2. rebuild deterministic length-limited Huffman trees;
3. replace an existing match with its literal bytes when that is cheaper under the tree
   being tested;
4. rebuild the trees once from the resulting frequencies;
5. feed code-length-code prices back into dynamic-header run-length encoding exactly four
   times; and
6. when a block is eligible, try a compatibility-sensitive representation of match
   length 258.

Defluff makes one decision per source block and keeps source block boundaries, except
that a non-final source block over 65,535 bytes may be written as several non-final stored
blocks (a format necessity, not a split search). It does not remove or merge blocks,
search split points, or iterate tree/token feedback to a fixed point.

## Conventions

"Decoded size" is the number of uncompressed bytes a block produces, not its number of
LZ77 tokens. "Payload cost" includes Huffman code bits and length/distance extra bits.
"Complete block cost" also includes the 3-bit block header and, as applicable, stored
padding or the dynamic tree description. Comparison strictness is part of the algorithm,
so this document distinguishes `<` from `<=` even where both decompress identically.

## High-level algorithm

The raw Deflate optimizer begins at `0x4066f0`:

```text
initialize the fixed Huffman tables
allocate a decoded-slot buffer and retain up to 32 KiB of prior output

repeat:
    reset block frequencies and candidate state
    read BFINAL and BTYPE and parse stored, fixed, or dynamic input
    decode the block into byte-addressable slots while recording its matches
    normalize any input use of symbol 284 for match length 258 to symbol 285
    choose and write one block (see "Complete decision procedure")
    retain at most the final 32 KiB of decoded history
until the source BFINAL bit was set

flush output and release the decoded-slot buffer
```

Defluff rebuilds every block: the exact source bit sequence is never a candidate, and even
the shortcut writer may replace a match with literals. An equal-size output can therefore
differ from the input.

## Parsing and retained block state

### Fixed-table initialization

`0x4066fb..0x4067db` constructs the standard Deflate fixed tables:

| Literal/length symbols | Code length |
|---|---:|
| `0..143` | 8 |
| `144..255` | 9 |
| `256..279` | 7 |
| `280..287` | 8 |

All 32 fixed distance codes have length 5. The decoder must still reject the reserved data
symbols that have fixed bit patterns.

### Stored, fixed, and dynamic input

All three block types are parsed in the standard way; stored bytes are copied into the
decoded buffer, and fixed and dynamic blocks share one data-token decoder. Reserved
`BTYPE=3`, an inconsistent stored length, an invalid tree, an impossible symbol, or a
distance beyond the available history returns an error. A compatible implementation should
fail the stream rather than repair it.

### Decoded-slot representation

The executable stores one 16-bit slot per decoded byte, not one record per token. The low
byte of every slot is the byte produced at that position; a match's first slots also carry
encoded length/distance metadata in their upper bytes. Consequently:

- the cutoff at `0x406a85` counts decoded bytes;
- expanding a match needs no new LZ decoding, because its literal bytes are already in the
  slots; and
- writing a stored candidate is a low-byte copy of the slots.

Implementation advice: a token vector plus a decoded-byte buffer works if it preserves the
same token identities, expanded bytes, frequencies, and cross-block history.

### Buffer growth and streaming I/O

`0x4067e4..0x406802` allocates an initial `65,536`-slot buffer and initializes the
retained history length to `32,768`. The growth path at `0x40663a` reallocates the slot
buffer when one block plus retained history no longer fits.

Input and output pass through separate 4 KiB buffers. The bit reader and writer keep their
partial-byte state across a refill or flush, so buffer boundaries are not block
boundaries.

### Cross-block history

Back-references may reach into earlier blocks. After emitting a block:

- if its decoded size exceeds `32,767`, the next block retains its last `32,768` bytes;
- if the old history plus the new block fits, the bytes are appended in place; and
- otherwise `0x40723f..0x407279` shifts the final `32,768` bytes into the retained-history
  position.

## The 25-byte fixed shortcut

At `0x406a77..0x406a8e`, Defluff computes:

```text
decoded_size = (end_slot_pointer - start_slot_pointer) / 2
```

If `decoded_size <= 25`, control jumps to `0x4071db`, which writes the source BFINAL bit,
`BTYPE=1`, and the block under the fixed tables. This path scores no stored output, builds
no dynamic tree, and does not call the payload-cost routine at `0x403250`. The writer at
`0x4035b0` still makes the same per-match comparison while emitting: it replaces a match
when all required literals have codes and their total is strictly smaller than the fixed
match. A tie keeps the match.

An empty block also takes this path and is written as a fixed block containing only
end-of-block; Defluff does not delete empty blocks. The test counts decoded bytes, not
tokens: one long match can exceed 25 bytes.

## Complete block candidates

For a block larger than 25 decoded bytes, the candidate array is ordered stored, fixed,
dynamic. The scan at `0x406c61..0x406c7d` replaces the current winner only when a later
cost is strictly smaller, so stored wins a complete-cost tie with fixed or dynamic, and
fixed wins a tie with dynamic. The source BFINAL bit and the selected two-bit BTYPE are
written at `0x406c7d..0x406c96`. The fixed and dynamic forms may mix retained matches and
literal expansions.

### Stored candidate

The cost at `0x406a94..0x406ace` includes:

- alignment after the three-bit block header;
- one `LEN/NLEN` pair for each group of at most `65,535` decoded bytes; and
- one extra byte for every later group, holding its block header and alignment.

The writer at `0x406ca8..0x40723a` copies the low byte of each slot and fragments at
exactly `65,535` bytes. Between fragments it writes a zero byte: the low three bits are
`BFINAL=0, BTYPE=00`, and the remaining five bits are stored-block alignment.

The source BFINAL bit is written on the first output block, so a final source block cannot
become several stored blocks: a later fragment would follow a block already marked final.
At `0x4074aa`, a final source block skips the stored candidate (index zero) when its cost
exceeds `0x80022`, the maximum aligned cost of one 65,535-byte stored block, so it may use
stored output only when it fits one stored block. This is a limit of this writer, not of
Deflate.

### Fixed candidate

The fixed candidate costs the existing tokens under the fixed tables with `0x403250`,
expanding matches by the strict rule below, and adds the three-bit header. When the block
has no match state that needs the general evaluator, `0x4073c6..0x4074a5` computes the
same fixed cost directly from symbol frequencies and the fixed code-length ranges.

### Dynamic candidates

Defluff considers these dynamic candidates in order:

1. the input dynamic data trees, when the source block was dynamic, with a header
   rebuilt by Defluff;
2. fresh data trees built from the original token frequencies;
3. adjusted data trees built after the fresh-tree match/literal decision; and
4. when eligible, fresh and adjusted versions of the length-258 alias tree.

Every dynamic score includes the complete dynamic header, payload, extra bits, and
end-of-block. The selected tables and token decisions are emitted at
`0x406fa4..0x40710e`. The source header's exact bit pattern is not kept: it is parsed to
code lengths and re-encoded by Defluff's four-pass header encoder.

## Match-to-literal feedback

### One cost comparison

Routine `0x403250` walks the decoded slots under a supplied literal/length table and
distance table. For each existing match:

```text
match_cost = length_code_length
           + length_extra_bit_count
           + distance_code_length
           + distance_extra_bit_count

literal_cost = sum(code_length[decoded_byte] for every byte in the match)
```

The decision order matters:

1. If the length symbol or distance symbol has no code in the candidate table, expand the
   match to literals immediately.
2. Otherwise sum the literal costs in decoded order.
3. If any required literal has no code, keep the match.
4. Stop summing as soon as the literal total reaches the match cost, and keep the match.
5. Expand only when every literal is encodable and the literal total is strictly smaller
   than the match cost.

Equal cost keeps the match, and a missing literal is never treated as zero cost. The
routine returns the payload cost and accumulates new literal/length and distance
frequencies. The writer applies the same table-dependent rule, so scored and emitted
substitutions agree.

### Two-stage dynamic feedback

The dynamic path builds fresh trees from the original frequencies, scores all tokens, and
records the frequencies after strictly cheaper expansions. It then builds adjusted trees
from those frequencies, scores all tokens once more, and stops; there is no convergence
loop. If a match symbol removed in the fresh pass has no code in an adjusted tree, step 1
above keeps that match expanded; other decisions may change with the new code lengths.

### Candidate tie rules inside the dynamic path

The dynamic sub-search uses different tie rules from the final block-type scan:

- the fresh tree replaces the preceding dynamic candidate only when strictly smaller
  (`0x406bb2..0x406bb4`);
- the ordinary adjusted tree replaces it when smaller **or equal**
  (`0x406c39..0x406c43`); and
- both length-258 alias candidates require a strict saving
  (`0x40730f..0x40731b` and `0x4073b1..0x4073b3`).

An equal-cost adjusted tree can therefore become the dynamic state that is serialized, but
an equal-cost dynamic state still loses to stored or fixed in the final scan. These rules
explain some same-size rewrites and must be kept for bit-for-bit parity.

## Defluff's Huffman length builder

The builder at `0x404050` serves literal/length, distance, and 19-symbol code-length
trees. Callers trim trailing zero-frequency entries before data-tree construction. Data
trees have a maximum length of 15, the code-length tree 7.

### Active leaves and trivial cases

Only positive-frequency symbols become leaves, sorted ascending by
`(frequency, symbol_number)`. No active symbols produce no lengths; one active symbol gets
a one-bit code. At the data-tree call sites, trimming makes that symbol the last in the
supplied span; a port should assign the code to the actual active symbol rather than rely
on layout.

### Ordinary two-queue construction

With at least two leaves, the binary runs the linear merge phase of the two-queue Huffman
algorithm:

```text
leaves   = positive-frequency leaves sorted by (frequency, symbol)
branches = empty queue

while total available nodes > 1:
    left  = remove_smallest(leaves, branches)
    right = remove_smallest(leaves, branches)
    append branch(weight(left) + weight(right), left, right) to branches

assign each leaf its depth below the final branch
```

Branches are created in nondecreasing weight, so no heap is needed. When the front leaf and
front branch have equal weight, the comparisons at `0x40422a` and `0x404249` take the
**leaf** first; equal-frequency leaves are already in symbol order. If the greatest depth
is within the maximum, these depths are the code lengths.

### Package-list length limiting

If the tree is too deep, `0x404490` transfers to the package-list routine at `0x4047de`,
whose reconstructed algorithm is binary package-merge:

```text
leaves = active symbols sorted by (frequency, symbol)
target = 2 * number_of_leaves - 2
previous = leaves

for each further level up to max_bits - 1:
    packages = pair adjacent items in previous
               and replace each pair by an item with their summed weight
    current = stable merge(leaves, packages)
    keep at most target items
    previous = current

select the first target items from the final list
expand packages recursively
the number of occurrences of each leaf is that symbol's code length
```

During each merge an original leaf is taken before a package of equal weight, so the leaf
order supplies the final tie rule. The result is a minimum-weight set of prefix-code
lengths within the maximum length. Package-merge runs only when the ordinary tree is too
deep; it is not an extra candidate.

### Canonical codes

Codes are canonical (counted by length, assigned in increasing symbol order) and written
bit-reversed for Deflate's least-significant-bit-first packing. Any legal canonical-table
implementation works, but the **lengths** must follow Defluff's tie rules: different
legal lengths can tie on payload yet differ in dynamic-header cost.

## Dynamic-header reconstruction

Routine `0x404d00` encodes a pair of data-tree length arrays and returns the complete
dynamic-header cost. It works on the transmitted literal/length span followed immediately
by the transmitted distance span, so a run of equal lengths may cross the HLIT/HDIST
boundary.

### Trimming HLIT and HDIST

The literal/length span is trimmed to its highest non-zero length but keeps the required
minimum through symbol 256; the distance span is trimmed to its highest non-zero length
but keeps the required minimum. The counts are written as `HLIT-257` and `HDIST-1`.
Trimming happens before RLE, so omitted trailing zeros never become repeat tokens.

### Initial greedy length RLE

The seed code-length stream is built greedily from the concatenated lengths:

- **Non-zero run:** emit the value once; while at least three copies remain, emit symbol
  16 for up to six; emit any one- or two-value tail explicitly.
- **Zero run:** while at least 11 zeros remain, emit symbol 18 for up to 138; while at
  least three remain, emit symbol 17 for up to 10; emit the last one or two explicitly.

### Build the 19-symbol tree

The frequencies of RLE symbols `0..18` go through the builder at `0x404050` with a
maximum length of 7. The code-length lengths are sent in the RFC 1951 permutation
`16, 17, 18, 0, 8, 7, 9, 6, 10, 5, 11, 4, 12, 3, 13, 2, 14, 1, 15`; trailing zeros in
that order are dropped, but at least four entries are written. This sets HCLEN; each
entry costs three bits.

### Four finite feedback passes

At `0x4050f2`, the binary sets a pass counter to exactly four. Each pass:

```text
count symbols in the current RLE stream
build a new max-7 code-length tree
repack every equal-length run using the new symbol prices
use that repacked stream as the next pass's input
```

The fourth state is returned and emitted. The routine does not keep the smallest of the
seed and the four headers, does not stop early at a fixed point, and runs no fifth pass.

### Local non-zero-run repacking

```text
literal_price(v) = code_length_of_symbol_v
repeat16_price    = code_length_of_symbol_16 + 2 extra bits
```

The first non-zero value is emitted explicitly; symbol 16 can then represent 3 through 6
copies. The repacker is local and greedy, not a shortest-path encoder:

- it normally compares one symbol-16 chunk with writing that chunk explicitly;
- equal local cost keeps symbol 16 (`0x405634` rejects it only when more expensive);
- for exactly three remaining values, `0x40561a` adds a one-bit bias, so symbol 16 must be
  strictly cheaper; and
- when exactly seven or eight values remain, it also tests two symbol-16 tokens (`4+3` for
  seven, `5+3` for eight), used when no more expensive than either all explicit values or
  one six-value repeat followed by explicit values.

A missing symbol 16 disables that form. A missing explicit value makes the pass invalid,
because symbol 16 cannot introduce the first value of a non-zero run.

### Local zero-run repacking

```text
zero_price     = code_length_of_symbol_0
repeat16_price = code_length_of_symbol_16 + 2
repeat17_price = code_length_of_symbol_17 + 3
repeat18_price = code_length_of_symbol_18 + 7
```

The rules are:

1. While more than ten zeros remain, use symbol 18 for up to 138 zeros. A short tail may
   remain.
2. For a remaining run of 3 through 10, compare symbol 17 with explicit zeros.
3. A short tail can sometimes use symbol 16. If a preceding symbol 18 established a
   previous zero, symbol 16 repeats that zero directly for a 3..6 tail. Otherwise emit one
   explicit zero and let symbol 16 cover the remaining 3..6 zeros.
4. The symbol-16 form must be strictly cheaper than symbol 17 and no more expensive than
   explicit zeros.
5. Symbol 17 wins its comparison with explicit zeros on a tie.
6. If symbol 17 has no code, the branch falls back to explicit zeros rather than selecting
   symbol 16 by itself.

Long-run symbol 18 use is not compared globally with combinations of 17, 16, and explicit
zeros, so a generic shortest-path RLE encoder will not reproduce Defluff.

### Dynamic-header cost

The header cost is the dynamic-block type and count fields (as split between caller and
routine), `3 * HCLEN` bits of code-length-code lengths, one code-length Huffman code per
RLE token, and 2, 3, or 7 extra bits for symbols 16, 17, or 18. End-of-block and the data
payload are scored separately by `0x403250`.

## Length-258 symbol handling

### Normalizing an alias found in the input

RFC 1951 assigns match length 258 to literal/length symbol 285. Some arithmetic table
decoders also read symbol 284 with its five extra bits set to 31 as length 258, although
symbol 284's range ends at 257. When Defluff decodes that alias, `0x4066c2..0x4066dc`
moves its frequency from symbol 284 to 285. Ordinary candidates then use symbol 285,
saving five extra bits per match before any tree-length effect.

### Creating the alias as an output candidate

The reverse transformation at `0x40727e` runs only when the normalized block contains both
an ordinary symbol-284 match (length 227..257) and a symbol-285 match (length 258).
Length-258 matches alone do not trigger it. It adds the symbol-285 frequency to symbol
284, clears symbol 285, rebuilds the dynamic tree, and encodes length 258 as symbol 284
with extra value 31. Fresh and adjusted feedback trees are both tried, and each must be
strictly smaller than the current dynamic candidate.

This form does not conform to RFC 1951's length ranges. A strict decoder may reject it,
even though many table-driven decoders produce the expected 258-byte match. Implementation
advice: make it opt-in and test it against the intended decoders.

## Complete decision procedure

For one source block, preserving candidate and tie order:

```text
parse block and normalize any input length-258 alias

if decoded_size <= 25:
    emit fixed block, applying the strict fixed-table match/literal rule
    finish

stored_cost = cost_stored(decoded_bytes, current_alignment)
if source_BFINAL and stored output needs more than one fragment:
    stored candidate is ineligible

fixed_state = substitute_matches_under(fixed_tables)
fixed_cost = 3 + payload_cost(fixed_state, fixed_tables)

dynamic_cost = infinity
dynamic_state = none

if source block is dynamic:
    source_header = four_pass_header(source_data_lengths)
    source_state = substitute_matches_under(source_data_tables)
    dynamic_cost = source_header.cost + source_state.payload_cost
    dynamic_state = source tables/state

fresh_tables = build_trees(original_frequencies)
fresh_header = four_pass_header(fresh_tables.lengths)
fresh_state = substitute_matches_under(fresh_tables)
fresh_cost = fresh_header.cost + fresh_state.payload_cost
if fresh_cost < dynamic_cost:
    dynamic_cost = fresh_cost
    dynamic_state = fresh tables/state

adjusted_tables = build_trees(fresh_state.frequencies)
adjusted_header = four_pass_header(adjusted_tables.lengths)
adjusted_state = substitute_matches_under(adjusted_tables)
adjusted_cost = adjusted_header.cost + adjusted_state.payload_cost
if adjusted_cost <= dynamic_cost:
    dynamic_cost = adjusted_cost
    dynamic_state = adjusted tables/state

if ordinary symbol 284 and symbol 285 are both present:
    alias frequencies = combine 285 into 284
    run the same fresh/adjusted construction for alias tables
    accept each alias state only if its complete cost is strictly smaller

winner = first minimum of [eligible stored, fixed, dynamic]
emit winner with the source BFINAL value
```

`substitute_matches_under()` scores a state and supplies the frequencies for the next
tree.

## Container handling and command-line contract

The 0.3.2 outer layer recognizes Deflate streams in:

- GZIP;
- ZIP archives compatible with the supported PKZIP 2.0-era structures; and
- PNG/APNG compressed data, including `IDAT`, `fdAT`, `zTXt`, `iCCP`, and compressed
  `iTXt` content.

The 0.3.2 release notes name `iCCP` and `iTXt` support as new.

The program is a filter: it reads standard input, writes standard output, has no optimizer
switches, and requires input and output to be different streams. The outer layer locates
compressed payloads, runs the raw optimizer, and repairs container lengths, offsets, or
checksums as needed; uncompressed-data checksums do not change.

The container layer was analyzed less thoroughly than the raw optimizer. Implementation
advice: use a fully validated container parser, treat each Deflate payload as an
independent job, and do not assume ZIP data descriptors, ZIP64, unusual extra fields, split
archives, or malformed-but-tolerated files work because a simple ZIP does.

## Complexity and compute characteristics

Compute is modest because there is no match search. Per block:

- parsing and each match/literal scoring pass are linear in decoded size;
- tree setup is dominated by an `O(A log A)` sort of at most 286 active leaves, and the
  two-queue merge is linear;
- package-merge is bounded by alphabet size times maximum length: at most 286 symbols and
  15 levels for a data tree, or 19 and 7 for the header tree;
- header feedback is four passes over at most `HLIT + HDIST <= 316` code lengths; and
- token feedback is two tree generations.

Memory is linear in the largest decoded source block plus 32 KiB of history; the initial
slot allocation is 128 KiB (`65,536 * 2` bytes). Because slots are per decoded byte, a
large block grows memory even if it holds only a few long matches. A port can keep tokens
plus the decoded block instead, but match expansion still needs every referenced literal
byte.

## Methods not present in the analyzed core

The following should not be attributed to Defluff 0.3.2; a port that adds them should
document them as extensions:

- a new LZ77 parser or match search, iterative optimal parsing, or a high-level engine
  such as libdeflate, zlib, or Zopfli;
- arbitrary block splitting or adjacent-block merging;
- a large option grid or a global shortest-path encoder for dynamic-header RLE;
- unconstrained iteration of match/tree feedback;
- keeping every intermediate four-pass header and selecting the smallest;
- preserving the exact source block bits as a fallback; or
- guaranteeing standards-compliant output when the length-258 alias candidate wins.

## Exact emulation with Unicorn

The Windows specimen was loaded into Unicorn's 32-bit x86 mode at its preferred PE image
base after the harness checked its SHA-256. The harness mapped the image, stack, heap,
input/output globals, and the optimizer entry at `0x4066f0`, so the original machine
instructions ran for parsing, tree construction, package-merge, header feedback,
candidate selection, and serialization.

The harness supplied only the imported operating-system and C-runtime functions:
`malloc`, `calloc`, `realloc`, `free`, `memcpy`, `ReadFile`, `WriteFile`, `ExitProcess`,
and `qsort`. The `qsort` shim reproduced the comparator's total ordering of eight-byte
leaf records by `(unsigned frequency, unsigned symbol)`. Allocation and I/O shims preserved
the requested sizes, state changes, and 4 KiB refill/flush behavior.

The recorded differential campaign included:

- 6,000 generated Huffman frequency sets, including over-depth cases that force the
  package-list path;
- 2,000 generated dynamic-header feedback cases;
- 130 generated complete raw Deflate streams;
- 800 small fixture streams;
- 100 crafted multi-block streams; and
- focused decoded-size cases at 24/25/26, 4095/4096/4097, 32767/32768/32769, and
  65535/65536/65537 bytes, plus a 70,000-byte Huffman block.

The focused cases covered the 25-byte shortcut, buffer crossings, 32 KiB history
rollover, stored fragmentation at 65,535 bytes, final-block stored eligibility, slot-buffer
reallocation, and cross-block matches. Every whole-stream comparison first required
decompression to the original bytes; primitive tests then compared exact code lengths, RLE
tokens, branch choices, bit costs, or serialized bytes. Exact byte comparison finds
tie-order mistakes that decompression alone cannot.

## Suggested validation for a reimplementation

Decode every result with at least two independent decoders, plus a strict decoder when
the length-258 alias is enabled, and check container lengths and checksums. Beyond the
boundaries named above (ties, missing codes, 25 bytes, 32 KiB, 65,535 bytes), test:

- Huffman lengths, not only payload cost, on dense, sparse, equal-frequency, one-symbol,
  and over-depth sets;
- non-zero remaining runs of 3, 6, 7, 8, 9, and 12; zero runs of 2, 3, 6, 7, 10, 11, 138,
  139, and 276; runs crossing the HLIT/HDIST boundary; and all four header passes, to find
  the first wrong repack;
- 4 KiB I/O crossings at every bit offset, and matches that start in retained history and
  overlap new output; and
- each container payload location alone and combined, including multiple `IDAT`/`fdAT`
  chunks, ZIP central-directory offsets, data descriptors, archive comments, and an
  explicit reject path for unsupported features.

## Address index

| Address or range | Reconstructed purpose |
|---|---|
| `0x403100` | Buffered input bit reader |
| `0x403250..0x403527` | Token payload cost, strict match-to-literal decision, and adjusted frequencies |
| `0x403530` | Buffered output bit writer |
| `0x4035b0` | Emit retained matches or their table-dependent literal expansions |
| `0x404050` | Ordinary Huffman length builder and over-depth dispatch |
| `0x404180..0x404490` | Sorted-leaf/two-queue Huffman construction |
| `0x4047de` | Package-list length limiter |
| `0x404d00` | Complete dynamic-header RLE feedback and cost |
| `0x4050f2` | Initialize the exact four-pass header counter |
| `0x405129..0x4057fb` | Local run repacking under code-length symbol prices |
| `0x40561a` | One-bit bias for an exactly-three-value symbol-16 choice |
| `0x405634` | Keep ordinary symbol 16 on an equal local cost |
| `0x405eb0` | Dynamic-header parsing path used by the raw optimizer |
| `0x406170` | Shared Huffman token decode into slots and frequencies |
| `0x40663a` | Grow decoded-slot storage |
| `0x4066c2..0x4066dc` | Normalize input length-258 symbol-284 alias to symbol 285 |
| `0x4066f0` | Raw Deflate optimizer entry |
| `0x4066fb..0x4067db` | Initialize fixed Huffman lengths/tables |
| `0x4067e4..0x406802` | Initial 65,536-slot allocation and 32 KiB history state |
| `0x406a77..0x406a8e` | Decoded-size calculation and `<= 25` shortcut |
| `0x406a94..0x406ace` | Stored cost and fragment count |
| `0x406ae4..0x406af6` | General fixed candidate cost |
| `0x406afc..0x406b26` | Input-dynamic-tree candidate |
| `0x406b30..0x406bbe` | Fresh dynamic trees and strict acceptance |
| `0x406bc3..0x406c43` | Adjusted dynamic trees and equal-cost acceptance |
| `0x406c52..0x406c7d` | Stored/fixed/dynamic first-minimum selection |
| `0x406c7d..0x406c96` | Emit source BFINAL and selected BTYPE |
| `0x406ca8..0x40723a` | Stored writer and 65,535-byte fragmentation |
| `0x406fa4..0x40710e` | Dynamic header and payload writer |
| `0x407118..0x40718f` | Append or replace retained block history |
| `0x4071db..0x407206` | Short fixed-block writer |
| `0x40723f..0x407279` | Shift and retain final 32 KiB of history |
| `0x40727e..0x4073c1` | Optional output length-258 alias dynamic candidates |
| `0x4073c6..0x4074a5` | Direct fixed-frequency cost fallback |
| `0x4074aa..0x4074b6` | Skip multi-fragment stored candidate for source BFINAL |

## Confidence and remaining limits

Confidence is high for the raw optimizer (candidate order, tie rules, Huffman builder,
package-list limiter, match and header feedback, stored fragmentation, history, and
length-258 paths), which combines instruction evidence with exact execution of the
original routines. It is moderate for container compatibility: the supported formats are
identifiable, but historical ZIP and malformed-container variants are far more numerous,
so a port should publish a narrower, tested compatibility contract.

## Attribution and public references

Defluff was written by Joachim Henke; the executable identifies copyright years
2010–2011. The public [Defluff announcement and release
thread](https://encode.su/threads/1214-defluff-a-deflate-huffman-optimizer) gives the
author's description (repeating the Huffman-coding step on already compressed streams,
complementary to DeflOpt rather than a replacement), supported-container notes, version
history, and warnings that the software was new and its output should be verified.

This document is an independent interoperability analysis. Having a binary and a
description of its behavior does not grant a license to copy or redistribute its code;
reimplementations should use original code and preserve attribution.
