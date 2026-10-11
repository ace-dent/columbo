<!-- SPDX-License-Identifier: MIT -->

# Coupled literal/length and distance swaps

Date: 10 September 2026. Baseline: `b931212`, which includes R8 and the exact
positive-run pricing optimization. **Accepted and implemented in Max only**
as R9; the [route catalog](../routes-and-methods.md#route-gate-reference)
describes the current method. Implements the paired literal/distance extension
proposed in
[new byte-saving methods](new-byte-saving-methods.md#1-spend-payload-bits-on-better-code-length-permutations).

## Why coupling can save bits

One code-length tree and one RLE sequence describe both payload length lists,
so a swap in each alphabet can together make a different shared description
profitable even when either swap alone finds no saving.

A deterministic synthetic frequency search produced the checked-in witness.
Swapping literal/length symbols 266 and 268 adds two payload bits; swapping
distance symbols 14 and 18 adds six. Together they remove ten header bits,
saving **two meaningful bits: 4,974 → 4,972**. Every single nonzero,
unequal-length swap in either alphabet ties or loses under full header
repricing, and the exact CL-tree/RLE solver ties on the unchanged parent. A
generated stored prefix supplies the long-distance history; no private fixture
bytes are embedded.

## Bounds

[`header/coupled.rs`](../../src/deflate/header/coupled.rs) implements the
[R9 route row](../routes-and-methods.md#route-gate-reference). Its menu filters
and rankings are heuristics, not lower bounds; every retained pair is fully
repriced from the unchanged parent. The common gate allows at most 1 MiB each
of compressed and decoded data, 128 parsed blocks and no discarded wire blocks.

A loose full-block generation bound is 42,846 units for 286 literal/length and
30 usable distance symbols. On the measured 64-bit platform, the swap and pair
allocations and the length array total **10,654 bytes of element storage**.

## Controls and isolated evidence

Fresh Default runs reproduce all 365 previous raw outputs exactly. Each parent
first receives R7, R8 and R7 again, so those methods' own gains are excluded.

An exploratory 32×32 cross-product without the production stream price cap
improves **43/365 streams by 336 bits and 39 bytes**. Exhaustive single-swap
controls on its 49 winning blocks perform **933,145 full header prices**, with no
tax band or transition filter, skipping a pair only if its payload plus the 17
fixed header bits cannot beat the incumbent. Exact CL-tree/RLE repricing of the
unchanged parent finds no gain on any of those blocks.

The bounded production method keeps **43 wins, 293 bits and 34 bytes**. It beats
the best exhaustively priced single swap on **21 blocks in 20 streams**, keeping
76 bits beyond that control on the same outputs.

A real frame from the Steam sticker APNG is a single-swap local optimum:

| Choice | Complete block bits |
| --- | ---: |
| Parent, including exact CL-tree/RLE optimization | 2,023 |
| Best fully repriced single swap in either alphabet | 2,023 |
| Selected LL swap alone, with exact CL-tree/RLE optimization | 2,023 |
| Selected DD swap alone, with exact CL-tree/RLE optimization | 2,024 |
| Both swaps together | **2,020** |

Here LL symbols 266/272 and DD symbols 8/24 exchange lengths, at a payload cost
of three and one bits; the exact CL solver also finds the paired header
optimal. Two strictly improving single-swap steps cannot reach the pair; longer
paths are not ruled out.

Fresh ten-second baseline Max runs on the 43 exploratory witnesses all reach
their timeouts. The R7/R8/R7 control then strengthens 23 of those frozen outputs
by 140 bits, which are not attributed to R9. R9 then improves **42/43 distinct
controlled Max parents, by 302 bits and 37 bytes**; this overlaps the
Default-parent sample.

Isolated production runs take 11.654 seconds for the 365 Default-derived parents
and 2.531 seconds for the 43 Max-derived parents, including parsing, emission
and validation. Other local work could run during these timings.

### Budget coverage and limits

Instrumentation reproduces the uninstrumented production bytes on all 408
parents. It records **5,753,474 generation units and 61,437 full prices**. The
largest per-stream generation charge is 115,760 units, and no generation
allowance is exhausted. Eleven Default-derived and six Max-derived streams use
all 512 prices.

A control with the same 32/256 menus but no stream work or price caps recovers
43 further bits / five bytes on the Default-derived `kzipmix` stream, and on the
Max-derived parents 46 bits / five bytes on `kzipmix` and one bit on
`pengbrew_160x160`, with no other output differences. It improves the same 43
and 42 streams. The production caps therefore leave some gains in
multi-block streams.

## Public API and acceptance

R9 is accepted in Max only. Raw inputs and wrappers, and the strict and relaxed
samples, overlap; their savings are not additive. Meaningful-bit totals come
from parsing every emitted Deflate stream, including wrapped streams.

All **404 Default outputs are byte-identical** to the current baseline. Timings
are alternating paired observations from persistent API runners, excluding
compilation, startup and file I/O.

| Default workload | Baseline | With R9 | Difference |
| --- | ---: | ---: | ---: |
| 365 raw streams | 88.723 s | 88.963 s | +0.27% |
| 39 containers | 58.995 s | 58.931 s | -0.11% |

Strict Max improves **5/13 selected cases**, with no regressions. Both arms
receive ten seconds per case, and all 13 cases time out in each arm.

| Input | Baseline bytes | With R9 | Meaningful bits saved |
| --- | ---: | ---: | ---: |
| basi6a16, raw | 4,072 | 4,070 | 16 |
| Steam sticker frame, raw | 253 | 253 | 3 |
| kzipmix GZIP member, raw | 52,661 | 52,661 | 0 |
| happy frame, raw | 6,541 | 6,541 | 0 |
| basi6a16.png | 4,151 | 4,149 | 16 |
| f04n0g08.png | 263 | 263 | 7 |
| briefcase.png | 2,348 | 2,348 | 0 |
| grayscale_alpha_8_reduce_alpha.png | 21,901 | 21,901 | 0 |
| happy.png | 125,168 | 125,164 | 34 |
| Steam sticker APNG | 1,841 | 1,841 | 0 |
| kzipmix-20200115-bsd.tar.gz | 52,679 | 52,679 | 0 |
| checkers_src.zip | 14,014 | 14,014 | 0 |
| XYB.icc.zlib | 347 | 347 | 0 |

Relaxed Max improves **2/4 selected cases**, with no regressions; all four cases
time out in each arm.

| Input | Baseline bytes | With R9 | Meaningful bits saved |
| --- | ---: | ---: | ---: |
| Steam sticker frame, raw | 253 | 253 | 3 |
| happy.png | 125,167 | 125,164 | 34 |
| kzipmix-20200115-bsd.tar.gz | 52,679 | 52,679 | 0 |
| checkers_src.zip | 14,014 | 14,014 | 0 |

The Defluff relaxed Default benchmark passes **66/66 comparisons** (61 better,
5 ties, no errors or parity misses; 109-byte / 932-bit lead unchanged). It
checks Default compatibility only. The fresh report is
`work/coupled-swap-validation/public/defluff.md`.

## Tests and reproduction

All **548 Rust tests** pass: 484 library, 50 CLI, six private-corpus and eight
public-API tests. They cover both menus, the synthetic single-swap barrier, all
eight incoming alignments and generation, price and callback stops; zero-budget
Max stays identical to Default in both strictness modes. Every output was
decoded independently, with wrapper checksums, metadata and zlib windows
checked. Checks run:

```sh
cargo test --release --locked -- --include-ignored
cargo clippy --all-targets --all-features -- -D warnings
cargo fmt --all -- --check
python3 -m unittest discover -s tests -p 'test_*.py'
```

Local evidence is in `work/coupled-swap-validation/`; `baseline-state.json`
records the baseline source. `work/run-coupled-validation.py` runs alternating
public-API comparisons, and `work/verify-coupled-public.py` decodes raw streams
and checks wrapper identities, metadata, checksums and zlib windows. These
helpers need the private corpus.

Comparable stripped release executables grow from 1,744,592 to 1,761,104 bytes,
an increase of 16,512 bytes. Builds use Rust 1.97.1 on macOS Apple Silicon, with
the repository's release profile, fat LTO and overflow checks. Candidate
SHA-256: `a31ab444fbc6a27ed408371b690e7dbb1a6bce9b8179d15010a85e067f0d8045`.
`public/source-hashes.json` records the benchmarked source.
