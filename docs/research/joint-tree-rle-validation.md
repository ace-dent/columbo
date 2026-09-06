<!-- SPDX-License-Identifier: MIT -->

# Joint payload-tree and RLE search: validation

Date: 6 September 2026. Baseline: `9a74a21`, including restoration,
payload/header tradeoffs, advertised literal-span search and the efficiency
changes. **Accepted and implemented** as R4 in the
[route catalogue](../routes-and-methods.md#route-gate-reference).
This implements proposal 3 in [the research memo](new-byte-saving-methods.md).
It adds a search dimension to Columbo; worldwide novelty is not claimed.

## Why it saves bits

A dynamic Deflate block stores both compressed data and a compressed
description of its Huffman code tables. Repeated code lengths can make that
description cheaper. The format permits repeat instructions to cross between
the literal/length and distance lists.
[RFC 1951 §3.2.7](https://www.rfc-editor.org/rfc/rfc1951#section-3.2.7)

Choosing a tree only for its data cost can miss a smaller complete block.
The new search chooses the code lengths and their repeat instructions together,
charging both costs at every step. It can change several lengths, their
histogram, and unused-code support together, beyond the earlier local swaps.

On the completed `basi0g04` parent, data cost increases from 956 to 961 bits.
The rest of the block shrinks from 337 to 325 bits: **five more data bits buy
twelve fewer header bits, saving seven bits overall**. The result is 1,286
meaningful bits and 161 raw bytes, down from 1,293 bits and 162 bytes.
Six distance lengths change, including two previously unused positions that
receive codes. Tokens, distances, decoded data and advertised counts are fixed.

## Solver and exactness boundary

[`joint.rs`](../../src/deflate/joint.rs) freezes the parent's code-length (CL)
Huffman tree and advertised literal/length and distance spans. It jointly
chooses payload lengths and a legal RLE description under that CL tree.
Payload lengths are limited to nine bits. A smaller effective depth is exact
when the fixed CL tree has no direct symbol for a larger admitted length:
repeat instructions cannot introduce a positive length for the first time.

For depth `D`, each alphabet has `2^D` units of Kraft capacity. A positive
length `l` consumes `2^(D-l)` units; zero consumes none. The dynamic-programming
state is `(next position, remaining capacity, previous length)`. An edge emits
one direct length or repeat instruction and charges its CL codeword, repeat
extra bits and the payload-frequency cost of every assigned length.
Prefix sums make that last charge constant time.

The two payload alphabets must each finish with exactly zero remaining
capacity. An edge crossing their seam must finish the literal capacity and
consume the appropriate part of a fresh distance capacity. The previous
length carries through the seam. Positive-frequency positions, including EOB,
cannot receive zero. Unused positions can receive zero or positive lengths;
reserved distance positions 30 and 31 are forced to zero in this domain.
Complete capacity with lengths of at least one bit consequently supplies at
least two usable distance codes. The source CL tree and both resulting
payload trees must satisfy Columbo's strict compatibility rules.

Two independent suffix optimizers provide lower bounds: one minimizes remaining
payload cost subject to Kraft capacity, and one minimizes remaining header cost
while ignoring capacity. They can choose different assignments, so adding their
minima remains optimistic. Pruning against that sum cannot discard a possible
strict improvement in the declared domain.

With sufficient budget, the solver finds the minimum fixed-CL cost if a strict
improvement exists. It does not prove an unrestricted Deflate optimum. A budget
or deadline cutoff returns only an already completed winner, or no candidate;
it does not establish that better encodings are absent. It does not enumerate
other CL trees, larger depths, changed advertised counts or alternate tokens.

After finding a complete winner, the existing full header feedback search
reprices that selected payload tree at the **same advertised spans**. The
frozen-CL winner remains available if feedback ties or loses. This additional
step is not an exhaustive joint optimization over CL trees: only the selected
tree receives feedback, and fixed-CL ties or losers are not separately repriced.

## Production bounds and placement

R4 follows the complete R3 result. It admits streams of at most 128 KiB
compressed and decoded, at most 128 parsed blocks, and no discarded wire blocks.
Each invocation shares a `2^27` work budget across all blocks. The accounting
reserves preparation, allocation and dense-scan work before allocating tables,
then charges each attempted search edge. Impossible blocks also consume budget.
Full header repricing runs at most once per successful block, hence at most
128 times per invocation.

At depth nine and the maximum 318 advertised entries, the largest state table
has 1,800,117 cells. Costs, packed predecessor links and suffix bounds use less
than 24 MiB of scratch per active solver. Blocks use this memory sequentially;
completed candidates retain only their small trees and RLE. This is a per-call
bound, so concurrent container workers can each have an active solver.
Large table allocations are fallible. Preparation and search poll the stop
policy; an exhausted work budget cannot be reset by the next block.

Default and timed Max work use the existing route-start and hard-stop gates.
Max's mandatory Default comparison floor includes the same bounded pass without
the optional deadline. Its historical Max seed remains unchanged. The final
Max incumbent can receive R4 while timed work is still admitted.

All earlier complete parents remain independently selectable. The shared
terminal emitter preserves tokens and boundaries, regenerates stored-block
padding, and runs no token replay. It checks decoded size, CRC-32, Adler-32 and
actual maximum distance. Replacement requires a strict improvement in complete
bytes, then meaningful bits. A local header gain absorbed by padding is rejected.

## Completed Default raw streams

The sample contains 365 distinct completed Default streams from 25 fixture
families: 127 PngSuite streams and 238 deterministic additional samples.
Baseline outputs were freshly regenerated and matched the previous completed
R3 outputs byte for byte. Neither production arm timed out.

The independent prototype and integrated production pass produced identical
outputs on every case. Full existing header repricing of each unchanged parent
tied all 365, isolating the contribution of joint tree selection.

| Sample | Improved streams | Meaningful bits saved | Raw bytes saved |
| --- | ---: | ---: | ---: |
| PngSuite, 127 distinct streams | 55 | 700 | 87 |
| Other families, 238 distinct streams | 105 | 2,644 | 332 |
| **Total** | **160 / 365** | **3,344** | **419** |

There were no regressions. Gains appeared in 19 of the 25 sampled families,
including PNG/APNG, ZIP, GZIP and zlib. APNG frames and related archive members
remain correlated observations; one GZIP and one zlib winner do not establish
generality across those formats. Raw baseline size totals 440,071 bytes.

The prototype found 174 improving block plans. All emitted candidates were
reparsed, checked for strict trees and compared for identical tokens and decoded
identity. Python zlib independently decoded both production arms against the
original sources. A separate run of the production solver exhausted no shared
work budget; the largest charge was 76,804,039 units, below 134,217,728.

## Actual containers

Twenty original containers were optimized through the public API. The sample
retains the previous thirteen validation files and adds seven containers selected
from strong raw witnesses. It is a targeted validation sample, not a random
estimate of average savings.

Nineteen files improved, saving **7,540 meaningful bits and 946 physical bytes**.
`basi4a16.png` was the unchanged control. Selected results are:

| Container | Meaningful bits saved | File bytes saved |
| --- | ---: | ---: |
| APNG `clock.png` | 2,060 | 258 |
| APNG `happy.png` | 3,796 | 473 |
| GZIP `kzipmix-20200115-bsd.tar.gz` | 257 | 32 |
| ZIP `samples.zip` | 714 | 90 |
| ZIP `kskinmkr_src.zip` | 137 | 17 |
| zlib `XYB.icc.zlib` | 42 | 6 |
| PNG `basi0g04.png` | 7 | 1 |

PNG/APNG checks included chunk CRCs, image/frame data and relevant metadata.
ZIP and GZIP passed independent archive/decode checks; extracted streams were
checked for meaningful bits and sufficient advertised windows. All decoded
contents and checked metadata matched. Raw and wrapper samples overlap and
must not be added. The complete APNG and archive totals include members absent
from the sampled raw set. Counts come from parsing emitted streams, because the
public API reports removed bytes times eight when a physical file shrinks.

## Max controls

Twenty-five fixed Max references were taken from the preceding R3 validation:
ten earlier recorded references and fifteen eligible distinct streams from the
efficiency review's Max outputs. These include deadline-limited outputs and
correlated frames; they are not all unrestricted Max fixed points.

Without any timed search, R4 improves **20 of 25** references, saving **417 bits
and 53 bytes**, with no losses. This includes the previously completed
`f04n0g08` endpoint: 1,597 bits become 1,588 bits.

Three fresh Max A/B pairs used the same scheduler with a test-only gate disabling
only R4 in the baseline. Arm order alternated; each had a ten-second optional
allowance plus the existing grace.

| Fresh raw reference | Baseline bits | R4 bits | Outcome |
| --- | ---: | ---: | --- |
| `basi0g04` | 1,293 | 1,286 | Both timed out; 162 → 161 bytes |
| `Project1.vbp` ZIP member | 3,480 | 3,459 | Both timed out; 435 → 433 bytes |
| Small 8x8 PNG stream | 347 | 347 | Both completed; 44 bytes |

An additional fixed-parent control covered all 25 recorded and three fresh
baselines. Ordinary same-tree header repricing found no improvement, and the
isolated R4 pass reproduced every measured output byte for byte. This separates
the observed fresh gains from deadline-dependent search differences. Python
zlib independently decoded all retained Max outputs against their parents;
the fresh pairs were also compared with their original inputs.

This is focused Max evidence, not a broad completed Max benchmark. The existing
zero-optional-time PNG regression also requires Max's mandatory Default result
to match the completed Default bytes, now including R4.

## Runtime and acceptance

Persistent public-API runners alternated old/new order per input. Timings exclude
compilation, startup and file I/O; they are one paired observation, not a
statistical guarantee.

| Workload | Baseline | With R4 | Difference |
| --- | ---: | ---: | ---: |
| 365 raw streams | 76.671 s | 81.370 s | +4.699 s / +6.1% |
| 20 selected containers | 31.390 s | 35.372 s | +3.983 s / +12.7% |

Cost is uneven. The largest raw increase was 337 ms on `parachute.png`'s stream.
The complete `clock.png` APNG increased from 1.539 to 3.175 seconds while saving
258 bytes. The isolated production solver took 5.403 seconds across the raw
parents, excluding their optimization, parsing and emission. The independent
prototype took 5.962 seconds including parsing, control pricing, emission,
validation and I/O.

The method is accepted because it finds reproducible gains beyond the existing
tree/header passes across several families and fixed Max parents, keeps complete
incumbents, and has explicit work and memory limits. Its extra time and scratch
memory are material costs, particularly for containers with many small streams.
Larger depths, multiple CL seeds, joint span choices and token feedback remain
separate experiments; these results do not validate them.

## Tests and reproduction

The release suite passes 507 tests: 448 library, 46 CLI and 13 public API tests.
The independent small oracle enumerates every complete length vector in two
alphabets, then applies the existing shortest-RLE solver. Ninety-six generated
frequency/CL models cover missing direct/repeat symbols, zero-frequency filler
choices and reserved positions. Tests also cover threshold ties, separate Kraft
capacities with crossing instructions 16 and 18, exhausted shared budgets,
interruption, complete-path reconstruction, strict compatibility, all eight
stored alignments and the final 1,286-bit PNG result. Formatting and Clippy with
warnings denied pass.

Local evidence is retained in the ignored `work/joint-tree-rle-validation/`
directory: source hashes and the saved baseline library, probe plans and costs,
all paired outputs/timings, independent decode and wrapper checks, production
work counters, fixed and fresh Max outputs, and test logs. The source hashes in
`production-hashes.json` identify the measured production implementation.

Reproduction helpers are `work/run-joint-tree-rle-probe.py`,
`work/run-joint-tree-rle-paired.py`, `work/summarize-joint-tree-rle.py`,
`work/verify-joint-tree-rle-results.py` and `work/prepare-joint-tree-rle-max.py`,
with injected production-work and Max-control tests in `work/`. They require the
private fixture corpus. The production runner was linked only after an explicit
release build, and every raw output was checked against the independent probe.
