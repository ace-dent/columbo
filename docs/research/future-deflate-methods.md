<!-- SPDX-License-Identifier: MIT -->

# Existing-stream Deflate optimization research

Status: **complete for currently known in-scope methods**.

This is a technical decision record for optimizing an existing Deflate stream
without finding new LZ77 matches. Detailed production behavior is catalogued in
[`routes-and-methods.md`](../routes-and-methods.md).

## Decision

No reviewed encoder exposes an untested, broadly useful method that satisfies
Columbo's scope and measured runtime policy. Reopen this audit only when one of
the following exists:

- a new portable upstream method that operates on existing tokens;
- a reproducible output miss that identifies a specific absent tree, header,
  token-spelling, or boundary state; or
- a profile showing materially duplicated work or a significant checksum,
  parser, or emission hot path.

## Audit basis

- Initial Columbo audit: `a95929541930d2f1d6ccacb57a42abe4b4fe0a80`,
  12 August 2026.
- Priority-one implementation baseline:
  `6045adefe671da49d962ffcaa2158f9f197426fe`, 15 August 2026.
- Zopfli pseudo-frequency source revalidated 15 August 2026.
- ECT [`ab3f68b`](https://github.com/fhanau/Efficient-Compression-Tool/commit/ab3f68bf4f0b1fdfb1646c6c70aa54a59d2515bb)
  verified 25 August 2026.
- libdeflate [`92e6a0d`](https://github.com/ebiggers/libdeflate/commit/92e6a0db9fa848d742f9eb286c92afc60f2c3dda)
  (v1.26 `HEAD`) verified 25 August 2026. Since `b122c8b`, only CI,
  release-note, and public-header files changed; compressor and decompressor
  sources did not.
- Turtledeflate classifications are pinned separately in
  [`turtledeflate-methods.md`](./turtledeflate-methods.md).

## Scope and invariants

Columbo may:

- reuse, remove, merge, split, or move Deflate block boundaries;
- choose stored, fixed, dynamic, or compatible exact-source representations;
- change Huffman trees and dynamic-header RLE;
- replace an existing match with literals or proven submatches entirely inside
  that match at its original distance; and
- compare coupled alternatives under exact complete-stream cost.

Columbo must not:

- search history for new matches or alternative distances;
- extend matches across unproven bytes;
- decode and recompress raw bytes with Zopfli, libdeflate, 7-Zip, ECT, or
  Turtledeflate;
- use randomized or perturbed LZ77 parsing;
- share one transmitted dynamic tree between distinct blocks; or
- emit reserved, oversubscribed, or incomplete-tree tricks in strict mode.

Every accepted rewrite retains a complete incumbent and compares output bytes
first, then meaningful Deflate bits. Padding-only changes do not win.

## Current coverage

| Area | Implemented coverage | Reopen condition |
| --- | --- | --- |
| Containers | PNG/APNG, GZIP, ZIP, and zlib reconstruction; metadata handling; duplicate-frame reuse; exact candidate comparison | A format-specific diagnosed miss |
| Blocks | Stored/fixed/dynamic pricing; merge/group/split routes; cuts inside proven matches; alignment-aware boundary graph; adaptive split; one reseat; one forced-split escape | A reproducible miss requiring wider lookahead |
| Tokens | Length-258 handling; match-to-literal families; same-distance repacking; proven-submatch graph; bounded multi-match header-aware composition | A diagnosed joint-header miss outside the current beam |
| Data trees | DeflOpt, Defluff, deft4j, and Columbo builders; source-tree reuse; equal-frequency assignments; swaps; three pseudo-frequency methods; paired exact depth-10/depth-9 candidates; pair/quad Kraft moves; paired pricing | A concrete absent near-optimal tree shape |
| Dynamic header | All eight repeat-code masks; balanced and zero-continuation encodings; multiple inherited routes; exact shortest RLE for one fixed tree; bounded feedback | A real header requiring a retained alternate RLE histogram |
| Runtime | Canonical plan and header caches; range and edge reuse; bounded fingerprints; parser/decode/emission hot paths | Measured duplicate work or a new profile hotspot |

Existing match intervals do not overlap. Under one fixed Huffman cost model,
their shortest paths factor into the per-match graphs already implemented.
Additional value requires joint header-aware selection across alternate paths,
which the bounded composition route supplies.

## Retained size methods

| Method | Production bound | Evidence |
| --- | --- | --- |
| All-literals endpoint | Complete fixed/dynamic pricing for dense blocks through 80,000 decoded bytes, or blocks through 1,000,000 bytes with at most 256 matches; token allocation occurs only after a strict win | In a 34-file strict corpus, two outputs improved: 493,708 bytes and 7 bytes; Max found another 11 bytes on the larger result |
| Symmetric and paired balanced-tree moves | Pair/quad Kraft-preserving moves on both alphabets; retain at most four moves per side and price at most sixteen pairs; Default requires non-positive combined payload delta; Max permits +18 bits | Removes the former literal-only asymmetry while preserving exact complete-header acceptance; focused literal, distance, paired, and opportunity tests pass |
| Zopfli RLE-friendly pseudo-frequencies | One independently implemented paired Max candidate; no tree-family cross-product | Three of eleven Max files improved by 2, 13, and 9 bytes; other files tied; short no-gain cost was about 0.26 s/file |
| Reduced-depth payload trees | Max only; build exact Defluff literal/distance pairs at maximum depths 10 and 9; no cross-product; poll the route deadline; accept only an exactly priced complete payload and dynamic header | Focused depth-10 and depth-9 cases beat every ordinary depth-15 tree-family pairing by 26 and 11 bits. Max A/B improved three of six PNGs by 5, 1, and 3 bytes and a 48.8 KiB GZIP by 2 bytes; aggregate PNG wall time was 88.132 → 88.250 s |
| Dual RLE-smoothed terminal tree frontier | Terminal floor for completed compact Huffman streams: build independent Brotli fixed-point and classic Zopfli nearby-count seed families, then exact-price paired maximum depths 15 through 9 against the original token counts. One bounded pair/quad tree-only closure prices shapes exposed by a winning smoother and resmooths once. At most eight blocks/4,096 tokens, 8 KiB compressed, and 128 KiB decoded; no token search or lineage displacement | Default A/B across 176 PngSuite files selected the family in 36 of 166 Deflate streams and saved 300 aggregate bytes / 2,428 meaningful bits, with a 26-byte largest win. Nine streams gained 17 bytes / 115 bits from reduced depths beyond depth 15; the classic Zopfli seeds uniquely added 4 bytes / 32 bits across four streams; and the one-round balanced closure won 28 of 36 smoothed streams and added 19 bytes / 217 bits. Smoothing cost 51.633 ms across 165 eligible streams; closure cost 610.470 ms across its 36 inputs. Sample ZIP wins were 2 bytes and 9 bytes at complete-file level |
| Header-aware proven-spelling composition | Max compact M3 only; source parent ≤4,000 tokens/80,000 bytes; generated spelling ≤8,000 tokens; 2–128 matches; target ≤8 matches; ≤4 spellings/match; beam 16; ≤32 exact plans | 98 of 1,706 eligible scanned blocks improved locally; final A/B wins were 1 byte/6 bits and 2 bits at equal bytes |
| Adjacent-boundary reseat | Max comparison floors of 2–8 Huffman blocks; ≤8,192 tokens/512 KiB; keep one strongest strict replacement | Repeatable final wins: 10 bytes/80 bits, 1 byte/8 bits, and 3 bits at equal bytes |
| Forced-split escape | Max only; 2–7 Huffman blocks; ≤8,192 tokens/512 KiB; force one well-separated runner-up adaptive basin, replan two children, then attempt one reseat | Reproducible 4-byte/25-bit final win; no displacement in the first wider PNG/APNG sample |

## Retained execution optimizations

Percentages are isolated controls and do not add linearly to whole-file time.

| Optimization | Measured result | Validation |
| --- | ---: | --- |
| Safe scratch match expansion and sliced history-ring updates | 26.44 → 10.30 ms, about 61% | Oracle covers all match lengths, overlap periods, ring wrap, and sampled distances through 32,768 |
| Decode entries carry base value and extra-bit width | 10.88 → 10.18 ms, about 6.4% | Canonical fallback and generated-table comparisons |
| Literal/distance decode roots of 10/8 bits | About 2.3% faster real-stream parsing than 9/9 | Generated trees across root widths 7–11 |
| Contiguous progressive root-table expansion | 24.01 → 18.67 ms for 20,000 deep-table builds, about 22%; about 0.3% on real parsing | Canonical table oracle |
| Generic Huffman construction | Variant zero about 34.5% faster; variant one about 45%; mixed-tie variants about 91% | Exact topology oracle across 19-, 30-, and 286-symbol alphabets and wrapped counts |
| Match codeword plus extra field in one write | 1.68 → 1.16 ms for 100,000 matches, about 31% | Byte-identical wrapper differentials |
| Exact planned writer capacity | About 3% faster 100,000-match control | Enforced planned bit limit; growable writers remain fallible |
| Aligned exact-source bit copying | 294.11 → 3.30 ms for 160 near-1 MiB copies, about 98.9% | Oracle covers every input/output residue and boundary/long lengths |
| Incremental parser model accounting | 252.15 → 244.03 ms synthetic, about 3.2%; real GZIP parse about 1.9% | Exact model limit succeeds; one byte less fails |
| Packed 64-bit decode entry | 1.120 → 1.086 s on a 109-dynamic-block parse, about 3.0% | Entry-size invariant and generated-tree decode oracle |
| Fixed 32-bit writer drain | Literal emission 62.24 → 52.42 ms, about 15.8%; match emission 147.33 → 115.72 ms, about 21.5% | Independent bit-vector oracle covers widths 0–32, alignment, direct bytes, drain boundaries, and partial tails in both writer modes |
| Route-local header kernel cache | Neutral short Max control: 3.99 → 4.01 s | Exact length/policy key, collision verification, 512-entry cap, independent payload pricing |

## Rejected or deferred experiments

### Size and search

| Experiment | Result | Reconsider only if |
| --- | --- | --- |
| K-best code-length-RLE feedback | One synthetic header saved 1 bit; 20 Default and 8 Max real comparisons produced no final gain; Default sample 2.67 → 4.00 s; isolated cleanup 84 → 178 ms | A diagnosed real header requires a specific alternate histogram or a cheaper frontier representation exists |
| Broad reduced-depth tree frontier | Depths 14–11 produced no unique wins in a 162-file PNG probe; multiplying reduced literal/distance trees through the existing family cross-product slowed that corpus by about 12% and a 48.8 KiB no-gain GZIP control from about 1.5 to 4.2 s | A diagnosed miss needs one of the omitted depths or a substantially cheaper joint frontier |
| Repeated post-smoothing balanced closure | Extending the retained one tree-only closure to four rounds added another 12 bytes / 106 bits on PngSuite, but raised isolated closure time from 0.619 to 1.550 s | A cheaper move frontier can recover the later rounds without repeating the complete bounded price |
| Post-smoothing proven-feedback replay | The full compact feedback closure added the same 19 bytes as the retained direct tree-only closure but only 194 bits, costing 3.068 s instead of 0.619 s. Forcing its Max composition cost 9.348 s and reduced the win count | A diagnosed smoothed tree requires an actual token rewrite that the direct balanced-tree closure cannot reach |
| Unused-symbol tree graft | Constructed win; 0 wins across 1,278 source dynamic blocks and 6 full Max routes; broad placement roughly doubled a short control | A diagnosed tree has the exact missing graft shape |
| Standalone second adaptive basin | 0 output changes across 10 priority-guard files | A new miss demonstrates a useful basin without the forced-split/reseat route |
| Complete frequency-planning cache | One reuse among hundreds of probes; no output change; hit-bearing Max case 18.4 → 19.0 s | Instrumentation finds a materially denser duplicate-work class |
| Merged-block source-tree warm start | 0 changes across 45 comparisons; 34-file sample about 0.86% slower | A concrete merged block benefits from a known neighbouring tree |
| Coarse transition scout | 0 changes across 16 Max comparisons; about 0.4% slower | A diagnosed transition is missed by adaptive exact histograms |

### Execution

| Experiment | Result | Decision |
| --- | --- | --- |
| 11-bit literal root | Faster synthetic deep-tree decode, but about 3× table-build cost and slower complete parsing | Keep 10 bits |
| Wide word-at-a-time reader refill | Correct but about 1% slower | Keep byte refill |
| Literal-run decode loops | Direct probe about 5.6% slower; prefetched variant about 5% slower | Persistent token/frequency materialization removes libdeflate's output-only advantage |
| Distance-one match fill | Isolated copy about 40% faster, but only 2,015/514,233 matches qualified and complete parsing slowed about 1.4% | Remove specialization |
| Huffman active-vector ownership transfer | Variants one/two slightly slower; variant three noise-level faster | Keep clone/locality behavior |
| Consecutive dynamic-tree cache | 0 identical adjacent tables across 460 dynamic blocks | No opportunity |
| Decoder allocation workspace | 1.129 → 1.127 s with slower runs in the sample | Neutral; remove ownership complexity |
| Progressive second-level table fill | 58.49 → 141.12 ms for 20,000 builds | Keep strided fill for small subtables |
| Precomputed complete length spellings | Unconditional match control about 3.6% faster; gated form about 0.8% faster and literal controls 1–4% slower; representative streams lacked long runs | Remove table and dispatch |
| Two-literal emission packing | Synthetic literals about 29% faster; all-match control about 2.8% slower; real streams neutral to about 1% faster; eligibility fallback slowed them 5–7% | Remove traversal complexity |
| Parser-owned match scratch | 59.20 → 59.04 ms | Noise; compiler already removes material initialization cost |
| Drain all 4–7 complete writer bytes | Literals 52.42 → 60.23 ms; matches 115.72 → 128.68 ms | Keep fixed four-byte drain |

Stored blocks already use an aligned input slice, bulk decoded-byte copy, and
bulk history update. Columbo must still materialize one literal token and its
frequency per byte; an output-only stored/literal shortcut is incompatible
with its persistent model.

## ECT pinned-source audit

The 25 August 2026 audit used ECT commit
[`ab3f68b`](https://github.com/fhanau/Efficient-Compression-Tool/commit/ab3f68bf4f0b1fdfb1646c6c70aa54a59d2515bb).
The table separates ideas that change an already-proven Deflate stream from
recompression and container transforms that would change Columbo's scope.

| ECT area | Assessment | Disposition |
| --- | --- | --- |
| [`GetAdvancedLengths`](https://github.com/fhanau/Efficient-Compression-Tool/blob/ab3f68bf4f0b1fdfb1646c6c70aa54a59d2515bb/src/zopfli/deflate.cpp#L681-L770) | Compares raw counts, classic Zopfli nearby-count smoothing, Brotli fixed-point smoothing, and reduced maximum depths | Raw depth 10/9 is retained in Max. Both smoothing families are independently priced at every depth from 15 through 9 in the bounded terminal frontier; keeping both produced unique wins. Depths 14–11 had no unique raw-count wins in the wider probe |
| Dynamic-header spelling | ECT tries repeat-code masks and a small collection of header encodings | Already covered by Columbo's all-mask grid, balanced/residual and zero-continuation spellings, exact shortest RLE, and feedback/pruned-header routes |
| `ReplaceBadCodes` | Replaces length-3 through length-7 matches with known literals under one priced tree, then rebuilds | Already covered more broadly by Columbo's exact match-to-literal families, all-literals endpoint, and header-aware/proven feedback |
| Block splitting | Coarse-to-fine 3/9 probes on a greedy LZ parse, entropy or exact-count pricing, and largest-block recursion | Existing-token portion is covered by cumulative histograms, adaptive exact pricing, internal-match cuts, boundary graph, reseat, and forced-split escape. ECT's pre-split greedy match discovery is out of scope |
| Cost-model mixing, optimal parsing, and match hashing | Repeated parsing with new LZ77 matches and perturbed/mixed cost models | Out of scope under the existing-token invariant |
| Per-block multithreading | Compresses independent split blocks concurrently | Columbo already parallelizes bounded route families, grouping-range pricing, APNG work, and ZIP members where memory/deadline policy permits; another overlapping arena was not justified |
| ARM/x86 checksum and inflate paths | Architecture-specific intrinsics, SIMD checksum code, and chunked zlib inflate | Columbo's safe-Rust slicing-by-eight CRC, batched Adler, packed decode tables, and match-copy paths have focused profiles. Hardware intrinsics conflict with the crate-wide unsafe-code prohibition and no checksum hotspot was measured |
| OptiPNG, Leanify, and mozjpeg routes | Pixel/filter reductions, archive cleanup, and JPEG transforms | Container-domain work outside Columbo's Deflate-only optimization contract; metadata stripping and lossless wrapper reconstruction already remain separate supported policies |

## Upstream assessment

| Project | Relevant method | Columbo disposition |
| --- | --- | --- |
| [Google Zopfli](https://github.com/google/zopfli) | All repeat-code masks; RLE-friendly pseudo-frequencies | Masks were already covered; pseudo-frequencies retained independently as one Max candidate. Optimal parsing, match discovery, and repeated block splitting are out of scope |
| [QVXLabs Zopfli](https://github.com/QVXLabs/zopfli) | Fixed-point costs, reusable match caches, scaled iterations | Depends on fresh LZ77 parsing; comparison-encoder work only |
| [libdeflate](https://github.com/ebiggers/libdeflate) | Match cache and minimum-cost parse, feedback passes, all-literals fallback, fixed-tree small-block pricing, fast tables/copies/bit I/O | All-literals and useful portable execution themes are implemented; remaining ratio methods require new matches; remaining hardware checksum/CPU dispatch work lacks a significant Columbo hotspot |
| [7-Zip](https://github.com/ip7z/7zip) and [AdvanceCOMP](https://github.com/amadvance/advancecomp) | Priced optimal parsing and recompression | Outside existing-token scope |
| [ECT](https://github.com/fhanau/Efficient-Compression-Tool) | Modified zlib/Zopfli recompression; newer Brotli fixed-point count smoothing beside classic Zopfli smoothing; exact comparison across reduced maximum payload-tree depths; coarse-to-fine block splitting; short-match literal replacement | Both smoothing families are retained independently as one bounded terminal tree frontier, and the raw reduced-depth dimension as paired depth-10/depth-9 Max candidates. Columbo already has broader header-RLE masks, match-to-literal feedback, adaptive/exact-histogram splitting, bounded reseat/escape routes, and parallel range pricing. Fresh match discovery, recompression, PNG pixel transforms, and JPEG work remain outside scope |
| Turtledeflate | Match enumeration, parse models, boundary refinement | Independent cumulative-histogram, adaptive-boundary, reseat, and forced-split concepts are implemented; match discovery remains excluded |

## Source map

| Source | Responsibility |
| --- | --- |
| [`header.rs`](../../src/deflate/header.rs) | Data-tree candidates, exact payload pricing, dynamic-header RLE, finished-tree moves |
| [`huffman.rs`](../../src/deflate/huffman.rs) | Length-limited builders, pseudo-frequencies, decode tables |
| [`search.rs`](../../src/deflate/search.rs) | Same-distance, match-family, proven-submatch, feedback, and composition searches |
| [`stream.rs`](../../src/deflate/stream.rs) | Grouping, splitting, boundary graph, reseat, forced split, range caches |
| [`block.rs`](../../src/deflate/block.rs) | Exact representation selection and canonical plan cache |
| [`bitstream.rs`](../../src/deflate/bitstream.rs) | Safe bit reader/writer and exact-source copying |
| [`parse.rs`](../../src/deflate/parse.rs) | Validating persistent token/plain model and resource accounting |

## Validation policy

For every experiment:

1. retain the original complete candidate;
2. compare output bytes, then meaningful Deflate bits;
3. reparse output and verify decoded size/content;
4. test strict and relaxed policies separately;
5. compare Default and Max with identical budgets;
6. record wall time, retained memory, opportunities, cache hits, and deadline
   completion; and
7. treat fixtures as read-only; use temporary copies for mutation.

Retained methods require focused oracles plus wrapper-level differentials.
Default methods require broad wins with negligible runtime regression. Rare
header or boundary wins remain Max-only.

Latest retained execution pass:

- 444 Rust tests and formatting passed on Apple Silicon;
- warning-free all-target Clippy passed after the retained change;
- focused exact-cost oracles cover both retained maximum depths and complete
  payload-tree validity; and
- release A/B runs covered the 176-file PngSuite default corpus, six Max PNGs,
  GZIP controls, zlib controls, and compact ZIP samples. The new terminal
  smoother won 36 PngSuite streams with no regressions; every emitted
  contender remains subject to Columbo's wrapper reparse and decoded-identity
  checks.

## Provenance and licensing

External projects identify techniques; retained implementations are written
from Columbo's data model, invariants, and exact-cost behavior.

- Zopfli-derived source attribution and Apache-2.0 provenance remain beside
  the independently written pseudo-frequency transform.
- ECT source was not copied or translated. Its search exposed the general
  reduced-maximum-depth dimension and the value of comparing Zopfli's classic
  nearby-count smoother with Brotli's newer fixed-point smoother for Deflate.
  Columbo's two-depth raw policy, fixed-size transforms, existing exact
  package-merge builder, paired terminal frontier, work gates, complete-header
  pricing, and regression fixtures are independently written. The transforms
  retain explicit Google Zopfli Apache-2.0 and Google Brotli MIT attribution
  beside their implementations.
- libdeflate source was not copied or translated. The source-bit residue path,
  parser accounting, 64-bit decode layout, fixed 32-bit writer drain, names,
  tests, and control flow are Columbo-specific.
- An exact-line and identifier audit found no copied libdeflate implementation
  text. No dependency or license file changed.
- No architecture-specific implementation was added. Hardware checksum paths
  remain excluded unless profiling proves a significant hot path on Apple
  Silicon or x86_64.
