<!-- SPDX-License-Identifier: MIT -->

# Fixed-tree boundary slide: validation

Date: 2 October 2026. Baseline: `a2a4c7a`. **Accepted and implemented** as
R1c in the [route catalogue](../routes-and-methods.md#route-gate-reference).
It came out of testing length-threshold cuts inside matches, which are
rejected below. It adds a search dimension to Columbo; worldwide novelty is
not claimed.

## Why it saves bits

A Deflate block boundary changes neither the decoded bytes nor the 32 KiB
history window, so a token can move to the neighbouring block whenever that
block's trees can code it. If both blocks keep their transmitted trees, every
header is unchanged and the move costs exactly the difference between the
token's code lengths in the two trees. A cheaper cut therefore saves payload
bits outright. [RFC 1951 §3.2.3](https://www.rfc-editor.org/rfc/rfc1951#section-3.2.3)

Columbo's other boundary routes plan fresh trees for a few sampled cuts:
encoder flush points, decoded eighths, 32-token anchors, histogram and
alphabet transitions. None prices every token cut against the trees actually
selected, and R1–R5 keep boundaries fixed. On `PngSuite.png`, Default output
fell from 2,178 to 2,142 bytes and Max output from 2,128 to 2,118.

## Method

[`slide.rs`](../../src/deflate/slide.rs) visits each boundary between two
non-stored blocks. The feasible cuts are those leaving every token on a side
whose trees code it, with both blocks nonempty: an emptied block would be
discarded by the parser, which later terminal methods refuse. One prefix/suffix
pass prices every feasible cut, keeping the current cut on a tie and otherwise
the nearest cheapest one. Sweeps repeat, at most 16, while the payload strictly
falls; each accepted move lowers it, so they terminate. Unchanged blocks keep
their original bits; moved blocks keep their transmitted header with an
adjusted bit count. Stored blocks and their boundaries never move. The caller
emits and validates the complete stream and accepts only a byte/meaningful-bit
win, with stored padding repriced.

The pass is linear in parsed tokens. Its one candidate parse costs no more
than a replay round, which a candidate may already pay at any size, so R1c has
no size or block-count class. A 1 MiB class excluded 727 of the first 2,246
predicted Default bytes, all from PNG streams of 1.2–3.2 MB decoded.

### Placement

R1c ends the terminal sweep. A slide fitted to the current trees can remove an
improvement a later tree-changing method would find from the unslid
boundaries. Placed after R1b, it deterministically made
`ODF_textdocument_128x128.png` Max output 10 bytes larger and `psydk-Pink.png`
2 bytes larger. In the first, Max's complete Default endpoint gained one byte
from the slide and became the parent of Max's terminal tree floors; the
bounded-depth floor then no longer found the 5 bytes it found on the unslid
endpoint. Therefore:

- Default runs R1c once, after R5. Nothing follows it.
- Max runs R1c only after an R1–R11 sweep that changed nothing, as R10 and R11
  wait for earlier methods; R12 repeats the sweep after a win.
- Max's mandatory Default endpoints keep the slid result only as a final
  competitor, compared after Max's terminal searches. Max still never trails
  Default, and the slid endpoint never chooses Max's terminal parent.

With this placement both files improve: 6,744 → 6,742 and 4,318 → 4,317 bytes.

## Rejected: cuts inside matches at length-code thresholds

The investigated proposal derived inside-match cuts from `LENGTH_BASE`
transitions (`a = base, base − 1, n − base, n − base + 1`, plus one- and
two-byte tails). Its structural claim reproduces: those cuts cover every
canonical pair of fragment signatures for all lengths 3–258, at most 59 per
length.

A fixed-tree scan of 518 completed Default PNG outputs priced every position at
each Huffman boundary, inside matches included. Only 122 of 1,011 boundaries
fell inside a match. Inside-match positions added 12 bits beyond the best
token cut across all outputs, and none on 283 Max streams. The same scan found
17,967 bits from token cuts alone, which led to this method.

## Measurements

Paired A/B runs alternate the baseline and candidate per input and compare
complete files. Every PNG output was decoded independently against its source,
and all 40 GZIP and ZIP outputs with Python's `gzip` and `zipfile`.

| Set | Files | Smaller / larger | Bytes saved | CPU |
| --- | ---: | --- | ---: | --- |
| Default PNG: PngSuite, small, imageworsener, pkmn-col, medium | 518 | 108 / 0 | 3,561 | 521.26 → 522.82 s |
| Default PNG, large group | 7 | 3 / 0 | 1,117 | 42.91 → 43.78 s |
| Default GZIP and ZIP sample | 40 | 35 / 0 | 8,629 | 312.15 → 313.29 s |
| Max, 12 medium PNGs, 60 s | 12 | 8 / 0 | 485 | 1,694.82 → 1,704.98 s |
| Max, 61 multi-block small PNGs, 30 s | 61 | 11 / 0 | 22 | 3,980.08 → 4,004.83 s |

All 502 Default outputs covered by the offline fixed-tree scan of the baseline
saved exactly the bytes it predicted, including streams above 1 MiB decoded;
the 16 files it missed saved another 1,315 bytes. The large group includes
`Partnership_Card___John_Lewis_Finance.png` (9.6 MB decoded), which saved
1,022 bytes; its Default CPU rose from 29.77 to 30.50 s, the largest relative
cost measured. GZIP gains come mostly from large multi-block tar archives, for
example 1,922 bytes on `kzipmix-20200115-linux-static.tar.gz`.

In Max, four medium PNGs above 1 MiB decoded did not change: their primary
routes use the whole 60-second allowance, so G0 admits no terminal method.
R1c follows that existing rule rather than starting work after the deadline.

## Validation

- `two_block_slides_match_an_exhaustive_cut_oracle` compares 300 pseudo-random
  two-block streams with literals and matches against every feasible cut;
  more than 30 exercise a slide. Each emitted result reparses with identical
  trees, nonempty blocks, decoded bytes and planned bit count.
- Further unit tests cover an exact literal-run cut and its fixed point,
  stored boundaries, an expired stop, and the terminal dispatch in Default and
  Max.
- All library, binary, CLI, public API and private-corpus tests pass in debug
  and release. `cargo fmt --check` and `cargo clippy --all-targets -- -D
  warnings` pass.
- The release executable grows from 1,794,320 to 1,810,848 bytes; `__text`
  grows by 13,304 bytes, about 7.6 KiB of it the slide itself and the rest
  the final-competitor plumbing.

## Limits

The pass is greedy per boundary: each move is optimal for its pair given the
neighbouring blocks, not a joint optimum over all boundaries, and its trees are
fixed. It never cuts inside a match, moves a stored boundary, or removes a
block. Merging a block into a neighbour whose trees can code it, which would
also save a header, is a possible extension; none of the measured slides
emptied a block.

## Follow-up: respelling joining matches

Date: 2 October 2026. Baseline: `bf2fe12`, the first R1c. A fixed-tree scan of
its 518 Default PNG outputs found no remaining token-cut gain, confirming that
fixed point. Its cuts were often held back by a match the neighbouring block's
trees cannot code. Spelling such a joining match as its decoded literals, when
the neighbour codes them, would have saved another 14,553 bits; cuts inside
matches added only 19.

A token keeps its own spelling in its own block. A joining match now costs the
cheaper of its own spelling and its literals, with an equal price keeping the
match; a joining literal the neighbour cannot code still blocks the cut. The
scan still walks the pair once per pass, now with each token's decoded bytes,
and the same rule spells every moved token, so the planned payload equals the
emission. A later sweep can return some of those literals to their original
block, where they cost less than the original match; each step strictly lowers
the payload. Literals need no match proof, and shorter matches at the same
distance are not tried.

The two-block oracle now repeats this move rule by brute force: every cut, home
and joining prices, the same tie order, until no cut is cheaper. It agrees on
all 300 streams; 214 slide, and 90 of those respell a match. A targeted test
moves a match that the left tree cannot code into that block as three literals.

| Set | Files | Smaller / larger | Bytes saved vs `bf2fe12` | CPU |
| --- | ---: | --- | ---: | --- |
| Default PNG: PngSuite, small, imageworsener, pkmn-col, medium | 518 | 32 / 0 | 1,875 | 535.95 → 538.41 s |
| Default PNG, large group | 7 | 2 / 0 | 1,333 | 43.67 → 44.47 s |
| Default GZIP and ZIP sample | 40 | 18 / 0 | 939 | 313.96 → 316.29 s |
| Max, 12 medium PNGs, 60 s | 12 | 2 / 0 | 80 | 1,696.06 → 1,701.51 s |
| Max, 61 multi-block small PNGs, 30 s | 61 | 1 / 0 | 1 | 3,988.16 → 3,996.97 s |

Gains concentrate where neighbouring blocks use different match alphabets:
`nerd.png` saved 1,205 bytes, `download_webp__260×280_.png` 837 and
`floor pattern.png` 595. In Max, `FsqwhPuaIAIlojU.png` now returns the slid
Default result, which beats its deadline-limited Max search by 15 bytes. Every
PNG output and all 40 GZIP and ZIP outputs decoded identically. `__text` grows
by 3,432 bytes; the executable stays at 1,810,848 bytes.

## Follow-up: re-planning moved blocks

Date: 6 October 2026. Baseline: `120a21f`. The slide fits boundaries to the
transmitted trees, and in Default nothing follows it, so each moved block kept
trees fitted to its old contents. A moved block is now re-planned with the
ordinary block planner, `plan_block`, using the call's options; the cheaper of
the transmitted and re-planned codes is kept, and a tie keeps the transmitted
header. Only moved blocks are re-planned. Stored blocks after a re-planned one
still have their padding repriced at the actual alignment.

Re-planning happens only after the sweeps, so the slide itself stays exactly
testable: the oracle tests run it without re-planning, and a separate test
checks that a block left with only `a` literals loses its now-unused `z` code
and gets smaller.

On the 518-file PNG set, the remaining fixed-tree headroom after `120a21f` was
zero for token cuts and literal respelling; respelling joining matches as
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
(476 bytes), `FsqwhPuaIAIlojU.png` (343) and `4.2.07.PNG` (260); in Max the
second arrives through the slid Default result. Repeated isolated runs put the
first's CPU cost at about 0.55 s on 30.5 s; `y2rc2_large.png`, which showed the
largest increase in the batch, timed identically in isolation. In the GZIP
sample, the three copies of `kzipmix-20200115-linux-static.tar.gz` each saved
1,124–1,403 bytes. Every PNG output and all 40 GZIP and ZIP outputs decoded
identically. `__text` grows by 1,244 bytes, which crosses a 16 KiB page: the
executable grows from 1,810,848 to 1,827,360 bytes.


## Follow-up: sliding to a fixed point

Date: 8 October 2026. Baseline: `7212d6b`. Re-planning fits a moved block's
trees to its new contents, but its cuts were fitted to the trees it had
before. The new trees can make further cuts cheaper, so one R1c invocation did
not reach a fixed point. A control arm in the distance-alphabet ladder
experiment, which slid a second time after R1c, found 1 byte on 365 raw
streams, 503 on 1,052 Default PNGs, 2,145 on 202 container files and 2,004 in
a 60-second Max sample, with no output larger.

The slide now runs in rounds. A round runs the existing sweeps with every tree
fixed, then re-plans each block the round moved, keeping a re-planned code
only when it is strictly cheaper; a tie keeps the current trees. If any block
took a new code, the next round sweeps again under the new trees and
re-plans the blocks it moves. Otherwise the cuts are already optimal for the
trees that stay, and the slide ends; a round that moves nothing also ends it.
Every round strictly lowers the planned stream, so rounds terminate. The cap
is 16; the probe below never needed more than 5. A block re-planned as stored
stays stored, so its boundaries stay fixed, as they would after a reparse.

The rounds work on the plan and emit once. Each invocation therefore keeps its
single candidate emission and parse, its whole-stream byte/meaningful-bit
admission and its linear-finalization start rule, and it needs no size class:
each round is linear in parsed tokens, plus the re-plans of the blocks it
moved. The call sites are unchanged: Default's single sweep, Max's settled
sweep and R12 closure, and Max's slid Default endpoint, which remains only a
final competitor.

### Round counts

An instrumented copy with a 1,000-round cap recorded every invocation in
Default runs. An invocation with zero rounds found no cheaper cut.

| Set | Invocations | Rounds: 0 / 1 / 2 / 3 / 4 / 5 | Planned bits after round 1 |
| --- | ---: | --- | ---: |
| Raw streams | 59 | 32 / 26 / 1 / 0 / 0 / 0 | 4 |
| PNG | 540 | 134 / 347 / 45 / 12 / 2 / 0 | 4,365 |
| GZIP, zlib, ZIP and APNG | 981 | 555 / 404 / 13 / 5 / 2 / 2 | 25,181 |
| Relaxed: raw streams and 16 PNG | 74 | 32 / 28 / 6 / 7 / 1 / 0 | 1,678 |

The most rounds came from `kzipmix-20200115-linux-static.tar.gz`: five rounds
over 175 blocks, 7,004 bits after the first. Among PNGs,
`FsqwhPuaIAIlojU.png` needed four rounds for 662 bits.

### Measurements

Paired runs alternate five arms per input, in four lanes for Default and two
for Max, and compare complete files: the baseline; the control above, which
slides twice; this change; and, for coordination, the distance-ladder
prototype with and without this change. The sets are those of the
distance-ladder validation.

| Set | Files | Smaller / larger | Bytes saved | Beyond the control: smaller / larger, bytes | CPU: baseline → candidate (control) |
| --- | ---: | --- | ---: | --- | --- |
| Raw streams | 365 | 1 / 0 | 1 | 0 / 0, 0 | 100.91 → 101.32 s (100.44) |
| PNG: medium, large, small, imageworsener, pkmn-col | 1,052 | 54 / 0 | 549 | 10 / 0, 46 | 1,553.55 → 1,557.16 s (1,555.52) |
| GZIP, zlib, ZIP and APNG | 202 | 17 / 0 | 3,145 | 7 / 0, 1,000 | 919.05 → 929.45 s (918.75) |
| Relaxed: raw streams and 16 PNG | 381 | 11 / 0 | 210 | 6 / 0, 33 | 218.69 → 219.08 s (220.25) |
| Max, 60 s, 25 PNG | 25 | 13 / 3 | 589 net | 5 / 3, −147 net | 3,104.44 → 3,138.23 s (3,138.97) |

No Default output grew against the baseline or the control. Two ZIP layouts
are rejected by every arm. The control's second slide already reaches the
fixed point on most files; the further rounds matter on large multi-block
streams. On `kzipmix-20200115-linux-static.tar.gz` the candidate saves 875
bytes and the control 535; on `FsqwhPuaIAIlojU.png`, 83 against 60.

**Max.** Max results depend on timing: 24 of the 25 runs used their whole
allowance, and the timed bounded routes end wherever the deadline finds
them. In the five-arm run the candidate saved 668 bytes on 13 files and lost
79 on three, and trailed the control on three files. Those files and
`FsqwhPuaIAIlojU.png` were then run three more times per arm, in the same two
lanes:

| File | Baseline | Control | Candidate | Five-arm run: baseline, control, candidate |
| --- | --- | --- | --- | --- |
| `floor pattern-deflopt.png` | 519,838 ×3 | 519,834 ×3 | 519,834 ×3 | 519,873, 519,840, 519,945 |
| `Partnership_Card___John_Lewis_Finance-deft4j-t139s.png` | 2,751,732–2,752,397 | 2,751,009–2,751,010 | 2,750,981–2,751,007 | 2,751,089, 2,751,010, 2,751,089 |
| `4.2.03.PNG` | 748,571 ×2, 748,614 | 748,571 ×2, 748,614 | 748,570 ×2, 748,614 | 748,623, 748,618, 748,624 |
| `nascar.png` | 22,761 ×2, 22,767 | 22,761, 22,767 ×2 | 22,767 ×3 | 22,761, 22,767, 22,767 |
| `FsqwhPuaIAIlojU.png` | 544,139 ×3 | 543,640 ×3 | 543,613 ×2, 543,614 | 544,145, 543,640, 543,614 |

The losses do not repeat. `nascar.png` and `4.2.03.PNG` take one of the same
two sizes in every arm; on the other three files the candidate is never
larger than the control in a repeat. The steady Max gain is
`FsqwhPuaIAIlojU.png`, 525–526 bytes. Max process CPU follows the allowance,
not the method.

**Cost.** Default CPU rises 0.2–1.1% across the sets. Timed alone, three
runs each, `kzipmix-20200115-linux-static.tar.gz` takes 9.68 → 10.32 s
(control 9.98 s), 6.6% for its five rounds over 175 blocks, and the 3.2 MB
`Partnership_Card___John_Lewis_Finance.png` 30.71 → 31.06 s (control
31.20 s): the extra rounds cost less than the control's second emission and
parse.

### Validation

- `iterated_slide_equals_repeated_slides_and_reaches_a_fixed_point` generates
  400 streams of two to four Huffman blocks with different alphabets, some
  with a stored block between them, in both strict modes. The iterated slide
  must emit exactly the bytes obtained by running one round at a time, each
  from a fresh parse of the previous emission, until a round finds nothing;
  sliding its result again must find nothing. 281 streams slide and 21 need
  more than one round; with the round cap set to one the test fails. Each
  result also reparses with its planned bit count, decoded bytes, block count,
  nonempty blocks, unchanged stored blocks and, in strict mode, strictly
  compatible codes.
- The terminal dispatch test now also checks that sliding the accepted
  candidate again, in Default and Max, finds nothing. The fixed-tree oracle
  and the other slide tests run unchanged, with one round.
- All library, binary, CLI, public API, doc and Python tests pass in debug and
  release, including the private-corpus regressions with
  `--include-ignored`. `cargo fmt --check` and `cargo clippy --all-targets
  --all-features -- -D warnings` pass.
- Python's `zlib`, `gzip` and `zipfile` decoded every output of all five arms
  and compared it with its source, including PNG chunk CRCs and APNG frames:
  1,825 raw, 5,260 PNG, 970 container, 1,905 relaxed and 125 Max outputs, with
  no mismatch. The eight container sources Python cannot decode give
  identical outputs in every arm.
- `__text` grows by 2,156 bytes; the release executable stays at 1,827,360
  bytes.

### Coordination with the distance-alphabet ladder

The distance-alphabet ladder, R13, was an uncommitted prototype when this
was measured. It slides once more after a winning ladder; that slide now runs
to a fixed point too, without a change to the ladder. The two prototypes
compose as follows:

| Set | Ladder → ladder with this change: smaller / larger, bytes | This change → ladder with this change: smaller / larger, bytes |
| --- | --- | --- |
| Raw streams | 0 / 0, 0 | 1 / 0, 1 |
| PNG | 31 / 7, +138 −7 | 50 / 0, 475,932 |
| GZIP, zlib, ZIP and APNG | 14 / 0, 1,051 | 26 / 0, 205 |
| Relaxed | 5 / 1, +44 −2 | 16 / 0, 95,050 |

Against the ladder alone, this change makes seven PNG files one byte larger
and one relaxed file two bytes larger. The ladder decides each block with its
boundaries fixed, so its choice depends on where the slide left them. On
`4.2.03.PNG` both arms end with the same boundaries, but they keep different
distance symbols in blocks 22 and 23. Without this change the ladder chose
them for the blocks as one slide left them; with it, after the fixed-point
slide had moved 1,471 decoded bytes from block 22 into block 23, it dropped
different symbols and spelled other matches as literals. The final blocks
cost 13 bits more and cross a byte boundary. With this change as the
baseline, the ladder makes no Default output larger, and its own container
gain falls from 2,299 to 205 bytes, since most of it came from the slide that
followed a ladder win. Max was not repeated for the ladder arms; their
five-arm differences include the same timing outliers as above.
