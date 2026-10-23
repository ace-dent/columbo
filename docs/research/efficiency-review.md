# Efficiency review

A dated log of merged changes that remove repeated calculation and copying
without changing output: which search routes run, their tie-breaking, work
budgets and stop checks stay the same.

## Methods

Focused timings run extracted original and revised functions on identical
inputs, with the same allocation helpers and stopping implementation
(`SearchStop`). They use one local macOS arm64 host, Rust 1.97.1 and optimized
builds with overflow checks. From `625fb30` through `da2eefc`, builds also use
fat LTO and one codegen unit, and values are medians of seven batches
alternating baseline and candidate order. From `f31050b` through `da2eefc`, the
baseline is built from an immutable Git archive of its commit. Focused timings measure the named
operation, not whole-file speed; unless an entry says otherwise, sampled
whole-file timings were largely unchanged.

Equivalence checks use these terms:

- **Comparisons**: original and revised functions on identical inputs must
  return identical results, remaining budgets and stop-check counts,
  including under interruption.
- **API comparisons**: baseline and candidate public-API runs, in Default and
  zero-budget Max plus any listed ten-second Max runs, must give identical
  output bytes and reported savings, with independent PNG/APNG, GZIP, ZIP,
  zlib and raw Deflate decoding. A Max run that reaches its deadline does not
  show complete-search timing equivalence.
- **Contributor-guide checks**: locked build, formatting, all-feature Clippy
  with warnings denied, all-target debug tests including the private corpus,
  documentation tests, and a package list that excludes private fixtures and
  scratch artifacts.

These checks apply to entries through `da2eefc`. Rust tests ran with
`cargo test --release` for `9825467` and with
`cargo test --release -- --include-ignored`, which adds the private-corpus
regressions, from `996c30e` on; from `fae577c` on they also passed in debug
mode. Every entry passed Clippy with warnings denied and formatting checks,
and every entry through `931f8a9` passed whitespace checks. From `f31050b` on,
entries also passed the contributor-guide checks, and from `a60ce63` on, the
final source hashes match the tested build. Baseline and candidate release
executables had the same size except at `042d430` (1,794,288 → 1,794,304
bytes).

Each entry gives its batch sizes and the ignored `work/` and `target/`
directories holding its harnesses, baselines, source hashes, API drivers and
logs.

## Summary

Output equivalence lists API comparisons (inputs; ten-second Max runs),
function comparisons and tests passed; every comparison matched exactly.
Headline speedups give the largest focused gain, any slowdown, and whole-file
gains where established.

| Date | Baseline | Area | Change | Headline speedup | Output equivalence |
| --- | --- | --- | --- | --- | --- |
| 6 Sep 2026 | [`9825467`](#9825467-6-september-2026) | Dynamic-header RLE; proven match splitting; ZIP processing | Monotonic deque for symbol-18 zero runs; each submatch built and priced once (256 instead of 32,896 operations at 258 bytes); ZIP payloads borrowed until replaced | RLE 7.66×; match splitting 8.62×; Default CLI 1.29× | 24 full-file (16 Default, 8 Max); 8,192 match-solver; 499 Rust tests |
| 9 Sep 2026 | [`996c30e`](#996c30e-9-september-2026) | Greedy source-block merging; canonical match lengths | Merged slots retired in place with two forward cursors; a compile-time 256-byte table replaces a scan of up to 28 length families | Merging 159.81×; length encoding 14.55× | 32 API (16 inputs); 576 route; all 65,536 `u16` lengths; 516 Rust, 6 Python tests |
| 9 Sep 2026 | [`625fb30`](#625fb30-9-september-2026) | Joint-tree payload bounds; symbol-set admission | Each code length priced once per suffix-table row, visiting only capacities where it fits; admission rejects once a running total exceeds its limit | Payload bounds 2.59×; joint solver 1.20×; over-limit rejection 47.50× | 36 API (16 inputs; 4 Max, 1 completed); 1,800 table, 5,120 solver, 1,920 admission; 522 Rust, 6 Python tests |
| 9 Sep 2026 | [`4cc332f`](#4cc332f-9-september-2026) | Range histograms for boundary scoring | Short ranges counted directly when that visits no more tokens than rebuilding two checkpoint tails | Short ranges 13.43×; indexed ranges 1.00× | 36 API (16 inputs; 4 Max, 1 completed); 592,138 range, 34,832 direct-counter; 522 Rust tests |
| 10 Sep 2026 | [`f31050b`](#f31050b-10-september-2026) | Max header-tree run prices | Each positive run length priced once per code-price pair in closed form, then weighted by run histograms | Price preparation 5.99×; full header search 2.57× | 36 API (16 inputs; 4 Max, 1 completed); 7,680 price-table, 1,920 header-search, 17,808-case formula test; 541 Rust, 10 Python tests |
| 12 Sep 2026 | [`a60ce63`](#a60ce63-12-september-2026) | Code-length transition counting | One shared counter for pair swaps, rotations and coupled swaps visits each affected edge once | Counters 5.46×; candidate menus 2.93× | 36 API (16 inputs; 4 Max completed); 106,700 counter, 2,640 menu; 571 Rust, 13 Python tests |
| 13 Sep 2026 | [`fae577c`](#fae577c-13-september-2026) | Fixed-block coalescing; package-merge leaf order | Join borrows the last output plan, so joined vectors stay uniquely owned; leaves sorted once by weight and original index | Joins 263.67×; package merge 1.81× (0.96× on small tied input) | 38 API (17 inputs; 4 Max completed); 83,328 package-merge, 21,600 append; 572 Rust, 13 Python tests |
| 13 Sep 2026 | [`042d430`](#042d430-13-september-2026) | Match spelling and exchange bounds (R10, R11) | Prices per length family: suffix minima for exact spelling, greatest fitting width for lower bounds | 258-byte spelling 4.20× dense, 103.38× sparse; short bounds 0.61× | 44 API (19 inputs; 6 Max completed); 9,216 regression-test, 32,768 spelling, 12,288 bound; 584 Rust, 13 Python tests |
| 15 Sep 2026 | [`931f8a9`](#931f8a9-15-september-2026) | Package merge and frequency smoothers (`huffman.rs`) | Tagged leaf-or-pair nodes (56 → 32 bytes), two alternating level buffers, shared equal-count run marker | Package merge 1.19×; 13 fewer allocation requests and about 41% less peak requested memory at 286 symbols | 44 API (19 inputs; 6 Max completed); 83,328 package-merge, 139,074 smoother; 585 Rust, 13 Python tests |
| 15 Sep 2026 | [`9860910`](#9860910-15-september-2026) | Same-distance partitions; saturated header caches | One range-minimum query per length family via the shared `minima.rs` helper; cache copy cloned only when the cache has room | Partition tables 5.68×; generated whole-file inputs 1.38× | 50 API (21 inputs; 8 Max completed); 2,639 partition-table; 588 Rust, 13 Python tests |
| 15 Sep 2026 | [`da2eefc`](#da2eefc-15-september-2026) | Original-match restoration | Monotonic-queue minimum per length family, batched exact work charges, only the winning match built | 4,096-byte interval 3.85× (22.02× with only length 258); 12 bytes 0.91× | 50 API (21 inputs; 8 Max completed); 632 oracle, 18,504 charge, 4,736 harness; 591 Rust, 13 Python tests |
| 1 Oct 2026 | [`62f772a`](#62f772a-1-october-2026) | `solve_proven_submatch_avoiding` | One `SuffixMinima` query per length family; stop checks every 32 positions instead of every 32 edges | Solver about 2×; whole-file CPU unchanged | 32 Default inputs, run twice; 400 oracle, 100,000 random |
| 13 Oct 2026 | [`9ffa010`](#9ffa010-13-october-2026) | Code-length tree building (`huffman.rs`, `header.rs`) | 12-byte heap entries with child indices replace 88-byte linked nodes; code-length reordering returns early when lengths already follow frequencies; the order-key tree that duplicates DeflOpt variant 3 is no longer built | Default CPU 1.13× (33 inputs) | 104 API (52 inputs; 4 Default runs reached the deadline, 3 rerun to completion); 33 Default inputs; 640,000 heap-builder, 200,000 reorder; 659 Rust, 13 Python tests |
| 13 Oct 2026 | [`dc68b5f`](#dc68b5f-13-october-2026) | Header-plan reuse across routes (`header.rs`) | Route-local header-cache misses fall through to a per-thread store of packed completed plans, two generations within an 8 MiB charge | Default CPU 1.38× (33 inputs); peak memory +3.8 MiB median, +20.0 MiB largest | 104 API (52 inputs; Default at the 180-second budget and zero-budget Max); 33 Default inputs; 662 Rust, 13 Python tests |
| 20 Oct 2026 | [`04d63bd`](#04d63bd-20-october-2026) | Repeated block plans, terminal reparses, entropy-cut logarithms (`block.rs`, `optimize.rs`, `stream/entropy_cut.rs`) | A per-thread store of completed block plans keyed by symbol counts, extra bits, source header and policy; an accepted terminal candidate's validated parse handed to the next method; the `c log2 c` table grown to the block's counts | Default CPU 1.29× (33 inputs); Partnership card 1.52× with 100 MiB less peak memory; peak memory +1.9 MiB median, +11.9 MiB largest | 104 API (52 inputs; Default at the 180-second budget and zero-budget Max); 33 Default and 5 APNG inputs; 685 Rust, 13 Python tests |
| 23 Oct 2026 | [`c94eb07`](#c94eb07-23-october-2026) | Code-length trees and DeflOpt local rewrites (`header.rs`, `huffman.rs`) | A per-thread cache of code-length trees by builder and RLE histogram; one record of local-rewrite steps shared by the four heap variants; rewrites count their symbols as they are built; packed heap keys | Default CPU 1.25× (33 inputs); large APNG files 1.22–1.39×; a completing Max run 1.56×; peak memory +0.2 MiB median, +3.1 MiB largest | 104 API (52 inputs; Default at the 180-second budget and zero-budget Max); 33 Default inputs, 7 large PNG/APNG files and a completing Max run; 20,000 rewrite comparisons; 681 Rust, 13 Python tests |
| 23 Oct 2026 | [`0141972`](#0141972-23-october-2026) | Max header planning (`header.rs`) | Shortest RLEs found once per header plan and tree, in stack buffers, with their symbol counts; the code-length tree cache grown to 16,384 slots | Captured Max header plans 1.14×; completing Max runs 1.30× and 1.11×; Default CPU 1.01× (33 inputs); peak memory +1.2 MiB median | 104 API (52 inputs; Default at the 180-second budget and zero-budget Max); 89,255 replayed header plans; 2 completing Max runs; 20,000 shortest-RLE comparisons; 666 Rust, 13 Python tests |
| 23 Oct 2026 | [`d72c937`](#d72c937-23-october-2026) | Max header planning (`header.rs`, `huffman.rs`) | Code-length trees cached by RLE histogram, builders that must agree sharing one tree; repeat rewrites skipped when the RLE's symbol counts show nothing can change; one scan per shortest-RLE repeat window, in buffers kept for the header plan; RLE and tree buffers sized once; a branch-free Kraft sum; packed keys in Columbo's heaps | Captured Max header plans 1.26×; completing Max runs 1.29× and 1.12×; Default CPU 1.14× (33 inputs); peak memory −0.1 MiB median | 104 API (52 inputs; Default at the 180-second budget and zero-budget Max); 89,255 replayed header plans; 2 completing Max runs; 403,865 captured shortest-RLE calls; 20,000 rewrite and 768 heap-merge comparisons; 668 Rust, 7 Python tests |

## Entries

### `9825467`, 6 September 2026

Batches: medians of five; 20,000 RLE or 2,000 match-solver calls per batch.

| Workload | Before | After | Speedup |
| --- | ---: | ---: | ---: |
| RLE, 318 zero lengths | 63.74 µs | 8.32 µs | 7.66× |
| RLE, 318 mixed run lengths | 13.41 µs | 6.36 µs | 2.11× |
| RLE, 318 varying nonzero lengths | 2.46 µs | 2.16 µs | 1.14× |
| Match splitting, 12 bytes | 0.46 µs | 0.40 µs | 1.17× |
| Match splitting, 64 bytes | 30.83 µs | 5.99 µs | 5.15× |
| Match splitting, 258 bytes | 791.06 µs | 91.82 µs | 8.62× |

Default CLI medians over three runs, including startup and I/O, went from
0.326 to 0.253 seconds on the generated raw stream (1.29×), from 0.804 to
0.728 seconds on `checkers_src.zip` (1.10×) and from 0.064 to 0.055 seconds on
a generated 32 MiB stored ZIP (1.16×). Ten-second Max runs often reached their
deadline, so they show no algorithm speedup.

Committed in `8165633`. Artifacts: `work/efficiency-review/`,
`target/efficiency-review-baseline/`.

### `996c30e`, 9 September 2026

Batches: medians of five runs calling the route directly on one-byte stored
blocks; the public optimizer can bypass this route for wholly stored input.

| Source blocks | Before | After | Speedup |
| --- | ---: | ---: | ---: |
| 128 | 0.556 ms | 0.060 ms | 9.27× |
| 1,024 | 30.132 ms | 0.515 ms | 58.51× |
| 4,096 | 495.570 ms | 3.101 ms | 159.81× |

Length encoding averaged 15.353 ns before and 1.055 ns after (14.55×), the
median of seven batches of 2,560,000 calls over all legal lengths.

Artifacts: `work/efficiency-review-sep9/`,
`target/efficiency-review-sep9-baseline/`.

### `625fb30`, 9 September 2026

Batches: 100 payload-table calls (20,000 for the short case), ten joint-solver
calls, and 20,000 symbol-admission calls on 8,192-token blocks.

| Workload | Before | After | Speedup |
| --- | ---: | ---: | ---: |
| Payload bounds, eight symbols, depth three | 0.466 µs | 0.266 µs | 1.75× |
| Payload bounds, 286 dense symbols, depth nine | 1,613.375 µs | 669.081 µs | 2.41× |
| Payload bounds, 286 sparse symbols, depth nine | 1,646.482 µs | 637.376 µs | 2.58× |
| Payload bounds, 32 distance positions, depth nine | 169.730 µs | 65.510 µs | 2.59× |
| Joint solver, depth three | 294.912 µs | 288.321 µs | 1.02× |
| Joint solver, depth six | 600.237 µs | 499.225 µs | 1.20× |
| Joint solver, depth nine | 67.378 ms | 66.327 ms | 1.02× |
| Symbol admission, accepted at match limit | 2.709 µs | 2.711 µs | 1.00× |
| Symbol admission, exceeds match limit | 2.705 µs | 0.122 µs | 22.21× |
| Symbol admission, exceeds byte limit | 2.653 µs | 0.056 µs | 47.50× |

The paired-alphabet route needed no change: it already caches its range
estimates, prepared edges, header plans and sampled split positions. A slow
first Default `Load.png` sample (0.847 ms versus 1.273 ms) was rechecked over
30 alternating pairs: medians were 0.838 ms versus 0.807 ms, and the median
paired candidate/baseline ratio was 0.993.

Artifacts: `work/efficiency-review-joint/`,
`target/efficiency-review-joint-baseline/`.

### `4cc332f`, 9 September 2026

Batches: 100,000 calls on identical mixed literal/match input.

| Token range | Before | After | Speedup |
| --- | ---: | ---: | ---: |
| Empty, 255..255 | 0.696 µs | 0.054 µs | 12.99× |
| One token, 254..255 | 0.699 µs | 0.052 µs | 13.43× |
| Within a checkpoint, 224..248 | 0.661 µs | 0.069 µs | 9.63× |
| Across a checkpoint, 250..270 | 0.483 µs | 0.064 µs | 7.51× |
| Aligned, 256..512 | 0.272 µs | 0.271 µs | 1.00× |
| Long, 17..32760 | 0.483 µs | 0.483 µs | 1.00× |

Long ranges keep the checkpoint index, whose calls still visit at most
510 tokens.

Artifacts: `work/efficiency-review-ranges/`.

### `f31050b`, 10 September 2026

Batches: 20,000 preparation calls, using a three-bit repeat-16 code, or 100
full-search calls.

| Workload | Before | After | Speedup |
| --- | ---: | ---: | ---: |
| Prepare prices, distinct short runs | 0.684 µs | 0.591 µs | 1.16× |
| Prepare prices, 15 symbols with 20-value runs | 12.188 µs | 2.035 µs | 5.99× |
| Prepare prices, mixed run histograms | 4.051 µs | 1.257 µs | 3.22× |
| Prepare prices, one 317-value positive run | 14.727 µs | 5.910 µs | 2.49× |
| Full header search, distinct short runs | 7.336 µs | 7.321 µs | 1.00× |
| Full header search, shared run lengths | 131.500 µs | 58.516 µs | 2.25× |
| Full header search, mixed run histograms | 71.976 µs | 51.942 µs | 1.39× |
| Full header search, one long positive run | 107.325 µs | 41.773 µs | 2.57× |

A run's optimal price depends only on its size, literal-code price and
repeat-16 price, so a closed form (`positive_run_cost` in
`src/deflate/header/tree.rs`) replaces the per-symbol dynamic program.

Artifacts: `work/efficiency-review-header-runs/`,
`target/efficiency-review-header-runs-snapshot/`.

### `a60ce63`, 12 September 2026

Batches: one million counter calls, 100 swap/coupled menu calls, five
rotation-menu calls at 32 and 120 symbols, and one at 286.

| Workload | Before | After | Speedup |
| --- | ---: | ---: | ---: |
| Pair-swap counter | 0.017 µs | 0.003 µs | 5.46× |
| Coupled-swap counter across the seam | 0.022 µs | 0.006 µs | 3.61× |
| Rotation counter with adjacent positions | 0.022 µs | 0.006 µs | 3.43× |
| Pair-swap menu, 286 symbols | 323.592 µs | 131.490 µs | 2.46× |
| Coupled menus, 286 + 30 symbols | 523.295 µs | 353.775 µs | 1.48× |
| Rotation menu, 32 symbols | 115.508 µs | 39.400 µs | 2.93× |
| Rotation menu, 120 symbols | 8,582.633 µs | 3,505.008 µs | 2.45× |
| Rotation menu, 286 symbols | 100,226.541 µs | 40,153.708 µs | 2.50× |

Callers supply strictly increasing replacement positions, so only the
preceding or following replacement can share an edge. The change removes
31 production lines and adds no allocation.

Artifacts: `work/efficiency-review-transitions/`,
`target/efficiency-review-transitions-baseline/`.

### `fae577c`, 13 September 2026

Batches: 1,000 package-merge calls; ten fixed-join runs at 128 and 1,024
blocks and three at 4,096, with 16 literal bytes per block, including cloning
the shared input plans and dropping the result.

| Workload | Before | After | Speedup |
| --- | ---: | ---: | ---: |
| Package merge, 19 positions with tied weights | 1.261 µs | 1.308 µs | 0.96× |
| Package merge, 286 descending weights | 53.375 µs | 29.491 µs | 1.81× |
| Package merge, 286 ascending weights | 28.748 µs | 29.458 µs | 0.98× |
| Package merge, 286 mixed weights | 42.234 µs | 31.452 µs | 1.34× |
| Join 128 fixed blocks | 65.108 µs | 7.158 µs | 9.10× |
| Join 1,024 fixed blocks | 4,352.983 µs | 49.925 µs | 87.19× |
| Join 4,096 fixed blocks | 53,671.875 µs | 203.555 µs | 263.67× |

The join timings isolate accumulated payload copying and exclude per-block
planning. Default whole-file baseline/candidate ratios were 0.96–1.02.

Artifacts: `work/efficiency-review-sep13/`,
`target/efficiency-review-sep13-baseline/`.

### `042d430`, 13 September 2026

Batches: 1,000 spelling or 100,000 bound calls. Dense cases use eight-bit
literal/length prices and a five-bit distance code with thirteen extra bits;
sparse cases keep only length symbol 285, with unchanged literal prices.

| Workload | Before | After | Speedup |
| --- | ---: | ---: | ---: |
| Spell 3 bytes, dense prices | 0.136 µs | 0.100 µs | 1.36× |
| Spell 12 bytes, dense prices | 0.280 µs | 0.229 µs | 1.22× |
| Spell 64 bytes, dense prices | 4.833 µs | 2.846 µs | 1.70× |
| Spell 128 bytes, dense prices | 18.783 µs | 7.247 µs | 2.59× |
| Spell 258 bytes, dense prices | 74.694 µs | 17.766 µs | 4.20× |
| Spell 258 bytes, mixed/absent length prices | 77.584 µs | 15.828 µs | 4.90× |
| Spell 258 bytes, only length 258 available | 68.144 µs | 0.659 µs | 103.38× |
| Bound 3 bytes | 0.011 µs | 0.019 µs | 0.61× |
| Bound 12 bytes | 0.030 µs | 0.044 µs | 0.68× |
| Bound 64 bytes | 0.167 µs | 0.103 µs | 1.61× |
| Bound 258 bytes | 0.628 µs | 0.125 µs | 5.02× |

Each family query takes two lookups in an incremental table of six
power-of-two suffix minima, and the lower bound examines at most 29 family
widths instead of up to 256. The sparse gain comes from skipping unavailable
edges.

Artifacts: `work/efficiency-review-response/`,
`target/efficiency-review-response-baseline/`.

### `931f8a9`, 15 September 2026

Batches: 2,000 package-merge calls or 20,000 smoothing calls on 286 positions
in seven-value runs. Allocations below come from a separate instrumented
harness on descending weights; requests include reallocations, and peak bytes
count live requested allocations, not RSS or allocator overhead.

| Workload | Before | After | Speedup |
| --- | ---: | ---: | ---: |
| Package merge, 19 positions with tied weights | 1.441 µs | 1.206 µs | 1.19× |
| Package merge, 286 descending weights | 33.565 µs | 30.461 µs | 1.10× |
| Package merge, 286 ascending weights | 30.786 µs | 29.119 µs | 1.06× |
| Package merge, 286 mixed weights | 33.806 µs | 30.963 µs | 1.09× |
| Zopfli smoothing | 1.130 µs | 1.151 µs | 0.98× |
| Brotli smoothing | 0.991 µs | 1.000 µs | 0.99× |

| Symbols / maximum depth | Requests before → after | Peak bytes before → after |
| --- | ---: | ---: |
| 2 / 1 | 5 → 5 | 260 → 164 |
| 19 / 7 | 16 → 11 | 7,915 → 4,862 |
| 286 / 15 | 29 → 16 | 241,070 → 143,052 |
| 320 / 15 | 30 → 17 | 471,840 → 275,552 |

Five alternating Default pairs on the ZIP and PNG whole-file outliers gave
baseline/candidate median ratios of 0.95 and 0.96 (ZIP 4.28–6.12 seconds
before, 4.19–5.56 seconds after), so no reliable whole-file speedup is
established.

Artifacts: `work/efficiency-review-huffman-storage/`,
`target/efficiency-review-huffman-storage-baseline/`.

### `9860910`, 15 September 2026

Batches: 20,000, 20,000, 1,000, 500, 200 and 20 calls for the rows in order,
under fixed Huffman prices, each building a complete partition table.

| Active matches / maximum deficit | Before | After | Speedup |
| --- | ---: | ---: | ---: |
| 2 / 1 | 0.336 µs | 0.278 µs | 1.21× |
| 2 / 6 | 0.415 µs | 0.383 µs | 1.08× |
| 16 / 31 | 12.743 µs | 5.871 µs | 2.17× |
| 16 / 127 | 180.741 µs | 34.867 µs | 5.18× |
| 16 / 257 | 709.645 µs | 128.429 µs | 5.53× |
| 257 / 257 | 11,596.800 µs | 2,041.412 µs | 5.68× |

For `A` active matches, maximum deficit `D` and `F` ≤ 29 length families,
partitioning goes from `O(A D²)` to `O(A D (F + log 32))`; fixed scratch
arrays grow by 4,860 bytes on this 64-bit host. On the two generated partition
inputs, five alternating pairs gave short Default 26.778 → 19.339 ms (1.38×)
and deep Max 682.771 → 578.480 ms (1.18×); other sampled files were broadly
unchanged.

Artifacts: `work/efficiency-review-partition/`,
`target/efficiency-review-partition-baseline/`.

### `da2eefc`, 15 September 2026

Batches: 10,000, 10,000, 1,000, 500, 100 and 20 calls for the fixed-tree rows
in order and 100 for the sparse-tree row, each building the complete interval
result with the original work quota.

| Interval / available lengths | Before | After | Speedup |
| --- | ---: | ---: | ---: |
| 3 bytes / fixed tree | 0.208 µs | 0.209 µs | 1.00× |
| 12 bytes / fixed tree | 0.446 µs | 0.489 µs | 0.91× |
| 64 bytes / fixed tree | 5.621 µs | 4.458 µs | 1.26× |
| 258 bytes / fixed tree | 77.301 µs | 27.979 µs | 2.76× |
| 1,024 bytes / fixed tree | 601.748 µs | 156.714 µs | 3.84× |
| 4,096 bytes / fixed tree | 2,261.565 µs | 587.588 µs | 3.85× |
| 4,096 bytes / only length 258 | 950.161 µs | 43.152 µs | 22.02× |

Each family's monotonic queue holds at most 32 suffix positions across
certificates of at most 4 KiB. Fixed match-lookup arrays shrink from 6,216 to
3,016 bytes on this host.

Artifacts: `work/efficiency-review-restoration/`,
`target/efficiency-review-restoration-baseline/`.

### `62f772a`, 1 October 2026

Batches: none; direct timings, with the 32 mixed Default inputs run twice in
alternating order.

| Workload | Before | After |
| --- | ---: | ---: |
| Solver, `02-ct-c4-c0.png` | 34 ms | 18 ms |
| Solver, `16-c3-8bits-4bits.png` | 13 ms | 3 ms |
| Total CPU, 32 mixed Default inputs | 130.22 s | 130.32 s |

A suffix minimum keeps the lowest end on a tie, preserving the source-first,
literal-next, ascending-length order. Stop checks are time-based, so moving
them from every 32 edges to every 32 positions leaves completed results
unchanged.

Measured and rejected in the same review:

- Pricing each alphabet once before the ordinary literal × distance grid kept
  identical output on 200 files but cost 0.2% more CPU; header pricing
  dominates the saved sums.
- Caching composition beam sort keys would need a context threaded through
  four call sites for a beam of at most sixteen states; the redundant final
  re-sort was removed instead.
- Preallocating CLI input from file metadata left read time (0.10 s) and peak
  memory unchanged on a 600 MB input.
- Comparing APNG frame sizes before contents is unnecessary: frame sorting
  already stops at the first differing byte of each zlib stream.

### `9ffa010`, 13 October 2026

Batches: none; whole-file CPU (user plus system) over 33 stratified Default
inputs, alternating baseline and candidate order. The first two rows ran each
input twice per build and the last row once.

| Candidate | Before | After | Speedup |
| --- | ---: | ---: | ---: |
| Compact heap builder | 410.5 s | 398.6 s | 1.03× |
| Plus reorder early exit | 408.4 s | 374.9 s | 1.09× |
| Plus variant-3 reuse | 203.5 s | 180.7 s | 1.13× |

A sampled Default profile of the same inputs put 38% of self time in the
DeflOpt-style heap builder: sift-down 20.8%, the builder 11.4% and depth
assignment 5.8%. Almost all of it builds 19-symbol code-length trees. On
`ss1.png`, 267,921 header plans built 19.3 million heaps, 18.9 million of them
for the code-length alphabet.

- **Heap builder.** Heap entries hold frequency, height and node index
  (12 bytes, was an 88-byte node with optional symbol, child and parent links,
  side, order and height). Branches record their two child indices, depths are
  assigned in one reverse pass from the root, and overflow repair indexes
  symbols directly. Leaves still enter in symbol order and branches in
  creation order, so the frequency, height and order ties are unchanged.
- **Reorder early exit.** `reorder_code_length_lengths` exchanges lengths
  where a more frequent symbol has a longer code. When every symbol at each
  length is at least as frequent as every symbol at any longer length, no
  exchange lowers the cost and the full pass changes nothing, so a check of
  per-length frequency minima and maxima returns first. Lengths of 8 or more
  fall through to the full pass.
- **Variant-3 reuse.** Heap variant 3 breaks both sift-down ties by frequency
  alone, so its DeflOpt and order-key builders return the same tree.
  `push_unique` already discarded the second copy, so candidate order and the
  earlier-wins rule are unchanged; the copy is no longer built.

Equivalence: the original builder and reorder pass are kept as test-only
references. The compact builder matched on 40,000 random frequency sets ×
4 variants × 2 tie rules × maximum lengths 7 and 15, and the early exit matched
the full pass on 200,000 cases, over 50,000 of which took it. API comparisons
ran Default with a ten-second budget and zero-budget Max on 52 inputs; 100 of
104 runs matched, and in the other four Default runs at least one side reached
the deadline. Rerun with the 180-second default budget, `world-fs8.png`,
`Banana512.png` and `y2rc2_large.png` completed on both sides with identical
output; the 2.7 MB `Partnership_Card___John_Lewis_Finance.png` does not
complete Default within that budget and was not rerun. Every CLI timing run
matched byte for byte. Rust tests, including the private corpus, passed in
release and debug modes, with documentation and Python tests, a locked build,
formatting and all-feature Clippy with warnings denied. Release executables
grew from 1,843,888 to 1,860,464 bytes.

Measured and rejected in the same review:

- Memoizing heap builds within a header plan: only 1.3 million of the
  19.3 million builds on `ss1.png` repeated an earlier input.
- A shortcut for frequency sets without ties: 408,560 builds, about 2%.
- Counting RLE frequencies inside the local rewriters: a few percent of self
  time, for a changed interface at every call site.

Artifacts: `work/efficiency-review-code-length-heap/`,
`target/efficiency-review-code-length-heap-baseline/`.

### `dc68b5f`, 13 October 2026

Batches: none; whole-file CPU (user plus system) and peak resident memory over
the same 33 Default inputs, once per build in alternating order.

| Store budget | CPU before | CPU after | Speedup | Peak memory, median / largest increase |
| --- | ---: | ---: | ---: | ---: |
| 16 MiB, unpacked entries | 180.7 s | 123.4 s | 1.46× | +5.0 / +72.1 MiB |
| 8 MiB, packed entries (retained) | 180.2 s | 130.8 s | 1.38× | +3.8 / +20.0 MiB |

Instrumented Default runs of these inputs made 1.45 million uncached header
plans. 56% repeated the code lengths and policy of a plan already completed
earlier on the same thread, and took 56% of planning time, but the route-local
caches answered only 1.2% of lookups: most block planning, such as structural
ranges, owned-block planning and boundary-graph edges, calls `plan_block`
after a canonical-cache miss, which starts a fresh 512-entry header cache.
Header plans run on one thread per input in Default.

A header plan depends only on the trimmed code lengths, Default or Max policy
and RLE mask; payload bits only shift every candidate's cost, which the
route-local cache already relies on. Misses there, and calls to
`plan_for_trimmed_lengths`, now consult a thread-local store keyed the same
way. Each entry packs the code lengths two per byte and the RLE tokens as byte
pairs into one allocation. When the current generation reaches half the
budget it replaces the previous one, and a hit in the previous generation moves
forward. Replaying the logged plan sequences with unpacked plan sizes, this
policy kept 38% of planning time at 8 MiB of plan data and 55% at 32 MiB,
close to a true LRU of equal size and against 56% unbounded. Measured peak
memory grew about 1.6 times the charged budget plus 5 MiB (4, 8 and 16 MiB on
`ss1.png` and `y2rc2_large.png`), so the 16 MiB first trial was halved.
`optimize` empties the calling thread's store on return; worker threads free
theirs on exit.

Equivalence: the store's tests cover generation turnover against
uncached plans, rejection of a fingerprint match with different lengths or
policy, and agreement with uncached planning across payload bits, Default and
Max policies and RLE masks. API comparisons on 52 inputs (39 PNG, 7 raw
Deflate, 4 ZIP, 1 GZIP, 1 zlib), in Default with the 180-second budget and in
zero-budget Max, matched in all 104 runs. Every CLI timing run matched byte for
byte. Rust tests, including the private corpus, passed in release and debug
modes, with documentation and Python tests, a locked build, formatting and
all-feature Clippy with warnings denied.

Artifacts: `work/efficiency-review-header-plan-store/`,
`target/efficiency-review-header-plan-store-baseline/`.

### `d72c937`, 23 October 2026

Batches: whole-file CPU over the 33 stratified Default inputs once per build in
alternating order; three alternating runs of each completing Max run; two
alternating runs of the header-plan replay.

A sampled profile of ten inputs in Max at 20 seconds (239,122 CPU samples) put
70% under `plan_for_trimmed_lengths_uncached`: 28% in code-length trees despite
their cache, 14% in `shortest_rle`, 10% in the DeflOpt and deft4j repeat
rewrites and 3% in the Kraft-sum check of payload code lengths. Memory
allocation took about 8%.

- **Code-length trees by histogram.** One cache entry per RLE histogram holds
  every builder's tree, so each histogram is hashed, compared and stored once
  instead of once per builder. Simulated on the completing Clefable run's
  requests, 4,096 entries (1.3 MiB per thread) built 2.42 million trees where
  the previous 16,384 slots (1.7 MiB) built 2.69 million, before the sharing
  below. Two pairs of builders always build the same tree and share one: the
  DeflOpt and order-key heaps' variant 3, which breaks every tie by frequency
  alone, and Defluff's limited builder with Columbo's variant 0, which start
  from the same Huffman tree and differ only in how they limit one deeper than
  7 bits; that pair agreed on every histogram counted.
- **Rewrites that cannot change.** The DeflOpt local rewrite and deft4j's prune
  spell out a repeat code only when that is cheaper (deft4j's pre-rebuild
  prune also when it ties), and DeflOpt makes code 17 into code 16 when 16 is
  no longer. A repeat spelled out costs more the longer it is, so the RLE's
  symbol counts and the tree show whether any token could change, without
  reading the RLE. In the Clefable run, 86% of DeflOpt rewrites changed
  nothing, and the check answered 78% of all calls; it answered 51% of
  deft4j's pre-rebuild prunes and 54% of its final ones. The deft4j path also
  counts the pruned RLE's symbols as it builds it and checks for missing codes
  from the counts.
- **Shortest RLE.** Each repeat code's window is scanned once for its shortest
  cheapest repeat instead of pricing every count, and the costs, steps and
  deque live in buffers kept for the header plan and written before they are
  read, instead of stack arrays filled on every call.
- **Allocation.** The RLE spellings, the shortest RLE and the DeflOpt rewrite
  reserve one token per code length; heap-builder, package-merge and Columbo
  merge buffers are sized once instead of growing. RLE capacity alone retired
  7% fewer instructions in the Clefable run.
- **Branch-free Kraft sum.** The completeness check of code lengths
  vectorizes, with wrapping additions that cannot overflow keeping release
  overflow checks out of the loop: 52 instead of 345 ns on 316 lengths.
- **Packed keys in Columbo's heaps.** Its single- and two-heap merges compare a
  64-bit key, frequency above order, instead of reading the nodes. Equal keys
  compare as before, so each heap moves exactly as it did; variants 2 and 3
  took 987 and 920 instead of 1,220 and 1,197 ns per code-length tree.

| Measure | Before | After | Speedup |
| --- | ---: | ---: | ---: |
| 89,255 header plans captured from ten Max runs, replayed | 5.95 s | 4.74 s | 1.26× |
| `036-Clefable-0.png`, Max completing its search | 3.58 s | 2.78 s | 1.29× |
| `114-Tangela-1.png`, Max completing its search | 0.29 s | 0.26 s | 1.12× |
| 33 Default inputs | 99.8 s | 87.2 s | 1.14× |

The Clefable run retired 63.7 instead of 78.2 billion instructions: the
shortest RLE saved 1.2%, the tree cache 2.6%, the rewrite checks and Kraft sum
8.1%, RLE capacity 7.1% and the remaining buffers and keys 0.9%, each against
the build before it. Default peak memory fell 0.1 MiB at the median and rose
2.1 MiB at most.

Deadline-bound Max cannot show equivalence. On every sixteenth DeflOpt PNG by
name (58 files, Max at 10 seconds, alternating builds), 56 outputs were
identical, and `4.2.03.png` and
`interlaced_rgba_8_should_be_grayscale_alpha_8.png` were 4 bytes and 1 byte
smaller; none grew.

Equivalence: new tests compare deft4j's prune with the previous implementation
on 20,000 random RLEs and trees under both tie rules, including its counted
frequencies, and Columbo's keyed heap merges with the previous heaps on 768
random alphabets of small counts, where leaves and branches tie; the existing
DeflOpt-rewrite and shortest-RLE comparisons still pass. On 403,865 shortest-RLE
calls captured from four Max runs, the search matched the baseline's. The
89,255 replayed header plans matched plan for plan. API comparisons on the 52
inputs, in Default with the 180-second budget and in zero-budget Max, matched
in all 104 runs, and every CLI timing run and both completing Max runs matched
byte for byte. Rust tests, including the private corpus, passed in release and
debug modes, with documentation and Python tests, a locked build, formatting,
all-feature Clippy with warnings denied and a package list without private
fixtures or scratch artifacts. `__text` grew from 840,184 to 841,128 bytes and
the shipped executable from 925,616 to 925,632.

Measured and rejected in the same review:

- Spelling each run of equal code lengths from per-value tables, since no code
  crosses a run and a run's cheapest spelling depends only on its value and
  length: 1.33× on the captured calls, but 8% slower on a small sprite whose
  lengths are mostly one run of zeros, 7.6% more instructions in the Clefable
  run, and 1.03× on the replay.
- Keeping the shortest RLE's running minimum in locals: 2.11 instead of 1.90
  seconds on 875,112 calls.
- Skipping feedback states an earlier RLE seed of the same plan covered: 11.5%
  of states, but the replay gained about 1% and the Clefable run 0.7% fewer
  instructions.
- A larger header-plan store: same-thread repeats of uncached plans fell from
  10.6% at 8 MiB to 6.1% at 16 MiB and 3.7% at 32 MiB, for that memory per
  thread.
- 8,192 tree-cache entries: 0.4% fewer instructions in the Clefable run for
  1.3 MiB more per thread.

Artifacts: `work/efficiency-review-max-trees/`.

### `0141972`, 23 October 2026

Batches: median of three alternating runs for the header-plan replay and the
completing Max runs; whole-file CPU over the 33 stratified Default inputs once
per build in alternating order.

Almost every Max run uses its whole allowance: given 60 seconds, 31 of the 33
stratified inputs searched until the deadline, so a faster Max finds more in
the same time rather than finishing sooner. A sampled profile of ten inputs in
Max at 20 seconds (245,553 CPU samples) put 73% under
`plan_for_trimmed_lengths_uncached`, 21% in `shortest_rle` and 29% in
code-length trees. `shortest_rle`, which only Max's exhaustive header search
calls, finds the cheapest RLE of a plan's code lengths under each feedback
tree. Counters found 2.55 million exhaustive header plans calling it 18.6
million times, 46% of them with a tree the same plan had already used, and the
code-length cache returning 71% of Max's trees at 4,096 slots, 75% at 16,384
and 77% at 65,536.

- **Shortest RLE once per plan and tree.** A header plan's code lengths are
  fixed, and its RLE seeds often reach the same feedback tree, so the plan
  keeps each tree's shortest RLE with its symbol counts, which the feedback
  pass counted again before.
- **Stack buffers.** The shortest-RLE search keeps its costs, steps and
  repeat-end deque in fixed buffers for a header's at most 316 code lengths,
  instead of three allocations per call; longer input takes heap buffers.
- **Larger code-length cache.** 16,384 slots, about 1.7 MiB per thread.

| Measure | Before | After | Speedup |
| --- | ---: | ---: | ---: |
| 89,255 header plans captured from ten Max runs, replayed | 6.97 s | 6.10 s | 1.14× |
| `036-Clefable-0.png`, Max completing its search | 8.48 s | 6.53 s | 1.30× |
| `114-Tangela-1.png`, Max completing its search | 0.31 s | 0.28 s | 1.11× |
| 33 Default inputs | 98.6 s | 97.8 s | 1.01× |

The Clefable run retired 78.2 instead of 105.3 billion instructions. The replay
plans every twentieth captured header plan, which spreads out the repeats the
code-length cache relies on, so it understates the cache's share. Default
peak memory rose 1.2 MiB at the median and 4.5 MiB at most, from the larger
cache.

Deadline-bound Max cannot show equivalence. On every sixteenth DeflOpt PNG by
name (58 files, Max at 10 seconds, alternating builds), 56 outputs were
identical; `4.2.03.png` was 4 bytes smaller, which did not repeat, and
`hXlpvqi4.png-medium.png` 111 bytes larger in two of three reruns at 10
seconds and equal at 20 and 40, a deadline cut like those of
[23 October](../benchmark-miss-regression-audit.md#rows-that-grew-23-october).

Equivalence: a new test compares the shortest-RLE search with the previous
implementation on 20,000 random code-length sequences and trees, including
its counted frequencies. The 89,255 replayed header plans matched the
baseline's plan for plan. API comparisons on the 52 inputs, in Default with
the 180-second budget and in zero-budget Max, matched in all 104 runs, and
every CLI timing run and both completing Max runs matched byte for byte. Rust
tests, including the private corpus, passed in release and debug modes, with
documentation and Python tests, a locked build, formatting, all-feature Clippy
with warnings denied and a package list without private fixtures or scratch
artifacts. `__text` grew from 839,848 to 840,184 bytes; the shipped executable
keeps its 925,616 bytes.

Measured and rejected in the same review:

- A 65,536-slot cache: 77% of Max's trees instead of 75%, for 6.8 MiB per
  thread.
- Keeping shortest RLEs per thread across plans: 52% of calls repeat there
  against 46% within a plan, and the key would need the plan's code lengths.

Artifacts: `work/efficiency-review-max-headers/`.

### `c94eb07`, 23 October 2026

Batches: none; whole-file CPU (user plus system) and peak resident memory over
the 33 stratified Default inputs, once per build in alternating order, while
other applications loaded the machine.

| Candidate | CPU before | CPU after | Speedup | Peak memory, median / largest increase |
| --- | ---: | ---: | ---: | ---: |
| Code-length tree cache, 4,096 slots | 125.3 s | 115.5 s | 1.08× | +0.5 / +3.2 MiB |
| Plus counted rewrites and packed heap keys | 124.7 s | 106.1 s | 1.18× | +0.1 / +2.8 MiB |
| Plus shared local-rewrite steps | 129.0 s | 102.1 s | 1.26× | +0.4 / +4.4 MiB |
| Retained | 126.4 s | 100.9 s | 1.25× | +0.2 / +3.1 MiB |

A sampled Default profile of the same inputs put 60% of CPU under
`plan_for_trimmed_lengths_uncached`, the uncached header planner, and 28% in
`make_lengths_variant_heap_inner`, the DeflOpt and order-key heap builders,
16% in its sift-down alone. Counters in a test build found 777,383 header plans
building 73.9 million code-length codes (19 symbols, at most 7 bits). Only
about 5% repeated within one header plan, but 56% repeated a builder and RLE
histogram already built on the same thread. The DeflOpt local rewrite ran
34.3 million times on RLEs of 223 tokens on average, and 68% of those calls
repeated an earlier one from the same RLE seed, with the same RLE and tree.

- **Code-length tree cache.** Each code-length builder (the DeflOpt and
  order-key heaps by variant, deft4j's Java heap, Defluff's limited and exact
  builders, and Columbo's own) is a pure function of the RLE histogram, and
  header plans for different code lengths often share one. A per-thread
  direct-mapped cache of 4,096 slots, about 0.4 MiB, keyed by builder and
  histogram, returned 46% of all code-length codes, against 49% at 16,384
  slots, 51% at 262,144 and 56% unbounded. Each hit compares the complete key,
  and the cache is emptied with the header-plan store.
- **Shared local-rewrite steps.** The four heap variants each run the DeflOpt
  local rewrite from the same RLE seed, and a step depends only on the current
  RLE and the tree the variant builds there: the prices it offers, whether the
  loop goes on, and the next RLE. A variant that builds an explored step's tree
  at the same RLE would offer the same prices again, which cannot change the
  best plan, since a plan replaces it only when strictly smaller; it moves to
  the recorded outcome instead.
- **Counted rewrites.** The local rewrite counts its output's symbols as it
  builds it instead of in a second pass, starts its output only at the first
  change, since a search's last rewrite usually changes nothing, and the loop
  takes its repeat rank from those counts instead of another pass.
- **Packed heap keys.** The heap sift-down compares one 64-bit key per entry,
  the frequency above the tie field, instead of matching the tie rule at each
  comparison.

On larger inputs, alternating builds once each:

| Input | CPU before | CPU after | Speedup |
| --- | ---: | ---: | ---: |
| `976310_99309…` (APNG) | 46.9 s | 38.3 s | 1.22× |
| `703080_80061…` (APNG) | 23.1 s | 18.2 s | 1.27× |
| `1172470_101529…` (APNG) | 40.1 s | 32.0 s | 1.25× |
| `1172470_101537…` (APNG) | 23.8 s | 19.3 s | 1.23× |
| `1328670_115967…` (APNG) | 73.0 s | 52.7 s | 1.39× |
| `bored.png` | 4.3 s | 3.7 s | 1.16× |
| `elephant-2.png` | 7.1 s | 5.3 s | 1.34× |
| `036-Clefable-0.png`, Max at 150 s, completing its search | 13.3 s | 8.5 s | 1.56× |

APNG peak memory varies by up to 60 MiB between runs of one build; medians of
three runs rose 3.5 MiB on `1328670_115967…` and 3.1 MiB on `976310_99309…`,
about one 0.4 MiB cache per APNG worker lane.

Equivalence: a new test compares the rewrite with the previous implementation
on 20,000 random RLEs and trees, including its counted frequencies, and the
existing swapping-sift test checks the packed keys under all three tie rules.
API comparisons on the 52 inputs, in Default with the 180-second budget and in
zero-budget Max, matched in all 104 runs. Every CLI timing run, the eight
larger inputs above and the completing Max run matched byte for byte. Rust
tests, including the private corpus, passed in release and debug modes, with
documentation and Python tests, a locked build, formatting, all-feature Clippy
with warnings denied and a package list without private fixtures or scratch
artifacts. `__text` grew from 907,836 to 909,776 bytes; the shipped
executable keeps its 993,120 bytes.

Measured and rejected in the same review:

- A larger cache: 16,384 slots returned 49% of codes instead of 46%, about 1%
  of CPU, for 1.7 MiB per thread.
- One tree for all eight heap variants when a build compares no equal
  frequencies, since every variant then makes the same comparisons: only 2.5%
  of code-length builds were tie-free.

Artifacts: `work/efficiency-review-code-length-trees/`.

### `04d63bd`, 20 October 2026

Batches: none; whole-file CPU (user plus system) and peak resident memory over
the 33 stratified Default inputs, once per build in alternating order, with no
competing load except where noted.

| Candidate | CPU before | CPU after | Speedup | Peak memory, median / largest increase |
| --- | ---: | ---: | ---: | ---: |
| Block-plan store, 4 MiB | 167.1 s | 149.6 s | 1.12× | +2.0 / +44.1 MiB |
| Block-plan store, 8 MiB | 167.1 s | 138.1 s | 1.21× | +2.2 / +11.8 MiB |
| Plus the kept terminal parse | 162.5 s | 129.9 s | 1.25× | +2.6 / +12.6 MiB |
| Plus the grown logarithm table | 160.0 s | 124.4 s | 1.29× | +2.7 / +12.0 MiB |
| Plus per-stream release (retained) | 164.5 s | 128.0 s | 1.29× | +1.9 / +11.9 MiB |

A sampled Default profile of the same inputs put 64% of samples under
`best_dynamic_plan_cached`, 52% in its uncached header plans, and 6% in
`parse_validated_rewrite`. An instrumented build logged every block plan:
149,738 plans took 118 seconds, and 35.4 seconds of that re-planned a token
state the same run had already planned. Plans with equal symbol counts, source
header and policy always had equal cost, and no Default run reached its stop.
By call site, `plan_block`, which the token-spelling searches and owned-block
planning use, repeated 26.0 of its 76.5 seconds; the boundary graph's and
structural ranges' edges 6.3 of 32.4; alphabet-cut edges 1.5 of 6.4; the
canonical cache 1.1 of 3.3; merges 0.1 of 1.4. Replaying the logged plans
through a two-generation store kept 6.0 seconds at 1 MiB, 15.3 at 2 MiB, 29.5
at 4 MiB and 35.0 at 8 MiB, against 35.3 unbounded.

- **Block-plan store.** Planning reads a block's tokens only through their
  symbol counts and extra bits: the fixed cost, the source tree's cost and
  every candidate tree's payload are sums over symbols. A plan therefore
  depends on the counts, the source block's own header and the strict and
  Default/Max policy, not on token order. Misses in
  `plan_reusable_block_with_header_cache` fall through to a per-thread store
  keyed by those inputs, the token count and the extra bits; a hit compares the
  used-symbol set, every count, the packed source header and the policy. Plans
  are packed like stored header plans, in two generations within an 8 MiB
  charge, and the header store now shares the generation logic
  (`plan_store.rs`). A plan is stored only when its stop is still unreached
  after planning (stops only move forward), and a call whose stop is already
  reached plans without the store, so it returns the plan its deadline allows.
- **Kept terminal parse.** Building a candidate validates it by parsing the
  emitted bytes, then dropped the parse; the next terminal method parsed the
  same bytes again. An accepted terminal candidate now hands its validated
  parse to the terminal parse cache. On the Partnership card this also removed
  the overlap of the two parses: peak memory fell from 1,042 to about 940 MiB,
  and CPU from 42.5 to 27.9 seconds with the store.
- **Grown logarithm table.** R15's scans read `c log2 c` for each side's
  totals and its frequent symbols' counts at every token, and the table
  covered counts below 4,096 only, so large blocks computed the logarithm by
  repeated squaring at each cut. The table now grows to the block's token
  count (at most 2^19 entries) with the same values. On 12 of the inputs, run
  twice under load, the entropy-cut step fell from 16.0 to 9.8 seconds.
- **Per-stream release.** APNG Default runs up to eight worker lanes, each
  with its own store kept across frames, and the largest stickers' peak memory
  rose by 20 to 42 MiB. Plans rarely repeat across streams, so each stream's
  optimization now empties its thread's store on return: on five large APNG
  files peak memory differed from the baseline by −9 to +8 MiB, within its
  run-to-run spread of up to 15 MiB, and CPU fell 0–8% (`976310_99309…` 52.3
  → 48.3 seconds).

Equivalence: new tests compare stored plans with uncached planning for random
blocks in both token orders, with and without a source header, across
policies; reject a fingerprint match with different counts, source header or
policy; check that reached stops neither use nor fill the store; and check
the grown table and the kept parse against direct computation. API
comparisons on the 52 inputs, in Default with the 180-second budget and in
zero-budget Max, matched in all 104 runs. Every CLI timing run and five large
APNG files matched byte for byte. Rust tests, including the private corpus,
passed in release and debug modes, with documentation and Python tests, a
locked build, formatting, all-feature Clippy with warnings denied and a package
list without private fixtures or scratch artifacts. The final source matches
the tested snapshot. The release executable grew from 1,910,064 to 1,926,608
bytes, all of it the store's code: without it the build is 1,910,080 bytes.

Measured and rejected in the same review:

- Skipping the validation parse of a strict candidate that already loses to
  its source: 4 of 399 validations, 0.12 seconds.
- A 4 MiB store: 11.5 seconds less saving than 8 MiB on the same inputs.
- Pruning tree pairs in `best_dynamic_plan_cached` by payload plus a header
  lower bound: the cheapest header is 26 bits, while candidate trees differ by a
  few payload bits against headers of hundreds, so it would skip almost none.
- Prefix sums of literal costs for match expansion: each call already visits
  each decoded byte once, so they would not reduce its work.

Artifacts: `work/efficiency-review-block-plan-store/`.
