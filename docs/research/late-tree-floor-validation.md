<!-- SPDX-License-Identifier: MIT -->

# Late tree floor: validation

Date: 16 October 2026. Baseline: `4af8ff1`. Implemented as R18; the
[route catalog](../routes-and-methods.md#route-gate-reference) describes the
method.

## Why it saves bits

The terminal tree floors price two families of Huffman trees for every
completed block: Brotli and Zopfli smoothed trees for compact streams, and the
bounded-depth frontier, which exactly prices depth-limited trees at every
ceiling from the first feasible one through 14. Default runs them once, before
R1. Every later step that changes blocks then leaves the new blocks without
the bounded-depth family: R13 drops distance symbols, R15 cuts blocks, R16
merges them, R17 re-spells them, and D4 replaces the whole spelling. R14
prices the smoothed family in the finish, but no finish step prices depth
limits. APNG Default frames skip the first floors altogether, to keep
per-frame cost down, so their trees never see a depth limit. Running the floors
once more at the end of the finish closes both gaps for the price of one pass.

## Origin

With the misses and regressions of the 16 October refresh explained, three
probes looked for new headroom:

| Probe | Result |
| --- | --- |
| D4 for single-block sources, or with a 32 KiB decoded floor | Nothing on 150 random PNGs; −29 to +17 bytes on 120 APNG-heavy PNGs |
| D4 in the planner's block layout, finished beside D4 | −3,083 bytes on 17 of 116 D4-eligible PNGs for 14% more CPU on them |
| A second Default pass over Default's output, 33 mixed files | −1,424 bytes on 26 |

Tracing the second pass showed the bounded-depth tree floor and R14 taking a
large share: on `OwlAlpha-0.5.png` the floor alone saved 94 of the second
pass's 100 bytes. Running the tree floors after R17, with no other change:

| Sample | Smaller / larger | Bytes | CPU |
| --- | --- | ---: | --- |
| 33 mixed files | 13 / 0 | −240 | +1.4% |
| 150 random PNGs | 23 / 0 | −453 | +1.3% |
| 120 APNG-heavy PNGs | 43 / 0 | −4,017 | +1.1% |

A slide after a late tree-floor win added nothing (net +1 byte on the 120).

## Design

- **R18.** After R17 in Default, and after Max's last sweep, the terminal tree
  floors run once more on the final candidate: the compact payload-tree floor
  and its balanced closure for streams of at most 8 KiB compressed and
  128 KiB decoded, and the bounded-depth frontier unless the compact pass
  covered it, with Max's hard-stop rescue. A result is kept only if strictly
  smaller.
- **Max's Default endpoints.** `slid_default_endpoint`, Default's exact finish
  inside Max's mandatory floors, ends with R18 too, so Max never ends above
  Default.

## Measurements

Paired runs alternate the baseline and the candidate per input in four
lanes, at the 180-second allowance, and compare complete files. Another
session's builds loaded the machine during the PNG run, so its wall times are
inflated; both arms shared the load. No output grew in any Default set.

| Set | Files | Smaller / larger | Bytes saved | Process CPU |
| --- | ---: | --- | ---: | --- |
| Raw streams | 365 | 18 / 0 | 96 | 58.61 → 58.69 s |
| PNG: medium, large, small, imageworsener, pkmn-col | 1,052 | 174 / 0 | 2,768 | 1,884.64 → 1,886.90 s |
| GZIP, zlib, ZIP and APNG | 202 | 71 / 0 | 2,091 | 727.93 → 740.82 s |
| Relaxed: raw streams and 16 PNG | 381 | 32 / 0 | 372 | 275.21 → 279.79 s |

The largest PNG savings are the three `IMG_8555` copies (94–148 bytes), two
`FsqwhPuaIAIlojU` copies (127–138), the three `OwlAlpha-0.5` copies (96 each),
three `frymire` and three `micropython const` copies (66–73). Among
containers, every `kzipmix` and `pngout` GZIP improves, by up to 430 bytes on
the 2.3 MB `kzipmix-20200115-linux-static.tar.gz`; APNG files and ZIP archives
gain tens of bytes.

**Cost.** CPU rose 0.1% on raw streams and PNG, 1.8% on containers and 1.7%
on relaxed inputs. Serial runs:

| File | Baseline | R18 | Bytes saved |
| --- | ---: | ---: | ---: |
| `FsqwhPuaIAIlojU.png` | 19.30 s | 19.09 s | 138 |
| `IMG_8555.PNG` | 17.01 s | 17.08 s | 145 |
| `OwlAlpha-0.5.png` | 2.34 s | 2.34 s | 96 |
| `kzipmix-20200115-linux-static.tar.gz` | 21.56 s | 22.31 s | 423 |
| APNG sticker `1217060_193922_…png` | 9.28 s | 9.84 s | 1,070 |

**Max.** On the 25-PNG Max sample at 60 seconds, seven files saved 244 bytes
and two grew by 3: `nascar.png` +2, which tied in two side-by-side reruns,
and `Craig_Smith.png` +1, which repeats at 60 and 120 seconds while its
Default output falls by 11 bytes, a parent-path effect. Every Max output was
no larger than its Default output.

## Validation

- `terminal_merges_follow_the_settled_finish` now also checks that Max's
  Default endpoint, which ends with R18, leaves no strictly smaller
  bounded-depth tree.
- Python's `zlib`, `gzip` and `zipfile` decoded every candidate output and
  compared it with its source, including PNG chunk CRCs and APNG frames: 365
  raw, 1,052 PNG, 194 container, 381 relaxed and 25 Max outputs, with no
  mismatch. Python cannot decode three legacy-method ZIP sources, and Columbo
  rejects two ZIP layouts in both arms.
- Formatting, Clippy and all 681 Rust tests pass, including the
  private-corpus regressions in debug and release, with 13 Python tests and a
  locked build.
- The release executable keeps its size, 1,910,064 bytes.

Harness, lists, arm crates and logs: `work/late-tree-floor-validation/`.
