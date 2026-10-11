<!-- SPDX-License-Identifier: MIT -->

# Header-directed certified match response

Date: 13 September 2026. Baseline: `bf3c20fc2af5b190551bac3ec29a080153c096c1`, including
terminal-method closure and its larger Max size class. Route numbers below are those of
this snapshot, in which terminal closure was R11 with ten input-score slots; closure is
now R12, with one slot per terminal method, and length-symbol exchange is R11.

**Accepted and implemented in Max only** as R10, with the deadline variation documented
below; the [route catalog](../routes-and-methods.md#route-gate-reference) describes the
current method.

## Why the joint move helps

Changing a payload tree can make the current tokens larger while making a different
spelling of the same certified matches cheaper; neither intermediate has to win. This is a
bounded tree-first experiment motivated by [proposal
4](novel-deflate-formulations.md#4-synthesize-short-headers-first-then-fit-certified-spellings-to-them),
limited to the parent tree and single nonzero code-length swaps within either payload
alphabet.

The checked-in synthetic fixture is generated and decodes to 276 bytes. Swapping
literal/length positions 7 and 267 and adapting matches gives:

| Encoding | Complete block bits |
| --- | ---: |
| Parent | 438 |
| Swap alone | 438 |
| Winning spelling under the parent tree | 440 |
| Cheapest certified response under the parent tree | 438 |
| Swap and response together | **437** |

## Exactness and limits

[`header/response.rs`](../../src/deflate/header/response.rs) implements the
[R10 route row](../routes-and-methods.md#route-gate-reference). For a fixed proposed tree
and the disjoint match intervals, its per-match DP gives the exact minimum payload cost.
It proves no optimum over other trees, certificates, token ties, layouts or header-search
families, and the header uses the existing full repricer, not an exact CL-tree search.
The 128-bit tax limit and the transition filter are heuristics: a response might repay a
larger tax. The pass does not recover certificates lost by earlier literalization; R1
handles restoration.

## Frozen-parent controls

Fresh baseline Default outputs were generated for 365 distinct raw streams. The
exploratory 256-profile solver, before stream work caps and unchanged-spelling exclusion,
improves 23 streams by 47 meaningful bits and four bytes. The production pass improves
**19/365, by 41 bits and four bytes**: four tree-only wins are excluded because the pass
skips unchanged spellings, and the shared work cap loses one more bit on a medium ZIP
member.

Seven targeted source streams received fresh ten-second baseline Max runs, all completing
without a reported timeout. Rerunning the R1–R9 terminal closure without a deadline
changes none of them. The response pass improves **five of these seven frozen Max outputs
by 21 bits and four bytes**; these overlap the Default sample and are not additive.

A control enumerating every unequal positive single swap in both alphabets on the five
winning blocks, pruned only by the admissible 17-bit fixed-header lower bound, performs
**7,804 full header prices** and finds no improvement. The production candidate beats the
exact unchanged-tree response on two blocks:

| Frozen Max block | Parent / best single swap | Unchanged-tree response | Joint response |
| --- | ---: | ---: | ---: |
| `sample-project.zip` member `351f0b74d9a2036d` | 2,681 | 2,681 | **2,679** |
| `013-Weedle-1.png`, block 1 | 1,497 | 1,495 | **1,494** |

For the ZIP witness, exchanging lengths at positions 112 and 257 alone costs 2,684 bits,
and its selected new spelling under the old tree 2,682; together they cost 2,679. Broader
fixed-token enumeration uses 933 header prices on this block, against 55 complete
response prices on its stream in production. The other three frozen Max improvements come
from an exact response under the unchanged tree, so the 21-bit total is not all due to
the joint move; the two interaction witnesses keep three bits beyond the unchanged-tree
response.

Every isolated result and the synthetic fixture are independently decoded with Python
zlib, and each generated match's interval, distance and length encoding is checked.

Instrumentation records 1,677,881,582 charged work units and 2,760 full prices across the
365 Default parents. Of 153 streams that enter generation, 17 exhaust the shared work
limit and none the price limit; the largest price count is 256. The isolated pass takes
3.704 seconds in total, with a 230.201 ms maximum, including parsing, emission and
verification. On the seven Max parents it takes 0.397 seconds, maximum 142.252 ms; two
exhaust work, none exhausts prices. The caps preserve all five observed frozen Max wins.

## Placement and timing control

An initial integration also ran on early Max comparison floors before R1–R9 had settled.
The retained placement follows the settled endpoint, so a new spelling cannot redirect an
unfinished search; all five frozen Max witnesses remain in scope.

The first 26-case paired strict-Max comparison had nine improvements and one one-byte
regression on `briefcase.png` (2,342 versus 2,343 bytes). A repeat reversed it: the
unchanged baseline emitted 2,343 bytes and the initial candidate 2,342, both without a
reported timeout. The pass found no candidate on that PNG, whose one 65,664-byte,
1,365-token block exceeds the per-block limits; the variation comes from the timed
schedule, whose primary phase can yield before the file deadline without reporting a
timeout. A matched 30-second-allowance run produces identical 2,343-byte files from both
binaries (24.9 seconds each, 17,937 Deflate bits). The row is recorded as a timed
comparison exception, not as non-regressing.

## Public-API, wrapper and regression validation

The runner alternates baseline/candidate order for each input with the same options and
allowance, with nothing else running.

| Comparison | Cases | Final result |
| --- | ---: | --- |
| Default raw | 365 | All output bytes identical; 94.041 → 94.789 s total (+0.80%) |
| Default wrappers | 39 | All output bytes identical; 61.455 → 61.858 s total (+0.65%) |
| Strict Max, 10 s | 26 | Seven improvements, 18 ties, one reproduced baseline deadline-variation row; eight timeouts in each arm |
| Relaxed Max, 10 s | 4 | All four tie; three timeouts in each arm |
| Defluff compatibility | 66 | 61 better, five ties, no missing cases, errors or misses |

The strict Max improvements include five target raw streams, `s35i3p04.png` and
`XYB.icc.zlib`. The raw ZIP member `351f0b74d9a2036d` reaches 2,670 bits after later
refinement by existing methods, against 2,681 at baseline; the isolated response alone
saves two bits on its frozen parent. Container and extracted-raw cases overlap and are
not additive. All outputs decode to their source contents, with wrapper structure,
checksums, metadata and zlib windows checked.

Defluff's totals remain **109 bytes and 932 meaningful bits smaller** than its
references; this is Default compatibility evidence, not an R10 gain. Results carry the
final binary SHA-256 `f1ab6e09aa790df75382daf5cbe0e055d05169e37c8a0d9fd320748a7e47faad`.
The release executable grows from 1,761,232 to 1,777,760 bytes: **16,528 bytes**.

The Rust suite passes 577 tests with ignored tests included; all 13 Python tests pass.
Focused tests compare the interval DP against exhaustive tiny spellings and cover the
synthetic interaction barrier, source ties, canonical length 258, work/price/stop limits
and all eight incoming bit alignments.

Reproduction scripts, CSVs, baseline/candidate binaries and hashes are under
`work/header-response-validation/`; the source tests use a generated fixture and need no
private corpus.
