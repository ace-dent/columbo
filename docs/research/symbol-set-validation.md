<!-- SPDX-License-Identifier: MIT -->

# Payload symbol-set removal: validation

Date: 8 September 2026. Baseline: `287c02d`, including the completed R4
joint tree/RLE pass. **Accepted and implemented** as R5 in the
[route catalogue](../routes-and-methods.md#route-gate-reference).
This implements proposal 4 in [the research memo](new-byte-saving-methods.md).
It adds a search dimension to Columbo; worldwide novelty is not claimed.

## Why it saves bits

A dynamic Deflate block stores both its compressed data and a compressed
description of its Huffman tables. Removing several seldom-used length or
distance symbols can make that description smaller: zero runs, advertised
alphabet tails, and the rebuilt code lengths can all change.
[RFC 1951 §3.2.7](https://www.rfc-editor.org/rfc/rfc1951#section-3.2.7)

The replacement data can be more expensive while the complete block gets
smaller. On the completed `g10n3p04` parent, payload cost rises from 436 to
459 bits, while the rest of the block falls from 295 to 268 bits:
**23 extra data bits buy 27 fewer header bits, saving four bits overall**.
The raw stream shrinks from 731 bits / 92 bytes to 727 bits / 91 bytes.
The best single-symbol control reaches 728 bits; the group reaches one bit
further. The decoded bytes are identical.

## Candidate generation and scope

[`symbol_set.rs`](../../src/deflate/symbol_set.rs) tries intervals containing
two to four used length symbols or distance symbols. Unused positions between
support endpoints remain forbidden, so splitting a match cannot reintroduce a
hole inside the intended removed interval. Singles and pairs also supply the
best eight seeds from each alphabet; crossing these lists supplies at most
64 mixed length/distance sets, each targeting two to four used symbols.

Every occurrence of a forbidden distance symbol becomes decoded literals.
Every other match with a forbidden length symbol is solved by an exact local
shortest-path search over literals and shorter matches, excluding the whole
length set. Each submatch stays entirely inside its parent match and retains
that parent's distance. Other tokens are unchanged. No history lookup, new
match distance, image transformation, or external recompression is involved.

A cheap complete-block estimate ranks proposals, normalized against the same
estimate of the parent. At most eight proposals within a sixteen-bit estimated
losing window receive full pricing. The window and shortlist are heuristics,
not admissible impossibility bounds. This is not an exhaustive optimization
over symbol subsets or Huffman trees.

Each selected token spelling receives the Default tree-family grid, paired
bounded-depth trees with full header pricing, and the existing RLE-smoothed
count family. All generated trees use strict compatibility. One feedback
attempt revisits the same original parent matches and the same forbidden set
under the selected dynamic or fixed table prices. The first complete candidate
survives if feedback stops, ties, or loses. Removed *payload* symbols can still
receive unused filler codewords when constructing complete Huffman trees.

Existing single-symbol expansion and the header-aware proven-composition beam
already overlap this search. Jointly forbidden interior intervals and mixed
alphabet sets supply the additional coverage; the controls below separate it
from simply running those older methods later.

## Bounds, placement and retention

R5 follows the completed R4 result. It admits streams of at most 128 KiB
compressed and decoded, at most 128 parsed blocks, and no discarded wire
blocks. Eligible blocks have a strict dynamic parent and at most 8,192 tokens.
Each proposal affects at most 64 matches and 8,192 decoded bytes.

A stream invocation shares `2^25` rewrite-work units and 128 spelling prices.
The work budget reserves token scans, affected bytes and all attempted local
match edges before each rewrite. A length-`n` match reserves
`(n - 1) * (n - 2) / 2` edges, including forbidden edges. Cheap histogram and
mask work is separately bounded by the fixed menu: at most 224 base masks and
64 mixtures per eligible block. A failed reservation does not reset the budget;
cheaper later proposals may use the remainder.

There are at most eight fully priced sets per block, each with at most one
feedback price. A spelling price comprises the bounded table families above,
not one individual Huffman-tree evaluation. Large token allocations are
fallible. The shortlist retains at most eight vectors of 16,384 tokens each:
1.75 MiB with the measured 14-byte token layout, excluding current/best/feedback
vectors and the existing table planners' scratch. This is a per-invocation
bound; container workers can operate concurrently.

Default and timed Max use the existing route-start and hard-stop policies.
Stops retain only completed candidates. Max's mandatory Default endpoint
includes the same bounded pass. APNG Max now explicitly retains its own exact
Default endpoint (initial planner plus R1–R5) alongside its historical
replay-bounded seed. A smaller intermediate seed cannot dominate the terminal
children of a different parent. This adds mandatory APNG floor work; it does
not add standalone Default feedback families to APNG Default.

The common terminal emitter now accepts complete token-and-table plans while
preserving block boundaries. It runs no replay, regenerates stored padding at
the actual alignment, verifies decoded size/CRC-32/Adler-32, and propagates the
actual maximum distance. Only an improvement in complete bytes, then meaningful
bits, replaces the retained parent. A local saving absorbed by padding loses.

## Completed Default raw streams and controls

The sample contains 365 distinct raw streams from 25 fixture families:
127 PngSuite streams and 238 additional deterministic samples. Baseline Default
outputs were freshly regenerated from the saved baseline library; they match
the preceding completed R4 outputs exactly. Neither production arm timed out.

| Sample | Improved streams | Meaningful bits saved | Raw bytes saved |
| --- | ---: | ---: | ---: |
| PngSuite | 7 / 127 | 95 | 14 |
| Other families | 20 / 238 | 765 | 99 |
| **Total** | **27 / 365** | **860** | **113** |

There were no regressions. The independent shortlist prototype, isolated
production pass and integrated public-API outputs match byte for byte on all
365 cases. Every isolated rewrite was checked against its parent match
intervals and distances, and every dynamic output tree passed strict checks.
Python zlib independently decoded both production arms against the originals.

On every group-improving block, controls tried all eligible single-symbol
removals and the existing Max block/group/proven-composition routes. Controls
received the same added tree families, including repricing unchanged parent
tokens. A separate emission combined the best parent, single and legacy choice
per block, including actual stored alignment. Group removal beats this control
union on **nine streams, saving 49 additional bits and eight bytes**:

| Witness | Additional bits beyond the control union |
| --- | ---: |
| `basi4a16` | 4 |
| `g10n3p04` | 1 |
| GZIP `kzipmix` member | 17 |
| PNG `briefcase` | 6 |
| PNG `132-Ditto-1` | 4 |
| PNG `absoluterecord` | 9 |
| `s37n3p04` | 1 |
| Two distinct `Dots.playdate-pulp.zip` members | 5 and 2 |

The 860-bit production improvement includes coverage already available through
other routes or table families. The 49-bit figure isolates the added group
choices on these witnesses. Controls were evaluated on group-winning blocks;
they are not a complete alternative benchmark over all blocks in the corpus.
Related image streams, APNG frames and archive members remain correlated.

## Actual containers

Twenty-six original containers were optimized through the public API: the
previous twenty validation files plus six containers containing new witnesses.
Ten files improved, saving **1,344 meaningful bits and 174 physical bytes**.
Neither arm timed out, and no file regressed. Selected results are:

| Container | Meaningful bits saved | File bytes saved |
| --- | ---: | ---: |
| APNG `clock.png` | 1,106 | 141 |
| APNG `happy.png` | 120 | 16 |
| GZIP `kzipmix-20200115-bsd.tar.gz` | 66 | 8 |
| PNG `g10n3p04.png` | 4 | 1 |
| ZIP `Dots.playdate-pulp.zip` | 8 | 2 |

PNG/APNG CRCs, decoded image/frame data and checked metadata match. ZIP and
GZIP pass independent archive/decode checks; all extracted Deflate streams
were parsed for meaningful bits and sufficient advertised windows. Raw and
wrapper samples overlap and must not be added. These are selected validation
files, not a random estimate of average savings.

## Defluff benchmark follow-up

The [66-pair Defluff benchmark](../defluff-benchmark.md) also passes in its
established normal relaxed mode (`--strict 0`): 61 results beat Defluff and
five tie, with no errors or parity misses. Total savings versus Defluff are
109 file bytes and 932 meaningful bits. Compared with a fresh build of
`287c02d`, R5 improves six cases by 14 bits and one byte, with no regressions.
Single sequential whole-corpus runs took 6.851 and 7.757 seconds; this is a
13.2% observed increase, including process startup and I/O, rather than a
controlled estimate of general overhead. The benchmark report records all six
witnesses and reproduction provenance. Its inputs overlap the other samples.

## Max evidence

An isolated R5 pass improves four of 25 recorded R4 Max references, saving
**25 bits and four bytes**, without timed search or regressions. The references
include earlier deadline-limited outputs and two versions of `basi4a16`;
they are not 25 independent unrestricted Max fixed points.

Three fresh public-API A/B pairs used the saved baseline library and the
production candidate, alternated arm order, and allowed ten optional seconds
plus the existing grace:

| Raw input | Baseline bits | Candidate bits | Outcome |
| --- | ---: | ---: | --- |
| `basi4a16` | 21,972 | 21,968 | Both timed out; one byte saved |
| `g10n3p04` | 731 | 722 | Both completed; one byte saved |
| `Dots` member ending `9c0b65f4e0075ba9` | 1,937 | 1,935 | Both timed out; one byte saved |

Applying R5 without timed work to those three fixed baseline outputs reproduces
both timed gains exactly. On `g10n3p04` it reaches 727 bits; the completed
integrated Max result reaches 722 through the wider pipeline. That nine-bit
fresh improvement must not be described as an isolated nine-bit symbol-set
rewrite of the 731-bit parent. All retained Max streams were independently
decoded. Existing zero-optional-budget tests require both standalone and APNG
Max to retain their exact Default endpoint in strict and relaxed modes.

## Runtime and acceptance

Persistent API runners alternated old/new order per input. Timings exclude
compilation, process startup and file I/O. These are single paired observations.

| Workload | Baseline | With R5 | Difference |
| --- | ---: | ---: | ---: |
| 365 raw streams | 80.635 s | 88.640 s | +8.005 s / +9.9% |
| 26 selected containers | 36.464 s | 40.946 s | +4.483 s / +12.3% |

Cost is uneven: `clock.png` rises from 3.146 to 4.626 seconds and `happy.png`
from 1.699 to 2.555 seconds. The largest raw increase was 295 ms on the sampled
GZIP member. The isolated production checks across 365 raw and 25 Max parents
used at most 20,370,283 rewrite units and 39 spelling prices in one stream;
none exhausted either shared budget.

The wider exploratory search required over 50,000 spelling prices including
controls and took over twenty minutes. A full Max table cross-product was also
less effective than adding the two targeted table families to Default pricing.
Those broader variants are not retained. The bounded method is accepted for
reproducible additional coverage, strict certificate preservation and explicit
limits. Its modest size gains have a measurable runtime cost, especially on
containers with many small streams. Larger sets, more feedback rounds and
boundary changes remain separate experiments.

## Tests and reproduction

The release suite passes **511 tests**: 452 library, 46 CLI and 13 public API.
An exhaustive oracle enumerates all spellings and all forbidden subsets for
lengths 3–10 under eight price profiles. Tests also cover sparse interval holes,
reserved distances, exhausted budgets, interruption before and after complete
prices, strict trees, empty/stored proof checks, all eight stored alignments,
and exact Default/Max floor retention. Formatting and Clippy with warnings
denied pass.

Local evidence is in the intentionally ignored `work/symbol-set-validation/`
directory: baseline provenance, source hashes, all output arms, costs, controls,
independent decode results, work counters, and logs. The paired runner was
linked after an explicit release build. `benchmark-source-hashes.json` records
the measured sources; `production-hashes.json` records final sources. Their
only difference is an added empty-input guard in a test-only proof helper.

Reproduction helpers are `work/run-symbol-set-paired.py`,
`work/verify-symbol-set-results.py`, `work/prepare-symbol-set-validation.py`,
`work/symbol-set-controls.rs`, and `work/run-symbol-set-max-paired.py`.
Prototype sources and preparation helpers are also retained in `work/`.
They require the private fixture corpus. The recorded manifests identify
individual raw streams, their original containers and fixed Max provenance.
