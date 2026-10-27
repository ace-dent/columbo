<!-- SPDX-License-Identifier: MIT -->

# Length-symbol exchange

13 September 2026. Baseline: `2a2df1f236b6d572ee757d6b41d72463ceea24b4`,
including header-directed match response and the existing terminal closure.
**Accepted and implemented in Max only** as
[R11](../routes-and-methods.md#header-directed-match-response-and-length-exchange-r10-r11),
whose entry gives the current method, gates and budgets.

## Why it helps

R10 changes positive code lengths but preserves symbol support. R11 moves a
used length symbol's code length to an absent length symbol and respells the
matches, so it is a joint support/token move. The new code need not be used: it
may enable a cheaper submatch, or keep the parent tree's Kraft capacity while
moving an unused code to a position that is cheaper to describe. Rebuilding a
frequency-optimal tree after removing a symbol does not reach these shapes.

One frozen Max witness, `cthn0g04.deflate`, has these exact costs:

| Encoding | Payload including EOB | Complete header | Complete stream |
| --- | ---: | ---: | ---: |
| Current Max parent | 409 | 272 | 681 bits / 86 bytes |
| Exchange | 421 | 258 | 679 bits / 85 bytes |

Its single use of length symbol 265 is removed, and that code length moves to
symbol 261, which stays unused. The payload pays twelve bits and the header
saves fourteen.

Of seven frozen Max wins, two use their new length code (six bits in total)
and five leave it unused (twelve bits). An earlier Default probe of filler
relocation alone found no improvements.

The generated regression fixture (seed 138, settled under R10, no corpus
bytes) decodes to 296 bytes. R10 cannot improve its 352-bit parent; the
exchange emits 351 bits.

## Search domain

The [implementation](../../src/deflate/header/response/exchange.rs) reuses the
[exact interval solver](../../src/deflate/header/response.rs). Only symbols
257–285 take part; every absent symbol is a recipient, even if no current match
can use it. Each pair is priced with the minimum legal literal span and with
the source's advertised span, if larger; this does not enumerate every legal
code-length tree or advertised span. The ranking bound is admissible for a
tree with its chosen generated header, not for other header encodings of that
tree.

## Isolated and frozen-parent controls

Across 365 completed Default streams, the bounded search improves eight,
saving 29 meaningful bits and two bytes. Absent literal recipients added no
wins in the initial probe and are not implemented.

Eight discovered cases received fresh ten-second baseline Max runs, none
timing out. Repeating R1–R10 on them without a deadline changes nothing. Seven
improve under the new pass, saving eighteen meaningful bits and one byte:

| Frozen Max source | Parent bits | Exchange bits | Recipient used? |
| --- | ---: | ---: | --- |
| `cthn0g04` | 681 | 679 | No |
| `ctjn0g04` | 720 | 718 | Yes |
| `filmreel.png`, first block | 1,885 | 1,881 | Yes |
| `107-Hitmonchan-0.png`, second block | 1,616 | 1,613 | No |
| `s32n3p04` | 872 | 869 | No |
| `samples.zip` / `samples/pg.exe` | 2,863 | 2,861 | No |
| `samples.zip` / `samples/rnd_arr1.exe` | 3,691 | 3,689 | No |

For the two multi-block entries the table gives the changed block's cost;
their stream savings are four and three bits. These Max cases overlap the
Default sample and are not additive to it.

A broader control enumerates all absent length recipients and both candidate
spans with unlimited work and price budgets and no interval bound; only the
exact payload plus the seventeen fixed header bits may prune a full header
price. It reaches the same best scores on all eight frozen Max parents with
1,300 full header prices. The bounded pass uses 1,306 prices and 38,050,096
charged work units in aggregate; one stream exhausts work, none exhausts
prices, and all seven improvements remain. Python zlib decodes every isolated
stream.

On the broader Default parents, the pass charges 594,906,077 work units and
23,944 full header prices; three streams exhaust work, none exhaust prices,
and the largest price count is 371. Isolated time, including parsing,
emission and verification, is 6.996 seconds in total (maximum 176.372 ms),
and 0.387 seconds over the Max parents (maximum 159.829 ms). These are not
public-API overhead estimates.

## Public validation

The frozen source and binary were compared with the committed baseline in
alternating order, with identical options and time allowances and no other
work running.

| Public comparison | Cases | Result |
| --- | ---: | --- |
| Default raw | 365 | All output bytes identical; 103.869 → 101.103 s total (-2.66%) |
| Default wrappers | 39 | All output bytes identical; 68.616 → 65.332 s total (-4.79%) |
| Strict Max, 10 s | 24 | 11 improvements, 13 ties, no regressions; timeouts 6 baseline / 8 candidate |
| Relaxed Max, 10 s | 4 | 0 improvements, 4 ties, no regressions; timeouts 3 baseline / 3 candidate |
| Defluff compatibility | 66 | 61 better, 5 ties, no errors, missing cases or misses |

Strict Max takes 222.612 → 224.608 seconds in total, about 0.90% more. Two
ZIP-member cases newly report timeout: `864b0ea02721ef82` (9.858 → 10.025
seconds, saves two bits) and `c1eea5d0f9da6649` (9.942 → 10.066 seconds,
ties). Both still return complete valid results under the unchanged
ten-second allowance and grace. Default's timing differences are not a speedup
attributable to this Max-only search.

Every output decodes independently, with wrapper structure, checksums,
metadata and window bounds checked. Public Max gains can include later
refinements by existing methods, and raw streams overlap their containers, so
these totals do not add to the frozen-parent gains. The Defluff totals remain
109 bytes and 932 meaningful bits smaller than its references: a Default
compatibility check, not an R11 gain.

The source passes 583 Rust tests, ignored tests included, and all 13 Python
tests. Focused tests include exhaustive tiny spelling oracles, interval-bound
admissibility, complete unpruned exchange enumeration and all eight incoming
bit offsets.

The release executable grows from 1,777,760 to 1,794,288 bytes, an increase of
16,528 bytes. Candidate SHA-256:
`11590cb41ebf0ea2cf321077fe2d0c9909f5fbf4863bd8b921eb51e5513906fc`.

Scripts, probes, original-input manifests, frozen binaries, source hashes,
CSVs and outputs are kept under `work/support-exchange-validation/`.
