<!-- SPDX-License-Identifier: MIT -->

# Joint code-length tree and RLE search

Date: 9 September 2026. Baseline: `aa7888b`, including R6 and the subsequent
joint-search and short-range performance changes. **Accepted and implemented
in Max only** as R7 in the [route catalogue](../routes-and-methods.md#route-gate-reference),
following the existing [validation policy](future-deflate-methods.md#validation-policy).

This implements the header-tree proposal in
[new byte-saving methods](new-byte-saving-methods.md#6-search-the-header-tree-directly).
The implementation solves the full seven-bit code-length alphabet for fixed
data-tree length lists, rather than enumerating only tiny support sets.
Additional coverage within Columbo is demonstrated; worldwide novelty is not
claimed.

## Why this can save bits

Deflate compresses its data-tree descriptions with another Huffman tree: the
code-length (CL) tree. Choosing a repeat spelling changes that tree's symbol
frequencies, while choosing the tree changes which repeats are cheapest.
Alternating these decisions can settle at a result that neither decision
alone improves. The new solver chooses both together.

On the completed Default `g10n3p04` witness, the existing full header search
costs 268 bits. Giving length value 6 a one-bit CL code instead of two bits,
removing repeat-16 support, and changing the zero-repeat prices produces a
265-bit header. The data trees, advertised counts, tokens and payload bits
are identical. A freshly captured Max parent has a different fixed data tree;
its header falls from 259 to 255 bits and its stream from 91 to 90 bytes.

The wire format is unchanged: CL codes are at most seven bits, and repeats
encode the concatenated literal/length and distance length lists, including
across their seam. [RFC 1951 §3.2.7](https://datatracker.ietf.org/doc/html/rfc1951#section-3.2.7)

## Exact search by factoring run costs

[`header/tree.rs`](../../src/deflate/header/tree.rs) groups equal values into
maximal runs over the complete transmitted length list. For each positive
length value, it records the number of runs of each length. A nonzero run must
start with an explicit value; subsequent values can use literals or repeat 16.
Its cheapest cost depends only on that value's CL code length and repeat 16's
code length.

For each of the eight repeat-16 choices (absent, or length 1–7):

1. Compute each positive value's exact aggregate run cost for CL lengths 1–7.
2. Use dynamic programming to assign these mandatory positive-value codes.
   A length `l` consumes `2^(7-l)` of 128 Kraft units. Keep the cheapest cost
   and reconstruction choices for every attainable capacity.
3. Enumerate the CL lengths of zero, repeat 17 and repeat 18, including absence.
   Their capacity consumption selects the required positive-value DP result.
4. Price all zero runs exactly under the four chosen zero/repeat prices, then
   compare the complete header cost. Three monotone queues price the repeat
   ranges without rescanning every permissible run length.

A zero run may begin with literal zero, repeat 17 or repeat 18. Repeat 16 is
admitted only after at least one zero has already been emitted, including a
zero produced by another repeat. The positive-run calculation enforces the
same previous-value rule. No reset occurs at the LL/DD seam.

Every distinct positive value in the data-length list needs a CL code: its
first occurrence cannot be introduced by a repeat. The optional symbols
16, 17, 18 and 0 occupy the first four transmission-order positions. Therefore
HCLEN is fixed by the last mandatory positive value; it does not vary across
this search. The solver includes its actual three-bit-per-entry cost and all
fixed header fields and repeat extras.

Complete enumeration is exact **for these fixed data trees and advertised
spans**. It is not an optimum over tokens, data trees, spans, or partitions.
No useful CL support is omitted: unused leaves can be removed and unary nodes
suppressed without lengthening an emitted code or increasing HCLEN. Valid
Deflate data-length lists require at least two emitted CL symbols, so this
removal cannot create the forbidden singleton-completion case. Impossible
repeat symbols are omitted only when no run can emit them.

The positive-value DP cost plus the fixed header fields is a lower bound on
the complete cost, since zero-run costs are nonnegative. This bound may skip
profiles that cannot strictly beat the incumbent. Stops and work limits do
not prove optimality; they preserve completed candidates only.

## Bounds and placement

R7 is a terminal Max sibling following R6. It may strengthen a completed
ordinary or APNG Default comparison endpoint within that owner's existing Max
deadline or route window. The historical Max seed remains independent. It
also considers the final Max incumbent while another timed route may start.
It adds no mandatory Default work, deadline or grace period.

The common terminal gate admits at most 1 MiB each of compressed and decoded
stream data, 128 parsed blocks, and no discarded wire blocks. Only dynamic
blocks are eligible. Each list has at most 318 lengths; all 15 positive length
values are supported. There are at most eight positive-value Kraft DPs and
4,096 zero/repeat price combinations per block.

One invocation shares **2^24 charged work units** across its stream. Charges
cover fixed-array initialization, positive-run transitions, attempted Kraft
edges, price profiles, and zero-run transitions. A repeat window's queue work
is amortized linear in the charged positions. These units are an admitted-work
measure, not CPU instructions. Reservations fail before entering that work;
completed trees survive a failure or deadline stop.

Scratch is fixed by the Deflate alphabet: run counts use `16 × 319` u16s,
positive-value choices use `16 × 129` bytes, and three zero-run queues each
have 319 indices. The Kraft frontier has 129 costs. Positive-value prices are
reused across all zero/repeat completions for a repeat-16 choice, and zero-run
scratch is reused without per-profile allocation. There is no tree-state hash
cache or retained path beam. One winning RLE spelling is reconstructed with
the existing shortest-path routine; it and the final plan use bounded vectors.
The five main fixed structures occupy 22,819 bytes on the measured 64-bit
platform; this excludes compiler stack temporaries and RLE reconstruction
vectors. Across all 387 frozen stream parents, 376 dynamic blocks were
eligible, the largest stream charge was 9,784,020 units, and no work
reservation failed. Thus those isolated runs completed their admitted search
rather than stopping at the production work cap. The common parser, parent
candidate and emitter retain their separate limits.

Payload cost is recovered from the validated original block and its exact
header cost. R7 does not rescan the token array. The emitter preserves tokens,
data-code lengths, HLIT and HDIST, regenerates later stored padding, validates
decoded identity and strict generated trees, and propagates actual distances
to wrappers. Only a whole-stream byte/meaningful-bit improvement replaces the
parent. R7 performs no replay or history search.

## Oracle and frozen-parent evidence

The independent exhaustive control enumerates complete CL assignments and
runs the existing full-sequence shortest-RLE solver for each. It agrees with
the factored solver on 80 deterministic small cases covering optional support
and repeated values. Separate checks cover all 15 mandatory values and an
optimal seven-bit CL tree. Zero-run pricing agrees with full shortest paths
across literal/repeat support combinations and lengths around 3, 6, 10, 11,
138 and 276, through the 318-entry bound.

Fresh baseline Default outputs cover 365 distinct raw streams from the
existing 25-family sample. All independently decode to the original bytes and
match the previous completed Default endpoints. The unconstrained oracle and
bounded production terminal pass produce **byte-identical outputs on all 365**.
Twenty-two streams improve by **45 meaningful bits and three bytes**. Ordinary
full header repricing improves none of their parent headers. The gains are
additional CL-tree/RLE coverage, not extra calls to the existing repricer.

Fresh ten-second Max runs on those 22 witnesses provide a second set of frozen
parents. Applying the oracle and production pass to those exact bytes gives
identical outputs on all 22: **20 improve, saving 41 bits and four bytes**.
The existing full header repricer again finds no improvement. Python zlib
independently verifies every output; Columbo also checks complete strict
trees, decoded identity and unchanged token sequences. Three improving Max parents, including the byte-saving `g10n3p04`,
completed before their deadline. The isolated comparison depends only on
those frozen bytes. The two samples purposefully overlap and their gains must
not be added.

The research oracle spent 884 ms in its solver over 345 Default dynamic
blocks, with an observed maximum of 8.7 ms per block. Its largest per-block
counts were 3,684 priced zero/repeat profiles, 856,926 zero-run positions and
51,002 attempted Kraft edges. The production pass, including parsing,
emission and validation, took 727 ms over the 365 frozen Default parents and
76 ms over the 22 frozen Max parents. These are separate single-run timings,
not a head-to-head speed claim. The production and research code differ in
scratch reuse and incumbent pruning.

Both comparable stripped release executables occupy 1,728,048 bytes.

## Public API and acceptance

The method is retained in **Max only** under the existing policy for rare
header wins. Its exact solver adds independently verified search coverage
and uses the current Max allowance. Default receives no additional search.

All 365 raw and 39 container Default outputs are byte-identical to the fresh
baseline. Independent checks verify raw decoded bytes and container payloads,
PNG chunk CRCs, APNG frame/control structure, ZIP contents/metadata, GZIP
integrity, and advertised zlib windows.

| Default workload | Baseline | With Max-only R7 | Difference |
| --- | ---: | ---: | ---: |
| 365 raw streams | 90.376 s | 90.276 s | −0.1% |
| 39 containers | 58.873 s | 58.973 s | +0.2% |

These are alternating, single paired observations of persistent API runners,
excluding compilation, process startup and file I/O. They are not general
speed guarantees.

Strict Max A/Bs use ten seconds in both arms and alternate arm order. Ten of
13 selected cases improve, with no regression:

| Input | Baseline bytes | With R7 | Meaningful bits saved |
| --- | ---: | ---: | ---: |
| g10n3p04, raw | 91 | 90 | 4 |
| g04n3p04, raw | 95 | 95 | 1 |
| checked.png stream, raw | 222 | 222 | 4 |
| yahoo.png stream, raw | 1,835 | 1,834 | 2 |
| Dots ZIP member, raw | 219 | 218 | 2 |
| textrend ZIP member, raw | 4,720 | 4,720 | 2 |
| g10n3p04.png | 212 | 212 | 3 |
| checked.png | 484 | 484 | 4 |
| yahoo.png | 2,946 | 2,945 | 2 |
| Dots.playdate-pulp.zip | 3,445 | 3,445 | 0 |
| clock.png, APNG | 23,107 | 23,107 | 1 |
| asyoulik-gzip.txt.gz | 48,425 | 48,425 | 0 |
| XYB.icc.zlib | 347 | 347 | 0 |

The selected cases total 25 bits / four bytes saved. Raw streams and their
wrappers overlap, so this is not independent workload breadth. Twelve cases
in each arm reached their timeout; `g10n3p04` raw completed before its deadline
in both and repeated the frozen-parent one-byte gain. Other timed differences
may include which completed comparison endpoint wins and subsequent Max work;
the frozen controls establish direct search coverage separately.

Four relaxed Max pairs also pass: `g10n3p04` raw saves four bits / one byte,
its PNG saves three bits, `clock.png` saves two bits, and the Dots ZIP ties.
Both arms time out in three cases. These runs overlap the strict sample and
their totals must not be added. Every raw stream and reconstructed wrapper
passes independent identity, CRC and window checks.

The final Defluff relaxed benchmark passes all **66 comparisons**: 61 better,
five ties, no errors or parity misses, with the same 109-byte / 932-bit lead
as the pre-existing report. This is a Default compatibility check; it does
not exercise R7 or attribute new Defluff savings to the Max-only method. The
fresh report and state remain in `work/header-tree-validation/public/defluff.md`
and `work/header-tree-validation/defluff.json`.

Final benchmark binary SHA-256:
`8715df657ca6d3ab9eab4740d4e7d0413e00b394904abd0547e534f5b8e7ec51`.

## Checked-in validation and reproduction

- All **530 Rust tests pass**, including the private-corpus regressions.
- Focused tests cover exact small-tree enumeration, seven-bit completeness,
  all positive CL literals, optional zero/repeat support, budget exhaustion,
  deadline interruption, and independent full-RLE prices.
- Complete emission tests cover all eight incoming bit alignments, later
  stored blocks, cross-block history and unchanged data trees/tokens.
- Strict and relaxed policies are tested separately, including relaxed empty
  and singleton distance alphabets. A zero-Max-budget test requires the
  completed Default output even though R7 can improve that exact endpoint.
- Warning-free all-target Clippy and formatting pass.

Local evidence is under `work/header-tree-validation/`. `baseline-state.json`
records the baseline commit and source hashes. Saved baseline/candidate API
runners use the repository's release profile, Rust 1.97.1, fat LTO and overflow
checks on macOS Apple Silicon.

`work/prepare-header-tree-probe.py` and `work/header-tree-probe.rs` reproduce
the research oracle in a temporary source copy. The separate production probe
uses the actual integrated terminal pass. `work/run-header-tree-validation.py`
runs alternating public-API pairs; `work/verify-header-tree-public.py` checks
independent raw/wrapper identity, metadata, CRCs, window requirements and exact
meaningful bits. These local helpers require the private fixture corpus and
rebuilt runners. Fixtures remain read-only; checked-in tests are standalone.
`public/source-hashes.json` records the benchmark build;
`public/final-source-hashes.json` additionally captures the final strengthened
relaxed-distance test. Production source and the benchmark binary are unchanged
between those records.
