<!-- SPDX-License-Identifier: MIT -->

# Advertised literal/length span: validation

Date: 6 September 2026. Baseline: `e4400b5`, including original-match
restoration, payload/header tradeoffs and the subsequent efficiency changes.
**Accepted and implemented** as R3;
[the route catalog](../routes-and-methods.md#literallength-span-r3) describes
the current method.

## Why adding unused entries can save bits

Advertising a few more unused literal/length entries inserts zeros at the
alphabet boundary. That can make a repeat run cheaper or change the best
code-length tree enough to shorten the header, while the data codewords stay
identical; HLIT costs five bits for any legal count, and repeat codes may cross
the literal/distance seam
([RFC 1951 §3.2.7](https://www.rfc-editor.org/rfc/rfc1951#section-3.2.7)).

On the completed `basi0g04` stream, changing the advertised count from 269 to
277 extends the seam's zero run from three to eleven lengths, and the stream
drops from 1,294 to 1,293 meaningful bits with unchanged payload codes. This
win also needs a different code-length tree; keeping the parent's misses it.

R3 exhausts the legal counts when time and budget permit; it does not prove a
global optimum over all Huffman trees or RLE descriptions. The validated rule
limited streams to 128 KiB compressed and decoded and 128 parsed blocks.

## Controlled raw-stream results

The sample contains 365 distinct completed Default streams from 25 fixture
families: 127 distinct PngSuite streams and 238 additional deterministic
samples. The regenerated baseline matched the previous payload-tradeoff
results byte for byte. No measured Default run timed out.

Full enumeration priced 2,490 alternative spans, and the production pass
reproduced every probe output exactly. Full same-tree header repricing tied
all 365 parents, so every gain below comes from the new count choice.

| Completed raw stream | Advertised count change | Meaningful bits saved | Physical bytes saved |
| --- | --- | ---: | ---: |
| `basi0g04` | 269 → 277 | 1 | 0 |
| `basi4a16` | 274 → 275 | 1 | 0 |
| `Project1.vbp` from `kskinmkr_src.zip` | 267 → 268 | 2 | 1 |
| **Total** | | **4** | **1** |

The last case changes 3,482 bits to 3,480, crossing a byte boundary, by
inserting only one zero before a nonzero distance length; looking only for
longer repeat runs is therefore insufficient.

Two shortcuts were rejected: shortest RLE under the parent's or the ordinary
repricer's fixed code-length tree missed all three gains, and a candidate list
based on seam run lengths 3, 6, 7, 10 and 11 missed the byte-saving case. Full
enumeration is cheap enough to keep. The pass runs after R2's payload-costly
swaps, so these savings are not additive to R2's earlier independent probe.

Python zlib decoded both probe arms and the production results. There were no
regressions.

## Actual containers

Thirteen original containers were optimized through the public API, with
container structure, checksums, meaningful bits and advertised windows
independently checked.

| Container | Meaningful bits saved | Physical bytes saved |
| --- | ---: | ---: |
| PngSuite `basi0g04.png` | 1 | 0 |
| PngSuite `basi4a16.png` | 1 | 0 |
| APNG `clock.png` | 6 | 0 |
| ZIP `kskinmkr_src.zip` | 2 | 1 |
| Nine unchanged controls | 0 | 0 |
| **Total** | **10** | **1** |

The unchanged controls include GZIP and zlib references. Raw and wrapper
samples overlap and are not additive. Meaningful-bit totals come from parsing
outputs, because the public API reports removed bytes × 8 when a file shrinks.

## Max validation

A fixed-parent check covered 25 distinct Max references: ten recorded parents
from the payload-tradeoff validation and fifteen eligible streams from eight
current efficiency-review Max outputs with a 10-second optional allowance.
Two, both variants of `basi4a16`, improved by one bit each with no byte
change. Same-tree header repricing ties all 25 parents.

Three fresh Max A/B pairs also used a 10-second optional allowance; a
test-only gate disabled only R3 for the baseline.

| Fresh raw reference | Baseline bits | New bits | Outcome |
| --- | ---: | ---: | --- |
| `basi0g04` | 1,294 | 1,293 | Both reached the deadline; bytes tie |
| `Project1.vbp` ZIP member | 3,482 | 3,480 | Both reached the deadline; 436 → 435 bytes |
| Small 8x8 PNG stream | 347 | 347 | Both completed in about 5.6 seconds |

A fixed-parent check reproduced every fresh output byte for byte, and
same-tree repricing tied all three baselines, so the gains are not timing
effects. This is focused Max evidence, not a broad Max benchmark; not all
recorded parents were unrestricted fixed points.

## Runtime and acceptance

Old/new order alternated in batches of eight cases. Timings measure the public
API call, excluding compilation, process startup and file I/O.

| Workload | Baseline | New pass | Difference |
| --- | ---: | ---: | ---: |
| 365 raw streams | 75.787 s | 76.284 s | +0.497 s / +0.66% |
| 13 actual containers | 14.200 s | 14.159 s | −0.041 s / effectively neutral |

These are single paired-run observations. The isolated full probe took
0.91 seconds, including the rejected shortcuts, parsing, emission, validation
and I/O; the largest raw case used 44 alternative-span prices.

The gains are small and rare. The method is kept because the savings reproduce
beyond ordinary header repricing, include a physical byte in a real archive,
and cost little. HDIST search, joint span/swap products and alternative global
header solvers remain separate research.

## Tests and reproducibility

The release suite passes 504 tests: 445 library, 46 CLI and 13 public API tests.
New coverage includes the frozen one-bit witness, every legal advertised count
against an explicit oracle, the HLIT=286 limit, budget and stop limits, all
eight stored-block alignments and the exact PNG spelling in Max's
zero-optional-time Default floor.

Local evidence is in the ignored `work/literal-span-validation/` directory:
baseline hashes (`source-state.json`), full enumeration (`probe.csv`,
`moves.csv`, `spans.csv`), paired outputs, wrapper metrics, independent
checks, Max records and test logs. The helpers `work/run-literal-span-probe.py`,
`work/run-literal-span-paired.py`, `work/verify-literal-span-results.py` and
`work/prepare-literal-span-max.py` need the private fixture corpus. A first
paired attempt that linked a stale library is quarantined under
`stale-runner-check/` and contributes no measurements.
