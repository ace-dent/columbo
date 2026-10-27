<!-- SPDX-License-Identifier: MIT -->

# Distance-alphabet ladder: validation

Date: 8 October 2026. Baseline: `7212d6b`. **Accepted and implemented** as R13,
after
[rebasing onto the fixed-point slide](#rebased-onto-the-fixed-point-slide); the
[route catalog](../routes-and-methods.md#distance-alphabet-ladder-r13) describes
the current method.

## Why it saves bits

Spelling every match outside a kept set of distance symbols as literals removes
those codes from the distance tree and those matches' length symbols from the
literal/length tree. The literals may cost more payload while the block gets
smaller: on image data with a strong period, a few short distances can carry
the block while many rare far distances lengthen every code.
[RFC 1951 §3.2.7](https://www.rfc-editor.org/rfc/rfc1951#section-3.2.7)

The idea comes from zenflate's PNG mode, which compares each block with a
runs-only parse. Its notes warn that demoting matches to literals is no proxy
for that parse, since other matches swallow the starts of runs; Columbo can
only remove existing matches, so the idea had to be measured. Existing searches
did not reach these candidates: R5 bans at most four adjacent used symbols (at
most 64 matches and 8,192 bytes per block), Max's match-group search combines at
most five groups ranked by per-match saving, which puts valuable long-distance
groups last, and the cumulative band routes act only on length symbols.

## Method

[`distance_band.rs`](../../src/deflate/distance_band.rs) implements the
[R13 entry](../routes-and-methods.md#distance-alphabet-ladder-r13).

### Placement

R13 follows R1c, which runs once more after a win because the slide fitted its
cuts to trees that the ladder replaced. Three earlier placements failed:

- **After R5, before R1c.** On the first 160 PNG pairs this saved 476,273 bytes,
  but five files grew by one byte each. On `4.2.07.PNG` the ladder found 7 bits
  before the slide, and the slide that followed ended 7 bits worse than the
  baseline slide, crossing a byte boundary. After the slide, the same file has
  471 bits of ladder headroom.
- **Mid-sweep in Max.** Placed after R5 in Max's sweep, a win redirected
  R6–R9: the generated closure stream in
  `terminal_max_closure_revisits_methods_after_a_later_tree_or_split_win`
  stopped at 36,675 bits instead of at most 36,637.
- **Exhaustive pricing in Max.** Max's mandatory first sweep must reproduce the
  Default endpoint exactly when its optional budget has expired. Pricing rungs
  with Max's tree families broke that equality, so both modes use the ordinary
  planner, as R5 does.

## Headroom probe

A test-only probe priced every rung exactly on every non-stored block of
completed baseline Default outputs: the 365-stream raw set and 1,052 PNG files
from five fixture sets. Gains are measured against `plan_block` on the parsed
parent block.

| Set | Files | Blocks probed | Winning blocks | Files gaining | Bits |
| --- | ---: | ---: | ---: | ---: | ---: |
| Raw streams | 365 | 382 | 5 | 5 | 20 |
| PNG medium | 387 | 3,510 | 147 | 57 | 636,020 |
| PNG large | 21 | 498 | 46 | 6 | 3,160,346 |
| PNG small, imageworsener, pkmn-col | 644 | 952 | 0 | 0 | 0 |

The gain is concentrated: six of 63 PNG files hold 99.9% of it. Three are copies
of the same image in the fixture sets' original, DeflOpt and deft4j variants.
`Partnership_Card___John_Lewis_Finance-deft4j-t139s.png` alone holds 3,043,857
bits: its Default output was 427 KB larger than the original image's Default
output, and the ladder recovers most of that. `floor pattern.png` holds 316,666
bits and `nerd.png` 54,076. The kept sets are short distances: distance 1, 2, 4
or 7–8 alone, or every distance up to 6–16.

### Rung selection

A second probe recorded each rung's estimate beside its exact price. Exactly
pricing only the best-estimated rungs loses almost nothing:

| Policy | PNG bits | Share of exact optimum | Exact prices |
| --- | ---: | ---: | ---: |
| Every rung | 3,796,366 | 100% | 256,417 |
| Best 2 within 64 estimated bits | 3,796,350 | 100.0% | 4,831 |
| **Best 2 not above the parent's estimate** | 3,796,328 | 100.0% | 1,061 |
| Best 1 not above the parent's estimate | 3,796,312 | 100.0% | 774 |
| Every shortest-distance rung | 3,728,204 | 98.2% | 87,791 |
| Every single-symbol rung | 3,734,014 | 98.4% | 92,736 |
| Every frequency rung | 2,684,119 | 70.7% | 87,791 |

On the raw set the chosen policy finds 15 of the 20 bits. The frequency ladder
holds most small wins that the other two miss, so all three ladders remain.

## Measurements

Paired runs alternate three arms per input in four lanes and compare complete
files: the baseline, the candidate, and a control that skips the ladder but
always slides a second time where R13 would run. The control isolates a
separate effect: R1c did not yet reach a fixed point, so a second slide alone
saved bytes (see the
[rebased measurements](#rebased-onto-the-fixed-point-slide)).

| Set | Files | Smaller / larger | Bytes saved | Control: smaller / bytes | CPU: baseline → candidate (control) |
| --- | ---: | --- | ---: | --- | --- |
| Raw streams | 365 | 2 / 0 | 2 | 1 / 1 | 100.13 → 100.54 s (99.63) |
| PNG: medium, large, small, imageworsener, pkmn-col | 1,052 | 57 / 0 | 476,350 | 54 / 503 | 1,529.04 → 1,544.14 s (1,533.79) |
| GZIP, zlib, ZIP and APNG | 202 | 28 / 0 | 2,299 | 17 / 2,145 | 899.32 → 917.54 s (906.14) |
| Relaxed: raw streams and 16 PNG | 381 | 18 / 0 | 95,218 | 11 / 177 | 208.43 → 212.90 s (210.58) |
| Max, 60 s, 25 PNG | 25 | 4 / 0 | 1,327 | 15 / 2,004 | 3,162.25 → 3,168.14 s (3,171.76) |

No output grew against the baseline in any set. The container set holds every
GZIP, zlib and ZIP fixture except `samplelib-zip`, plus three APNG sets; every
arm rejects two ZIP layouts. The Max set holds the 16 PNG files with probe
headroom above a few bits, eight random medium controls and the deft4j
Partnership card; every Max run used its full allowance.

**Attribution.** On PNG and relaxed output the ladder provides the gain:
against the control, the candidate saves 475,910 and 95,041 bytes. On PNG it is
63 bytes worse than the control on 19 files, where only the control's
unconditional second slide helped. On containers the ladder itself adds 190
bytes over the control and is 36 bytes behind it on five files; most of the
candidate's 2,299 container bytes come from the second slide after a ladder win
of a few bits. On `kzipmix-20200115-linux-static.tar.gz` the ladder saves 67
bits and the following slide 4,283; the control alone reaches 535 of the
candidate's 544 bytes.

In Max the ladder matters only for the poor deft4j stream, 1,312 bytes; Max's
broader routes already reach the collapsed layouts (`floor pattern.png` ends at
519,838 bytes in Max against 546,595 for Default with the ladder). The control's
repeated slide found 2,004 bytes there, 499 of them on `FsqwhPuaIAIlojU.png`.

**Selection window.** An earlier run of the same placement priced rungs up to 64
estimated bits above the parent. It saved 476,355 PNG bytes for 1.8% more CPU;
the final window saves 476,350 for 1.0%.

**Cost.** CPU rises 0.4–2.1% in Default. Isolated, the GZIP above takes
9.75 → 10.6 s (control 9.94 s), and the original 3.2 MB Partnership card
30.44 → 31.25 s (control 30.89 s). On that card the ladder step takes 367 ms,
including its candidate's emission and validation, and the following slide
450 ms.

## Validation

- `rung_estimates_match_materialized_tokens` checks, on 200 pseudo-random blocks
  in both strict modes and more than 1,000 rungs, that every frequency-delta
  estimate equals the estimate of the materialized tokens, and that each rung
  keeps exactly the matches of its kept symbols.
- A generated period-4 block with one length-3 match in each of distance symbols
  5–29 collapses to its distance-4 runs in both modes and emits exactly its
  priced bits. The Max closure test keeps its bound, and an expired Max budget
  still reproduces the Default endpoint.
- Python's `zlib`, `gzip` and `zipfile` decoded every control and candidate
  output against its source, including PNG chunk CRCs and APNG frames: 730 raw,
  2,104 PNG, 388 container, 762 relaxed and 50 Max outputs, with no mismatch.
  Python cannot decode eight container sources: six legacy ZIP methods, whose
  outputs are identical in every arm, and the two layouts Columbo rejects.
- Formatting, Clippy and all tests pass in debug and release, including the
  private-corpus regressions.
- The release executable grows from 1,827,360 to 1,843,888 bytes, one 16 KiB
  page; `__text` grows by 16,616 bytes.

Because the ladder cannot add the distance-1 matches that other matches
swallowed, most images gain nothing; the gains come from a few images whose
encoder spread a periodic image across many distances, and from one poor deft4j
stream.

## Rebased onto the fixed-point slide

Date: 8 October 2026. Baseline: `617af17`, where R1c
[slides to a fixed point](boundary-slide-validation.md#follow-up-sliding-to-a-fixed-point).
R13's code is unchanged; its slide after a win now also runs to a fixed point.
The five-arm runs of that validation include this baseline and the ladder on top
of it:

| Set | Files | Smaller / larger | Bytes saved | CPU |
| --- | ---: | --- | ---: | --- |
| Raw streams | 365 | 1 / 0 | 1 | 101.32 → 101.32 s |
| PNG: medium, large, small, imageworsener, pkmn-col | 1,052 | 50 / 0 | 475,932 | 1,557.16 → 1,566.03 s |
| GZIP, zlib, ZIP and APNG | 202 | 26 / 0 | 205 | 929.45 → 944.54 s |
| Relaxed: raw streams and 16 PNG | 381 | 16 / 0 | 95,050 | 219.08 → 223.74 s |

No Default output grows. The container gain falls from 2,299 to 205 bytes
because the fixed-point slide now finds what the ladder's second slide found.
Seven PNG files and one relaxed file end one or two bytes larger than with the
ladder on the old baseline, as the
[slide validation](boundary-slide-validation.md#coordination-with-the-distance-alphabet-ladder)
explains.

**Max.** In the 25-file Max sample the ladder made four files smaller and five
larger against the new baseline, 106 bytes against 110, but 24 of 25 runs used
their whole allowance. Three more runs per arm of the six files that moved most:

| File | Baseline | With the ladder |
| --- | --- | --- |
| `floor pattern.png` | 519,834 ×3 | 519,833 ×3 |
| `floor pattern-deflopt.png` | 519,834 ×3 | 519,833 ×3 |
| `4.2.07.PNG` | 725,447 ×3 | 725,447 ×3 |
| `4.2.07-deflopt.png` | 725,440 ×2, 725,444 | 725,440 ×2, 725,444 |
| `4.2.07-deft4j-t8s.PNG` | 725,443 ×2, 725,449 | 725,443 ×2, 725,457 |
| `FsqwhPuaIAIlojU.png` | 543,613 ×2, 543,614 | 543,613 ×2, 544,098 |

Most repeats match or gain a byte. `FsqwhPuaIAIlojU.png` ended 485 bytes larger
in one of three runs, and 93 bytes larger in the five-arm run, while the
baseline reached 543,613–543,614 in all four. A plausible but unisolated cause
is the ladder's mandatory work on Max's slid Default endpoint, which leaves less
time for routes that all reach the deadline on this stream. This remains a known
Max cost rather than a reason to withhold the ladder from Default.

Every output of both arms decoded identically with Python. All tests pass on
the combined code in debug and release.
