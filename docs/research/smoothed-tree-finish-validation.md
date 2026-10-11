<!-- SPDX-License-Identifier: MIT -->

# Smoothed-tree finish: validation

Date: 11 October 2026. Baseline: `c28f1ad`. **Prototype**, implemented as
R14 in the [route catalogue](../routes-and-methods.md#route-gate-reference).
It extends an existing Columbo method to every stream size; worldwide novelty
is not claimed.

## Why it saves bits

A dynamic header transmits every payload code length through a run-length
code. Counts that are nearly equal but not equal produce irregular lengths
that run-length code poorly. Smoothing such counts before building the tree
gives a tree whose header is cheaper by more than its payload costs, when the
tree is priced against the true counts.
[RFC 1951 §3.2.7](https://www.rfc-editor.org/rfc/rfc1951#section-3.2.7)

Columbo already prices two smoothed seed families: Brotli's fixed-point
streak smoother and Zopfli's nearby-count smoother, each at maximum depths 15
through 9 (`plan_rle_smoothed_tree_candidate`). Before this change they ran
only in the compact payload-tree floor, for outputs of at most 8 KiB
compressed, 128 KiB decoded and eight blocks; the recorded evidence for that
class came from PngSuite. Zopfli derivatives such as ECT and zenzop compare
smoothed and depth-limited trees for every block they write.

## Origin

A review of [zenzop](https://github.com/imazen/zenzop), a Zopfli port whose
enhanced mode applies ECT's tree selection to every block, prompted a probe
of both its own smoother and Columbo's existing families on completed outputs.

- **Columbo's existing families** beat the transmitted trees of 279 of 1,052
  completed `7212d6b` PNG outputs by 141,526 bits, 99.9% of it in streams
  outside the compact class. Regenerated with `93cda73`, the same 279 outputs
  still held 143,670 bits.
- **zenzop's "Brotli-inspired" smoother** is not Brotli's algorithm: it marks
  whole non-zero runs whose counts stay within a relative 1.21 times their
  mean, and averages adjacent marked zero and non-zero runs together. Beyond
  Columbo's ordinary plans and both existing families it found 1,517 bits on
  those 279 regenerated outputs and none on 365 raw streams, so it was not
  added.

## Method

R14 plans each non-stored block of a completed stream: the smoothed families
at every depth from 15 through 9, priced exactly against the true counts. A
block takes the plan only when it is strictly smaller than its current bits
and, in strict mode, uses strictly compatible codes. If any block changed,
R1c slides once more under the new trees. Only a whole-stream byte or
meaningful-bit win replaces the parent.

### Placement

R14 follows R13. In Default it ends the single sweep; in Max it waits, like
R10, R11, R1c and R13, for a sweep in which every earlier method changed
nothing, or for the last sweep, and R12 repeats the sweep after a win. Max's
mandatory Default endpoints apply the same R1c, R13, R14 finish to their slid
final competitor. Like R1c it is linear finalization with no size or
block-count class. The compact payload-tree floor keeps its earlier pass and
balanced closure.

A four-arm A/B compared R14 with the alternative of pricing the same families
in the general bounded-depth floor, before the terminal searches:

- **Early, in the floor.** Smoothed trees chosen before R2–R5 redirected
  them. On one raw ZIP stream R4 found 63 bits from the frontier's tree and
  nothing from the smoothed one, ending 45 bits and 6 bytes larger. On PNG
  six files grew by one byte, two of them outside the 128 KiB class, where
  the slide and ladder were redirected instead.
- **Both.** The floor and R14 together saved 5,145 more PNG bytes than R14
  alone, but five files still grew by one byte against the baseline and CPU
  rose 0.7% more.

## Measurements

Paired runs alternate four arms per input in four lanes and compare complete
files: the baseline, R14 alone (**final**), the same families in the general
floor alone (**early**), and both.

| Set | Files | Early: smaller / larger, bytes | **Final: smaller / larger, bytes** | Both: smaller / larger, bytes |
| --- | ---: | --- | --- | --- |
| Raw streams | 365 | 21 / 1, +132 −6 | **19 / 0, +141** | 27 / 1, +174 −6 |
| PNG: medium, large, small, imageworsener, pkmn-col | 1,052 | 249 / 6, +24,256 −6 | **262 / 0, +27,472** | 279 / 5, +32,584 −5 |
| GZIP, zlib, ZIP and APNG | 202 | 55 / 3, +3,953 −18 | **89 / 0, +20,301** | 95 / 3, +21,092 −18 |
| Relaxed: raw streams and 16 PNG | 381 | 35 / 1, +6,368 −6 | **34 / 0, +7,139** | 42 / 1, +9,648 −6 |

| Set | CPU: baseline | Early | **Final** | Both |
| --- | ---: | ---: | ---: | ---: |
| Raw streams | 96.96 s | 97.02 s | **97.53 s** | 97.38 s |
| PNG | 1,564.53 s | 1,583.13 s | **1,598.29 s** | 1,609.67 s |
| Containers | 936.14 s | 938.73 s | **950.57 s** | 965.73 s |
| Relaxed | 220.18 s | 222.20 s | **226.20 s** | 227.81 s |

R14 alone grew no output and saved more than the early placement in every
set. On PNG the gain is broad but small per file: 246 of 387 medium images
gained 21,019 bytes and 9 of 21 large images 6,434 bytes, while small,
imageworsener and pkmn-col images gained 19 bytes together. The largest
wins were 1,983–2,027 bytes on the Partnership card's original and DeflOpt
copies, about 0.07%, and 1,714 bytes on `floor pattern.png`. Among
containers, the three linux-static `kzipmix` tar.gz copies saved
1,394–1,476 bytes each, and each multi-frame Steam APNG about 1 KB.

R14 saved more than the probe's 17,959 bytes of PNG headroom because R1c
slides again under the new trees. On
`kzipmix-20200115-linux-static.tar.gz`, R14 saves 9,442 bits and the
following slide 2,370 more; on `floor pattern.png` they save 5,349 and 8,368
bits.

**Cost.** CPU rose 0.6–2.7% in Default. Isolated, the GZIP above takes
11.28 → 12.13 s (R14 353 ms, the following slide 491 ms) for 1,476 bytes,
and the original 3.2 MB Partnership card 31.52 → 33.38 s for 1,983 bytes.

**Max.** On the 25-PNG Max sample at 60 seconds, two lanes, R14 shortened 19
files by 4,136 bytes; every run used its full allowance. The deft4j
Partnership card saved 3,572 bytes and the `floor pattern.png` copies 76–95
each. One file, `FsqwhPuaIAIlojU.png`, ended one byte larger. That loss did
not reproduce: two serial reruns of each arm gave 543,613 bytes for the
baseline both times and 543,432 and 543,506 bytes with R14. Process CPU was
3,181.94 → 3,196.54 s, within the time-bound runs' allowance.

## Validation

- `smoothed_tree_finish_follows_the_terminal_searches` builds a literal-only
  block of 40 symbols with 20–22 occurrences each. Its smoothed tree costs
  4,698 bits against 4,713 for the ordinary plan and 4,712 for the
  bounded-depth frontier. Nine copies exceed the compact class. In both modes
  R14 adopts a tree at least as cheap on every block, saves at least
  9 × 15 bits, reparses to the same bytes with strictly compatible codes in
  strict mode, and does nothing under an expired stop. Default's terminal
  sweep and Max's Default endpoint both finish with it.
- The existing Max closure test keeps its bound, and an expired Max budget
  still reproduces the Default terminal endpoint exactly.
- Python's `zlib`, `gzip` and `zipfile` decoded every final output and
  compared it with its source, including PNG chunk CRCs and APNG frames:
  365 raw, 1,052 PNG, 194 container and 381 relaxed and 25 Max outputs, with
  no mismatch. Python cannot decode eight container sources: six legacy ZIP
  methods and the two layouts Columbo rejects.
- `cargo fmt --check`, `cargo clippy --all-targets --all-features -- -D
  warnings`, all library, binary, CLI, public API, doc and Python tests pass.
  The private-corpus regressions pass with `--include-ignored` in debug and
  release.
- The release executable keeps its size, 1,843,888 bytes; `__text` grows by
  108 bytes.

## Limits

Per file the gain is small: at most 0.07% of the measured images. The
method adds no tree family: it applies Columbo's two existing smoothers at
every size, without the compact floor's balanced closure, and changes no
tokens or boundaries itself.

Smoothing earlier, before the terminal searches, finds trees that the slide
and ladder then build on: the early and both arms saved up to 5,145 PNG bytes
more than R14 alone. Every such placement measured here also grew some
outputs, so a monotone way to capture that extra gain remains open.

## Follow-up: repeating the finish

Date: 11 October 2026. Baseline: `a880bcf`, the first R14. Each R13 or R14
win slides the blocks again, and the slid blocks can expose another collapse
or smoothing win. Default now runs R13 and R14 one more pass after a pass in
which either won. Max's later sweeps already revisit both, through R12, until
neither wins; Max uses Default's one-pass bound only when no further sweep
can start, and in its mandatory Default endpoints, so that an expired Max
budget still reproduces Default exactly. Given time, Max's savings are never
bounded by it: with a 300-second allowance, `4.2.03-deft4j-t15s.PNG` ended
its last Max sweep with neither step finding a candidate, at 748,539 bytes,
below the unbounded Default repetition's 748,929.

Repeating to a fixed point in Default was measured first. On
`4.2.03-deft4j-t15s.PNG` it took five passes: the second smoothing pass and
its slide saved 4,401 bits, the three later passes 189, 13 and 25 bits. On the
large PNGs and tar.gz archives, later passes cost seconds for a few bytes.

| Set | Files | Unbounded: smaller, bytes | **One pass: smaller, bytes** | Two passes: smaller, bytes |
| --- | ---: | --- | --- | --- |
| Raw streams | 365 | 0, 0 | **0, 0** | 0, 0 |
| PNG | 1,052 | 89, 4,844 | **88, 4,297** | 89, 4,799 |
| GZIP, zlib, ZIP and APNG | 202 | 26, 1,474 | **26, 1,120** | 26, 1,278 |
| Relaxed | 381 | 14, 1,167 | **14, 1,034** | 14, 1,167 |

No output grew in any arm; every step still accepts only a strict win. The
unbounded arm, measured against `a880bcf` on a quiet machine, raised CPU by
4.5% on PNG, 4.5% on containers and 5.1% relaxed. The bounded arms ran on a
loaded machine, so their corpus CPU is not comparable; serial timings of the
costliest files decide between them:

| File | `a880bcf` | One pass | Two passes | Unbounded |
| --- | ---: | ---: | ---: | ---: |
| Partnership card | 33.47 s | 35.30 s, −200 B | 36.97 s, −283 B | 39.57 s, −284 B |
| `kzipmix-20200115-linux-static.tar.gz` | 12.25 s | 13.79 s, −259 B | 14.66 s, −274 B | 16.40 s, −275 B |
| `floor pattern.png` | 13.46 s | 14.31 s, −339 B | 14.63 s, −341 B | 15.02 s, −341 B |
| `FsqwhPuaIAIlojU-deflopt.png` | 16.33 s | 16.88 s, −192 B | 17.29 s, −199 B | 18.50 s, −208 B |
| `BNDT_on_X…-deflopt.png` | 8.02 s | 8.51 s, −77 B | 8.84 s, −87 B | 9.90 s, −90 B |
| `4.2.03-deft4j-t15s.PNG` | 2.03 s | 2.28 s, −584 B | 2.51 s, −607 B | 3.02 s, −612 B |

One pass keeps 89% of the unbounded PNG gain for about half the extra time of
two; the rest remains reachable in Max.

On the 25-PNG Max sample at 60 seconds, two lanes, one pass changed 9 files:
5 smaller by 35 bytes and 4 larger by 9, with process CPU 3,155.09 →
3,154.88 s. Serial reruns did not reproduce the two largest losses:
`4.2.07.PNG` gave 725,415 bytes in both arms, and `4.2.03.PNG` gave 748,531
bytes with `a880bcf` and 748,530 with one pass in two further runs each, after
one earlier one-pass run of 748,590.

All 1,992 outputs of the one-pass arm decoded identically with Python's
`zlib`, `gzip` and `zipfile`. The R14 test now also checks that its stream
ends where neither R13 nor R14 improves the Default result. All tests pass,
including the private-corpus regressions in debug and release.
