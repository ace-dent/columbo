<!-- SPDX-License-Identifier: MIT -->

# Alphabet-boundary search: validation

Date: 9 September 2026. Baseline: `3a314d1`, including the completed R5
symbol-set pass. **Accepted and implemented in Max only** as R6 in the
[route catalogue](../routes-and-methods.md#route-gate-reference).
This implements proposal 5 of [the research memo](new-byte-saving-methods.md).
“New” means additional coverage within Columbo; worldwide novelty is not claimed.

## Why paired boundaries can save bits

A region that uses different symbols can benefit from its own Huffman tables.
The surrounding regions can then use smaller alphabets. Savings in data
codewords and table descriptions can outweigh the added headers and end codes.
Deflate boundaries do not reset the history window, so existing matches remain
valid across them. [RFC 1951 sections 2 and 3.2](https://datatracker.ietf.org/doc/html/rfc1951)

The synthetic test `paired_cuts_can_win_when_either_cut_alone_loses` encodes a
323-byte literal sequence in 1,508 bits. A cut at token 32 costs 1,527 bits;
a cut at token 288 costs 1,513 bits. Taking both cuts costs **1,464 bits**,
saving 44 bits. This demonstrates the coupled objective. The real-input
controls below establish additional coverage beyond existing split searches.

## Candidate generation and bounds

[`alphabet.rs`](../../src/deflate/stream/alphabet.rs) finds the first and last
occurrence of each literal/length and distance symbol. It forms intervals for
individual symbols, groups of two to four consecutive *used* symbols, and
cumulative high-symbol tails. Sorting and deduplication remove repeated pairs;
the initial menu has at most 1,580 entries. EOB is not a payload occurrence.

Rank each pair by the estimated sum of its prefix, middle and suffix blocks.
Fixed pricing is exact; dynamic pricing uses the existing histogram estimator,
and the stored estimate assumes alignment zero. At most eight pairs within
64 estimated bits of the similarly estimated parent receive full pricing.
The shortlist, window and admission rules are heuristics, not impossibility
bounds or an exhaustive partition search.

Exact pricing uses the Default table grid in both strictness policies, including
EOB and actual stored padding. If a complete pair improves its parent, add the
selected endpoints to that block's existing eighth/32-token, entropy-scout and
adaptive best/secondary anchors. The shared eight-alignment boundary solver
then admits arbitrary interior slices, with at most 80 cuts. It can combine
old and new anchors. The original parent and completed direct winner survive
if the larger graph stops, exhausts its budget or fails an allocation.

The pass admits at most 1 MiB each of compressed and decoded stream data,
128 parsed blocks, and no discarded wire blocks. Eligible non-stored blocks
contain 16–32,768 tokens. One invocation shares **4,096 range prices and
2^26 charged work units**. Charges cover the initial token scan, histogram
reconstruction, and the token-storage bytes plus decoded bytes of every newly
priced range. Entropy/adaptive scouting has its existing separate fixed bounds.
These units describe admitted work, not CPU instructions. A failed reservation
leaves the remainder available for cheaper later work.

Range prices are cached within a block. The existing header cache also shares
exact length-list/RLE kernels across ranges, with at most 512 entries. Payload
and extra bits remain range-specific, and cache hits compare complete keys.
This reuse preserved all 365 Default-parent and 25 frozen-Max outputs exactly.
Instrumentation observed at most 666 retained range entries and 512 header
kernels in one block search; 8,507 header hits accompanied 103,943 misses over
the instrumented sample. The smallest remaining work allowance was 138 units;
the largest stream price count was 1,016, below the 4,096 ceiling.

Cached range entries retain tables, not copies of every range's tokens and
plain bytes. Eighty cuts bound the graph to 3,160 edges; at most 24 direct
range prices precede it. The source composite retains its existing 48 MiB
model ceiling. Optional vectors and maps reserve fallibly, and the stream
emitter reserves space for all remaining original plans after a split.
These are per-invocation limits; parent parses and concurrent routes are separate.

## Scope, placement and retention

New anchors lie between existing tokens. Old inside-match anchors can still
produce canonical submatches at the original distance, or one/two known
literals. No history search or new match distance is introduced. This sibling
splits individual source blocks; it does not merge across source boundaries.

R6 runs **only in Max**. After an exact Default or APNG Default comparison
endpoint is established, it may strengthen that endpoint within the caller's
remaining Max allowance. The historical Max seed remains separate. Prebuilt
and APNG owners use their deadline; the concurrent floor owner keeps its
assigned route window. R6 also considers the final Max incumbent after R5 if
a timed route can still start. It adds no mandatory Default work and creates
no additional deadline or grace period. A zero optional budget skips it.

Stops retain completed candidates. Later source blocks fall back to their
original representations, regenerating stored padding at the new alignment.
The common emitter checks strict generated trees and verifies decoded size,
CRC-32 and Adler-32. Actual maximum distance propagates to wrappers. Only a
whole-stream byte/meaningful-bit improvement replaces the parent; there is
no replay inside R6.

## Real-input anchor controls

Fresh baseline Default outputs were regenerated for 365 distinct raw streams
from 25 fixture families. They exactly match the preceding R5 endpoints.
Applying the bounded pass to these completed parents improves 19 streams,
saving **4,335 meaningful bits and 539 bytes**. Integrated Default-enabled
experimental outputs matched the isolated pass on all 365 cases. These are
research measurements of the available opportunities, **not production
Default savings**: the retained implementation leaves Default unchanged.

The broad initial probe priced 32 pairs per block, saving 5,285 bits / 657
bytes in 19.8 seconds including parsing, emission and checks. Eight pairs
retained 5,268 bits / 655 bytes in 15.0 seconds. Production stream work limits
reduce this to the 539-byte result; they skip some additional opportunities.

For all 19 improving streams, a stronger control retained the same completed
parent and ran each eligible source block through the old anchors with **Max
table pricing**, including exact inside-match, entropy and adaptive anchors.
It had no production work ceiling. Nine streams still improve with the new
anchors, totaling **377 additional meaningful bits and 48 bytes**:

| Witness | Bits beyond the old-anchor control | Bytes |
| --- | ---: | ---: |
| PngSuite | 60 | 7 |
| basi3p04 | 22 | 3 |
| f00n0g08 | 1 | 0 |
| f02n0g08 | 9 | 1 |
| sampled apng-large stream | 28 | 4 |
| sampled apng-medium stream | 51 | 7 |
| sampled imageworsener stream | 16 | 2 |
| sampled medium PNG stream | 49 | 6 |
| sampled small ZIP member | 141 | 18 |

These are complete-stream comparisons with actual padding, not sums of
isolated block estimates. They prove additional anchors within this graph;
they do not establish an unrestricted Deflate or full Max optimum.
Every isolated output match was checked against its parent interval and
distance. Python zlib independently confirmed decoded-byte identity.

The Default-enabled experiment also covered 39 PNG/APNG, GZIP, ZIP and zlib
containers: the preceding 26-file sample plus 13 witness containers. Twenty
improved, saving 679 bytes and 5,465 aggregate payload bits. Independent checks
verified PNG chunk CRCs, APNG frames and control structure, GZIP/ZIP payloads
and metadata, and advertised zlib windows. Raw and container samples overlap;
their gains must not be added. Neither Default API arm timed out.

## Max validation

On 25 previously captured completed Max parents, the isolated pass improves
one generated raw sample by 100 bits / 12 bytes; the other 24 tie. A second
isolation used five freshly captured baseline Max outputs:

| Frozen fresh Max parent | Parent bits | R6-only bits | Bytes saved |
| --- | ---: | ---: | ---: |
| basi3p04 | 1,344 | 1,341 | 0 |
| f00n0g08 | 1,737 | 1,737 | 0 |
| f02n0g08 | 2,231 | 2,222 | 1 |
| sampled kskinmkr_src.zip member | 10,173 | 9,804 | 46 |
| generated sample | 3,213 | 3,113 | 12 |

Four improve, saving 481 bits / 59 bytes, including **381 bits / 47 bytes on
three real inputs**. This test has no wall-clock cutoff, verifies strict
trees and parent match certificates, and independently decodes every output.
The two frozen samples overlap, including the generated witness; do not add
their totals or count that generated input as additional real-workload breadth.

Fresh public-API A/B runs alternated arm order and configured ten seconds in
both Max arms. All eight runs in each arm reported reaching their timeout;
wall times include the existing grace. Six cases improve without any regression:

| Input | Baseline bytes | With R6 | Meaningful bits saved |
| --- | ---: | ---: | ---: |
| basi3p04, raw | 168 | 165 | 24 |
| f00n0g08, raw | 218 | 217 | 5 |
| f02n0g08, raw | 279 | 274 | 42 |
| kskinmkr_src.zip member, raw | 1,272 | 1,221 | 408 |
| Animated.png, APNG | 5,905 | 5,905 | 0 |
| XYB.icc.zlib | 347 | 347 | 0 |
| generated sample, raw | 402 | 398 | 29 |
| clock.png, APNG | 23,126 | 23,107 | 152 |

The retained-mode run saves 83 bytes / 660 bits across this selected sample.
An earlier exploratory run improved the same six inputs, conservatively
repeating 82 bytes; clock.png varied by one byte. Timed output deltas include
which completed floor wins and subsequent Max work. The frozen tests above,
rather than timing differences, establish direct additional search coverage.

Two separate relaxed Max A/Bs also pass: basi3p04 saves 3 bytes / 24 bits and
clock.png saves 18 bytes / 143 bits. Independent decoders verified the raw
stream and every APNG frame. These overlapping runs are not additive totals.

## Default cost and acceptance

The existing [validation policy](future-deflate-methods.md#validation-policy)
requires negligible Default overhead and keeps rare boundary methods in Max.
The cached Default-enabled experiment cost 12.4% on the raw sample and 27.7%
on the selected containers, so that placement was rejected. The full bounded
search is retained in Max, within its existing optional allowance.

| Default workload | Baseline | Retained Max-only build | Difference |
| --- | ---: | ---: | ---: |
| 365 raw streams | 88.195 s | 88.391 s | +0.2% |
| 39 containers | 58.283 s | 58.365 s | +0.1% |

All 404 Default outputs are **byte-identical** to the fresh baseline and the
earlier baseline outputs. Timings are single paired observations of persistent
API runners, excluding compilation, process startup and file I/O. They are
not general speed guarantees. Max runtime is constrained by its configured
allowance, so its timed comparisons do not establish an algorithm speedup.

The final [Defluff relaxed benchmark](../defluff-benchmark.md) passes all 66
pairs: 61 better, five ties, no errors or parity misses, totaling 109 bytes /
932 bits better than Defluff. Output metrics match the previous report. This
is a Default compatibility/regression check; it does not exercise R6 or claim
new Defluff savings from the Max-only method.

## Tests and reproduction

- **520 Rust tests pass**, including all ten private-corpus regressions.
- Exhaustive partition enumeration checks the shared graph at all eight
  alignments, including stored edges, against independent direct pricing.
- Focused tests cover coupled cuts, budget exhaustion, mid-search stopping,
  strict/relaxed output, later stored padding and cross-block match history.
- A zero-budget Max test uses an input that R6 can improve and requires the
  ordinary output, guarding against accidental mandatory alphabet work.
- Warning-free all-target Clippy, formatting and whitespace checks pass.
- Baseline and candidate use Rust 1.97.1 on macOS Apple Silicon, with the
  repository's release profile, LTO and overflow checks.

Local evidence is under `work/alphabet-boundary-validation/`; final-mode API
outputs, manifests, source hashes and verification are in its `max-only/`
subdirectory. The saved baseline library and `source-state.json` identify the
baseline. Probe preparation copies source into temporary directories before
injecting tests; fixtures remain read-only.

`work/run-alphabet-retained-validation.py` reproduces the 365 raw, 39 container,
eight Max and Defluff comparisons with persistent baseline/candidate runners.
`work/verify-alphabet-retained.py` checks identity, wrappers, window needs and
meaningful-bit metrics. Separate relaxed-Max scripts exercise both policies.
The frozen-parent probes, exact anchor-control CSVs, source hashes and their
logs preserve the isolated evidence. These ignored helpers require the private
fixture corpus and appropriately rebuilt runners; they are not portable CI
fixtures. The synthetic and exhaustive correctness tests are checked in.
