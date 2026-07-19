# Defluff 0.3.2: reverse-engineered methods

This document describes the Deflate post-processing methods implemented by Defluff
0.3.2. It is intended as an implementation-oriented reference for projects that want to
reproduce, compare with, or learn from those methods.

The ground truth for every Defluff-specific statement is the original executable. The
32-bit Windows build, `defluff-0.3.2-windows-i686.exe`, was disassembled and its optimizer
was executed directly under an x86 emulator. Reconstructed pseudocode and independent
implementations were used to test the interpretation, but they do not override the
executable.

## Reference binaries and evidence policy

The principal specimen is the Windows executable because the virtual addresses in this
document refer to it. The same release's Darwin executable is retained as a second build
artifact.

| Property | Windows specimen | Darwin specimen |
|---|---|---|
| Product version | Defluff 0.3.2 | Defluff 0.3.2 |
| Author shown by the program | Joachim Henke | Joachim Henke |
| Format | PE32 console executable, Intel 80386 | Mach-O executable, Intel i386 |
| SHA-256 | `8847fba3ff5bc23fd4d5178f5383b910a33302266bd2354035a00f4f10e2d54f` | `c647ac35c9d0c173141fc9381a1d74c9660eafcff1ddc7617c13d354862d428c` |
| Local copy | [Windows executable](binaries/defluff/defluff032/defluff-0.3.2-windows-i686.exe) | [Darwin executable](binaries/defluff/defluff032/defluff-0.3.2-darwin-x86) |

**Address convention.** Unless a passage explicitly says otherwise, every hexadecimal code
address or range in this document—for example, `0x4066f0`—is an **absolute virtual
address in `defluff-0.3.2-windows-i686.exe`**, the PE32 Windows specimen with SHA-256
`8847fba3ff5bc23fd4d5178f5383b910a33302266bd2354035a00f4f10e2d54f`. The image's
preferred PE base is `0x00400000`, so an RVA can be obtained by subtracting
`0x00400000`; the displayed values are not RVAs or file offsets. They are also not
addresses from the Darwin build, which is retained only as a second release artifact.
These addresses will not apply to a different release, build, or repacked executable.

Statements use these evidence classes:

- **Observed** means the control flow, data flow, comparison, constant, or output operation
  is directly present in the disassembly.
- **Emulation-verified** means original instructions from the hash-checked PE image were
  executed under Unicorn and compared with a separately reconstructed result.
- **Reconstructed** means the description combines several observed instruction sequences
  into a higher-level algorithm. These interpretations were checked against executable
  behaviour wherever practical.
- **Implementation advice** explains how another project can reproduce the behaviour
  without copying the executable's memory layout. It is not a claim about unavailable
  source code.

The executable is stripped. Routine names in this document are descriptive names assigned
during reverse engineering. Addresses and branch conditions are the durable references.

## Scope

Defluff optimizes bytes in an existing Deflate stream. It parses each source block,
retains its decoded literals and matches, and chooses a cheaper serialization from a
small deterministic family. It does **not** run a high-level compressor and does not
search the decoded data for new LZ77 matches.

Its principal methods are:

1. choose stored, fixed, or dynamic output for an existing block;
2. rebuild deterministic length-limited Huffman trees;
3. replace an existing match with its literal bytes when that is cheaper under the tree
   being tested;
4. rebuild the trees once from the resulting frequencies;
5. feed code-length-code prices back into dynamic-header run-length encoding exactly four
   times; and
6. optionally use a compatibility-sensitive representation of match length 258.

Defluff preserves source block boundaries except when one non-final source block is
serialized as several stored blocks. It does not merge adjacent blocks, search arbitrary
split points, discover replacement matches, or iterate tree/token feedback to a fixed
point.

The public release description characterises Defluff as repeating the Huffman-coding step
on already compressed streams, and says its methods are complementary to DeflOpt rather
than a replacement for it. That description is useful background, but the executable is
authoritative for the details below.

## Terminology and conventions

This document uses the RFC 1951 symbol spaces:

- **literal/length alphabet:** symbols `0..285`, with `256` as end-of-block;
- **distance alphabet:** symbols `0..29`;
- **code-length alphabet:** symbols `0..18`, including repeat symbols `16`, `17`, and `18`;
- **HLIT:** the transmitted literal/length count, at least 257;
- **HDIST:** the transmitted distance count, at least 1; and
- **HCLEN:** the transmitted code-length-code count, at least 4.

"Decoded size" means the number of uncompressed bytes produced by a block. It is not the
number of LZ77 tokens. "Payload cost" includes Huffman code bits and length/distance extra
bits. "Complete block cost" also includes the block prologue, stored padding, or dynamic
tree description as applicable.

Comparison strictness is part of the algorithm. This document therefore distinguishes
`<` from `<=` even where both choices decompress identically.

## High-level algorithm

The raw Deflate optimizer begins at `0x4066f0`. A faithful high-level reconstruction is:

```text
initialise the fixed Huffman tables
allocate a decoded-slot buffer and retain up to 32 KiB of prior output

repeat:
    reset block frequencies and candidate state
    read BFINAL and BTYPE
    parse stored, fixed, or dynamic input
    decode the block into byte-addressable slots while recording its matches
    normalise any input use of symbol 284 for match length 258 to symbol 285

    if decoded_size <= 25:
        write a fixed block, expanding any match that is strictly dearer than its
        literal bytes under the fixed tables
    else:
        score stored output
        score fixed output, including strict match-to-literal substitutions

        if the input was dynamic:
            repack its data trees and score them as an initial dynamic candidate

        build fresh dynamic data trees from the original token frequencies
        score them and record match-to-literal substitutions
        build adjusted data trees from those substituted frequencies
        score those trees once

        if eligible, repeat the dynamic search with the length-258 alias
        choose stored, fixed, or dynamic according to complete bit cost
        serialize the chosen form

    retain at most the final 32 KiB of decoded history
until the source BFINAL bit was set

flush output and release the decoded-slot buffer
```

Defluff reconstructs blocks rather than preserving source bits. Even the shortcut writer
may replace a match with literals, and the exact source bit sequence is not an immutable
candidate. An equal-size output can therefore differ from the input.

## Parsing and retained block state

### Fixed-table initialization

`0x4066fb..0x4067db` constructs the standard Deflate fixed tables:

| Literal/length symbols | Code length |
|---|---:|
| `0..143` | 8 |
| `144..255` | 9 |
| `256..279` | 7 |
| `280..287` | 8 |

All 32 fixed distance codes have length 5. The decoder must still reject reserved data
symbols even though the fixed code space assigns them bit patterns.

### Stored, fixed, and dynamic input

The loop accepts all three defined Deflate block types. Stored input is aligned to the
next byte, validates `LEN` against `NLEN`, and copies the bytes into the decoded buffer.
Fixed input uses the initialized standard tables. Dynamic input parses `HLIT`, `HDIST`,
`HCLEN`, expands code-length repeat symbols, constructs canonical decode tables, and then
uses the shared data-token decoder.

Reserved `BTYPE=3`, an inconsistent stored length, an invalid tree, an impossible symbol,
or a distance that cannot refer to available history causes an error return. A compatible
implementation should fail the stream rather than repair malformed input silently.

### Decoded-slot representation

The executable stores one 16-bit slot for every decoded byte, not one record for every
token. Literal slots carry the literal byte. A match's first slots also carry encoded
length/distance metadata in their upper bytes, while the low byte of every slot retains
the byte produced at that decoded position.

That representation has three important consequences:

- the cutoff at `0x406a85` counts decoded bytes;
- expanding a match requires no new LZ decoding because its literal bytes are already in
  the slots; and
- writing a stored candidate is a low-byte copy of the same decoded slots.

An implementation can use a conventional token vector plus a decoded-byte buffer instead.
It only needs to preserve the same token identities, expanded bytes, frequencies, and
cross-block history.

### Buffer growth and streaming I/O

`0x4067e4..0x406802` allocates an initial `65,536`-slot buffer and initializes the retained
history length to `32,768`. The growth path at `0x40663a` reallocates the slot buffer when
a single block plus retained history no longer fits.

Input and output pass through separate 4 KiB buffers. The bit reader and writer keep their
partial-byte state across a refill or flush. Boundary validation crossed both 4 KiB
limits; treating those buffers as block boundaries is incorrect.

### Cross-block history

Back-references may refer into earlier blocks. After emitting a block:

- if its decoded size exceeds `32,767`, the next block retains its last `32,768` bytes;
- if the old history plus new block fits, the bytes are appended in place; and
- otherwise `0x40723f..0x407279` shifts the final `32,768` bytes into retained-history
  position.

This is history preservation, not block merging. Each source block still receives its own
optimization decision and output prologue.

## The 25-byte fixed shortcut

At `0x406a77..0x406a8e`, Defluff computes:

```text
decoded_size = (end_slot_pointer - start_slot_pointer) / 2
```

If `decoded_size <= 25`, control jumps to `0x4071db`. The writer emits the source BFINAL
bit, `BTYPE=1`, and the block using the fixed tables. This path does not score stored
output, build a dynamic tree, or call the separate payload-cost routine at `0x403250`.
The writer at `0x4035b0` nevertheless performs the equivalent per-match comparison while
emitting: it replaces a match when all required literals have codes and their total is
strictly smaller than the fixed match representation. A tie retains the match.

The shortcut includes an empty block: Defluff writes it as a fixed block containing only
end-of-block. It does not delete empty blocks.

Implementation pitfall: a block containing one long match can have very few tokens but
more than 25 decoded bytes. The shortcut is based on decoded bytes, so token count is the
wrong test.

## Complete block candidates

For a block larger than 25 decoded bytes, the local candidate array is ordered:

1. stored;
2. fixed; and
3. dynamic.

The scan at `0x406c61..0x406c7d` replaces the current winner only when the later cost is
strictly smaller. Therefore stored wins a complete-cost tie with fixed or dynamic, and
fixed wins a tie with dynamic.

The selected source BFINAL bit and selected two-bit BTYPE are written at
`0x406c7d..0x406c96`. The three candidates describe the same uncompressed bytes, but the
fixed and dynamic forms may use a mixture of retained matches and literal expansions.

### Stored candidate

The cost calculation at `0x406a94..0x406ace` includes:

- alignment after the three-bit block prologue;
- one `LEN/NLEN` pair for each group of at most `65,535` decoded bytes; and
- an additional stored-block prologue and alignment byte for every later group.

The writer at `0x406ca8..0x40723a` copies the low byte of each decoded slot and fragments
at exactly `65,535` bytes. Between fragments it writes a zero byte: the low three bits are
`BFINAL=0, BTYPE=00`, and the remaining five bits provide stored-block alignment.

There is one source-defined restriction. The original BFINAL bit is written on the first
output block. A final source block therefore cannot safely become several stored blocks,
because a later fragment would follow a block already marked final. At `0x4074aa`, a final
source block skips candidate zero when its stored cost exceeds `0x80022`, the maximum
aligned cost of one 65,535-byte stored block. In practical terms:

- a non-final source block may become several non-final stored blocks; but
- a final source block may use stored output only when it fits one stored block.

This is not a general limitation of Deflate; it is a limitation of this writer's BFINAL
placement and candidate gate.

### Fixed candidate

The fixed candidate uses the standard fixed tables and costs the existing token sequence
with `0x403250`. Matches may be expanded to literals by the strict rule described later.
The block's three-bit prologue is then included in the candidate cost.

When the block has no ordinary match state requiring the general evaluator,
`0x4073c6..0x4074a5` computes the same fixed cost directly from symbol frequencies and
the standard code-length ranges.

### Dynamic candidates

Dynamic output is not one tree. Defluff considers, in order:

1. the input dynamic data trees, when the source block was dynamic, with the header
   reconstructed by Defluff;
2. fresh data trees built from the original token frequencies;
3. adjusted data trees built after the fresh-tree match/literal decision; and
4. when eligible, fresh and adjusted versions of the length-258 alias tree.

Every dynamic score includes the complete dynamic header, token payload, extra bits, and
end-of-block. The selected data tables and the corresponding token decisions are emitted
at `0x406fa4..0x40710e`.

The source dynamic header's literal bit pattern is not retained. It is parsed to code
lengths and passed through Defluff's own four-pass header encoder.

## Match-to-literal feedback

### One cost comparison

Routine `0x403250` walks the decoded-slot sequence under a supplied literal/length table
and distance table. For each existing match it computes:

```text
match_cost = length_code_length
           + length_extra_bit_count
           + distance_code_length
           + distance_extra_bit_count

literal_cost = sum(code_length[decoded_byte] for every byte in the match)
```

Its decision order is significant:

1. If the required length symbol or distance symbol has no code in the candidate table,
   expand the match to literals immediately.
2. Otherwise sum the literal costs in decoded order.
3. If any required literal has no code, retain the match.
4. Stop summing as soon as the literal total reaches or exceeds the match cost; retain the
   match in that case.
5. Expand only when every literal is encodable and the complete literal total is strictly
   smaller than the match cost.

Equal cost therefore retains the match. A missing literal is not treated as zero cost.

The routine returns the selected payload cost and simultaneously accumulates new
literal/length and distance frequencies. Its writer uses the same table-dependent rule,
so the scored substitutions and emitted substitutions agree.

### Fixed two-stage dynamic feedback

The dynamic route uses a bounded feedback sequence:

```text
build fresh literal/length and distance trees from original frequencies
score all tokens under those trees
record frequencies after strictly cheaper match-to-literal expansions

build adjusted trees from the recorded frequencies
score all tokens once under the adjusted trees
stop
```

If a match symbol removed during the fresh pass has no code in an adjusted tree, step 1
of the comparison keeps that match expanded. Other decisions may change because literal
and match code lengths changed.

There is no open-ended convergence loop. Two table generations—fresh and adjusted—are the
complete feedback method.

### Candidate tie rules inside the dynamic route

The dynamic sub-search has different tie rules from the final block-type scan:

- the fresh tree replaces the preceding dynamic candidate only when strictly smaller
  (`0x406bb2..0x406bb4`);
- the ordinary adjusted tree replaces it when smaller **or equal**
  (`0x406c39..0x406c43`); and
- both length-258 alias candidates require a strict saving
  (`0x40730f..0x40731b` and `0x4073b1..0x4073b3`).

An equal-cost adjusted ordinary tree can therefore become the dynamic state later
serialized, but an equal-cost dynamic state still loses to stored or fixed in the final
candidate scan. These rules explain some same-size rewrites and must be preserved for
bit-for-bit parity.

## Defluff's Huffman length builder

The builder begins at `0x404050`. It is used for literal/length trees, distance trees, and
the 19-symbol code-length tree. Callers trim trailing zero-frequency entries before data-
tree construction. Data trees use a maximum length of 15; the code-length tree uses 7.

### Active leaves and trivial cases

Only positive-frequency symbols become leaves. Records are sorted ascending by the total
key:

```text
(frequency, symbol_number)
```

No active symbols produce an empty set of lengths. One active symbol receives a one-bit
code. At actual data-tree call sites, trimming makes that symbol the last symbol in the
supplied span; a reimplementation should assign the code to the actual active symbol
rather than depend on layout.

### Ordinary two-queue construction

With at least two leaves, the binary implements the linear merge phase of the classic
two-queue Huffman algorithm:

```text
leaves   = positive-frequency leaves sorted by (frequency, symbol)
branches = empty queue

while total available nodes > 1:
    left  = remove_smallest(leaves, branches)
    right = remove_smallest(leaves, branches)
    append branch(weight(left) + weight(right), left, right) to branches

assign each leaf its depth below the final branch
```

Completed branches are naturally nondecreasing in weight, so they need no heap. When the
front leaf and front branch have equal weight, the comparisons at `0x40422a` and
`0x404249` choose the **leaf** first. Equal-frequency leaves already have symbol-number
order from the initial sort.

If the greatest depth is within the supplied maximum, those depths are the final code
lengths.

### Package-list length limiting

If the ordinary tree is too deep, `0x404490` transfers to the package-list routine at
`0x4047de`. The reconstructed algorithm is binary package-merge:

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

During each merge, an original leaf is selected before a package of equal weight. The
original leaf order therefore supplies the final tie rule. This produces a minimum-weight
prefix-code length set subject to the specified maximum length.

Package-merge is only used when the ordinary tree exceeds the limit. It is not an
additional candidate when the unconstrained tree already fits.

### Canonical codes and Deflate bit order

After lengths are chosen, the executable constructs canonical Huffman codes: count codes
by length, derive each length's first code, and assign codes in increasing symbol order.
Deflate transmits Huffman codes most-significant-bit first within its logical code while
packing stream bytes least-significant-bit first, so the writer uses the corresponding
bit-reversed integer representation.

Any legal canonical-table implementation is suitable, but the chosen **lengths** must
retain Defluff's tie rules. Different legal lengths can have identical payload cost yet a
different dynamic-header cost.

## Dynamic-header reconstruction

Routine `0x404d00` encodes a selected pair of data-tree length arrays and returns its
complete dynamic-header cost. It operates on the transmitted literal/length span followed
immediately by the transmitted distance span. A run of equal lengths may cross the HLIT /
HDIST boundary.

### Trimming HLIT and HDIST

The literal/length span is trimmed to the highest non-zero length while retaining the
required minimum through symbol 256. The distance span is trimmed to its highest non-zero
length while retaining the required transmitted minimum. The resulting count fields are
written as `HLIT-257` and `HDIST-1`.

Trimming is performed before RLE, so omitted trailing zeros never become repeat tokens.

### Initial greedy length RLE

The first code-length stream is constructed greedily from the concatenated lengths.

For a non-zero run:

1. emit the value once;
2. while at least three copies remain, emit symbol 16 for up to six copies; and
3. emit any one- or two-value tail explicitly.

For a zero run:

1. while at least 11 zeros remain, emit symbol 18 for up to 138 zeros;
2. while at least three remain, emit symbol 17 for up to 10 zeros; and
3. emit the final one or two zeros explicitly.

This is only the seed. The next stages repeatedly rebuild the code-length tree and
re-price that RLE.

### Build the 19-symbol tree

The frequencies of RLE symbols `0..18` are passed through the same builder at `0x404050`,
with a maximum length of 7. The code-length lengths are transmitted in the RFC 1951
permutation:

```text
16, 17, 18, 0, 8, 7, 9, 6, 10, 5, 11, 4, 12, 3, 13, 2, 14, 1, 15
```

Trailing zeros in that order are omitted, but at least four entries are written. This
determines HCLEN. Each retained code-length length costs three bits.

### Four finite feedback passes

At `0x4050f2`, the binary sets a pass counter to exactly four. Each pass performs:

```text
count symbols in the current RLE stream
build a new max-7 code-length tree
repack every equal-length run using the new symbol prices
use that repacked stream as the next pass's input
```

The fourth state is the state returned and emitted. The routine does not keep the smallest
of the seed and four intermediate headers, does not stop early at a fixed point, and does
not run a fifth pass.

### Local non-zero-run repacking

Let:

```text
literal_price(v) = code_length_of_symbol_v
repeat16_price    = code_length_of_symbol_16 + 2 extra bits
```

The first non-zero value is emitted explicitly. For the remaining copies, symbol 16 can
represent 3 through 6 values. The repacker is local and greedy rather than a shortest-path
encoder:

- normally it compares one symbol-16 chunk with writing that chunk explicitly;
- equal local cost keeps symbol 16 (`0x405634` rejects it only when more expensive);
- for exactly three remaining values, `0x40561a` adds a one-bit bias, so symbol 16 needs a
  strict one-bit advantage; and
- when exactly seven or eight values remain, it also tests two symbol-16 tokens:
  `4+3` for seven, or `5+3` for eight. The pair is used when it is no more expensive than
  either all literals or one six-value repeat followed by literals.

Missing symbol 16 disables that representation. Missing the explicit value makes a pass
invalid because the first value of a non-zero run cannot be introduced by symbol 16.

### Local zero-run repacking

For zeros, the relevant prices are:

```text
zero_price     = code_length_of_symbol_0
repeat16_price = code_length_of_symbol_16 + 2
repeat17_price = code_length_of_symbol_17 + 3
repeat18_price = code_length_of_symbol_18 + 7
```

The executable applies these local rules:

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

Long-run symbol 18 use is not globally compared with combinations of 17, 16, and literal
zeros. This is another reason a generic shortest-path RLE encoder will not reproduce
Defluff exactly.

### Dynamic-header cost

The final header cost consists of:

- the dynamic-block type and count fields as accounted by the caller/routine contract;
- `3 * HCLEN` bits for the transmitted code-length-code lengths;
- one code-length Huffman code for each RLE token; and
- 2, 3, or 7 extra bits for symbols 16, 17, or 18 respectively.

The end-of-block code and data-token payload are scored separately by `0x403250`. A
candidate is complete only after header and payload costs are added.

## Length-258 symbol handling

### Normalizing an alias found in the input

RFC 1951 assigns match length 258 to literal/length symbol 285. Some arithmetic table
decoders also interpret symbol 284 with its five extra bits set to 31 as length 258, even
though symbol 284's assigned range ends at 257.

When Defluff decodes that alias from the input, `0x4066c2..0x4066dc` immediately moves its
frequency from symbol 284 to symbol 285. Ordinary candidates then serialize the standard
symbol 285 representation. This saves five extra bits before considering any tree-length
effect.

### Creating the alias as an output candidate

Defluff can also deliberately try the reverse transformation. The route at `0x40727e` is
entered only when the normalized block contains both:

- at least one ordinary symbol-284 match, of length 227..257; and
- at least one symbol-285 match, of length 258.

It adds the symbol-285 frequency to symbol 284, clears symbol 285, rebuilds the dynamic
tree, and encodes length 258 as symbol 284 plus extra value 31. Both the fresh and adjusted
feedback trees are tried. Each must be strictly smaller than the current dynamic candidate
to replace it.

This representation is compatibility-sensitive and not standards-conforming under the
assigned ranges in RFC 1951. It should be an explicit opt-in in a new implementation, with
tests against the intended decoder population. A strict decoder may reject it even though
many table-driven decoders produce the expected 258-byte match.

The existing symbol-284 requirement is also material. Defluff does not create this output
candidate merely because a block contains length-258 matches; it first requires ordinary
symbol-284 traffic whose combined frequency may benefit from the alias.

## Complete decision procedure

The following pseudocode preserves the significant candidate and tie ordering for one
source block:

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

Two details are easy to lose in a clean-room implementation:

- `substitute_matches_under()` both scores the current state and supplies frequencies for
  the next tree, but the feedback chain has only two generations; and
- the exact source compressed block is not included in the final minimum.

## Block boundaries and iteration

Defluff runs the decision procedure once per source block. The next block begins only after
the chosen serialization has been written and history has been retained.

It does not:

- merge an empty block into its neighbour;
- remove an empty block;
- merge adjacent fixed or dynamic blocks;
- split a Huffman block at searched positions;
- repeat optimization until the output reaches a fixed point; or
- feed output from a high-level compressor back into its raw optimizer.

A non-final block can be physically fragmented only when stored output wins and exceeds
65,535 bytes. Those fragments preserve the original non-final status and are an output
format necessity, not an optimization search over split points.

Running Defluff and another post-processor repeatedly may expose new opportunities because
one program's tied or strictly smaller serialization changes the block structure and
frequencies seen by the next. That external alternation is not an internal Defluff method.

## Container handling and command-line contract

The 0.3.2 executable's outer layer recognizes Deflate streams embedded in:

- GZIP;
- ZIP archives compatible with the supported PKZIP 2.0-era structures; and
- PNG/APNG compressed data, including `IDAT`, `fdAT`, `zTXt`, `iCCP`, and compressed
  `iTXt` content.

The public 0.3.2 release notes specifically identify `iCCP` and `iTXt` support as additions
in that version. Earlier release notes identify the completion of PNG/APNG support and a
fix for broken output, which is one reason validation of rewritten containers remains
essential.

The program is a filter: it reads standard input, writes standard output, has no optimizer
switches, and requires input and output to be different streams. The outer layer locates
eligible compressed payloads, invokes the raw optimizer, and repairs container lengths,
offsets, or checksums where the container representation requires them. Uncompressed-data
checksums do not change because the decoded bytes do not change.

Container support is shallower than the raw method documentation here. A new project
should use its own fully validated container parser and treat each located Deflate payload
as an independent optimization job. In particular, ZIP data descriptors, ZIP64, unusual
extra fields, split archives, and malformed-but-tolerated files should not be assumed to
work merely because a simple ZIP does.

## Complexity and compute characteristics

Defluff's compute use is modest compared with a fresh compressor because it performs no
match search.

For one block:

- parsing and each match/literal scoring pass are linear in decoded size;
- sorting at most 286 literal/length leaves dominates ordinary tree setup at
  `O(A log A)`, where `A` is the active alphabet size;
- the two-queue Huffman merge is linear after sorting;
- package-merge is bounded by alphabet size times maximum code length—at most 286 symbols
  and 15 levels for a data tree, or 19 symbols and 7 levels for the header tree;
- the dynamic-header feedback performs exactly four bounded passes over at most
  `HLIT + HDIST <= 316` code lengths; and
- token feedback performs two tree generations, not an unbounded search.

Memory is linear in the largest decoded source block plus 32 KiB of retained history. The
initial slot allocation is 128 KiB (`65,536 * 2` bytes) and grows when necessary. The
package-list tree and header arrays are small bounded workspaces.

The important resource caveat is the per-decoded-byte slot representation: a very large
single source block grows memory even if it contains only a few long matches. A modern
implementation can avoid that exact layout by retaining tokens plus the decoded block,
but match expansion still needs access to every referenced literal byte.

## Methods not present in the analysed core

The following should not be attributed to Defluff 0.3.2:

- a new LZ77 parser or new-match search;
- libdeflate, zlib, Zopfli, or another high-level Deflate engine;
- iterative optimal parsing;
- arbitrary block splitting or adjacent-block merging;
- a large option grid for dynamic-header RLE;
- a global shortest-path code-length RLE encoder;
- unconstrained iteration of match/tree feedback;
- keeping every intermediate four-pass header and selecting the smallest;
- preserving the exact source block bits as a fallback; or
- guaranteeing standards-compliant output when the optional length-258 alias wins.

Some of these can be valuable additions in another optimizer. They should be documented as
extensions rather than described as recovered Defluff behaviour.

## Exact emulation with Unicorn

### What was emulated

The Windows specimen was loaded into Unicorn's 32-bit x86 mode at its preferred PE image
base. Before execution, the harness required the exact SHA-256 listed above. It mapped the
executable image, stack, heap, input/output globals, and the optimizer entry at `0x4066f0`.

Execution therefore ran the original machine instructions for parsing, tree construction,
package-merge, header feedback, candidate selection, and serialization. It did not
translate those routines into Python or replace them with a behavioural model.

### Narrow runtime shims

Only imported operating-system and C-runtime boundaries were supplied by the harness:

- `malloc`, `calloc`, `realloc`, and `free`;
- `memcpy`;
- `ReadFile`, `WriteFile`, and `ExitProcess`; and
- `qsort`.

The `qsort` shim reproduced the comparator's observed total ordering of eight-byte leaf
records by `(unsigned frequency, unsigned symbol)`. Allocation and I/O shims preserved the
executable's requested sizes, state changes, and 4 KiB refill/flush behaviour.

This distinction matters. Unicorn validated the reconstructed algorithms against the
original optimizer code while avoiding dependence on a 32-bit Windows host runtime.

### Validation corpus

The recorded differential campaign included:

- 6,000 generated Huffman frequency sets, including over-depth cases that force the
  package-list path;
- 2,000 generated dynamic-header feedback cases;
- 130 generated complete raw Deflate streams;
- 800 small fixture streams;
- 100 crafted multi-block streams; and
- focused decoded-size cases at 24/25/26, 4095/4096/4097, 32767/32768/32769, and
  65535/65536/65537 bytes, plus a 70,000-byte Huffman block.

The focused cases exercised:

- the 25-byte fixed shortcut;
- input and output buffer crossings;
- retained-history rollover at 32 KiB;
- stored fragmentation at 65,535 bytes;
- final-block stored eligibility;
- slot-buffer reallocation; and
- cross-block matches.

Every whole-stream comparison first required successful decompression to the original
bytes. Primitive tests then compared exact code lengths, RLE tokens, branch choices, bit
costs, or serialized bytes as appropriate. Exact byte comparison is especially useful
for finding tie-order mistakes that a decompression-only test cannot expose.

## Suggested validation strategy for a reimplementation

### 1. Structural validity

For every result:

- decode with at least two independent Deflate decoders;
- compare all uncompressed bytes;
- verify every block header, dynamic tree, end-of-block symbol, and distance bound; and
- verify enclosing container lengths and checksums.

Use a stricter decoder as well as common permissive decoders when the length-258 alias is
enabled.

### 2. Huffman-builder parity

Generate dense, sparse, equal-frequency, one-symbol, and deliberately over-depth frequency
sets. Compare:

- trailing-span trimming;
- ordinary two-queue code lengths;
- leaf-before-branch equal-weight choices;
- package-list activation;
- leaf-before-package equal-weight choices; and
- final canonical codes.

Payload cost alone is insufficient: two length sets can tie on payload and differ in the
dynamic header.

### 3. Header parity

Generate literal/length and distance length arrays with runs at every boundary relevant to
symbols 16, 17, and 18. Include:

- non-zero remaining runs of 3, 6, 7, 8, 9, and 12;
- zero runs of 2, 3, 6, 7, 10, 11, 138, 139, and 276;
- runs crossing the HLIT/HDIST boundary;
- equal-cost explicit/repeat choices; and
- trees in which one repeat symbol is absent.

Compare all four intermediate passes even though only pass four is emitted. This localizes
a divergence to the first wrong repack rather than the final header.

### 4. Match-feedback parity

Craft matches for which literals are cheaper by one bit, equal, or dearer by one bit.
Also test a missing match code, missing distance code, and missing literal code. Verify the
decision and the adjusted frequency arrays after the fresh pass and adjusted pass.

### 5. Candidate-order parity

Construct blocks where stored/fixed/dynamic costs tie and where fresh/adjusted dynamic
states tie. Verify:

- stored before fixed before dynamic at the block level;
- strict fresh-tree replacement;
- non-strict ordinary adjusted-tree replacement;
- strict alias replacement; and
- exclusion of multi-fragment stored output for a final source block.

### 6. Streaming and history parity

Cross 4 KiB I/O boundaries at every bit offset, not only on byte boundaries. Test blocks
on both sides of 25 bytes, 32 KiB, and 65,535 bytes. Include matches that begin in retained
history and overlap into newly decoded output.

### 7. Container parity

Test each supported payload location separately, then combine several streams in one
object. For PNG/APNG, include multiple `IDAT`/`fdAT` chunks and each supported compressed
text/profile chunk. For ZIP, test local headers, central-directory offsets, data
descriptors, archive comments, and unsupported features with an explicit reject path.

## Implementation requirements and common pitfalls

A compatible implementation should preserve all of the following:

- count 16-bit decoded slots, or equivalent decoded bytes, for the `<= 25` shortcut;
- retain up to 32 KiB across source blocks;
- trim inactive data-tree suffixes before calling the Huffman builder;
- sort leaves by `(frequency, symbol)`;
- choose a leaf before an equal-weight branch and before an equal-weight package;
- invoke package-merge only after the ordinary tree exceeds its maximum;
- concatenate HLIT and HDIST lengths before finding RLE runs;
- run exactly four header feedback passes and emit the fourth;
- reproduce the local, asymmetric repeat-token tie rules;
- expand a match only for a strict payload saving, except when its own codes are absent;
- keep a match if any required literal code is absent;
- rebuild adjusted dynamic trees once and no more;
- retain the ordinary adjusted dynamic state on an equal dynamic cost;
- preserve stored/fixed/dynamic final tie order;
- prevent a final source block from becoming multiple stored fragments; and
- separate input-alias normalization from opt-in output-alias creation.

Common incorrect simplifications include using token count for the short cutoff, using a
generic priority queue whose equal-weight order differs, applying package-merge to every
tree, optimizing the header RLE globally, selecting the smallest of all header passes, or
running token/tree feedback to convergence. Each produces valid Deflate but not Defluff's
method.

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

Confidence is high for the raw optimizer's candidate order, tie rules, Huffman builder,
package-list limiter, match feedback, dynamic-header feedback, stored fragmentation,
history handling, and length-258 paths. Those conclusions combine direct instruction
evidence with exact execution of the original PE routines under Unicorn.

Confidence is moderate for exhaustive container compatibility. The supported format and
chunk families are identifiable from the executable and release notes, but the space of
historical ZIP and malformed-container variants is much larger than the raw Deflate core.
A new implementation should publish a narrower, tested compatibility contract rather than
assuming every file accepted by contemporary tools is covered.

This analysis does not claim original source equivalence. Compiler optimizations erase
names and some structural intent, and the Darwin build has not been used as the address
reference. Where a statement is implementation advice or a higher-level reconstruction,
it is labelled accordingly; the hash-identified executable remains the final arbiter.

## Attribution and public references

Defluff was written by Joachim Henke. The executable identifies copyright years
2010–2011. The public [Defluff announcement and release
thread](https://encode.su/threads/1214-defluff-a-deflate-huffman-optimizer) provides the
author's description, supported-container notes, version history, and warnings that the
software was new and should be used with output verification.

This document is an independent interoperability analysis. Possession of a binary and a
description of its behaviour do not by themselves grant a licence to redistribute or copy
its code. Reimplementations should use original code, preserve attribution, validate their
own output, and obtain separate legal advice where required.
