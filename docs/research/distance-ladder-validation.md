<!-- SPDX-License-Identifier: MIT -->

# Distance-alphabet ladder: validation

Date: 8 October 2026. Baseline: `7212d6b`. **Accepted and implemented** as
R13 in the [route catalogue](../routes-and-methods.md#route-gate-reference),
after [rebasing onto the fixed-point slide](#rebased-onto-the-fixed-point-slide).
It adds a search dimension to Columbo; worldwide novelty is not claimed.

## Why it saves bits

Every distance code a dynamic block uses costs header bits, and every extra
code lengthens the others. On image data with a strong period, a few short
distances can carry the block while many rare far distances inflate both
trees. Spelling every match outside a kept set of distance symbols as its
decoded literals removes those codes from the distance tree and those matches
from the length tree. The literals may cost more payload than the matches
they replace, while the complete block gets smaller.
[RFC 1951 §3.2.7](https://www.rfc-editor.org/rfc/rfc1951#section-3.2.7)

The idea comes from zenflate's PNG mode, which compares each block with a
runs-only parse because "any offset code beyond distance 1 lengthens the run
codes". Its notes also warn that demoting matches to literals is no proxy for
that parse, since other matches swallow the starts of runs. Columbo cannot
add the missing distance-1 matches; it can only remove existing ones, so the
transfer had to be measured rather than assumed.

Columbo's existing searches did not reach these candidates. R5 bans at most
four adjacent used symbols, affecting at most 64 matches and 8,192 bytes per
block. Max's match-group search ranks groups by their local per-match saving
and combines at most five, which puts valuable long-distance groups last. The
cumulative band routes act only on length symbols.

## Method

[`distance_band.rs`](../../src/deflate/distance_band.rs) builds three ladders
of kept distance-symbol sets over the symbols a block uses: the shortest
distances first, the most frequent first, and each symbol alone. Keeping every
used symbol is the parent and keeping none is the all-literals endpoint, so
neither is a rung; at most 88 masks remain after deduplication.

One linear pass records, for each distance symbol, the literal bytes, length
symbols and extra bits its matches contribute. Each rung's histograms then
follow by subtraction, and `estimate_boundary_block_bits` prices them with
DeflOpt variant-0 trees and a greedy header, or the fixed tree, without
building tokens. At most two rungs whose estimate does not exceed the parent's
own estimate are materialized and priced with the ordinary non-exhaustive
block planner. A block keeps the cheaper plan only when it is strictly smaller
than its current bits. Ties in the estimate keep the smaller mask.

### Placement

R13 follows R1c. In Default it runs once; after a win, R1c runs once more,
because the slide fitted its cuts to trees the ladder replaced. In Max it
waits, like R10, R11 and R1c, for a sweep in which every earlier method
changed nothing, or for the last sweep; R12 repeats the sweep after a win.
Max's mandatory Default endpoints apply the same R1c, R13, R1c finish to their
slid final competitor, so Max never trails Default. Like R1c it is linear
finalization with no size or block-count class.

Three earlier choices failed:

- **After R5, before R1c.** The first prototype ran where R5 ends. On the
  first 160 PNG pairs it saved 476,273 bytes, but five files grew by one
  byte each. On `4.2.07.PNG` the ladder found 7 bits before the slide; the
  slide that followed ended 7 bits worse than the baseline slide and crossed
  a byte boundary. After the slide, the same file has 471 bits of ladder
  headroom.
- **Mid-sweep in Max.** Placed after R5 in Max's sweep, a win redirected R6–R9:
  the generated closure stream in
  `terminal_max_closure_revisits_methods_after_a_later_tree_or_split_win`
  stopped at 36,675 bits instead of at most 36,637.
- **Exhaustive pricing in Max.** Max's mandatory first sweep must reproduce
  the Default endpoint exactly when its optional budget has expired. Pricing
  rungs with Max's tree families broke that equality, so both modes use the
  ordinary planner, as R5 does.

## Headroom probe

Before integration, a test-only probe priced every rung exactly on every
non-stored block of completed baseline Default outputs: the 365-stream raw
set and 1,052 PNG files from five fixture sets. Gains are measured against
`plan_block` on the parsed parent block.

| Set | Files | Blocks probed | Winning blocks | Files gaining | Bits |
| --- | ---: | ---: | ---: | ---: | ---: |
| Raw streams | 365 | 382 | 5 | 5 | 20 |
| PNG medium | 387 | 3,510 | 147 | 57 | 636,020 |
| PNG large | 21 | 498 | 46 | 6 | 3,160,346 |
| PNG small, imageworsener, pkmn-col | 644 | 952 | 0 | 0 | 0 |

The gain is real but concentrated: six of 63 PNG files hold 99.9% of it.
Three are copies of the same image in the fixture sets' original, DeflOpt and
deft4j variants. `Partnership_Card___John_Lewis_Finance-deft4j-t139s.png`
alone holds 3,043,857 bits: its Default output was 427 KB larger than the
original image's Default output, and the ladder recovers most of that.
`floor pattern.png` holds 316,666 bits and `nerd.png` 54,076. The kept sets
are short distances: distance 1, 2, 4 or 7–8 alone, or every distance up to
6–16.

### Rung selection

A second probe recorded each rung's estimate beside its exact price. Ranking
by estimate and exactly pricing only the best rungs loses almost nothing:

| Policy | PNG bits | Share of exact optimum | Exact prices |
| --- | ---: | ---: | ---: |
| Every rung | 3,796,366 | 100% | 256,417 |
| Best 2 within 64 estimated bits | 3,796,350 | 100.0% | 4,831 |
| **Best 2 not above the parent's estimate** | 3,796,328 | 100.0% | 1,061 |
| Best 1 not above the parent's estimate | 3,796,312 | 100.0% | 774 |
| Every shortest-distance rung | 3,728,204 | 98.2% | 87,791 |
| Every single-symbol rung | 3,734,014 | 98.4% | 92,736 |
| Every frequency rung | 2,684,119 | 70.7% | 87,791 |

On the raw set the chosen policy finds 15 of the 20 bits. The frequency
ladder holds most small wins that the other two miss, so all three remain.

## Measurements

Paired runs alternate three arms per input in four lanes and compare
complete files: the baseline, the candidate, and a control that skips the
ladder but always slides a second time where R13 would run. The control
separates a different effect. R1c re-planned the blocks it moved, so it was
not at a fixed point after one run, and a second slide alone saved bytes.
R1c now runs to a fixed point; see the
[rebased measurements](#rebased-onto-the-fixed-point-slide).

| Set | Files | Smaller / larger | Bytes saved | Control: smaller / bytes | CPU: baseline → candidate (control) |
| --- | ---: | --- | ---: | --- | --- |
| Raw streams | 365 | 2 / 0 | 2 | 1 / 1 | 100.13 → 100.54 s (99.63) |
| PNG: medium, large, small, imageworsener, pkmn-col | 1,052 | 57 / 0 | 476,350 | 54 / 503 | 1,529.04 → 1,544.14 s (1,533.79) |
| GZIP, zlib, ZIP and APNG | 202 | 28 / 0 | 2,299 | 17 / 2,145 | 899.32 → 917.54 s (906.14) |
| Relaxed: raw streams and 16 PNG | 381 | 18 / 0 | 95,218 | 11 / 177 | 208.43 → 212.90 s (210.58) |
| Max, 60 s, 25 PNG | 25 | 4 / 0 | 1,327 | 15 / 2,004 | 3,162.25 → 3,168.14 s (3,171.76) |

No output grew against the baseline in any set. The container set holds every
GZIP, zlib and ZIP fixture except `samplelib-zip`, plus three APNG sets; two
ZIP layouts are rejected by every arm. The Max set holds the 16 PNG files
with probe headroom above a few bits, eight random medium controls and the
deft4j Partnership card; every Max run used its full allowance.

**Attribution.** On PNG and relaxed output the ladder holds the gain: against
the control the candidate saves 475,910 and 95,041 bytes. On PNG it is 63
bytes worse than the control on 19 files, where only the control's
unconditional second slide helped. On containers the ladder itself adds
little: 190 bytes over the control, and 36 bytes behind it on five files.
Most of the candidate's 2,299 container bytes come from the second slide that
follows a ladder win of a few bits. On
`kzipmix-20200115-linux-static.tar.gz` the ladder saves 67 bits and the
following slide 4,283; the control alone reaches 535 of the candidate's 544
bytes.

In Max the ladder matters only for the poor deft4j stream, 1,312 bytes.
Max's broader routes already reach the collapsed layouts: `floor pattern.png`
ends at 519,838 bytes in Max against 546,595 for Default with the ladder.
The control's repeated slide found 2,004 bytes there, 499 of them on
`FsqwhPuaIAIlojU.png`.

**Selection window.** An earlier run of the same placement priced rungs up
to 64 estimated bits above the parent. It saved 476,355 PNG bytes for 1.8%
more CPU; the final window saves 476,350 for 1.0%.

**Cost.** CPU rises 0.4–2.1% in Default. Isolated, the GZIP above takes
9.75 → 10.6 s (control 9.94 s), and the original 3.2 MB Partnership card
30.44 → 31.25 s (control 30.89 s). On that card the ladder step takes 367 ms,
including its candidate's emission and validation, and the following slide
450 ms.

## Validation

- `rung_estimates_match_materialized_tokens` checks, on 200 pseudo-random
  blocks and both strict modes, that every frequency-delta estimate equals
  the estimate of the materialized tokens; more than 1,000 rungs are checked.
  The same test confirms each rung keeps exactly the matches of its kept
  symbols and spells the rest as the decoded bytes.
- A generated period-4 block with one length-3 match in each of distance
  symbols 5–29 collapses to its distance-4 runs in both modes; the plan
  emits exactly its priced bits and reparses to the same bytes, with
  strictly compatible codes in strict mode. Further tests cover the rung
  menu, stored and one-symbol blocks, an expired stop, and the Default sweep
  and Max Default endpoint finishing with the ladder.
- The existing Max closure test keeps its bound, and an expired Max budget
  still reproduces the Default terminal endpoint exactly.
- Python's `zlib`, `gzip` and `zipfile` decoded every control and candidate
  output and compared it with its source, including PNG chunk CRCs and APNG
  frames: 730 raw, 2,104 PNG, 388 container, 762 relaxed and 50 Max outputs,
  with no mismatch. Python cannot decode eight container sources: six legacy
  ZIP methods, whose outputs are identical in every arm, and the two layouts
  Columbo rejects.
- `cargo fmt --check`, `cargo clippy --all-targets --all-features -- -D
  warnings`, all library, binary, CLI, public API, doc and Python tests pass.
  The private-corpus regressions pass with `--include-ignored` in debug and
  release.
- The release executable grows from 1,827,360 to 1,843,888 bytes, one
  16 KiB page; `__text` grows by 16,616 bytes.

## Limits

The ladder only removes matches. zenflate's runs-only parse also adds the
distance-1 matches that other matches had swallowed; Columbo cannot, so most
images gain nothing. The measured gains come from a few images whose encoder
spread a periodic image across many distances, and from one poor deft4j
stream.

Kept sets are three ladders, not arbitrary subsets, and each block is decided
alone with its boundaries fixed. Rungs are estimated with one tree family and
priced with the ordinary planner only. Max gains little because its broader
routes already reach most of these layouts.

The second slide after a win is part of R13. The control showed that R1c
also left bytes behind without the ladder; R1c now
[slides to a fixed point](boundary-slide-validation.md#follow-up-sliding-to-a-fixed-point),
and most of the ladder's container gain above moved to it.

## Rebased onto the fixed-point slide

Date: 8 October 2026. Baseline: `617af17`, where R1c
[slides to a fixed point](boundary-slide-validation.md#follow-up-sliding-to-a-fixed-point).
R13's code is unchanged; its slide after a win now also runs to a fixed
point. The five-arm runs of that validation include this baseline and the
ladder on top of it, with identical production code:

| Set | Files | Smaller / larger | Bytes saved | CPU |
| --- | ---: | --- | ---: | --- |
| Raw streams | 365 | 1 / 0 | 1 | 101.32 → 101.32 s |
| PNG: medium, large, small, imageworsener, pkmn-col | 1,052 | 50 / 0 | 475,932 | 1,557.16 → 1,566.03 s |
| GZIP, zlib, ZIP and APNG | 202 | 26 / 0 | 205 | 929.45 → 944.54 s |
| Relaxed: raw streams and 16 PNG | 381 | 16 / 0 | 95,050 | 219.08 → 223.74 s |

No Default output grows. The PNG and relaxed gains are those measured above;
the container gain falls from 2,299 to 205 bytes because the fixed-point
slide now finds what the ladder's second slide found. Against the ladder on
the old baseline, the combination makes seven PNG files one byte larger and
one relaxed file two bytes larger; the
[slide validation](boundary-slide-validation.md#coordination-with-the-distance-alphabet-ladder)
traces this to the ladder choosing kept symbols for the blocks as the slide
left them.

**Max.** In the 25-file Max sample the ladder made four files smaller and
five larger against the new baseline, 106 bytes against 110, but 24 of 25
runs used their whole allowance and those differences include timing
outliers. Three more runs per arm of the six files that moved most:

| File | Baseline | With the ladder |
| --- | --- | --- |
| `floor pattern.png` | 519,834 ×3 | 519,833 ×3 |
| `floor pattern-deflopt.png` | 519,834 ×3 | 519,833 ×3 |
| `4.2.07.PNG` | 725,447 ×3 | 725,447 ×3 |
| `4.2.07-deflopt.png` | 725,440 ×2, 725,444 | 725,440 ×2, 725,444 |
| `4.2.07-deft4j-t8s.PNG` | 725,443 ×2, 725,449 | 725,443 ×2, 725,457 |
| `FsqwhPuaIAIlojU.png` | 543,613 ×2, 543,614 | 543,613 ×2, 544,098 |

Most repeats match or gain a byte. `FsqwhPuaIAIlojU.png` ended 485 bytes
larger in one of three runs, and 93 bytes larger in the five-arm run, while
the baseline reached 543,613–543,614 in all four. A plausible cause, not
isolated, is the ladder's mandatory work on Max's slid Default endpoint,
which leaves less time for routes that all reach the deadline on this
stream. Max gains little from the ladder, so this remains a known cost
rather than a reason to withhold it from Default.

Every output of both arms decoded identically with Python, including the
repeats. All library, binary, CLI, public API, doc and Python tests pass on
the combined code in debug and release, with the private-corpus regressions.
