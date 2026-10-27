<!-- SPDX-License-Identifier: MIT -->

# Terminal token replay: validation

Date: 14 October 2026. Baseline: `d3550e1`. Implemented as R17; the
[route catalog](../routes-and-methods.md#terminal-token-replay-r17) describes
the method.

## Why it saves bits

The main planner chooses each block's token spelling (which matches stay,
which become literals) together with its trees. R13–R16 then re-cut, merge and
re-tree the blocks after that search has finished, so the final blocks carry
spellings chosen for other blocks and other codes. Under a block's own final
codes some matches can cost more than their literals, and expanding groups of
matches and refitting the trees can pay even where no single match does.

## Origin

A second complete Default pass over R16 outputs still made 114 of 153 files
smaller by 13,515 bytes. On `download_webp__260×280_.png` the second pass's
main planner alone saved 5,088 bits with almost the same blocks: it turned
2,234 matches into 10,289 literals and cut header bits from 8,905 to 8,655.
R1 re-spells under fixed trees too, but it runs before R2–R5 and only below
128 KiB.

Probes against `d3550e1` replayed the existing extended Default floor
(`plan_block_with_floor`), with each block's final codes as source codes,
after R16:

| Probe, Default | 33-file sample: smaller / larger, bytes, CPU | 120 random PNGs: smaller / larger, bytes, CPU |
| --- | --- | --- |
| Every block | 16 / 0, −4,136, +4–8% | 53–54 / 0–1, −1,380, +13–20% |
| Only blocks with a match dearer than its literals | 15 / 0, −685, +1.7% | 46 / 0, −1,233, +3.9% |
| **Every block of ≥4,096 tokens; smaller ones only with a dearer match (R17)** | **15 / 0, −4,065, +2.8%** | **47 / 1, −1,226, +4.0%** |

On `download_webp` the replay saved 3,433 bytes (1.6%), more than five times
the second pass, because the floor's grouped expansions refit large blocks'
trees; on animated stickers, whose frames hold many small blocks, the
unconditional floor added little for its time, so small blocks need a match
that is individually dearer than its literals.

## Design

- **Per block.** Dynamic blocks with at most 50,000 tokens and 10 MB decoded,
  the extended floor's own limits. The floor runs on the parsed block, whose
  current codes are its source codes, at the block's real alignment, and its
  plan is kept only if strictly smaller than the block as encoded and not
  stored.
- **Gate.** Blocks of 4,096 tokens or more always get the floor. A smaller
  block gets it only if some match costs strictly more than its literals
  under its codes, priced as the table replay prices it (an absent code counts
  15 bits).
- **Placement.** After R16 and its R15 follow-up, once the sweeps settle; a
  win slides once more. Max uses the same gate, since the ungated floor's extra
  savings on small blocks came at a poor rate per second, and an R16 or R17
  win lets R12 sweep again. Max's slid Default endpoint ends the same way.

## Measurements

Paired runs alternate the baseline and R17 per input in four lanes, at the
180-second allowance, and compare complete files. No output grew in any
Default set, and no Default run reached its deadline.

| Set | Files | Smaller / larger | Bytes saved | Process CPU |
| --- | ---: | --- | ---: | --- |
| Raw streams | 365 | 8 / 0 | 20 | 49.67 → 49.97 s |
| PNG: medium, large, small, imageworsener, pkmn-col | 1,052 | 135 / 0 | 16,197 | 1,255.94 → 1,282.20 s |
| GZIP, zlib, ZIP and APNG | 202 | 77 / 0 | 2,686 | 691.46 → 726.95 s |
| Relaxed: raw streams and 16 PNG | 381 | 21 / 0 | 10,527 | 212.28 → 223.09 s |

On PNG, 123 of 387 medium images saved 12,144 bytes, led by both
`download_webp` copies (3,398–3,399 each), both `floor pattern` copies (1,319
each) and three `FsqwhPuaIAIlojU` copies (306–567); six large images saved
4,037, 3,683 of them on the deft4j copy of the Partnership card. Among
containers, 20 of 21 GZIP files saved 1,579 bytes and 17 APNG files 840.

**Cost.** CPU rose 0.6% on raw streams, 2.1% on PNG and 5.1% on containers
and relaxed inputs. Serial runs on a quiet machine:

| File | Baseline | R17 | Bytes saved |
| --- | ---: | ---: | ---: |
| `download_webp__260×280_.png` | 2.05 s | 2.33 s | 3,399 |
| Partnership card, deft4j copy | 31.18 s | 33.20 s | 3,683 |
| `floor pattern.png` | 11.02 s | 11.65 s | 1,319 |
| `FsqwhPuaIAIlojU.png` | 14.21 s | 16.41 s | 567 |
| `kzipmix-20200115-linux-static.tar.gz` | 17.53 s | 20.12 s | 381 |

The tar.gz is the least efficient case: its many blocks of 4,096 tokens or
more all get the floor for a small saving.

**Max.** On the 25-PNG Max sample at 60 seconds, 11 files saved 4,376 bytes
(the deft4j Partnership card 3,683) and two grew by 78; neither growth
reproduced in two further side-by-side runs of each build
(`floor pattern-deft4j-t182s.png` 521,009 for the baseline and 520,997 with
R17, then 520,957 for both; `4.2.07-deflopt.png` 725,413 for both twice).
Every Max output was no larger than the same file's Default output.

## Validation

- `dearer_matches_are_found_with_the_table_replay_estimate` checks the gate's
  cost model: a match whose literals are cheaper is dearer, and neither
  expensive literals nor an absent literal (15 bits) makes it so.
- `terminal_token_replay_respells_a_block_under_its_codes` emits a block whose
  explicit codes make some matches dearer than their literals, parses it back,
  and checks that R17 returns a strictly smaller plan with the same bytes, that
  a reached stop returns none, and that a same-size block without a dearer
  match is left alone.
- Python's `zlib`, `gzip` and `zipfile` decoded every candidate output and
  compared it with its source, including PNG chunk CRCs and APNG frames: 365
  raw, 1,052 PNG, 194 container, 381 relaxed and 25 Max outputs, with no
  mismatch. Python cannot decode three legacy-method ZIP sources, and Columbo
  rejects two ZIP layouts in both arms.
- Formatting, Clippy and all 677 Rust tests pass, including the
  private-corpus regressions in debug and release, with 13 Python tests and a
  locked build.
- The release executable keeps its size, 1,893,536 bytes.

Harness, lists, arm crates and logs: `work/token-replay-validation/`; the
probes: `work/after-r15-probes/`.
