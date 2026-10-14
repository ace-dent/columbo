<!-- SPDX-License-Identifier: MIT -->

# Entropy cuts: validation

Date: 14 October 2026. Baseline: `2b40fb0`. Implemented as R15; the
[route catalog](../routes-and-methods.md#route-gate-reference) describes the
method.

## Why it saves bits

A Huffman block transmits one literal/length table and one distance table.
When a block's statistics change part-way, both regions are coded with a
compromise table. For fixed counts a Huffman code is already optimal, so
within one block the only remaining payload loss is integer code lengths;
coding each region with its own table can lower the payload by more than the
extra header costs.

Bit accounting of 30 completed Default PNG outputs (49.6 Mbit) shows where
the rest of the opportunity lies. Dynamic headers took 0.47% of all bits
(0.2–0.4% on large images, 2–13% on the smallest), extra bits 10.6%, and
payload codes 88.9%, of which 0.82% of all bits lay above the zeroth-order
entropy of each block's own counts. Tree and header methods with fixed tokens
and blocks can only work within the header share, which R2–R14 already
target. A cut changes the counts each table must serve, which none of them
can.

Default's own splitting is bounded for speed: a source block is probed at its
decoded eighths only when it holds at least 32 KiB, and the adaptive split,
entropy-state scout and alphabet-boundary pairs (R6) are Max-only. The
terminal slide (R1c) moves boundaries but never adds one.

## Origin

A Python estimate on 29 of those outputs scanned every finished Huffman block
for the cut that most lowered the two sides' zeroth-order entropy, charging
one more header at the block's own header cost. 53 of 295 blocks had a
positive estimate: 14,768 bits (0.053% of all bits) for one cut per block and
19,691 bits (0.071%) for two. `OwlAlpha-0.5.png` alone estimated 6,386 bits.

A probe build added one cut per block after R14, chosen at 256 checkpoints
with floating-point entropy and priced with R6's exact range planner, then
slid once more after a win:

| Probe, Default | Smaller / larger | Bytes saved | CPU |
| --- | --- | ---: | ---: |
| 33-file stratified sample | 10 / 0 | 4,765 | 131.7 → 134.7 s |
| 120 further random PNGs | 33 / 0 | 18,232 | 501.7 → 525.8 s |
| Same 120, one more finish pass after a win | 30 / 0 | 12,143 more | 553.2 → 572.4 s |

`OwlAlpha-0.5.png` saved 792 bytes, close to its estimate. Cutting again kept
finding savings, so R15 cuts each kept half recursively.

## Design

- **Exact scan.** One pass over a token range moves each token from the right
  side to the left, updating both sides' literal/length and distance
  histograms and their sums of `c log2 c`. Each side's zeroth-order entropy,
  with one end-of-block symbol, is then known at every token cut in constant
  time, so the scan is exact over all cuts rather than sampled.
- **Fixed point.** Logarithms use 16 fraction bits computed by repeated
  squaring of the normalized mantissa, with a table for counts below 4,096.
  Integer arithmetic keeps the chosen cut identical on every platform; no
  floating point reaches a compression decision.
- **Acceptance.** The lowest cut that saves at least 32 estimated bits, with
  at least 16 tokens per side, is priced with the bounded planner (R6's range
  pricing, in both modes) and kept only if the block's planned sequence,
  including bit alignment, is strictly smaller. Each kept half is cut the same
  way, left first.
- **Bounds.** Per pass, work units (tokens scanned and range prices' token
  and byte volumes) were first fixed at 2^28 and now scale with the stream
  ([follow-up](#follow-up-work-that-scales-with-the-stream)), with 4,096
  range prices; the stop is polled every 65,536 scanned tokens and before each
  price. Completed cuts are kept when either ends the search.
- **Placement.** After R14, as part of the linear finish: a win slides the
  blocks once more (R1c); in Default, a pass in which R13, R14 or R15 wins
  repeats all three once; Max's R12 sweeps revisit them while time remains.

## Measurements

Paired runs alternate the baseline and R15 per input in four lanes, at the
180-second allowance, and compare complete files. No output grew in any set,
and no Default run reached its deadline.

| Set | Files | Smaller / larger | Bytes saved | Process CPU |
| --- | ---: | --- | ---: | --- |
| Raw streams | 365 | 12 / 0 | 576 | 41.71 → 42.38 s |
| PNG: medium, large, small, imageworsener, pkmn-col | 1,052 | 115 / 0 | 153,436 | 958.40 → 1,024.97 s |
| GZIP, zlib, ZIP and APNG | 202 | 65 / 0 | 62,572 | 492.57 → 533.01 s |
| Relaxed: raw streams and 16 PNG | 381 | 21 / 0 | 6,226 | 127.16 → 137.33 s |

R14's validation saved 141, 27,472, 20,301 and 7,139 bytes on the same sets. On PNG, three
copies of the 2.7 MB Partnership card saved 127,257 bytes together, 51,560 on
the original (1.9%); 99 of 387 medium images saved 25,502 bytes, led by
`FsqwhPuaIAIlojU.png` (3,608), `OwlAlpha-0.5.png` (1,606) and `monarch.PNG`
(525); small and pkmn-col images saved 53 bytes together. Among containers,
all 21 GZIP files improved, by 55,879 bytes in all, the three linux-static
`kzipmix` tar.gz copies by 14,225–14,372 bytes each (0.6%).

**Cost.** CPU rose 1.6% on raw streams and 6.9–8.2% on the other sets. Serial
runs on a quiet machine show where:

| File | Baseline | R15 | Bytes saved |
| --- | ---: | ---: | ---: |
| Partnership card | 28.21 s | 35.70 s | 51,559 |
| `kzipmix-20200115-linux-static.tar.gz` | 9.98 s | 14.25 s | 14,372 |
| `FsqwhPuaIAIlojU.png` | 11.72 s | 13.24 s | 3,608 |
| `floor pattern.png` | 9.01 s | 10.15 s | 602 |
| `OwlAlpha-0.5.png` | 0.91 s | 1.40 s | 1,606 |
| `monarch.PNG` | 2.22 s | 2.66 s | 525 |

On the tar.gz, the first R15 pass took 1.53 s and saved 11,262 bytes. Its
slide and the repeated R13–R15 pass it triggers took 3.93 s more and saved
3,369 bytes, mostly from R13 (1,067) and R14 (2,034) on the new blocks.

**Max.** On the 25-PNG Max sample at 60 seconds, two lanes, R15 shortened 10
files by 7,671 bytes and grew none; 24 runs per arm used their full
allowance, and process CPU was 3,076.14 → 3,077.80 s. The deft4j Partnership
card saved 3,666 bytes and `FsqwhPuaIAIlojU.png` 3,440. One further file,
`4.2.07.PNG`, kept its size with different bytes.

## Validation

- `fixed_point_logarithm_matches_log2` checks exact powers of two,
  monotonicity to 100,000 and agreement with `f64::log2` within two units of
  the last fixed-point place on 10,000 random values up to 2^64.
- `moving_cut_entropy_matches_direct_histograms` checks the moving-cut sums
  against entropies recomputed from histograms at every 97th cut of 12,000
  random tokens, with counts beyond the 4,096-entry table.
- `best_entropy_cut_finds_a_regime_change_and_ignores_one_regime` finds the
  exact token where two literal regimes meet, also from an offset start, finds
  no cut in one regime or a too-short range, and returns `None` when the stop
  or budget ends the scan.
- `entropy_cuts_split_each_regime_into_its_own_block` cuts a block of three
  disjoint literal regimes into exactly those three blocks in strict and
  relaxed modes, the second cut coming from a kept half; the plans are smaller,
  decode to the same bytes and keep strictly compatible codes, and a reached
  stop returns `None`. Two more tests keep a completed cut when range prices
  run out and leave a one-regime block whole.
- `smoothed_tree_finish_follows_the_terminal_searches` now lets the whole
  sweep cut R14's blocks and checks that R13, R14 and R15 are at a fixed point
  after the Default sweep.
- Python's `zlib`, `gzip` and `zipfile` decoded every output of both arms and
  compared it with its source, including PNG chunk CRCs and APNG frames: 730
  raw, 2,104 PNG, 388 container, 762 relaxed and 50 Max outputs, with no
  mismatch. Python cannot decode three legacy-method ZIP sources, and Columbo
  rejects two ZIP layouts in both arms.
- Formatting, Clippy and all 668 Rust tests pass, including the
  private-corpus regressions in debug and release, with 13 Python tests and a
  locked build.
- The release executable grows from 1,860,480 to 1,877,008 bytes.

Harness, lists, arm crates and logs: `work/entropy-cut-validation/`; the
probes and estimators: `work/entropy-cut-probe/`.

## Follow-up: work that scales with the stream

Date: 14 October 2026. Baseline: `b212cee`, R16. A second Default pass over
R16 outputs and an uncapped probe showed that R15's fixed 2^28 work units per
stream stopped its recursion on the largest streams. Without a cap, R15's
first pass used 840 million units on the deft4j copy of the Partnership card
(48 blocks, 7.2 million tokens, 9.6 MB decoded), 338 million on the original
(171 blocks), 49 million on the 2.3 MB `kzipmix` tar.gz and 27 million on
`FsqwhPuaIAIlojU.png`: about 7–50 units per token and decoded byte, since
each recursion level costs work in proportion to the stream. The 4,096 range
prices never bound.

R15 and R16 now allow 64 work units per token and decoded byte of the stream
per pass, at least 2^28, in both modes. A bounded pass leaves time for the
slides and merges that follow it, which matters when Max's time is limited;
in Max, R12 repeats a pass that changed the stream, so further cuts and merges
stay reachable as time allows. Max keeps Default's endpoint as a final
competitor, and its mandatory Default floors use the same Default work, so Max
does not end above Default.

| Set | Files | Smaller / larger | Bytes saved | Process CPU |
| --- | ---: | --- | ---: | --- |
| Raw streams | 365 | 0 / 0 | 0 | 45.62 → 45.39 s |
| PNG | 1,052 | 3 / 0 | 21,159 | 1,126.27 → 1,135.75 s |
| GZIP, zlib, ZIP and APNG | 202 | 0 / 0 | 0 | 612.27 → 614.09 s |
| Relaxed | 381 | 0 / 0 | 0 | 178.39 → 178.12 s |

Every other Default output was byte-identical. The three Partnership card
copies saved 18,794 bytes (deft4j copy, 0.7%), 1,195 and 1,170; serially, the
deft4j copy took 24.38 → 27.37 s and the original 39.66 → 40.05 s.

On the 25-PNG Max sample at 60 seconds, the deft4j Partnership card also saved
18,794 bytes; four other files moved by 1–4 bytes in either direction, within
the deadline variance those files showed in earlier repeats. Every Max output
was no larger than the same file's Default output from this run. All outputs
decoded independently, and all 674 Rust tests pass in debug and release,
including `terminal_layout_work_scales_with_stream_volume`.

Harness and logs: `work/layout-budget-validation/`.
