# DeflOpt 2.07: reverse-engineered methods

This document describes the compression and container-rewriting methods implemented by
DeflOpt 2.07. It is intended as an implementation-oriented reference for projects that
want to reproduce, compare with, or learn from those methods.

The ground truth for every DeflOpt-specific statement in this document is the disassembled
32-bit Windows executable, not a later reimplementation. Source code and behavioural tests
were used to check interpretations, but they do not override the executable.

## Reference binary and evidence policy

The analysed specimen is [DeflOpt.exe](binaries/deflopt/DeflOpt207/DeflOpt.exe):

| Property | Value |
|---|---|
| Product version | DeflOpt 2.07 |
| Author | Ben Jos Walbeehm |
| Release date reported by the program | 5 September 2007 |
| Format | PE32 console executable, Intel 80386 |
| SHA-256 | `af675e098c8884af0310cbecee2c5ec6ff7214a5dc5560833c51d9dc82c81bdc` |

All addresses below are virtual addresses in that exact executable. They are not expected
to apply to another DeflOpt release or to a repacked binary.

Statements use these evidence classes:

- **Observed** means that the described control flow, data flow, comparison, or constant is
  directly present in the disassembly.
- **Reconstructed** means that the algorithm is a higher-level interpretation of several
  observed instruction sequences. These interpretations have been checked against the
  executable's outputs and, where practical, with independent models.
- **Implementation advice** describes how another project can implement compatible
  behaviour. It is not a claim about DeflOpt's internal source code.

The executable contains no symbols. Routine names in this document are descriptive names
assigned during reverse engineering. Addresses, branch directions, limits, and strictness
of comparisons are the durable references.

## Scope

DeflOpt is a post-processor for existing Deflate streams. It parses each source block,
replays its decoded token sequence, and tries a bounded set of alternative encodings. It does
not perform a new LZ77 match search. Its main opportunities are:

1. re-encode an existing dynamic block header more compactly;
2. build several alternative length-limited Huffman trees from the existing token
   frequencies;
3. replace selected matches with their literal bytes when that is strictly cheaper under
   the table currently being tried;
4. choose a cheaper legal block representation;
5. remove empty blocks and exact fixed-to-fixed boundaries; and
6. remove or normalise optional container structures according to command-line policy.

The core search is deliberately small. It does not split blocks, search for new matches,
perform a general adjacent-block merge, or explore arbitrary completed-Huffman-tree
neighbourhoods.

## Terminology and conventions

This document uses the RFC 1951 symbol spaces:

- **literal/length alphabet:** symbols `0..285`, with `256` as end-of-block;
- **distance alphabet:** symbols `0..29` for ordinary Deflate streams;
- **code-length alphabet:** symbols `0..18`, including repeat symbols `16`, `17`, and `18`;
- **HLIT:** the transmitted literal/length count, at least 257;
- **HDIST:** the transmitted distance count, at least 1;
- **HCLEN:** the transmitted code-length-code count, at least 4.

Bit costs in this document include every bit that influences DeflOpt's choice: block
headers, padding, tree descriptions, Huffman code bits, and length/distance extra bits.
They exclude container bytes unless a container section says otherwise.

"Strictly smaller" is important throughout. The binary normally updates a winner with
`candidate < incumbent`, not `candidate <= incumbent`. Equal-cost choices therefore keep
the earlier representation, and an equal-cost rewrite does not displace original input.

## High-level algorithm

The raw Deflate entry and per-block replay are centred on `0x404d80` and `0x404db0`.
At a high level, the binary behaves as follows:

```text
initialise input bits, output bits, and the 32 KiB history window

while the source stream has another block:
    save the complete block-start replay state
    parse one source block and preserve its token/data meaning
    emit or account the current representation

    if the decoded block is empty:
        discard its output while retaining consumed input state
        continue

    if the source and retry state permit alternatives:
        score the bounded alternatives
        if one is strictly smaller:
            restore the block-start state
            replay the same source block in the winning mode
            possibly repeat for the one later stored-block comparison

    finalise the chosen block and its BFINAL state

normalise the containing GZIP, PNG, or ZIP structure
keep the complete rewritten object only under the selected file-level policy
```

This is a replay optimizer rather than a decompressor followed by a conventional
recompressor. The distinction explains both its speed and its limits: the expensive match
discovery work has already been done by the program that produced the input.

## Deflate parsing and replay state

### Complete block-start snapshots

At `0x404e44..0x404e84`, the binary copies a `0x200c`-dword (`0x8030`-byte)
state area between live and saved storage. A zero retry mode saves the current state; a
non-zero retry mode restores it. The saved state includes input and output bit positions,
block bookkeeping, current representation, and the 32 KiB history window.

This gives every retry the same:

- source bit position;
- output bit position and therefore the same alignment;
- already-decoded history;
- block-local counters; and
- output prefix.

An implementation does not need the same memory layout, but it does need an equivalent
transactional checkpoint. Replaying from an incomplete checkpoint can corrupt back-
references or mis-score stored-block padding.

### Stored, fixed, and dynamic input

The parser reads `BFINAL` and `BTYPE`, then follows the standard Deflate representation:

- stored blocks are byte-aligned, verify `LEN` against `NLEN`, and copy bytes into both
  output and history;
- fixed blocks use the standard fixed literal/length and distance tables loaded at
  `0x405a79`; and
- dynamic blocks decode `HLIT`, `HDIST`, `HCLEN`, the 19-symbol code-length tree, and the
  combined literal/length and distance length arrays at `0x40510e..0x4059cc`.

The dynamic-header path expands symbols `16`, `17`, and `18` and requires the final number
of expanded lengths to equal `HLIT + HDIST`. It then constructs separate literal/length
and distance decoding tables.

The shared token loop is visible at `0x405e24..0x40665e`. It decodes literals,
end-of-block, lengths, distances, and extra bits while maintaining the circular history
window. During a candidate replay, a required symbol that has no code in the proposed
table makes that replay invalid. The binary does not silently add the missing symbol.

### What is retained from the source

The optimizer preserves the source stream's semantic token sequence:

- literal byte;
- end-of-block;
- match length and its length symbol/extra value; and
- match distance and its distance symbol/extra value.

It may turn a match into the corresponding literals, but it does not discover a different
match. This means a compatible implementation can parse into an explicit token vector and
replay it, even though the binary performs much of that work in a streaming state machine.

### Fixed tables

The fixed tables selected at `0x405a79` are the standard Deflate tables:

| Literal/length symbols | Code length |
|---|---:|
| `0..143` | 8 |
| `144..255` | 9 |
| `256..279` | 7 |
| `280..287` | 8 |

All fixed distance codes have length 5. Symbols 286 and 287 are not legal length symbols,
even though the fixed alphabet assigns bit patterns to them.

## Source-type gates and bounded retries

The retry controller is at `0x405dc0..0x406da0`. The current-form and next-form bytes
restrict which comparisons are made. The following behaviour is directly supported by the
branches at `0x405dc9`, `0x406b40`, and the surrounding state transitions:

| Source/current form | Alternatives considered |
|---|---|
| Stored | Preserve stored data; it is not recompressed as fixed or dynamic |
| Fixed | Replay fixed; a stored block may replace it if strictly smaller |
| Dynamic | Repack/replay the dynamic form; compare fixed and a rebuilt dynamic form; then compare stored |
| Rebuilt dynamic retry | May receive the later stored comparison |

The longest useful chain is therefore:

```text
source dynamic -> selected rebuilt dynamic -> selected stored
```

That is at most three linear passes over one source block. Each transition must improve the
score strictly, so the state machine cannot cycle.

### Original dynamic block fallback

At `0x406bcb..0x406d9b`, DeflOpt can copy the exact original dynamic block bits after
rewinding to the saved block start. It does so when that original bit sequence is strictly
smaller than the replayed form and no other retry has won. This is stronger than merely
reconstructing the same tree: it preserves any unusually efficient original header RLE.

Implementation advice: retain the original block's exact bit span as an immutable
candidate. A reconstructed equivalent block can be larger because Deflate permits many
encodings of the same code-length sequence.

### Empty-block deletion

The decoded byte count is calculated at `0x405dab..0x405db8`. When it is zero,
`0x406a94..0x406b11` restores the output/history side of the checkpoint while keeping
the already-consumed source input position. The empty block consequently disappears from
the output.

This is not an arbitrary merge algorithm. It is deletion of a block that contributes no
decoded bytes. An independent writer must still produce a valid final stream; in
particular, an all-empty input requires some legal final-block representation.

### Consecutive fixed-block coalescing

At `0x405130..0x405182`, when the previous selected output and the next selected output
are both fixed, the binary:

1. backs the output bit position up by 7 bits, removing the previous fixed end-of-block
   code; and
2. continues with the next block's symbols without writing its 3-bit block header.

This saves exactly 10 bits at each removed fixed/fixed boundary. It can apply even when a
source dynamic block selected fixed output. No analogous general dynamic-block merge is
present in the mapped core.

## Canonical Huffman construction

Routine `0x404b30` converts a length array into canonical Deflate records. It:

1. counts the number of symbols at each bit length;
2. computes the canonical first code for each length;
3. assigns codes in symbol order;
4. reverses the code bits for Deflate's least-significant-bit-first wire format; and
5. records the length, reversed code, and symbol.

It has two observed modes.

### Scoring/canonicalisation mode

When the maximum-depth output pointer is null, the routine does not build a decode table.
It finds the last symbol with a non-zero length, returns the active span through an optional
pointer, and floors an all-zero span to one. Callers subsequently enforce Deflate's
minimums of 257 literal/length symbols and one distance symbol.

### Decode-table mode

When the maximum-depth pointer is present, the routine records the maximum length,
allocates a dense table with `1 << max_length` entries, and fills all prefixes covered by
each reversed canonical code. The parser can then decode by table lookup.

No separate oversubscribed-tree check is visible inside this helper. A new implementation
should validate malformed trees explicitly at its input boundary while preserving the same
canonical assignment for valid streams.

## DeflOpt's Huffman length builder

The central length builder begins at `0x407710`. It produces length-limited trees for the
literal/length, distance, and code-length alphabets.

### Leaves and trivial cases

Only symbols with non-zero frequency become heap leaves.

- No active symbol produces no ordinary tree.
- One active symbol receives length 1.
- Two or more active symbols enter the heap merge process.

Callers ensure required Deflate semantics such as an end-of-block symbol. Implementations
should not infer that a zero-symbol literal tree is valid merely because the generic helper
has a trivial branch.

### Node keys

Each node carries at least:

- total frequency;
- subtree height;
- leaf or child information; and
- the destination associated with an original symbol.

There are two heap comparison policies:

```text
frequency-only:
    lower frequency first
    equal frequency retains the heap operation's existing order

height-aware:
    lower frequency first
    if frequencies tie, lower subtree height first
    if both tie, retain existing order
```

The height-aware repair helper is `0x407b30`. It reads the node value at offset `+4`,
which is subtree height. It does **not** use a symbol-order or tree-pointer tie key. The
initial heapify at `0x4077d3..0x40780f` compares frequency only.

This distinction corrects a tempting but inaccurate reading of the absolute addresses in
the disassembly. Implementations seeking DeflOpt parity should not introduce an order-key
tie breaker here.

### The four variants

A variant counter runs from 0 through 3. Its two bits select the comparison policy at the
two heap repairs performed during each merge:

| Variant | Repair after removing first child | Repair after inserting parent |
|---:|---|---|
| 0 | height-aware | height-aware |
| 1 | height-aware | frequency-only |
| 2 | frequency-only | height-aware |
| 3 | frequency-only | frequency-only |

Variant bit 1 controls the first repair; variant bit 0 controls the second. The calls at
`0x4078f4` and `0x4079d1` select the shared height-aware helper. The alternative inline
loops use frequency only.

### Merge procedure

For each merge, the binary reconstructs this sequence:

```text
a = heap root
remove a and repair the heap using the variant's first policy

b = heap root
replace b's heap slot with a parent whose:
    frequency = frequency(a) + frequency(b)
    height    = max(height(a), height(b)) + 1
repair the heap using the variant's second policy

record the two child tree references in the parent
record the order in which leaves leave the heap
```

The executable mutates the second consumed node slot into the parent at
`0x407930..0x407956`; a clean implementation may allocate a separate parent. The two
representations have the same comparison keys and leaf-consumption order. Independent
equivalence testing across the relevant alphabet sizes confirmed identical final lengths
for all four variants.

The completed tree is walked by `0x407bd0`, which writes the depth of each leaf.

### Limiting maximum code length

Literal/length and distance codes are limited to 15 bits; code-length codes are limited to
7 bits. If the ordinary tree is deeper, the repair near `0x407a3b..0x407ad4` changes the
histogram of code counts.

For a current deepest occupied length `max_depth > max_bits`, its essential transformation
is:

```text
bits = greatest occupied length below max_bits

count[bits]     -= 1
count[bits + 1] += 2
count[max_depth]     -= 2
count[max_depth - 1] += 1

while count[max_depth] == 0:
    max_depth -= 1
```

The process repeats until no occupied length exceeds the limit. The repaired lengths are
then assigned shortest-to-longest using the saved leaf-consumption order. Preserving that
order matters: assigning the same histogram to a different leaf order produces a different
payload cost.

## Dynamic-tree candidate generation

Routine `0x407c90` constructs the main candidates. For each of the four heap variants it:

1. builds a 286-entry literal/length length array;
2. builds a 30-entry distance length array;
3. hashes and de-duplicates each full array independently;
4. computes the active HLIT or HDIST span; and
5. caches its canonical representation and payload cost.

De-duplication occurs before active-span trimming. Two candidates are duplicates only when
the complete 286-byte or 30-byte length array matches. Each side can retain at most four
unique candidates.

The selector at `0x407f3b..0x40872b` then considers the cross-product:

```text
up to 4 code-length-tree variants
    x up to 4 distance-tree candidates
    x up to 4 literal/length-tree candidates
    = at most 64 generated tree/header trials
```

The loop order observed at `0x4086e3..0x408725` is code-length variant outermost,
distance candidate next, and literal/length candidate innermost. Because winner updates are
strict, this enumeration order determines which equal-cost candidate survives.

After the generated cross-product, the selector makes a final pass over the saved
best/original candidate. The `jbe` at `0x408725` accounts for this pass; it is not evidence
of an unbounded loop.

### Payload cost

For a token sequence, the payload portion is:

```text
sum(literal_or_length_frequency[s] * literal_or_length_code_length[s])
+ sum(distance_frequency[d] * distance_code_length[d])
+ sum(length_extra_bit_count for every match)
+ sum(distance_extra_bit_count for every match)
```

The end-of-block symbol has frequency one and is included in the first sum. A table is not
usable if any required symbol has length zero.

### Complete dynamic-block cost

For `HLIT`, `HDIST`, `HCLEN`, an RLE stream describing the combined length arrays, and a
code-length-code table, DeflOpt's comparison corresponds to:

```text
3                         block header: BFINAL + BTYPE
+ 5 + 5 + 4               HLIT, HDIST, HCLEN fields
+ 3 * HCLEN               transmitted code-length-code lengths
+ sum(clen_length[token] + repeat_extra_bits[token])
+ payload_bits
```

Repeat extra-bit widths are 2 for symbol 16, 3 for 17, and 7 for 18. Here `HCLEN` is one
plus the index of the last non-zero code-length code in Deflate's permutation, floored to
four entries.

## Encoding a dynamic header

Dynamic headers have their own small optimization problem: the literal/length and distance
length arrays are concatenated and encoded by a 19-symbol code-length alphabet. DeflOpt
uses a greedy initial RLE plus a bounded feedback loop; it does not solve the general
shortest-path covering problem over all legal RLE tokenisations.

### Initial greedy RLE

The initial writer begins at `0x408035`.

For a run of zero lengths:

```text
while run >= 11:
    emit 18 for min(run, 138) zeros
while run >= 3:
    emit 17 for min(run, 10) zeros
emit remaining zeros explicitly as symbol 0
```

For a run of one non-zero length:

```text
emit the value explicitly once
while at least 3 repeats remain:
    emit 16 for min(remaining, 6) repeats
emit the remainder explicitly
```

Values `0..15` are literal code-length symbols. Symbol 16 repeats the previous decoded
length 3 through 6 times, 17 writes 3 through 10 zeros, and 18 writes 11 through 138 zeros.

### Build the code-length tree

Frequencies of the resulting RLE tokens are fed back into the same four-variant Huffman
length builder, this time with 19 symbols and a maximum depth of 7. The active code-length
span is transmitted in the fixed Deflate permutation:

```text
16, 17, 18, 0, 8, 7, 9, 6, 10, 5, 11, 4, 12, 3, 13, 2, 14, 1, 15
```

### Local replacement of repeat tokens

Routine `0x4071b0` and the retry path at `0x4081c5..0x4085cb` rescan an existing RLE
stream under its current code-length tree. For a repeat token representing `n` decoded
length values, compare:

```text
repeat_cost   = clen_length[repeat_symbol] + repeat_extra_bits
explicit_cost = n * clen_length[decoded_value]
```

The repeat is replaced by explicit values only when `explicit_cost < repeat_cost`. A tie
keeps the repeat. The explicit replacement is usable only when the decoded value has a
non-zero code in the current code-length tree.

The ordinary linear disassembly drifts into inline data immediately before `0x4071b0`.
The address is nevertheless a direct call target, and targeted disassembly beginning at
that address recovers the routine prologue and instruction stream. This is why the method
is anchored at `0x4071b0` even if a linear listing labels its first bytes incorrectly.

There is one narrow cross-token rewrite. An existing symbol 17 can become symbol 16 when:

- the previously decoded length is zero;
- the run length is legal for both representations (3 through 6);
- symbols 16 and 17 both have usable codes; and
- the code for 16 is no longer than the code for 17.

The extra-bit widths are equal for the overlapping range, so comparing code lengths is
sufficient. The branch is visible near `0x4072b3` and `0x408388`.

These are local rewrites of a previously formed RLE stream. They do not introduce every
possible legal split or covering.

### Reassign code-length-code lengths by frequency

The pass at `0x407432` scans pairs of non-zero code-length symbols. It swaps their assigned
lengths when the current assignment gives a shorter code to a less frequent symbol, or a
longer code to a more frequent symbol. Equal lengths and equal frequencies do not cause a
swap.

This preserves the code-length histogram and therefore Kraft validity while moving shorter
codes toward more frequent RLE symbols. Canonical codes are reconstructed after the
assignment changes.

### Drop stale repeat symbols and rebuild

Local RLE rewriting can remove the last use of repeat symbol 16, 17, or 18. Checks at
`0x407498..0x4074cf` clear a now-unused repeat symbol's code length and rebuild the
code-length tree. The stream is then rescanned because the changed code costs may enable
another strict local improvement.

### Bounded feedback and termination

The state machine at `0x4081c5..0x4087db` alternates between:

1. build or update the code-length tree;
2. scan the RLE stream for the specific local rewrites above;
3. save a stream only when its encoded size is strictly smaller; and
4. restore and score the saved best stream for the final pass.

This is a finite feedback loop over a small token stream. It cannot oscillate between
equal-cost forms because equal-cost replacements are not accepted. Complete dynamic
candidate updates at `0x4086c1` are also strict.

## Repacking an original dynamic tree

The path near `0x405959..0x405979` preserves the original literal/length and distance
length arrays while regenerating their dynamic header. This separates two possible gains:

- use exactly the source Huffman table with a better description; or
- use a newly built Huffman table with a newly optimized description.

That first option is important. A source compressor may already have selected a good
payload tree but encoded its lengths inefficiently. Conversely, DeflOpt retains the exact
source bit span as a fallback because its own greedy-plus-local RLE need not beat every
possible source header.

## Strict match-to-literal expansion

During replay, the branch at `0x40636f..0x406382` may replace an existing match with the
literal bytes that the match would copy.

For a match, compute under the current candidate tables:

```text
match_bits = literal_length_code_length[length_symbol]
           + length_extra_bit_count
           + distance_code_length[distance_symbol]
           + distance_extra_bit_count

literal_bits = sum(literal_length_code_length[copied_byte])
```

Expand only when all required codes exist and:

```text
literal_bits < match_bits
```

The `jae` at `0x406382` rejects equal or more expensive literal forms. On expansion the
binary decrements the old length- and distance-symbol frequencies, increments the literal
frequencies, and replays the copied bytes as literals. History semantics remain unchanged.

After replay has accumulated the adjusted frequencies, `0x406692..0x406743` invokes the
bounded dynamic-tree selector once. The rebuilt result replaces the current replay only if
the complete block becomes strictly smaller. Thus a locally favourable expansion is not
automatically a block-level winner; header and table costs still count.

This method has two deliberate limits:

- it only removes an existing match; it does not search for another match; and
- it rejects a candidate table that lacks a required literal instead of adding the literal
  and conducting a broader search.

## Fixed and stored block scoring

### Fixed block

The complete fixed candidate consists of its 3-bit block header plus the token payload
under the fixed tables. Every match retains its length and distance extra bits. The
end-of-block symbol costs 7 fixed-tree bits.

The comparison at `0x406b40` is gated to the dynamic-source path. It is not an unrestricted
fixed-tree search for every input form.

### Stored block

At `0x406b7b..0x406bc6`, the stored cost for `N` decoded bytes at a given output bit
alignment is:

```text
3                               BFINAL + BTYPE
+ padding_to_next_byte_boundary
+ 16 + 16                       LEN + NLEN
+ 8 * N                         payload
```

Padding is calculated after the 3-bit block header. The candidate is rejected when
`N > 65535`; this path does not split one decoded source block into multiple stored blocks.
It wins only when its complete cost is strictly smaller than the selected alternative.

## Complete per-block decision procedure

The following pseudocode is suitable as a clean-room implementation guide. It describes
the reconstructed method, not the executable's register-level organisation.

```text
function optimize_one_source_block(source, output_alignment, history):
    original_bits = exact source block bit span
    parsed = parse source block into tokens/plain bytes

    if parsed.plain_size == 0:
        return EMPTY_DELETION

    if source.type == STORED:
        return original stored representation

    current = replay the source-tree representation

    if source.type == DYNAMIC:
        original_tree_repacked = optimize_dynamic_header(
            source.literal_lengths,
            source.distance_lengths,
            parsed.tokens)
        current = strict_best(current, original_tree_repacked)

        fixed = replay parsed.tokens with fixed tables
        current = strict_best(current, fixed)

        selected_dynamic = strict_best_in_DeflOpt_order(
            build_dynamic_candidate_cross_product(parsed.frequencies))
        current = strict_best(current, selected_dynamic)

        if selected_dynamic won and is replayed:
            adjusted_tokens = replay selected_dynamic while strictly
                              expanding eligible matches to literals
            adjusted_dynamic = build_dynamic_candidate_cross_product_once(
                frequencies(adjusted_tokens))
            replayed_selected = encode(adjusted_tokens, selected_dynamic.tree)
            current = strict_best(replayed_selected, adjusted_dynamic)

    stored = score one stored block if parsed.plain_size <= 65535
    current = strict_best(current, stored)

    if source.type == DYNAMIC and no rewrite retry was selected:
        current = strict_best(current, original_bits)

    return current
```

Three qualifications preserve observed behaviour:

1. The binary implements this through retry modes and state restoration, so comparisons
   occur in a specific order. When exact parity matters, retain the candidate enumeration
   and strict-tie order described above.
2. Fixed-source blocks do not enter the generated dynamic search, and stored-source blocks
   do not enter either Huffman search.
3. The exact-original branch at `0x406bcb` is a state-specific fallback after the stored
   comparison, not a licence to reorder every representation as an unordered candidate
   set. A bit-for-bit clone should reproduce the retry state machine.

At stream assembly time, delete empty blocks, join consecutive selected fixed blocks using
the exact 10-bit rule, and set the final surviving block's `BFINAL` bit correctly. The
finalization tail is at `0x407130..0x40719b`.

## Container methods

DeflOpt applies the same raw Deflate optimizer inside GZIP, PNG, and ZIP. Container code
also removes or rewrites metadata, which may account for savings independent of the
Deflate methods.

### Common whole-object selection

The wrapper at `0x402a70..0x402ccf` compares the rebuilt object with the original.
Default behaviour keeps a rewrite only when the file becomes smaller in whole bytes.

- `/b` also accepts a positive bit saving that does not reduce byte length.
- `/f` forces a rewrite, while the program documentation states that the result will not
  be larger than the input.

This outer comparison is distinct from the strict per-block comparisons.

### PNG

The PNG and zlib paths are `0x402050..0x402983`, with the top-level PNG entry at
`0x403000..0x40315c`.

Observed methods include:

- validate and retain/rewrite the two-byte zlib header so its FCHECK modulus is correct;
- optimize the raw Deflate payload and retain the Adler-32 trailer;
- coalesce consecutive `IDAT` chunks into one zlib stream;
- write the new chunk length and CRC; and
- with `/k`, locate and optimize the zlib payloads in `iCCP`, `zTXt`, and compressed
  `iTXt` chunks.

By default, the executable keeps `IHDR`, `PLTE`, `tRNS`, `IDAT`, and `IEND` and discards
other chunks. `/k` keeps the other structures. No `fdAT`/APNG frame-data optimization is
visible in the mapped PNG path.

### GZIP

The GZIP entry is `0x402ce0..0x402ff7`. It validates the signature and method, walks
`FEXTRA`, `FNAME`, `FCOMMENT`, and `FHCRC`, and sends the raw Deflate body through the
common optimizer.

Without `/k`, it preserves `FTEXT` and `FNAME`, discards `FEXTRA` and `FHCRC`, and discards
`FCOMMENT` unless `/c` is set. The `/d` option concerns the rewritten filesystem timestamp,
not the GZIP MTIME field.

At `0x402f28`, compressed payload length is derived from the end of the file minus the
payload start and the eight-byte trailer. The routine then returns without checking for
another member. The analysed binary therefore handles one GZIP member, not concatenated
members.

### ZIP

The ZIP entry is `0x403160..0x40376f`. It is central-directory driven:

- central-directory loading: `0x409540..0x409b1c`;
- central-entry parsing: `0x409270..0x409519`;
- local-header and data-descriptor checks: `0x408e70..0x409260`;
- local-header writer: `0x409de0`;
- central-header writer: `0x409b20`; and
- EOCD/ZIP64 writers: `0x409cb0` and `0x409d20`.

Method-8 members use the common raw Deflate optimizer. Stored and unsupported compression
methods are copied. Rewritten local and central headers clear general-purpose flag bit 3,
because the now-known CRC and sizes are placed directly in the headers and the data
descriptor is removed.

Without `/k`, local and central extra fields are removed. Without `/c`, per-member and
archive comments are removed. Stored directory-name entries and data descriptors are
always removed according to the supplied DeflOpt documentation. These are historical
DeflOpt policies, not recommendations for a modern metadata-preserving tool.

### Command-line policy

The option parser at `0x403b20..0x403ef6` recognizes:

| Option | Binary behaviour relevant to output |
|---|---|
| `/a` | scan matching files by signature, regardless of extension |
| `/b` | accept positive bit saving without a byte saving |
| `/c` | keep GZIP and ZIP comments |
| `/d` | preserve the filesystem date/time of rewritten files |
| `/f` | force rewrite under the program's no-larger policy |
| `/k` | keep otherwise removable container structures |
| `/r` | recurse through directories |
| `/s` | silent output |
| `/v` | verbose output |

These flags affect discovery, metadata, reporting, or the outer keep/rewrite decision.
They do not enable additional Deflate candidate families.

## Methods not present in the analysed DeflOpt core

The following techniques appear in later optimizers, experiments, or ports, but are not
supported by the mapped disassembly as DeflOpt 2.07 methods:

- a fresh LZ77 match search;
- arbitrary adjacent-block merging or replanning;
- identical-dynamic-tree merging;
- block splitting;
- multi-block stored output for a single source block larger than 65,535 bytes;
- permutation of equal-frequency symbols after a tree has been built;
- general Kraft-preserving length moves or bridge searches;
- a symbol-order/tree-pointer heap tie key;
- end-of-block-biased tree families;
- match expansion that first introduces missing literal codes and then searches broadly;
- package-merge candidates derived from Defluff;
- the alternate length-258 symbol alias used by some encoders;
- shortest-path optimization of the complete dynamic-header RLE;
- Java/deft4j heap variants, pruning passes, or candidate-state queues;
- APNG `fdAT` optimization; and
- time-budgeted, recursive, or unbounded search.

This exclusion list is important for both attribution and performance comparisons. A port
may sensibly add these techniques, but should label them as extensions rather than DeflOpt
parity.

## Implementation requirements and common pitfalls

An independent implementation should preserve the following details when compatibility is
the goal:

1. **Score at the actual output alignment.** Stored padding and cross-block effects depend
   on the bit position at the start of the block.
2. **Keep the exact original dynamic block bits.** Reconstructing its semantic tree is not
   an equivalent fallback.
3. **Use strict winner comparisons.** Replacing `<` with `<=` changes tie outcomes and can
   cause needless rewrites.
4. **Preserve candidate order.** Strict comparisons make enumeration order observable.
5. **De-duplicate full arrays.** Hash all 286 literal/length entries or all 30 distance
   entries before trimming HLIT/HDIST.
6. **Implement the two actual heap keys.** Initial heapify and inline repairs are
   frequency-only; `0x407b30` breaks a frequency tie with subtree height.
7. **Retain leaf-consumption order through overflow repair.** A correct length histogram
   assigned to the wrong symbols is not DeflOpt's tree.
8. **Count every extra bit.** Tree and payload comparisons include length, distance, and RLE
   extra fields.
9. **Reject unusable candidate tables.** Do not emit a token whose code length is zero.
10. **Bound source-type transitions.** Do not accidentally turn the optimizer into a much
    larger fixed/dynamic/stored search for every source block.
11. **Treat fixed coalescing as an exact special case.** It removes 7 EOB bits and a 3-bit
    header; it is not evidence for general block merging.
12. **Separate Deflate gain from metadata stripping.** Container-size comparisons otherwise
    attribute unrelated savings to the compression algorithm.
13. **Validate malformed input deliberately.** The executable's helper boundaries do not
    always expose modern defensive checks, and binary parity on valid input does not require
    reproducing unsafe failure behaviour.

## Suggested validation strategy

A new implementation can be tested in layers:

### 1. Structural validity

- Decompress every result with at least two independent Deflate decoders.
- Compare decompressed bytes exactly.
- Validate PNG CRCs and Adler-32, GZIP CRC/ISIZE, and ZIP local/central metadata.

### 2. Primitive parity

- Test canonical codes for known length arrays.
- Test all four heap variants on zero-, one-, two-, and many-symbol frequency sets.
- Include length-overflow cases and compare repaired symbol lengths, not only histograms.
- Test full-array candidate de-duplication.
- Test every boundary length for RLE symbols 16, 17, and 18.

### 3. Cost parity

- Score fixed, dynamic, and stored forms at every starting bit alignment `0..7`.
- Verify strict ties keep the incumbent.
- Include absent-symbol failures.
- Verify one fixed/fixed join saves exactly 10 bits.

### 4. Replay parity

- Compare individual source-block choices with DeflOpt where the surrounding container
  permits reliable extraction.
- Include original dynamic headers that beat the reconstructed header.
- Include matches that are cheaper, equal, and more expensive than their literals.
- Include empty blocks and consecutive fixed winners.

### 5. Container parity

Run metadata-preserving and metadata-stripping cases separately. The original program's
default stripping policies can make whole-file byte counts match even when the Deflate
payload differs, or make a compression-correct port appear worse when it intentionally
preserves metadata.

## Address index

This table is a compact route back to the disassembly.

| Address or range | Reconstructed role |
|---|---|
| `0x402050..0x4020c7` | raw Deflate stream wrapper |
| `0x4020d0..0x4021af` | zlib header/trailer setup |
| `0x4021b0..0x402983` | PNG chunk walker and compressed-chunk handling |
| `0x402a70..0x402ccf` | common whole-object output selection |
| `0x402ce0..0x402ff7` | GZIP handler |
| `0x403000..0x40315c` | PNG entry |
| `0x403160..0x40376f` | ZIP entry |
| `0x403b20..0x403ef6` | command-line option parser |
| `0x404b30` | canonical Huffman records, active-span trim, decode table |
| `0x404d80`, `0x404db0` | raw Deflate entry and per-block parse/replay |
| `0x404e44..0x404e84` | block-start state save/restore |
| `0x40510e..0x4059cc` | dynamic-header parse/replay |
| `0x405130..0x405182` | exact consecutive-fixed coalescing |
| `0x405a79` | select standard fixed tables |
| `0x405dc0..0x406da0` | bounded retry and representation selection |
| `0x40636f..0x406382` | strict match-to-literal comparison |
| `0x406692..0x406743` | one rebuilt-dynamic search after adjusted replay |
| `0x406a94..0x406b11` | empty-block deletion |
| `0x406b40` | gated fixed comparison for dynamic source |
| `0x406b7b..0x406bc6` | stored-block cost and 65,535-byte limit |
| `0x406bcb..0x406d9b` | exact original dynamic-block fallback |
| `0x407130..0x40719b` | selected-block finalization |
| `0x4071b0` | local code-length RLE rewrite and final writer |
| `0x407432..0x4074f0` | code-length length reassignment and stale-symbol rebuild |
| `0x407710` | four-variant Huffman length builder |
| `0x407a3b..0x407ad4` | maximum-length histogram repair and reassignment |
| `0x407b30` | frequency-then-subtree-height heap repair |
| `0x407bd0` | tree-depth walk |
| `0x407c90..0x407f35` | unique literal/distance candidate generation |
| `0x407f3b..0x40872b` | bounded dynamic cross-product and strict selector |
| `0x408035..0x408199` | initial greedy dynamic-header RLE |
| `0x4081c5..0x4085cb` | bounded RLE/tree feedback loop |
| `0x4086c1` | strict complete dynamic-candidate update |
| `0x408e70..0x409260` | ZIP local-header/data-descriptor checking |
| `0x409540..0x409b1c` | ZIP central-directory loader |
| `0x409b20` | ZIP central-header writer |
| `0x409cb0`, `0x409d20` | ZIP64/EOCD writers |
| `0x409de0` | ZIP local-header writer |

## Confidence and remaining limits

The Deflate core call graph, direct and indirect call sites, candidate bounds, strict-tie
branches, heap variants, and wrapper dispatch were audited against the disassembly. No
unclassified core call points to another compression search, and the core imports do not
contain a scheduling clock. The mapped methods therefore form a coherent, bounded
optimizer with no known missing DeflOpt compression family.

This is still a semantic reconstruction of a stripped executable, not recovered original
source. Register allocation, temporary object layout, diagnostics, and malformed-input
edge behaviour are intentionally abstracted where they do not change the valid-stream
method. If a claimed technique conflicts with the address-level behaviour described here,
the reference binary and its disassembly take precedence.

## Attribution

DeflOpt and the analysed executable are Copyright (C) 2003-2007 Ben Jos Walbeehm. This
document is an independent reverse-engineering description intended for interoperability,
research, and comparative implementation work.
