<!-- SPDX-License-Identifier: MIT -->

# Existing-stream Deflate optimization research

Status: **upstream audit complete; diagnosed in-scope misses have reopened local research**.

This decision record covers optimizing an existing Deflate stream without
finding new LZ77 matches. [`routes-and-methods.md`](../routes-and-methods.md)
is the authority for current production behavior; its
[route-gate reference](../routes-and-methods.md#route-gate-reference) gives
each route's bounds.

Follow-up, 5 September 2026: [new byte-saving research](new-byte-saving-methods.md)
found reproducible gains on completed Default raw streams, reopening the
absent-tree/header-state conditions below. The resulting methods, each with its
validation report:

- R1 [permanent original-match proofs](permanent-match-proofs-validation.md);
- R2 [positive-payload swaps](payload-header-tradeoff-validation.md) and R3
  [advertised literal-span search](literal-span-validation.md);
- R4 [joint payload-tree/RLE search](joint-tree-rle-validation.md);
- R5 [joint removal of small payload symbol sets](symbol-set-validation.md);
- Max only: R6 [alphabet-support boundary pairs](alphabet-boundary-validation.md),
  R7 [joint code-length tree/RLE solver](code-length-tree-validation.md),
  R8 [three-symbol code-length rotations](code-length-rotation-validation.md),
  R9 [coupled literal/length and distance swaps](coupled-code-length-swaps-validation.md),
  R10 [header-directed certified match response](header-response-validation.md)
  and R11 [length-symbol exchange](length-symbol-exchange-validation.md).

Follow-up, 2–7 October 2026: a further seven-proposal memo was tested against
completed Columbo outputs. Its length-threshold cuts inside matches were
rejected, but measuring them exposed the
[fixed-tree boundary slide](boundary-slide-validation.md) (R1c). The other
proposals are under
[rejected or deferred experiments](#rejected-or-deferred-experiments); the memo
itself is not retained.

Follow-up, 14 October 2026: bit accounting of completed Default outputs found
headers at 0.47% of all bits and payload codes only 0.82% above the entropy of
each block's own counts, so further savings must change those counts. That
led to R15 [entropy cuts](entropy-cut-validation.md), which adds the block cuts
Default's bounded split search leaves out, and R16
[terminal merges](terminal-merge-validation.md), which removes boundaries that
cost more than they save once the finish settles. R15's fixed work budget
then proved too small for the largest streams and now
[scales with the stream](entropy-cut-validation.md#follow-up-work-that-scales-with-the-stream),
and a [header-aware fallback cut](entropy-cut-validation.md#follow-up-a-header-aware-fallback-cut)
now tries again where the entropy cut fails. A second Default pass showed the
main planner re-spelling R15/R16 blocks, which R17
[terminal token replay](token-replay-validation.md) now does in one pass. Lifting R1b–R5's 128 KiB Default
class in the same review saved 90 bytes on 33 files for 6.7% more CPU and grew
two outputs, so the class stays.

Follow-up, 23 October 2026: on 149 current Default PNG outputs (`0d87ee8`,
36.2 Mbit), headers take 0.71% of all bits, match extra bits 19.3%, and payload
codes sit 0.61% above the entropy of each block's own counts. Entropy-plus-header
estimates put further partition changes at 0.03–0.05% each (one more cut, two
cuts, or a merge), and a planner-priced respelling probe found 0.008%, so no
token, tree or partition method was taken forward; see
[rejected or deferred experiments](#rejected-or-deferred-experiments). Probe
and data: `work/new-method-probes-20261023/`.

## Decision

The Zopfli fork-network review produced one retained method: re-run Columbo's
own package-list builder with package-before-leaf equality, then exact-price
that equal-payload tree at full and restricted depths. The later zenflate and
zenzop reviews motivated R13 and R14 (see
[upstream assessment](#upstream-assessment)). No other reviewed encoder offers
an untested, broadly useful method within Columbo's scope and measured runtime
policy. Reopen this audit only when one of these exists:

- a new portable upstream method that operates on existing tokens;
- a reproducible output miss that identifies a specific absent tree, header,
  token-spelling, or boundary state; or
- a profile showing substantial duplicated work or a significant checksum,
  parser, or emission hot path.

## Audit basis

- Initial Columbo audit: `a95929541930d2f1d6ccacb57a42abe4b4fe0a80`,
  12 August 2026.
- Priority-one implementation baseline:
  `6045adefe671da49d962ffcaa2158f9f197426fe`, 15 August 2026.
- Google Zopfli
  [`ccf9f05`](https://github.com/google/zopfli/commit/ccf9f0588d4a4509cb1040310ec122243e670ee6)
  and its public direct-fork index were verified 25 August 2026 (method under
  [Zopfli fork-network audit](#zopfli-fork-network-pinned-source-audit)).
- Pinned distinct Zopfli lineages: KrzYmod
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
- A runtime follow-up on 2 September 2026 profiled a symbolized Max run,
  audited every production frequency recount, and rechecked the resulting
  deterministic and deadline-bounded paths against the prior binary.

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
| Blocks | Stored/fixed/dynamic pricing; merge/group/split routes; cuts inside proven matches; alignment-aware boundary graph; adaptive split; one reseat; one forced-split escape; Max alphabet-support boundary pairs (R6); terminal fixed-tree boundary slide to a fixed point (R1c); terminal recursive entropy cuts (R15); terminal merges after the settled finish (R16) | A reproducible miss requiring wider lookahead |
| Tokens | Length-258 handling; match-to-literal families; same-distance repacking; proven-submatch graph; bounded multi-match header-aware composition; terminal original-match restoration (R1); symbol-set removal (R5); Max certified match response and length-symbol exchange (R10, R11); distance-alphabet ladder (R13); terminal token-floor replay on the finished blocks (R17); length-family chains, re-spell from literals and the narrow block search on the source's own blocks (D4) | A diagnosed joint-header miss outside the current beam |
| Data trees | DeflOpt, Defluff, deft4j, and Columbo builders; package-first and leaf-first exact package-merge ties; source-tree reuse; equal-frequency assignments; swaps; three pseudo-frequency methods; paired exact depth-10/depth-9 candidates; a completed-stream frontier across every feasible restricted maximum depth; an independent per-alphabet depth cross-product for Max terminal work; pair/quad Kraft moves; paired pricing; strict distance completion (R1b); terminal positive-payload length swaps (R2); Max three-symbol rotations and coupled LL/DD swaps (R8, R9); smoothed trees for every completed block (R14) | A concrete absent near-optimal tree shape |
| Dynamic header | All eight repeat-code masks; balanced and zero-continuation encodings; multiple inherited routes; exact shortest RLE for one fixed tree; exhaustive header pricing for Max bounded-depth terminal candidates; bounded feedback; terminal advertised literal-span search (R3); joint payload-tree/RLE DP under one fixed CL tree at depth ≤9 (R4); bounded Max joint CL-tree/RLE search for fixed data trees and spans (R7) | A real header outside the completed solver domain, or a diagnosed budget cutoff |
| Runtime | Canonical plan and header caches; range and edge reuse; exact-verified bounded fingerprints; pointer-aware token adoption; fixed-size merge-histogram composition; parser/decode/emission and Huffman-heap hot paths | Measured duplicate work or a new profile hotspot |

A second 5-second profile after the runtime changes had no SipHash stacks and
only 47/3,732 top-of-stack samples in `count_frequencies`. The
DeflOpt-compatible heap, dynamic-header planning and tree-family candidates
dominated. The heap's avoidable writes were removed. The candidate breadth
stays, because each completed tree can give a different exact byte/bit result.

Existing match intervals do not overlap, so under one fixed Huffman cost model
their shortest paths factor into the existing per-match graphs. Further gains
need joint header-aware selection across alternate paths, which the bounded
composition route supplies.

## zRecompress follow-up

zRecompress 2.12 was recovered from the
[SourceForge release](https://sourceforge.net/projects/nikkhokkho/files/zRecompress/)
and verified at SHA-256
`0096edf8cf130040275456f3c286d8ce11b68f946bda98e479f8c27445201b85`.
Its active Deflate path is an old 7-Zip encoder. Two concepts were evaluated
independently; no upstream code was imported.

| Concept | Decision | Evidence |
| --- | --- | --- |
| Non-strict equal-height Huffman heap | Reject | On equal frequency and height the comparison picks the right child, but also stops a repaired item above an equal child; a deterministic oracle found trees absent from Columbo's families for 583/2,000 sampled 19-symbol, 421/1,000 30-symbol and 98/100 286-symbol histograms; central-planner placement: 0 wins, 4 losses and 21.8% more wall time over 228 PngSuite reference comparisons, by redirecting later search; isolated exact-priced terminal sibling: 0 wins, 0 losses on the same 228, paired time 44.321 versus 44.076 seconds (noise); keep out until a diagnosed miss identifies its exact tree |
| Smallest zlib window advertisement | Retain as wrapper normalization | RFC 1950: CINFO is `log2(window size) - 8`; Columbo derives it from the largest distance seen when reparsing the final emitted bytes for identity validation, recomputes FCHECK, and validates the source against its original advertised window; tested from 256 bytes through 32 KiB; a standalone native benchmark oracle parses every Deflate token in IDAT, fdAT, zTXt, compressed iTXt, iCCP and top-level zlib outputs, requiring exact minimality only when Columbo saved file bytes, or tied bytes while saving meaningful bits (copied originals are not submitted); all PNG sources, references and outputs pass full `pngcheck -q` validation; cannot itself save file bytes, as the header stays two bytes; `XYB.icc.zlib`: Default saves nothing and copies the original header; a 352-byte Max output that saves one byte advertises the exact 512-byte `18 d3` window its maximum distance of 292 needs |

## Retained size methods

| Method | Production bound | Evidence |
| --- | --- | --- |
| All-literals endpoint | Complete fixed/dynamic pricing for dense blocks through 80,000 decoded bytes, or blocks through 1,000,000 bytes with at most 256 matches; token allocation occurs only after a strict win | In a 34-file strict corpus, two outputs improved: 493,708 bytes and 7 bytes; Max found another 11 bytes on the larger result |
| Symmetric and paired balanced-tree moves | Pair/quad Kraft-preserving moves on both alphabets; retain at most four moves per side and price at most sixteen pairs; Default requires non-positive combined payload delta; Max permits +18 bits | Removes the former literal-only asymmetry while preserving exact complete-header acceptance; focused literal, distance, paired, and opportunity tests pass |
| Zopfli RLE-friendly pseudo-frequencies | One independently implemented paired Max candidate; no tree-family cross-product | Three of eleven Max files improved by 2, 13, and 9 bytes; other files tied; short no-gain cost was about 0.26 s/file |
| Package-first exact package merge | Re-run Columbo's package-list reconstruction with package-before-leaf equality; the full-depth shape joins only the exhaustive tree family; every restricted depth compares leaf-first and package-first shapes; the ordinary frontier prices all four paired-alphabet combinations; Max deduplicates both families before its independent alphabet cross-product; the completed incumbent and exact header price stay authoritative | Oracle: distinct trees with identical weighted payload cost; 31 files / 62 Default/Max records: 18 records covering 12 files improved, 33 file bytes / 285 meaningful bits in aggregate, no growth; Default 24 bytes / 197 bits; Max 9 bytes / 88 bits; total wall time 377.75 → 378.76 s (+0.27%); Max 346.38 → 347.03 s (+0.19%); the isolated full-depth addition uniquely saved another 4 Max bits |
| Reduced-depth payload trees | Max only; build exact Defluff literal/distance pairs at maximum depths 10 and 9; no cross-product; poll the route deadline; accept only an exactly priced complete payload and dynamic header | Focused depth-10 and depth-9 cases beat every ordinary depth-15 tree-family pairing by 26 and 11 bits; Max A/B: three of six PNGs improved by 5, 1 and 3 bytes, a 48.8 KiB GZIP by 2 bytes; aggregate PNG wall time 88.132 → 88.250 s |
| Bounded-depth terminal tree frontier | Every completed dynamic block; minimum feasible maximum depth from `2^depth >= populated symbols`; exact-price the deduplicated leaf-first and package-first raw-count trees for both alphabets and all four same-depth pairings at every ceiling through 14; Max deduplicates both tie families across every feasible depth per alphabet and exact-prices their complete cross-product, so the two alphabet ceilings may differ; pairs visited by increasing separable payload lower bound, polling the hard stop; if the hard boundary is already reached, a Max rescue prices every transmitted dynamic block of a parent with at most 1 MiB compressed, 1 MiB decoded and 128 blocks; the unrestricted 15-bit completed parent stays independent; no corpus-trained size band decides eligibility | Against the pre-frontier binary: 55 of 176 PngSuite files byte-smaller, eight more changed only in meaningful bits, 139 aggregate bytes saved, no growth; wall time 27.70 → 27.82 s; three unrelated medium GZIPs gained 3, 75 and 68 bytes (146 total); earlier timed validation reproduced 5 bytes / 19 bits; the general Max extension later reproduced three more wins, conservatively 26 bytes / 208 bits, with no reproduced loss and neutral aggregate timed runtime |
| Compact payload-tree terminal floor | Completed compact Huffman streams; compare the raw bounded-depth frontier with independent Brotli fixed-point and classic Zopfli nearby-count seed families, the latter exact-priced at paired maximum depths 15 through 9; one bounded pair/quad tree-only closure prices shapes exposed by a winner, then the compact price repeats once; at most eight blocks/4,096 tokens, 8 KiB compressed and 128 KiB decoded; no token search or lineage displacement | Before the raw frontier existed, Default A/B on 176 PngSuite files: smoothing selected in 36 of 166 Deflate streams, 300 aggregate bytes / 2,428 meaningful bits saved, largest win 26 bytes; classic Zopfli seeds uniquely added 4 bytes / 32 bits across four streams; the one-round balanced closure won 28 of 36 smoothed streams and added 19 bytes / 217 bits; smoothing cost 51.633 ms over 165 eligible streams, closure 610.470 ms over its 36 inputs; sample ZIP wins 2 bytes and 9 bytes at complete-file level |
| Header-aware proven-spelling composition | Max compact M3 only; source parent ≤4,000 tokens/80,000 bytes; generated spelling ≤8,000 tokens; 2–128 matches; target ≤8 matches; ≤4 spellings/match; beam 16; ≤32 exact plans | 98 of 1,706 eligible scanned blocks improved locally; final A/B wins were 1 byte/6 bits and 2 bits at equal bytes |
| Adaptive closed-loop proven composition | Additive Max sibling of the completed M3 composition parent; one unsplit source block ≤1,024 decoded bytes; at least three match menus and 3–128 source matches; forward ranked prefixes plus a reverse repair sweep; rerank only after a strict exact win; at most two rounds and 24 shared complete-block prices | 48 paired PNG/ZIP/GZIP files (96 Default/Max rows): all Default rows tied; `imageworsener/p8tbg.png` stably improved from 963 to 961 meaningful bits at 318 bytes across focused repeats and the final full run; two large, ineligible Max streams differed at the wall-clock frontier, where baseline repeats moved by 1,435 and 1,018 bits and the final candidate respectively matched the repeated baseline and beat it by 529 bits; no eligible loss reproduced |
| Adjacent-boundary reseat | Max comparison floors of 2–8 Huffman blocks; ≤8,192 tokens/512 KiB; keep one strongest strict replacement | Repeatable final wins: 10 bytes/80 bits, 1 byte/8 bits, and 3 bits at equal bytes |
| Forced-split escape | Max only; 2–7 Huffman blocks; ≤8,192 tokens/512 KiB; force one well-separated runner-up adaptive cut, replan two children, then attempt one reseat | Reproducible 4-byte/25-bit final win; no displacement in the first wider PNG/APNG sample |
| Fixed-tree boundary slide (R1c) | Terminal boundary step; Default: once after R5, again after each winning R13 or R14; Max: only after an R1–R11 sweep that changed nothing, or once no further sweep can start, with Max's slid Default endpoint kept only as a final competitor; no size or block-count class; at most 16 linear sweeps per round; stored boundaries fixed; every Huffman block keeps a token; a joining match costs the cheaper of its own spelling and its literals; moved blocks are re-planned, keeping the cheaper codes; repeats under new trees until no block moves, at most 16 rounds | Three paired A/B stages, each against its predecessor: 6,640 bytes on 518 Default PNGs, 2,928 on the large PNG group, 15,471 on 40 GZIP/ZIP files; Max: 932 bytes on 12 medium PNGs, 25 on 61 small multi-block PNGs; no output grew in any stage; every output decoded identically; per-stage CPU rose 0.3–1.2% on the 518-file set and up to 3.2% on the large group (about 1.8% for its largest file re-timed in isolation); an early placement after R1b grew two Max outputs by redirecting Max's terminal tree floors, which the final placement avoids; a fourth stage, sliding to a fixed point: 549 bytes on 1,052 Default PNGs, 3,145 on 202 container files, 210 on 381 relaxed inputs, each more than a slide-twice control, no output larger, CPU up 0.2–1.1%; in Max its only repeatable change was 525 bytes on one PNG; [validation](boundary-slide-validation.md) |
| Distance-alphabet ladder (R13) | Default: after R1c; Max: after a settled sweep; R1c runs once more after a win; after an R13 or R14 win, Default runs both one more pass and Max's sweeps repeat them without a pass bound while another sweep can start; Max's slid Default endpoint gets the Default finish; no size or block-count class; three ladders of kept distance-symbol sets, estimated from per-symbol histogram deltas; at most two rungs per block not above the parent's estimate, priced with the ordinary planner | Three-arm A/B: 476,350 bytes on 1,052 Default PNGs, 2,299 on 202 container files, 95,218 on 381 relaxed inputs, 1,327 in a 60-second Max sample; no output grew; six images hold almost all the PNG gain; a slide-twice control shows the ladder holds the PNG and relaxed gains, while most container and Max bytes came from repeating R1c; rebased onto R1c's fixed point: 475,932 PNG, 205 container and 95,050 relaxed bytes, no Default output larger; near neutral in Max, with one timing-sensitive loss; Default CPU rose 0.4–2.1%; placed before R1c it made five PNGs one byte larger; mid-sweep in Max it redirected R6–R9; [validation](distance-ladder-validation.md) |
| Smoothed-tree finish (R14) | Default: after R13; Max: after a settled sweep; R1c runs once more after a win; R13 and R14 repeat as for R13; Max's slid Default endpoint gets the Default finish; no size or block-count class; prices Columbo's existing Brotli fixed-point and Zopfli nearby-count smoothed trees at depths 15–9 for every completed block, adopting each strictly smaller plan | Four-arm A/B: 27,472 bytes on 262 of 1,052 Default PNGs, 20,301 on 89 of 202 container files, 141 on 365 raw streams, 7,139 on 381 relaxed inputs, no output larger; 60-second Max sample: 4,136 bytes on 19 of 25 PNGs, its one 1-byte loss not reproducing serially; Default CPU rose 0.6–2.7%; pricing the same families early, in the general floor, redirected R2–R5, R1c and R13 and grew six PNGs (with R14 as well: 5,145 more PNG bytes, still five grown); follow-up running R13 and R14 one more Default pass after either wins, with Max's sweeps repeating them without a pass bound: 4,297 more PNG bytes, 1,120 container bytes, 1,034 relaxed bytes, no output larger; an unbounded Default repetition: 4,844 PNG bytes, but CPU 4.5% above the first R14; [validation](smoothed-tree-finish-validation.md) |
| Entropy cuts (R15) | Default: after R14; Max: after a settled sweep; R1c runs once more after a win, and a winning pass repeats R13–R15 as for R13; no size or block-count class; non-stored blocks of ≥32 tokens; one exact linear fixed-point entropy scan per token range proposes the cut, with an entropy-plus-header-estimate cut as the fallback, the bounded planner prices it, and each kept half is cut again; per pass 64 work units per token and decoded byte (at least 2^28) and 4,096 range prices | Bit accounting of completed Default PNG outputs: headers 0.47% of bits, payload 0.82% above own-count entropy, so only new counts can save more; an entropy estimate put one cut per block at 0.053% of bits. Paired A/B against `2b40fb0`: 576 bytes on 12 of 365 raw streams, 153,436 on 115 of 1,052 Default PNGs, 62,572 on 65 of 202 container files, 6,226 on 21 of 381 relaxed inputs, no output larger; the Partnership card saved 51,560 bytes (1.9%) and every GZIP improved; CPU rose 1.6% on raw streams and 6.9–8.2% elsewhere, concentrated in large streams (Partnership card 28.2 → 35.7 s serially); 60-second Max sample: 7,671 bytes on 10 of 25 PNGs, none larger; [validation](entropy-cut-validation.md) |
| Terminal merges (R16) | Once the sweeps settle, in both modes and Max's slid Default endpoint; no size or block-count class; one left-to-right pass merges adjacent non-stored blocks when one bounded plan at the real alignment is strictly smaller than the pair as encoded, a merged block absorbing later neighbors too; a win slides and runs R15 once more; per pass 64 units of planned volume per token and decoded byte, at least 2^28 | After R15, an entropy estimate put merges at 0.061% of bits on 1,052 PNG outputs; merging before R15 or inside the repeated finish grew outputs by redirecting R13–R15, while merging after the settled finish cannot. Paired A/B against `3d3f198`: 1 byte on 1 of 365 raw streams, 35,117 on 91 of 1,052 Default PNGs (one 1-byte timing-dependent loss), 4,639 on 24 of 202 container files, 4,393 on 15 of 381 relaxed inputs; CPU rose 0.6–3.5%; 60-second Max sample: 6,091 bytes on 5 of 25 PNGs, four 1–6-byte losses that did not reproduce; [validation](terminal-merge-validation.md) |
| Terminal token replay (R17) | After R16 and its R15 follow-up, once the sweeps settle, in both modes and Max's slid Default endpoint; dynamic blocks with ≤50,000 tokens and ≤10 MB decoded; replays the Default extended token floor from each block's final codes at its real alignment, every block of ≥4,096 tokens and smaller ones only with a match dearer than its literals; a win slides once more | A second Default pass over R16 outputs saved 13,515 bytes on 153 files, much of it by re-spelling matches as literals under R15/R16 blocks' trees. Paired A/B against `d3550e1`: 20 bytes on 8 of 365 raw streams, 16,197 on 135 of 1,052 Default PNGs (`download_webp` 3,399, 1.6%), 2,686 on 77 of 202 container files, 10,527 on 21 of 381 relaxed inputs, no output larger; CPU +0.6–5.1%; the ungated floor cost 13–20% more CPU on APNG for little more; 60-second Max sample: 4,376 bytes on 11 of 25, two losses that did not reproduce; [validation](token-replay-validation.md) |
| Source family chains (D4) | Default, sources in M2's class decoding to ≥128 KiB, after the terminal tree floors; Max in its complete Default floor. Each nonempty source block keeps the smaller of its ordinary plan and a length-family chain: a prune chain from the block's own codes, then up to three expansions of the length symbol whose matches cost least as literals in total, each followed by a prune chain of ≤32 heap-code rebuilds and expansions of matches no cheaper than their literals. A candidate strictly smaller before the finish is finished beside the planner's endpoint and the smaller kept | In the 60-second Max sample Max beat Default by 75,648 bytes, mostly through M1 and M2; tracing M1's graph showed its wins begin with whole length-family expansions, which Default's match-by-match pruning cannot reach. Paired A/B against `8463a1d`: 58,615 bytes on 7 of 1,052 PNGs (`floor pattern.png` 22,139, 4.1%; `download_webp` 6,518, 3.0%), 57,059 on 4 of 381 relaxed inputs, raw streams and containers unchanged, no output larger; CPU +4.5% on PNG and +13.3% on relaxed inputs, about half of it on the winners; building D4 from M1's graph instead saved 66,314 PNG bytes for +24.7% CPU. 60-second Max sample, with D4 in Max's complete Default floor because Max at 20 seconds otherwise ended `floor pattern.png` above Default: 63 bytes saved on two files, two 3–6-byte losses that tied in reruns, every Max output no larger than Default; [validation](source-family-chain-validation.md) |
| Source-layout search (D4) | Extends D4: each source block's seed, the smaller of its ordinary plan and its length-family chain, gets the no-split route's narrow whole-block search, including the large-source length bands; no merges | After `5b16e33`, Max still beat Default by 18,754 bytes on the Max sample, most of it on `nerd.png`, where Max's no-split route alone reached 403,084 bytes against Default's 409,608. Paired A/B against `5b16e33`: 19,600 bytes on 11 of 1,052 PNGs (`nerd.png` 6,281, 1.5%), 17,241 on 7 of 381 relaxed inputs, raw streams and containers unchanged, no output larger; CPU +2.2% on PNG and +5.1% on relaxed inputs. 60-second Max sample: both `floor pattern` copies about 1,000 bytes smaller, two 2–5-byte losses that tied in reruns, every Max output no larger than Default, and Max's margin over Default down from 18,635 to 3,392 bytes; [validation](source-layout-search-validation.md) |
| Re-spell from literals (D4) | A third D4 seed per source block: from deft4j's heap codes for the block's bytes as literals, each pass keeps exactly the source matches cheaper than their literals under the current codes and rebuilds the codes, until the spelling repeats or after 32 passes; a dropped match can return | After `d4a6960`, Max beat Default by only 3,423 bytes on the Max sample and 1,480 on 40 containers, and fixed-code restoration had 37 bytes of headroom on 16 large outputs, so only a new spelling basin could still pay. Paired A/B against `d4a6960`: 18,968 bytes on 19 of 1,052 PNGs (three `frymire` copies about 2,815 each, 0.8%) with `floor pattern.png` 40 bytes larger, deterministically, 3,696 on 6 of 381 relaxed inputs with the same 41-byte loss, raw streams and containers unchanged; CPU +2.0% on PNG, +1.0% on containers; 60-second Max sample: 2,097 bytes saved, every Max output no larger than Default; [validation](literal-respell-validation.md) |
| Late tree floor (R18) | After R17 in Default and after Max's last sweep, and at the end of Max's mandatory Default endpoints: the terminal tree floors once more, compact smoothed families for ≤8 KiB compressed and ≤128 KiB decoded, and the bounded-depth frontier unless the compact pass covered it; keeps only a strictly smaller stream | A second Default pass over Default's output saved 1,424 bytes on 26 of 33 files, much of it in the bounded-depth tree floor, which runs before R13–R17 and D4 change the blocks and which APNG Default frames skip. Paired A/B against `4af8ff1`: 96 bytes on 18 of 365 raw streams, 2,768 on 174 of 1,052 PNGs, 2,091 on 71 of 202 container files (every `kzipmix` and `pngout` GZIP), 372 on 32 of 381 relaxed inputs, no output larger; CPU +0.1–1.8%; on 120 APNG-heavy PNGs 4,017 bytes on 43; 60-second Max sample: 244 bytes saved, two 1–2-byte losses, every Max output no larger than Default; [validation](late-tree-floor-validation.md) |
| APNG header fit | `ApngDefault` only, and the APNG Default endpoint of `ApngMax`: R2–R5 run once after R18 instead of in the terminal sweep, so R2–R4 fit the codes R14 and R18 choose and R5 compares its rewrites against that fitted parent; the floor phase of every other policy runs the terminal tree floors before the sweep, and those policies keep R2–R5 there; no size or block-count class | A second Default pass over `6c81dfa`'s output saved 7,325 bytes on 87 of 120 APNG-heavy PNGs, mostly in R4, because `ApngDefault` skips the first tree floors and fits headers to codes that R14 and R18 then replace. First form (`b831d49`, R2–R4 only), against `6c81dfa`: 1,321 bytes on 38 of 202 container files, all APNG, none larger; raw streams, PNG and relaxed inputs unchanged. With R5 moved too, on all 2,431 Steam stickers in Default against `6c81dfa`: 261,016 bytes / 2,089,046 bits on 2,338 files, 58 larger by 1,059 bytes in total, median time per file unchanged; against `b831d49` 67,715 bytes, 254 larger; Max no larger than Default; [validation](apng-header-fit-validation.md) |

### Timed deft4j validation of the Max tree-frontier extension

Baseline commit `6246dc5` (binary `09220ea9…`) against candidate
`eb182344…`, run through `work/benchmark_deft4j_timed.py`. The thirteen cases
span eight PNG/APNG families, two ZIP, two GZIP and one raw zlib family. Each
case used its timed deft4j allowance; every result was reparsed and checked for
exact decoded identity.

| Reproduced case | File bytes | Meaningful Deflate bits | Isolated cause |
| --- | ---: | ---: | --- |
| `PngSuite/basi3p04.png` | -2 | -9 | exhaustive bounded-tree header, then independent alphabet depths |
| `apng-small/Animated.png` | -2 | -6 | independent alphabet depths |
| `8x8-zip/Checked.playdate-pulp.zip` | -1 | -4 in the final run; at least -2 across repeats | exhaustive bounded-tree header |

One further 1-byte/8-bit APNG change tied exactly on an immediate A/B repeat
and is treated as deadline jitter. The reproducible total is 5 bytes / 19 bits
across three cases, with no regression among the thirteen. Extending the
existing balanced-tree closure changed no timed output and was removed.

### General mixed-depth frontier and lower-bound validation

Baseline commit `45571da` (binary `8318973a…`) against the retained build
`3af16f7a…`, run through the same timed-deft4j harness on fifteen PNG/APNG,
ZIP, GZIP and zlib cases.

| Reproduced case | Conservative file-byte gain | Conservative meaningful-bit gain |
| --- | ---: | ---: |
| `steam-shop_apng/bored.png` | 19 | 150 |
| `kensilverman-gz/pngout-20200115-linux.tar.gz` | 5 | 42 |
| `medium-gz/asyoulik-gzip.txt.gz` | 2 | 16 |

The conservative reproduced total is 26 bytes / 208 bits. The final paired run
saw three wins totaling 31 bytes / 254 bits, and aggregate Max time went from
212.57 to 210.16 seconds. One ZIP run differed by 1 byte/1 bit, but immediate
HEAD, scoring-only and final repeats all tied, so it is recorded as deadline
variance. The Default timing controls are under *Separable bounded-frontier
payload scoring* below.

## Retained execution optimizations

Percentages are isolated controls and do not add linearly to whole-file time.

| Optimization | Measured result | Validation |
| --- | ---: | --- |
| Safe scratch match expansion and sliced history-ring updates | 26.44 → 10.30 ms, about 61% | Oracle covers all match lengths, overlap periods, ring wrap, and sampled distances through 32,768 |
| Decode entries carry base value and extra-bit width | 10.88 → 10.18 ms, about 6.4% | Canonical fallback and generated-table comparisons |
| Literal/distance decode roots of 10/6 bits | Distance root from eight to six bits: about 1–1.5% faster on three real parsing controls; weighted synthetic decode about 3% faster | Root widths are one bit above `ceil(log2(max alphabet size))`: 10 for 288 literal/length symbols, 6 for 32 distance symbols; generated-tree oracles cover root widths 7–11; the retained distance width passed real-stream A/B |
| Single-level code-length decoder | Build 64–67% faster and decode 53–54% faster in isolated release controls | The 19-symbol code-length alphabet has a seven-bit format maximum, so an independently built 128-byte table stores symbol and width directly; canonical-table comparison, malformed-shape rejection and physical-end fallback tests pass |
| Contiguous progressive root-table expansion | 24.01 → 18.67 ms for 20,000 deep-table builds, about 22%; about 0.3% on real parsing | Canonical table oracle |
| Generic Huffman construction | Variant zero about 34.5% faster; variant one about 45%; mixed-tie variants about 91% | Exact topology oracle across 19-, 30-, and 286-symbol alphabets and wrapped counts |
| Route-local Huffman/header scratch reuse | Symbolized Mittens profile: top-stack allocator samples in the DeflOpt/order heap path 209 → 78; four warmed paired Default runs: 3.43 → 3.39 s user time, about 1.3%; elapsed about 2% lower, close to run noise; equal-time Max runs kept the same 78,808-byte / 630,459-bit winner and used the saved work inside the fixed deadline | Reused and standalone heap builders match across every variant and several alphabet sizes; header, short-family and full release tests preserve candidate order, exact bit accounting and output |
| Match codeword plus extra field in one write | 1.68 → 1.16 ms for 100,000 matches, about 31% | Byte-identical wrapper differentials |
| Exact planned writer capacity | About 3% faster 100,000-match control | Enforced planned bit limit; growable writers remain fallible |
| Aligned exact-source bit copying | 294.11 → 3.30 ms for 160 near-1 MiB copies, about 98.9% | Oracle covers every input/output residue and boundary/long lengths |
| Incremental parser model accounting | 252.15 → 244.03 ms synthetic, about 3.2%; real GZIP parse about 1.9% | Exact model limit succeeds; one byte less fails |
| Packed 64-bit decode entry | 1.120 → 1.086 s on a 109-dynamic-block parse, about 3.0% | Entry-size invariant and generated-tree decode oracle |
| Fixed 32-bit writer drain | Literal emission 62.24 → 52.42 ms, about 15.8%; match emission 147.33 → 115.72 ms, about 21.5% | Independent bit-vector oracle covers widths 0–32, alignment, direct bytes, drain boundaries, and partial tails in both writer modes |
| Route-local header-plan cache | Neutral short Max control: 3.99 → 4.01 s | Exact length/policy key, collision verification, 512-entry cap, independent payload pricing |
| Per-thread header-plan store | 33 Default inputs: CPU 180.2 → 130.8 s, about 27%; peak memory +3.8 MiB median, +20.0 MiB largest | Same exact key and collision verification behind the route-local cache; packed two-generation store within an 8 MiB charge, emptied when `optimize` returns; 104 API runs and the CLI timing runs byte-identical |
| Separable bounded-frontier payload scoring | Five Default controls output-identical; two frozen-baseline totals 11.13 and 10.58 s, versus 10.52 s retained | Literal and distance sums exactly reconstruct the prior payload score; increasing-payload order is an admissible lower-bound traversal; header construction is skipped only for payloads already no better than a complete incumbent |
| Brotli-shaped entropy-state boundary scout | Three-regime, 2,100-literal raw stream: same byte-identical 2,400-bit fixed point in one replay instead of three; six-run release medians 617 → 180 ms, about 70.8%; twelve representative PNG/GZIP/ZIP controls output-identical; paired Max time ratios 0.996–1.005 | Exhaustive small-path oracle; deterministic transition and 16-cut-cap tests; deadline/stored/small-input gates; strict exact-boundary improvement test; both end-to-end raw outputs independently decompressed to the original bytes |

## Rejected or deferred experiments

### Size and search

| Experiment | Result | Reconsider only if |
| --- | --- | --- |
| K-best code-length-RLE feedback | One synthetic header saved 1 bit; 20 Default and 8 Max real comparisons produced no final gain; Default sample 2.67 → 4.00 s; isolated cleanup 84 → 178 ms | A diagnosed real header requires a specific alternate histogram or a cheaper frontier representation exists |
| Central-planner broad reduced-depth cross-product | Depths 14–11 multiplied through the ordinary literal/distance family cross-product: no unique wins in the original 162-file PNG probe; corpus about 12% slower; a 48.8 KiB no-gain GZIP control from about 1.5 to 4.2 s | Keep the broad family multiplication rejected; mixed raw-count alphabets run only in the additive terminal frontier, with fixed deduplicated tree families, payload bounds ordering and pruning exact header work, deadline or explicit rescue work bounds, and no redirected feedback lineage |
| Token-band-selected depth inside the central planner | One count-selected depth found attractive local trees, but changed intermediate prices redirected later feedback; PngSuite final regressions (for example 1,688 → 1,692 and 1,568 → 1,569 bytes); runtime rose about 4.8% | Keep the completed parent independent; the retained terminal sibling evaluates every feasible restricted ceiling and accepts only an exact whole-stream win |
| Repeated post-smoothing balanced closure | Four rounds instead of the retained one: another 12 bytes / 106 bits on PngSuite, isolated closure time 0.619 → 1.550 s; a later deadline-aware Max-only fixed point: no output change in the timed deft4j probe or isolated winning cases, one isolated run 0.43 s slower | A cheaper move frontier can recover the later rounds without repeating the complete bounded price |
| Post-smoothing proven-feedback replay | Full compact feedback closure: the same 19 bytes as the retained direct tree-only closure but only 194 bits, at 3.068 s instead of 0.619 s; forcing its Max composition: 9.348 s and fewer wins | A diagnosed smoothed tree requires an actual token rewrite that the direct balanced-tree closure cannot reach |
| Guetzli within-round priority refresh | Two additive variants kept the retained root-ranked forward/reverse path and its 24-price cap; stepwise frequency-state re-ranking: 0/39 eligibility-heavy tiny-image outputs changed (339.66 → 340.26 s, +0.18%), no eligible change across 48 mixed PNG/ZIP/GZIP files / 96 Default-Max rows; re-ranking every next move from the preceding exact Huffman table: 0/39 tiny outputs changed (339.66 → 340.65 s, +0.29%); newest 20 rolling regression guards: the retained binary missed six historical floors in 32 attempts, the frequency candidate five in 31, the sole difference a favorable `medium/parachute.png` deadline swing outside the ≤1,024-byte route; both binaries passed the separate `Kiwi512`/`black817`/`FsqwhPuaIAIlojU` guards on first attempt; both variants removed; the rebuilt distribution binary matched the retained SHA-256 exactly | An eligible real stream must require a within-round order that neither the root-ranked prefixes, aggressive-endpoint repair, nor strict-win outer rerank reaches |
| Unused-symbol tree graft | Constructed win; 0 wins across 1,278 source dynamic blocks and 6 full Max routes; broad placement roughly doubled a short control | A diagnosed tree has the exact missing graft shape |
| Standalone secondary adaptive cut | 0 output changes across 10 priority-guard files | A new miss demonstrates a useful secondary cut without the forced-split/reseat route |
| Complete frequency-planning cache | One reuse among hundreds of probes; no output change; hit-bearing Max case 18.4 → 19.0 s | Instrumentation finds a substantially denser duplicate-work class |
| Merged-block source-tree warm start | 0 changes across 45 comparisons; 34-file sample about 0.86% slower | A concrete merged block benefits from a known neighboring tree |
| Coarse transition scout | 0 changes across 16 Max comparisons; about 0.4% slower | A diagnosed transition is missed by adaptive exact histograms |
| Length-threshold cuts inside matches | Structural claim reproduced: the cut family covers every canonical fragment-signature pair, at most 59 per length 3–258; fixed-tree scan of 518 completed Default PNG outputs: only 122 of 1,011 Huffman boundaries inside a match; inside-match positions added 12 bits beyond the best token cut, none on 283 Max streams, and 19 bits once joining matches could be respelled as literals | A diagnosed boundary needs an inside-match cut that a token cut with respelling cannot reach |
| Same-distance submatch respelling at a slid boundary | At R1c's fixed point, the cheapest literal-and-same-distance-match spelling of a joining match within 256 tokens of each boundary added 453 bits across 1,302 boundaries of 518 Default PNG outputs; token cuts and literal respelling had no remaining gain | A diagnosed boundary whose joining match is much cheaper as shorter same-distance matches than as literals |
| Partial stored-block absorption | Outputs almost never place a stored block beside a Huffman block: none in 511 Default PNG streams; one in 283 Max streams; one of 805 stored blocks across 532 GZIP/ZIP streams | Outputs commonly retain stored blocks next to Huffman blocks |
| Eliminating a short code-length value to shorten HCLEN | Code-length value 1 or 2 set the transmitted code-length count in 1,439 of 1,734 dynamic blocks; only 45 used that length rarely enough (at most 3 bits per saved entry) to pay for removing it; the bounded-depth frontier already removes values 15, 14 and 13; R1b prices degenerate distance completions | A diagnosed block whose rare short code sets HCLEN outside R1b's degenerate distance case |
| Literal evacuation using original match proofs | Deferred, not prototyped; 518 Default PNG outputs: 134 rare literals (at most four occurrences) entirely inside source match certificates, in 25 files; 35 between zero code lengths; a saving needs a forbidden-literal spelling plus tree and header rebuild, which a scan cannot price | A prototype combining R1's certificates with R5's symbol bans measures a material saving |
| Useful literal activation by leaf split | Not prototyped; a narrower form of the unused-symbol graft above, near the certified-response space R10 and R11 already search | A diagnosed block needs one specific absent literal to enable a cheaper certified spelling |
| Header run-statistics tree frontier | Not prototyped; its closest experiment, K-best code-length-RLE feedback, found no real gain; R4 and R7–R9 cover nearby joint tree/header states; signature growth is the central risk | The K-best reopen condition above is met, or a compact signature representation makes the frontier cheap |
| Per-alphabet entropy-state scout | A focused 20,000-token oracle exposed a distance transition hidden from the joint seed windows and gave a strict exact-graph win; family-spaced 34-file PNG/ZIP/GZIP A/B: all Default and 33/34 Max outputs tied; one apparent 90-byte / 724-bit Max win did not reproduce; four alternated fixed-12-second repeats overlapped (baseline 412,774–412,834 bytes; candidate 412,774–412,864), both reaching the same best result; aggregate Max time 409.97 → 407.15 s, within deadline noise | A diagnosed real stream repeatedly wins from an independent literal/length or distance transition under a deterministic work budget rather than a wall-clock race |
| R2–R5 rerun after R13–R16 (Default) | A second pass over R16 outputs found most of its gain in R2, R4 and R5 on APNG frames, since R15 and R16 change blocks after R2–R5 have run; rerunning R2–R5 last, only on blocks R13–R16 changed, saved 6,274 bytes on 70 of 120 random PNGs (mostly APNG) with no growth, but CPU rose 38%; R2–R4: 5,219 bytes, +23%; R2, R3, R5: 4,241 bytes, +22%; R2 and R3: 1,652 bytes, +7%; on a 33-file stratified sample all forms saved under 60 bytes | A cheaper header method closes most of the gap on the blocks R15 and R16 create; Max's R12 sweeps already rerun R2–R5 after a finish win |
| R15 before R2–R5 (Default) | One early R15 pass, with its slide, before R2: 7,195 bytes on 43 of 120 random PNGs but two outputs larger and CPU +25% from header work on more blocks; 3,302 bytes on the 33-file sample, 3,252 of them on the Partnership card through more finish passes | A large-stream case needs more finish passes than R13–R15's one repeat |
| Interval cuts in R15 | When no single cut paid, price the best pair of cuts on a 64-point grid: 131 bytes on 4 of 33 files, and the Partnership card 3,228 bytes larger as the extra scans spent the fixed per-stream budget; CPU +4% | A diagnosed block needs a middle segment that no single cut, recursive cut or R16 merge reaches |
| Re-planned boundary relocation | After R16, move each boundary between two non-stored neighbors to the pair's lowest-entropy cut, re-planning both trees: an entropy estimate put it at 0.043% of bits on 1,052 R16 PNG outputs, mostly in moves beyond R1c's reach, but exact pricing kept few: 234 bytes on 12 of 33 files (CPU +4.5%, the Partnership card +9 s) and 455 on 13 of 120 PNGs | A diagnosed boundary needs a long move that R1c, R15 and R16 cannot make together |
| Header-aware R15 cut as the first choice; ranked entropy fallbacks | Pricing the entropy-plus-header cut first instead of the entropy cut: 5,581 bytes on 33 files but three larger (`skinsrc.zip` +296) and 10 of 120 PNGs larger; up to two more entropy cuts an eighth of the range apart after a failed cut: 2,967 and 338 bytes for 6% more CPU. The header-aware cut is kept only as the fallback | A cheaper candidate order beats the retained fallback without growing outputs |
| Max outer replay after the terminal sweeps | A second 30-second Max pass over 40 small and medium PNGs' Max outputs saved only 354 bytes against the first pass's 12,024, since 39 of 40 first passes used their whole allowance; an outer replay loop would rarely get time | Max routinely reaches its fixed point well before its allowance |
| Late R4 on blocks R15 and R16 create | Rerunning the joint tree/RLE step only on blocks whose spans R15 or R16 created, the largest step in a second Default pass: 3,884 bytes on 120 random PNGs (mostly APNG) for 13.8% more CPU | A cheaper joint tree/RLE solver makes new blocks pay |
| D4 built by deft4j's graph (M1) | Default pricing M1's per-block graph and greedy merges, against `8463a1d`: 66,314 bytes on 21 of 1,052 PNGs and 60,893 on 6 of 381 relaxed inputs, none larger, but CPU +24.7% and +38%; containers unchanged for +3.5%. The graph costs about 8 µs per token, and its greedy merges re-plan an ever larger block (12 s of merges on `16million-pschmidt.png` for nothing). Wider forms cost more: on every source, replacing the candidate before the finish when smaller, 12,527 bytes on 33 files with `skinsrc.zip` 264 bytes larger and CPU +51%; within M2's class, finishing both endpoints, 13,252 bytes for +24.5% but +47.5% on 120 PNGs for 1,660 bytes; finishing only an already smaller M1, 12,791 bytes for +20%. Tracing the graph showed its wins come from whole length-family expansion and prune chains; the retained length-family chains keep 88% of its PNG bytes for a sixth of its added CPU | A cheaper graph build or merge pass |
| Length-family chain inside the planner | In every planner search instead of D4 only: 8,455 bytes on 33 files, but three larger (`y2rc2_large.png` +52, `Lemon512.png` +57, `ss1.png` +67) by redirecting later search, and CPU +19.6%; on 120 PNGs 66 bytes with eight larger for +8.3% | Wins concentrate where D4 already applies; reconsider for a diagnosed single-block miss |
| Length-family chain in R17 | From each finished block's final codes, before D4: 4,397 bytes on 5 of 33 files, none larger, CPU within noise, missing `floor pattern.png`. On top of `d4a6960`, where D4 already re-spells those streams: 63 bytes on 5 of 33 under R17's gate, 215 on 8 on every dynamic block for 3.5% more CPU | A finished block shows a whole-family miss D4 cannot reach |
| D4 chains without the size gate | On every Huffman source: 35 more bytes on 120 PNGs for 5.2% more CPU, and nothing more on 33 files for 2.7% | APNG-sized streams start to gain |
| No-split route merges in D4 | M2's whole route as D4's builder, with deft4j seeds and long-run merges: 26,099 bytes on 27 of 116 D4-eligible PNGs for +43% CPU on them; with chain seeds 25,282 bytes for +16%; without merges, the retained form, 19,610 bytes for +4.9%. The merges' 5,672 bytes cost 58 s, mostly on streams that do not improve (`FsqwhPuaIAIlojU.png` +8 s, each `16million-pschmidt.png` copy +5 s) | A cheaper long-run merge price |
| Restoring source matches beyond R1's limits | Headroom on 16 Default PNG outputs of 20–300 KB at `d4a6960`: source matches aligned with output token boundaries in one block that are cheaper than the output tokens under that block's final codes totaled 299 bits (37 bytes) | Restoration with tree changes, not under fixed codes |
| D4 without M2's class bounds | At least 128 KiB decoded and two fixed or dynamic blocks, with no compressed-size or block-count limit: no output changed on 10 large streams (three Partnership card copies, two `y2rc2_large.png` copies, five GZIP files) for 18% more CPU on them | A large stream shows a source-basin miss |
| Two-way re-spell from the source's codes | The retained re-spell started from the source's codes instead of literals: no change on its four probe winners except 3 bytes | A source whose codes are far from its counts |
| D4 for single-block or smaller sources | With all three seeds: single-block sources ≥128 KiB, nothing on 150 random PNGs and 17 bytes larger on 120 APNG-heavy PNGs (deadline-bound stickers); a 32 KiB decoded floor, nothing on 150 PNGs and 29 bytes on 120 for more CPU | A seed that pays on small blocks |
| D4 in the planner's layout | The source's tokens cut at the selected candidate's block boundaries, each range seeded and searched as in D4: always smaller than D4's candidate before the finish, but choosing it there grew `download_webp` by 244 bytes and `frymire.PNG` by 156; finishing both layouts saved 3,083 bytes on 17 of 116 D4-eligible PNGs, none larger, for 14% more CPU on them | A cheaper finish, or a pre-finish predictor of the better layout |
| A slide after R18 | Sliding once more after a late tree-floor win: net +1 byte on 120 PNGs | A tree change that moves the best boundaries |
| R2–R4 after R17 or R18 for every stream | Moving the header fit later for every policy, on 120 APNG-heavy PNGs against `6c81dfa`: R4 alone after R17 3,748 bytes, 10 larger; R2–R5 after R17 7,707 bytes for +18% CPU; R2–R4 after R17 5,411 bytes, 7 larger; R2–R4 after R18 6,358 bytes, but 22 of 1,052 PNGs and 21 of 202 containers grew by up to 11 bytes, all small static streams such as Playdate ZIP members, where the floor phase has already chosen the trees and the early fit gives R5 and R13–R15 a better parent | A static stream whose codes change after the sweep |
| Header fit both early and late for compact streams | Keeping the in-sweep fit for streams of ≤8 KiB compressed and ≤128 KiB decoded beside the late one: 6,580 bytes on 81 of 120 APNG-heavy PNGs, none larger, but +33% CPU, since most APNG frames are compact and ran R2–R4 twice; its size limits came from the compact tree floor, not from this effect. Repeating R18 and R2–R4 after a win recovered only part of the small-stream losses | A size class derived from where the early fit pays |
| R2–R4 after R18 with R5 left in the sweep (`b831d49`) | Under `ApngDefault`, R5 then ran before the fit, kept token rewrites that beat only the unfitted header, and left the late R4 less to gain: in the full sticker benchmark 103 Default rows grew by 1,315 bytes against `6c81dfa`; on all 2,431 stickers 216 files grew by 3,072 bytes, against 58 by 1,059 bytes once R5 moved with the fit, which saved 67,715 bytes more | None: R5 compares against its parent, so it follows the fit |
| R2–R4 again after a late R5 win (`ApngDefault`) | Refit the header after R5's late rewrite: never larger; on 178 APNG-heavy files 599 bytes on 60 (0.0025%), for 1.7% more median and 4% more total CPU; `apng-medium/ball.png` recovered 12 of the 78 bytes it lost when R5 moved | A cheaper refit, or a frame class where R5's rewrites are common |
| Fractional-cost respelling | Zopfli's statistical cost model: price each symbol at −log2 p of the block's current counts, the exact marginal of its entropy bound, instead of its integer code length; iterate four passes of a per-match shortest path over literals and same-distance submatches, recounting each pass, every candidate priced by the planner. On 1,042 Huffman blocks of 150 Default outputs at `0d87ee8` (31.95 Mbit): the integer-cost control found 1,609 bits, fractional costs 1,615, the better of the two 2,536 (0.008%); re-planning the unchanged tokens found nothing | A block class whose spelling needs counts far from its current ones, where the integer iteration stalls |

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
| Parser-owned match scratch | 59.20 → 59.04 ms | Noise; the compiler already removes any significant initialization cost |
| Drain all 4–7 complete writer bytes | Literals 52.42 → 60.23 ms; matches 115.72 → 128.68 ms | Keep fixed four-byte drain |
| deft4j keyed state hashing | SipHash took 404/3,687 sampled main-thread stacks (11.0%), though hashes only select an exact-verified state lookup; a compact incremental word fingerprint removed the hotspot; expanded-state fingerprints match a complete token scan; paired Max runs kept the same 78,808-byte / 630,459-bit winner | Retain the fast fingerprint and complete state verification |
| Merge/adoption frequency recounts | Block joins rescanned copied token buffers despite owning authoritative histograms; representation-only plan adoption recounted an unchanged shared token buffer; fixed-size checked histogram addition and pointer-aware adoption remove both scans; the complete 480-test release suite passed, including direct joined-histogram equivalence; a ten-case historical regression guard gave the same six deadline-floor misses, exact byte/bit deltas and 22 attempts for both candidate and saved baseline | Retain the derived-state reuse |
| DeflOpt/order heap swapping | The tree builder was the largest sampled CPU site and swapped the descending candidate at every heap level; an equivalent hole sift moves children upward and writes the candidate once; Huffman-family tests and paired normal/Max outputs unchanged; whole-file normal timing within noise (3.27–3.30 s candidate versus 3.28 s baseline user time) | Retain the lower-write sift; do not widen tree search from the saved time |

Stored blocks already use an aligned input slice, bulk decoded-byte copy and
bulk history update. Columbo still materializes one literal token and its
frequency per byte, so an output-only stored/literal shortcut does not fit its
persistent model.

## Zopfli fork-network pinned-source audit

The 25 August 2026 review started from Google Zopfli `ccf9f05`, classified all
337 entries in GitHub's public direct-fork index in newest order,
cross-checked the first 100 by stars, and inspected every distinct compressor
lineage. Recent zero-star forks were cloned separately so star ordering could
not hide a new implementation. Unchanged mirrors and packaging- or CI-only
changes count as one classification, not as separate algorithm audits.

| Lineage | Distinct work | Columbo disposition |
| --- | --- | --- |
| Google Zopfli | Repeat-code masks, RLE-friendly nearby-count smoothing, length-limited package merge, repeated optimal LZ parsing, and block splitting | Masks and independently written smoothing already retained; one missing existing-token dimension: on equal weight, package merge picks the package where Columbo's Defluff family picked the leaf, so Columbo now prices both equal-payload outcomes at full and restricted depths; fresh matches and repeated optimal parsing out of scope |
| KrzYmod and FrKay | Special header spellings, Brotli-like count smoothing, reversed equal-count ordering, repeated split/recompression passes, slow fixed-tree trials, and compressor hot-path work | Exact shortest header RLE covers the special spellings; both smoothing families and ascending/descending equal-frequency assignments retained; stored/fixed/dynamic blocks exactly priced; remaining ratio and speed changes are inside fresh LZ parsing; existing-token split work is already broader (cumulative histograms, exact boundary pricing, reseat, forced-split escape) |
| QVXLabs | Deterministic fixed-point parse costs, reusable longest-match caches, packed DP state, shared tree-encoding preprocessing, histogram-repeat shortcuts, and allocation reductions | Longest-match and DP changes require fresh parsing; Columbo already has one persistent token model, lazily materialized histograms, exact canonical/header caches, duplicate-tree removal and shared route scratch; no absent existing-token method found |
| fhanau and descendant JayXon | Non-recursive Boundary Package Merge, faster LZ cost models, greedy-stat reuse, and match-finder cleanup | Equal-weight package choice tested and retained through Columbo's list builder; its one remaining recursive walk is bounded by Deflate's maximum tree depth of 15 and is not a profile hotspot, so more indexing has no measured case; other changes are fresh-LZ hot paths |
| Snesnopic and fonttools | Strict-alignment-safe wide loads in `GetMatch` | Fixes C undefined behavior and portability in the fresh match finder; Columbo forbids unsafe Rust, uses checked byte slices, and does no fresh match discovery |
| MegaByte and ImageOptim | PNG filtering, palette/genetic transforms, iteration stagnation, per-block time scaling, deterministic sorting, and smaller estimation windows | Pixel/palette work is outside the Deflate-stream contract; Columbo already has one file deadline, proportional per-stream slices, reclaim passes and deterministic candidate order; iteration and window changes are tied to fresh match discovery |
| zopfli-rs and the earlier rjpower port | Rust translation, streaming writers, arenas/caches, fuzz parity, and container APIs | Useful engineering validation; no different existing-token algorithm; Columbo's safe parser/writer, persistent model, memory accounting and wrapper support cover the applicable dimensions |
| enh-google, XhmikosR, brk, mayhemheroes, skn123, ryantig, and other sampled mirrors | Upstream snapshots, build portability, CI, fuzzing workflows, or no unique compressor commit | No compression or execution candidate to test in Columbo |

## 7-Zip pinned-source audit

The 25 August 2026 audit used 7-Zip 26.02 commit
[`f9d78aff`](https://github.com/ip7z/7zip/commit/f9d78aff31a5f2521ae7ddbdc97c4a8855808959).

| 7-Zip area | Assessment | Columbo disposition |
| --- | --- | --- |
| [`DeflateEncoder.cpp`](https://github.com/ip7z/7zip/blob/f9d78aff31a5f2521ae7ddbdc97c4a8855808959/CPP/7zip/Compress/DeflateEncoder.cpp) bounded Huffman depth | Selects a reduced payload-tree depth from token-count bands | Retained as a more general additive frontier: prefix-code capacity gives the first feasible depth; every restricted ceiling through 14 exactly priced; the completed 15-bit parent stays independent; Max exhausts header encodings across every unique feasible literal/distance depth pairing, ordered by an admissible payload bound and limited by deadline or explicit finalization work; neither eligibility nor depth choice is trained on the validation corpus |
| [`HuffmanDecoder.h`](https://github.com/ip7z/7zip/blob/f9d78aff31a5f2521ae7ddbdc97c4a8855808959/CPP/7zip/Compress/HuffmanDecoder.h) and [`DeflateDecoder.cpp`](https://github.com/ip7z/7zip/blob/f9d78aff31a5f2521ae7ddbdc97c4a8855808959/CPP/7zip/Compress/DeflateDecoder.cpp) table shapes | Use a compact direct table for the seven-bit code-length alphabet and 10/6-bit payload roots | Retained with independently derived Rust layouts: a checked 128-byte code-length table plus general two-level payload tables; the 10/6 roots follow the rule of one bit above the balanced width |
| Recursive block subdivision | Recursively considers midpoint subdivisions while pricing encoded blocks | Existing-token portion already covered more broadly (cumulative histograms, adaptive exact cuts, residue-aware boundary graph, inside-match boundaries, reseat, forced-split escape); 7-Zip's subdivision is coupled to its fresh match parser |
| Optimal parsing, match finder, and repeated price feedback | Finds new LZ77 matches and reparses under updated prices | Out of scope: Columbo may rewrite only source-proven matches and literals |
| [`HuffEnc.c`](https://github.com/ip7z/7zip/blob/f9d78aff31a5f2521ae7ddbdc97c4a8855808959/C/HuffEnc.c) low-count leaf buckets | Reduces sorting work for a different Huffman construction path | Not retained: alphabets are at most 286 symbols; the generic builders already have exact topology oracles and optimized two-front/heap paths; profiling found no leaf-sorting hotspot; a new ordering would create a new tree family, not a topology-preserving speedup |
| Word-oriented bit I/O and architecture-specific checksum paths | Optimize a different buffered-I/O and platform model | Columbo controls rejected a wider reader refill and writer drain; intrinsics conflict with `forbid(unsafe_code)`; no checksum hotspot measured |

The resulting frontier has no empirical size gate and does linear
reparse/emission work within the parser's model limits. Its production
behavior is the *Bounded-depth terminal tree floor* in the
[method catalog](../routes-and-methods.md#block-huffman-and-token-methods).

## ECT pinned-source audit

The 25 August 2026 audit used ECT commit
[`ab3f68b`](https://github.com/fhanau/Efficient-Compression-Tool/commit/ab3f68bf4f0b1fdfb1646c6c70aa54a59d2515bb).

| ECT area | Assessment | Disposition |
| --- | --- | --- |
| [`GetAdvancedLengths`](https://github.com/fhanau/Efficient-Compression-Tool/blob/ab3f68bf4f0b1fdfb1646c6c70aa54a59d2515bb/src/zopfli/deflate.cpp#L681-L770) | Compares raw counts, classic Zopfli nearby-count smoothing, Brotli fixed-point smoothing, and reduced maximum depths | Raw depth 10/9 stays in Max; both smoothing families are priced at every depth from 15 through 9 in the compact terminal floor and, through R14, for every completed block; the later 7-Zip audit moved raw restricted depths into an additive completed-stream frontier, where depths lost to central feedback became useful without a family cross-product |
| Dynamic-header spelling | ECT tries repeat-code masks and a small collection of header encodings | Covered by Columbo's all-mask grid, balanced/residual and zero-continuation spellings, exact shortest RLE, and feedback/pruned-header routes |
| `ReplaceBadCodes` | Replaces length-3 through length-7 matches with known literals under one priced tree, then rebuilds | Covered more broadly by Columbo's exact match-to-literal families, all-literals endpoint, and header-aware/proven feedback |
| Block splitting | Coarse-to-fine 3/9 probes on a greedy LZ parse, entropy or exact-count pricing, and largest-block recursion | Existing-token portion covered by cumulative histograms, adaptive exact pricing, internal-match cuts, boundary graph, reseat and forced-split escape; ECT's pre-split greedy match discovery is out of scope |
| Cost-model mixing, optimal parsing, and match hashing | Repeated parsing with new LZ77 matches and perturbed/mixed cost models | Out of scope under the existing-token invariant |
| Per-block multithreading | Compresses independent split blocks concurrently | Columbo already parallelizes bounded route families, grouping-range pricing, APNG work and ZIP members where memory/deadline policy permits; another overlapping arena not justified |
| ARM/x86 checksum and inflate paths | Architecture-specific intrinsics, SIMD checksum code, and chunked zlib inflate | Columbo's safe-Rust slicing-by-eight CRC, batched Adler, packed decode tables and match-copy paths have focused profiles; hardware intrinsics conflict with the crate-wide unsafe-code prohibition; no checksum hotspot measured |
| OptiPNG, Leanify, and mozjpeg routes | Pixel/filter reductions, archive cleanup, and JPEG transforms | Container-domain work outside the Deflate-only contract; metadata stripping and lossless wrapper reconstruction stay separate supported policies |

## Upstream assessment

| Project | Relevant method | Columbo disposition |
| --- | --- | --- |
| [Google Zopfli](https://github.com/google/zopfli) and fork network | Repeat-code masks; RLE-friendly pseudo-frequencies; equal-weight package-merge choice; alternate header/count/split policies and hot paths in KrzYmod, FrKay, fhanau/JayXon, MegaByte, ImageOptim, and maintained ports | Masks and smoothing already covered; package-before-leaf equality retained through Columbo's independent package-list builder and exact full/restricted-depth pricing; exact header RLE, equal-frequency assignments, stored/fixed/dynamic selection, deadlines and boundary routes cover the other existing-token dimensions; optimal parsing, match discovery, PNG pixel transforms and repeated recompression out of scope |
| [QVXLabs Zopfli](https://github.com/QVXLabs/zopfli) | Fixed-point costs, reusable match caches, packed DP state, tree-preprocessing reuse, histogram shortcuts, and scaled iterations | Parsing work depends on fresh LZ77 matches; applicable reuse already covered by persistent tokens, cached exact header plans, the canonical plan cache, deduplicated tree families and scratch ownership |
| [libdeflate](https://github.com/ebiggers/libdeflate) | Match cache and minimum-cost parse, feedback passes, all-literals fallback, fixed-tree small-block pricing, fast tables/copies/bit I/O | All-literals and useful portable execution themes implemented; remaining ratio methods require new matches; remaining hardware checksum/CPU dispatch work lacks a significant Columbo hotspot |
| [Google Brotli](https://github.com/google/brotli) | High-quality block splitting learns a bounded family of entropy-code states and assigns the complete symbol stream while charging state changes | Retained as a Max-only Deflate boundary scout over existing tokens that contributes only bounded cut anchors; Columbo's exact residue-aware graph stays the acceptance authority; context maps, transforms and new LZ parsing do not transfer |
| [Google Guetzli](https://github.com/google/guetzli) | Closed-loop search repeatedly ranks local JPEG changes under a current global model, measures complete encoded candidates, and revisits decisions after accepted progress | Only the feedback shape transfers, applied independently to tiny existing-token spelling menus: exact-price forward prefixes, repair backwards from the aggressive endpoint, rerank after a strict complete-block win; perceptual scoring, pixel/coefficient transforms and lossy JPEG decisions out of scope |
| [7-Zip](https://github.com/ip7z/7zip) | Token-count-selected payload-tree depth; compact code-length decoder; 10/6-bit payload decode roots; recursive splitting; priced optimal parsing and match finding | Restricted depths generalized into a complete feasible terminal frontier; compact decoder and 10/6 roots retained independently; existing boundaries cover the in-scope split dimension; fresh match discovery and optimal LZ parsing out of scope |
| [AdvanceCOMP](https://github.com/amadvance/advancecomp) | Recompression | Outside existing-token scope |
| [ECT](https://github.com/fhanau/Efficient-Compression-Tool) | Modified zlib/Zopfli recompression; newer Brotli fixed-point count smoothing beside classic Zopfli smoothing; exact comparison across reduced maximum payload-tree depths; coarse-to-fine block splitting; short-match literal replacement | Both smoothing families retained independently in the compact terminal tree floor and, for every completed block, in R14; raw reduced depths covered by Max depth-10/depth-9 pairs and the complete feasible terminal frontier; Columbo already has broader header-RLE masks, match-to-literal feedback, adaptive/exact-histogram splitting, bounded reseat/escape routes and parallel range pricing; fresh match discovery, recompression, PNG pixel transforms and JPEG work out of scope |
| Turtledeflate | Match enumeration, parse models, boundary refinement | Independent cumulative-histogram, adaptive-boundary, reseat, and forced-split concepts are implemented; match discovery remains excluded |
| [zenflate](https://github.com/imazen/zenflate) [`f7708e5`](https://github.com/imazen/zenflate/commit/f7708e5) | Katajainen and heuristic length limits, Brotli/Zopfli count smoothing with a depth sweep, a precode flag search, exact block-type costs, Zopfli and libdeflate splitters, near-optimal and Zopfli-style parsing, and a PNG runs-only guard | Fixed-token methods already covered more broadly; the runs-only guard motivated R13, which can only remove matches; parsing and match finding excluded |
| [zenzop](https://github.com/imazen/zenzop) [`8e426b0`](https://github.com/imazen/zenzop/commit/8e426b0) | Faster Zopfli port; enhanced mode applies ECT's precode masks with two extra repeat-16 spellings, raw/Zopfli/Brotli-style smoothed trees with a depth sweep for every block, and a deterministic randomized parse | Precode spellings covered by the balanced repeat-16 seed and exact RLE searches; its "Brotli-inspired" smoother is a different heuristic worth 1,517 unique bits on 1,052 PNG outputs, so not added; its every-block tree comparison motivated R14, which applies Columbo's existing smoothed families beyond the compact class; parsing excluded |

## Source map

| Source | Responsibility |
| --- | --- |
| [`header.rs`](../../src/deflate/header.rs) | Data-tree candidates, exact payload pricing, dynamic-header RLE, finished-tree moves, positive-payload swaps (R2), literal-span search (R3) and smoothed-tree candidates (compact floor, R14) |
| [`header/tree.rs`](../../src/deflate/header/tree.rs) | Joint CL-tree/RLE optimization using run costs and complete Kraft capacity (R7) |
| [`header/rotate.rs`](../../src/deflate/header/rotate.rs) | Bounded three-symbol payload-length rotations with full header pricing (R8) |
| [`header/coupled.rs`](../../src/deflate/header/coupled.rs) | Bounded paired literal/length and distance swaps under the shared header price (R9) |
| [`header/response.rs`](../../src/deflate/header/response.rs) | Bounded Max tree proposals with exact certified token responses and complete header pricing (R10) |
| [`header/response/exchange.rs`](../../src/deflate/header/response/exchange.rs) | Bounded Max length-symbol reassignment with header-first pricing and certified payload bounds (R11) |
| [`huffman.rs`](../../src/deflate/huffman.rs) | Length-limited builders, pseudo-frequencies, decode tables |
| [`search.rs`](../../src/deflate/search.rs) | Same-distance, match-family, proven-submatch, feedback, and composition searches |
| [`restore.rs`](../../src/deflate/restore.rs) | Terminal restoration of original matches under unchanged trees (R1) |
| [`joint.rs`](../../src/deflate/joint.rs) | Joint payload code lengths and RLE under a fixed code-length tree (R4) |
| [`symbol_set.rs`](../../src/deflate/symbol_set.rs) | Joint removal of small payload symbol sets (R5) |
| [`stream.rs`](../../src/deflate/stream.rs) | Grouping, splitting, entropy-state scout, boundary graph, reseat, forced split, range caches |
| [`stream/alphabet.rs`](../../src/deflate/stream/alphabet.rs) | Alphabet-support boundary pairs (R6) |
| [`slide.rs`](../../src/deflate/slide.rs) | Terminal fixed-tree boundary slide, joining-match literal respelling and moved-block re-planning to a fixed point (R1c) |
| [`distance_band.rs`](../../src/deflate/distance_band.rs) | Distance-alphabet ladder (R13) |
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

Test records for recent Max methods follow; the R10 and R11 reports separate
frozen-parent gains from timed comparisons.

| Change | Tests passed | Default outputs | Other checks |
| --- | --- | --- | --- |
| [Length-symbol exchange](length-symbol-exchange-validation.md) (R11) | 583 Rust, 13 Python | all 404 preserved | decoded identity, Defluff |
| [Header-directed response](header-response-validation.md) (R10) | 577 Rust, 13 Python | all 404 preserved | decoded identity, Defluff |
| [Coupled code-length swaps](coupled-code-length-swaps-validation.md) (R9) | 548 | all 404 preserved | strict/relaxed Max, Defluff |

Earlier execution and profiling validation:

- 466 Rust tests and formatting passed on Apple Silicon, and all-target Clippy
  passed without warnings after the retained change;
- focused oracles cover canonical equivalence and physical-end fallback for
  the compact code-length decoder, generated payload decode tables, the
  prefix-capacity depth bound, exact dynamic-header cost, complete
  payload-tree validity, exact enumeration of the independent alphabet depth
  cross-product, payload-lower-bound ordering, deadline prefix retention and
  rescue work limits;
- release A/B runs covered the 176-file PngSuite default corpus, three medium
  GZIPs, 804 KiB and 2.38 MiB GZIP controls, six Max PNGs, zlib controls,
  compact ZIP samples, and the thirteen- and fifteen-case timed deft4j
  cross-format Max A/Bs. The bounded-depth frontier produced no reproduced
  byte regression;
- the Guetzli-shaped closed-loop candidate was benchmarked with binaries
  byte-identical to the final distribution binary; and
- the wrapper smoke suite passed after bypassing two assertions that also fail
  on the frozen baseline: a stale literal `Replay 1/8` expectation versus the
  current computed replay limit, and a malformed-PNG exit-policy expectation.

## Provenance and licensing

External projects identify techniques; retained implementations are written
from Columbo's data model, invariants and exact-cost behavior.

- Google Zopfli Apache-2.0 and Google Brotli MIT attribution sit beside the
  independently written pseudo-frequency transforms.
- Zopfli forks were read for concepts and observable invariants only; no fork
  source code, control flow, constants or helper names were copied or
  translated. The package-before-leaf equality policy came from that audit;
  Columbo applies it as an explicit tie policy on its existing Defluff package
  list, deduplicates the result, and accepts it only through exact
  complete-header and complete-stream comparison.
- ECT source was not copied or translated. It exposed the reduced
  maximum-depth dimension and the value of comparing Zopfli's classic
  nearby-count smoother with Brotli's fixed-point smoother for Deflate.
  Columbo's raw frontiers, fixed-size transforms, package-merge builder, work
  policy, complete-header pricing and regression fixtures are independently
  written.
- Guetzli source was not copied or translated. Its iterative local/global
  feedback shape motivated an independently written forward-prefix and
  reverse-repair search over existing proven spellings; no perceptual or JPEG
  code transferred.
- 7-Zip code, identifiers, constants and control flow were not copied or
  translated. Its audit identified the bounded-depth and decoder-table
  dimensions. Columbo derives a full feasible depth frontier from prefix-code
  capacity, accepts by exact terminal comparison rather than upstream token
  bands, and builds its own Rust decode layouts with separate malformed-input
  and end-of-buffer handling.
- libdeflate source was not copied or translated; an exact-line and identifier
  audit found no copied implementation text. The source-bit residue path,
  parser accounting, 64-bit decode layout, fixed 32-bit writer drain, names,
  tests and control flow are Columbo-specific. No dependency or license file
  changed.
- No architecture-specific implementation was added. Hardware checksum paths
  stay excluded unless profiling proves a significant hot path on Apple
  Silicon or x86_64.
