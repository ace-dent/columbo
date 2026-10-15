<!-- SPDX-License-Identifier: MIT -->

# Source family chains: validation

Date: 15 October 2026. Baseline: `8463a1d`. Implemented as D4 with the
length-family chain; the
[route catalog](../routes-and-methods.md#route-gate-reference) describes the
method.

## Why it saves bits

Default's planner prunes matches one at a time: it expands each match whose
literals cost no more under the current codes, rebuilds the codes, and
repeats. It stops once every remaining match is cheaper than its literals on
its own. Expanding every match of one length symbol together can still pay:
the symbol leaves the alphabet, the rebuilt codes give its code space to the
other symbols, and under them further matches become dearer than their
literals, so pruning starts again. deft4j's least-expensive family transform
takes this step. The planner never does for large blocks: its cumulative
short-length bands expand only the shortest lengths, and R5 stops at 8,192
tokens.

## Origin

In the 60-second Max sample, Max beat Default on 22 of 25 PNGs by 75,648
bytes, mostly through the deft4j-derived source routes M1 and M2.
`floor pattern.png` shows the gap. Its source has 25 blocks of 16,383 tokens
and 303,573 matches. Default ended with 84 blocks, 34,699 matches and
4,333,880 bits; M1's lineage ended with 26 blocks, 37,646 matches and
4,160,240 bits, 4.0% fewer, with header bits down from 33,419 to 11,641.

The first attempt priced M1 in Default (D4 with the deft4j-derived graph and
its greedy merges), finished it beside the planner's endpoint when it was
already smaller, and kept the smaller. Paired against `8463a1d`, it saved
66,314 bytes on 21 of 1,052 PNGs and 60,893 on 6 of 381 relaxed inputs, with
no output larger, but CPU rose 24.7% and 38%. The graph costs about 8 µs per
token, and its greedy merges re-plan an ever larger block: the build took
13.7 s on `IMG_8555.PNG` (8.7 s of it merging) and 12.1 s on
`16million-pschmidt.png` (11.9 s), and neither improved.

Tracing which graph states won showed where the gain comes from. On
`floor pattern.png`, each of the largest per-block wins, 45–51 kbit on a
16,383-token block, began by expanding a long-length family (symbols
272–285), followed by about ten prune passes. On
`download_webp__260×280_.png`, the same pattern saved 9–28 kbit on seven of
its eight 16,383-token blocks, while Default's planner on the same nine
source blocks saved at most 686 bits on seven of them (18,225 and 1,957 on
the other two). The planner's own pruning loop, rerun with deft4j's heap
codes, found nothing more: without a family expansion first, almost no match
is dearer than its literals.

## Design

- **Length-family chain.** For one block, a prune chain expands every match
  no cheaper than its literals under the current codes, then rebuilds
  deft4j's heap codes from the new counts, up to 32 times. The first chain
  starts from the block's own codes. Then, up to three times, every match of
  the length symbol whose matches cost least as literals in total is
  expanded and another chain runs. Matches are priced as the table replay
  prices them (an absent code counts 15 bits). Every state is planned with
  the ordinary planner and with the heap codes, and the smallest plan is
  kept only if strictly smaller than the block's ordinary plan.
- **D4.** Default keeps the source's own block layout and gives each
  nonempty block the smaller of its ordinary plan and its chain. Only if
  that candidate is strictly smaller than the selected candidate before the
  finish is it finished through R1–R17 as well, and the smaller endpoint is
  kept, so the planner's endpoint is never lost. A reached stop leaves no
  candidate.
- **Gate.** Sources in M2's class (≤1 MiB compressed, 2–128 nonempty fixed or
  dynamic blocks) that decode to at least 128 KiB, after D1–D3 and the
  terminal tree floors, under G0.
- **Max.** Max's own routes use M1, whose graph explores the same transforms
  more widely, but within a short allowance M1 may not finish. Max therefore
  runs D4 in its complete Default floor as mandatory work: both lineages get
  R1 and the mandatory Default finish, and the lineage whose Default endpoint
  is smaller is kept. Without this, Max at 20 seconds ended
  `floor pattern.png` at 519,882 bytes, above Default's 519,736 at the same
  allowance; with it, Max reached 519,736 at both 10 and 20 seconds.

Placements and gates, against `8463a1d`; all but the retained row predate the
chain's first prune chain from the block's own codes:

| Variant | 33-file sample: smaller / larger, bytes, CPU | 120 random PNGs: smaller / larger, bytes, CPU |
| --- | --- | --- |
| D4 by M1's graph, gated | 5 / 0, −12,791, +19.7% | 1 / 1, −22, +2.3% |
| Chain in every planner search, plus R17 | 9 / 3, −8,455, +19.6% | 10 / 8, −66, +8.3% |
| Chain in R17 only | 5 / 0, −4,397, within noise | not run |
| **D4 by chains, gated (retained)** | **2 / 0, −7,223, +3.5%** | **2 / 0, −4, +1.4%** |
| D4 by chains, every Huffman source | 2 / 0, −7,089, +7.3% | 4 / 2, −35, +5.2% |

The retained form's two 120-PNG changes are deadline-bound APNG stickers;
before the first prune chain was added, the same two grew by 3 and 24 bytes.
On the 116 PNGs where the graph form changed time or size, the chains saved
58,611 bytes for 37.5 s more CPU, against 66,314 bytes for 276 s in the
graph form's suite run.

## Measurements

Paired runs alternate the baseline and the candidate per input in four
lanes, at the 180-second allowance, and compare complete files. No output
grew in any Default set.

| Set | Files | Smaller / larger | Bytes saved | Process CPU |
| --- | ---: | --- | ---: | --- |
| Raw streams | 365 | 0 / 0 | 0 | 42.88 → 42.95 s |
| PNG: medium, large, small, imageworsener, pkmn-col | 1,052 | 7 / 0 | 58,615 | 1,097.18 → 1,146.79 s |
| GZIP, zlib, ZIP and APNG | 202 | 0 / 0 | 0 | 569.27 → 574.46 s |
| Relaxed: raw streams and 16 PNG | 381 | 4 / 0 | 57,059 | 149.30 → 169.11 s |

On PNG, both `floor pattern` copies saved 22,139 and 21,930 bytes (4.1%),
both `download_webp` copies 6,518–6,526 (3.0%), both `BNDT` copies 705–793,
and a deft4j copy of `Home___X` 4 bytes.

**Cost.** CPU was unchanged on raw streams and rose 0.9% on containers, 4.5%
on PNG and 13.3% on relaxed inputs, whose small set is dominated by the four
winners. Serial runs on a quiet machine:

| File | Baseline | D4 | Bytes saved |
| --- | ---: | ---: | ---: |
| `floor pattern.png` | 11.61 s | 17.44 s | 22,139 |
| `download_webp__260×280_.png` | 2.32 s | 3.44 s | 6,518 |
| `BNDT_on_X…X.png` | 7.37 s | 9.96 s | 705 |
| `IMG_8555.PNG` | 14.51 s | 14.94 s | 0 |
| `16million-pschmidt.png` | 2.90 s | 3.23 s | 0 |
| `ss1.png` | 15.26 s | 15.29 s | 0 |
| Partnership card (not eligible) | 42.07 s | 42.08 s | 0 |

Most of a winner's extra time is the second finish: on `floor pattern.png`
the chains take 0.70 s of its 5.83 s. An eligible stream that does not win
pays only the chains, here at most 0.43 s.

**Max.** On the 25-PNG Max sample at 60 seconds, before D4 joined Max's
floor, three files saved 8 bytes and none grew, and every Max output was no
larger than its Default output; but the 20-second check above found the
short-allowance gap. With D4 in Max's floor, two files saved 63 bytes and two
grew by 9 (`floor pattern.png` 6, `4.2.03.PNG` 3); both tied in two further
side-by-side runs of each build. Every Max output was again no larger than
its Default output. Max's margin over Default on the sample fell from 75,813
to 18,754 bytes, since Default now reaches most of what M1 found.

## Validation

- `least_expensive_length_symbol_totals_each_family` checks the family
  choice: the length symbol whose matches together cost least, or save
  most, as literals, and none without matches.
- `length_family_chain_respells_a_block_below_its_bound` checks that the
  chain returns a strictly smaller plan with the same bytes on a block whose
  matches cost more than their literals, and nothing at or above its bound
  or after a reached stop.
- `default_source_topology_uses_the_no_split_class_above_a_decoded_floor`
  checks D4's gate.
- Python's `zlib`, `gzip` and `zipfile` decoded every candidate output and
  compared it with its source, including PNG chunk CRCs and APNG frames: 365
  raw, 1,052 PNG, 194 container, 381 relaxed and 25 Max outputs, with no
  mismatch. Python cannot decode three legacy-method ZIP sources, and Columbo
  rejects two ZIP layouts in both arms.
- Formatting, Clippy and all 680 Rust tests pass, including the
  private-corpus regressions in debug and release, with 13 Python tests and a
  locked build.
- The release executable grew by 16 bytes, to 1,893,552.

Harness, lists, arm crates and logs: `work/family-chain-validation/`; the
graph-built D4 run: `work/source-topology-validation/`.
