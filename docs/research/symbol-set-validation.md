<!-- SPDX-License-Identifier: MIT -->

# Payload symbol-set removal: validation

Date: 8 September 2026. Baseline: `287c02d`, including the completed R4
joint tree/RLE pass. **Accepted and implemented** as R5; the
[route catalog](../routes-and-methods.md#route-gate-reference) gives the
current method, gates and budgets. Implements proposal 4 in
[the research memo](new-byte-saving-methods.md).

## Why it saves bits

Removing several seldom-used length or distance symbols can shrink the
dynamic header: zero runs, advertised alphabet tails and the rebuilt code
lengths can all change.
[RFC 1951 §3.2.7](https://www.rfc-editor.org/rfc/rfc1951#section-3.2.7)

The replacement data can cost more while the complete block gets smaller. On
the completed `g10n3p04` parent, payload cost rises from 436 to 459 bits,
while the rest of the block falls from 295 to 268 bits: **23 extra data bits
buy 27 fewer header bits, saving four bits overall**. The raw stream shrinks
from 731 bits / 92 bytes to 727 bits / 91 bytes with identical decoded bytes.
The best single-symbol control reaches 728 bits; the group reaches one bit
further.

## Method notes

[`symbol_set.rs`](../../src/deflate/symbol_set.rs) implements the R5 row. Its
estimate window and shortlist are heuristics, not admissible bounds, and the
search is not exhaustive over symbol subsets or Huffman trees. Single-symbol
expansion and the proven-composition beam already overlap it; jointly
forbidden interior intervals and mixed-alphabet sets supply the additional
coverage, which the controls below separate. The validated rule limited
streams to 128 KiB compressed and decoded and 128 parsed blocks; the shortlist
holds at most 1.75 MiB of tokens per invocation.

This change also made APNG Max keep its own exact Default endpoint (initial
planner plus R1–R5) beside its replay-bounded seed, because a smaller
intermediate seed cannot dominate the terminal children of a different
parent.

## Completed Default raw streams and controls

The sample contains 365 distinct raw streams from 25 fixture families:
127 PngSuite streams and 238 additional deterministic samples. Baseline Default
outputs, regenerated from the saved baseline library, match the preceding
completed R4 outputs exactly. Neither production arm timed out.

| Sample | Improved streams | Meaningful bits saved | Raw bytes saved |
| --- | ---: | ---: | ---: |
| PngSuite | 7 / 127 | 95 | 14 |
| Other families | 20 / 238 | 765 | 99 |
| **Total** | **27 / 365** | **860** | **113** |

There were no regressions. The shortlist prototype, isolated production pass
and integrated public-API outputs match byte for byte on all 365 cases. Every
isolated rewrite was checked against its parent match intervals and
distances, and Python zlib decoded both production arms against the originals.

On every group-improving block, controls tried all eligible single-symbol
removals and the existing Max block/group/proven-composition routes, with the
same added tree families, including repricing unchanged parent tokens. A
separate emission combined the best parent, single and legacy choice per
block, including actual stored alignment. Group removal beats this control
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

The 860-bit production gain includes coverage already available through other
routes or table families; the 49-bit figure isolates the added group choices
on these witnesses. Controls ran only on group-winning blocks, so they are not
a complete alternative benchmark.

## Actual containers

Twenty-six original containers were optimized through the public API: the
previous twenty validation files plus six containing new witnesses. Ten
improved, saving **1,344 meaningful bits and 174 physical bytes**, with no
timeout or regression. Selected results:

| Container | Meaningful bits saved | File bytes saved |
| --- | ---: | ---: |
| APNG `clock.png` | 1,106 | 141 |
| APNG `happy.png` | 120 | 16 |
| GZIP `kzipmix-20200115-bsd.tar.gz` | 66 | 8 |
| PNG `g10n3p04.png` | 4 | 1 |
| ZIP `Dots.playdate-pulp.zip` | 8 | 2 |

Container structure, checksums, metadata, meaningful bits and advertised
windows were independently checked. Raw and wrapper samples overlap and are
not additive. These are selected files, not a random estimate of average
savings.

## Defluff benchmark follow-up

The [66-pair Defluff benchmark](../defluff-benchmark.md) also passes in its
established relaxed mode (`--strict 0`): 61 results beat Defluff and five tie,
with no errors or parity misses. Total savings versus Defluff are 109 file
bytes and 932 meaningful bits. Compared with a fresh build of `287c02d`, R5
improves six cases by 14 bits and one byte, with no regressions. Single
sequential whole-corpus runs took 6.851 and 7.757 seconds, a 13.2% observed
increase including process startup and I/O.

## Max evidence

An isolated R5 pass improves four of 25 recorded R4 Max references, saving
**25 bits and four bytes**, without timed search or regressions. The
references include earlier deadline-limited outputs and two versions of
`basi4a16`; they are not 25 independent unrestricted Max fixed points.

Three fresh public-API A/B pairs used the saved baseline library and the
production candidate, alternated arm order, and allowed ten optional seconds
plus the existing grace:

| Raw input | Baseline bits | Candidate bits | Outcome |
| --- | ---: | ---: | --- |
| `basi4a16` | 21,972 | 21,968 | Both timed out; one byte saved |
| `g10n3p04` | 731 | 722 | Both completed; one byte saved |
| `Dots` member ending `9c0b65f4e0075ba9` | 1,937 | 1,935 | Both timed out; one byte saved |

Applying R5 without timed work to the three fixed baseline outputs reproduces
both timed gains exactly. On `g10n3p04` it reaches 727 bits; the completed
integrated Max result reaches 722 through the wider pipeline, so the nine-bit
fresh gain is not an isolated symbol-set rewrite.

## Runtime and acceptance

Persistent API runners alternated old/new order per input. Timings exclude
compilation, process startup and file I/O; these are single paired
observations.

| Workload | Baseline | With R5 | Difference |
| --- | ---: | ---: | ---: |
| 365 raw streams | 80.635 s | 88.640 s | +8.005 s / +9.9% |
| 26 selected containers | 36.464 s | 40.946 s | +4.483 s / +12.3% |

Cost is uneven: `clock.png` rises from 3.146 to 4.626 seconds and `happy.png`
from 1.699 to 2.555 seconds. The largest raw increase was 295 ms on the
sampled GZIP member. Across 365 raw and 25 Max parents, one stream used at
most 20,370,283 rewrite units and 39 spelling prices; none exhausted either
shared budget.

The wider exploratory search needed over 50,000 spelling prices including
controls and took over twenty minutes, and a full Max table cross-product was
less effective than adding the two targeted table families to Default
pricing; neither is kept. The bounded method is accepted for reproducible
additional coverage and explicit limits, at a measurable runtime cost,
especially on containers with many small streams. Larger sets, more feedback
rounds and boundary changes remain separate experiments.

## Tests and reproduction

The release suite passes **511 tests**: 452 library, 46 CLI and 13 public API.
An exhaustive oracle enumerates all spellings and all forbidden subsets for
lengths 3–10 under eight price profiles. Other tests cover sparse interval
holes, budget and stop limits, all eight stored alignments, and exact
Default/Max floor retention, including zero-budget standalone and APNG Max.

Local evidence is in the Git-ignored `work/symbol-set-validation/` directory:
baseline provenance, source hashes, all output arms, costs, controls,
independent decode results, work counters and logs.
`benchmark-source-hashes.json` records the measured sources and
`production-hashes.json` the final ones; they differ only by an empty-input
guard in a test-only proof helper. Reproduction helpers are
`work/run-symbol-set-paired.py`, `work/verify-symbol-set-results.py`,
`work/prepare-symbol-set-validation.py`, `work/symbol-set-controls.rs` and
`work/run-symbol-set-max-paired.py`, with prototype sources in `work/`; they
need the private fixture corpus.
