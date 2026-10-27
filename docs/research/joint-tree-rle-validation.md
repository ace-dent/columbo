<!-- SPDX-License-Identifier: MIT -->

# Joint payload-tree and RLE search: validation

Date: 6 September 2026. Baseline: `9a74a21`, which includes restoration,
payload/header tradeoffs, advertised literal-span search and the efficiency
changes. **Accepted and implemented** as R4; the
[route catalog](../routes-and-methods.md#joint-tree-and-rle-r4) describes the
current method. Implements proposal 3 in
[the research memo](new-byte-saving-methods.md).

## Why it saves bits

Repeated code lengths make a dynamic header cheaper, and repeat codes may cross
from the literal/length list into the distance list.
[RFC 1951 §3.2.7](https://www.rfc-editor.org/rfc/rfc1951#section-3.2.7)
Choosing a tree only for its payload cost can miss a smaller complete block.
Unlike local swaps, a joint search can change several lengths, their histogram
and unused-code support at once.

On the completed `basi0g04` parent, payload cost increases from 956 to 961 bits
while the rest of the block shrinks from 337 to 325 bits: **five more payload
bits buy twelve fewer header bits, saving seven bits overall**. The result is
1,286 meaningful bits and 161 raw bytes, down from 1,293 bits and 162 bytes. Six
distance lengths change, including two previously unused positions that receive
codes.

## Exactness boundary

[`joint.rs`](../../src/deflate/joint.rs) implements the
[R4 entry](../routes-and-methods.md#joint-tree-and-rle-r4). A depth below nine
is exact when the fixed CL tree has no direct symbol for a larger length,
because repeat codes cannot introduce a positive length for the first time. The
suffix bounds are admissible, so pruning cannot discard a strict improvement:
with sufficient budget the solver finds the minimum fixed-CL cost. It does not
prove an unrestricted Deflate optimum or search other CL trees, larger depths,
changed advertised counts or alternative tokens. A budget or deadline cutoff
returns only an already completed winner, or no candidate.

## Production bounds

Beyond the route row, R4 accepts streams of at most 128 KiB compressed and
decoded, at most 128 parsed blocks and no discarded wire blocks. At depth nine
and the maximum 318 advertised entries, the largest state table has 1,800,117
cells. Concurrent container workers can each have an active solver.

## Completed Default raw streams

The sample contains 365 distinct completed Default streams from 25 fixture
families: 127 PngSuite streams and 238 deterministic additional samples.
Baseline outputs were freshly regenerated and matched the previous R3 outputs
byte for byte; neither production arm timed out. The independent prototype and
the integrated production pass produced identical outputs on every case, and
full header repricing of each unchanged parent tied all 365.

| Sample | Improved streams | Meaningful bits saved | Raw bytes saved |
| --- | ---: | ---: | ---: |
| PngSuite, 127 distinct streams | 55 | 700 | 87 |
| Other families, 238 distinct streams | 105 | 2,644 | 332 |
| **Total** | **160 / 365** | **3,344** | **419** |

There were no regressions. Gains appeared in 19 of the 25 families, including
PNG/APNG, ZIP, GZIP and zlib; APNG frames and related archive members are
correlated, and one GZIP and one zlib winner do not establish generality across
those formats. Raw baseline size totals 440,071 bytes.

The prototype found 174 improving block plans. Python zlib decoded both
production arms against the original sources. A separate run of the production
solver exhausted no shared work budget; the largest charge was 76,804,039
units, below 134,217,728.

## Actual containers

Twenty original containers were optimized through the public API: the previous
thirteen validation files plus seven containers selected from strong raw
witnesses. This is a targeted sample, not an estimate of average savings.
Nineteen improved, saving **7,540 meaningful bits and 946 physical bytes**;
`basi4a16.png` was the unchanged control. Selected results:

| Container | Meaningful bits saved | File bytes saved |
| --- | ---: | ---: |
| APNG `clock.png` | 2,060 | 258 |
| APNG `happy.png` | 3,796 | 473 |
| GZIP `kzipmix-20200115-bsd.tar.gz` | 257 | 32 |
| ZIP `samples.zip` | 714 | 90 |
| ZIP `kskinmkr_src.zip` | 137 | 17 |
| zlib `XYB.icc.zlib` | 42 | 6 |
| PNG `basi0g04.png` | 7 | 1 |

Independent checks of container structure, checksums, metadata, meaningful bits
and advertised windows all matched. Raw and wrapper samples overlap and are not
additive, and the APNG and archive totals include members absent from the raw
sample. Bit counts come from parsing emitted streams, because the public API
reports removed bytes times eight when a physical file shrinks.

## Max controls

Twenty-five fixed Max references came from the preceding R3 validation: ten
earlier recorded references and fifteen eligible distinct streams from the
efficiency review's Max outputs, including deadline-limited outputs and
correlated frames. Without any timed search, R4 improves **20 of 25**, saving
**417 bits and 53 bytes**, with no losses; the completed `f04n0g08` endpoint
falls from 1,597 to 1,588 bits.

Three fresh Max A/B pairs used a test-only gate that disabled only R4 in the
baseline, alternating arm order and a ten-second optional allowance plus the
existing grace:

| Fresh raw reference | Baseline bits | R4 bits | Outcome |
| --- | ---: | ---: | --- |
| `basi0g04` | 1,293 | 1,286 | Both timed out; 162 → 161 bytes |
| `Project1.vbp` ZIP member | 3,480 | 3,459 | Both timed out; 435 → 433 bytes |
| Small 8x8 PNG stream | 347 | 347 | Both completed; 44 bytes |

On all 25 recorded and three fresh baselines, same-tree header repricing found
no improvement, and the isolated R4 pass reproduced every measured output byte
for byte, separating the fresh gains from deadline-dependent search
differences. This is focused Max evidence, not a broad Max benchmark.

## Runtime and acceptance

Persistent public-API runners alternated old/new order per input. Timings
exclude compilation, startup and file I/O; they are one paired observation.

| Workload | Baseline | With R4 | Difference |
| --- | ---: | ---: | ---: |
| 365 raw streams | 76.671 s | 81.370 s | +4.699 s / +6.1% |
| 20 selected containers | 31.390 s | 35.372 s | +3.983 s / +12.7% |

Cost is uneven: the largest raw increase was 337 ms on `parachute.png`'s stream,
and the complete `clock.png` APNG rose from 1.539 to 3.175 seconds while saving
258 bytes. The isolated production solver took 5.403 seconds across the raw
parents, excluding their optimization, parsing and emission; the independent
prototype took 5.962 seconds including parsing, control pricing, emission,
validation and I/O.

R4 is accepted for reproducible gains beyond the existing tree/header passes
across several families and fixed Max parents, with explicit work and memory
limits. Its extra time and scratch memory are significant costs, particularly
for containers with many small streams. Larger depths, multiple CL seeds, joint
span choices and token feedback remain untested.

## Tests and reproduction

The release suite passes 507 tests: 448 library, 46 CLI and 13 public API tests.
An independent small oracle enumerates every complete length vector in two
alphabets and applies the shortest-RLE solver over 96 generated frequency/CL
models. Other tests cover crossing repeat codes, budget and stop limits, strict
compatibility, all eight stored alignments and the 1,286-bit PNG result. A
zero-optional-time Max regression requires Max's mandatory Default result to
match the Default bytes, including R4.

Local evidence is in the ignored `work/joint-tree-rle-validation/` directory;
`production-hashes.json` identifies the measured production implementation.
Reproduction helpers are `work/run-joint-tree-rle-probe.py`,
`work/run-joint-tree-rle-paired.py`, `work/summarize-joint-tree-rle.py`,
`work/verify-joint-tree-rle-results.py` and `work/prepare-joint-tree-rle-max.py`,
with injected production-work and Max-control tests in `work/`. They require the
private fixture corpus.
