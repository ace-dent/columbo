# DeflOpt 2.07: reverse-engineered methods

This document describes the Deflate and container-rewriting methods of DeflOpt 2.07 for
projects that reproduce, compare with, or learn from them. The disassembled 32-bit Windows
executable is the ground truth; source code and behavioral tests were used to check
interpretations but do not override it.

## Reference binary and evidence policy

The analyzed specimen is [DeflOpt.exe](binaries/deflopt/DeflOpt207/DeflOpt.exe):

| Property | Value |
|---|---|
| Product version | DeflOpt 2.07 |
| Author | Ben Jos Walbeehm |
| Release date reported by the program | 5 September 2007 |
| Format | PE32 console executable, Intel 80386 |
| SHA-256 | `af675e098c8884af0310cbecee2c5ec6ff7214a5dc5560833c51d9dc82c81bdc` |

All addresses are virtual addresses in this exact executable; they do not apply to another
release or a repacked binary. The executable has no symbols, so routine names here were
assigned during reverse engineering. Addresses, constants, and branch conditions are
observed in the disassembly; higher-level algorithms are reconstructed from several
instruction sequences and checked against the executable's outputs and, where practical,
independent models. Paragraphs marked **Implementation advice** are guidance for ports,
not claims about DeflOpt.

## Scope

DeflOpt post-processes existing Deflate streams. It parses each source block, replays its
decoded tokens, and tries a bounded set of alternative encodings without a new LZ77 match
search. Its methods are:

1. re-encode an existing dynamic block header more compactly;
2. build several length-limited Huffman trees from the existing token frequencies;
3. replace a match with its literal bytes when that is strictly cheaper under the table
   being tried;
4. choose a cheaper legal block type;
5. remove empty blocks and exact fixed-to-fixed boundaries; and
6. remove or normalize optional container structures according to command-line policy.

It does not split blocks, merge adjacent blocks in general, or explore neighborhoods of a
completed Huffman tree.

## Conventions

Bit costs include every bit that influences DeflOpt's choice: block headers, padding, tree
descriptions, Huffman code bits, and length/distance extra bits. They exclude container
bytes unless a container section says otherwise.

The binary normally updates a winner with `candidate < incumbent`, not `<=`. Equal-cost
choices therefore keep the earlier representation, and an equal-cost rewrite does not
displace the original input.

## High-level algorithm

The raw Deflate entry and per-block replay are centered on `0x404d80` and `0x404db0`:

```text
initialize input bits, output bits, and the 32 KiB history window

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

    finalize the chosen block and its BFINAL state

normalize the containing GZIP, PNG, or ZIP structure
keep the complete rewritten object only under the selected file-level policy
```

## Deflate parsing and replay state

### Complete block-start snapshots

At `0x404e44..0x404e84`, the binary copies a `0x200c`-dword (`0x8030`-byte) state area
between live and saved storage. A zero retry mode saves the current state; a non-zero
retry mode restores it. The state includes input and output bit positions, block
bookkeeping, current representation, and the 32 KiB history window, so every retry starts
from the same source position, output alignment, decoded history, block-local counters,
and output prefix.

A port needs an equivalent transactional checkpoint, not the same layout. An incomplete
checkpoint can corrupt back-references or mis-score stored-block padding.

### Stored, fixed, and dynamic input

Stored blocks are byte-aligned and `LEN` is checked against `NLEN`. Fixed blocks use the
fixed tables loaded at `0x405a79`. Dynamic headers are parsed at `0x40510e..0x4059cc`; the expanded
code-length sequence must contain exactly `HLIT + HDIST` entries.

The shared token loop at `0x405e24..0x40665e` decodes literals, end-of-block, and matches
while maintaining the circular history window. During a candidate replay, a required
symbol with no code in the proposed table makes that replay invalid; the binary does not
add the missing symbol.

Replay keeps the source's literals, end-of-block, and matches (length and distance
symbols with their extra values). A match may become literals, but no different match is
found, so a port can replay an explicit token vector where the binary uses a streaming
state machine.

### Fixed tables

The tables selected at `0x405a79` are the standard Deflate fixed tables:

| Literal/length symbols | Code length |
|---|---:|
| `0..143` | 8 |
| `144..255` | 9 |
| `256..279` | 7 |
| `280..287` | 8 |

All fixed distance codes have length 5. Symbols 286 and 287 have bit patterns but are not
legal length symbols.

## Source-type gates and bounded retries

The retry controller is at `0x405dc0..0x406da0`. Its current-form and next-form bytes
restrict which comparisons are made. The branches at `0x405dc9` and `0x406b40` and the
surrounding state transitions support this table:

| Source/current form | Alternatives considered |
|---|---|
| Stored | Preserve stored data; it is not recompressed as fixed or dynamic |
| Fixed | Replay fixed; a stored block may replace it if strictly smaller |
| Dynamic | Repack/replay the dynamic form; compare fixed and a rebuilt dynamic form; then compare stored |
| Rebuilt dynamic retry | May receive the later stored comparison |

The longest chain is:

```text
source dynamic -> selected rebuilt dynamic -> selected stored
```

That is at most three linear passes over one source block. Each transition must improve the
score strictly, so the state machine cannot cycle.

### Original dynamic block fallback

At `0x406bcb..0x406d9b`, DeflOpt can rewind to the saved block start and copy the exact
original dynamic block bits. It does so when that bit sequence is strictly smaller than the
replayed form and no other retry has won. This keeps an original header RLE that is more
efficient than DeflOpt's own.

Implementation advice: keep the original block's exact bit span as an immutable candidate.
Rebuilding the same tree is not equivalent, because Deflate permits many encodings of the
same code-length sequence.

### Empty-block deletion

The decoded byte count is calculated at `0x405dab..0x405db8`. When it is zero,
`0x406a94..0x406b11` restores the output/history side of the checkpoint while keeping the
consumed source input position, so the empty block disappears from the output.

This only deletes a block with no decoded bytes; it is not a general merge. A writer must
still produce a valid final stream; an all-empty input needs some legal final-block
representation.

### Consecutive fixed-block coalescing

At `0x405130..0x405182`, when the previous and next selected outputs are both fixed, the
binary:

1. backs the output bit position up by 7 bits, removing the previous fixed end-of-block
   code; and
2. continues with the next block's symbols without writing its 3-bit block header.

This saves exactly 10 bits per removed fixed/fixed boundary, including when a source
dynamic block selected fixed output. The mapped core has no analogous general
dynamic-block merge.

## Canonical Huffman construction

Routine `0x404b30` converts a length array into canonical Deflate codes, assigned in
symbol order and stored bit-reversed for Deflate's least-significant-bit-first packing,
each recorded with its length and symbol. It has two modes:

- **Scoring/canonicalization:** when the maximum-depth output pointer is null, it builds no
  decode table. It finds the last symbol with a non-zero length, returns the active span
  through an optional pointer, and floors an all-zero span to one. Callers then enforce
  Deflate's minimums of 257 literal/length symbols and one distance symbol.
- **Decode table:** when the pointer is present, it records the maximum length, allocates
  a dense table of `1 << max_length` entries, and fills every prefix covered by each
  reversed canonical code.

The helper has no oversubscribed-tree check. A port should reject malformed trees at its
input boundary while keeping the same canonical assignment for valid streams.

## DeflOpt's Huffman length builder

The length builder at `0x407710` produces length-limited trees for the literal/length,
distance, and code-length alphabets.

### Leaves and trivial cases

Only symbols with non-zero frequency become heap leaves. No active symbol produces no
ordinary tree; one active symbol receives length 1; two or more enter the heap merge.
Callers, not this helper, ensure required symbols such as end-of-block.

### Node keys

Each node carries at least its total frequency, subtree height, leaf or child information,
and the destination of an original symbol. There are two heap comparison policies:

```text
frequency-only:
    lower frequency first
    equal frequency retains the heap operation's existing order

height-aware:
    lower frequency first
    if frequencies tie, lower subtree height first
    if both tie, retain existing order
```

The height-aware repair helper `0x407b30` reads the node value at offset `+4`, the subtree
height. It does **not** use a symbol-order or tree-pointer tie key. The initial heapify at
`0x4077d3..0x40780f` compares frequency only. A parity port should not add an order-key
tie-breaker here.

### The four variants

A variant counter runs from 0 through 3. Its two bits select the comparison policy for the
two heap repairs in each merge:

| Variant | Repair after removing first child | Repair after inserting parent |
|---:|---|---|
| 0 | height-aware | height-aware |
| 1 | height-aware | frequency-only |
| 2 | frequency-only | height-aware |
| 3 | frequency-only | frequency-only |

Variant bit 1 controls the first repair and bit 0 the second. The calls at `0x4078f4` and
`0x4079d1` select the shared height-aware helper; the alternative inline loops use
frequency only.

### Merge procedure

The reconstructed merge step is:

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

The executable turns the second consumed node's slot into the parent at
`0x407930..0x407956`. A port may allocate a separate parent: the comparison keys and
leaf-consumption order are the same, and independent equivalence testing across the
relevant alphabet sizes confirmed identical final lengths for all four variants.

`0x407bd0` walks the completed tree and writes each leaf's depth.

### Limiting maximum code length

Literal/length and distance codes are limited to 15 bits, code-length codes to 7. If the
tree is deeper, the repair near `0x407a3b..0x407ad4` adjusts the histogram of code counts.
While the deepest occupied length `max_depth` exceeds `max_bits`, each step does:

```text
bits = greatest occupied length below max_bits

count[bits]     -= 1
count[bits + 1] += 2
count[max_depth]     -= 2
count[max_depth - 1] += 1

while count[max_depth] == 0:
    max_depth -= 1
```

The repaired lengths are then assigned shortest-to-longest in the saved leaf-consumption
order. Assigning the same histogram in a different leaf order can produce a different
payload cost.

## Dynamic-tree candidate generation

Routine `0x407c90` builds the main candidates. For each of the four heap variants it:

1. builds a 286-entry literal/length length array;
2. builds a 30-entry distance length array;
3. hashes and de-duplicates each full array independently;
4. computes the active HLIT or HDIST span; and
5. caches its canonical representation and payload cost.

De-duplication happens before active-span trimming: two candidates are duplicates only when
the complete 286-byte or 30-byte arrays match. Each side keeps at most four unique
candidates.

The selector at `0x407f3b..0x40872b` then tries the cross-product:

```text
up to 4 code-length-tree variants
    x up to 4 distance-tree candidates
    x up to 4 literal/length-tree candidates
    = at most 64 generated tree/header trials
```

The loop order at `0x4086e3..0x408725` is code-length variant outermost, then distance
candidate, then literal/length candidate. Because winner updates are strict, this order
decides which equal-cost candidate survives. A final pass then scores the saved
best/original candidate; the `jbe` at `0x408725` belongs to this pass and is not an
unbounded loop.

### Payload cost

```text
sum(literal_or_length_frequency[s] * literal_or_length_code_length[s])
+ sum(distance_frequency[d] * distance_code_length[d])
+ sum(length_extra_bit_count for every match)
+ sum(distance_extra_bit_count for every match)
```

The end-of-block symbol has frequency one and is included in the first sum. A table is not
usable if any required symbol has length zero.

### Complete dynamic-block cost

```text
3                         block header: BFINAL + BTYPE
+ 5 + 5 + 4               HLIT, HDIST, HCLEN fields
+ 3 * HCLEN               transmitted code-length-code lengths
+ sum(clen_length[token] + repeat_extra_bits[token])
+ payload_bits
```

Repeat extra-bit widths are 2 for symbol 16, 3 for 17, and 7 for 18. `HCLEN` is one plus
the index of the last non-zero code-length code in Deflate's permutation, floored to four.

## Encoding a dynamic header

DeflOpt encodes the concatenated literal/length and distance length arrays with a greedy
initial RLE plus a bounded feedback loop. It does not search all legal RLE tokenizations
for the shortest one.

### Initial greedy RLE

The initial writer begins at `0x408035`. For a run of zero lengths:

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

### Build the code-length tree

The RLE token frequencies go through the same four-variant builder with 19 symbols and a
maximum depth of 7. The active span is transmitted in the RFC 1951 permutation
`16, 17, 18, 0, 8, 7, 9, 6, 10, 5, 11, 4, 12, 3, 13, 2, 14, 1, 15`.

### Local replacement of repeat tokens

Routine `0x4071b0` and the retry path at `0x4081c5..0x4085cb` rescan an existing RLE stream
under its current code-length tree. For a repeat token representing `n` decoded length
values:

```text
repeat_cost   = clen_length[repeat_symbol] + repeat_extra_bits
explicit_cost = n * clen_length[decoded_value]
```

The repeat becomes explicit values only when `explicit_cost < repeat_cost` and the decoded
value has a non-zero code in the current code-length tree. A tie keeps the repeat.

(Inline data just before `0x4071b0` can make a linear disassembly mislabel the routine's
first bytes; disassembling from the call target recovers the prologue.)

One cross-token rewrite exists: an existing symbol 17 becomes symbol 16 when:

- the previously decoded length is zero;
- the run length is legal for both (3 through 6);
- symbols 16 and 17 both have usable codes; and
- the code for 16 is no longer than the code for 17.

Symbol 16 has one fewer extra bit than symbol 17 (2 versus 3), so this saves at least one
bit. The branch is near `0x4072b3` and `0x408388`.

### Reassign code-length-code lengths by frequency

The pass at `0x407432` scans pairs of non-zero code-length symbols and swaps their lengths
when the less frequent symbol has the shorter code. Equal lengths or equal frequencies do
not swap. Swapping keeps the length histogram, and therefore Kraft validity; canonical
codes are rebuilt afterward.

### Drop stale repeat symbols and rebuild

Local rewriting can remove the last use of repeat symbol 16, 17, or 18. Checks at
`0x407498..0x4074cf` clear a now-unused repeat symbol's code length and rebuild the
code-length tree. The stream is rescanned, because the changed code costs may allow
another strict improvement.

### Bounded feedback and termination

The state machine at `0x4081c5..0x4087db` alternates:

1. build or update the code-length tree;
2. scan the RLE stream for the local rewrites above;
3. save a stream only when its encoded size is strictly smaller; and
4. restore and score the saved best stream for the final pass.

It accepts only strictly smaller streams, so it terminates. Complete dynamic candidate
updates at `0x4086c1` are also strict.

## Repacking an original dynamic tree

The path near `0x405959..0x405979` keeps the original literal/length and distance length
arrays and regenerates only their dynamic header. This helps when the source compressor
chose a good payload tree but described it inefficiently. Together with the rebuilt trees
and the exact-original fallback, DeflOpt can keep the source table with a better header, a
new table with a new header, or the source block unchanged.

## Strict match-to-literal expansion

During replay, the branch at `0x40636f..0x406382` may replace a match with the literal
bytes it copies. Under the current candidate tables:

```text
match_bits = literal_length_code_length[length_symbol]
           + length_extra_bit_count
           + distance_code_length[distance_symbol]
           + distance_extra_bit_count

literal_bits = sum(literal_length_code_length[copied_byte])
```

The match is expanded only when all required codes exist and `literal_bits < match_bits`;
the `jae` at `0x406382` rejects equal or more expensive literals. Expansion decrements the
old length- and distance-symbol frequencies, increments the literal frequencies, and
replays the bytes as literals. History is unchanged.

After replay, `0x406692..0x406743` runs the bounded dynamic-tree selector once on the
adjusted frequencies. The result replaces the current replay only if the complete block,
including header and tables, becomes strictly smaller. A table that lacks a required
literal is rejected; the literal is never added.

## Fixed and stored block scoring

### Fixed block

The fixed candidate is its 3-bit block header plus the token payload under the fixed
tables, including every match's extra bits and the 7-bit end-of-block code. The comparison
at `0x406b40` is gated to the dynamic-source path; it is not a fixed-tree search for every
input form.

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
`N > 65535`; this path never splits one source block into several stored blocks. It wins
only when strictly smaller than the selected alternative.

## Complete per-block decision procedure

This clean-room pseudocode describes the reconstructed method, not the executable's
register-level organization:

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

The binary reaches these comparisons through retry modes and state restoration, in a fixed
order. The exact-original branch at `0x406bcb` is a state-specific fallback after the
stored comparison, not a member of an unordered candidate set, so a bit-for-bit clone
should reproduce the retry state machine and its strict-tie order.

At stream assembly, delete empty blocks, join consecutive selected fixed blocks with the
exact 10-bit rule, and set `BFINAL` on the last surviving block. The finalization tail is
at `0x407130..0x40719b`.

## Container methods

DeflOpt runs the same raw Deflate optimizer inside GZIP, PNG, and ZIP. Container code also
removes or rewrites metadata, which can account for savings unrelated to Deflate.

### Common whole-object selection

The wrapper at `0x402a70..0x402ccf` keeps a rewrite only when the file becomes smaller in
whole bytes. `/b` also accepts a positive bit saving with no byte saving. `/f` forces a
rewrite; the program documentation states that the result will not be larger than the
input.

### PNG

The PNG and zlib paths are `0x402050..0x402983`; the top-level PNG entry is
`0x403000..0x40315c`. DeflOpt:

- validates the two-byte zlib header and keeps or rewrites it so its FCHECK modulus is
  correct;
- optimizes the raw Deflate payload and keeps the Adler-32 trailer;
- combines consecutive `IDAT` chunks, which carry one zlib stream, into one `IDAT` chunk
  with a new length and CRC; and
- with `/k`, also optimizes the zlib payloads in `iCCP`, `zTXt`, and compressed `iTXt`.

By default, it keeps `IHDR`, `PLTE`, `tRNS`, `IDAT`, and `IEND` and discards other chunks;
`/k` keeps them. The mapped PNG path has no `fdAT`/APNG optimization.

### GZIP

The GZIP entry `0x402ce0..0x402ff7` validates the signature and method, walks `FEXTRA`,
`FNAME`, `FCOMMENT`, and `FHCRC`, and passes the raw Deflate body to the common optimizer.
Without `/k`, it keeps `FTEXT` and `FNAME`, discards `FEXTRA` and `FHCRC`, and discards
`FCOMMENT` unless `/c` is set. `/d` concerns the rewritten file's filesystem timestamp, not
the GZIP MTIME field.

At `0x402f28`, the payload length is taken as the end of the file minus the payload start
and the eight-byte trailer, and no further member is sought: only one GZIP member is
handled.

### ZIP

The ZIP entry `0x403160..0x40376f` is central-directory driven:

- central-directory loading: `0x409540..0x409b1c`;
- central-entry parsing: `0x409270..0x409519`;
- local-header and data-descriptor checks: `0x408e70..0x409260`;
- local-header writer: `0x409de0`;
- central-header writer: `0x409b20`; and
- EOCD/ZIP64 writers: `0x409cb0` and `0x409d20`.

Method-8 members use the common raw Deflate optimizer; stored and unsupported methods are
copied. Rewritten local and central headers clear general-purpose flag bit 3, because the
CRC and sizes are written into the headers and the data descriptor is removed.

Without `/k`, local and central extra fields are removed. Without `/c`, per-member and
archive comments are removed. According to the supplied DeflOpt documentation, stored
directory-name entries and data descriptors are always removed.

### Command-line policy

The option parser at `0x403b20..0x403ef6` recognizes:

| Option | Binary behavior relevant to output |
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

None of these enables additional Deflate candidate families.

## Methods not present in the analyzed DeflOpt core

These appear in later optimizers, experiments, or ports, but not in the DeflOpt 2.07
disassembly; a port that adds them should label them as extensions:

- a fresh LZ77 match search, block splitting, or arbitrary adjacent-block merging or
  replanning;
- identical-dynamic-tree merging;
- multi-block stored output for a source block larger than 65,535 bytes;
- permuting equal-frequency symbols after a tree is built, general Kraft-preserving length
  moves, or bridge searches;
- a symbol-order/tree-pointer heap tie key, or end-of-block-biased tree families;
- match expansion that first adds missing literal codes and then searches broadly;
- Defluff's package-merge candidates or the alternate length-258 symbol alias;
- shortest-path optimization of the complete dynamic-header RLE;
- deft4j's Java heap variants, pruning passes, or candidate-state queues;
- APNG `fdAT` optimization; and
- time-budgeted, recursive, or unbounded search.

## Suggested validation

Decode every result with at least two independent decoders and check the container
checksums and metadata. Then test the boundaries this document identifies: all four heap
variants on zero-, one-, two-, and many-symbol sets; overflow repair compared per symbol,
not by histogram; full-array de-duplication; every boundary length for RLE symbols 16, 17,
and 18; all costs at start alignments `0..7`; strict ties; absent symbols; the 10-bit
fixed/fixed join; and original headers that beat the reconstructed one. Compare whole-file
sizes only with matched metadata policies.

## Address index

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

The core call graph, call sites, candidate bounds, strict-tie branches, heap variants, and
wrapper dispatch were audited against the disassembly. No unclassified core call leads to
another compression search, and the core imports contain no scheduling clock, so no
DeflOpt compression family is known to be missing.

This is a semantic reconstruction, not recovered source. Register allocation, temporary
layouts, diagnostics, and malformed-input behavior are abstracted where they do not change
the valid-stream method; the helpers lack some modern defensive checks, which a port need
not reproduce.

## Attribution

DeflOpt and the analyzed executable are Copyright (C) 2003-2007 Ben Jos Walbeehm. This
document is an independent reverse-engineering description intended for interoperability,
research, and comparative implementation work.
