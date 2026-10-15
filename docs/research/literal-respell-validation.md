<!-- SPDX-License-Identifier: MIT -->

# Re-spell from literals: validation

Date: 15 October 2026. Baseline: `d4a6960`. Adds a third seed to D4's
per-block search; the
[route catalog](../routes-and-methods.md#route-gate-reference) describes the
method, and the [source-layout search](source-layout-search-validation.md)
the search it seeds.

## Why it saves bits

Every match-pruning loop in Columbo is one-way: it starts from the source's
matches, expands those no cheaper than their literals under the current codes,
rebuilds the codes, and repeats; an expanded match never returns. Under codes
shaped by many matches, a short match can stay just cheaper than its literals
even when a spelling with fewer matches, and literal codes shaped for it,
would be smaller overall. Starting from the other end avoids that basin. With
codes built from the block's bytes as literals, no match has a code, so only
matches that clearly pay survive the first pass; their counts then give
matches codes, and each later pass re-decides every source match under the
new codes, so a match can return. The pass repeats until the spelling repeats.

## Origin

After `d4a6960`, Max beat Default by only 3,392 bytes on the 25-PNG Max sample,
and by 1,480 bytes on a stratified sample of 40 containers (APNG files 31–303
bytes each, GZIP files tied), so no Max route still hid a large gap. Probes
for new headroom:

| Probe | Result |
| --- | --- |
| Length-family chain in R17, on top of D4 | 63 bytes on 33 files; 215 on every block for +3.5% CPU |
| Restoring aligned source matches under final codes, any size | 37 bytes of headroom on 16 large outputs |
| D4 without M2's class bounds | no change on 10 large streams, +18% CPU on them |
| Two-way re-spell from the source's codes, as a D4 seed | 3 bytes on 4 probe files |
| **Two-way re-spell from literal codes, as a D4 seed** | **18,928 bytes on 19 of 116 eligible PNGs, one 40-byte loss, +5% CPU on them** |

Changes under fixed codes are exhausted: the finish leaves no aligned source
match worth restoring. Only a different spelling with different codes still
pays, and of the two starting points only the literal one reaches it.

## Design

- **Re-spell from literals.** For one block with matches: start from
  deft4j's heap codes for its bytes as literals and no distance codes. Each
  pass keeps exactly the source matches cheaper than their literals under the
  current codes (an absent code counts 15 bits), plans the state exactly with
  the ordinary planner and the heap codes, and rebuilds the heap codes from
  the new counts; it stops when the spelling repeats or after 32 passes. The
  smallest plan is kept only if strictly below the block's best seed so far.
- **D4.** Each nonempty source block's seed is now the smallest of its
  ordinary plan, its length-family chain and its re-spell from literals; the
  narrow whole-block search runs from it as before. Gate, finish and Max's
  mandatory D4 lineage are unchanged.

## Measurements

Paired runs alternate the baseline and the candidate per input in four
lanes, at the 180-second allowance, and compare complete files.

| Set | Files | Smaller / larger | Bytes saved | Process CPU |
| --- | ---: | --- | ---: | --- |
| Raw streams | 365 | 0 / 0 | 0 | 62.19 → 62.39 s |
| PNG: medium, large, small, imageworsener, pkmn-col | 1,052 | 19 / 1 | 18,968 − 40 | 1,244.25 → 1,269.73 s |
| GZIP, zlib, ZIP and APNG | 202 | 0 / 0 | 0 | 580.35 → 586.11 s |
| Relaxed: raw streams and 16 PNG | 381 | 6 / 1 | 3,696 − 41 | 183.77 → 182.01 s |

On PNG, the three `frymire` copies saved 2,814–2,820 bytes each (0.8%), the
three `TruePNG calling-cleric` copies 1,056–1,828, both `nerd` copies 991 and
its deft4j copy 382, both `download_webp` copies 640–645, both `BNDT` copies
397–425, both `Matrix` copies 284–285, the three `LevelLoading` copies 258,
and the deflopt `floor pattern` 52.

**The loss.** `floor pattern.png` grew by 40 bytes, deterministically in both
modes: on some of its blocks the re-spell is the smallest seed before the
finish, but the finished D4 endpoint is 40 bytes larger than the one the
chain seeds reached. Avoiding it would mean finishing a D4 candidate per seed.

**Cost.** CPU rose 0.3% on raw streams, 1.0% on containers and 2.0% on PNG,
and was within noise on relaxed inputs. Serial runs on a quiet machine:

| File | Baseline | Candidate | Bytes saved |
| --- | ---: | ---: | ---: |
| `frymire.PNG` | 2.33 s | 3.37 s | 2,814 |
| `TruePNG calling-cleric.png` | 2.27 s | 3.30 s | 1,828 |
| `nerd.png` | 6.89 s | 6.47 s | 991 |
| `download_webp__260×280_.png` | 3.45 s | 3.52 s | 645 |
| `Matrix.png` | 3.08 s | 3.47 s | 284 |
| `LevelLoading.png` | 0.94 s | 1.37 s | 258 |
| `floor pattern.png` | 17.83 s | 17.54 s | −40 |
| `IMG_8555.PNG` | 15.59 s | 15.88 s | 0 |
| `ss1.png` | 15.26 s | 15.27 s | 0 |
| `FsqwhPuaIAIlojU.png` | 17.14 s | 17.42 s | 0 |

An eligible stream that does not improve pays the re-spell passes, here at
most 0.29 s.

**Max.** On the 25-PNG Max sample at 60 seconds, five files saved 2,097 bytes
(the three `nerd` copies 345–864) and two grew by 65: `floor pattern.png` by
the same deterministic 40 bytes, through Max's mandatory D4 lineage, and its
deft4j copy by 25, which did not reproduce in two side-by-side runs. Every
Max output was no larger than its Default output; Max's margin over Default
on the sample fell from 3,423 to 1,794 bytes.

## Validation

- `respell_from_literals_keeps_only_paying_matches_below_its_bound` checks
  that the re-spell returns a strictly smaller plan with the same bytes on a
  block whose matches cost more than their literals, nothing at or above its
  bound or after a reached stop, and nothing for a block without matches.
- Python's `zlib`, `gzip` and `zipfile` decoded every candidate output and
  compared it with its source, including PNG chunk CRCs and APNG frames: 365
  raw, 1,052 PNG, 194 container, 381 relaxed and 25 Max outputs, with no
  mismatch. Python cannot decode three legacy-method ZIP sources, and Columbo
  rejects two ZIP layouts in both arms.
- Formatting, Clippy and all 681 Rust tests pass, including the
  private-corpus regressions in debug and release, with 13 Python tests and a
  locked build.
- The release executable grew by one 16 KiB page of segment alignment, to
  1,910,064 bytes.

Harness, lists, arm crates and logs: `work/literal-respell-validation/`.
