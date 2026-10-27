<!-- SPDX-License-Identifier: MIT -->

# Fixed-tree boundary slide: validation

Date: 2 October 2026. Baseline: `a2a4c7a`. **Accepted and implemented** as R1c;
the [route catalog](../routes-and-methods.md#fixed-tree-boundary-slide-r1c)
describes the current method. It came out of testing length-threshold cuts
inside matches, rejected below.

## Why it saves bits

A block boundary changes neither the decoded bytes nor the 32 KiB window, so a
token can move to the neighboring block whenever that block's trees can code
it. With both trees fixed, the headers are unchanged and a cheaper cut saves
payload bits outright.
[RFC 1951 §3.2.3](https://www.rfc-editor.org/rfc/rfc1951#section-3.2.3)
No earlier route priced every token cut against the selected trees. On
`PngSuite.png`, Default output fell from 2,178 to 2,142 bytes and Max output
from 2,128 to 2,118.

## Method

[`slide.rs`](../../src/deflate/slide.rs) implements the R1c route row. The pass
is linear in parsed tokens and parses one candidate, so it needs no size class.
A 1 MiB class would have excluded 727 of the first 2,246 predicted Default
bytes, all from PNG streams of 1.2–3.2 MB decoded.

### Placement

A slide fitted to the current trees can remove an improvement that a later
tree-changing method would find. Placed after R1b, it made
`ODF_textdocument_128x128.png` Max output 10 bytes larger and `psydk-Pink.png`
2 bytes larger: in the first, the slid Default endpoint became the parent of
Max's terminal tree floors, and the bounded-depth floor lost the 5 bytes it
found on the unslid endpoint. Default therefore runs R1c after R5, Max only
after an R1–R11 sweep that changed nothing, and Max's slid Default endpoint is
only a final competitor. With this placement both files improve: 6,744 → 6,742
and 4,318 → 4,317 bytes. (R13 and R14 were added after R1c later.)

## Rejected: cuts inside matches at length-code thresholds

The proposal derived inside-match cuts from `LENGTH_BASE` transitions
(`a = base, base − 1, n − base, n − base + 1`, plus one- and two-byte
tails). Those cuts do cover every canonical pair of fragment signatures for all
lengths 3–258, at most 59 per length. But a fixed-tree scan of 518 completed
Default PNG outputs, pricing every position at each Huffman boundary, found
only 122 of 1,011 boundaries inside a match. Inside-match positions added 12
bits beyond the best token cut across all outputs, and none on 283 Max streams;
token cuts alone found 17,967 bits.

## Measurements

Paired A/B runs alternate the arms per input and compare complete files. Every
PNG output was decoded independently, and all 40 GZIP and ZIP outputs with
Python's `gzip` and `zipfile`.

| Set | Files | Smaller / larger | Bytes saved | CPU |
| --- | ---: | --- | ---: | --- |
| Default PNG: PngSuite, small, imageworsener, pkmn-col, medium | 518 | 108 / 0 | 3,561 | 521.26 → 522.82 s |
| Default PNG, large group | 7 | 3 / 0 | 1,117 | 42.91 → 43.78 s |
| Default GZIP and ZIP sample | 40 | 35 / 0 | 8,629 | 312.15 → 313.29 s |
| Max, 12 medium PNGs, 60 s | 12 | 8 / 0 | 485 | 1,694.82 → 1,704.98 s |
| Max, 61 multi-block small PNGs, 30 s | 61 | 11 / 0 | 22 | 3,980.08 → 4,004.83 s |

The 502 Default outputs covered by the offline fixed-tree scan saved exactly the
predicted bytes, including streams above 1 MiB decoded; the 16 files it missed
saved another 1,315 bytes. `Partnership_Card___John_Lewis_Finance.png` (9.6 MB
decoded) saved 1,022 bytes, and its Default CPU rose from 29.77 to 30.50 s, the
largest relative cost measured. GZIP gains come mostly from large multi-block
tar archives, for example 1,922 bytes on
`kzipmix-20200115-linux-static.tar.gz`.

Four Max medium PNGs above 1 MiB decoded did not change: their primary routes
use the whole 60-second allowance, so G0 starts no terminal method. R1c now
starts until the hard stop, within G0t's
[finalization share](../benchmark-miss-regression-audit.md#linear-finalization-share-on-8-october-2026).

## Validation

- `two_block_slides_match_an_exhaustive_cut_oracle` compares 300 pseudo-random
  two-block streams against every feasible cut; more than 30 slide. Each result
  reparses with identical trees, nonempty blocks, decoded bytes and planned bit
  count.
- Unit tests cover literal-run cuts, stored boundaries, an expired stop and the
  terminal dispatch in Default and Max; all tests, `cargo fmt --check` and
  `cargo clippy --all-targets -- -D warnings` pass.
- The release executable grows from 1,794,320 to 1,810,848 bytes; `__text`
  grows by 13,304 bytes, about 7.6 KiB of it the slide and the rest the
  final-competitor plumbing.

Each move is optimal for its pair given the neighboring blocks, not a joint
optimum over all boundaries. Merging a block into a neighbor would also save a
header, but none of the measured slides emptied a block.

## Follow-up: respelling joining matches

Date: 2 October 2026. Baseline: `bf2fe12`, the first R1c. A fixed-tree scan of
its 518 Default PNG outputs found no remaining token-cut gain. Cuts were often
held back by a match the neighboring block cannot code; spelling such a match
as its decoded literals would have saved another 14,553 bits, while cuts inside
matches added only 19.

A joining match now costs the cheaper of its own spelling and its literals; a
tie keeps the match. Shorter matches at the same distance are not tried. The
two-block oracle applies this rule by brute force and agrees on all 300
streams; 214 slide, and 90 of those respell a match.

| Set | Files | Smaller / larger | Bytes saved vs `bf2fe12` | CPU |
| --- | ---: | --- | ---: | --- |
| Default PNG: PngSuite, small, imageworsener, pkmn-col, medium | 518 | 32 / 0 | 1,875 | 535.95 → 538.41 s |
| Default PNG, large group | 7 | 2 / 0 | 1,333 | 43.67 → 44.47 s |
| Default GZIP and ZIP sample | 40 | 18 / 0 | 939 | 313.96 → 316.29 s |
| Max, 12 medium PNGs, 60 s | 12 | 2 / 0 | 80 | 1,696.06 → 1,701.51 s |
| Max, 61 multi-block small PNGs, 30 s | 61 | 1 / 0 | 1 | 3,988.16 → 3,996.97 s |

Gains concentrate where neighboring blocks use different match alphabets:
`nerd.png` saved 1,205 bytes, `download_webp__260×280_.png` 837 and
`floor pattern.png` 595. In Max, `FsqwhPuaIAIlojU.png` returns the slid Default
result, which beats its deadline-limited Max search by 15 bytes. Every PNG
output and all 40 GZIP and ZIP outputs decoded identically. `__text` grows by
3,432 bytes; the executable stays at 1,810,848 bytes.

## Follow-up: re-planning moved blocks

Date: 6 October 2026. Baseline: `120a21f`. Moved blocks kept trees fitted to
their old contents. Each moved block is now re-planned with `plan_block`,
keeping the cheaper of the transmitted and re-planned codes (a tie keeps the
transmitted header).

On the 518-file PNG set, the remaining fixed-tree headroom after `120a21f` was
zero for token cuts and literal respelling. Respelling joining matches as
same-distance submatches would have added only 453 bits, so that extension was
not taken.

| Set | Files | Smaller / larger | Bytes saved vs `120a21f` | CPU |
| --- | ---: | --- | ---: | --- |
| Default PNG: PngSuite, small, imageworsener, pkmn-col, medium | 518 | 34 / 0 | 1,204 | 559.30 → 566.02 s |
| Default PNG, large group | 7 | 3 / 0 | 478 | 46.39 → 47.88 s |
| Default GZIP and ZIP sample | 40 | 19 / 0 | 5,903 | 324.46 → 324.65 s |
| Max, 12 medium PNGs, 60 s | 12 | 4 / 0 | 367 | 1,686.44 → 1,687.38 s |
| Max, 61 multi-block small PNGs, 30 s | 61 | 1 / 0 | 2 | 3,745.20 → 3,758.04 s |

The largest PNG gains were `Partnership_Card___John_Lewis_Finance.png`
(476 bytes), `FsqwhPuaIAIlojU.png` (343, in Max through the slid Default result)
and `4.2.07.PNG` (260). Repeated isolated runs put the first's CPU cost at about
0.55 s on 30.5 s; `y2rc2_large.png`, the largest increase in the batch, timed
identically in isolation. The three copies of
`kzipmix-20200115-linux-static.tar.gz` each saved 1,124–1,403 bytes. Every PNG
output and all 40 GZIP and ZIP outputs decoded identically. `__text` grows by
1,244 bytes, crossing a 16 KiB page: the executable grows from 1,810,848 to
1,827,360 bytes.

## Follow-up: sliding to a fixed point

Date: 8 October 2026. Baseline: `7212d6b`. Re-planned trees can make further
cuts cheaper, so one R1c invocation did not reach a fixed point. A control arm
in the distance-alphabet ladder experiment, which slid a second time after R1c,
found 1 byte on 365 raw streams, 503 on 1,052 Default PNGs, 2,145 on 202
container files and 2,004 in a 60-second Max sample, with no output larger.

The slide now repeats rounds while a re-plan takes a strictly cheaper code, up
to 16 rounds; the probe below never needed more than 5. Rounds work on the plan
and emit once.

### Round counts

An instrumented copy with a 1,000-round cap recorded every invocation in Default
runs. Zero rounds means no cheaper cut was found.

| Set | Invocations | Rounds: 0 / 1 / 2 / 3 / 4 / 5 | Planned bits after round 1 |
| --- | ---: | --- | ---: |
| Raw streams | 59 | 32 / 26 / 1 / 0 / 0 / 0 | 4 |
| PNG | 540 | 134 / 347 / 45 / 12 / 2 / 0 | 4,365 |
| GZIP, zlib, ZIP and APNG | 981 | 555 / 404 / 13 / 5 / 2 / 2 | 25,181 |
| Relaxed: raw streams and 16 PNG | 74 | 32 / 28 / 6 / 7 / 1 / 0 | 1,678 |

The most rounds came from `kzipmix-20200115-linux-static.tar.gz`: five rounds
over 175 blocks, 7,004 bits after the first. Among PNGs, `FsqwhPuaIAIlojU.png`
needed four rounds for 662 bits.

### Measurements

Paired runs alternate five arms per input, in four lanes for Default and two for
Max: the baseline; the control above, which slides twice; this change; and the
distance-ladder prototype with and without this change. The sets are those of
the distance-ladder validation.

| Set | Files | Smaller / larger | Bytes saved | Beyond the control: smaller / larger, bytes | CPU: baseline → candidate (control) |
| --- | ---: | --- | ---: | --- | --- |
| Raw streams | 365 | 1 / 0 | 1 | 0 / 0, 0 | 100.91 → 101.32 s (100.44) |
| PNG: medium, large, small, imageworsener, pkmn-col | 1,052 | 54 / 0 | 549 | 10 / 0, 46 | 1,553.55 → 1,557.16 s (1,555.52) |
| GZIP, zlib, ZIP and APNG | 202 | 17 / 0 | 3,145 | 7 / 0, 1,000 | 919.05 → 929.45 s (918.75) |
| Relaxed: raw streams and 16 PNG | 381 | 11 / 0 | 210 | 6 / 0, 33 | 218.69 → 219.08 s (220.25) |
| Max, 60 s, 25 PNG | 25 | 13 / 3 | 589 net | 5 / 3, −147 net | 3,104.44 → 3,138.23 s (3,138.97) |

No Default output grew against the baseline or the control; every arm rejects
two ZIP layouts. The control's second slide reaches the fixed point on most
files; further rounds matter on large multi-block streams. On
`kzipmix-20200115-linux-static.tar.gz` the candidate saves 875 bytes and the
control 535; on `FsqwhPuaIAIlojU.png`, 83 against 60.

**Max.** Results depend on timing: 24 of the 25 runs used their whole allowance.
In the five-arm run the candidate saved 668 bytes on 13 files, lost 79 on three,
and trailed the control on three. Those files and `FsqwhPuaIAIlojU.png` were run
three more times per arm, in the same two lanes:

| File | Baseline | Control | Candidate | Five-arm run: baseline, control, candidate |
| --- | --- | --- | --- | --- |
| `floor pattern-deflopt.png` | 519,838 ×3 | 519,834 ×3 | 519,834 ×3 | 519,873, 519,840, 519,945 |
| `Partnership_Card___John_Lewis_Finance-deft4j-t139s.png` | 2,751,732–2,752,397 | 2,751,009–2,751,010 | 2,750,981–2,751,007 | 2,751,089, 2,751,010, 2,751,089 |
| `4.2.03.PNG` | 748,571 ×2, 748,614 | 748,571 ×2, 748,614 | 748,570 ×2, 748,614 | 748,623, 748,618, 748,624 |
| `nascar.png` | 22,761 ×2, 22,767 | 22,761, 22,767 ×2 | 22,767 ×3 | 22,761, 22,767, 22,767 |
| `FsqwhPuaIAIlojU.png` | 544,139 ×3 | 543,640 ×3 | 543,613 ×2, 543,614 | 544,145, 543,640, 543,614 |

The losses do not repeat. `nascar.png` and `4.2.03.PNG` take one of the same two
sizes in every arm; on the other three files the candidate is never larger than
the control in a repeat. The steady Max gain is `FsqwhPuaIAIlojU.png`, 525–526
bytes. Max process CPU follows the allowance, not the method.

**Cost.** Default CPU rises 0.2–1.1% across the sets. Timed alone, three runs
each, `kzipmix-20200115-linux-static.tar.gz` takes 9.68 → 10.32 s (control
9.98 s), 6.6% for its five rounds over 175 blocks, and the 3.2 MB
`Partnership_Card___John_Lewis_Finance.png` 30.71 → 31.06 s (control 31.20 s):
the extra rounds cost less than the control's second emission and parse.

### Validation

- `iterated_slide_equals_repeated_slides_and_reaches_a_fixed_point` generates
  400 streams of two to four Huffman blocks, some with a stored block between
  them, in both strict modes. The iterated slide must emit exactly the bytes of
  repeated single rounds, and sliding its result again must find nothing. 281
  streams slide and 21 need more than one round; with the round cap set to one
  the test fails.
- The terminal dispatch test checks that sliding the accepted candidate again,
  in Default and Max, finds nothing.
- All tests pass in debug and release, including the private-corpus
  regressions; `cargo fmt --check` and
  `cargo clippy --all-targets --all-features -- -D warnings` pass.
- Python's `zlib`, `gzip` and `zipfile` decoded every output of all five arms
  against its source, including PNG chunk CRCs and APNG frames: 1,825 raw, 5,260
  PNG, 970 container, 1,905 relaxed and 125 Max outputs, with no mismatch. The
  eight container sources Python cannot decode give identical outputs in every
  arm.
- `__text` grows by 2,156 bytes; the release executable stays at 1,827,360
  bytes.

### Coordination with the distance-alphabet ladder

The distance-alphabet ladder, R13, was merged after this change; its
[rebased measurements](distance-ladder-validation.md#rebased-onto-the-fixed-point-slide)
come from the same runs. The two prototypes compose as follows:

| Set | Ladder → ladder with this change: smaller / larger, bytes | This change → ladder with this change: smaller / larger, bytes |
| --- | --- | --- |
| Raw streams | 0 / 0, 0 | 1 / 0, 1 |
| PNG | 31 / 7, +138 −7 | 50 / 0, 475,932 |
| GZIP, zlib, ZIP and APNG | 14 / 0, 1,051 | 26 / 0, 205 |
| Relaxed | 5 / 1, +44 −2 | 16 / 0, 95,050 |

Against the ladder alone, this change makes seven PNG files one byte larger and
one relaxed file two bytes larger: the ladder chooses kept symbols for each
block where the slide left its boundaries. On `4.2.03.PNG`, after the
fixed-point slide moved 1,471 decoded bytes from block 22 into block 23, the
ladder kept different distance symbols in both blocks, which ended 13 bits
larger and crossed a byte boundary. With this change as the baseline, the ladder
makes no Default output larger, and its container gain falls from 2,299 to 205
bytes, since most of it came from the slide after a ladder win. Max was not
repeated for the ladder arms.
