<!-- SPDX-License-Identifier: MIT -->

# Alphabet-boundary search: validation

Date: 9 September 2026. Baseline: `3a314d1`, including the completed R5
symbol-set pass. **Accepted and implemented in Max only** as R6; the
[route catalog](../routes-and-methods.md#route-gate-reference) describes the
current method. Implements proposal 5 of
[the research memo](new-byte-saving-methods.md).

## Why paired boundaries can save bits

A region with different symbols can get its own Huffman tables, letting its
neighbors use smaller alphabets; matches stay valid because block boundaries do
not reset the window.
[RFC 1951 sections 2 and 3.2](https://datatracker.ietf.org/doc/html/rfc1951)

The synthetic test `paired_cuts_can_win_when_either_cut_alone_loses` encodes a
323-byte literal sequence in 1,508 bits. A cut at token 32 costs 1,527 bits and
a cut at token 288 costs 1,513 bits; both cuts together cost **1,464 bits**,
saving 44 bits.

## Bounds and instrumentation

[`alphabet.rs`](../../src/deflate/stream/alphabet.rs) implements the
[R6 route row](../routes-and-methods.md#route-gate-reference). The interval menu
has at most 1,580 entries. The shortlist, window and eligibility rules are
heuristics, not an exhaustive partition search. Beyond the route row's limits,
the pass accepts at most 1 MiB each of compressed and decoded data, 128 parsed
blocks and no discarded wire blocks. Eighty cuts bound the graph to 3,160
edges, after at most 24 direct range prices. R6 adds no history search, new
distance, cross-block merge or Default work.

The header-plan cache preserved all 365 Default-parent and 25 frozen-Max
outputs exactly. Instrumentation observed at most 666 retained range entries
and 512 header plans in one block search, 8,507 header hits against 103,943
misses, a smallest remaining work allowance of 138 units, and at most 1,016
range prices per stream.

## Real-input anchor controls

Raw, container and Max samples below overlap; their gains are not additive.

Fresh Default outputs for 365 distinct raw streams from 25 fixture families
exactly match the preceding R5 endpoints. Applying the bounded pass to these
parents improves 19 streams, saving **4,335 meaningful bits and 539 bytes**; an
integrated Default-enabled build matched the isolated pass on all 365. These
measure opportunities, **not production Default savings**: the retained
implementation leaves Default unchanged.

The broad initial probe priced 32 pairs per block and saved 5,285 bits / 657
bytes in 19.8 seconds, including parsing, emission and checks. Eight pairs kept
5,268 bits / 655 bytes in 15.0 seconds. Production work limits reduce this to
the 539-byte result.

A stronger control ran each eligible block of the same 19 parents through the
old anchors (exact inside-match, entropy and adaptive) with **Max table
pricing** and no work ceiling. Nine streams still improve with the new anchors,
by **377 additional meaningful bits and 48 bytes**:

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

These are complete-stream comparisons with actual padding, within this graph,
not an unrestricted Deflate or full Max optimum.

The Default-enabled experiment also covered 39 PNG/APNG, GZIP, ZIP and zlib
containers (the earlier 26-file sample plus 13 witness containers). Twenty
improved, saving 679 bytes and 5,465 payload bits; neither Default API arm
timed out.

## Max validation

On 25 previously captured Max parents, the isolated pass improves one generated
raw sample by 100 bits / 12 bytes; the other 24 tie. Five freshly captured
baseline Max outputs give:

| Frozen fresh Max parent | Parent bits | R6-only bits | Bytes saved |
| --- | ---: | ---: | ---: |
| basi3p04 | 1,344 | 1,341 | 0 |
| f00n0g08 | 1,737 | 1,737 | 0 |
| f02n0g08 | 2,231 | 2,222 | 1 |
| sampled kskinmkr_src.zip member | 10,173 | 9,804 | 46 |
| generated sample | 3,213 | 3,113 | 12 |

Four improve, saving 481 bits / 59 bytes, including **381 bits / 47 bytes on
three real inputs**. This test has no wall-clock cutoff. Both frozen samples
include the generated witness.

Public-API A/B runs alternated arm order with ten seconds in both Max arms; all
eight runs in each arm reached their timeout. Six cases improve, none regresses:

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

The run saves 83 bytes / 660 bits on this selected sample. An earlier
exploratory run improved the same six inputs by 82 bytes; clock.png varied by
one byte. Timed deltas also reflect later Max work; the frozen tests above
isolate the search. Two relaxed Max A/Bs also pass: basi3p04 saves 3 bytes / 24
bits and clock.png 18 bytes / 143 bits.

## Default cost and acceptance

The [validation policy](future-deflate-methods.md#validation-policy) requires
negligible Default overhead and keeps rare boundary methods in Max. The cached
Default-enabled experiment cost 12.4% on the raw sample and 27.7% on the
selected containers, so that placement was rejected.

| Default workload | Baseline | Retained Max-only build | Difference |
| --- | ---: | ---: | ---: |
| 365 raw streams | 88.195 s | 88.391 s | +0.2% |
| 39 containers | 58.283 s | 58.365 s | +0.1% |

All 404 Default outputs are **byte-identical** to the fresh and earlier
baselines. Timings are single paired observations of persistent API runners,
excluding compilation, process startup and file I/O.

The [Defluff relaxed benchmark](../defluff-benchmark.md) passes all 66 pairs
(61 better, five ties, no errors or parity misses; 109 bytes / 932 bits ahead
of Defluff, unchanged). It checks Default compatibility only.

## Tests and reproduction

520 Rust tests pass, including all ten private-corpus regressions. Exhaustive
partition enumeration checks the shared graph at all eight alignments,
including stored edges, against independent direct pricing. A zero-budget Max
test requires the ordinary output on an input R6 can improve. Every output was
decoded independently (Python zlib; PNG CRCs, APNG structure, GZIP/ZIP payloads
and zlib windows for containers). Builds: Rust 1.97.1, macOS Apple Silicon,
release profile with LTO and overflow checks.

Local evidence is under `work/alphabet-boundary-validation/` (`max-only/`
holds final API outputs, manifests, source hashes and verification).
`work/run-alphabet-retained-validation.py` reproduces the 365 raw, 39
container, eight Max and Defluff comparisons;
`work/verify-alphabet-retained.py` checks identity, wrappers, window needs and
meaningful bits. These helpers need the private fixture corpus.
