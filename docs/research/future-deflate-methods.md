<!-- SPDX-License-Identifier: MIT -->

# Existing-stream Deflate optimization research

Status: **complete for currently known in-scope methods**.

This is a technical decision record for optimizing an existing Deflate stream
without finding new LZ77 matches. Detailed production behavior is catalogued in
[`routes-and-methods.md`](../routes-and-methods.md).

## Decision

The Zopfli fork-network review produced one retained method: independently
re-run Columbo's exact package-list builder with package-before-leaf equality,
then exact-price that equal-payload tree at full and restricted depths. No
other reviewed encoder exposes an untested, broadly useful method that
satisfies Columbo's scope and measured runtime policy. Reopen this audit only
when one of the following exists:

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
- Google Zopfli
  [`ccf9f05`](https://github.com/google/zopfli/commit/ccf9f0588d4a4509cb1040310ec122243e670ee6)
  and its public direct-fork index were verified 25 August 2026. The audit
  classified all 337 direct forks in newest order, cross-checked the first 100
  by stars, then inspected distinct compressor lineages rather than counting
  unchanged mirrors as separate implementations.
- Pinned distinct Zopfli lineages were KrzYmod
  [`11bd7f8`](https://github.com/MrKrzYch00/zopfli/commit/11bd7f84a6774c52d6b0f969c5c82ac67c9c52b9),
  FrKay
  [`9501b29`](https://github.com/frkay/zopfli/commit/9501b29d0dacc8b8efb18ae01733b2768dd40b62),
  QVXLabs
  [`d3ac16a`](https://github.com/QVXLabs/zopfli/commit/d3ac16a9013dd036f0dd3cb5220b54e94bde4fc9),
  fhanau
  [`3315c43`](https://github.com/fhanau/zopfli/commit/3315c43be52e189d947a645a5ecc790e320c35e3),
  Snesnopic
  [`11c3738`](https://github.com/Snesnopic/zopfli/commit/11c37384825e593bb1f6fc03b8f72b65c7029d0f),
  fonttools
  [`400a507`](https://github.com/fonttools/zopfli/commit/400a50779d7272b044f0ae339c5576c0e842d79e),
  MegaByte
  [`f72f44c`](https://github.com/MegaByte/zopfli/commit/f72f44ce0ed354cc7d00d00571c1664a1447d005),
  ImageOptim
  [`fe6d762`](https://github.com/ImageOptim/zopfli/commit/fe6d7627992573820152124e4af49453862f33af),
  and maintained Rust reimplementation zopfli-rs
  [`3b560cb`](https://github.com/zopfli-rs/zopfli/commit/3b560cb81539b9a1fbc05439a63624ecb3a01702).
- ECT [`ab3f68b`](https://github.com/fhanau/Efficient-Compression-Tool/commit/ab3f68bf4f0b1fdfb1646c6c70aa54a59d2515bb)
  verified 25 August 2026.
- libdeflate [`92e6a0d`](https://github.com/ebiggers/libdeflate/commit/92e6a0db9fa848d742f9eb286c92afc60f2c3dda)
  (v1.26 `HEAD`) verified 25 August 2026. Since `b122c8b`, only CI,
  release-note, and public-header files changed; compressor and decompressor
  sources did not.
- 7-Zip [`f9d78aff`](https://github.com/ip7z/7zip/commit/f9d78aff31a5f2521ae7ddbdc97c4a8855808959)
  (26.02 `HEAD`) verified 25 August 2026.
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
| Containers | PNG/APNG, GZIP, ZIP, and zlib reconstruction; metadata handling; duplicate-frame reuse; exact candidate comparison; smallest sufficient zlib CINFO derived from emitted distances | A format-specific diagnosed miss |
| Blocks | Stored/fixed/dynamic pricing; merge/group/split routes; cuts inside proven matches; alignment-aware boundary graph; adaptive split; one reseat; one forced-split escape | A reproducible miss requiring wider lookahead |
| Tokens | Length-258 handling; match-to-literal families; same-distance repacking; proven-submatch graph; bounded multi-match header-aware composition | A diagnosed joint-header miss outside the current beam |
| Data trees | DeflOpt, Defluff, deft4j, and Columbo builders; package-first and leaf-first exact package-merge ties; source-tree reuse; equal-frequency assignments; swaps; three pseudo-frequency methods; paired exact depth-10/depth-9 candidates; a completed-stream frontier across every feasible restricted maximum depth; an independent per-alphabet depth cross-product for Max terminal work; pair/quad Kraft moves; paired pricing | A concrete absent near-optimal tree shape |
| Dynamic header | All eight repeat-code masks; balanced and zero-continuation encodings; multiple inherited routes; exact shortest RLE for one fixed tree; exhaustive header pricing for Max bounded-depth terminal candidates; bounded feedback | A real header requiring a retained alternate RLE histogram |
| Runtime | Canonical plan and header caches; range and edge reuse; bounded fingerprints; parser/decode/emission hot paths | Measured duplicate work or a new profile hotspot |

Existing match intervals do not overlap. Under one fixed Huffman cost model,
their shortest paths factor into the per-match graphs already implemented.
Additional value requires joint header-aware selection across alternate paths,
which the bounded composition route supplies.

## zRecompress follow-up

zRecompress 2.12 was recovered from the
[SourceForge release](https://sourceforge.net/projects/nikkhokkho/files/zRecompress/)
and verified at SHA-256
`0096edf8cf130040275456f3c286d8ce11b68f946bda98e479f8c27445201b85`.
Its active Deflate path is an old 7-Zip encoder. Two concepts were evaluated
independently; no upstream implementation code was imported.

| Concept | Decision | Evidence |
| --- | --- | --- |
| Non-strict equal-height Huffman heap | Reject | The comparison selects the right child when frequency and height are equal, but also stops a repaired item above an equal child. A deterministic oracle confirmed that this asymmetry produces trees outside Columbo's existing families: 583/2,000 sampled 19-symbol histograms, 421/1,000 30-symbol histograms, and 98/100 286-symbol histograms were absent from the ordinary family set. Novelty did not become compression value. Naive central-planner placement produced 0 wins, 4 losses, and 21.8% more wall time across 228 PngSuite reference comparisons because it redirected later search. An isolated exact-priced terminal sibling then produced 0 wins and 0 losses across the same 228 comparisons; paired time was 44.321 versus 44.076 seconds, which is noise. Keep the rule out until a diagnosed miss identifies its exact tree. |
| Smallest zlib window advertisement | Retain as wrapper normalization | RFC 1950 defines CINFO as `log2(window size) - 8`. Columbo now derives it from the largest distance recorded when the final emitted bytes are reparsed for identity validation, recomputes FCHECK, and separately validates the source against its original advertised window. Boundaries from 256 bytes through 32 KiB are tested. A standalone native benchmark oracle independently parses every Deflate token in IDAT, fdAT, zTXt, compressed iTXt, iCCP, and top-level zlib output streams. It requires exact minimality only when Columbo saved file bytes or tied bytes while saving meaningful Deflate bits; copied originals are not submitted to that oracle. All PNG sources, references, and outputs still receive full `pngcheck -q` conformance validation. This normalization cannot itself reduce file bytes because the header remains two bytes. On `XYB.icc.zlib`, Default makes no file saving and therefore copies the original header unchanged; a 352-byte Max output that saves one byte advertises the exact 512-byte `18 d3` window required by its maximum distance of 292. |

## Retained size methods

| Method | Production bound | Evidence |
| --- | --- | --- |
| All-literals endpoint | Complete fixed/dynamic pricing for dense blocks through 80,000 decoded bytes, or blocks through 1,000,000 bytes with at most 256 matches; token allocation occurs only after a strict win | In a 34-file strict corpus, two outputs improved: 493,708 bytes and 7 bytes; Max found another 11 bytes on the larger result |
| Symmetric and paired balanced-tree moves | Pair/quad Kraft-preserving moves on both alphabets; retain at most four moves per side and price at most sixteen pairs; Default requires non-positive combined payload delta; Max permits +18 bits | Removes the former literal-only asymmetry while preserving exact complete-header acceptance; focused literal, distance, paired, and opportunity tests pass |
| Zopfli RLE-friendly pseudo-frequencies | One independently implemented paired Max candidate; no tree-family cross-product | Three of eleven Max files improved by 2, 13, and 9 bytes; other files tied; short no-gain cost was about 0.26 s/file |
| Package-first exact package merge | Re-run Columbo's existing package-list reconstruction with package-before-leaf equality. The full-depth shape is added only to the exhaustive tree family. Every restricted depth compares leaf-first and package-first shapes; the ordinary frontier prices all four paired-alphabet combinations, while Max deduplicates both families before its independent alphabet cross-product. The completed incumbent and exact header price remain authoritative | A deterministic oracle found distinct trees with identical weighted payload cost. Across 31 files and 62 Default/Max records, 18 records covering 12 files improved, saving 33 file bytes / 285 meaningful bits in aggregate with no growth. Default saved 24 bytes / 197 bits; Max saved 9 bytes / 88 bits. Total wall time was 377.75 → 378.76 s (+0.27%); Max was 346.38 → 347.03 s (+0.19%). The isolated full-depth addition uniquely saved another 4 Max bits |
| Reduced-depth payload trees | Max only; build exact Defluff literal/distance pairs at maximum depths 10 and 9; no cross-product; poll the route deadline; accept only an exactly priced complete payload and dynamic header | Focused depth-10 and depth-9 cases beat every ordinary depth-15 tree-family pairing by 26 and 11 bits. Max A/B improved three of six PNGs by 5, 1, and 3 bytes and a 48.8 KiB GZIP by 2 bytes; aggregate PNG wall time was 88.132 → 88.250 s |
| Bounded-depth terminal tree frontier | For every completed dynamic block, derive the minimum feasible maximum depth from `2^depth >= populated symbols`, then exact-price the deduplicated leaf-first and package-first raw-count trees for both alphabets and all four same-depth pairings at every ceiling through 14. Max deduplicates both tie families across every feasible depth for each alphabet and exact-prices their complete cross-product, allowing the two alphabet ceilings to differ. Pairs are visited by increasing separable payload lower bound and poll the hard stop. If the hard boundary is already reached, a one-block rescue prices only the largest transmitted dynamic block under 1 MiB compressed, 1 MiB decoded, and 128 blocks. The unrestricted 15-bit completed parent remains independent. There is no corpus-trained admission band | Against the pre-frontier binary, 55 of 176 PngSuite files became byte-smaller and eight more changed only by meaningful bits, saving 139 aggregate bytes with no growth; wall time was 27.70 → 27.82 s. Three unrelated medium GZIPs gained 3, 75, and 68 bytes (146 total). Earlier timed validation reproduced 5 bytes / 19 bits. The general Max extension later reproduced three more wins conservatively totaling 26 bytes / 208 bits, with no reproduced loss and neutral aggregate timed runtime |
| Compact payload-tree terminal floor | Terminal floor for completed compact Huffman streams: compare the raw bounded-depth frontier with independent Brotli fixed-point and classic Zopfli nearby-count seed families, the latter exact-priced at paired maximum depths 15 through 9. One bounded pair/quad tree-only closure prices shapes exposed by a winner and repeats the compact price once. At most eight blocks/4,096 tokens, 8 KiB compressed, and 128 KiB decoded; no token search or lineage displacement | Before the raw frontier was added, Default A/B across 176 PngSuite files selected smoothing in 36 of 166 Deflate streams and saved 300 aggregate bytes / 2,428 meaningful bits, with a 26-byte largest win. The classic Zopfli seeds uniquely added 4 bytes / 32 bits across four streams; the one-round balanced closure won 28 of 36 smoothed streams and added 19 bytes / 217 bits. Smoothing cost 51.633 ms across 165 eligible streams; closure cost 610.470 ms across its 36 inputs. Sample ZIP wins were 2 bytes and 9 bytes at complete-file level |
| Header-aware proven-spelling composition | Max compact M3 only; source parent ≤4,000 tokens/80,000 bytes; generated spelling ≤8,000 tokens; 2–128 matches; target ≤8 matches; ≤4 spellings/match; beam 16; ≤32 exact plans | 98 of 1,706 eligible scanned blocks improved locally; final A/B wins were 1 byte/6 bits and 2 bits at equal bytes |
| Adaptive closed-loop proven composition | Additive Max sibling of the completed M3 composition parent; one unsplit source block ≤1,024 decoded bytes; at least three match menus and 3–128 source matches; forward ranked prefixes plus a reverse repair sweep; rerank only after a strict exact win; at most two rounds and 24 shared complete-block prices | On 48 paired PNG/ZIP/GZIP files (96 Default/Max rows), all Default rows tied. The stable `imageworsener/p8tbg.png` result improved from 963 to 961 meaningful bits at 318 bytes across focused repeats and the final full run. Two large, ineligible Max streams differed at the wall-clock frontier; baseline repeats moved by 1,435 and 1,018 bits, with the final candidate respectively matching the repeated baseline and beating it by 529 bits. No eligible loss reproduced |
| Adjacent-boundary reseat | Max comparison floors of 2–8 Huffman blocks; ≤8,192 tokens/512 KiB; keep one strongest strict replacement | Repeatable final wins: 10 bytes/80 bits, 1 byte/8 bits, and 3 bits at equal bytes |
| Forced-split escape | Max only; 2–7 Huffman blocks; ≤8,192 tokens/512 KiB; force one well-separated runner-up adaptive basin, replan two children, then attempt one reseat | Reproducible 4-byte/25-bit final win; no displacement in the first wider PNG/APNG sample |

### Timed deft4j validation of the Max tree-frontier extension

The retained comparison froze commit `6246dc5` as binary
`09220ea9…` and compared it with candidate `eb182344…` through
`work/benchmark_deft4j_timed.py`. The thirteen cases span eight PNG/APNG
families, two ZIP families, two GZIP families, and one raw zlib family. The
runner used each fixture's timed deft4j allowance, reparsed every result, and
verified exact decoded identity.

| Reproduced case | File bytes | Meaningful Deflate bits | Isolated cause |
| --- | ---: | ---: | --- |
| `PngSuite/basi3p04.png` | -2 | -9 | exhaustive bounded-tree header, then independent alphabet depths |
| `apng-small/Animated.png` | -2 | -6 | independent alphabet depths |
| `8x8-zip/Checked.playdate-pulp.zip` | -1 | -4 in the final run; at least -2 across repeats | exhaustive bounded-tree header |

The full run reported one additional 1-byte/8-bit APNG change, but an immediate
A/B repeat tied exactly, so it is treated as deadline jitter rather than
evidence. The reproducible total is therefore 5 bytes / 19 bits across three
cases, with no regression among the thirteen. Variant isolation found no
timed output change from extending the existing balanced-tree closure, so that
experiment was removed. Retention rests on complete finite frontiers and exact
acceptance; the corpus supplies validation, not routing thresholds.

### General mixed-depth frontier and lower-bound validation

The follow-up froze commit `45571da` as binary `8318973a…` and compared it
with retained build `3af16f7a…` through the same validating timed-deft4j
harness on fifteen cases across PNG/APNG, ZIP, GZIP, and zlib. Literal and
distance payload contributions are computed once;
their sum is an admissible lower bound, so the planner may visit the smallest
bound first and skip a dynamic-header search only when payload alone cannot
strictly beat the complete incumbent.

| Reproduced case | Conservative file-byte gain | Conservative meaningful-bit gain |
| --- | ---: | ---: |
| `steam-shop_apng/bored.png` | 19 | 150 |
| `kensilverman-gz/pngout-20200115-linux.tar.gz` | 5 | 42 |
| `medium-gz/asyoulik-gzip.txt.gz` | 2 | 16 |

The conservative reproduced total is 26 bytes / 208 bits. The final paired run
observed three wins totaling 31 bytes / 254 bits and changed aggregate Max time
from 212.57 to 210.16 seconds. One ZIP run differed by 1 byte/1 bit, but immediate HEAD,
scoring-only, and final repeats all tied at the same output, so the isolated
row is recorded as deadline variance rather than a regression. Five Default
controls were byte/bit identical; two baseline totals were 11.13 and 10.58
seconds, and the retained build took 10.52 seconds. Exact frontier oracles,
deadline-prefix tests, wrapper reparsing, and decoded-identity checks remain
authoritative; measured files do not select depth, order, or eligibility.

## Retained execution optimizations

Percentages are isolated controls and do not add linearly to whole-file time.

| Optimization | Measured result | Validation |
| --- | ---: | --- |
| Safe scratch match expansion and sliced history-ring updates | 26.44 → 10.30 ms, about 61% | Oracle covers all match lengths, overlap periods, ring wrap, and sampled distances through 32,768 |
| Decode entries carry base value and extra-bit width | 10.88 → 10.18 ms, about 6.4% | Canonical fallback and generated-table comparisons |
| Literal/distance decode roots of 10/6 bits | The distance change from eight to six bits was about 1–1.5% faster on three real parsing controls; a weighted synthetic decode was about 3% faster | Root widths are one bit above `ceil(log2(max alphabet size))`: 10 for 288 literal/length symbols and 6 for 32 distance symbols. Generated-tree oracles cover root widths 7–11, and the retained distance width passed real-stream A/B |
| Single-level code-length decoder | Build was 64–67% faster and decode 53–54% faster in isolated release controls | Deflate's 19-symbol code-length alphabet has a format maximum of seven bits, so an independently built 128-byte table stores symbol and width directly. Canonical-table comparison, malformed-shape rejection, and physical-end fallback tests pass |
| Contiguous progressive root-table expansion | 24.01 → 18.67 ms for 20,000 deep-table builds, about 22%; about 0.3% on real parsing | Canonical table oracle |
| Generic Huffman construction | Variant zero about 34.5% faster; variant one about 45%; mixed-tie variants about 91% | Exact topology oracle across 19-, 30-, and 286-symbol alphabets and wrapped counts |
| Match codeword plus extra field in one write | 1.68 → 1.16 ms for 100,000 matches, about 31% | Byte-identical wrapper differentials |
| Exact planned writer capacity | About 3% faster 100,000-match control | Enforced planned bit limit; growable writers remain fallible |
| Aligned exact-source bit copying | 294.11 → 3.30 ms for 160 near-1 MiB copies, about 98.9% | Oracle covers every input/output residue and boundary/long lengths |
| Incremental parser model accounting | 252.15 → 244.03 ms synthetic, about 3.2%; real GZIP parse about 1.9% | Exact model limit succeeds; one byte less fails |
| Packed 64-bit decode entry | 1.120 → 1.086 s on a 109-dynamic-block parse, about 3.0% | Entry-size invariant and generated-tree decode oracle |
| Fixed 32-bit writer drain | Literal emission 62.24 → 52.42 ms, about 15.8%; match emission 147.33 → 115.72 ms, about 21.5% | Independent bit-vector oracle covers widths 0–32, alignment, direct bytes, drain boundaries, and partial tails in both writer modes |
| Route-local header kernel cache | Neutral short Max control: 3.99 → 4.01 s | Exact length/policy key, collision verification, 512-entry cap, independent payload pricing |
| Separable bounded-frontier payload scoring | Five Default controls were output-identical; two frozen-baseline totals were 11.13 and 10.58 s, versus 10.52 s retained | Literal and distance sums exactly reconstruct the prior payload score. Increasing-payload order is an admissible lower-bound traversal; only payloads already no better than a complete incumbent skip header construction |
| Brotli-shaped entropy-state boundary scout | A three-regime, 2,100-literal raw stream reached the same byte-identical 2,400-bit fixed point in one replay instead of three; six-run release medians were 617 → 180 ms, about 70.8%. Twelve representative PNG/GZIP/ZIP controls were output-identical; paired Max time ratios were 0.996–1.005 | Exhaustive small-path oracle; deterministic transition and 16-cut-cap tests; deadline/stored/small-input gates; strict exact-boundary improvement test; both end-to-end raw outputs independently decompressed to the original bytes |

## Rejected or deferred experiments

### Size and search

| Experiment | Result | Reconsider only if |
| --- | --- | --- |
| K-best code-length-RLE feedback | One synthetic header saved 1 bit; 20 Default and 8 Max real comparisons produced no final gain; Default sample 2.67 → 4.00 s; isolated cleanup 84 → 178 ms | A diagnosed real header requires a specific alternate histogram or a cheaper frontier representation exists |
| Central-planner broad reduced-depth cross-product | Depths 14–11 produced no unique wins in the original 162-file PNG probe when multiplied through the ordinary literal/distance family cross-product; that placement slowed the corpus by about 12% and a 48.8 KiB no-gain GZIP control from about 1.5 to 4.2 s | Keep the broad family multiplication rejected. Independently mixed raw-count alphabets now run only as an additive terminal frontier: tree families are fixed and deduplicated, payload bounds order/prune exact header work, deadline or explicit rescue bounds control work, and no intermediate feedback lineage is redirected |
| Token-band-selected depth inside the central planner | One count-selected depth found attractive local trees, but changing intermediate prices redirected later feedback: PngSuite included final regressions (for example 1,688 → 1,692 and 1,568 → 1,569 bytes) and runtime rose about 4.8% | Keep the completed parent independent. The retained terminal sibling evaluates every mathematically feasible restricted ceiling and accepts only an exact whole-stream win |
| Repeated post-smoothing balanced closure | Extending the retained one tree-only closure to four rounds added another 12 bytes / 106 bits on PngSuite, but raised isolated closure time from 0.619 to 1.550 s. A later deadline-aware Max-only fixed point produced no output change in the timed deft4j probe or isolated winning cases, while one isolated run was 0.43 s slower | A cheaper move frontier can recover the later rounds without repeating the complete bounded price |
| Post-smoothing proven-feedback replay | The full compact feedback closure added the same 19 bytes as the retained direct tree-only closure but only 194 bits, costing 3.068 s instead of 0.619 s. Forcing its Max composition cost 9.348 s and reduced the win count | A diagnosed smoothed tree requires an actual token rewrite that the direct balanced-tree closure cannot reach |
| Guetzli within-round priority refresh | Two additive variants preserved the retained root-ranked forward/reverse path and shared its 24-price cap. Stepwise frequency-state re-ranking changed 0/39 eligibility-heavy tiny-image outputs (339.66 → 340.26 s, +0.18%) and produced no eligible change across 48 mixed PNG/ZIP/GZIP files / 96 Default-Max rows. Re-ranking every next move from the preceding exact Huffman table also changed 0/39 tiny outputs (339.66 → 340.65 s, +0.29%). On the newest 20 rolling regression guards, the retained binary missed six historical floors in 32 attempts and the frequency candidate missed five in 31; the sole difference was a favorable `medium/parachute.png` deadline swing outside the ≤1,024-byte route. Both binaries passed the separate `Kiwi512`/`black817`/`FsqwhPuaIAIlojU` guards on their first attempts. Both variants were removed, and the rebuilt distribution binary matched the retained SHA-256 exactly | An eligible real stream must require a within-round order that neither the root-ranked prefixes, aggressive-endpoint repair, nor strict-win outer rerank reaches |
| Unused-symbol tree graft | Constructed win; 0 wins across 1,278 source dynamic blocks and 6 full Max routes; broad placement roughly doubled a short control | A diagnosed tree has the exact missing graft shape |
| Standalone second adaptive basin | 0 output changes across 10 priority-guard files | A new miss demonstrates a useful basin without the forced-split/reseat route |
| Complete frequency-planning cache | One reuse among hundreds of probes; no output change; hit-bearing Max case 18.4 → 19.0 s | Instrumentation finds a materially denser duplicate-work class |
| Merged-block source-tree warm start | 0 changes across 45 comparisons; 34-file sample about 0.86% slower | A concrete merged block benefits from a known neighbouring tree |
| Coarse transition scout | 0 changes across 16 Max comparisons; about 0.4% slower | A diagnosed transition is missed by adaptive exact histograms |
| Per-alphabet entropy-state scout | A focused 20,000-token oracle exposed a distance transition hidden from the joint seed windows and produced a strict exact-graph win. Across a family-spaced 34-file PNG/ZIP/GZIP A/B, all Default outputs and 33/34 Max outputs tied; one apparent 90-byte / 724-bit Max win did not reproduce. Four alternated fixed-12-second repeats overlapped (baseline 412,774–412,834 bytes; candidate 412,774–412,864), with both reaching the same best result. Aggregate Max time was 409.97 → 407.15 s, within deadline noise | A diagnosed real stream repeatedly wins from an independent literal/length or distance transition under a deterministic work budget rather than a wall-clock race |

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

## Zopfli fork-network pinned-source audit

The 25 August 2026 review began at Google Zopfli `ccf9f05`, classified all 337
entries in GitHub's public direct-fork index in newest order, cross-checked the
first 100 by stars, and inspected every distinct compressor lineage found
there. Recent zero-star forks were cloned separately so star ordering could not
hide a new implementation. Unchanged mirrors, packaging-only changes, and
CI-only changes are one classification, not independent algorithm audits.

| Lineage | Distinct work | Columbo disposition |
| --- | --- | --- |
| Google Zopfli | Repeat-code masks, RLE-friendly nearby-count smoothing, length-limited package merge, repeated optimal LZ parsing, and block splitting | Masks and independently written smoothing were already retained. The audit found one missing existing-token dimension: package merge selects the package on an equal weight where Columbo's Defluff family selected the leaf. Columbo now prices both equal-payload outcomes at full and restricted depths. Fresh matches and repeated optimal parsing remain out of scope |
| KrzYmod and FrKay | Special header spellings, Brotli-like count smoothing, reversed equal-count ordering, repeated split/recompression passes, slow fixed-tree trials, and compressor hot-path work | Exact shortest header RLE covers the special spellings; both smoothing families and ascending/descending equal-frequency assignments are retained; stored/fixed/dynamic blocks are exactly priced. The remaining ratio and speed changes operate inside fresh LZ parsing. Existing-token split work is already broader through cumulative histograms, exact boundary pricing, reseat, and forced-split escape |
| QVXLabs | Deterministic fixed-point parse costs, reusable longest-match caches, packed DP state, shared tree-encoding preprocessing, histogram-repeat shortcuts, and allocation reductions | Longest-match and DP changes require fresh parsing. Columbo already keeps one persistent token model, lazily materialized histograms, exact canonical/header caches, duplicate-tree removal, and shared route scratch. The source exposed no absent existing-token method after those mappings |
| fhanau and descendant JayXon | Non-recursive Boundary Package Merge, faster LZ cost models, greedy-stat reuse, and match-finder cleanup | The equal-weight package choice was tested and retained through Columbo's existing list builder. Its only remaining recursive walk is bounded by Deflate's maximum tree depth of 15 and was not a profile hotspot; replacing it with more indexing has no measured execution case. The other changes are fresh-LZ hot paths |
| Snesnopic and fonttools | Strict-alignment-safe wide loads in `GetMatch` | Fixes C undefined behavior and portability inside the fresh match finder. Columbo forbids unsafe Rust and uses checked byte slices; it neither has the unaligned-load defect nor performs fresh match discovery |
| MegaByte and ImageOptim | PNG filtering, palette/genetic transforms, iteration stagnation, per-block time scaling, deterministic sorting, and smaller estimation windows | Pixel/palette work is outside the Deflate-stream contract. Columbo already owns one file deadline, proportional per-stream slices, reclaim passes, and deterministic candidate ordering. Iteration and window changes are coupled to fresh match discovery |
| zopfli-rs and the earlier rjpower port | Rust translation, streaming writers, arenas/caches, fuzz parity, and container APIs | Useful engineering validation but no different existing-token compression algorithm. Columbo's safe parser/writer, persistent model, memory accounting, and wrapper support already cover the applicable dimensions |
| enh-google, XhmikosR, brk, mayhemheroes, skn123, ryantig, and other sampled mirrors | Upstream snapshots, build portability, CI, fuzzing workflows, or no unique compressor commit | No compression or execution candidate to test in Columbo |

All lineages were read for concepts and observable invariants only. The retained
implementation reuses Columbo's own package-list data structure and exact
header selector, with a new explicit equality policy and independent tests. No
fork source code, control flow, constants, or helper names were copied or
translated.

## 7-Zip pinned-source audit

The 25 August 2026 audit used 7-Zip 26.02 commit
[`f9d78aff`](https://github.com/ip7z/7zip/commit/f9d78aff31a5f2521ae7ddbdc97c4a8855808959).
The implementation was studied for algorithmic structure only; no 7-Zip code,
identifiers, constants, or control flow were copied or translated.

| 7-Zip area | Assessment | Columbo disposition |
| --- | --- | --- |
| [`DeflateEncoder.cpp`](https://github.com/ip7z/7zip/blob/f9d78aff31a5f2521ae7ddbdc97c4a8855808959/CPP/7zip/Compress/DeflateEncoder.cpp) bounded Huffman depth | Selects a reduced payload-tree depth from token-count bands | Retained as a more general additive frontier: prefix-code capacity derives the first feasible depth, every restricted ceiling through 14 is exactly priced, and the completed 15-bit parent remains independent. Max exhausts header encodings across every unique feasible literal/distance depth pairing, ordered by a mathematically admissible payload bound and constrained by deadline or explicit finalization work. Neither eligibility nor depth selection is trained on the validation corpus |
| [`HuffmanDecoder.h`](https://github.com/ip7z/7zip/blob/f9d78aff31a5f2521ae7ddbdc97c4a8855808959/CPP/7zip/Compress/HuffmanDecoder.h) and [`DeflateDecoder.cpp`](https://github.com/ip7z/7zip/blob/f9d78aff31a5f2521ae7ddbdc97c4a8855808959/CPP/7zip/Compress/DeflateDecoder.cpp) table shapes | Use a compact direct table for the seven-bit code-length alphabet and 10/6-bit payload roots | Retained with independently derived Rust layouts: a checked 128-byte code-length table plus general two-level payload tables. The 10/6 roots also follow the alphabet-capacity rule of one bit above the balanced width |
| Recursive block subdivision | Recursively considers midpoint subdivisions while pricing encoded blocks | Columbo already covers the existing-token portion more broadly with cumulative histograms, adaptive exact cuts, a residue-aware boundary graph, inside-match boundaries, reseat, and forced-split escape. 7-Zip's subdivision is coupled to its fresh match parser |
| Optimal parsing, match finder, and repeated price feedback | Finds new LZ77 matches and reparses under updated prices | Out of scope: Columbo may rewrite only source-proven matches and literals |
| [`HuffEnc.c`](https://github.com/ip7z/7zip/blob/f9d78aff31a5f2521ae7ddbdc97c4a8855808959/C/HuffEnc.c) low-count leaf buckets | Reduces sorting work for a different Huffman construction path | Not retained. Columbo's alphabets are at most 286 symbols, its generic builders already have exact topology oracles and optimized two-front/heap paths, and profiling did not identify leaf sorting as a remaining hotspot. Replacing the ordering would also create a new tree family rather than a topology-preserving speed change |
| Word-oriented bit I/O and architecture-specific checksum paths | Optimize a different buffered-I/O and platform model | Existing Columbo controls rejected a wider reader refill and wider writer drain. Intrinsics conflict with `forbid(unsafe_code)`, and no checksum hotspot was measured |

The new frontier has no empirical size gate. Compact streams receive it inside
their existing bounded tree-only finalizer; every other completed topology can
use the general pass while route time remains. The general pass polls the hard
deadline between blocks, performs linear reparse/emission work within the
parser's existing model limits, and replaces the incumbent only after exact
emitted-bit comparison.

## ECT pinned-source audit

The 25 August 2026 audit used ECT commit
[`ab3f68b`](https://github.com/fhanau/Efficient-Compression-Tool/commit/ab3f68bf4f0b1fdfb1646c6c70aa54a59d2515bb).
The table separates ideas that change an already-proven Deflate stream from
recompression and container transforms that would change Columbo's scope.

| ECT area | Assessment | Disposition |
| --- | --- | --- |
| [`GetAdvancedLengths`](https://github.com/fhanau/Efficient-Compression-Tool/blob/ab3f68bf4f0b1fdfb1646c6c70aa54a59d2515bb/src/zopfli/deflate.cpp#L681-L770) | Compares raw counts, classic Zopfli nearby-count smoothing, Brotli fixed-point smoothing, and reduced maximum depths | Raw depth 10/9 remains in Max. Both smoothing families are independently priced at every depth from 15 through 9 in the compact terminal floor. The later 7-Zip audit motivated moving raw restricted depths into an additive completed-stream frontier; that placement made depths previously lost to central feedback useful without a family cross-product |
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
| [Google Zopfli](https://github.com/google/zopfli) and fork network | Repeat-code masks; RLE-friendly pseudo-frequencies; equal-weight package-merge choice; alternate header/count/split policies and hot paths in KrzYmod, FrKay, fhanau/JayXon, MegaByte, ImageOptim, and maintained ports | Masks and smoothing were already covered. Package-before-leaf equality is newly retained through Columbo's independent package-list builder and exact full/restricted-depth pricing. Exact header RLE, equal-frequency assignments, stored/fixed/dynamic selection, deadlines, and boundary routes cover the other existing-token dimensions. Optimal parsing, match discovery, PNG pixel transforms, and repeated recompression remain out of scope |
| [QVXLabs Zopfli](https://github.com/QVXLabs/zopfli) | Fixed-point costs, reusable match caches, packed DP state, tree-preprocessing reuse, histogram shortcuts, and scaled iterations | Parsing work depends on fresh LZ77 matches. Applicable reuse is already covered by Columbo's persistent tokens, cached exact header kernels, canonical plan cache, deduplicated tree families, and scratch ownership |
| [libdeflate](https://github.com/ebiggers/libdeflate) | Match cache and minimum-cost parse, feedback passes, all-literals fallback, fixed-tree small-block pricing, fast tables/copies/bit I/O | All-literals and useful portable execution themes are implemented; remaining ratio methods require new matches; remaining hardware checksum/CPU dispatch work lacks a significant Columbo hotspot |
| [Google Brotli](https://github.com/google/brotli) | High-quality block splitting learns a bounded family of entropy-code states and assigns the complete symbol stream while charging state changes | The search shape is retained as a Max-only Deflate boundary scout over existing tokens. It contributes only bounded cut anchors; Columbo's exact residue-aware graph remains the acceptance authority. Brotli context maps, transforms, and new LZ parsing do not transfer to the existing-token contract |
| [Google Guetzli](https://github.com/google/guetzli) | Closed-loop search repeatedly ranks local JPEG changes under a current global model, measures complete encoded candidates, and revisits decisions after accepted progress | Only the feedback shape transfers. Columbo independently applies it to tiny existing-token spelling menus: exact-price forward prefixes, repair backwards from the aggressive endpoint, and rerank after a strict complete-block win. Perceptual scoring, pixel/coefficient transforms, and lossy JPEG decisions remain outside scope |
| [7-Zip](https://github.com/ip7z/7zip) | Token-count-selected payload-tree depth; compact code-length decoder; 10/6-bit payload decode roots; recursive splitting; priced optimal parsing and match finding | Restricted depths are generalized into a complete feasible terminal frontier; the compact decoder and 10/6 roots are retained independently. Existing Columbo boundaries cover the in-scope split dimension. Fresh match discovery and optimal LZ parsing remain outside scope |
| [AdvanceCOMP](https://github.com/amadvance/advancecomp) | Recompression | Outside existing-token scope |
| [ECT](https://github.com/fhanau/Efficient-Compression-Tool) | Modified zlib/Zopfli recompression; newer Brotli fixed-point count smoothing beside classic Zopfli smoothing; exact comparison across reduced maximum payload-tree depths; coarse-to-fine block splitting; short-match literal replacement | Both smoothing families are retained independently in one compact terminal tree floor, with the raw reduced-depth dimension covered by Max depth-10/depth-9 pairs and the complete feasible terminal frontier. Columbo already has broader header-RLE masks, match-to-literal feedback, adaptive/exact-histogram splitting, bounded reseat/escape routes, and parallel range pricing. Fresh match discovery, recompression, PNG pixel transforms, and JPEG work remain outside scope |
| Turtledeflate | Match enumeration, parse models, boundary refinement | Independent cumulative-histogram, adaptive-boundary, reseat, and forced-split concepts are implemented; match discovery remains excluded |

## Source map

| Source | Responsibility |
| --- | --- |
| [`header.rs`](../../src/deflate/header.rs) | Data-tree candidates, exact payload pricing, dynamic-header RLE, finished-tree moves |
| [`huffman.rs`](../../src/deflate/huffman.rs) | Length-limited builders, pseudo-frequencies, decode tables |
| [`search.rs`](../../src/deflate/search.rs) | Same-distance, match-family, proven-submatch, feedback, and composition searches |
| [`stream.rs`](../../src/deflate/stream.rs) | Grouping, splitting, entropy-state scout, boundary graph, reseat, forced split, range caches |
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

- 466 Rust tests and formatting passed on Apple Silicon;
- warning-free all-target Clippy passed after the retained change;
- focused oracles cover canonical equivalence and physical-end fallback for
  the compact code-length decoder, generated payload decode tables, the
  prefix-capacity depth bound, exact dynamic-header cost, and complete
  payload-tree validity, plus exact enumeration of the independent alphabet
  depth cross-product, payload-lower-bound ordering, deadline prefix retention,
  and rescue work limits;
- release A/B runs covered the 176-file PngSuite default corpus, three medium
  GZIPs, 804 KiB and 2.38 MiB GZIP controls, six Max PNGs, zlib controls, and
  compact ZIP samples, plus thirteen- and fifteen-case timed deft4j
  cross-format Max A/Bs. The bounded-depth frontier produced no reproduced
  byte regression;
- the Zopfli fork-network candidate used frozen baseline and candidate release
  binaries across 31 files from PNG, ZIP, and GZIP families. Its 62 paired
  Default/Max records contained 18 improvements and no output growth;
- the Guetzli-shaped closed-loop candidate used byte-identical final and
  benchmarked distribution binaries across 48 PNG/ZIP/GZIP files and 96 paired
  Default/Max rows. The only eligible output change was a repeatable 2-bit Max
  win. Two ineligible large-stream endpoint differences fell inside measured
  baseline deadline variance;
- two later within-round Guetzli feedback variants were removed after 39-file
  tiny-image probes produced no output change. The frequency-state variant also
  produced no eligible change in a 96-row mixed-format A/B. Newest-20 and
  three-file large-image regression-guard replays exposed no new attributable
  miss, and the restored optimized binary matched the retained binary hash;
- the wrapper smoke suite passed after bypassing two assertions that also fail
  on the frozen baseline: a stale literal `Replay 1/8` expectation versus the
  current computed replay limit, and a malformed-PNG exit-policy expectation;
- every emitted contender remains subject to Columbo's wrapper reparse and
  decoded-identity checks.

## Provenance and licensing

External projects identify techniques; retained implementations are written
from Columbo's data model, invariants, and exact-cost behavior.

- Zopfli-derived source attribution and Apache-2.0 provenance remain beside
  the independently written pseudo-frequency transform.
- The package-before-leaf equality policy was identified in the Zopfli audit,
  but no upstream package-merge implementation was copied or translated.
  Columbo refactors its pre-existing Defluff package list behind an explicit
  tie policy, starts from an independently produced valid fallback, deduplicates
  the result, and accepts it only through Columbo's exact complete-header and
  complete-stream comparisons.
- ECT source was not copied or translated. Its search exposed the general
  reduced-maximum-depth dimension and the value of comparing Zopfli's classic
  nearby-count smoother with Brotli's newer fixed-point smoother for Deflate.
  Columbo's raw frontiers, fixed-size transforms, existing exact package-merge
  builder, work policy, complete-header pricing, and regression fixtures are
  independently written. The transforms retain explicit Google Zopfli
  Apache-2.0 and Google Brotli MIT attribution beside their implementations.
- Guetzli source was not copied or translated. Its iterative local/global
  feedback shape motivated an independently written forward-prefix and reverse-
  repair search over Columbo's existing proven token spellings. Columbo's exact
  Huffman/header price, decoded-identity invariant, strict incumbent comparison,
  tiny-stream gate, and work caps remain authoritative; no perceptual or JPEG
  implementation transferred.
- 7-Zip source was not copied or translated. Its audit identified the useful
  bounded-depth and decoder-table dimensions. Columbo independently derives a
  full feasible depth frontier from prefix-code capacity, uses exact terminal
  acceptance rather than upstream token bands, and builds Rust-specific decode
  layouts with separate malformed-input and end-of-buffer handling.
- libdeflate source was not copied or translated. The source-bit residue path,
  parser accounting, 64-bit decode layout, fixed 32-bit writer drain, names,
  tests, and control flow are Columbo-specific.
- An exact-line and identifier audit found no copied libdeflate implementation
  text. No dependency or license file changed.
- No architecture-specific implementation was added. Hardware checksum paths
  remain excluded unless profiling proves a significant hot path on Apple
  Silicon or x86_64.
