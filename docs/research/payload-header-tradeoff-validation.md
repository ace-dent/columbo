<!-- SPDX-License-Identifier: MIT -->

# Payload/header tradeoff: implementation and validation

Date: 6 September 2026. Baseline: `ee6ff1d` with its committed original-match
restoration implementation. **Accepted and implemented** as the bounded R2
terminal pass;
[the route catalog](../routes-and-methods.md#payloadheader-tradeoff-r2)
describes the current method.

## Why it saves bits

A slightly worse data code can have a much cheaper description: swapping two
code lengths can create repeated lengths that the header describes more
compactly. Columbo keeps the swap only when the complete result is smaller.

For example, the completed `basi2c16` raw stream uses 4,071 meaningful bits.
Swapping the lengths of distance symbols 13 and 17 spends **one payload bit**
and removes **seven header bits**, producing 4,065 bits. Both files occupy
509 bytes. On `basi4a16`, seven extra payload bits remove eighteen header bits:
the PNG shrinks from 2,827 to 2,826 bytes.

The proposal ranking is a heuristic, not an acceptance test, and the pass tries
at most one swap per block, with no combined literal/length and distance move
or iterative swap walk. The validated rule limited streams to 128 KiB
compressed and decoded and 128 parsed blocks, with a stream-wide budget of
**1,024 full header prices**, including unchanged-tree controls.

## Controlled raw-stream results

The sample contains 365 SHA-distinct source streams and 365 distinct completed
Default parents: 127 PngSuite streams plus 238 deterministic samples, from 25
families and 270 source containers. It emphasizes compact streams and is not a
full-corpus benchmark; APNG frames and archive members are correlated.

The initial probe priced 9,558 swap candidates. The production
implementation reproduced **all 365 probe outputs byte for byte**, the new
baseline reproduced all cached parents, and neither arm hit its deadline.

| Comparison | Streams improved | Physical bytes saved | Meaningful bits saved |
| --- | ---: | ---: | ---: |
| Same-tree full header repricing versus completed Default | 21 | 7 | 46 |
| Complete new pass versus completed Default | 108 | 81 | 653 |
| Added swap contribution versus same-tree repricing | 96 | 74 | 607 |

The rows are alternative comparisons, not additive totals. Sixty-one streams
lose at least one physical byte, and winners span 19 fixture families. The
365 parents total 440,153 bytes. There were no regressions, and Python zlib
decoded all probe and production outputs.

## Actual containers

Eleven original containers were optimized through the public API,
alternating old/new order. Independent zlib, `pngcheck`, `gzip -t` and
`unzip -tqq` checks verified payloads, checksums and metadata; extracted
streams were checked for meaningful bits and sufficient advertised windows.

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
`XYB.icc.zlib`. Raw and container results overlap and are not additive.
Meaningful-bit totals come from parsing the streams, not from the public API's
removed bytes × 8.

## Max evidence

A frozen-parent check of ten recorded Max outputs from the previous
restoration pass improves seven, saving four physical bytes and 35 meaningful
bits. These are selected, correlated references, and seven of the original
Max runs had exhausted their deadline. Of the three completed runs, `f04n0g08`
improves from 1,600 to 1,597 bits and the other two keep their parent.
Same-tree header repricing ties all ten, so all 35 bits come from the swaps;
`f04n0g08` spends seven payload bits to save ten header bits.

Three fresh, sequential Max A/B pairs used a 30-second optional allowance; a
test-only gate disabled only the new pass for the baseline.

| Fresh raw reference | Baseline bits | New bits | Completion |
| --- | ---: | ---: | --- |
| `png--8x8-png--e8e82bf98731d3c3` | 347 | 347 | Both completed in about 12.3 s |
| `s34i3p04` | 1,560 | 1,559 | Both reached the deadline |
| `zip--samplelib-zip--e882327364c2214e` | 1,186 | 1,185 | Both reached the deadline |

The fresh pairs change no bytes. A fixed-parent control finds four swap bits
on the timed `s34i3p04` baseline and one on the ZIP-member baseline, and
same-tree repricing ties both. The ZIP-member fresh output matches its
fixed-parent swap output exactly; the `s34i3p04` output differs, so its
one-bit timed advantage is not an isolated measurement of this method. This
is focused Max coverage, not a broad completed Max benchmark.

## Runtime and acceptance

An alternating paired production run, excluding compilation, measured:

| Workload | Baseline | New pass enabled | Increase |
| --- | ---: | ---: | ---: |
| 365 raw streams | 81.728 s | 85.362 s | 3.635 s / 4.45% |
| 11 actual containers | 11.561 s | 12.404 s | 0.843 s / 7.29% |

These are single paired-run observations. The isolated broad probe took
3.843 s, including parsing, proposal generation, both emitted controls and
validation; its maximum per-stream swap count was 523, within the production
price cap.

The method is kept for reproducible wins across multiple families, beyond
full same-tree repricing, with bounded work and modest overhead; it promises
neither large savings nor a complete optimum.

## Tests and reproduction

Regression tests cover a small literal-only witness (three extra payload bits
save four header bits), an exhaustive short-sequence transition oracle, price
and stop limits, all eight stored-block alignments and the PNG result inside
Max's zero-optional-time Default floor. The final release suite passes 496
tests (438 library, 46 CLI, 12 public API).

Local evidence is under the ignored `work/payload-swap-validation/`
directory: `source-state.json`, `probe.csv`, `moves.csv`, paired outputs and
timings, wrapper checks, Max results and logs. The probe
`work/payload-swap-probe.rs`, A/B driver `work/run-payload-swap-paired.py` and
container checker `work/verify-payload-swap-results.py` need the private
fixture corpus. Baseline hashes and parent provenance are with the
`work/restoration-implementation/` evidence.
