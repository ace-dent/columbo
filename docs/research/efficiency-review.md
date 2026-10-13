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
