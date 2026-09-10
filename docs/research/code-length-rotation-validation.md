<!-- SPDX-License-Identifier: MIT -->

# Three-symbol code-length rotations

Date: 10 September 2026. Baseline: `81effe5`, including the committed R7
code-length tree/RLE solver and the subsequent test-layout cleanup.
**Accepted and implemented in Max only** as R8 in the
[route catalogue](../routes-and-methods.md#route-gate-reference), following the
[validation policy](future-deflate-methods.md#validation-policy).

This implements the three-symbol extension proposed in
[new byte-saving methods](new-byte-saving-methods.md#1-spend-payload-bits-on-better-code-length-permutations).
The contribution is additional search coverage within Columbo, not a claim
of worldwide novelty or a globally optimal Deflate stream.

## Why a rotation can save bits

A payload tree's code lengths are themselves compressed in the dynamic
header. Rotating three lengths can create a cheaper run of equal lengths.
The resulting header saving can exceed the extra payload cost. Individual
pair swaps may tie or lose, preventing an improving pair-swap walk from
reaching the same three-way assignment.

The generated regression fixture contains 253 literal bytes. Its lengths at
symbols 4, 13 and 18 change from `(7, 6, 5)` to `(5, 7, 6)`. Those symbols
occur 2, 3 and 6 times, respectively. Payload cost rises from 1,111 to 1,116
bits, while the complete header falls from 150 to 144 bits. The block saves
one meaningful bit: **1,261 → 1,260**. Every fully repriced pair swap ties or
worsens the parent, and R7's exact fixed-tree header solver cannot improve it.

The cycle keeps every positive length in the same alphabet. Its length
histogram, complete Kraft sum, support, maximum depth and advertised counts
are unchanged. Only the assignment of existing lengths to symbols changes.
Tokens, distances, decoded bytes and block boundaries remain fixed. The wire
format uses ordinary canonical Huffman trees and the existing repeat codes.
[RFC 1951 §3.2.7](https://www.rfc-editor.org/info/rfc1951/)

## Candidate generation and bounds

[`header/rotate.rs`](../../src/deflate/header/rotate.rs) considers both
orientations of triples with three distinct positive lengths. Equal lengths
would reduce the move to a pair swap; zero lengths would change support.
Literal/length and usable distance symbols are searched separately. Reserved
distance positions 30 and 31 are never rotated.

For the three changed positions, the payload delta is the sum of `frequency[s] × (new_length[s] − old_length[s])`. The implementation
computes it with signed integer arithmetic and checked total-bit updates.
The parsed parent supplies frequencies and the exact original header cost,
so candidate pricing does not rescan the token array.

The current bounded family admits payload deltas from −32 through +32 bits
and requires fewer adjacent length transitions. At most six edges change;
shared edges between adjacent positions are counted once, including the
LL/DD seam. The score `payload_delta − 3 × transitions_removed` ranks the
best 64 proposals per alphabet. It is a heuristic, not a bound or an
optimality claim. Every retained proposal receives full existing header/RLE
repricing before it can replace the parent.

One terminal invocation shares **2^24 generation-work units** and **512
complete header prices** across its stream. Charges cover the initial symbol
scan, outer loop visits and triple visits with both orientations. Each
orientation has at most six local edges. These are admitted-work units, not
CPU instructions. Even the loose worst-case bound for one full literal/length
and distance alphabet is only 11,628,422 units, so a single block fits the
generation allowance when no deadline interrupts it. Multi-block streams
share the allowance. No unbounded tree cache or path beam is introduced.

The fixed sequence has at most 318 entries and the symbol index array at
most 286. One menu holds at most 65 proposals during insertion. The existing
header pricer and the selected dynamic plan retain their separately bounded
vectors. On the measured 64-bit platform, the sequence, symbol index array
and 65-entry menu require 5,726 bytes of element storage in total. This
excludes compiler temporaries, vector metadata, allocator overhead, the
existing header pricer and retained plans.

Instrumentation across all 396 frozen-parent invocations records 595,342,147
generation units and 20,514 complete prices. Two streams exhaust generation
work, producing ten failed reservations; one other stream consumes the full
512-price allowance. The largest charge is 16,777,215 units. A separate
control completes generation and both 64-entry menus for every block without
stream work/price caps. It produces byte-identical outputs on all 396 parents,
including the capped cases. This validates the caps on this sample, not on
all possible inputs. There is no route-level cache; no cache-hit claim is
needed.

R8 follows R7 in Max, on a completed ordinary/APNG Default comparison
endpoint and on the final eligible Max incumbent. It uses the owner's
existing deadline or route window. The common gate admits at most 1 MiB each
of compressed and decoded data, 128 parsed blocks, and no discarded wire
blocks. Strict mode refuses a non-strict parent; relaxed distance exceptions
remain unchanged. The historical Max seed remains independent.

Every cycle is a sibling of the unchanged parent; the method performs no
replay. A stop retains all completed improvements. The common emitter checks
decoded identity, reprices later stored padding, propagates actual maximum
distances to wrappers, and accepts only a complete byte/meaningful-bit win.
Default receives no additional search or mandatory comparison-floor work.

## Controls and isolated gains

Fresh baseline Default runs on all 365 distinct raw inputs reproduce the
previous Default outputs exactly. Each output also passes independent Python
zlib decoding. For the first experiment, R7 is applied to these frozen
parents before testing rotations, so the results do not attribute R7's own
savings to the new method.

The initial research generator trims advertised spans, finds 31 improving
streams, and saves 185 bits / 20 bytes. The production pass preserves the
parent's spans: it retains **30 improving streams, 184 bits and 20 bytes**.
The omitted one-bit result came from the different span policy. These are
365 distinct parent encodings and 30 distinct winning parents.

Controls on the research winners price every single nonzero pair swap in
both payload alphabets. A pair is skipped only when its payload plus the
17 fixed header bits cannot beat the incumbent, even with a free tree
description. There is no payload-tax cutoff or transition heuristic in this
control. Across 54 dynamic blocks, it prices 919,216 pairs in 134.8 seconds.
Most initial rotation wins are also reachable by this much larger search.
Three rotation results beat the best single pair: `sample_11-fs8.png` raw,
one `happy.png` frame, and the `mineplay.exe` member of `kwincheat.zip` retain
2, 5 and 1 additional bits, respectively. This is eight bits beyond that
control, not another additive corpus total.

A separate control optimizes the CL tree/RLE exactly for each of the three
constituent pair swaps. It finds three examples where none of those first
steps improves the parent:

| Parent | Parent bits | Pair AB | Pair AC | Pair BC | Rotation bits |
| --- | ---: | ---: | ---: | ---: | ---: |
| basn6a16 | 26,683 | 26,683 | 26,690 | 26,687 | 26,682 |
| 3d2.png frame | 41,670 | 41,670 | 41,670 | 41,680 | 41,660 |
| kwincheat.txt member | 20,490 | 20,497 | 20,491 | 20,494 | 20,489 |

These establish a barrier for reaching the same rotation through two
strictly improving pair swaps. They do not rule out a longer path through
other assignments or a different token/tree search.

Fresh ten-second Max runs on the 31 research witnesses provide frozen
parents from the current baseline. The bounded production pass improves
**29 of 31, saving 204 meaningful bits and 22 bytes**. All 31 parents and the
29 winning parents are distinct encodings. These inputs overlap the first
sample, so the gains must not be added. Frozen-byte comparisons isolate the
method from timed route scheduling; independent zlib decoding and the common
emitter verify every output. The same-span control also solves the unchanged
parent's CL tree/RLE exactly: all 184 Default-parent bits and 204 Max-parent
bits remain additional to that control. No saving here comes merely from
re-running R7.

The production pass takes 5.408 seconds over the 365 Default-plus-R7 parents
and 1.704 seconds over the 31 Max parents, including parsing, emission and
validation. These are separate single-run observations, not general runtime
guarantees or public-API overhead estimates.

## Rejected span experiment

Before rotations, an oracle jointly searched every legal literal span and
the full CL tree/RLE space. For a nonempty distance alphabet, trailing zero
distance lengths are dominated by trimming: the terminal zero-run tokens
can be removed under the same CL tree without increasing cost. The empty
distance exception needs a separate analysis and is not covered by that
argument.

On all 365 Default-plus-R7 parents, the oracle finds only two gains totaling
three bits and no bytes. Existing full span repricing finds both. This did
not establish new joint-search coverage and was not added to production.
The experiment remains in `work/span-tree-validation/`.

## Public API and acceptance

The method is retained in Max only. Frozen-parent controls establish new
coverage, the public API retains whole-file wins under the existing Max
allowance, and Default performs no additional search.

All **404 Default outputs are byte-identical** to the current baseline:
365 raw streams and 39 containers. Independent checks verify decoded bytes,
PNG CRCs, APNG frame/control structure, ZIP contents and metadata, GZIP
integrity, and advertised zlib windows.

| Default workload | Baseline | With R8 | Difference |
| --- | ---: | ---: | ---: |
| 365 raw streams | 87.813 s | 88.333 s | +0.6% |
| 39 containers | 58.054 s | 58.375 s | +0.6% |

These are alternating, single paired observations of persistent API runners,
excluding compilation, startup and file I/O. They do not establish a general
speed guarantee.

Strict Max pairs grant ten seconds in both arms and alternate arm order.
Seven of 12 selected cases improve, with no regressions:

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

The selected cases save 53 meaningful bits and four bytes. Raw streams and
their wrappers overlap; this is not independent workload breadth. Every
strict Max case reaches its timeout in both arms. Timed differences can
include downstream search decisions after the improved comparison floor.
The frozen-parent evidence isolates the new operation separately.

Relaxed Max improves two of four selected cases: `basi4a16` raw saves 12 bits
and one byte, and `happy.png` saves 11 bits with equal bytes. The PNG
`sample_11-fs8` and `kwincheat.zip` tie. There are no regressions, and both
arms time out on all four. These inputs overlap the strict sample and their
gains must not be added. All meaningful-bit totals come from parsing the
emitted Deflate streams, including streams inside wrappers.

The final Defluff relaxed benchmark passes **66/66 comparisons**: 61 better,
five ties, no errors or parity misses. Its 109-byte / 932-bit lead is
unchanged. This is a Default compatibility check; it does not exercise R8
or attribute new Defluff savings to the Max-only search. The fresh report
and state are in `work/rotation-validation/public/defluff.md` and
`work/rotation-validation/defluff.json`.

## Checked-in tests and reproduction

All 538 Rust tests pass, including the private-corpus regressions. The six
new unit tests cover full pair-swap pricing on a generated witness, independent
exhaustive menu ranking, exact transition deltas, inverse cycles, signed
payload prices, work/price exhaustion, late callback stops, and actual emitted
relaxed empty/singleton distance trees. Two optimizer tests cover all eight
incoming alignments, later stored padding, cross-block history, unchanged
tokens and alphabet histograms, and zero-budget Default parity.

All-target/all-feature Clippy is warning-free. Formatting, diff checks and
the six Python distribution-tooling tests pass. No fixture bytes are embedded
in the new tests; the checked-in witness is generated from synthetic counts.

Local evidence is in `work/rotation-validation/`. `baseline-state.json`
records the baseline source and unrelated pre-existing documents. The initial
research generator, exhaustive pair control and exact intermediate control
live in separate temporary source copies. The production probe invokes the
integrated terminal pass. `work/run-rotation-validation.py` runs alternating
public-API pairs; `work/verify-rotation-public.py` independently verifies raw
and reconstructed container identities, CRCs, metadata and zlib windows.
These helpers need the private corpus; the checked-in tests are standalone.

The comparable stripped release executables grow from 1,728,048 to
1,744,560 bytes: 16,512 bytes. Builds use Rust 1.97.1 on macOS Apple Silicon
with the repository's release profile, fat LTO and overflow checks.
Candidate binary SHA-256:
`e3bd1f722b68cd8f3892a90f2f78473c29bde815cddb76eacde273c65af73364`.
`public/source-hashes.json` records the benchmarked source. Final audit
confirms the current source and executable match that snapshot. Instrumented
probe output matches the uninstrumented production pass on every parent;
all instrumentation remains outside production source.
