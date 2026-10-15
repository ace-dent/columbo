<!-- SPDX-License-Identifier: MIT -->

# Source-layout search: validation

Date: 15 October 2026. Baseline: `5b16e33`. Extends D4: each source block,
seeded by its length-family chain, now gets the no-split route's narrow
whole-block search. The
[route catalog](../routes-and-methods.md#route-gate-reference) describes the
method; [source family chains](source-family-chain-validation.md) describes
the chain.

## Why it saves bits

D4 keeps the source's block layout, where Default's planner starts from a
different partition. The length-family chain re-spells each block, but it only
expands matches: it does not try Default's other token searches on the
source block, such as the cumulative length bands for large blocks,
same-distance runs, or feedback trees. Max's no-split route (M2) runs these
searches on the source's own blocks and reaches a different basin. Seeded with
the chain, the same search starts from the better spelling.

## Origin

After `5b16e33`, Max still beat Default by 18,754 bytes on the 60-second Max
sample. Most of it was `nerd.png`: 6,741–6,775 bytes on two copies and 1,768 on
its deft4j copy, with `download_webp` 1,114–1,150 bytes. On `nerd.png`, Max's
no-split route alone reached 403,084 bytes, against Default's final 409,608.

Probes priced M2's route in Default as D4's builder, against `5b16e33`, on the
116 PNGs where D4 can apply:

| Builder | Smaller / larger | Bytes | CPU on the 116 |
| --- | --- | ---: | --- |
| M2's route: deft4j seeds, long-run merges | 27 / 0 | −26,099 | +43% |
| M2's route with chain seeds | 24 / 0 | −25,282 | +16% |
| **Chain seeds, narrow search, no merges** | **11 / 0** | **−19,610** | **+4.9%** |

Seeding with the chain kept the deft4j seeds' bytes within a few hundred per
file and cut the build to a third or less (`BNDT_on_X…X.png` 8.87 → 3.12 s).
The long-run merges added 5,672 bytes for 58 s, most of it on streams that did
not improve (`FsqwhPuaIAIlojU.png` +8 s, each `16million-pschmidt.png` copy
+5 s). Without a seed, the search alone missed `download_webp` entirely.

## Design

- **Per block.** Each nonempty source block's seed is the smaller of its
  ordinary plan and its length-family chain. The narrow whole-block search
  runs from it: the ordinary planner's search without match groups, ordered
  queues or replay, plus the large-source length bands; token
  transformations still start from the source tokens, and the seed stays the
  incumbent unless beaten.
- **No merges.** Blocks keep the source layout; R16 can still merge them
  during the finish.
- **Unchanged.** D4's gate, the strictly-smaller check before the finish, the
  second finish, and Max's mandatory D4 lineage.

## Measurements

Paired runs alternate the baseline and the candidate per input in four
lanes, at the 180-second allowance, and compare complete files. No output
grew in any Default set.

| Set | Files | Smaller / larger | Bytes saved | Process CPU |
| --- | ---: | --- | ---: | --- |
| Raw streams | 365 | 0 / 0 | 0 | 48.74 → 48.87 s |
| PNG: medium, large, small, imageworsener, pkmn-col | 1,052 | 11 / 0 | 19,600 | 1,183.84 → 1,209.40 s |
| GZIP, zlib, ZIP and APNG | 202 | 0 / 0 | 0 | 583.37 → 585.63 s |
| Relaxed: raw streams and 16 PNG | 381 | 7 / 0 | 17,241 | 172.96 → 181.82 s |

On PNG, both `nerd` copies saved 6,281 bytes (1.5%) and its deft4j copy 1,723;
both `floor pattern` copies 1,093–1,199, on top of D4's chains; the deft4j and
deflopt copies of `TruePNG calling-cleric` 926 and 361; both `BNDT` copies
516–568; and both `download_webp` copies 316–336.

**Cost.** CPU rose 0.3% on raw streams, 0.4% on containers, 2.2% on PNG and
5.1% on relaxed inputs, about half of it on the winners. Serial runs on a
quiet machine:

| File | Baseline | Candidate | Bytes saved |
| --- | ---: | ---: | ---: |
| `nerd.png` | 4.31 s | 6.93 s | 6,281 |
| `floor pattern.png` | 17.58 s | 17.97 s | 1,093 |
| `BNDT_on_X…X.png` | 10.04 s | 11.06 s | 568 |
| `TruePNG calling-cleric-deflopt.png` | 1.99 s | 2.63 s | 361 |
| `download_webp__260×280_.png` | 3.47 s | 3.47 s | 336 |
| `FsqwhPuaIAIlojU.png` | 16.86 s | 17.34 s | 0 |
| `ss1.png` | 15.43 s | 15.42 s | 0 |
| `IMG_8555.PNG` | 15.09 s | 15.67 s | 0 |
| `16million-pschmidt.png` | 3.24 s | 3.48 s | 0 |

An eligible stream that does not improve pays the narrow search, here at most
0.58 s.

**Max.** On the 25-PNG Max sample at 60 seconds, both `floor pattern` copies
saved 990–1,003 bytes, and two files grew by 7 bytes (`4.2.07-deft4j-t8s.PNG`
5, `nascar.png` 2); both tied in two further side-by-side runs of each build.
Every Max output was no larger than its Default output, and Max's margin over
Default on the sample fell from 18,635 to 3,392 bytes. At equal 10- and
20-second allowances, Max was no larger than Default on `nerd.png` and
`floor pattern.png`.

## Validation

- D4's gate and the length-family chain keep their tests; the narrow search
  is the no-split route's existing search.
- Python's `zlib`, `gzip` and `zipfile` decoded every candidate output and
  compared it with its source, including PNG chunk CRCs and APNG frames: 365
  raw, 1,052 PNG, 194 container, 381 relaxed and 25 Max outputs, with no
  mismatch. Python cannot decode three legacy-method ZIP sources, and Columbo
  rejects two ZIP layouts in both arms.
- Formatting, Clippy and all 680 Rust tests pass, including the
  private-corpus regressions in debug and release, with 13 Python tests and a
  locked build.
- The release executable keeps its size, 1,893,552 bytes.

Harness, lists, arm crates and logs: `work/source-layout-validation/`.
