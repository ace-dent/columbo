<!-- SPDX-License-Identifier: MIT -->

# Terminal merges: validation

Date: 14 October 2026. Baseline: `3d3f198`. Implemented as R16; the
[route catalog](../routes-and-methods.md#route-gate-reference) describes the
method.

## Why it saves bits

Joining two blocks saves one dynamic header and one end-of-block code, but
codes both regions with one compromise table. Default's main search merges
neighbors only when they decode to at most 512 bytes together, and once the
terminal sweep has begun, nothing removes a boundary: the slide (R1c) moves
boundaries under fixed trees, and R15 only adds them. Blocks whose boundaries
came from earlier layouts, or whose statistics converged after R13–R15
rewrote their trees, can therefore cost more apart than together.

## Origin

After R15, the 1,052 completed Default PNG outputs from its validation run
(367.5 Mbit) were scanned with a zeroth-order entropy estimate that charges
each block its own header bits. Merging adjacent pairs estimated 225,258 bits
(0.061% of all bits) across 1,209 pairs, more than the single cuts R15 left
(125,737 bits, mostly rejected by exact pricing) or two-cut intervals
(109,013 bits beyond single cuts).

Exact probes against `3d3f198` compared three placements and three other
candidates:

| Probe, Default | 33-file sample: smaller / larger, bytes | 120 random PNGs: smaller / larger, bytes |
| --- | --- | --- |
| Merges before R15 | 8 / 1, −3,915 (`skinsrc.zip` +62) | 13 / 0, −632 |
| Merges after R15, inside the repeated finish | 9 / 0, −5,299 | 14 / 1, −708 (+2 on one APNG) |
| **Merges after the settled finish (R16)** | **9 / 0, −5,223** | **12 / 0, −476** |
| Interval cuts in R15 | 4 / 1, +3,097 (Partnership card +3,228) | — |
| Header methods R2–R5 rerun after R13–R15 | 6 / 0, −51 | — |
| R7 in Default | 2 / 0, −2 | — |
| A second complete Default pass | 25 / 0, −2,348, 72 s more CPU | — |

The earlier placements grow outputs because a strictly smaller intermediate
can lead R13–R15 to a different, worse fixed point: on one APNG frame a 6-bit
merge let R14 end at 2,229 bytes instead of 2,227. After the settled finish,
every R16 step only lowers the finished result, so no output can grow.

## Design

- **Pass.** Left to right, each block is held as the pending block. If it and
  its neighbor are both non-stored, the pair is merged into one range and
  planned with the bounded table search (as in R6 and R15), at the bit
  alignment where the pending block starts. The merge is kept when it is
  strictly smaller than the pending block's bits plus the neighbor's encoded
  bits at its own alignment; the merged block may then absorb the next
  neighbor. Otherwise the pending block is written and the neighbor becomes
  pending.
- **Exact reuse.** Unchanged blocks keep their encoding. A merged block keeps
  the plan it was accepted with, since nothing after it changes its starting
  alignment.
- **Bounds.** One pass per invocation; planned token and decoded-byte
  volume, since consecutive merges re-plan a growing block, was first fixed at
  2^28 units and now scales with the stream like R15's
  ([follow-up](entropy-cut-validation.md#follow-up-work-that-scales-with-the-stream));
  the stop is polled before each pair, and completed merges are kept.
- **Placement.** Once the sweeps settle: after Default's sweep and its
  repeated finish, and in Max when no further sweep can start or a sweep
  changed nothing. A win slides once more (R1c), then runs R15 once more with
  its own slide, so a merged block can be cut where its statistics change. In
  Max, a merge win counts as an improvement for R12. Max's slid Default
  endpoint ends the same way.

## Measurements

Paired runs alternate the baseline and R16 per input in four lanes, at the
180-second allowance, and compare complete files. No Default run reached its
deadline.

| Set | Files | Smaller / larger | Bytes saved | Process CPU |
| --- | ---: | --- | ---: | --- |
| Raw streams | 365 | 1 / 0 | 1 | 44.02 → 44.28 s |
| PNG: medium, large, small, imageworsener, pkmn-col | 1,052 | 91 / 1 | 35,117, 1 lost | 1,024.63 → 1,058.62 s |
| GZIP, zlib, ZIP and APNG | 202 | 24 / 0 | 4,639 | 536.09 → 554.99 s |
| Relaxed: raw streams and 16 PNG | 381 | 15 / 0 | 4,393 | 150.65 → 153.46 s |

On PNG, 8 of 21 large images saved 18,682 bytes, 14,779 of them on the deft4j
copy of the Partnership card, and 83 of 387 medium images 16,434 bytes, led by
both copies of `319057651-….png` (2,150 each) and `download_webp…png` (1,945
each); small, imageworsener and pkmn-col images did not change. Among
containers, 15 of 21 GZIP files saved 4,294 bytes, the linux-static `kzipmix`
tar.gz copies 438–1,255 bytes each, and 9 ZIP files 345 bytes.

The one larger PNG, `81817-2-deflopt.png`, is not an R16 effect: its second
stream stops at a search slice, and five serial runs of each build gave
5,747–5,748 bytes for the baseline and 5,746–5,748 with R16.

**Cost.** CPU rose 0.6% on raw streams and 1.9–3.5% on the other sets. The
merge pass is linear; most of the time goes to the slide and the R15 pass that
follow a win, which visit every block. Serial runs on a quiet machine:

| File | Baseline | R16 | Bytes saved |
| --- | ---: | ---: | ---: |
| Partnership card | 35.78 s | 39.32 s | 1,528 |
| Partnership card, deft4j copy | 19.84 s | 24.22 s | 14,779 |
| `kzipmix-20200115-linux-static.tar.gz` | 14.28 s | 16.71 s | 1,248 |
| `319057651-….png` | 2.02 s | 2.19 s | 2,150 |
| `download_webp__260×280_.png` | 1.88 s | 2.04 s | 1,945 |
| `OwlAlpha-0.5.png` | 1.39 s | 1.54 s | 117 |

On the tar.gz, the merges took 0.86 s and saved 984 bytes; the slide and the
R15 pass with its slide that follow took 2.97 s and saved 264 bytes more.

**Max.** On the 25-PNG Max sample at 60 seconds, two lanes, five files were
smaller by 6,091 bytes (the deft4j Partnership card 5,821) and four larger by
13 bytes; 24 runs per arm used their full allowance, and process CPU was
3,057.38 → 3,041.60 s. None of the four losses reproduced: two further runs of
each, with both builds side by side, gave identical sizes for both builds in
every run (`floor pattern.png` 519,633 bytes; `4.2.07-deft4j-t8s.PNG`
725,417; `4.2.07-deflopt.png` 725,413 and 725,414; `floor
pattern-deft4j-t182s.png` 521,057 and 521,023).

## Validation

- `merges_neighbors_that_share_their_statistics` merges two blocks of one
  literal regime into one smaller block in strict and relaxed modes, with
  strictly compatible codes and the same decoded bytes; a reached stop returns
  `None`.
- `keeps_neighbors_whose_statistics_differ` keeps two blocks with disjoint
  literal alphabets apart.
- `merges_chain_and_stop_at_stored_blocks` merges three blocks into one and
  two into one on either side of a stored block, which stays stored.
- `completed_merges_survive_an_exhausted_budget` keeps the first of two merges
  when the work budget covers exactly one pair, and none when it covers less.
- `terminal_merges_follow_the_settled_finish` checks that R16 alone, the whole
  Default sweep, and Max's slid Default endpoint each end with two
  same-statistics blocks merged into one.
- Python's `zlib`, `gzip` and `zipfile` decoded every output of both arms and
  compared it with its source, including PNG chunk CRCs and APNG frames: 730
  raw, 2,104 PNG, 388 container, 762 relaxed and 50 Max outputs, with no
  mismatch. Python cannot decode three legacy-method ZIP sources, and Columbo
  rejects two ZIP layouts in both arms.
- Formatting, Clippy and all 673 Rust tests pass, including the
  private-corpus regressions in debug and release, with 13 Python tests and a
  locked build.
- The release executable grows from 1,877,008 to 1,877,024 bytes.

Harness, lists, arm crates and logs: `work/terminal-merge-validation/`; the
probes and estimators: `work/after-r15-probes/`.
