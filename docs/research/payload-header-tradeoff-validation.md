<!-- SPDX-License-Identifier: MIT -->

# Payload/header tradeoff: implementation and validation

Date: 6 September 2026. Baseline: `ee6ff1d` with its committed original-match
restoration implementation. **Accepted and implemented** as the bounded R2
terminal pass in [the route catalogue](../routes-and-methods.md#route-gate-reference).
This is an additional search dimension in Columbo; worldwide novelty is not claimed.

## Why it saves bits

A dynamic Deflate block stores both compressed data and a header describing its
codes. A slightly worse data code can have a substantially cheaper description.
Swapping two code lengths can create repeated lengths that the header describes
more compactly. Columbo keeps the swap only when the complete result is smaller.

For example, the completed `basi2c16` raw stream uses 4,071 meaningful bits.
Swapping the lengths of distance symbols 13 and 17 spends **one payload bit**
and removes **seven header bits**, producing 4,065 bits. Both files occupy
509 bytes. On `basi4a16`, seven extra payload bits remove eighteen header bits:
the PNG shrinks from 2,827 to 2,826 bytes.

## Implemented search

The new pass runs after original-match restoration. It preserves all tokens,
distances and block boundaries. It first fully reprices the unchanged dynamic
trees, then explores swaps of unequal, nonzero lengths within either data
alphabet. Swapping preserves symbol support, maximum depth and the Kraft sum.
Reserved distance symbols 30 and 31 are excluded; strict mode still requires
complete literal/length, distance and code-length trees and two usable distance
symbols.

For symbols `a` and `b`, the extra payload cost is:

`tax = (frequency[a] - frequency[b]) × (length[b] - length[a])`.

The bounded menu admits taxes of 1–18 bits and requires fewer adjacent length
transitions. It retains at most 32 proposals per alphabet, ordered by
`tax - 3 × transitions_removed`, with deterministic ties. Transition changes
are computed from the at-most-four affected edges, including the literal/distance
seam. This ranking is a heuristic, not an impossibility proof or an acceptance
test. Every admitted tree receives existing full header/RLE pricing.

All proposals are siblings of the unchanged parent: there is at most one
selected swap per block per pass, with no combined LL/DD move or iterative swap walk.
The ordinary greedy swap guard still rejects positive payload deltas; its
historical descendants are unchanged.

The selected stream must be at most 128 KiB compressed and decoded, with at
most 128 parsed blocks and no wire blocks discarded by parser normalization.
Each invocation has a stream-wide budget of **1,024 full header prices**, including
unchanged-tree controls. New timed routes obey the soft deadline; admitted work
polls the hard stop between proposals and blocks. Completed prices survive
interruption. Max's mandatory Default comparison floor includes the identical
bounded pass, while its historical search seed stays independent. Max may also apply the pass
to its final selected result while timed work is still permitted; each invocation
has its own price budget.

Emission regenerates stored-block padding at its new alignment. The common
candidate builder checks decoded size, CRC-32 and Adler-32 and propagates the
actual maximum distance to wrappers. Complete file/stream bytes, then meaningful
bits, decide acceptance. No token search or replay follows.

## Controlled raw-stream results

The sample contains 365 SHA-distinct source streams and 365 distinct completed
Default parents: 127 PngSuite streams plus 238 deterministic samples from other
fixture families. It covers 25 families and 270 source containers. APNG frames
and archive members remain correlated observations. The sample deliberately
emphasizes compact streams and is not a full-corpus benchmark.

Parents were the completed current implementation, including original-match
restoration. The initial probe priced 9,558 swap candidates. The integrated
production implementation subsequently reproduced **all 365 probe outputs
byte for byte**, and the newly measured baseline reproduced all cached parents.
Neither arm hit its deadline.

| Comparison | Streams improved | Physical bytes saved | Meaningful bits saved |
| --- | ---: | ---: | ---: |
| Same-tree full header repricing versus completed Default | 21 | 7 | 46 |
| Complete new pass versus completed Default | 108 | 81 | 653 |
| Added swap contribution versus same-tree repricing | 96 | 74 | 607 |

These rows are alternative comparisons, not additive totals. Sixty-one streams
lose at least one physical byte. Winning streams span 19 fixture families.
The 365 completed parents total 440,153 bytes; the absolute gains are small,
as expected for already optimized data. There were no regressions.

Every emitted probe candidate was reparsed and checked for strict compatibility
and decoded identity. Python zlib independently decoded all control and candidate
streams. Fresh production outputs were checked again against their parents.

## Actual containers

Eleven original containers were optimized through the public API, alternating
old/new order by case. Independent zlib, `pngcheck`, `gzip -t` and `unzip -tqq`
checks verified payloads, checksums and relevant metadata. Extracted streams
were inspected for exact meaningful bits and sufficient advertised windows.

| Container | Physical bytes saved | Meaningful bits saved |
| --- | ---: | ---: |
| PngSuite `basi4a16.png` | 1 | 11 |
| PngSuite `tbbn2c16.png` | 0 | 1 |
| APNG `clock.png` | 14 | 120 |
| APNG `happy.png` | 28 | 225 |
| GZIP `kzipmix-20200115-bsd.tar.gz` | 7 | 51 |
| ZIP `kwincheat.zip` | 6 | 44 |
| ZIP `sample-project.zip` | 0 | 4 |
| Four unchanged controls | 0 | 0 |
| **Total** | **56** | **456** |

The unchanged controls were `f00n0g08.png`, `s33i3p04.png`, `rekzip.zip` and
`XYB.icc.zlib`. Thus zlib wrapper compatibility was tested without a positive
saving on that particular reference. Raw and container results overlap and
must not be added. The public API reports removed bytes × 8 when a file shrinks;
the meaningful-bit totals above instead come from parsing the actual streams.

## Max evidence

A focused frozen-parent check uses ten recorded Max outputs after the previous
restoration pass. Seven improve, saving four physical bytes and 35 meaningful
bits. These are selected, correlated references: seven of the original Max runs
had exhausted their deadline, and these records are not a fresh completed Max
corpus. Among the three originally completed runs, `f04n0g08` improves from
1,600 to 1,597 bits; the other two retain their parent.

Full same-tree header repricing ties every one of these ten parents. All 35
saved bits therefore come from the new swaps, including the completed
`f04n0g08` witness, which spends seven payload bits to save ten header bits.

Three fresh Max A/B pairs used a 30-second optional allowance. A test-only gate
in a disposable source copy disabled just the new terminal pass for the
baseline; the rest of the scheduler and source were identical. Runs were
sequential, and outputs were independently decoded.

| Fresh raw reference | Baseline bits | New bits | Completion |
| --- | ---: | ---: | --- |
| `png--8x8-png--e8e82bf98731d3c3` | 347 | 347 | Both completed in about 12.3 s |
| `s34i3p04` | 1,560 | 1,559 | Both reached the deadline |
| `zip--samplelib-zip--e882327364c2214e` | 1,186 | 1,185 | Both reached the deadline |

These fresh pairs have no byte changes. A separate fixed-parent control finds
four swap bits on the timed `s34i3p04` baseline and one on the ZIP-member
baseline; same-tree repricing ties both. The ZIP-member fresh output exactly
matches its fixed-parent swap output. The `s34i3p04` fresh output differs, so
its one-bit timed advantage is not treated as an isolated measurement of this
method. A final timed pass cannot start after its soft deadline; the bounded
mandatory Default floor remains available independently.

This establishes focused Max coverage and preserved incumbents, not a broad
completed Max benchmark or an unrestricted fixed point.

## Runtime and acceptance

An alternating paired production run measured:

| Workload | Baseline | New pass enabled | Increase |
| --- | ---: | ---: | ---: |
| 365 raw streams | 81.728 s | 85.362 s | 3.635 s / 4.45% |
| 11 actual containers | 11.561 s | 12.404 s | 0.843 s / 7.29% |

Compilation was excluded. These are single paired-run observations, not
statistical performance guarantees. The isolated broad probe took 3.843 s,
including parsing, proposal generation, both emitted controls and validation.
Its maximum per-stream swap count was 523, within the production price cap.

The method is retained because the added search dimension produces reproducible
wins across multiple families, beyond full same-tree repricing, with bounded
work and modest measured overhead. It does not promise large savings or a
complete optimum. Larger models, higher taxes, transition-neutral permutations,
rotations, joint alphabet moves and wider header solvers remain unexplored by
this pass.

## Tests and reproduction

Regression tests cover a small literal-only witness (three extra payload bits
save four header bits), support/Kraft preservation, an exhaustive short-sequence
transition oracle, shared price limits, interruption, strict/relaxed emission,
all eight stored-block alignments, discarded empty blocks, and the PNG result
inside Max's zero-optional-time Default floor. A frozen, originally completed
Max witness exercises the gain without a timed search. The final release suite
passes 496 tests (438 library, 46 CLI, 12 public API); formatting and Clippy with
warnings denied also pass.

Local evidence is retained under the ignored `work/payload-swap-validation/`
directory: `source-state.json`, `probe.csv`, `moves.csv`, paired raw/container
outputs and timings, wrapper metrics and semantic checks, Max results and logs.
The standalone probe source is `work/payload-swap-probe.rs`; the production A/B
driver is `work/run-payload-swap-paired.py`; independent container checks are in
`work/verify-payload-swap-results.py`. These require the private fixture corpus.
The baseline source hashes and completed-parent provenance are preserved alongside
the previous `work/restoration-implementation/` evidence.
