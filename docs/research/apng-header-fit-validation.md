<!-- SPDX-License-Identifier: MIT -->

# APNG header fit: validation

Date: 16 October 2026. Baseline: `6c81dfa`. Under `ApngDefault`, R2–R4 now
run after R18 instead of in the terminal sweep; the
[route catalog](../routes-and-methods.md#route-gate-reference) describes the
order.

## Why it saves bits

R2 (payload/header tradeoff), R3 (literal/length span) and R4 (joint tree/RLE)
fit a block's code lengths and header to the codes it already has. They are
meant to follow the step that chooses those codes. Default's floor phase runs
the terminal tree floors before the sweep, so R2–R4 refine trees already
chosen from the compact, bounded-depth and feedback families. `ApngDefault`
skips those floors to keep per-frame cost down. Its R2–R4 then fit the
planner's ordinary codes, and R14 and R18, the first tree-family steps it
runs, replace most of them: on 20 APNG files R14 saved 94,436 bits in the
first pass. The early fit is lost, and it steers the later steps from a parent
that will not last. Run after R18, R2–R4 fit the codes that stay.

The rule follows the pipeline, not a size class: R2–R4 come after the stream's
first tree-family choice. Every other policy already satisfies it and is
unchanged.

## Origin

A second Default pass over `6c81dfa`'s output saved 1,211 bytes on 25 of 33
mixed files and 7,325 bytes on 87 of 120 APNG-heavy PNGs. On the 20 APNG files
with the largest second-pass savings, R4 found 26,466 bits in the second pass,
against 1,682 for R2 and 710 for R14. In the first pass on the same files,
R2–R5 took 638 of the 1,166 seconds that Default's reported steps took.

Placements tried, bytes against `6c81dfa`. CPU was measured while other heavy
work loaded the machine and is indicative only:

| Placement | 120 APNG-heavy PNGs | Other samples |
| --- | --- | --- |
| R4 after R17, every stream | 50 / 10, −3,748 bytes, +6.8% CPU | |
| R2–R5 after R17, every stream | 73 / 1, −7,707, +18.4% | |
| R2–R4 after R17, every stream | 73 / 7, −5,411, +9.4% | |
| R2–R4 after R18, every stream | 74 / 6, −6,358, +1.4–3.2% | full suite: 22 of 1,052 PNGs and 21 of 202 containers larger, by up to 11 bytes, all small streams (Playdate ZIP members, small PNGs) |
| As above, plus the early fit for compact streams (≤8 KiB compressed, ≤128 KiB decoded) | 81 / 0, −6,580, +33% | 33 mixed files 11 / 0, −101 |
| As above, plus R18 and R2–R4 repeated after a win | | recovered only part of the small-stream losses |
| **R2–R4 after R18 under `ApngDefault` only** | **66 / 2, −6,312, +3.7% (serial)** | other policies byte-identical to the baseline |

Moving the fit for every stream lost on small static streams. There the floor
phase has already chosen the trees and the header is a large share of the
bits, so the early fit gives R5, R1c and R13–R15 a better parent. Keeping it
for compact streams fixed the losses but ran R2–R4 twice on most APNG frames,
which are compact too, and its size limits were borrowed from another method
rather than derived from this effect.

## Design

- **`ApngDefault`.** The terminal sweep skips R2–R4; after R18 they run once,
  in order, on the final candidate, with the sweep's work class.
- **Other policies.** Unchanged: R2–R4 stay in the sweep after R1b.
- **Max.** Max's sweeps are unchanged. `ApngMax`'s APNG Default endpoint, the
  final competitor that keeps Max from ending above Default, follows
  `ApngDefault`'s order: R1b, R5, the finish, R18, then R2–R4. Max's parent
  keeps R2–R5 in its sweep order.

## Measurements

Paired runs alternate the baseline and the candidate per input in four
lanes, at the 180-second allowance, and compare complete files. Other work
loaded the machine during parts of the run (load average up to 16); both arms
shared it.

| Set | Files | Smaller / larger | Bytes saved | Process CPU |
| --- | ---: | --- | ---: | --- |
| Raw streams | 365 | 0 / 0 | 0 | 49.20 → 49.13 s |
| PNG: medium, large, small, imageworsener, pkmn-col | 1,052 | 0 / 1 | −1 | 1,360.97 → 1,361.80 s |
| GZIP, zlib, ZIP and APNG | 202 | 38 / 0 | 1,321 | 732.61 → 721.58 s |
| Relaxed: raw streams and 16 PNG | 381 | 0 / 0 | 0 | 236.15 → 232.86 s |

Only APNG files change. The one PNG row, `81817-2-deflopt.png` (+1), is
5,746 bytes for both executables in three serial reruns, a timing effect of
the baseline. Every ZIP, including the Playdate archives that lost when the
fit moved for every stream, is unchanged. All 38 container changes are APNG
files, led by `bored.png` (89 bytes), `APNG_throbber.png` (79–84), `dance.png`
(70) and `clock.png` (67).

**APNG-heavy sample.** On 120 random PNGs, mostly animated stickers, 66 files
saved 6,312 bytes and two stickers grew by 5 bytes each, repeatably. Run
serially with alternating executables, CPU rose 3.7% in total (674.1 → 698.9
s), while the median file was 1.5% faster, and the median APNG 2.4% faster.
A few large stickers carry the added cost.

**Max.** The 25-PNG Max sample has no APNG files, and its code paths are
unchanged: three files moved by 2–6 bytes either way, the deadline variance
recorded before, and every Max output was no larger than its Default output.
On four APNGs at 30 seconds, Max was no larger than the new Default and smaller
than the baseline's Max on each: `APNG_throbber.png` 106,108 against 106,169,
`bored.png` 245,631 against 245,634, `clock.png` 23,026 against 23,031, and the
largest sticker 187,611 against 188,046.

## Validation

- `terminal_merges_follow_the_settled_finish` now also checks the APNG
  order: Max's APNG Default endpoint, which ends with R2–R4, still merges the
  blocks and leaves R4 nothing to fit.
- Python's `zlib`, `gzip` and `zipfile` decoded every candidate output and
  compared it with its source, including PNG chunk CRCs and APNG frames: 365
  raw, 1,052 PNG, 194 container, 381 relaxed and 25 Max outputs, with no
  mismatch. Python cannot read eight of the 202 container sources: six ZIP
  files use legacy compression methods, and Columbo also rejects the other
  two, which have unsupported ZIP layouts, in both arms.
- Formatting, Clippy and all 681 Rust tests pass, including the
  private-corpus regressions in debug and release, with 13 Python tests and a
  locked build.
- The release executable keeps its size, 1,910,064 bytes.

Harness, lists, arm crates, load log and suite log:
`work/final-header-fit-validation/`.
