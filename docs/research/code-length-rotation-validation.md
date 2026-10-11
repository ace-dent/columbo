<!-- SPDX-License-Identifier: MIT -->

# Three-symbol code-length rotations

Date: 10 September 2026. Baseline: `81effe5`, which includes the R7
code-length tree/RLE solver. **Accepted and implemented in Max only** as R8;
the [route catalog](../routes-and-methods.md#route-gate-reference) describes
the current method. Implements the three-symbol extension proposed in
[new byte-saving methods](new-byte-saving-methods.md#1-spend-payload-bits-on-better-code-length-permutations).

## Why a rotation can save bits

Rotating three code lengths within one alphabet can create a cheaper run of
equal lengths in the dynamic header that outweighs the extra payload cost. When
each pair swap that makes up the rotation ties or loses, a walk of improving
pair swaps cannot reach it.
[RFC 1951 §3.2.7](https://www.rfc-editor.org/info/rfc1951/)

The generated regression fixture contains 253 literal bytes. Its lengths at
symbols 4, 13 and 18, which occur 2, 3 and 6 times, change from `(7, 6, 5)` to
`(5, 7, 6)`. Payload cost rises from 1,111 to 1,116 bits while the complete
header falls from 150 to 144 bits, saving one meaningful bit: **1,261 →
1,260**. Every fully repriced pair swap ties or worsens the parent, and R7's
exact fixed-tree header solver cannot improve it.

## Bounds and instrumentation

[`header/rotate.rs`](../../src/deflate/header/rotate.rs) implements the
[R8 route row](../routes-and-methods.md#route-gate-reference). The ranking
score `payload_delta − 3 × transitions_removed` is a heuristic; every retained
proposal is fully repriced before it can replace the parent. The common gate
allows at most 1 MiB each of compressed and decoded data, 128 parsed blocks and
no discarded wire blocks.

The loose worst-case generation bound for one full literal/length and distance
alphabet is 11,628,422 units, so a single block fits the 2^24-unit allowance
unless a deadline interrupts it. On the measured 64-bit platform, the
318-entry length array, 286-entry symbol index array and 65-entry menu use
5,726 bytes of element storage.

Instrumentation across all 396 frozen-parent invocations records 595,342,147
generation units and 20,514 complete prices. Two streams exhaust generation
work, with ten failed reservations, and one other stream uses all 512 prices;
the largest charge is 16,777,215 units. A control without stream work or price
caps gives byte-identical outputs on all 396 parents, including the capped
cases.

## Controls and isolated gains

Fresh Default runs on all 365 distinct raw inputs reproduce the previous Default
outputs exactly. R7 is applied to these frozen parents first, so its savings
are not attributed to rotations.

The initial research generator, which trims advertised spans, finds 31 improving
streams and saves 185 bits / 20 bytes. The production pass keeps the parent's
spans and retains **30 improving streams, 184 bits and 20 bytes**; the missing
one-bit result came from the span policy.

A control on the research winners prices every nonzero pair swap in both
payload alphabets, skipping a pair only when its payload plus the 17 fixed
header bits cannot beat the incumbent. Across 54 dynamic blocks it prices
919,216 pairs in 134.8 seconds and reaches most rotation wins. Three rotation
results beat the best single pair: `sample_11-fs8.png` raw, one `happy.png`
frame, and the `mineplay.exe` member of `kwincheat.zip` keep 2, 5 and 1
additional bits.

A second control optimizes the CL tree/RLE exactly for each of the three pair
swaps in a rotation. In three examples none of those first steps improves the
parent, so two strictly improving pair swaps cannot reach the rotation; a longer
path is not ruled out.

| Parent | Parent bits | Pair AB | Pair AC | Pair BC | Rotation bits |
| --- | ---: | ---: | ---: | ---: | ---: |
| basn6a16 | 26,683 | 26,683 | 26,690 | 26,687 | 26,682 |
| 3d2.png frame | 41,670 | 41,670 | 41,670 | 41,680 | 41,660 |
| kwincheat.txt member | 20,490 | 20,497 | 20,491 | 20,494 | 20,489 |

Fresh ten-second Max runs on the 31 research witnesses provide frozen parents.
The production pass improves **29 of 31, saving 204 meaningful bits and 22
bytes**. The same-span control also solves each unchanged parent's CL tree/RLE
exactly, so all 184 Default-parent and 204 Max-parent bits come from rotations,
not from re-running R7.

The production pass takes 5.408 seconds over the 365 Default-plus-R7 parents and
1.704 seconds over the 31 Max parents, including parsing, emission and
validation, in separate single runs.

## Rejected span experiment

An oracle that jointly searched every legal literal span and the full CL
tree/RLE space found only two gains on the 365 Default-plus-R7 parents, three
bits and no bytes, both already found by full span repricing; it was not added.
For a nonempty distance alphabet, trimming dominates trailing zero distance
lengths; the empty distance case needs a separate analysis. The experiment
remains in `work/span-tree-validation/`.

## Public API and acceptance

R8 stays Max-only. Raw streams and their wrappers, and the strict and relaxed
samples, overlap; their gains are not additive.

All **404 Default outputs are byte-identical** to the baseline: 365 raw streams
and 39 containers. Timings are alternating single paired observations of
persistent API runners, excluding compilation, startup and file I/O.

| Default workload | Baseline | With R8 | Difference |
| --- | ---: | ---: | ---: |
| 365 raw streams | 87.813 s | 88.333 s | +0.6% |
| 39 containers | 58.054 s | 58.375 s | +0.6% |

Strict Max pairs grant ten seconds in both arms and alternate arm order; every
case reaches its timeout in both arms. Seven of 12 selected cases improve, with
no regressions:

| Input | Baseline bytes | With R8 | Meaningful bits saved |
| --- | ---: | ---: | ---: |
| basi4a16, raw | 2,746 | 2,745 | 12 |
| sample_11-fs8.png stream, raw | 1,782 | 1,782 | 0 |
| kwincheat.txt member, raw | 2,562 | 2,562 | 1 |
| 3d2.png frame, raw | 5,202 | 5,202 | 0 |
| basi4a16.png | 2,825 | 2,824 | 12 |
| basi6a16.png | 4,153 | 4,151 | 10 |
| sample_11-fs8.png | 2,744 | 2,744 | 0 |
| happy.png, APNG | 125,168 | 125,168 | 14 |
| kwincheat.zip | 17,480 | 17,480 | 0 |
| kzipmix-20200115-bsd.tar.gz | 52,679 | 52,679 | 3 |
| XYB.icc.zlib | 347 | 347 | 0 |
| clock.png, APNG | 23,107 | 23,107 | 1 |

The selected cases save 53 meaningful bits and four bytes. Timed differences can
include later search decisions; the frozen-parent evidence isolates the
operation.

Relaxed Max improves two of four selected cases with no regressions, both arms
timing out on all four: `basi4a16` raw saves 12 bits and one byte, and
`happy.png` 11 bits with equal bytes; the PNG `sample_11-fs8` and
`kwincheat.zip` tie.

The final Defluff relaxed benchmark passes **66/66 comparisons** (61 better,
five ties, no errors or parity misses; 109-byte / 932-bit lead unchanged). It
checks Default compatibility only. The report and state are in
`work/rotation-validation/public/defluff.md` and
`work/rotation-validation/defluff.json`.

## Checked-in tests and reproduction

All 538 Rust tests pass, including the private-corpus regressions; the new
tests cover pricing, ranking, inverse cycles, work and stop limits, all eight
incoming alignments and zero-budget Default parity. Every output was decoded
independently, with container CRCs, metadata and zlib windows checked. The
checked-in witness is generated from synthetic counts.

Local evidence is in `work/rotation-validation/`; `baseline-state.json` records
the baseline source. `work/run-rotation-validation.py` runs alternating
public-API pairs, and `work/verify-rotation-public.py` verifies raw and
container identities, CRCs, metadata and zlib windows. These helpers need the
private corpus.

The comparable stripped release executables grow from 1,728,048 to 1,744,560
bytes: 16,512 bytes. Builds use Rust 1.97.1 on macOS Apple Silicon with the
repository's release profile, fat LTO and overflow checks. Candidate binary
SHA-256: `e3bd1f722b68cd8f3892a90f2f78473c29bde815cddb76eacde273c65af73364`.
`public/source-hashes.json` records the benchmarked source.
