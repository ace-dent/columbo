# Efficiency review

A dated log of merged changes that remove repeated calculation and copying
without changing output: which search routes run, their tie-breaking, work
budgets and stop checks stay the same. Reviews that merged nothing are
logged under [Reviews without a merged change](#reviews-without-a-merged-change),
so that their measured dead ends are not tried again.

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
| 24 Oct 2026 | [`71ecf9f`](#71ecf9f-24-october-2026) | Default terminal and header work (`slide.rs`, `search.rs`, `header.rs`) | Slide boundaries skipped while both blocks are unchanged since their last search found no cheaper cut; literal-cost sums stopped once they decide the comparison with the match; code-length runs found once for every RLE seed | Default CPU 1.11× (33 inputs); completing Max runs 1.04× and 1.00× (3.4% and 1.1% fewer instructions); peak memory unchanged | 104 API (52 inputs; Default at the 180-second budget and zero-budget Max); 33 Default inputs; 89,255 replayed header plans; 2 completing Max runs; 20,000 literal-sum, 20,000 joining-price and 20,000 run-list comparisons; 672 Rust, 7 Python tests |
| 24 Oct 2026 | [`e79a60a`](#e79a60a-24-october-2026) | Tree builders, RLE counts, bit reading and plan-cache keys (`huffman.rs`, `header.rs`, `bitstream.rs`, `block.rs`) | Heap entries carry packed keys and sift-downs specialize by tie rule; deft4j's and the generic builder's buffers shared and deft4j's heap keyed; RLE counts in four tables and costs without saturation; rewrites adjust counts at changes; the bit reader loads eight bytes at a time; the planner reuses the block key's extra bits; the canonical cache keys on counts | Default CPU 1.21× (33 inputs); large inputs 1.11–1.27×; completing Max runs 1.08× and 1.04× (6.1% and 2.9% fewer instructions); captured header plans 1.12×; peak memory unchanged | 104 API (52 inputs; Default at the 180-second budget and zero-budget Max); 33 Default and 5 large inputs; 89,255 replayed header plans; 2 completing Max runs; 6,000 shared-scratch and 4,000 bit-reader oracle comparisons; 674 Rust, 7 Python tests |
| 25 Oct 2026 | [`bab3748`](#bab3748-25-october-2026) | Short-family pricing, tree depths, package-merge, header-plan buffers, RLE rewrites (`search.rs`, `huffman.rs`, `header.rs`) | The short-family floor prices trees from its tokens' counts; merged trees get their depths in one pass; package-merge counts lengths in one pass; each thread reuses its header-plan search buffers; the RLE rewrites copy unchanged runs whole | Default CPU 1.05× (33 inputs); captured header plans 1.04×; a completing Max run 1.2% fewer instructions; peak memory unchanged | 104 API (52 inputs; Default at the 180-second budget and zero-budget Max); 33 Default inputs; 89,255 replayed header plans; 2 completing Max runs; 20,000 package-merge and 6,000 depth comparisons; 677 Rust, 7 Python tests |
| 25 Oct 2026 | [`d96ab5c`](#d96ab5c-25-october-2026) | Max header search and ordered queue (`header.rs`, `huffman.rs`, `search.rs`) | The generic builder's heap merges reuse their buffers; the shortest RLE picks its earliest minimum without branches, is counted in four tables and compared with the local rewrite from counts; the code-length tree slot is hashed in four lanes; the ordered queue's visited token sequences use a multiply-rotate hash instead of SipHash | Completing Max runs 3.4% and 10.5% fewer instructions; captured header plans 1.03×; Default CPU unchanged | 104 API (52 inputs; Default at the 180-second budget and zero-budget Max); 89,255 replayed header plans; 2 completing Max runs; 58 Max runs at 10 seconds; 20,000 minimum and 20,000 cost comparisons; 679 Rust, 7 Python tests |
| 25 Oct 2026 | [`2c702f5`](#2c702f5-25-october-2026) | Max shortest RLE and header caches (`header.rs`, `huffman.rs`) | Each run of equal code lengths spelled from per-thread tables by run length and costs; a plan's runs found once for its seeds and shortest RLEs; eight code lengths per fingerprint step; one tree-cache slot per feedback pass, histograms compared without `memcmp`; merge and best-plan buffers reused; plans packed in pairs | Captured shortest-RLE calls 3.2×; captured header plans 1.19×; completing Max runs 46.2% and 12.8% fewer instructions; Default CPU 1.03× | 104 API (52 inputs; Default at the 180-second budget and zero-budget Max); 89,255 replayed header plans; 403,865 captured shortest-RLE calls; 2 completing Max runs; 58 Max runs at 10 seconds; 6,000 kept-scratch and 2,000 packing comparisons; 680 Rust, 7 Python tests |
| 25 Oct 2026 | [`e25d20d`](#e25d20d-25-october-2026) | Header feedback passes (`header.rs`, `huffman.rs`) | A pass whose first code-length tree is a complete Huffman tree that fits 7 bits unrepaired prices and follows it alone, since every other code can at best tie; seeds and shortest RLEs counted as spelled | Captured header plans 1.44× (24.8% fewer instructions); completing Max runs 22.7% and 4.3% fewer instructions; Default CPU 1.08× | 104 API (52 inputs; Default at the 180-second budget and zero-budget Max); 89,255 replayed header plans; 2 completing Max runs; 58 Max runs at 10 seconds; 20,000 histogram comparisons; 681 Rust, 7 Python tests |
| 28 Oct 2026 | [`3b5fa64`](#3b5fa64-28-october-2026) | Block sums, heap trees and package-merge (`block.rs`, `header.rs`, `huffman.rs`, `search.rs`) | A block's extra bits, fixed-code price and source-header payload from its symbol counts; a variant-0 heap build that finds no comparison a tie rule could decide gives its tree to every variant sharing those rules, in the code-length tree cache and the payload tree lists; package-merge's buffers kept per thread | Default CPU 1.07× (33 inputs, 6.7% fewer cycles); replayed header plans 1.04×; a completing Max run 3.2% fewer instructions; peak memory −0.4 MiB median | 104 API (52 inputs; Default at the 180-second budget and zero-budget Max); 89,255 replayed header plans; 2 completing Max runs; 66 Default timing runs; 20,000 tracked-heap and 20,000 shuffled tree-cache comparisons; 688 Rust, 7 Python tests |
| 28 Oct 2026 | [`a6f67cd`](#a6f67cd-28-october-2026) | Heap entries, match expansion and RLE seeds (`huffman.rs`, `search.rs`, `header.rs`) | The DeflOpt-style heap packs frequency, tie field and index into one word; match expansion keeps its first walk's answers and spells the result out of line; a greedy RLE seed that only repeats an earlier mask's is not spelled | Default CPU 1.04× (33 inputs, 3.0% fewer cycles); replayed header plans 1.04×; a completing Max run 4.6% fewer instructions; `__text` 4.2 KB smaller | 104 API (52 inputs; Default at the 180-second budget and zero-budget Max); 89,255 replayed header plans; 2 completing Max runs; 66 Default timing runs; 20,000 expansion and 5,000 long-run seed comparisons; 689 Rust, 7 Python tests |
| 28 Oct 2026 | [`46b7c44`](#46b7c44-28-october-2026) | Boundary slide, Huffman decoding and paired match expansions (`slide.rs`, `bitstream.rs`, `huffman.rs`, `search.rs`) | The slide's cut search walks the pair once, pricing each token once; decoding takes each code and its extra bits from one 32-bit window; the strict and tie-including expansions under the same codes share one walk | Default CPU 1.02× (33 inputs, 2.1% fewer cycles); peak memory unchanged | 104 API (52 inputs; Default at the 180-second budget and zero-budget Max); 89,255 replayed header plans; 2 completing Max runs; 66 Default timing runs; 20,000 cut-search and 20,000 paired-expansion comparisons; 691 Rust, 7 Python tests |
| 28 Oct 2026 | [`c2aa4ba`](#c2aa4ba-28-october-2026) | Max header-plan buffers, expansion marks and generic heap entries (`header.rs`, `source_recode.rs`, `huffman.rs`) | The header planner's rewrites, local-rewrite steps, candidate trees, shortest-RLE memo and seed spellings use buffers kept per thread; the DeflOpt rewrite is skipped when the RLE already costs the shortest RLE's; source recode's expansion marks stop their literal sums at the match's cost; the generic builder's heap entries take one word | Replayed header plans 1.10× (5.5% fewer instructions); a completing Max run 7.1% fewer instructions; Default CPU 1.02× (33 inputs, 1.8% fewer cycles); peak memory −0.2 MiB median | 104 API (52 inputs; Default at the 180-second budget and zero-budget Max); 89,255 replayed header plans; 2 completing Max runs; 58 Max runs at 10 seconds; 66 Default timing runs; 20,000 expansion-mark comparisons; 692 Rust, 7 Python tests |
| 28 Oct 2026 | [`4013a14`](#4013a14-28-october-2026) | Code-length trees and feedback passes (`huffman.rs`, `header.rs`, `search.rs`) | Heap builds note tie turns per sift-down, and every variant whose rules agree shares the tree; the code-length tree cache tracks every heap build; an exhaustive pass prices a repaired first tree alone at exact Defluff's cost; seeds stop at a shortest RLE already followed; code-length heaps on stack arrays, a 19-length completeness check and exact Defluff on kept scratch | Replayed header plans 1.16× (9.4% fewer instructions, 11.0% fewer cycles); Default CPU 1.05× (33 inputs, 4.3% fewer cycles); a completing Max run 3.6% fewer instructions; executable one 16 KiB page larger | 104 API (52 inputs; Default at the 180-second budget and zero-budget Max); 89,255 replayed header plans; 2 completing Max runs; 58 Max runs at 10 seconds; 66 Default timing runs; 20,000 stack-array, 20,000 completeness and 20,000 first-tree comparisons; 694 Rust, 7 Python tests |
| 28 Oct 2026 | [`6e6a0c4`](#6e6a0c4-28-october-2026) | Per-thread header-plan store (`header.rs`) | Each generation's plans appended to one byte arena with an offset index; one byte per RLE token, code lengths checked by decoding the RLE instead of stored; 16 MiB budget | 177 completing Max runs 1.06× (6.0% fewer instructions); Default CPU 1.03× (33 inputs, 3.1% fewer cycles); peak memory −1.1 MiB median Default, −4 MiB median Max, +39 MiB largest | 104 API (52 inputs; Default at the 180-second budget and zero-budget Max); 177 completing and 2 long completing Max runs; 89,255 replayed header plans; 58 Max runs at 10 seconds; 66 Default timing runs; 65,536 token-encoding checks; 695 Rust, 7 Python tests |
| 2 Nov 2026 | [`3860542`](#3860542-2-november-2026) | Max length-swap trials and header checks (`header.rs`, `header/swap_costs.rs`, `huffman.rs`) | Each thread keeps the header costs of greedy length-swap trials by start lengths and pair, in sets of four that grow with use up to 3 MiB; the payload shape check counts codes in its Kraft pass; the shortest-RLE repeat test compares counts before tokens | Completing Max runs of three small deadline-bound PNGs 17.2%, 17.0% and 7.7% fewer instructions (1.21×, 1.19×, 1.07×); 177 completing sprites 4.0% fewer; Default 0.86% fewer; sprite peak memory +2 MiB median | 104 API (52 inputs; Default at the 180-second budget and zero-budget Max); 3 long and 177 completing Max runs; 11,343 replayed swap searches; 275,887 replayed header plans; 58 Max runs at 10 seconds; 66 Default timing runs; 20,000 shape-check comparisons; 703 Rust, 7 Python tests |
| 3 Nov 2026 | [`a540aac`](#a540aac-3-november-2026) | Validated parses and the bit reader (`optimize.rs`, `model.rs`, `bitstream.rs`) | Each thread keeps its last validated parse, which a reparse of the same bytes and limit shares; the endpoint-proven lineage takes its candidate's validating parse; the bit reader derives its position from its byte position | Default CPU 1.015× (33 inputs, 1.69% fewer instructions, 1.32% fewer cycles); Partnership card 1.03×; peak memory +0.2 MiB median, +2.7 MiB largest | 104 API (52 inputs; Default at the 180-second budget and zero-budget Max); 66 Default timing runs; 2 completing Max runs; 673 Rust, 7 Python tests |
| 3 Nov 2026 | [`4ebfefd`](#4ebfefd-3-november-2026) | Exact Defluff trees and swap-trial costs (`huffman.rs`, `header.rs`, `header/swap_costs.rs`) | Package-merge's fallback built only when it fails; one unconstrained Defluff tree per alphabet for every depth limit; the limited Columbo tree from an unrepaired variant-0 build; swap-trial costs in up to 2^19 slots | Completing Max runs of three small PNGs 6.6%, 3.4% and 4.6% fewer instructions; 177 completing sprites 2.3% fewer; 14,440 replayed large-block searches 3.1% fewer; Default 0.82% fewer | 104 API (52 inputs; Default at the 180-second budget and zero-budget Max); 3 long and 177 completing Max runs; 14,440 replayed large-block searches; 58 Max runs at 10 seconds; 66 Default timing runs; 16,000 tree-builder comparisons; 674 Rust, 7 Python tests |
| 3 Nov 2026 | [`851e373`](#851e373-3-november-2026) | Payload decoding, symbol counts, match expansion and the short-family floor (`parse.rs`, `huffman.rs`, `bitstream.rs`, `model.rs`, `search.rs`) | The parser's payload loop decodes through an inlined table path; symbol counts alternate two tables with wrapping additions; expansions copy unchanged token runs whole; the short-family floor prices its bands from one walk's count changes and spells only a winner; expansion walks price bytes from a 256-entry table with a cheapest-byte bound | Default CPU 1.069× (33 inputs, 7.84% fewer instructions, 6.27% fewer cycles); completing Max runs 0.4% and 4.2% fewer instructions; peak memory median unchanged, `Banana512.png` +22.1 MiB of cached freed blocks | 104 API (52 inputs; Default at the 180-second budget and zero-budget Max); 33 Default inputs with debug assertions; 66 Default timing runs; 2 completing Max runs; 4,000 symbol-count comparisons; 675 Rust, 7 Python tests |
| 3 Nov 2026 | [`58ccf01`](#58ccf01-3-november-2026) | APNG header fit: joint tree and RLE search and symbol-set spellings (`joint.rs`, `symbol_set.rs`) | R4 charges each state's edges at once and relaxes each edge family in its own loop; explicit lengths and zero runs relaxed once per position and capacity from the first cheapest state; reached capacities kept in a bitset; R5 keeps each match's restricted spelling by the banned symbols the solver reads | APNG Default CPU 1.48× and wall time 1.50× (62 inputs, 39.85% fewer instructions, 29.78% fewer cycles); 33 Default inputs 2.72% fewer instructions; completing Max runs 2.3% and 1.4% fewer; APNG peak memory median −6.9 MiB | 104 API (52 inputs; Default at the 180-second budget and zero-budget Max); 248 APNG runs at 180 and 4,000 seconds; 62 APNG inputs with debug assertions; 4,168 replayed R4 searches per step; 2,000 R4 comparisons; 677 Rust, 7 Python tests |
| 3 Nov 2026 | [`4aeb053`](#4aeb053-3-november-2026) | R4 arithmetic, R5 rewrites and header-planning buffers (`joint.rs`, `symbol_set.rs`, `header.rs`, `huffman.rs`) | R4's index and bound arithmetic wraps where it cannot overflow; R5 rewrites only a block's matches, scores the block's counts with their changes and spells tokens only for kept proposals; depth repair, RLE seeds and the first best header plan reuse or reserve their buffers | APNG Default CPU 1.063× and wall time 1.059× (62 inputs, 6.30% fewer instructions, 5.70% fewer cycles); 4,168 replayed R4 searches 19.1% fewer instructions; 33 Default inputs 1.07% fewer; completing Max runs 1.1% and 0.3% fewer | 104 API (52 inputs; Default at the 180-second budget and zero-budget Max); 248 APNG runs at 180 and 4,000 seconds; 62 APNG inputs with debug assertions; 4,168 replayed R4 searches; full-token rewrite comparisons on 8 blocks; 678 Rust, 7 Python tests |
| 3 Nov 2026 | [`3bb4834`](#3bb4834-3-november-2026) | Greedy run-length spelling and payload sums (`header.rs`) | The greedy speller writes its tokens in place and spells runs too short for a repeat code without branching; a code's data bits are summed without branches or overflow checks in one out-of-line copy | APNG Default CPU 1.036× and wall time 1.040× (62 inputs, 2.85% fewer instructions, 3.55% fewer cycles); 33 Default inputs 2.27% fewer instructions, 3.27% fewer cycles; completing Max runs 2.8% and 2.5% fewer | 104 API (52 inputs; Default at the 180-second budget and zero-budget Max); 248 APNG runs at 180 and 4,000 seconds; 62 APNG inputs with debug assertions; 160,000 greedy-spelling and 20,000 payload-sum comparisons; 679 Rust, 7 Python tests |
| 4 Nov 2026 | [`7d4e44c`](#7d4e44c-4-november-2026) | Max swap-trial costs and header-plan store (`header.rs`, `header/swap_costs.rs`) | Swap-trial costs in 8-byte slots, eight to a cache line, up to 2^22; trials' plans kept out of the header-plan store; 64 MiB store budget once a thread plans a Max block | Completing Max runs of three small PNGs 48.0%, 32.1% and 28.1% fewer instructions (1.85×, 1.44×, 1.39×); 177 completing sprites 1.34% fewer; Default unchanged; one of ten Max inputs at 20 seconds peaked 46–51 MiB higher | 104 API (52 inputs; Default at the 180-second budget and zero-budget Max); 4 long and 177 completing Max runs; 58 Max runs at 10 seconds; 66 Default timing runs; 684 Rust, 7 Python tests |
| 4 Nov 2026 | [`9a3f174`](#9a3f174-4-november-2026) | Header planning: tree values, runs, shape checks, buffers and the store's hash (`header.rs`) | Cached code-length trees keep their shape, order, HCLEN and RLE cost; runs found eight lengths at a time; known shapes not checked again; the planner's buffers borrowed in place; the store indexed by its fingerprints | Completing Max runs of three small PNGs 9.7%, 7.9% and 11.0% fewer instructions (1.10×, 1.07×, 1.13×); 177 completing sprites 7.24% fewer (1.062×); replayed swap searches 15.6% fewer; APNG 1.31% and Default 1.04% fewer | 104 API (52 inputs; Default at the 180-second budget and zero-budget Max); 3 long and 177 completing Max runs; 58 Max runs at 10 seconds; 124 APNG runs at 4,000 seconds; 66 Default timing runs; three replays; 20,000 run-finder comparisons; 686 Rust, 7 Python tests |
| 6 Nov 2026 | [`b35e8ee`](#b35e8ee-6-november-2026) | Boundary slide across finishing steps (`slide.rs`) | Each thread keeps the boundaries a stream's last slide left settled, with the left block's offset, both blocks' token buffers and trees; a later slide of the stream starts a pair settled when it matches one in full | Default CPU 1.012× (33 inputs, 2.11% fewer instructions, 1.42% fewer cycles); Partnership card 1.045× (5.87% fewer instructions); peak memory median unchanged, +6.2 MiB largest | 104 API (52 inputs; Default at the 180-second budget and zero-budget Max); 66 Default timing runs; 2 completing Max runs; 900 slides through the memo compared with fresh slides; 688 Rust, 7 Python tests |

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

### `b35e8ee`, 6 November 2026

Batches: retired instructions and cycles from `time -l` over the 33 stratified
Default inputs, two alternating rounds, for every candidate and for the final
build; whole-file Default CPU (user plus system) and peak resident memory over
the same inputs, one round alternating the builds; two alternating runs of
each completing Max run.

A sampled profile of the 33 Default inputs at `b35e8ee` (30,230 CPU samples)
was flat after the earlier rounds. Header planning was about half of the
samples, 22% of them building code-length trees, which probes showed were
not repeated work: the per-thread tree cache answered 69% of 52.7 million
lookups, and only 5.9% of its builds and 0.6% of the header store's misses
were for keys seen before. The boundary slide (R1c) took 5.9% of the samples
in its own code, 12–14% on the four slowest slide-heavy inputs. A probe that
fingerprinted every pair the slide searched found that 48% of the 11,264
pair searches, 36% of the 327 million tokens walked, repeated a pair, with the
same tokens and trees, that an earlier slide of the stream had found settled.
Each slide starts afresh, and the finishing steps between two slides change
few blocks.

- **Settled boundaries kept between slides.** Each thread keeps the
  boundaries that the stream's last slide left settled: the left block's
  decoded offset, both blocks' token buffers (shared, not copied) and both
  blocks' code lengths, in stream order. A later slide finds each boundary's
  entry by binary search on its offset, compares token counts and trees, then
  the tokens in full, and starts a pair that matches as settled, so its sweep
  skips the pair until a block changes, as it already did within a slide. The
  cut search depends only on the two blocks' tokens, decoded bytes and trees,
  and every candidate of a stream decodes to the same bytes, so the offset
  fixes the decoded bytes. The memo belongs to the stream's identity (decoded
  size, CRC-32, Adler-32), keeps one slide's boundaries and is released after
  the stream.

| Measure | Before | After | Change |
| --- | ---: | ---: | ---: |
| 33 Default inputs, instructions retired over two rounds | 945.7 billion | 925.7 billion | −2.11% |
| Same inputs, cycles over two rounds | 230.9 billion | 227.6 billion | −1.42% |
| Same inputs, CPU over one round | 37.8 s | 37.4 s | 1.012× |
| `Partnership_Card___John_Lewis_Finance.png`, instructions retired over two rounds | | | −5.87% |
| Same input, cycles over two rounds | | | −4.47% |
| Same input, Default CPU | 8.77 s | 8.40 s | 1.045× |
| `036-Clefable-0.png` and `114-Tangela-1.png`, Max completing its search | | | Instructions within 0.2% |

The skipped walks run at about four and a half instructions a cycle, so
cycles fall by less than instructions. Median Default peak memory did not
change; `download_webp__260×280_.png` peaked 6.2 MiB and `Lemon512.png` 5.5 MiB
higher, the token buffers the memo keeps from one slide to the next.

Equivalence: a new test slides 300 random streams through the memo and
compares each result with a slide without it: the same blocks slid again,
and the last slide's output reparsed with fresh buffers, 900 comparisons with
more than 300 boundaries taken from the memo. API comparisons on the 52
inputs, in Default with the 180-second budget and in zero-budget Max, matched
in all 104 runs, and every Default timing and instruction run and every
completing Max run matched byte for byte. Rust tests, including the private
corpus, passed in release and debug modes, with documentation and Python
tests, a locked build, formatting and all-feature Clippy with warnings denied.
`__text` grew from 911,552 to 915,708 bytes; the shipped executable keeps its
991,504 bytes, and the final source builds the tested executable byte for
byte.

Measured and rejected in the same review:

- A first version of the memo that hashed every block's tokens on each slide
  and looked boundaries up in a hash table: 1.71% fewer instructions but only
  0.18–0.34% fewer cycles, and 16 KiB more executable, one page step.
- deft4j's code-length tree builder on stack arrays of the alphabet's size:
  5–8% faster alone, but 0.28% more instructions and 0.64% more cycles in the
  whole program. The build's cost is the heap work that reproduces deft4j's
  ties.
- A 256-entry literal-cost table and cheapest-literal bound in the slide's
  pricing: 0.5% more instructions and 0.1–0.4% fewer cycles, within noise,
  since the byte-by-byte sum already stopped after one or two bytes.

Not changed in the same review:

- deft4j's code-length tree could take a heap variant's tree when no
  comparison was decided by a tie, but no tie being noted does not prove a
  unique merge order: a heap can choose between equal weights it never
  compared.
- Ranges cut from a composite still copy their tokens (about 2% of samples);
  sharing its storage needs a slice-view token model.

Artifacts: `work/efficiency-review-default9-20261106/`.

### `9a3f174`, 4 November 2026

Batches: one paired run of each build, alternating order, on the three small
PNGs whose Max search completes within 1,200 seconds; two alternating runs of
each completing Max run; one paired run of each build on the 177 sprites whose
Max search completes within 10 seconds and on every sixteenth DeflOpt PNG by
name (58 files, Max at 10 seconds); two rotating runs of each replay (275,887
header plans, 11,343 greedy length-swap searches and 14,440 large-block
dynamic-plan searches, captured for `4ebfefd`); retired instructions and
cycles over the 62 APNG inputs of [`58ccf01`](#58ccf01-3-november-2026) at
4,000 seconds and the 33 stratified Default inputs, two alternating rounds
each, and Default CPU and peak memory, one round.

A sampled profile of the three small PNGs' completing Max runs (167,053 CPU
samples) put 65% of header planning in greedy length-swap trials and 29% in
the block planner's tree-candidate pairs. In a profiling build with the
planner's helpers kept out of line, finding a plan's runs, spelling its seeds
and scanning for seed opportunities took 15% of samples; checking payload and
code-length tree shapes, testing whether code lengths follow their counts and
pricing the RLE under each candidate tree 8%; and SipHash, hashing the
header-plan store's fingerprints again, 1.3%. Each feedback pass worked out a
cached tree's shape, order and RLE cost again, though each is fixed by the
tree and the histogram that keys the cache, and each plan moved the planner's
per-thread buffers out of their cell and back.

- **Cached code-length trees keep what a search asks of them.** With its
  lengths, each tree in the cache keeps whether its shape is valid, whether its
  lengths follow the histogram's counts, so DeflOpt's reordering cannot change
  them, its HCLEN and the RLE's cost under it. A feedback pass, the DeflOpt
  local rewrite and deft4j's pruned header price a cached tree from these. An
  entry grows from 346 to 472 bytes, the cache to 1.8 MiB a thread.
- **Runs eight lengths at a time.** A plan's runs of equal code lengths come
  from comparing 8-byte words with the words one length earlier, which marks
  each run's start without a branch per length.
- **Shapes checked once.** The planner does not check the payload trees'
  shapes again after the route-local cache's one caller has, or for a swap
  trial, which exchanges two lengths of one alphabet.
- **Buffers in place.** The planner borrows its per-thread buffers where they
  lie instead of moving them out of a cell and back for every plan.
- **Fingerprints as the store's hash.** The per-thread header-plan store's
  index uses the fingerprint, already a mixed 64-bit hash, as its hash.

| Measure | Before | After | Change |
| --- | ---: | ---: | ---: |
| `star.png`, Max completing its search, instructions retired | 560.3 billion | 505.7 billion | −9.7% |
| Same run, user CPU | 38.2 s | 34.6 s | 1.10× |
| `basi4a16.png`, Max completing its search, instructions retired | 548.7 billion | 505.5 billion | −7.9% |
| Same run, user CPU | 39.4 s | 36.7 s | 1.07× |
| `crossroads.png`, Max completing its search, instructions retired | 1,668.0 billion | 1,485.1 billion | −11.0% |
| Same run, user CPU | 111.7 s | 98.7 s | 1.13× |
| 177 sprites, Max completing its search, instructions retired | 2,718.6 billion | 2,521.7 billion | −7.24% |
| Same runs, user CPU | 224.1 s | 211.0 s | 1.062× |
| `036-Clefable-0.png`, Max completing its search, instructions retired | 17.15 billion | 15.78 billion | −8.0% |
| `114-Tangela-1.png`, Max completing its search, instructions retired | 2.143 billion | 2.119 billion | −1.1% |
| 11,343 replayed swap searches, instructions retired | 559.5 billion | 472.0 billion | −15.6% |
| Same replay, cycles | 106.2 billion | 91.0 billion | −14.3% |
| 14,440 replayed large-block searches, instructions retired | 217.2 billion | 209.6 billion | −3.5% |
| 275,887 replayed header plans, instructions retired | 65.08 billion | 63.93 billion | −1.8% |
| 62 APNG inputs at 4,000 seconds, instructions retired over two rounds | 3,403.7 billion | 3,359.3 billion | −1.31% |
| Same inputs, cycles over two rounds | 1,001.7 billion | 986.5 billion | −1.52% |
| 33 Default inputs, instructions retired over two rounds | 955.35 billion | 945.46 billion | −1.04% |
| Same inputs, cycles over two rounds | 232.63 billion | 229.27 billion | −1.44% |

Measured step by step on the swap replay, each against the step before,
skipping known shapes took 2.28% of the instructions (2.0% of the cycles), the
store's own hash 0.46% (on completing runs 0.4% of the instructions but 1.4% to
1.8% of the CPU), runs eight lengths at a time 3.11% (3.9%), the buffers in
place 1.65% (1.9%) and the trees' kept values 8.84% (6.8%); the last also took
2.6% off the block replay. Default and APNG gain through the same header
planning.

Peak memory of the 177 sprites rose 1 MiB at the median and 6 MiB at most,
and the largest fell from 91 to 86 MiB; Default peak memory rose 0.6 MiB at
the median and 4.2 MiB at most. Each file's APNG peak rose 2.9 and 4.0 MiB at
the median in the two rounds, about the larger tree cache of each lane; single
runs ranged from 40 MiB lower to 68 MiB higher, and four runs of one build on
one file peaked at 141 to 201 MiB as its lanes overlapped differently.

Deadline-bound Max cannot show equivalence, so it was checked for losses: on
every sixteenth DeflOpt PNG by name, 57 of 58 outputs had the same size and 56
were identical, in 440.7 instead of 442.4 seconds. `merge.png` was 2 bytes
larger, 2,857 bytes, the baseline's own endpoint at 11 and 12 seconds: the
baseline gives 2,855 bytes at 10 seconds and 2,856 at 14, the candidate 2,856
at 11 and 12. Run to completion, both give 2,856 bytes, the candidate with 7.0%
fewer instructions. `test-conversion-truecoloralpha-paletted.png` gave other
bytes of the same size; in three more runs at 10 seconds each build gave
either output, one of them the bytes both builds' completed searches give.

Equivalence: new tests compare the run finder with a scan on 20,000 random
code-length sequences from a stale buffer, and pin the cache entry's 472
bytes; debug assertions check the kept RLE cost at every pricing, every
skipped shape check and the reused feedback cost against a fresh computation.
The replays matched plan for plan. API comparisons on the 52 inputs, in
Default with the 180-second budget and in zero-budget Max, matched in all 104
runs, and every Default, APNG and completing Max run matched byte for byte. A
release build with debug assertions completed the five completing Max runs
above and a Default run without a failed assertion and matched them byte for
byte. Rust tests, including the private corpus, passed in release and debug
modes, with documentation and Python tests, a locked build, formatting and
all-feature Clippy with warnings denied. `__text` grew from 906,980 to 909,512
bytes, leaving 6,682 bytes of the executable's last 16 KiB page; the shipped
executable keeps its 991,488 bytes, and the final source builds the tested
executable byte for byte.

Measured and rejected in the same review:

- Pricing a swap of two lengths that each stand alone, between neighbors
  unequal to both, as its start lengths: every plan the swap gives is the
  start's with two tokens exchanged, so its header costs the same (28,282
  planned trials checked), but only 0.4% of planned trials qualify.
- The balanced seed written in place and seed opportunities in closed form took
  0.76% and 0.17% of the swap replay's instructions, and in-place spellings
  without clearing their buffers added 0.2%, none with a gain in cycles beyond
  noise.
- The fingerprint hash also for the route-local cache and the swap starts:
  hashing there was 0.02% of samples, and its code took 3.8 KiB of the page.
- Larger trial slots for the swap trials the slots still drop: 964,000 on
  `star.png`, 7% of its run, would need 32 MiB more a thread; trials repeated
  from other start lengths took 0.4%.

Artifacts: `work/efficiency-review-max10-20261104/`.

### `7d4e44c`, 4 November 2026

Batches: one paired run of each build, alternating order, on the three small
PNGs whose Max search completes within 1,200 seconds; two alternating runs of
each completing Max run; one paired run of each build on the 177 sprites whose
Max search completes within 10 seconds and on every sixteenth DeflOpt PNG by
name (58 files, Max at 10 seconds); peak memory of ten inputs in Max at 20
seconds; retired instructions and cycles over the 33 stratified Default
inputs, two alternating rounds, and their CPU and peak memory, one round.

A sampled profile of ten inputs in Max at 20 seconds (236,752 CPU samples) put
62% under the uncached header planner: 36% of all samples planning the block
planner's tree-candidate pairs and 25% the greedy length-swap trials.
Instrumented builds found much of it repeating work the thread had done and
then dropped. In a completing Max run of `star.png`, 8.2 million of 21.8
million swap trials repeated a trial whose cost had left the 2^19 slots,
taking 30 of the run's 81 seconds; most repeats came 100,000 to 1,000,000
trials apart. Trial plans, 95% of the uncached plans on `crossroads.png` in 20
seconds, also went into the per-thread header-plan store, and once trials were
kept, half of the 680,000 exhaustive plans that missed the 16 MiB store in
the `star.png` run repeated one it had dropped, 9.4 of 49 instrumented
seconds.

- **Swap-trial costs in 8-byte slots, up to 2^22.** Each slot holds a 48-bit
  key, made of a 29-bit start identifier, the alphabet and the two 9-bit
  symbols, above a 16-bit cost, and a key hashes to a set of eight slots, one
  64-byte cache line. The slots grow fourfold once they have taken half as
  many trials as they hold, up to 2^22 slots, 32 MiB.
- **Trial plans kept out of the store.** A trial whose cost the thread keeps
  no longer adds its plan to the header-plan store, which keeps its room for
  the plans that repeat.
- **A 64 MiB header-plan store in Max.** A thread's store keeps its 16 MiB
  budget until the thread plans a block exhaustively, which only Max does, and
  64 MiB from then until it is emptied.

| Measure | Before | After | Change |
| --- | ---: | ---: | ---: |
| `star.png`, Max completing its search, instructions retired | 1,077.5 billion | 560.1 billion | −48.0% |
| Same run, user CPU | 68.1 s | 36.8 s | 1.85× |
| `basi4a16.png`, Max completing its search, instructions retired | 807.6 billion | 548.5 billion | −32.1% |
| Same run, user CPU | 54.9 s | 38.3 s | 1.44× |
| `crossroads.png`, Max completing its search, instructions retired | 2,318.9 billion | 1,667.1 billion | −28.1% |
| Same run, user CPU | 149.1 s | 107.2 s | 1.39× |
| `test-conversion-truecoloralpha-paletted.png`, Max completing its search, instructions retired | 1,094.2 billion | 664.4 billion | −39.3% |
| 177 sprites, Max completing its search, instructions retired | 2,753.3 billion | 2,716.4 billion | −1.34% |
| Same runs, user CPU | 215.9 s | 213.1 s | 1.013× |
| `036-Clefable-0.png`, Max completing its search, instructions retired | 17.19 billion | 17.14 billion | −0.3% |
| `114-Tangela-1.png`, Max completing its search, instructions retired | 2.149 billion | 2.142 billion | −0.3% |
| 33 Default inputs, instructions retired over two rounds | 955.09 billion | 955.06 billion | −0.00% |
| Same inputs, cycles over two rounds | 231.26 billion | 231.53 billion | +0.12% |

Measured step by step on the three small PNGs, each against the step before,
the larger trial slots growing at half full took 36.0%, 20.7% and 19.2% of the
instructions off `star.png`, `basi4a16.png` and `crossroads.png`, keeping
trial plans out of the store 4.7%, 2.7% and 5.5%, and the 64 MiB store 14.8%,
12.0% and 5.9%. Trial slots growing when full took 6.8%, 17.0% and 32.8% off
`star.png` at 2^20, 2^21 and 2^22, and 2^23 only added 1.4% on
`crossroads.png`. A 32 MiB store took 0.0%, 3.0% and 1.1%, and 128 MiB 18.2%,
12.6% and 8.7%.

Peak memory of the 177 sprites kept its 26 MiB median, rose by 3 MiB at most,
and the largest fell from 109 to 85 MiB. Default peak memory fell 0.5 MiB at
the median. In Max at 20 seconds, nine of ten inputs peaked lower or within
10 MiB; `4.2.03.png` peaked 46 to 51 MiB higher in three runs of each build
(44 MiB with macOS malloc's large-block cache off), which the store accounts
for: a 32 MiB store raised it 15 MiB, and the trial slots alone nothing. Each
thread that fills its store can hold 48 MiB more, and one that fills its trial
slots 26 MiB more.

Deadline-bound Max cannot show equivalence, so it was checked for losses: on
every sixteenth DeflOpt PNG by name, all 58 outputs had the same size and 57
were identical, in 442.7 instead of 443.1 seconds.
`test-conversion-truecoloralpha-paletted.png` gave other bytes of the same
size in each of three runs; run to completion, both builds give the
candidate's bytes, which it now reaches within 10 seconds. At 20 seconds,
`Banana512.png` was 1 byte smaller and the other nine inputs identical.

Equivalence: new tests check that the largest trial key leaves the empty-slot
marker free, that a thread out of identifiers drops its trials and starts
over, and that exhaustive block planning raises the store's budget until the
store is released; the swap-search test now also checks that the store holds
only the passes' start and committed plans (7 against 480 kept trials). API
comparisons on the 52 inputs, in Default with the 180-second budget and in
zero-budget Max, matched in all 104 runs, and every Default timing run and
every completing Max run matched byte for byte. A release build with debug
assertions completed the five completing Max runs above without a failed
assertion and matched them byte for byte. Rust tests, including the private
corpus, passed in release and debug modes, with documentation and Python
tests, a locked build, formatting and all-feature Clippy with warnings
denied. `__text` grew from 906,576 to 906,980 bytes; the
shipped executable keeps its 991,488 bytes, and the final source builds the
tested executable byte for byte.

Measured and rejected in the same review:

- Skipping header plans that a lower bound shows cannot beat the incumbent:
  84% to 94% of tree-candidate plans and 99.9% of swap trials lose, but no
  planned candidate's payload came within 29 bits of the incumbent, the
  smallest possible header, so only a near-exact bound would prune.
- A store shared across threads: plans repeated only on another thread were
  at most 5% of planning time on the ten inputs at 20 seconds.
- A code-length tree cache four times larger: 0.6% to 0.8% fewer instructions
  on the three small PNGs.

Artifacts: `work/efficiency-review-max9-20261104/`.

### `3bb4834`, 3 November 2026

Batches: retired instructions, cycles, CPU (user plus system), wall time and
peak memory from `time -l` over the 62 APNG inputs of
[`58ccf01`](#58ccf01-3-november-2026), two alternating rounds at the default
180-second timeout and two at the 4,000-second maximum; the 33 stratified
Default inputs over two alternating rounds; and two alternating runs of each
completing Max run.

A sampled profile of the APNG inputs at `3bb4834` (84,728 CPU samples) put
21.4% in the DeflOpt-style heap merges, about half of it building payload trees
and a third code-length trees, 8.1% in R4, 7.8% in the uncached header
planner's own code and 6.05% in the greedy run-length speller. An instrumented
build counted 16 million greedy spellings of 2.35 billion runs, 146 per call,
which took about 40 cycles per run in capacity checks and mispredicted
branches; the payload sum of a code's data bits checked every addition and
branched on every symbol. Repeated inputs were rare: 10.4% of payload tree
candidate sets, 14.7% of header estimates (27% of R13's) and 2.9% of 56
million code-length trees repeated on their thread.

- **Greedy spelling in place.** The greedy speller writes its tokens into a
  buffer sized for one token per code length, plus two, instead of pushing
  each one, and a run too short for any repeat code (fewer than 3 zeros or 4
  copies of a length) writes three slots without branching and advances by its
  length.
- **Branch-free payload sums.** A code's data bits are summed over every symbol
  without branches or overflow checks, since at most 286 counts below 2^32
  times lengths below 2^8 cannot wrap a u64, with a flag for a used symbol
  that has no code. One out-of-line copy serves both alphabets and every
  caller, so the loop vectorizes without adding code.

| Measure | Before | After | Change |
| --- | ---: | ---: | ---: |
| 62 APNG inputs, instructions retired over two rounds | 3,507.3 billion | 3,407.3 billion | −2.85% |
| Same inputs, cycles over two rounds | 1,069.0 billion | 1,031.1 billion | −3.55% |
| Same inputs, CPU over two rounds | 467.7 s | 451.3 s | 1.036× |
| Same inputs, wall time over two rounds | 84.6 s | 81.4 s | 1.040× |
| Same at 4,000 seconds, instructions retired | 3,507.2 billion | 3,406.7 billion | −2.86% |
| Same at 4,000 seconds, cycles | 1,047.0 billion | 1,008.6 billion | −3.67% |
| 33 Default inputs, instructions retired over two rounds | 978.6 billion | 956.4 billion | −2.27% |
| Same inputs, cycles over two rounds | 246.4 billion | 238.3 billion | −3.27% |
| `036-Clefable-0.png`, Max completing its search, instructions retired | 17.68 billion | 17.19 billion | −2.8% |
| `114-Tangela-1.png`, Max completing its search, instructions retired | 2.21 billion | 2.15 billion | −2.5% |

Measured step by step over the APNG inputs at 4,000 seconds, each against the
step before, writing greedy spellings in place took 1.02% of the instructions
(0.55% of the cycles), the branch-free payload sums 1.48% (1.44%), and the
branch-free short runs 0.39% (1.72%). Median APNG peak memory changed by
−0.1 MiB.

Equivalence: an existing test compares the greedy speller with a scanning
reference for all eight repeat-code masks on 20,000 random code-length
sequences, starting from stale buffers, and a new test compares the payload
sum with the checked one on 20,000 random alphabets, including used symbols
whose lengths are 0 or missing. A release build with debug assertions agreed
with the final build on 61 APNG inputs at 4,000 seconds; on
`animation_icos4d.ref.png`, whose smallest frame's allowance can end its
search, its two runs gave the two outcomes, one matching a debug-assertion
baseline and the other the release build. Every 4,000-second APNG run matched
byte for byte; at 180 seconds 123 of 124 did, the exception that file again,
where the baseline's second run retired 0.72 billion instructions fewer than
its first. API comparisons on the 52 inputs, in Default with the 180-second
budget and in zero-budget Max, matched in all 104 runs, and every Default and
completing Max run matched byte for byte. Rust tests, including the private
corpus, passed in release and debug modes, with documentation and Python
tests, a locked build, formatting and all-feature Clippy with warnings denied.
`__text` shrank from 900,468 to 900,380 bytes; the shipped executable keeps
its 974,960 bytes, and the final source builds the tested executable byte for
byte.

Measured and rejected in the same review:

- The payload sum inlined at its four call sites: 1.54% fewer instructions,
  like the out-of-line copy, but 1.1 KiB more code took the executable a
  16 KiB page larger.
- In-place writes in the shortest-RLE speller: 0.25% fewer instructions and
  0.31% fewer cycles, within run-to-run noise.
- The RLE seeds' zero-run remainders in closed form: 0.04%.

Not changed in the same review:

- The heap merges (21% of samples) build trees for new inputs. Recent-input
  caches of payload tree candidates (8.8% of calls repeat within 64) or of
  R13's estimates would each save about 0.9%, but need more code than the 222
  bytes left before the executable's next 16 KiB page.
- R4 (8.1% of samples) after the previous review's changes.

Artifacts: `work/efficiency-review-apng3-20261103/`.

### `4aeb053`, 3 November 2026

Batches: retired instructions, cycles, CPU (user plus system), wall time and
peak memory from `time -l` over the 62 APNG inputs of
[`58ccf01`](#58ccf01-3-november-2026), two alternating rounds at the default
180-second timeout and two at the 4,000-second maximum; the 33 stratified
Default inputs over two alternating rounds; two alternating runs of each
completing Max run; and one run per step of the 4,168 captured R4 searches.

A sampled profile of the APNG inputs at `4aeb053` (85,879 CPU samples) put
45.4% in each frame's initial planner, 34.5% in the APNG header fit and 16.1%
in the tree finish; R4 took 12.4% and R5 17.3% (pricing proposals 10.4%,
rewrites 4.1%, estimates 2.8%). Replayed, the R4 searches retired 286 billion
instructions, 22 billion of them before the search loop, so the loop retired
about 80 instructions per edge, much of it overflow checks on its index and
bound arithmetic. An instrumented build counted 3.67 million header-plan
requests: 25% hit the thread's store, every miss was a key new to its thread,
and 0.45% of misses repeated a key another thread had planned, so a larger or
shared store would save nothing.

- **R4 arithmetic.** The search's table indices stay below its 2^21 cells and a
  score plus two bounds below 2^64, so that arithmetic wraps instead of
  carrying the release build's overflow checks.
- **R5 rewrites of matches only.** A ban changes only matches, so R5 finds a
  block's matches once, and each rewrite visits only them, scores the block's
  counts and extra bits with the replaced matches' changes, and spells the
  whole token list only for a proposal it keeps and for the final re-spelling.
  The stop is still checked every 256 tokens, in order.
- **Fewer reallocations in header planning.** Depth repair keeps its leaf keys
  in the tree builder's scratch, the uncached header planner reuses one buffer
  for its RLE seeds, and its first best plan reserves one RLE token per code
  length, so a longer improvement need not grow it.

| Measure | Before | After | Change |
| --- | ---: | ---: | ---: |
| 62 APNG inputs, instructions retired over two rounds | 3,744.5 billion | 3,508.6 billion | −6.30% |
| Same inputs, cycles over two rounds | 1,161.6 billion | 1,095.4 billion | −5.70% |
| Same inputs, CPU over two rounds | 516.5 s | 485.8 s | 1.063× |
| Same inputs, wall time over two rounds | 88.6 s | 83.6 s | 1.059× |
| Same at 4,000 seconds, instructions retired | 3,745.0 billion | 3,508.8 billion | −6.31% |
| Same at 4,000 seconds, cycles | 1,159.1 billion | 1,091.3 billion | −5.84% |
| `4162040_4062578…` (APNG), Default CPU | 10.12 s | 9.11 s | 1.11× |
| 33 Default inputs, instructions retired over two rounds | 988.6 billion | 978.0 billion | −1.07% |
| Same inputs, cycles over two rounds | 247.6 billion | 246.1 billion | −0.63% |
| `036-Clefable-0.png`, Max completing its search, instructions retired | 17.88 billion | 17.69 billion | −1.1% |
| `114-Tangela-1.png`, Max completing its search, instructions retired | 2.21 billion | 2.20 billion | −0.3% |
| 4,168 replayed R4 searches, instructions retired | 285.6 billion | 231.2 billion | −19.1% |
| Same searches, cycles | 50.0 billion | 38.8 billion | 1.29× |

Measured step by step over the APNG inputs at 4,000 seconds, each against the
step before, the R4 arithmetic took 2.28% of the instructions (1.13% of the
cycles), the match-only R5 rewrites 3.53% (3.18%), and the reallocations
0.61% (1.64%). Median APNG peak memory changed by −0.2 MiB at 180 seconds and
by 0.0 MiB at 4,000 seconds; single runs moved by up to 57 MiB either way, and
no file rose in every run.

Equivalence: a new test compares the match-only R5 rewrite with the previous
full-token rewrite for every ban and crossed ban of 8 generated 700-token
blocks, with callback stops at varied checks, matching tokens, counts, extra
bits, budget left and stop checks; the existing per-edge test covers R4, and
the replayed searches matched. A release build with debug assertions agreed
with the final build on 61 APNG inputs at 4,000 seconds and with a
debug-assertion baseline on `animation_icos4d.ref.png`, whose smallest frame's
allowance a slower build reaches. All 248 APNG runs matched byte for byte, at
both timeouts. API comparisons on the 52 inputs, in Default with the
180-second budget and in zero-budget Max, matched in all 104 runs, and every
Default and completing Max run matched byte for byte. Rust tests, including
the private corpus, passed in release and debug modes, with documentation and
Python tests, a locked build, formatting and all-feature Clippy with warnings
denied. `__text` grew from 896,188 to 900,468 bytes; the shipped executable
keeps its 974,960 bytes, and the final source builds the tested executable
byte for byte.

Measured and rejected in the same review:

- One spare tree-building scratch per thread for header estimates and payload
  tree candidates, instead of a new one per call: 0.19% fewer instructions and
  0.7% fewer cycles, but its 600 bytes of code took the executable a 16 KiB
  page larger (991,376 bytes).
- Stopping R4's code 16 repeats at the first illegal count instead of testing
  each: 0.05% of the replay's instructions.
- A header-plan store shared across worker threads: 0.45% of misses repeated
  another thread's key.

Not changed in the same review:

- The header planner shared with all Default work remains about half of APNG
  samples; its 2.74 million uncached plans per pass were all new.
- R5's pricing of proposals (10.5% of samples) and the header estimates of R5,
  R13 and R15 (6%).

Artifacts: `work/efficiency-review-apng2-20261103/`.

### `58ccf01`, 3 November 2026

Batches: retired instructions, cycles, CPU (user plus system), wall time and
peak memory from `time -l` over 62 APNG inputs, two alternating rounds at the
default 180-second timeout and two at the 4,000-second maximum, where no frame
allowance binds; the 33 stratified Default inputs over two alternating rounds;
two alternating runs of each completing Max run; and one run of a replay of
4,168 captured R4 searches per step. The APNG inputs are 32 Steam stickers (24
at evenly spaced quantiles of Default time over the 2,431-sticker corpus, the 5
large stickers of earlier reviews and the 3 slowest) and all 30 files in
`apng-small`, `apng-medium`, `apng-large` and `steam-shop_apng`. Runs under a
competing load were discarded: small frames then reach their allowances, even
at 4,000 seconds, and are retried, which changes instruction counts too.

A sampled profile of the 62 APNG inputs at `58ccf01` (179,438 CPU samples) put
57.7% under the header fit that `ApngDefault` runs after the tree floors
(R2–R5), 41.6% in R4's joint tree and RLE search, and 35.4% in that search's
out-of-line edge closure. An instrumented build logged 4,169 searches charging
32.5 billion work units: none exhausted its budget, repeated searches were 0.3%
of the work, and each live state charged about 90 units, most of them for code
18 zero runs of 11–138 positions.

- **Specialized R4 edges.** Each live state's edges are charged at once,
  capped by what is left of the budget, so a budget that runs out still cuts
  the same edge. Explicit lengths and codes 16, 17 and 18 are relaxed in their
  own loops, where legality and the capacity arithmetic reduce to a few
  comparisons, instead of through one out-of-line closure per edge, and the
  literal/length and distance payload bounds share one table.
- **Merged R4 relaxations.** An explicit length or zero run adds the same cost
  from every previous length at a position and capacity, and among that
  position and capacity's edges only the code 16 repeats of length 0, which
  come first, reach the same states. While no edge from the position can reach
  the end, so the target holds, and the budget covers every state's edges, the
  search relaxes them once, from the first cheapest state, and still charges
  every state's edges.
- **Reached capacities.** A bitset per position marks the capacities some edge
  has reached, and the search skips the others; the budget and stop are still
  checked at every 32nd capacity. The payload bounds add with wrapping
  arithmetic, which cannot wrap there, so their loop vectorizes.
- **Kept R5 spellings.** The proven-submatch solver reads only the banned
  length symbols up to a match's own, so R5 keeps each match's spelling under
  the parent's code lengths by that set, and the other bans holding the symbol
  and the crossed bans reuse it. Each rewrite charges its budget as before.

| Measure | Before | After | Change |
| --- | ---: | ---: | ---: |
| 62 APNG inputs, instructions retired over two rounds | 6,222.9 billion | 3,742.8 billion | −39.85% |
| Same inputs, cycles over two rounds | 1,591.2 billion | 1,117.3 billion | −29.78% |
| Same inputs, CPU over two rounds | 727.1 s | 492.7 s | 1.48× |
| Same inputs, wall time over two rounds | 128.5 s | 85.7 s | 1.50× |
| Same at 4,000 seconds, instructions retired | 6,197.9 billion | 3,742.4 billion | −39.62% |
| Same at 4,000 seconds, cycles | 1,582.6 billion | 1,112.6 billion | −29.70% |
| `4162040_4062578…` (APNG), Default CPU | 37.27 s | 9.68 s | 3.85× |
| `794800_141419…` (APNG), Default CPU | 54.41 s | 32.62 s | 1.67× |
| 33 Default inputs, instructions retired over two rounds | 1,015.6 billion | 987.9 billion | −2.72% |
| Same inputs, cycles over two rounds | 247.5 billion | 242.8 billion | −1.89% |
| `036-Clefable-0.png`, Max completing its search, instructions retired | 18.30 billion | 17.87 billion | −2.3% |
| `114-Tangela-1.png`, Max completing its search, instructions retired | 2.24 billion | 2.21 billion | −1.4% |
| 4,168 replayed R4 searches, instructions retired | 1,628.1 billion | 285.7 billion | −82.5% |
| Same searches, cycles | 240.0 billion | 49.9 billion | 4.81× |

In the replay, each step against the step before, the specialized edges took
54.9% of the instructions (47.5% of the cycles), merged zero runs 30.5%
(27.0%), merged explicit lengths 31.2% (32.6%), and the bitset with the
vectorized bounds 18.5% (19.4%). Over the APNG inputs at 4,000 seconds the R4
changes took 37.49% of the instructions and the kept R5 spellings 3.40% more.
The gain follows R4's share of a file: `4162040_4062578…` ran 3.85× faster,
while `1328670_115967…` and `976310_99309…` barely changed.

Median APNG peak memory fell 6.9 MiB at 180 seconds and 7.5 MiB at 4,000
seconds; each R4 search holds up to 24 MiB of tables, now for less time. The
largest rise was 38.9 MiB in one run of
`Biker.png`, which fell by 45 MiB in another; at 4,000 seconds the largest rise
was 10.8 MiB.

Equivalence: a new test compares R4 with the previous per-edge search on 2,000
random problems, including budgets that run out mid-search, targets at and
above the optimum and callback stops, matching solutions, budget left and stop
checks; the replayed searches matched at every step. Another new test compares
R5 rewrites with and without kept spellings, budgets included, for every ban
and crossed ban of 12 generated blocks. A release build with debug assertions
agreed with the final build on 61 APNG inputs at 4,000 seconds and with a
debug-assertion baseline on the last. Every 4,000-second APNG run matched byte
for byte. At 180 seconds 123 of 124 did: `animation_icos4d.ref.png` gives its
smallest frame about 9 ms, and the baseline's second run, 0.54 billion
instructions short of its first, wrote another file. API comparisons on the 52
inputs, in Default with the 180-second budget and in zero-budget Max, matched
in all 104 runs, and every Default and completing Max run matched byte for
byte. Rust tests, including the private corpus, passed in release and debug
modes, with documentation and Python tests, a locked build, formatting and
all-feature Clippy with warnings denied. `__text` grew from 890,256 to 896,188
bytes; the shipped executable grew from 974,944 to 974,960 bytes, and the final
source builds the tested executable byte for byte.

Measured and rejected in the same review:

- One queue of Default APNG jobs, largest first, in place of contiguous lanes:
  with the faster R4, lanes already kept 6.0 of 8 busy on average and the queue
  6.3, for 1.8% less wall time at 4,000 seconds, but running the largest frames
  at once raised peak memory by up to about 150 MiB (`794800_141419…` from
  about 260 to 410 MiB).

Not changed in the same review:

- The header planner shared with all Default work is now half of APNG Default
  samples (51% under `best_dynamic_plan_cached`); each frame's initial planner
  is 50%.
- R4's code 16 repeats depend on the previous length, so they stay per state
  (1.35 billion edges over the inputs).
- R5's proposals run the full planner (9% of samples), and the header
  estimates of R5, R13 and R15 build two trees each (6%).

Artifacts: `work/efficiency-review-apng-20261103/`.

### `851e373`, 3 November 2026

Batches: retired instructions and cycles from `time -l` over the 33 stratified
Default inputs, two alternating rounds, for every step and for the final
build; whole-file Default CPU (user plus system) and peak resident memory over
the same inputs, one round alternating the builds; two alternating runs of
each completing Max run.

A sampled profile of the 33 Default inputs at `851e373` (34,320 CPU samples)
put 8.2% under the validating parser, with 3.3% of all samples in its
out-of-line symbol decoder, and found three passes over whole token lists that
could take less work: counting a candidate's symbols (2.8%), spelling an
expansion's tokens (2.6%) and choosing which matches to expand (2.4%). A
second profile after the parser changes put the short-family floor at 4.0%:
for each of its five bands it spelled the expanded tokens, counted them and
summed their extra bits, then threw the tokens away unless the band won.

- **Inlined payload decoding.** The parser's payload loop decodes literal,
  length and distance codes through an inlined copy of the decoder's table
  path, so a symbol no longer passes through a returned result and a separate
  call; codes the table cannot answer still take the out-of-line path. It drops
  the bits of a 32-bit window `peek_word` buffered without checking them
  again: a code takes at most 15 bits and its extra bits 13. The loop's symbol
  counts use wrapping additions, since the model limit keeps them far below
  `u32::MAX`, and one scratch buffer serves every match.
- **Two-table symbol counts.** `count_frequencies` counts alternate tokens in
  two tables with wrapping additions and sums them at the end, so repeats of a
  symbol in a row no longer wait on each other, and the release profile's
  overflow checks stay out of the loop.
- **Bulk expansion spelling.** An expansion copies each run of tokens between
  expanded matches in one block copy, needing only its decoded length, instead
  of pushing its tokens one at a time.
- **Short-family bands priced from counts.** One walk finds every band's
  count changes (the summary the planner's grouping already uses), so the
  floor prices each band from the block's counts and extra bits with those
  changes applied, skips the bands that would expand nothing or exceed the
  candidate limit as spelling them would, and spells the tokens only for a
  band that wins.
- **Byte-cost tables for match expansion.** The walks that choose which
  matches to expand under given code lengths price literals from a 256-entry
  table built once per walk, and skip pricing a match's bytes when even the
  cheapest byte's cost puts its literals above the match's cost.

| Measure | Before | After | Change |
| --- | ---: | ---: | ---: |
| 33 Default inputs, instructions retired over two rounds | 1,101.6 billion | 1,015.2 billion | −7.84% |
| Same inputs, cycles over two rounds | 261.2 billion | 244.9 billion | −6.27% |
| Same inputs, CPU over one round | 42.1 s | 39.4 s | 1.069× |
| `Partnership_Card___John_Lewis_Finance.png`, Default CPU | 9.12 s | 8.46 s | 1.08× |
| `036-Clefable-0.png`, Max completing its search, instructions retired | 18.36 billion | 18.29 billion | −0.4% |
| `114-Tangela-1.png`, Max completing its search, instructions retired | 2.34 billion | 2.24 billion | −4.2% |

Measured step by step, each against the step before, the inlined decoding
with its wrapping counts took 2.55% of the instructions, bulk spelling 2.63%,
short-family pricing from counts 1.66% (2.15% of the cycles), the byte-cost
tables 0.63%, the two-table counts 0.28%, and the unchecked bit drop and shared
scratch buffer about 0.2% together.

Median Default peak memory did not change, and every input but one stayed
within 1.2 MiB. `Banana512.png` peaked at 153.0 MiB instead of 130.9 MiB, in
each of six repeated runs, from the short-family change. The difference is the
allocator's cache of freed large blocks, not data the program holds: with
`MallocLargeCache=0` both builds peak near 90 MiB, the new one 0.7 MiB lower,
and a build that still spells and drops every band's tokens peaks at 129.6 MiB.
Those allocations had reused or evicted cached blocks; without them, more freed
blocks stay resident.

Equivalence: in debug builds the short-family floor checks that the block's
counts are its tokens' counts and that each band's counts and extra bits equal
those of its spelled tokens, and a release build with debug assertions agreed
with the final build on all 33 Default inputs. A new test compares the
two-table counts with a single pass on 4,000 token lists; existing tests
compare expansion spelling with a per-token reference on 20,000 cases and the
byte-cost pricing with full sums on 20,000 more. API comparisons on the 52
inputs, in Default with the 180-second budget and in zero-budget Max, matched
in all 104 runs, and every Default timing and instruction run and every
completing Max run matched byte for byte. Rust tests, including the private
corpus, passed in release and debug modes, with documentation and Python
tests, a locked build, formatting and all-feature Clippy with warnings denied.
`__text` grew from 886,004 to 890,256 bytes; the shipped executable keeps its
974,944 bytes, and the final source builds the tested executable byte for
byte.

Measured and rejected in the same review:

- A fast path for the parser's per-literal limit check: 0.12% more
  instructions.
- Inlining the bit writer's `write` into block payload writing: 0.05% fewer
  instructions and 0.22% fewer cycles, within run-to-run noise, for more code.

Not changed in the same review:

- Ranges cut from a composite copy their tokens (1.6% of samples at the end of
  the review); sharing the composite's storage needs a slice-view token model.
- Transformed blocks count their tokens' symbols (1.0%); deriving the counts
  from an expansion's changes would need each of six callers to carry them.

Artifacts: `work/efficiency-review-default8-20261103/`.

### `4ebfefd`, 3 November 2026

Batches: one paired run of each build, alternating order, on the three small
PNGs whose Max search completes within 1,200 seconds; two rotating runs of a
new replay of 14,440 large-block exhaustive dynamic-plan searches, every
seventh captured from five Max runs at 20 seconds; two alternating runs of each
completing Max run; one paired run of each build on the 177 sprites whose Max
search completes within 10 seconds and on every sixteenth DeflOpt PNG by name
(58 files, Max at 10 seconds); retired instructions and cycles over the 33
stratified Default inputs, two alternating rounds, and their CPU and peak
memory, one round.

A sampled profile of ten inputs in Max at 20 seconds (235,849 CPU samples)
had the shape of `3860542`'s. `exact_tree_candidate`, which prices each
dynamic-plan search's reduced-depth pair (ECT's 10- and 9-bit trees) and the
smoothed-tree candidates' depth limits, took 5.7%: 2.9% building exact Defluff
trees and 2.7% planning their headers. Each exact
Defluff tree built a generic fallback before package-merge, 1.5% of all
samples, although package-merge reads only the counts and the fallback is kept
only if package-merge fails, and each depth limit built the same unconstrained
tree again.

- **The package-merge fallback only when needed.** Package-merge now reports
  whether it replaced the lengths, and the exact Defluff builder makes its
  generic fallback only when it did not.
- **One unconstrained Defluff tree for every limit.** The tree before
  limiting does not depend on the limit, so the reduced-depth pair, the
  bounded-depth floor and the smoothed-tree candidates build it once per
  alphabet and derive each limit's lengths from it.
- **The limited Columbo tree from the variant-0 build.** It starts from the
  generic variant-0 tree and differs only when that needs repair, so Max's tree
  families take an unrepaired variant-0 tree for it.
- **Swap-trial costs in 2^19 slots.** On a traced run of `star.png`, sets of
  four in 2^19 slots kept 93% of the repeats an unbounded memo would, against
  79% in 2^18; the slots still grow with use, so only long searches reach
  6 MiB.

| Measure | Before | After | Change |
| --- | ---: | ---: | ---: |
| `crossroads.png`, Max completing its search, instructions retired | 2,589.7 billion | 2,418.9 billion | −6.6% |
| Same run, user CPU | 166.3 s | 155.9 s | 1.07× |
| `star.png`, Max completing its search, instructions retired | 1,162.2 billion | 1,123.0 billion | −3.4% |
| `basi4a16.png`, Max completing its search, instructions retired | 878.0 billion | 837.8 billion | −4.6% |
| 14,440 replayed large-block dynamic-plan searches, instructions retired | 231.5 billion | 224.3 billion | −3.1% |
| Same replay, cycles | 55.8 billion | 54.1 billion | −3.1% |
| 177 sprites, Max completing its search, instructions retired | 3,007.8 billion | 2,939.3 billion | −2.3% |
| Same runs, user CPU | 233.1 s | 227.7 s | 1.02× |
| `036-Clefable-0.png`, Max completing its search, instructions retired | 18.70 billion | 18.37 billion | −1.7% |
| `114-Tangela-1.png`, Max completing its search, instructions retired | 2.46 billion | 2.34 billion | −4.7% |
| 33 Default inputs, instructions retired over two rounds | 1,110.8 billion | 1,101.7 billion | −0.82% |
| Same inputs, cycles over two rounds | 265.5 billion | 262.6 billion | −1.08% |

The larger swap memo alone took 6.1%, 2.9% and 2.3% of the instructions off
the three small PNGs; the tree changes account for the replay, which plans no
swaps, and Default gains through the bounded-depth floor and the exact trees
of its feedback seeds. Default peak memory rose 0.9 MiB at most, and the
median did not change.

Deadline-bound Max cannot show equivalence, so it was checked for losses: on
every sixteenth DeflOpt PNG by name, 56 outputs were identical, in 449.9
instead of 450.1 seconds. `sample_20.png` gave 9,695 bytes instead of 9,692,
the baseline's own endpoint at 11 seconds and later, as in `3860542`'s review.
`interlaced_rgba_8_should_be_grayscale_alpha_8.png` was 1 byte larger once;
in three more runs at 10 seconds both builds gave 132,248 bytes, and 132,247
at 11, 12 and 14 seconds.

Equivalence: a new test compares the exact Defluff builder with the order it
replaced, the shared tree's limited lengths with the builder, and the limited
Columbo tree with an unrepaired variant-0 tree, on 4,000 alphabets of 19, 30
and 286 symbols at limits of 7, 9, 10 and 15 bits, more than 1,000 of them
limited. The replays matched plan for plan. API comparisons on the 52 inputs,
in Default with the 180-second budget and in zero-budget Max, matched in all
104 runs, and every Default timing run and every completing Max run matched
byte for byte. Rust tests, including the private corpus, passed in release and
debug modes, with documentation and Python tests, a locked build, formatting
and all-feature Clippy with warnings denied. `__text` grew from 884,112 to
886,004 bytes, and the shipped executable by one 16 KiB page, to 974,944 bytes.

Measured and rejected in the same review:

- Spelling the shortest RLE's tokens only when a pass adopts it or compares
  it: of 14.5 million computed in 20 seconds on `crossroads.png`, 8.5 million
  were adopted and 7.7 million had the counts of the pass's RLE, so their
  tokens were compared; nearly all needed their tokens.
- Ending a seed at a pass that starts from an RLE an earlier seed started from
  at the same or an earlier pass: 1.4 million such passes on `crossroads.png`,
  but the work after them was 356 milliseconds of 15.7 seconds.

Not changed in the same review:

- `sample` groups a function's addresses even without line tables, and
  `xctrace` and an unprivileged `dtrace` were not available, so the header
  planner's inlined self time (11.4%) could not be split by instruction.

Artifacts: `work/efficiency-review-max8-20261103/`.

### `a540aac`, 3 November 2026

Batches: retired instructions and cycles from `time -l` over the 33 stratified
Default inputs, two alternating rounds; whole-file Default CPU (user plus
system) and peak resident memory over the same inputs, two rounds alternating
the builds; two alternating runs of each completing Max run.

A sampled profile of the 33 Default inputs at `a540aac` (36,469 CPU samples) put
38% under uncached header planning, half of it building code-length trees, and
9.5% under the validating parser. Counters showed the header-plan store
already catching repeats: 0.4% of 17.4 seconds of uncached header planning
repeated a plan the thread had made. The parser did repeat work: of 4.0
seconds spent parsing 1,025 candidates, 16% parsed bytes the same thread had
just parsed and validated. The bounded-depth tree floor and the terminal parse
cache reparse a candidate that `build_candidate_and_parse` validated and
dropped, and Default's endpoint-proven lineage reparses the candidate whose
validated parse that function had just returned.

- **Kept validated parse.** Each thread keeps its last validated parse with
  its bytes and decoded limit. A parse is a function of the two, so a hit,
  which compares both in full, returns a copy sharing the kept blocks' token
  and decoded-byte buffers, and still checks the caller's identity. The entry
  is dropped before a different stream is parsed, so a stale model is never
  held while a new one is built, and is released when each stream ends. On
  the call sequence, one entry caught 509 of the 572 milliseconds that four
  would.
- **The endpoint-proven lineage takes its candidate's parse.**
  `build_candidate_with_proven_policy` keeps the validated parse that built
  its candidate instead of parsing the same bytes again.
- **A derived bit position.** The bit reader's position is the bits of the
  bytes it has loaded less those still buffered, so it no longer keeps a
  separate count with a checked addition at every decoded symbol. A slice
  holds at most `isize::MAX` bytes, so the derived count cannot overflow.

| Measure | Before | After | Change |
| --- | ---: | ---: | ---: |
| 33 Default inputs, instructions retired over two rounds | 1,129.3 billion | 1,110.3 billion | −1.69% |
| Same inputs, cycles over two rounds | 267.6 billion | 264.0 billion | −1.32% |
| Same inputs, CPU over two rounds | 85.5 s | 84.3 s | 1.015× |
| `Partnership_Card___John_Lewis_Finance.png`, Default CPU | 9.53 s | 9.24–9.27 s | 1.03× |
| `036-Clefable-0.png`, Max completing its search, instructions retired | 18.72 billion | 18.71 billion | −0.0% |
| `114-Tangela-1.png`, Max completing its search, instructions retired | 2.46 billion | 2.46 billion | −0.2% |

The kept parse and the endpoint-proven parse took 1.48% of the instructions
and the derived bit position another 0.22%. Median Default peak memory rose
0.2 MiB and at most 2.7 MiB (`y2rc2_large.png`, 68.8 to 71.5 MiB); the
Partnership card peaked 1.4–3.9 MiB lower, near 963 MiB.

Equivalence: a new test checks that a kept parse matches a fresh one block by
block and shares its buffers, that a hit is checked against the caller's
identity, and that another decoded limit fails as a fresh parse does; the bit
reader's reference tests cover the derived position across reads and aligned
reads. API comparisons on the 52 inputs, in Default with the 180-second budget
and in zero-budget Max, matched in all 104 runs, and every Default timing run
and every completing Max run matched byte for byte. Rust tests, including the
private corpus, passed in release and debug modes, with documentation and
Python tests, a locked build, formatting and all-feature Clippy with warnings
denied. `__text` grew from 883,932 to 884,112 bytes; the shipped executable
keeps its 958,528 bytes, and the final source builds the tested executable
byte for byte.

Measured and rejected in the same review:

- Validating without the model where a caller drops the parse: the parse is
  mostly decoding, so a check without tokens or counts cost 84% of a full
  parse, and 270 of the 399 final validations belong to terminal methods whose
  winners keep their parse. No instructions changed, cycles fell 0.18%, and
  the code grew by an executable page.
- Bounds to skip the payload-tree pairs of a block plan: 56.5% of the pairs
  that lose would need a header bound above 95% of their actual header.
- A cache of `tree_candidates` by counts: repeats took 5% of its 4.9 seconds.

Not changed in the same review:

- Code-length trees: of 16.5 million builds, 75% were another builder's tree
  for a histogram the thread had built before, and 42–96% of each builder's
  trees equal one already built for the histogram, where tie decisions differ
  but reconverge. No build was repeated for want of cache room (6%), and the
  heaps' tie tracking already shares the trees it can prove equal; histograms
  without ties, where every builder must agree, were 2% of builds.

Artifacts: `work/efficiency-review-default7-20261103/`.

### `3860542`, 2 November 2026

Batches: one paired run of each build, alternating order, on three small PNGs
whose Max search reaches a 20-second deadline but completes within 1,200
seconds, where two earlier candidates' paired runs agreed with it; two
alternating runs of each completing Max run; one paired run of each build
on the 177 sprites whose Max search completes within 10 seconds, alternating
order; one paired run of each build on every sixteenth DeflOpt PNG by name (58
files, Max at 10 seconds, alternating order); retired instructions and cycles
from `time -l` over the 33 stratified Default inputs, two alternating rounds;
two rotating runs of two new replays. The header-plan replay holds 275,887
uncached header plans sampled from seven Max runs at 20 seconds; the
swap-search replay holds the inputs of 11,343 greedy length-swap searches from
four Max runs, replayed thread by thread in their order.

A sampled profile of ten inputs in Max at 20 seconds at `3860542` (237,481 CPU
samples) put half of all samples under the ordered state queue, 95% of them
planning its children, and 17.7% under the greedy length swaps, which plan a
header for every swap of a pass's start lengths: on `crossroads.png`, 79% of
the dynamic-plan search. Counters on the per-thread header-plan store found
uncached plans the same thread had already made taking 24% of uncached
planning time on `crossroads.png`, 6% on `france.png` and `OwlAlpha-0.5.png`
and 2% on `Banana512.png`. On the small inputs nearly all were swap trials, most
of them 32,000 to a million misses apart, beyond the store's reach. Whole swap
searches never repeated: the queue's children keep their parent's code lengths
under slightly different counts, so passes start from the same lengths, but the
counts change which pairs are tried and what each costs in payload. Keyed by
start lengths and swapped pair, 35% of swap-trial time on `crossroads.png` and
`star.png` repeated an earlier trial on the same thread, and 27% on
`basi4a16.png`, most of them 32,000 to 256,000 trials earlier.

- **Swap-trial costs.** A trial's header depends only on the pass's start
  lengths, the swapped pair and the `exhaustive` flag, so each thread keeps
  the trials' header bits without their plans, 12 bytes a trial. A pass finds
  its start lengths by fingerprint, compares them in full and gets an
  identifier that is never reused, so a kept trial can answer only for the
  lengths it was planned from. A key hashes to a set of four slots kept in
  order of use. The slots start at 4,096 and grow fourfold, keeping their
  trials, each time they have taken as many new trials as they hold, up to
  2^18 slots, 3 MiB. The pass compares kept and new costs as before, and plans
  its chosen swap again when that swap's cost was kept, once per pass. Default
  never runs the swaps.
- **One-pass payload shape check.** The check that every uncached header plan
  makes on both payload alphabets counts the codes in the same branch-free pass
  as the Kraft sum. A single code is valid when it fills half the code space,
  which is when it is one bit long.
- **Shortest-RLE comparison.** A feedback pass stops when the shortest RLE is
  the RLE it already has, or a seed started from it earlier. It now tests the
  pass index first, then the symbol counts, which equal RLEs share, before
  comparing tokens.

| Measure | Before | After | Change |
| --- | ---: | ---: | ---: |
| `crossroads.png`, Max completing its search, instructions retired | 3,121.2 billion | 2,584.7 billion | −17.2% |
| Same run, user CPU | 198.8 s | 164.3 s | 1.21× |
| `star.png`, Max completing its search, instructions retired | 1,401.4 billion | 1,162.9 billion | −17.0% |
| Same run, user CPU | 86.5 s | 72.9 s | 1.19× |
| `basi4a16.png`, Max completing its search, instructions retired | 944.9 billion | 871.9 billion | −7.7% |
| Same run, user CPU | 63.0 s | 58.8 s | 1.07× |
| 11,343 replayed swap searches, instructions retired | 722.6 billion | 645.4 billion | −10.7% |
| 275,887 replayed header plans, instructions retired | 68.25 billion | 67.27 billion | −1.4% |
| 177 sprites, Max completing its search, instructions retired | 3,095.0 billion | 2,972.0 billion | −4.0% |
| Same runs, user CPU | 239.6 s | 232.4 s | 1.03× |
| `036-Clefable-0.png`, Max completing its search, instructions retired | 19.09 billion | 18.31 billion | −4.1% |
| `114-Tangela-1.png`, Max completing its search, instructions retired | 2.37 billion | 2.30 billion | −2.9% |
| 33 Default inputs, instructions retired over two rounds | 1,139.6 billion | 1,129.7 billion | −0.86% |
| Same inputs, cycles over two rounds | 268.9 billion | 267.5 billion | −0.52% |

The swap-search replay underestimates the memo, since its store sees no other
planning between searches: with the trials mapped directly to 2^18 slots, it
fell 6.1% while the completing runs fell 13.1%, 11.8% and 4.9%. The shape check
and the comparison account for the header-plan replay's 1.4% and the Default
inputs' 0.86%. On traced swap trials, sets of four kept as many repeats as
twice the slots mapped directly: 79% of an unbounded memo's on `star.png`,
against 64% for 2^18 direct slots and 78% for 2^19. A thread's slots first
filled their full 3 MiB on every swap search, which raised peak memory on
`036-Clefable-0.png` from 27 to 37 MiB; growing them with use took that to
30–32 MiB. Over the 177 sprites, median peak memory rose from 24 to 26 MiB and
at most by 8 MiB (`108-Lickitung-0.png`, 63 to 71 MiB). The three long runs
peaked at 402, 445 and 372 MiB, against 373, 457 and 389.

Deadline-bound Max cannot show equivalence, so it was checked for losses: on
every sixteenth DeflOpt PNG by name (58 files, Max at 10 seconds, alternating
builds), 57 outputs were identical, in 447.9 instead of 448.9 seconds.
`sample_20.png` came out 9,695 instead of 9,692 bytes in three runs of each
build. The baseline also gives 9,695 bytes at 11, 12, 14 and 30 seconds, so the
faster build reaches the baseline's later endpoint sooner; both end at 9,700
bytes when the search completes, in 99 seconds.

Equivalence: new tests run swap searches under six sets of counts twice each
with trials kept, then each again from empty costs, and compare the plans; grow
the trial slots through every size and find the most recent trials with their
costs; check that start lengths are compared in full and never get a
reused identifier; and compare the shape check with the counting reference on
20,000 alphabets, complete, single-code, empty and invalid. The replays matched
plan for plan and search for search. API comparisons on the 52 inputs, in
Default with the 180-second budget and in zero-budget Max, matched in all 104
runs, and every Default timing run and every completing Max run matched byte
for byte. Rust tests, including the private corpus, passed in release and debug
modes, with documentation and Python tests, a locked build, formatting,
all-feature Clippy with warnings denied and a package list without private
fixtures or scratch artifacts. `__text` grew from 877,060 to 883,344 bytes; the shipped
executable stays at 958,512 bytes apart from a 16-byte longer signature, and
the final source builds the tested executable byte for byte.

Measured and rejected in the same review:

- A lower bound to skip planning the queue's children, as in `e25d20d`: only
  7.2% of children could not join the beam or replace the incumbent, 3.0% of
  their planning time, and an optimal-code payload bound with the least header
  skipped 0.4% of it.
- A bound to skip swap trials: 95% of trials on `crossroads.png` ended 5 to 16
  bits worse than the pass's state, too close for any cheap header bound. A
  swap of two lengths that each stand alone between other values keeps every
  RLE's cost, and in 45,000 such trials the planned header never changed, but
  they were 1% of trials on the heavy inputs.
- Stopping the DeflOpt and deft4j RLE rewrites after their last repeat code:
  the per-token count cost more than the tail it skipped, 2.4% more
  instructions on the header-plan replay.

Not changed in the same review:

- Header plans another thread had already made took 0.1% to 3.8% of uncached
  planning time; sharing the store would need a lock on every lookup.
- With line tables in the binary, `sample` groups a function's addresses by
  source line and lists only a few, so per-line attribution of inlined code
  was not reliable; the changes above were chosen and kept by replays and
  completing runs.

Artifacts: `work/efficiency-review-max7-20261102/`.

### `6e6a0c4`, 28 October 2026

Batches: one paired run of each build on the 177 sprites whose Max search
completes within 10 seconds, alternating order; two alternating runs of each
completing Max run; retired instructions and cycles from `time -l` over the 33
stratified Default inputs, two alternating rounds; whole-file Default CPU (user
plus system) and peak resident memory over the same inputs, two rounds
alternating the builds; one paired run of each build on every sixteenth
DeflOpt PNG by name (58 files, Max at 10 seconds, alternating order); three
rotating runs of the header-plan replay.

A sampled profile of ten inputs in Max at 20 seconds at `6e6a0c4` (226,561 CPU
samples) put the shortest-RLE search first (10.0% of self time), then the heap
builder's tracked merge (4.9%), the greedy RLE seeds (4.8%) and the DeflOpt
local rewrite (4.6%). Counters in test builds showed that seeds and shortest
RLEs were not repeated: every seed spelling was distinct, and 354,163 of the
366,901 shortest RLEs computed in the replay were adopted. Counters on the
per-thread header-plan store were another matter. In Max runs of 20
seconds, 72–93% of its lookups missed, and 6–44% of uncached header-planning
time went to plans the same thread had already made: 10.7 of 24.6 seconds on
`crossroads.png`, 5.8 of 30.4 on `OwlAlpha-0.5.png`. Most were 16,000 to 64,000
misses after the first, while the store, at 400–790 charged bytes a plan,
held 8,000 to 16,000. An unbounded store cut the repeated time on
`crossroads.png` from 10.7 to 1.3 seconds but took peak memory to 1.17 GB.

- **Compact header-plan store.** Each generation of the store now appends its
  plans to one byte arena, found through an index of offsets, instead of a
  packed allocation per plan in a table of plan records. An entry is 22 bytes
  of fields and one byte per RLE token: lengths 0–15 as themselves, then codes
  16, 17 and 18 from 16, 20 and 28 plus their extra values. The code lengths,
  half of the old entry, are not stored, since the RLE spells them: a hit
  decodes the RLE and compares the lengths it spells, and the policy, with the
  lookup's. The arena grows in steps bounded by half the budget, so the charge
  is its used bytes and the index's slots, with no per-entry allocator
  overhead. The budget is now 16 MiB.

| Store | 24 heaviest completing sprites, instructions | Default cycles | Default peak memory, largest rise |
| --- | ---: | ---: | ---: |
| Compact, 8 MiB | −4.7% | −1.2% | not measured |
| Compact, 16 MiB | −7.1% | −2.4% | +23.1 MiB |
| Compact, 32 MiB | −9.0% | −2.5% | +50.6 MiB |

Each row is measured against `6e6a0c4`. A compact entry took 155 charged bytes
on `crossroads.png` and 296 on `OwlAlpha-0.5.png`, against 528 and 788 before,
so a 16 MiB generation held 54,000 and 28,000 plans.

| Measure | Before | After | Change |
| --- | ---: | ---: | ---: |
| 177 sprites, Max completing its search, instructions retired | 3,237.2 billion | 3,043.7 billion | −6.0% |
| Same runs, user CPU | 251.8 s | 237.0 s | 1.06× |
| `036-Clefable-0.png`, Max completing its search, instructions retired | 19.45–19.46 billion | 18.57 billion | −4.6% |
| Same run, user CPU | 1.52–1.53 s | 1.45 s | 1.05× |
| `114-Tangela-1.png`, Max completing its search, instructions retired | 2.41 billion | 2.37 billion | −1.6% |
| 33 Default inputs, instructions retired over two rounds | 1,168.5 billion | 1,140.3 billion | −2.41% |
| Same inputs, cycles over two rounds | 282.7 billion | 273.9 billion | −3.10% |
| Same inputs, CPU over two rounds | 89.8 s | 87.0 s | 1.03× |

Median Default peak memory fell 1.1 MiB. `y2rc2_large.png`, 1.19× faster,
peaked 20–22 MiB higher, near 70 MiB, `ss1.png` 11 MiB and the largest input
11–14 MiB, near 960 MiB. In Max, the 177 sprites' median peak fell 4 MiB;
`112-Rhydon-0.png`, the longest at 12 seconds and 1.14× faster, rose from 65 to
104 MiB, while `036-Clefable-0.png` fell from about 41 to 27 MiB.

Deadline-bound Max cannot show equivalence, so it was checked for losses: on
every sixteenth DeflOpt PNG by name (58 files, Max at 10 seconds, alternating
builds), 57 outputs were identical and `monarch.png`, which reached the
deadline, came out the same size with different bytes, in 436.7 instead of
438.9 seconds.

Equivalence: a new test round-trips every RLE token through its byte and checks
that every other symbol and extra value is refused; the store's turnover and
collision tests run against the compact store, and the per-thread store test
compares every stored plan with uncached planning. Header plans take no stop,
so a stored plan is complete, and every hit compares the complete code lengths
and policy. The 89,255 replayed header plans, which bypass the store, matched
plan for plan. API comparisons on the 52 inputs, in Default with the
180-second budget and in zero-budget Max, matched in all 104 runs, and every
Default timing run and every completing Max run matched byte for byte.
Rust tests, including the private corpus, passed in release and debug modes,
with documentation and Python tests, a locked build, formatting, all-feature
Clippy with warnings denied and a package list without private fixtures or
scratch artifacts. `__text` grew from 872,548 to 875,640 bytes; the shipped
executable keeps its 958,512 bytes, and the final source builds the tested
executable byte for byte.

Measured and not kept in the same review:

- A 32 MiB store: on the 24 heaviest completing sprites, 2.1% fewer
  instructions than 16 MiB, but Default peak memory up to 50.6 MiB higher
  instead of 23.1.
- No route-local header-plan caches, leaving the per-thread store to find every
  repeat: 0.8% fewer instructions on the 24 sprites and 0.4% fewer in Default,
  but removing them reaches through five modules' signatures; left for a
  separate change.
- The block-plan store showed few repeats in Max: none on `crossroads.png`,
  65 milliseconds of 39 seconds on `Banana512.png`, and 2.45 of 37 seconds on
  `OwlAlpha-0.5.png`, 4,000 to 8,000 misses after the first.

Artifacts: `work/efficiency-review-max6-20261028/`.

### `4013a14`, 28 October 2026

Batches: three rotating runs of the header-plan replay per comparison; two
alternating runs of each completing Max run; retired instructions and cycles
from `time -l` over the 33 stratified Default inputs, two alternating rounds;
whole-file Default CPU (user plus system) and peak resident memory over the
same inputs, two rounds alternating the builds; one paired run of each build
on every sixteenth DeflOpt PNG by name (58 files, Max at 10 seconds,
alternating order).

A sampled profile of ten inputs in Max at 20 seconds at `4013a14` (231,511 CPU
samples) put 22.7% under the code-length tree cache and 15.2% of self time in
the DeflOpt and order-key heap builder, then the shortest-RLE search (9.1%),
the DeflOpt local rewrite (4.2%) and the greedy seeds (4.1%). Counters in a
test build of the header-plan replay found 2.45 million code-length tree
builds, 1.26 million of them for the local rewrite's DeflOpt variants. Of the
1.35 million heap builds made for a histogram that already had a heap tree,
1.19 million built a tree it already had; only 169,000 builds repeated a tree
the cache had dropped.

- **Tie tracking per sift-down and per variant.** A heap build that tracks
  ties notes the comparisons between equal frequencies another rule would
  decide otherwise, and shared its tree with the other variants only when no
  comparison anywhere turned. It now notes them separately for a merge's two
  sift-downs, the one after removing the first child and the one after
  inserting the parent, which each variant breaks by its own rule (height,
  creation order or frequencies alone), and records where the creation order
  disagrees with the height rather than with frequencies alone. A variant
  whose rule for each sift-down agrees with the build's in every comparison
  noted there takes the same steps and builds the same tree. Any variant can
  now track, not only variant 0, and the code-length tree cache tracks every
  heap build: noting adds instructions to each comparison, but a histogram's
  later variants mostly follow an earlier build's steps.
- **Repaired first trees priced alone.** A feedback pass prices only its first
  code-length code when that is an unrepaired Huffman tree, since no code can
  then price lower. In the exhaustive search it now does the same for a
  repaired first tree that spells the RLE in as few bits as exact Defluff's
  package-merge code, the cheapest within 7 bits, which that search builds
  anyway: the heap tree codes only the used symbols, so no code sends fewer
  code-length codes either.
- **Seeds stop at a shortest RLE already followed.** A feedback pass depends
  only on its RLE, and pricing a header again cannot change the best plan. Each
  kept shortest RLE records the earliest pass a seed of the plan started from
  it, and a seed that would start the same or a later pass from it stops.
- **Kept buffers and fixed sizes for code-length codes.** Exact Defluff's
  builder takes the kept tree-building scratch instead of allocating its own;
  the code-length completeness check works on 19 lengths instead of the general
  chunked check; and the code-length cache's heap builds run on stack arrays of
  the alphabet's size, through the heap builder's steps inlined into both entry
  points.
- **Per-variant skips in tree lists.** The payload tree lists and the
  short-family floors' variant loops skip each variant a variant-0 build shows
  to build a tree already listed or both trees already priced, instead of
  stopping only when every variant agrees.

| Step | Replay instructions | Replay cycles |
| --- | ---: | ---: |
| Tie turns noted per sift-down | −1.5% | −2.0% |
| Plus repaired first trees, and creation order against the height | −7.0% | −7.4% |
| Plus seeds stopping at a shortest RLE already followed | −7.8% | −8.6% |
| Plus exact Defluff's kept scratch | −8.6% | −9.4% |
| Plus the 19-length completeness check | −9.9% | −10.5% |
| Plus every variant tracking, every cache build tracked | +2.0% | −1.5% |
| Plus code-length heaps on stack arrays | −1.7% | within noise |

The first five rows are measured against `4013a14` in the same rotating run,
the last two against the build before them. Tracking every build retires more
instructions but takes fewer cycles: the builds it skips are the
branch-mispredicting merges.

| Measure | Before | After | Change |
| --- | ---: | ---: | ---: |
| 89,255 header plans captured from ten Max runs, replayed (median) | 1.925 s | 1.661 s | 1.16× |
| Same replay, instructions retired | 28.40 billion | 25.74–25.75 billion | −9.4% |
| Same replay, cycles | 7.32–7.33 billion | 6.49–6.56 billion | −11.0% |
| `036-Clefable-0.png`, Max completing its search, instructions retired | 20.18–20.19 billion | 19.45–19.46 billion | −3.6% |
| Same run, user CPU | 1.57–1.58 s | 1.51 s | 1.04× |
| `114-Tangela-1.png`, Max completing its search, instructions retired | 2.42 billion | 2.41 billion | −0.4% |
| 33 Default inputs, instructions retired over two rounds | 1,193.6 billion | 1,167.7 billion | −2.17% |
| Same inputs, cycles over two rounds | 288.2 billion | 275.8 billion | −4.29% |
| Same inputs, CPU over two rounds | 91.9 s | 87.9 s | 1.05× |

Median Default peak memory rose 0.1 MiB. `y2rc2_large.png` peaked 2.6 MiB
higher in both rounds, near 50 MiB, and the largest input,
`Partnership_Card___John_Lewis_Finance.png`, 5.6 MiB higher in one round and
no higher in the other, near 945 MiB.

Deadline-bound Max cannot show equivalence, so it was checked for losses: on
every sixteenth DeflOpt PNG by name (58 files, Max at 10 seconds, alternating
builds), 57 outputs were identical and `Nutcracker.png`, which reached the
deadline, came out 1 byte smaller, in 437.3 instead of 438.2 seconds. The
previous review saw one build give that file both sizes.

Equivalence: new tests compare the stack-array code-length builder with the
shared-buffer builder on 20,000 random histograms, every variant of both heaps,
tracked and untracked, after builds of a larger alphabet; and the 19-length
completeness check with the general one on 20,000 length sets, lengths above
15 included. The existing variant-sharing test now checks every variant a
build reports, tracked builds of all four variants included, on 20,000
histograms of 19, 30 and 286 symbols; the first-tree test now also covers
4,221 repaired first trees at exact Defluff's cost, against every builder, and
checks exact Defluff's kept scratch against a fresh one. The 89,255 replayed
header plans matched plan for plan at every step. API comparisons on the 52
inputs, in Default with the 180-second budget and in zero-budget Max, matched in
all 104 runs, and every Default timing run and both completing Max runs
matched byte for byte. Rust tests, including the private corpus, passed in
release and debug modes, with documentation and Python tests, a locked build,
formatting, all-feature Clippy with warnings denied and a package list without
private fixtures or scratch artifacts. `__text` grew from 866,440 to 872,548
bytes and the shipped executable from 942,064 to 958,512, one 16 KiB page,
mostly the tracking merges for every variant and the stack-array builder; the
final source builds the tested executable byte for byte.

Measured and rejected in the same review:

- Sharing a tree with every variant when no merge's second removal tied the
  next least entry, so every heap rule must merge the same pair: the check on
  every merge cost more than it saved (26.81 against 26.37–26.40 billion
  replay instructions without it).
- deft4j's Java-heap builder on packed entries and branch children, with its
  nodes only for padding and repair: 25.72 against 25.60 billion replay
  instructions; its heap steps dominate, and repaired trees paid for both.
- Tracking every variant in the payload tree lists: 4.4% more Default
  instructions and 2.2% more cycles; on 286 symbols tracking costs more.
- A shortest-RLE search that prices without spelling: 354,163 of the 366,901
  shortest RLEs computed in the replay were adopted.
- Sharing local-rewrite steps across RLE seeds: 32,000 more of 1.97 million
  steps would repeat.

Artifacts: `work/efficiency-review-max5-20261028/`.

### `c2aa4ba`, 28 October 2026

Batches: three alternating runs of the header-plan replay; two alternating
runs of each completing Max run; retired instructions and cycles from
`time -l` over the 33 stratified Default inputs, two alternating rounds;
whole-file Default CPU (user plus system) and peak resident memory over the
same inputs, two rounds alternating the builds; one paired run of each build
on every sixteenth DeflOpt PNG by name (58 files, Max at 10 seconds,
alternating order).

A sampled profile of ten inputs in Max at 20 seconds at `c2aa4ba` (230,775 CPU
samples) put 54% under `consider_rle`, which follows each RLE seed through the
local rewrite and up to four feedback passes. The heap builder took 14.3% of
self time, the shortest-RLE search 9.6%, the DeflOpt local rewrite 5.4% and the
greedy seeds 3.9%; allocation took about 6%, as macOS's `free` reads the clock.
Source recode's expansion marks took 2.1% and the generic builder's heap pops
1.8%.

- **Header-plan buffers kept.** The feedback pass cloned the shortest RLE it
  chose and allocated every DeflOpt and deft4j rewrite, its local-rewrite step
  record, the step states and its candidate code-length trees; the
  shortest-RLE memo allocated an entry per tree and each seed spelling a new
  RLE. The per-thread header-plan search now keeps them all: rewrites write
  into two buffers, one swapped with the pass's RLE when it wins; the chosen
  shortest RLE is copied into the pass's buffer; the step record, its states
  and the candidate list are reused; the memo's entries stay allocated across
  plans and are refilled; and the seed spellers take the RLEs that finished
  seeds hand back.
- **Rewrite skipped at the shortest cost.** In the exhaustive search each
  feedback pass also finds the shortest RLE under the feedback tree, the
  cheapest spelling of those lengths. Every change the DeflOpt local rewrite
  makes is strictly cheaper, so when the pass's RLE already costs what the
  shortest does, and that cost is finite, the rewrite cannot change it and is
  not run. A debug assertion runs it anyway and checks that it finds nothing.
- **Expansion marks stop early.** Source recode marks the matches whose
  literals cost less than the match, or no more. The literal sum now stops once
  it exceeds the match's cost, after which no later byte can mark the match,
  as the block search's sums have done since `71ecf9f`.
- **One-word generic heap entries.** The generic builder's two keyed heaps
  held a 64-bit key and an index. As in the DeflOpt-style heap since
  `a6f67cd`, the frequency now takes the top 32 bits, the order, inverted for
  odd variants, the next 16 and the node the low 16; comparisons use the top
  48 bits. Alphabets are asserted to fit.

| Step | Replay instructions against `c2aa4ba` |
| --- | ---: |
| Shortest RLE copied, not cloned | −1.3% |
| Plus rewrites into kept buffers | −1.8% |
| Plus kept step record and candidate list | −2.6% |
| Plus pooled step states | −2.8% |
| Plus shortest-RLE memo kept across plans | −4.1% |
| Plus recycled seed buffers | −4.7% |
| Plus rewrite skipped at the shortest cost | −5.5% |

Each row is measured against the baseline in the same alternating run. The
replay does not reach source recode or the generic builder's heaps; in the
ten-input profile their shares fell from 2.05% to 1.68% for the marks and from
1.79% to 1.63% for the heap pops.

| Measure | Before | After | Change |
| --- | ---: | ---: | ---: |
| 89,255 header plans captured from ten Max runs, replayed (median) | 2.110 s | 1.924 s | 1.10× |
| Same replay, instructions retired | 30.01–30.07 billion | 28.37–28.39 billion | −5.5% |
| Same replay, cycles | 7.90–7.98 billion | 7.33–7.42 billion | −7.1% |
| `036-Clefable-0.png`, Max completing its search, instructions retired | 21.76–21.81 billion | 20.20–20.25 billion | −7.1% |
| Same run, user CPU | 1.90–2.04 s | 1.64–1.72 s | 1.17× |
| `114-Tangela-1.png`, Max completing its search, instructions retired | 2.45 billion | 2.41–2.42 billion | −1.3% |
| 33 Default inputs, instructions retired over two rounds | 1,214.8 billion | 1,194.9 billion | −1.64% |
| Same inputs, cycles over two rounds | 303.4 billion | 297.8 billion | −1.84% |
| Same inputs, CPU over two rounds | 95.7 s | 93.7 s | 1.02× |

Median Default peak memory fell 0.2 MiB. The largest input,
`Partnership_Card___John_Lewis_Finance.png`, peaked 11.0 MiB higher in one
round and 7.6 MiB lower in the other, around 950 MiB.

Deadline-bound Max cannot show equivalence, so it was checked for losses: on
every sixteenth DeflOpt PNG by name (58 files, Max at 10 seconds, alternating
builds), 54 outputs were identical, in 438.9 instead of 443.5 seconds. Four
runs that reached the deadline differed: two smaller, by 3 bytes and 1 byte,
one the same size and `Nutcracker.png` 1 byte larger. Run twice more, the
baseline gave `Nutcracker.png` in 47,731 and then 47,732 bytes, and the
candidate in 47,731 bytes both times, the second byte for byte as the
baseline's first.

Equivalence: a new test compares the early-stopping marks with the full
expansion delta on 20,000 random tokens, codes and decoded bytes, with cheap
literals and short matches in half of them so that more than 1,000 marks
are set, uncodable literals included; the seed-speller test now hands the
spellers stale buffers; and the existing comparisons of the generic builder's
heap merges with their reference implementations pass with the packed entries.
Debug tests run the skipped rewrite's assertion. The 89,255 replayed header
plans matched plan for plan. API comparisons on the 52 inputs, in Default with
the 180-second budget and in zero-budget Max, matched in all 104 runs, and
every Default timing run and both completing Max runs matched byte for byte.
Rust tests, including the private corpus, passed in release and debug modes,
with documentation and Python tests, a locked build, formatting, all-feature
Clippy with warnings denied and a package list without private fixtures or
scratch artifacts. `__text` grew from 865,300 to 866,440 bytes; the shipped
executable keeps its 942,064 bytes, and the final source builds the tested
executable byte for byte.

Not changed in the same review: the heap builder, still the largest share,
whose variants diverge at about the first merge and whose branch-free and
bottom-up sift-downs earlier reviews measured and rejected.

Artifacts: `work/efficiency-review-max4-20261028/`.

### `46b7c44`, 28 October 2026

Batches: whole-file CPU (user plus system) and peak resident memory over the
33 stratified Default inputs, two rounds alternating the builds; retired
instructions and cycles from `time -l` over the same inputs, two alternating
rounds per comparison; three alternating runs of the header-plan replay; two
alternating runs of each completing Max run.

A sampled Default profile of the 33 inputs at `46b7c44` (42,290 CPU samples),
with the slide's cut search and pricing kept out of line, put 24.8% of self
time in the heap builder and 7.2% in the boundary slide's cut search and
prices; the rest spread over steps of 4% or less. A third of the time is the
Default finish, whose terminal searches plan blocks the earlier sweeps have
already changed.

- **One walk per slide cut search.** The slide (R1c) found its scan limits in
  one walk, summed prefix and suffix prices in a second and scanned the cuts in
  a third, pricing a joining token, its literals included, up to three times.
  A cut's total differs from the first usable cut's by the left minus right
  prices of the tokens between them, so one walk keeps running sums and the
  best cut relative to that first cut, starting again whenever a left token
  the right trees cannot code moves it, and stops comparing at the first right
  token the left trees cannot code. Memory stays constant.
- **One window per decoded value.** Decoding peeked the root code bits, the
  subtable bits if needed, then the code and its extra bits again, each time
  checking and refilling the bit buffer. With 32 bits in the buffer, or eight
  more input bytes to load, the longest code and its extra bits are all
  there, so one window gives the entry and the value; near the end of input
  the decoder takes its former path.
- **Strict and tie-including expansions together.** Each seed of the block
  search's rebuild ladder first expands the block's matches whose literals cost
  less under the seed codes, then, as the ladder's first pass, those whose
  literals cost no more, under the same codes; deft4j's recode does the same.
  Each match's literal sum, taken up to the match's cost, decides both, so
  one walk spells both expansions.

| Candidate | Instructions | Cycles |
| --- | ---: | ---: |
| One walk per slide cut search | −1.22% | −0.98% |
| Plus one window per decoded value | −0.97% | −0.78% |
| Plus paired expansions | −0.50% | −0.34% |

Each row is measured against the build before it.

| Measure | Before | After | Change |
| --- | ---: | ---: | ---: |
| 33 Default inputs, CPU over two rounds | 96.3 s | 94.4 s | 1.02× |
| Same inputs, cycles over two rounds | 301.0 billion | 294.8 billion | −2.07% |
| Same inputs, instructions retired | 1,246.8 billion | 1,213.4 billion | −2.67% |
| 89,255 header plans captured from ten Max runs, replayed (median) | 2.114 s | 2.130 s | unchanged |
| `036-Clefable-0.png`, Max completing its search, instructions retired | 21.77–21.82 billion | 21.78–21.79 billion | unchanged |

`Partnership_Card___John_Lewis_Finance.png`, the largest input, gained 1.05×.
Median peak memory was unchanged and the largest rise 2.7 MiB; `Apricot512.png`
and `BNDT_on_X…png`, whose slides walk the longest pairs, kept their peaks
within 1 MiB.

Equivalence: new tests compare the one-walk cut search with the three-walk
form on 20,000 random block pairs, with trees missing codes on either side and
tokens overrunning their block, and the paired expansion with the two separate
expansions on 20,000 random token lists, code lengths and short decoded
buffers; the existing parser and decoder tests cover the decoding window. The
89,255 replayed header plans matched plan for plan. API comparisons on the 52
inputs, in Default with the 180-second budget and in zero-budget Max, matched
in all 104 runs, and every timing run and both completing Max runs matched
byte for byte. Rust tests, including the private corpus, passed in release and
debug modes, with documentation and Python tests, a locked build, formatting,
all-feature Clippy with warnings denied and a package list without private
fixtures or scratch artifacts. `__text` grew from 863,120 to 865,300 bytes;
the shipped executable keeps its 942,064 bytes, and the final source builds
the tested executable byte for byte.

Measured and rejected in the same review:

- Remembering terminal block searches that found nothing: only 685 of 8,957
  block searches on the 33 inputs repeated a block state already searched in
  vain, 0.11 of their 7.25 seconds, and the searches share work budgets a skip
  would have to charge.
- Package-merge nodes of 16 instead of 32 bytes, with 32-bit indices: 0.03%
  more instructions and 0.20% more cycles.
- Pricing each payload tree once per alphabet instead of once per tree pair:
  0.25% fewer instructions and 0.17% fewer cycles, within run-to-run spread.

Artifacts: `work/efficiency-review-default6-20261028/`.

### `a6f67cd`, 28 October 2026

Batches: whole-file CPU (user plus system) and peak resident memory over the
33 stratified Default inputs, two rounds alternating the builds; retired
instructions and cycles from `time -l` over the same inputs, two alternating
rounds per comparison; three alternating runs of the header-plan replay; two
alternating runs of each completing Max run.

A sampled Default profile of the 33 inputs at `a6f67cd` (42,503 CPU samples)
still put 28.0% of self time in the DeflOpt and order-key heap builder, then
the boundary slide (4.9%), match expansion (4.7%) and the greedy RLE seeds
(3.9%). Earlier reviews had ruled out sharing steps between the heap variants,
which diverge at about the first merge, and branch-free and bottom-up
sift-downs.

- **One-word heap entries.** An entry held a 64-bit key, the frequency and the
  tie field, and a 32-bit index: 16 bytes with padding. The frequency now takes
  the top 32 bits, the tie field the next 16 and the index the low 16, so every
  sift step moves half the bytes. Whole-key comparisons use the top 48 bits and
  frequency-only ones the top 32, so the index never takes part, as before.
  Alphabets of up to 32,768 symbols fit, far more than Deflate's.
- **Match expansion answers once.** Expanding selected matches walks the
  tokens twice, counting the output and then spelling it, and asked the
  caller's predicate in both walks. Every predicate is pure, so the first walk
  records the expanded matches' indices and the second reads them. The second
  walk, which no longer needs the predicate, is one out-of-line function
  instead of a copy per caller.
- **Repeated greedy seeds skipped.** The greedy RLE spelling uses code 16 only
  for a nonzero run of at least 4, and code 17 only where a zero run leaves at
  least 3 after code 18's repeats. A mask that disables only codes the spelling
  would not use repeats an earlier mask's seed, which the planner discarded
  after spelling it; 14% of 5.0 million greedy spellings did.

| Candidate | Instructions | Cycles |
| --- | ---: | ---: |
| One-word heap entries | −2.16% | −3.38% |
| Plus match expansion answers, in line | −0.16% | −0.63% |
| Plus repeated greedy seeds skipped | −0.60% | −0.72% |
| Second expansion walk out of line, against the in-line form | −0.30% | −0.09% |

Each row is measured against the build before it; the first three were
measured with the slide buffer rejected below also in place.

| Measure | Before | After | Change |
| --- | ---: | ---: | ---: |
| 33 Default inputs, CPU over two rounds | 102.9 s | 98.9 s | 1.04× |
| Same inputs, cycles over two rounds | 322.3 billion | 312.5 billion | −3.03% |
| Same inputs, instructions retired | 1,286.3 billion | 1,248.6 billion | −2.93% |
| 89,255 header plans captured from ten Max runs, replayed (median) | 2.272 s | 2.190 s | 1.04× |
| `036-Clefable-0.png`, Max completing its search, instructions retired | 22.88 billion | 21.81–21.84 billion | −4.6% |
| `114-Tangela-1.png`, Max completing its search, instructions retired | 2.42 billion | 2.44 billion | +0.9% |

Median peak memory was unchanged and the largest rise 3.0 MiB. The rows above
were measured while other work loaded the machine; each comparison alternates
the builds input by input.

Equivalence: a new test compares match expansion with its two-walk form on
20,000 random token lists and pure predicates, with token limits and short
decoded buffers; the seed test now also compares every mask set with the
unpruned spellings on 5,000 random code-length lists with zero runs up to 300
and nonzero runs up to 12; the existing heap tests, which compare the builder
with its reference on 40,000 alphabets, and the sift-down test cover the packed
entries. The 89,255 replayed header plans matched plan for plan. API
comparisons on the 52 inputs, in Default with the 180-second budget and in
zero-budget Max, matched in all 104 runs, and every timing run and both
completing Max runs matched byte for byte. Rust tests, including the private
corpus, passed in release and debug modes, with documentation and Python
tests, a locked build, formatting, all-feature Clippy with warnings denied and
a package list without private fixtures or scratch artifacts. `__text` fell
from 867,336 to 863,120 bytes; the shipped executable keeps its 942,064 bytes,
and the final source builds the tested executable byte for byte.

Measured and rejected in the same review:

- Pricing each boundary-slide token once into a buffer: 0.28% fewer cycles,
  but at 32 bytes a token the buffer raised peak memory by up to 42 MiB on
  large inputs (`Apricot512.png` 83.8 to 118.4 MiB); 32-bit prices, a quarter
  the size, were no faster. The slide's 11,264 cut searches walk 326 million
  tokens on the 33 inputs, and the first walk alone takes 1.16 of its 2.5
  seconds.
- Expansion answers kept in the generic function: one copy per predicate grew
  `__text` by 4,240 bytes and the executable across a 16 KiB page.
- Skipping boundary-graph edges whose lower bound (entropy-coded payload,
  fixed and stored prices) reaches all eight destination states: 73 of 37,237
  edges qualified, 0.01 of the 8.03 seconds spent preparing edges; 29,980 edges
  improve some destination state.
- Pricing each payload tree once per alphabet instead of once per pair, at
  most 0.45% of samples, and the repeated shape check of the header-plan
  kernel, 0.25%.

Artifacts: `work/efficiency-review-default5-20261028/`.

### `3b5fa64`, 28 October 2026

Batches: whole-file CPU (user plus system) and peak resident memory over the
33 stratified Default inputs, two rounds alternating the builds; retired
instructions and cycles from `time -l` over the same inputs, two alternating
rounds per comparison; three alternating runs of the header-plan replay; two
alternating runs of each completing Max run.

A sampled Default profile of the 33 inputs at `3b5fa64` (47,669 CPU samples)
put 27% of self time in the DeflOpt and order-key heap builder, 57% of it on
code-length codes and 32% on payload trees. Counters in a test build found 21.7
million code-length heap builds and 1.6 million payload builds behind 774,253
uncached header plans. The block planner also scanned every token for the
extra-bit total of the block-plan key (1.8% of samples) and again for the
fixed-code price.

- **Block sums from counts.** Each length and distance symbol carries a fixed
  number of extra bits, so a block's extra-bit total comes from its 316 symbol
  counts. So do the fixed-code price and, in the dynamic planner, the payload
  of the source header. Debug builds check each against the token scan.
- **Shared heap trees.** Every variant of the DeflOpt and order-key heaps
  starts from the same frequency-ordered heap; they differ only in the tie rule
  of each merge sift-down. A comparison asks whether one entry is strictly less
  than another, so it turns on a rule only when the frequencies are equal and
  the entry that would move up has the smaller height (DeflOpt) or creation
  order (order-key). A build that notes no such comparison follows the
  frequency-only steps, as every variant of its heap then does; with neither
  kind, every variant of both heaps does, and all build and repair the same
  tree. The code-length tree cache stores that tree for each of them, and the
  payload tree lists and the short-family floor's variant loops skip builds
  that would only repeat it. On the 33 inputs the DeflOpt variants agree on
  21.8% of code-length builds and all seven trees on 10.5%. Noting adds work
  to every comparison, so only a histogram's first heap tree and the first
  build in each list track, and only variant 0, which they all ask for first,
  has a tracking merge.
- **Package-merge buffers.** Defluff's limited code-length builder runs
  package-merge when its tree is deeper than 7 bits; the seven buffers it
  allocated per call are kept in the thread's tree-build scratch.

| Candidate | Instructions | Cycles |
| --- | ---: | ---: |
| Block sums from counts, against `3b5fa64` | −5.28% | |
| Shared heap trees and package-merge buffers, against the block sums | +0.38% | −2.72% |
| Package-merge buffers alone, with an earlier form of the sharing | −0.16% | −0.32% |

| Measure | Before | After | Change |
| --- | ---: | ---: | ---: |
| 33 Default inputs, CPU over two rounds | 106.7 s | 99.7 s | 1.07× |
| Same inputs, cycles over two rounds | 335.3 billion | 312.9 billion | −6.66% |
| Same inputs, instructions retired | 1,351.3 billion | 1,285.1 billion | −4.90% |
| 89,255 header plans captured from ten Max runs, replayed (median) | 2.264 s | 2.179 s | 1.04× |
| `114-Tangela-1.png`, Max completing its search, instructions retired | 2.50 billion | 2.42 billion | −3.2% |
| `036-Clefable-0.png`, Max completing its search, instructions retired | 22.89–22.90 billion | 22.91–22.92 billion | +0.1% |

The largest inputs gained 1.05–1.07×, `Banana512.png` 1.23×. Median peak memory
fell 0.4 MiB, and the largest rise was 4.7 MiB.

Equivalence: a new test builds 20,000 random alphabets of code-length,
distance and literal/length size, mostly of small, tied counts, and checks
every tracked build of both heaps, all variants, at limits 7 and 15, against
the untracked builds, and every tree a build reports as shared against that
variant's own. Another asks 20,000 random histograms for all 15 code-length
builders in shuffled order through the tree cache and compares each with the
builder's own tree. The 89,255 replayed header plans matched plan for plan. API
comparisons on the 52 inputs, in Default with the 180-second budget and in
zero-budget Max, matched in all 104 runs, and every timing run and both
completing Max runs matched byte for byte. Rust tests, including the private
corpus, passed in release and debug modes, with documentation and Python tests,
a locked build, formatting and a package list without private fixtures or
scratch artifacts. All-feature Clippy with warnings denied passed once three
errors present at `3b5fa64` were fixed separately: two `state` parameters and
a struct update that `9f36bdb` left unused. `__text` grew from 864,312 to
867,336 bytes; the shipped executable keeps its 942,064 bytes, and the final
source builds the tested executable byte for byte.

Measured and rejected in the same review:

- Tracking ties in every heap build: 1.006× over the block sums alone, the
  noting about as costly as the builds it saved.
- A tracking copy of all four merges: about 2.5% fewer cycles than the block
  sums, but `__text` reached 869,504 bytes and the shipped executable crossed
  a 16 KiB page, from 942,064 to 958,480 bytes. A runtime switch kept the size
  but retired 2.56% more instructions for 1.95% fewer cycles.
- Weaker conditions for sharing: only 11% of code-length builds compare no
  equal counts with different tie fields at all; requiring the second-least
  count below the third at every merge, which would also share deft4j's and the
  generic builder's trees, held for 15.1%, and 11.5% unrepaired.
- Reusing work across a header plan's RLE seeds: 214,408 of 1.7 million later
  feedback passes, and 6.8% of 9.55 million local-rewrite steps, repeat an RLE
  or step an earlier seed explored, and most seeds stop after their first pass.
- Skipping provably duplicate greedy RLE seeds: 14% of 5.0 million greedy
  spellings are dropped as duplicates, about 0.6% of CPU.

Artifacts: `work/efficiency-review-default4-20261027/`.

### `e25d20d`, 25 October 2026

Batches: three alternating runs of the header-plan replay, counting retired
instructions; two alternating runs of each completing Max run; one paired run
of each build on every sixteenth DeflOpt PNG by name (58 files, Max at 10
seconds, alternating order); whole-file CPU and peak resident memory over the
33 stratified Default inputs, once per build in alternating order.

A sampled profile of ten inputs in Max at 20 seconds at `e25d20d` (233,724 CPU
samples) put 27% under the code-length tree builders: the DeflOpt and
order-key heaps took 16.3% of samples and the generic node builder 8.2%. The
replayed header plans made 731,419 feedback passes, 52% of them on an RLE
histogram seen for the first time, and each pass asked for up to 14 trees.
Most of those trees could not change the result.

- **A first tree no other can beat.** A feedback pass prices every candidate
  code-length code for its RLE, keeps any strictly cheaper header, and
  follows the first code with the least RLE cost. Its first candidate is the
  order-key heap's variant 0. When that is a Huffman tree that fits 7 bits
  without repair and codes at least two symbols, no prefix code spells the
  RLE's symbols in fewer bits; the repeat codes' extra bits are the same under
  every code. Every usable code must give each of those symbols a length, so
  none sends fewer code-length codes either. Every other builder's code,
  reordered or not, can then at best tie on both counts, and a tie keeps the
  first. The pass prices and follows the first code alone, after deft4j's
  pruned header, which prices a different RLE and runs as before. The heap
  builder now reports a repair, and the tree cache keeps that with each tree.
  89% of the replayed passes (650,657 of 731,419) qualify.
- **Seeds and shortest RLEs counted as spelled.** The greedy, balanced and
  zero-repeat seeds and the shortest RLE add each run's explicit lengths and
  their repeat codes as they spell it, instead of counting the finished RLE.

| Measure | Before | After | Change |
| --- | ---: | ---: | ---: |
| 89,255 header plans captured from ten Max runs, replayed (median) | 3.33 s | 2.32 s | 1.44× |
| Same replay, instructions retired | 41.4 billion | 31.2 billion | −24.8% |
| `036-Clefable-0.png`, Max completing its search, instructions retired | 29.65 billion | 22.91 billion | −22.7% |
| `036-Clefable-0.png`, Max completing its search, CPU | 2.65 s | 2.02 s | 1.31× |
| `114-Tangela-1.png`, Max completing its search, instructions retired | 2.61 billion | 2.49 billion | −4.3% |
| 33 Default inputs, CPU | 58.8 s | 54.4 s | 1.08× |

The first-tree rule alone took the replay from 41.4 to 31.5 billion
instructions; counting as spelled saved another 1.4%. Over the Default inputs,
median peak memory rose 0.3 MiB and the largest rise was 2.6 MiB; the
completing Max runs rose 0.3 MiB or less in all but one run, which rose 1.6.

Deadline-bound Max cannot show equivalence, so it was checked for losses: on
every sixteenth DeflOpt PNG by name (58 files, Max at 10 seconds, alternating
builds), 54 outputs were identical and three were smaller, by 2, 5 and 15
bytes, in 444.7 instead of 451.3 seconds. `classr.png` was 14 bytes larger,
9,471 instead of 9,457, with the machine loaded; its primary routes stop at
their 8-second share, and six more runs of each build at 10 seconds all gave
9,457 bytes.

Equivalence: a new test builds 20,000 random RLE histograms, sparse, tied,
skewed and some deep enough to need repair, and wherever the first tree is a
complete unrepaired Huffman tree, checks every builder's usable code,
reordered or not, against it for RLE cost and header bits. The seed spellers'
reference test now also checks their counts against a recount, and debug
builds check the counts of every seed and shortest RLE. The 89,255 replayed
header plans matched plan for plan. API comparisons on the 52 inputs, in
Default with the 180-second budget and in zero-budget Max, matched in all 104
runs, and every Default timing run and both completing Max runs matched byte
for byte. Rust tests, including the private corpus, passed in release and
debug modes, with documentation and Python tests, a locked build, formatting,
all-feature Clippy with warnings denied and a package list without private
fixtures or scratch artifacts. `__text` grew from 859,052 to 861,676 bytes;
the shipped executable keeps its 942,064 bytes.

Measured and rejected in the same review:

- Keeping each tree's completeness and frequency-order checks with it in the
  tree cache, and reading a histogram's cache entry once per pass: a pass
  built about as many trees as it priced candidates, so the checks moved
  rather than vanished, and the replay retired 3.1% more instructions.
- One RLE cost per priced candidate, and branch-free comparison of candidate
  trees: 0.2% fewer instructions before the first-tree rule, which leaves the
  candidate loop to 11% of passes.
- Counting the feedback pass's DeflOpt rewrite before building it, since the
  shortest RLE replaces it unless they tie (93% of 569,080 replayed
  comparisons replaced it): 0.6% fewer instructions and no fewer cycles. Its
  cost is the token loop, not the output.
- A lower bound to skip planning token states in the ordered queue: a state
  joins the beam within 256 bits of its parent, and a Huffman bound on the
  payload misses the header's hundreds of bits.

Not changed in the same review:

- The DeflOpt local rewrite's trees, one per heap variant and state, about
  10% of samples after these changes: its path follows each tree's lengths,
  which a tree of equal cost can change.

Artifacts: `work/efficiency-review-max-huffman/`.

### `2c702f5`, 25 October 2026

Batches: medians of three alternating runs of the header-plan replay; two
alternating runs of each completing Max run; two alternating rounds of 403,865
shortest-RLE calls captured from Max runs; one paired run of each build on
every sixteenth DeflOpt PNG by name (58 files, Max at 10 seconds, alternating
order); whole-file CPU and peak resident memory over the 33 stratified Default
inputs, once per build in alternating order.

A sampled profile of ten inputs in Max at 20 seconds at `2c702f5` (233,398 CPU
samples) put the shortest-RLE search at 18.8% of samples, the DeflOpt and
order-key heaps at 13.8% and the generic node builder at 6.6%. The search
solved a header's 300-odd code lengths position by position, reading up to 12
earlier results at each position of a long zero run: 107 instructions per
position on the captured calls. Keeping only each position's token instead of
a 24-byte step, fixed-width windows and a loop over runs saved 5% of its
instructions and 10% of its cycles. A second profile, with the run spellings
below, put the header-plan fingerprint at 1.9% of samples, hashing the
code-length tree slot at 1.5% and comparing RLE histograms through `memcmp` at
1.1%.

- **Run spellings.** No repeat code crosses a run of equal code lengths: code
  16 repeats the length before it, and codes 17 and 18 write zeros. The
  shortest RLE is therefore each run's cheapest spelling in turn, and that
  spelling depends only on the run's length and the costs of its own code and
  of the repeat codes that can spell it: four costs for zeros, two otherwise.
  Each thread keeps, for each combination of costs, the token that starts
  each run length and the token that continues from each length left inside
  a run. Each is solved as the position-by-position search solved the
  position that many lengths before a run's end, so ties keep the same
  choice. A table too short for a run is rebuilt at least twice as long, up
  to the header's count of code lengths. Costs above 7 bits, which no
  code-length code has, can share a slot and rebuild it.
  Tables hold at most 131,072 run lengths at 4 bytes each before all are
  dropped, and are released with the thread's header plans. With every
  combination kept, the captured calls built tables for 0.27% of positions
  in the Clefable run and 0.08% elsewhere; per-call tables, rejected at
  [`d72c937`](#d72c937-23-october-2026), rebuilt them for every search.
- **Plan runs shared.** The planner finds a plan's runs once, and its RLE
  seeds and every shortest RLE for the plan read that list. The per-plan memo
  lends its shortest RLE, copied only when it beats the local rewrite.
- **Eight lengths per fingerprint step.** The header-plan fingerprint mixes
  eight code lengths at a time, after each side's count, with an odd
  multiplier spread across the word, instead of one length per
  multiplication. Both header caches compare complete code lengths on a
  fingerprint match, so the hash only places entries.
- **One tree slot per feedback pass.** A feedback pass asks the code-length
  tree cache for up to 14 builders' trees for one histogram, and now hashes
  it once. Histograms are compared by combining their differences, which
  vectorizes, instead of through `memcmp`.
- **Reused buffers.** The generic builder's reverse two-queue merge keeps its
  three buffers in the shared tree-building scratch, and an improved header
  plan reuses the previous best plan's buffers. Copying the code lengths only
  once a plan is chosen was rejected at
  [`bab3748`](#bab3748-25-october-2026) for 0.17% of Default samples; these
  allocations took 0.6% of Max samples.
- **Plans packed in pairs.** Packing a plan for the stores packs its code
  lengths two at a time and writes its RLE tokens into a presized buffer.

| Measure | Before | After | Change |
| --- | ---: | ---: | ---: |
| 403,865 captured shortest-RLE calls, run spellings only | 0.92 s | 0.29 s | 3.2× |
| 89,255 header plans captured from ten Max runs, replayed | 3.93 s | 3.29 s | 1.19× |
| `036-Clefable-0.png`, Max completing its search, instructions retired | 55.14 billion | 29.65 billion | −46.2% |
| `036-Clefable-0.png`, Max completing its search, CPU | 4.25 s | 2.64 s | 1.61× |
| `114-Tangela-1.png`, Max completing its search, instructions retired | 2.99 billion | 2.61 billion | −12.8% |
| 33 Default inputs, CPU | 59.8 s | 58.3 s | 1.03× |

The captured calls retired 4.63 instead of 13.28 billion instructions per
round. With the run spellings alone, the replay took 3.47 against 3.93
seconds, and the completing runs retired 34.27 and 2.71 billion instructions.
Peak memory rose 0.25 MiB in the Clefable run; over the Default inputs the
median fell 0.4 MiB and the largest rise was 0.8 MiB.

A profile of the same ten inputs with every change put the shortest-RLE search
at 5.0% of samples, the DeflOpt and order-key heaps at 16.8% and the generic
node builder at 8.5%.

Deadline-bound Max cannot show equivalence, so it was checked for losses: on
every sixteenth DeflOpt PNG by name (58 files, Max at 10 seconds, alternating
builds), 55 outputs were identical and two were smaller, by 1 and 12 bytes, in
449.7 instead of 469.5 seconds. `drive-blue-disk-fs8.png` was 1 byte larger,
15,699 instead of 15,698 bytes, in repeated runs. The baseline gives the same
15,699 bytes at 11 seconds and 15,696 at 12, and the candidate gives 15,696 at
11. With the time saved, three more terminal routes run before the deadline
and take the stream from 117,959 to 117,922 bits; the bounded-depth tree floor
then finds nothing, where from 117,959 bits it reached 117,920. The speedup
reaches the baseline's own step between 10 and 11 seconds a second sooner,
without dropping a route.

Equivalence: before any other change, the run spellings matched the previous
search on all 403,865 captured calls. A new test keeps one scratch across
6,000 random searches, with runs up to 320 long and costs above 7 bits in
every fifth, compares each result with a fresh scratch and every fortieth
with the scanned oracle, and reaches the entry limit. Another packs 2,000
random plans, with odd and even counts, empty sides, 32 distance codes and
rejected lengths, and compares the bytes with packing one length at a time
and the unpacked plan with the original. The thread-plan test now also checks
that releasing a thread's plans drops its search buffers. The existing
shortest-RLE oracle tests pass. The 89,255 replayed header plans matched plan
for plan. API comparisons on the 52 inputs, in Default with the 180-second
budget and in zero-budget Max, matched in all 104 runs, and every Default
timing run and both completing Max runs matched byte for byte. Rust tests,
including the private corpus, passed in release and debug modes, with
documentation and Python tests, a locked build, formatting, all-feature
Clippy with warnings denied and a package list without private fixtures or
scratch artifacts. `__text` grew from 855,804 to 859,052 bytes and the
shipped executable from 942,048 to 942,064.

Not changed in the same review:

- Building one heap variant's tree for all seven: in the replay, 62% of the
  2.94 million DeflOpt and order-key builds gave the tree all seven variants
  give, but only 2.3% compared no equal frequencies, and only 13% merged sets
  that equal frequencies could not change. Checking costs about one build,
  which outweighs skipping six in 13%.
- A compact node for the generic and deft4j builders, whose 88-byte nodes are
  mostly optional links: it would change about 70 field uses to save part of
  the generic builder's 8.5% of samples.

Artifacts: `work/efficiency-review-max-spellings/`.

### `d96ab5c`, 25 October 2026

Batches: two alternating runs of the header-plan replay and of each completing
Max run; one paired run of each build on every sixteenth DeflOpt PNG by name
(58 files, Max at 10 seconds, alternating order); whole-file Default CPU over
the 33 stratified inputs, once per build in alternating order.

A sampled profile of ten inputs in Max at 20 seconds (236,923 CPU samples) put
47% under the ordered state queue, Max's beam search over token states, and
71% under the uncached header planner. The shortest-RLE search took 18.9% of
samples, with its earliest-minimum scan another 2.3% when kept out of line;
the DeflOpt and order-key heaps 13.6%; and the generic node builder, which
Max runs for four more code-length trees per histogram, 10.4%, of which its
two-heap merge took 3.7%. The queue hashed each visited token sequence with
the standard library's SipHash (1%), and allocation took 4.5%.

- **Generic builder buffers.** The generic builder's heap merges fill the
  keyed heaps and active flags kept in the shared tree-building scratch
  instead of allocating them on every build.
- **Earliest minimum by selection.** The shortest-RLE search finds the first
  smallest remainder of each repeat window with conditional selects; which
  value is smallest is unpredictable, so the branches mispredicted.
- **Shortest RLE counted after assembly.** Its symbols are counted in four
  interleaved tables once the path is assembled, instead of one increment per
  token as it is.
- **Comparison from counts.** The feedback pass compares the shortest RLE
  with the DeflOpt local rewrite under the feedback tree. Both arrive with
  their exact symbol counts, which price them as their tokens would, so the
  comparison no longer walks their tokens.
- **Tree slot in four lanes.** The code-length tree cache hashes its 19-count
  histogram in four independent lanes, so the multiplications overlap. A hit
  still compares the whole histogram.
- **Visited token sequences.** The ordered queue's set of visited token
  sequences only answers membership and compares whole sequences, so it uses
  a multiply-rotate hash of each field instead of SipHash; it holds a few
  hundred sequences at most, so collisions cost little.

| Measure | Before | After | Change |
| --- | ---: | ---: | ---: |
| 89,255 header plans captured from ten Max runs, replayed, first four changes | 3.98 s | 3.84 s | 1.04× |
| `036-Clefable-0.png`, Max completing its search, instructions retired | 57.06 billion | 55.13 billion | −3.4% |
| `114-Tangela-1.png`, Max completing its search, instructions retired | 3.34 billion | 2.99 billion | −10.5% |
| 33 Default inputs, CPU | 59.9 s | 59.4 s | 1.01× |

With all six changes the replay took 3.85 and 3.94 seconds against 3.96 and
3.95, a noisier pair. Default peak memory was unchanged.

Deadline-bound Max cannot show equivalence, so it was checked for losses: on
every sixteenth DeflOpt PNG by name (58 files, Max at 10 seconds, alternating
builds), all 58 outputs were identical, in 469.2 instead of 470.3 seconds.

Equivalence: new tests compare the branch-free earliest minimum with the
previous scan on 20,000 random windows with ties and unreachable entries, and
the count-based price of an RLE with its token walk on 20,000 random RLEs and
trees, including unusable codes. The existing comparisons of the generic
builder's heap merges with their reference implementations pass with the
buffers, as does the shared-scratch test. The 89,255 replayed header plans
matched plan for plan. API comparisons on the 52 inputs, in Default with the
180-second budget and in zero-budget Max, matched in all 104 runs, and every
Default timing run and both completing Max runs matched byte for byte. Rust
tests, including the private corpus, passed in release and debug modes, with
documentation and Python tests, a locked build, formatting, all-feature Clippy
with warnings denied and a package list without private fixtures or scratch
artifacts. `__text` grew from 855,572 to 855,804 bytes; the shipped executable
keeps its 942,048 bytes.

Not changed in the same review:

- The shortest-RLE search itself, the largest Max cost: spelling each run
  from per-value tables, which a run's independence would allow, was measured
  and rejected at [`d72c937`](#d72c937-23-october-2026).
- A sorted list in place of the generic builder's two heaps: its keys are
  distinct, so heap layout cannot affect the result, but payload alphabets
  would need a second structure for 3.7% of samples.

Artifacts: `work/efficiency-review-max-20261025/`.

### `bab3748`, 25 October 2026

Batches: none; whole-file CPU (user plus system) and peak resident memory over
the 33 stratified Default inputs, once per build in alternating order, in three
comparisons; two alternating runs of each completing Max run and of the
header-plan replay.

A sampled Default profile of the 33 inputs at `bab3748` (54,301 CPU samples)
still put 27% in the DeflOpt and order-key heap builders and 48% under the
uncached header planner. The rest spread over steps of 1% to 4%. Several
repeated work whose result was already known: the short-family floor priced
four trees per length family by rescanning its tokens (`token_bits`, 1.7% of
samples), merged trees got their depths by recursion (1.8%), package-merge
walked each selected package's leaves recursively (0.8%), and each of the
777,383 uncached header plans started from empty buffers.

- **Short-family pricing from counts.** The floor already counts its expanded
  tokens to build each tree; a tree's data bits are the counts times the code
  lengths plus the extra bits, so each of the four trees per length family is
  priced in one pass over 316 counts instead of over the tokens. The counts
  are taken before the floor's added distance codes, and an unusable code
  rejects a tree in the same cases.
- **Tree depths in one pass.** deft4j's Java-heap builder and the generic node
  builder create every branch after its children, with the root last, so one
  pass from the last node gives each node its depth before its children need
  it; deft4j's overflow repair, which re-parents nodes, still walks the tree.
- **Package-merge counts.** A leaf's length is how often it occurs in the
  selected nodes. Every package follows the nodes it pairs, so one pass from
  the last node hands each node's count to its children, instead of walking
  each selected package's leaves.
- **Reused header-plan buffers.** Each thread keeps the uncached header
  planner's search, with its tree-builder and shortest-RLE buffers, between
  plans; the best plan and the shortest-RLE memo, which belong to one plan's
  code lengths, are cleared for each plan.
- **Whole-run rewrite copies.** The DeflOpt and deft4j RLE rewrites copy each
  unchanged run of tokens in one slice at the next change or the end, instead
  of pushing every token after the first change.

| Candidate | CPU before | CPU after | Speedup | Peak memory, median / largest increase |
| --- | ---: | ---: | ---: | ---: |
| Short-family pricing, tree depths, package-merge counts | 65.7 s | 63.3 s | 1.04× | −0.2 / +4.2 MiB |
| Plus reused header-plan buffers | 62.9 s | 61.0 s | 1.03× | −0.2 / +0.4 MiB |
| Plus whole-run rewrite copies (retained) | 63.4 s | 60.5 s | 1.05× | 0.0 / +1.0 MiB |

Each comparison after the first also reran the previous candidate, which took
61.3 seconds both times.

| Measure | Before | After | Speedup |
| --- | ---: | ---: | ---: |
| 89,255 header plans captured from ten Max runs, replayed | 4.24 s | 4.08 s | 1.04× |
| `036-Clefable-0.png`, Max completing its search | 2.50 s | 2.44 s | 1.02× |
| `114-Tangela-1.png`, Max completing its search | 0.25 s | 0.25 s | 1.00× |

The Clefable run retired 57.1 instead of 57.8 billion instructions; the
Tangela run 3.34 instead of 3.33 billion.

Equivalence: a new test compares package-merge with its previous recursive
form on 20,000 random alphabets, depth limits from 4 to 15 bits and both tie
rules; another builds 6,000 random alphabets with the generic builder's four
variants and deft4j's builder, unlimited, and compares the lengths with the
recursive walk over the same nodes, and debug builds check that each merged
tree's root is its last node. The existing comparisons of both RLE rewrites
with their reference implementations, counts included, still pass. The
89,255 replayed header plans matched plan for plan. API comparisons on the 52
inputs, in Default with the 180-second budget and in zero-budget Max, matched
in all 104 runs, and every CLI timing run and both completing Max runs matched
byte for byte. Rust tests, including the private corpus, passed in release and
debug modes, with documentation and Python tests, a locked build, formatting,
all-feature Clippy with warnings denied and a package list without private
fixtures or scratch artifacts. `__text` grew from 853,932 to 855,572 bytes; the
shipped executable keeps its 942,048 bytes.

Measured and rejected in the same review:

- A per-thread cache of payload trees by histogram: over the 33 inputs, only
  4.6% of the 92,109 literal/length histograms the dynamic planner builds
  trees for repeat on their thread, even without a size limit; distance
  histograms repeat 24%, but their 30-symbol trees are cheap.
- A larger header-plan store: 6.6% of uncached header plans repeat one their
  thread planned before, 7.7% of the uncached planning time.
- Copying a header plan's payload code lengths only once it is chosen: the
  copies on each improvement cost 0.17% of samples.

Artifacts: `work/efficiency-review-default3-20261025/`.

### `e79a60a`, 24 October 2026

Batches: none; whole-file CPU (user plus system) and peak resident memory over
the 33 stratified Default inputs, once per build in alternating order, in five
comparisons; two alternating runs of each completing Max run and of the
header-plan replay; one run of each build on five larger inputs.

A sampled Default profile of the 33 inputs at `e79a60a` (66,017 CPU samples)
still put 27% in the DeflOpt and order-key heap builders, 58% of that for
code-length trees and 32% for payload trees. Allocation took 2.9%, and another
1% went to `mach_absolute_time` in macOS's `free` and large-allocation paths:
deft4j's builder reserved 1,024 nodes of 88 bytes on every call. Validation
parsing took 7.4%, 6.2% of it decoding Huffman payloads. After the first two
changes below, a build with header helpers kept out of line put counting an
RLE seed's symbols at 2.5% and pricing a symbol histogram under a tree at
1.4%.

- **Keyed heap entries.** Each DeflOpt-style heap entry holds its frequency
  above its tie field, the height or creation order, in one 64-bit key, beside
  its index; a sift-down that breaks ties by frequency alone compares the
  keys' high halves. The comparisons are those of the previous packed keys, so
  every heap moves as before. The merge loop is specialized for each variant's
  pair of tie rules, and the leaves' removal order, which only overflow repair
  needs, is read from the branches' children when it does. On a sample of the
  builder's inputs from the 33 Default runs (27,771 code-length, 13,614
  distance and 13,428 literal/length histograms, each built in all eight
  variants), every result matched. Medians of five runs: 0.358 instead of
  0.406 seconds on the code-length histograms (1.13×), 0.289 instead of 0.345
  on distance (1.19×) and 2.91 instead of 3.75 on literal/length ones
  (1.29×).
- **Shared tree buffers.** deft4j's Java-heap builder and the generic node
  builder take their nodes and indices from the scratch the heap builder
  already shares, renamed `TreeBuildScratch`, instead of allocating them on
  every call; deft4j's buffers are sized to the alphabet. Allocation fell to
  2.3% of samples and `mach_absolute_time` to 0.5%.
- **Keyed Java heap.** deft4j's heap holds each node's frequency above its
  index, so the heap no longer reads 88-byte nodes to compare frequencies,
  the only key deft4j compares: 1.15× on code-length, 1.27× on distance and
  1.29× on literal/length histograms.
- **RLE counts and costs.** An RLE's symbols are counted in four interleaved
  tables, so a run of one symbol does not wait for each increment's store:
  2.6× on 104,029 RLE seeds of 220 tokens on average, spelled from the sampled
  payload histograms. A histogram's cost under a tree no longer saturates each
  term: a used symbol without a code gives `INF`, and otherwise each term is
  below 2^41, so a wrapping sum over a table of extra bits vectorizes: 3.0×.
- **Rewrites that count changes.** The DeflOpt local rewrite starts from its
  input's counts and adjusts them at each change instead of recounting every
  token, and deft4j's prune builds its output from the first change, as the
  DeflOpt rewrite already did.
- **Word-at-a-time bit reading.** With eight bytes of input left, the bit
  reader loads them at once and keeps the whole bytes that fit, instead of
  one byte per loop. Bits above those buffered are the input's next bits in
  place, so a later load rewrites them unchanged, and a stored block's byte
  copy first hands whole buffered bytes back to the input. Parsing four GZIP
  fixtures took 0.184 instead of 0.244 seconds.
- **Planner extra bits.** A block-plan store miss passes the extra-bit total
  its key already summed to the dynamic planner, which summed it again. The
  [`71ecf9f`](#71ecf9f-24-october-2026) review rejected this at about 0.75% of
  CPU, expecting a parameter threaded through many callers; the planner has
  two callers.
- **Count fingerprints.** The canonical plan cache fingerprints a block's
  token count and symbol counts, which its key compares too, instead of every
  token, so its cost no longer grows with the block; blocks whose tokens
  differ only in order share a chain, and the token comparison tells them
  apart. Both block-plan fingerprints mix counts in four independent lanes.
  Fingerprints only select entries, so neither change can alter a plan.

| Candidate | CPU before | CPU after | Speedup | Peak memory, median / largest increase |
| --- | ---: | ---: | ---: | ---: |
| Keyed heap entries | 76.5 s | 72.8 s | 1.05× | −0.2 / +1.2 MiB |
| Plus shared tree buffers | 76.7 s | 72.6 s | 1.06× | −0.1 / +2.5 MiB |
| Plus RLE counts and costs, bit reading, planner extra bits | 76.1 s | 67.7 s | 1.12× | −0.7 / −0.2 MiB |
| Plus rewrites that count changes, lane fingerprints | 76.0 s | 64.1 s | 1.19× | −0.1 / +1.1 MiB |
| Plus count fingerprints, keyed Java heap (retained) | 76.0 s | 62.6 s | 1.21× | −0.1 / +0.3 MiB |

Each comparison after the first also reran the previous candidate, which took
72.9, 71.6, 67.5 and 64.1 seconds. The peak-memory changes are within
run-to-run spread.

| Measure | Before | After | Speedup |
| --- | ---: | ---: | ---: |
| 89,255 header plans captured from ten Max runs, replayed | 4.62 s | 4.14 s | 1.12× |
| `036-Clefable-0.png`, Max completing its search | 2.68 s | 2.48 s | 1.08× |
| `114-Tangela-1.png`, Max completing its search | 0.26 s | 0.25 s | 1.04× |
| `703080_80061…` (APNG), Default CPU | 15.1 s | 12.1 s | 1.25× |
| `1172470_101537…` (APNG), Default CPU | 14.7 s | 11.6 s | 1.27× |
| `bored.png`, Default CPU | 2.9 s | 2.6 s | 1.11× |
| `elephant-2.png`, Default CPU | 3.4 s | 2.8 s | 1.22× |
| `kzipmix-20200115-linux-static.tar.gz`, Default CPU | 10.2 s | 8.8 s | 1.16× |

The Clefable run retired 57.8 instead of 61.5 billion instructions and the
Tangela run 3.33 instead of 3.43 billion.

Equivalence: the existing test comparing the heap builder with its
independent node-based reference on 40,000 random alphabets, in both limits,
variants and tie keys, still passes, as does the hole-against-swapping
sift-down test, now on keyed entries. A new test builds 6,000 random
alphabets with every builder in turn on one shared scratch and compares each
with a fresh build. Another runs 4,000 random inputs of up to 40 bytes
through 60 random peeks, reads, alignments and stored-byte reads, continuing
after errors, against a reader that takes bits by index; it fails if stored
reads do not hand back buffered bytes. The existing comparisons of both
rewrites with their reference implementations, counts included, still pass,
and debug builds check the DeflOpt rewrite's adjusted counts and the
planner's extra bits. The 89,255 replayed header plans matched plan for plan.
API comparisons on the 52 inputs, in Default with the 180-second budget and in
zero-budget Max, matched in all 104 runs, and every CLI timing run, the five
larger inputs and both completing Max runs matched byte for byte. Rust tests,
including the private corpus, passed in release and debug modes, with
documentation and Python tests, a locked build, formatting, all-feature
Clippy with warnings denied and a package list without private fixtures or
scratch artifacts. `__text` grew from 845,520 to 850,796 bytes, mostly the
four specialized merge loops; the shipped executable keeps its 925,632
bytes, and the final source builds the tested executable byte for byte.

Measured and rejected in the same review:

- A branch-free sift-down over sentinel-padded heaps, running a fixed
  `floor(log2(len))` levels: exact, but 0.425 instead of 0.361 seconds on the
  code-length sample and 3.04 instead of 2.91 on literal/length histograms.
- Fixed-size stack arrays for alphabets of at most 32 symbols: 1.11× on
  code-length histograms, but only with a second, fixed-size builder; the
  shared slice-based builder on stack arrays or growing scratch gained 1.01–
  1.04×.
- Two or four interleaved tables for a block's token counts: 1.04–1.19× on
  whole blocks of four GZIP fixtures, whose literals vary more than RLE
  symbols, and 2.6–3.4× slower on 64-token slices.
- Reading nodes by reference in the recursive depth assignment: no change; the
  compiler already avoided the copies.
- Fingerprinting every token in four lanes: the canonical fingerprint stayed
  at 1.8% of samples, its cost the branch on each token's kind.

Artifacts: `work/efficiency-review-default2-20261024/`.

### `71ecf9f`, 24 October 2026

Batches: none; whole-file CPU (user plus system) and peak resident memory over
the 33 stratified Default inputs, once per build in alternating order, in
three comparisons; two alternating runs of each completing Max run and of the
header-plan replay.

A sampled Default profile of the 33 inputs (72,407 CPU samples) put 25% under
code-length trees: 15% in the DeflOpt and order-key heap builders' sift-down
and 9% in the rest of those builders. The code-length tree cache answered 51%
of 75.3 million requests, and each heap variant was built about 4 million
times, once per histogram the cache did not hold. Below those, three self
costs repeated work whose outcome was already known: the fixed-tree boundary
slide (R1c) at 5.8%, match-to-literal expansion at 5.5% and the greedy
code-length RLE at 4.2%.

- **Settled slide boundaries.** A boundary's cheapest cut depends only on its
  two blocks' tokens and trees, yet every sweep and round searched every
  boundary again. Each block now carries a generation that a move or a
  re-plan bumps, and a boundary whose search found no cheaper cut records both
  blocks' generations; it is skipped until either changes. A search the stop
  cuts short records nothing. Counted in a test build, 59% of the slide's
  27,374 searches on the 33 inputs repeated a pair, with the same tokens and
  trees, that the same slide had already searched; on the Partnership card,
  which made 22,002 of them, 64%.
- **Literal sums that stop.** Match-to-literal expansion, the token replay's
  dearer-match check and Defluff's expansion compare a match's cost with the
  sum of its bytes' literal costs, and summed every byte first. Every literal
  costs at least one bit, so the sum now stops as soon as it decides the
  comparison: once it exceeds the match's cost, or reaches it where a tie
  keeps the match. A 258-byte match costing 20 bits reads at most 21 of its
  bytes. The slide prices a joining match, the cheaper of its own spelling and
  its literals, the same way. The length-family deltas and ranked trials,
  which need exact totals, still sum every byte.
- **Code-length runs found once.** Header plans spell their code lengths as
  RLE seeds under up to eight masks of codes 16, 17 and 18: greedily, and in
  balanced and zero-repeat forms where the runs allow. Each spelling, and the
  check for those forms, scanned the lengths for runs of equal values. The
  runs are now found once per plan and every spelling reads that list, its
  loop otherwise unchanged. This differs from the per-value run tables
  rejected at [`d72c937`](#d72c937-23-october-2026), which changed how each
  run is spelled.

| Candidate | CPU before | CPU after | Speedup | Peak memory, median / largest increase |
| --- | ---: | ---: | ---: | ---: |
| Settled slide boundaries | 84.2 s | 81.8 s | 1.03× | 0.0 / +0.8 MiB |
| Plus literal sums that stop | 87.1 s | 80.5 s | 1.08× | −0.2 / +2.0 MiB |
| Plus code-length runs found once (retained) | 84.9 s | 76.8 s | 1.11× | +0.1 / +3.0 MiB |

In the same comparisons, the settled boundaries alone took 85.2 seconds and
the stopping sums without the run list 78.5. Median peak memory was 39.4 MiB
before and 39.1–39.3 MiB with each candidate; the largest increases are within
run-to-run spread, since the settled marks take 12 bytes per boundary and the
run list is freed once a plan's seeds are spelled. A second profile, with the
first two changes, put the slide at 2.9% of samples and match expansion at
2.9%.

| Measure | Before | After | Speedup |
| --- | ---: | ---: | ---: |
| 89,255 header plans captured from ten Max runs, replayed | 4.88 s | 4.79 s | 1.02× |
| `036-Clefable-0.png`, Max completing its search | 2.84 s | 2.72 s | 1.04× |
| `114-Tangela-1.png`, Max completing its search | 0.27 s | 0.27 s | 1.00× |

The Clefable run retired 61.5 instead of 63.7 billion instructions and the
Tangela run 3.43 instead of 3.46 billion. Max header plans spend most of their
time in the shortest RLE and code-length trees, so the replay changed little.

Equivalence: new tests compare the stopping sums with full sums for all three
expansion checks on 20,000 random token lists and code lengths, including
missing codes; the slide's joining price with the previous one on 20,000
random cases; and the run-list spellings with the previous scanning spellers
on 20,000 random code-length lists under all eight masks. The 89,255 replayed
header plans matched plan for plan. API comparisons on the 52 inputs, in
Default with the 180-second budget and in zero-budget Max, matched in all 104
runs, and every CLI timing run and both completing Max runs matched byte for
byte. Rust tests, including the private corpus, passed in release and debug
modes, with documentation and Python tests, a locked build, formatting,
all-feature Clippy with warnings denied and a package list without private
fixtures or scratch artifacts. `__text` grew from 844,288 to 845,520 bytes and
the shipped executable from 925,616 to 925,632; the final source builds the
tested executable byte for byte.

Measured and rejected in the same review:

- Building the seven distinct DeflOpt and order-key heap variants together,
  sharing their steps until their tie rules diverge: on 13,858 code-length
  histograms the variants first diverged at step 1.1 of 11.4 on average,
  leaving 51% of the separate work. A prototype took 0.157 against 0.175–0.200
  seconds on 55,432 code-length histograms (1.11–1.27×), a few percent of
  Default CPU at most, and 0.53 against 0.36 seconds on 4,000 payload
  histograms (0.68×).
- Floyd's bottom-up sift-down, which descends by the smaller child to a leaf
  and sifts back up: 0.203 instead of 0.175 seconds on the same histograms,
  and not exact. The heap is built in frequency order alone and the height
  and order keys break ties only as nodes are taken, so the heap need not be
  ordered by the full key a bottom-up descent assumes; the trees changed.
- Passing the block-plan key's extra-bit total to the dynamic planner, which
  sums it again on a store miss: about 0.75% of Default CPU, for a parameter
  threaded through the planner's callers.
- A larger code-length tree cache: 16,384 and 65,536 histogram entries would
  have cut its misses by 1.8% and 4.7%, for about 4 and 20 MiB more per
  thread.

Not tried: reusing a cut search across slide calls. Another 20% of searches
repeated a pair from an earlier slide, but the key would hash both blocks'
tokens and trees.

Artifacts: `work/efficiency-review-default-20261024/`.

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

## Reviews without a merged change

### Max header planning, `b190dfc`, 6 November 2026

Batches: sampled profiles of the ten Max inputs at 20 seconds (238,726 CPU
samples) and of `crossroads.png`, `star.png` and `basi4a16.png` completing
their searches (151,015 samples), the latter repeated with 29 header-planning
helpers kept out of line; instrumented probes of `star.png` and
`basi4a16.png` completing; replays of 11,343 captured swap searches and
275,887 captured header plans, two rounds alternating the builds. Every
candidate gave the same replay digests as the baseline.

On the completing runs, 61% of the samples were under `consider_rle`, almost
all from the greedy length-swap trials' header plans; on `crossroads.png`
they came through the source-max route's state queue. Its parts were the
shortest RLE under each feedback tree (about 20% of all samples; 12% at 20
seconds), the four heap variants' DeflOpt local-rewrite pre-pass (about 18%),
and code-length tree lookups. At 20 seconds tree building weighed more:
package-merge, the payload tree candidates and deft4j's code-length heap.

Measured and rejected:

- **The shortest RLE's symbol counts from its run tables, its tokens spelled
  only when needed.** Each run spelling kept its symbol counts, so a header
  plan found the shortest RLE's counts without spelling it and spelled the
  tokens only to copy them into the next feedback pass or compare them with
  its RLE. The swap replay retired 15.6% more instructions in 8.9% more
  cycles, and the header replay 5.4% more in 3.7% more. The probe showed why:
  of 19.7 million shortest RLEs on `star.png`, 58% (74% on `basi4a16.png`)
  were later copied into a pass, so most of them were walked twice, and the
  count tables cost work to build.
- **Each code length's run spellings looked up once per shortest RLE**, for
  its longest run, instead of per run: 2.7% more instructions and 5.5% more
  cycles on the swap replay, 0.8% and 0.9% on the header replay. The
  existing per-value lookup was already cheap and well predicted, and the
  extra pass over the runs cost more. A first version restarted the lookups
  whenever a build replaced spellings found earlier; when two lengths' costs
  share a table slot, which only test costs above 7 bits allow, it never
  ended, so a replacement falls back to the per-run lookup.

Probed and not pursued:

- **A memo of shortest RLEs across header plans.** Of the 19.7 million on
  `star.png`, 3.05 million repeated the runs and tree of an earlier plan on
  the same thread, but only 133,000 of those within the last 65,536, so a
  memo would need millions of entries.
- **Reusing the pre-pass's rewrites in the feedback passes.** A feedback
  pass's DeflOpt rewrite repeats one that the pre-pass stored for the same
  RLE and tree in 4% of passes on `star.png` and 15% on `basi4a16.png`:
  about 0.2–0.6% of the samples.
- **A larger swap-trial cost memo.** Repeated trials after eviction were about
  7% of `star.png`'s time in the `9a3f174` review and 0.3% of
  `basi4a16.png`'s, so a 64 MiB memo would trade memory on every Max thread
  for one file.

Artifacts: `work/efficiency-review-max11-20261106/`.
