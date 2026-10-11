<!-- SPDX-License-Identifier: MIT -->

# Joint code-length tree and RLE search

Date: 9 September 2026. Baseline: `aa7888b`, which includes R6 and the later
joint-search and short-range performance changes. **Accepted and implemented
in Max only** as R7; the [route catalog](../routes-and-methods.md#route-gate-reference)
describes the current method. Implements the header-tree proposal in
[new byte-saving methods](new-byte-saving-methods.md#6-search-the-header-tree-directly)
for the full seven-bit code-length alphabet, not only tiny support sets.

## Why this can save bits

The RLE of the payload code lengths sets the symbol frequencies of the
code-length (CL) tree, and the CL tree decides which RLE is cheapest.
Alternating the two decisions can settle where neither alone improves; the
solver makes both together.
[RFC 1951 §3.2.7](https://datatracker.ietf.org/doc/html/rfc1951#section-3.2.7)

On the completed Default `g10n3p04` witness, the existing full header search
gives a 268-bit header. Giving length value 6 a one-bit CL code instead of two
bits, removing repeat-16 support, and changing the zero-repeat prices gives a
265-bit header, with identical payload trees, advertised counts, tokens and
payload bits. A freshly captured Max parent with a different payload tree falls
from 259 to 255 header bits and from 91 to 90 stream bytes.

## Exactness

[`header/tree.rs`](../../src/deflate/header/tree.rs) implements the
[R7 route row](../routes-and-methods.md#route-gate-reference). The cost of a
nonzero run depends only on the CL lengths of its value and of repeat 16, so
for each repeat-16 choice a Kraft DP over 128 units assigns the positive
values, and enumerating the CL lengths of 0, 17 and 18 selects the matching DP
result. Zero runs are then priced exactly.

A completed search is exact **for these fixed payload trees and advertised
spans**, not over tokens, payload trees, spans or partitions. It omits no
useful CL support: removing unused leaves and unary nodes never lengthens an
emitted code or increases HCLEN, and valid length lists emit at least two CL
symbols. Stops and work limits keep completed candidates but do not prove
optimality.

## Bounds

Beyond the route row, the common gate allows at most 1 MiB each of compressed
and decoded data, 128 parsed blocks and no discarded wire blocks. Each list has
at most 318 lengths, and each block needs at most eight Kraft DPs and 4,096
zero/repeat price combinations. The five main fixed scratch structures occupy
22,819 bytes on the measured 64-bit platform. Across all 387 frozen stream
parents, 376 dynamic blocks were eligible, the largest stream charge was
9,784,020 units, and no work reservation failed, so those isolated runs
completed their search.

## Oracle and frozen-parent evidence

The independent exhaustive control enumerates complete CL assignments and runs
the existing full-sequence shortest-RLE solver for each. It agrees with the
factored solver on 80 deterministic small cases covering optional support and
repeated values. Separate checks cover all 15 mandatory values and an optimal
seven-bit CL tree. Zero-run pricing agrees with full shortest paths across
literal/repeat support combinations and lengths around 3, 6, 10, 11, 138 and
276, up to the 318-entry bound.

Fresh Default outputs for 365 distinct raw streams from the 25-family sample
match the previous Default endpoints. The unconstrained oracle and the bounded
production pass produce **byte-identical outputs on all 365**. Twenty-two
streams improve by **45 meaningful bits and three bytes**; full header
repricing improves none of their parent headers.

Fresh ten-second Max runs on those 22 witnesses provide a second set of frozen
parents. The oracle and production pass again agree on all 22: **20 improve,
saving 41 bits and four bytes**, and full header repricing finds nothing. Three
improving Max parents, including the byte-saving `g10n3p04`, completed before
their deadline. The two samples overlap by design; their gains are not
additive.

The research oracle spent 884 ms in its solver over 345 Default dynamic blocks,
at most 8.7 ms per block. Its largest per-block counts were 3,684 priced
zero/repeat profiles, 856,926 zero-run positions and 51,002 attempted Kraft
edges. The production pass, including parsing, emission and validation, took
727 ms over the 365 frozen Default parents and 76 ms over the 22 frozen Max
parents. These are separate single runs, not a speed comparison.

Both comparable stripped release executables occupy 1,728,048 bytes.

## Public API and acceptance

R7 stays **Max only** under the policy for rare header wins. Raw streams and
their wrappers, and the strict and relaxed samples, overlap; their gains are not
additive. All 365 raw and 39 container Default outputs are byte-identical to the
fresh baseline. Timings are alternating single paired observations of
persistent API runners, excluding compilation, process startup and file I/O.

| Default workload | Baseline | With Max-only R7 | Difference |
| --- | ---: | ---: | ---: |
| 365 raw streams | 90.376 s | 90.276 s | −0.1% |
| 39 containers | 58.873 s | 58.973 s | +0.2% |

Strict Max A/B pairs use ten seconds in both arms and alternate arm order. Ten
of 13 selected cases improve, with no regression:

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

The selected cases save 25 bits / four bytes. Twelve cases in each arm reached
their timeout; `g10n3p04` raw completed in both and repeated the frozen-parent
one-byte gain.

Four relaxed Max pairs also pass: `g10n3p04` raw saves four bits / one byte,
its PNG three bits, `clock.png` two bits, and the Dots ZIP ties; both arms time
out in three cases.

The final Defluff relaxed benchmark passes all **66 comparisons** (61 better,
five ties, no errors or parity misses; 109-byte / 932-bit lead unchanged). It
checks Default compatibility only. The report and state are in
`work/header-tree-validation/public/defluff.md` and
`work/header-tree-validation/defluff.json`.

Final benchmark binary SHA-256:
`8715df657ca6d3ab9eab4740d4e7d0413e00b394904abd0547e534f5b8e7ec51`.

## Checked-in validation and reproduction

All **530 Rust tests pass**, including the private-corpus regressions. They
cover exact small-tree enumeration, seven-bit completeness, budget and deadline
stops, all eight incoming alignments and strict and relaxed emission. A
zero-Max-budget test requires the completed Default output even though R7 can
improve it. Every output and wrapper was verified independently (decoded bytes,
CRCs, metadata, zlib windows).

Local evidence is under `work/header-tree-validation/`; `baseline-state.json`
records the baseline commit and source hashes. Builds use Rust 1.97.1, fat LTO
and overflow checks on macOS Apple Silicon. `work/prepare-header-tree-probe.py`
and `work/header-tree-probe.rs` reproduce the research oracle;
`work/run-header-tree-validation.py` and `work/verify-header-tree-public.py` run
and verify the public-API pairs. These helpers need the private fixture corpus.
`public/source-hashes.json` records the benchmark build and
`public/final-source-hashes.json` the final relaxed-distance test; production
source and the benchmark binary are unchanged between them.
