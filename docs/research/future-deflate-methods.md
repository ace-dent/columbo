<!-- SPDX-License-Identifier: MIT -->

# Further existing-stream Deflate optimization research

This document identifies additional compression methods that remain in scope
after the current Columbo implementation. It is primarily a proposal; the
status column and implementation notes distinguish experiments that have since
entered the source.

The original Columbo source audit was pinned to commit
`a95929541930d2f1d6ccacb57a42abe4b4fe0a80` on 12 August 2026. The priority-one
area was revalidated and implemented against
`6045adefe671da49d962ffcaa2158f9f197426fe` on 15 August 2026. External projects
were initially checked on 12 August, and Zopfli's pseudo-frequency source was
revalidated on 15 August. Current-branch links are used where an upstream
project does not publish stable source snapshots, so their claims should be
rechecked before implementation. The libdeflate comparison was revalidated at
commit `b122c8be1d78b19f6d0a6efc5bb79bfcbb30dd51` on 20 August 2026. Its v1.26
head, commit `92e6a0db9fa848d742f9eb286c92afc60f2c3dda`, was checked on 22 August;
the intervening release and CI changes did not modify the Deflate compressor
or decompressor sources used by this review. The same upstream head was checked
again on 24 August before the fourth speed pass.

## Conclusion

There is no obvious broad default-mode technique left to import from a modern
Deflate encoder. Their largest gains come from finding new matches and
iteratively changing the LZ77 parse, which would turn Columbo into a
recompressor.

The remaining useful search space is mostly in coupling decisions that Columbo
currently optimizes separately:

1. the literal/length and distance data trees;
2. the data-tree code-length sequence and its RLE spelling;
3. that RLE spelling and its own nineteen-symbol Huffman tree; and
4. several locally valid proven-match rewrites whose combined frequency change
   can cross a dynamic-header discontinuity.

The first output-changing experiment selected was **symmetric distance-tree
and paired balanced-tree moves**. It is now implemented. Columbo applies its
bounded pair and quad Kraft-preserving search to both data alphabets, then
exactly prices a small paired cross-product. **K-best code-length-RLE
feedback** was the next broader search evaluated, but its bounded prototype was
not retained: one generated header improved by one bit, while the real-file
sample produced no observed gains and materially increased runtime. The
subsequent exact Zopfli RLE-friendly pseudo-frequency control produced distinct
complete-plan and real-file wins, so it is now retained as one paired Max
candidate.

Later evidence justified two coupled searches: a compact header-aware beam
across several proven match spellings, and boundary escape through one forced
runner-up split followed by one reseat. Both are now implemented behind tight
Max gates. Wider frequency-kernel caching, an unused-symbol tree graft, and a
standalone second split basin were measured and removed because they did not
repay their runtime or complexity.

| Priority | Experiment | Likely benefit | Initial runtime policy | Status |
| ---: | --- | ---: | --- | --- |
| 1 | Distance-side and paired balanced-tree moves | Tiny–small, broad | Bounded default; wider Max | Implemented |
| 2 | K-best code-length-RLE feedback | Small, broad | Zero-cost ties in default; bounded Max | Explored; not adopted |
| 3 | Exact Zopfli RLE-friendly pseudo-frequencies | Small, broad | One paired Max candidate | Implemented |
| 4 | Header-aware data-tree frontier and unused-symbol grafts | Small–medium on difficult headers | Max | Graft explored; not adopted |
| 5 | Header-aware proven-spelling composition | Medium on selected misses | Targeted Max | Implemented |
| 6 | Second split basin and one adjacent-boundary reseat | Medium on difficult files | Bounded Max | Reseat implemented; standalone second basin not adopted |
| 7 | Header-kernel and wider interval caching | Runtime saving | All applicable routes | Header implemented; frequency layer explored, not adopted |
| 8 | One forced-split escape | Rare but potentially substantial | Heavily gated Max | Implemented |

Every experiment must retain the original complete candidate and accept a
rewrite only when exact byte count, then meaningful Deflate bits, improves.

## Scope inventory

RFC 1951 leaves only a small number of ways to spell an already decoded byte
stream. Blocks may have arbitrary boundaries, their Huffman trees are
independent, LZ77 references may cross block boundaries, and a match may
overlap the bytes it is producing. Dynamic blocks transmit one combined
literal/length and distance code-length sequence, whose repeat codes may cross
the alphabet boundary. The relevant rules are in
[RFC 1951 sections 2 and 3.2.7](https://www.rfc-editor.org/rfc/rfc1951.html#section-3.2.7).

For Columbo, every in-scope improvement belongs to one of these areas:

| Area | Current coverage | Remaining plausible gap |
| --- | --- | --- |
| Wrapper and container bytes | PNG/APNG, GZIP, ZIP, and zlib reconstruction, metadata handling, duplicate-frame reuse, exact candidate comparison | Format-specific work only; no general Deflate method found |
| Block partition and representation | Stored/fixed/dynamic selection, merge/group/split routes, cuts inside proven matches, global alignment-aware boundary graph, adaptive split, one comparison-floor adjacent-boundary reseat, and one bounded forced-split escape | Revisit wider or repeated forced splitting only after another reproducible miss |
| Proven token spelling | Length-258 normalization/alias, match-to-literal families, same-distance repacking, per-match proven-submatch graph, combined rewrites, and bounded header-aware composition across several matches | Wider composition only after a diagnosed miss; the implemented compact beam covers the demonstrated gap |
| Literal/length and distance trees | Multiple DeflOpt, Defluff, deft4j, and Columbo builders; exact source-tree reuse/repack; equal-frequency assignments; greedy swaps; adjacency-aware pseudo-frequencies; symmetric pair/quad moves and a bounded paired cross-product | A near-optimal tree frontier only after a diagnosed miss; the bounded unused-symbol graft trial found no real-file win |
| Dynamic header | Eight repeat masks, balanced and zero-continuation packs, DeflOpt/deft4j/Defluff-derived routes, exact shortest RLE for one fixed code-length tree, four feedback passes | Retain several RLE alternatives because the cheapest parse under the current tree need not build the cheapest next tree |
| Search engineering | Route-local canonical block-plan and exact-length header-kernel caches, boundary range index, edge-kernel reuse, fingerprints and work/deadline gates | Share completed immutable interval kernels only after measurements expose enough duplicate cross-lineage work; the frequency-planning cache trial did not meet that threshold |

This inventory also explains why a new whole-block proven-match lattice is not,
by itself, a new method. Existing match intervals do not overlap, so under one
fixed Huffman cost model their local shortest paths factor into the per-match
graphs Columbo already solves. New value appears only when several alternate
local paths are retained and their *joint header effect* is priced.

## Representative encoder research

This is not a compression-ratio league table. The projects use different
corpora, runtime budgets, block policies, and release dates. The purpose is to
identify techniques, then classify whether they can operate on an existing
Deflate stream without discovering new matches.

### Google Zopfli and its maintained fork

Google Zopfli repeatedly runs an optimal LZ77 parse, updates symbol statistics,
and perturbs those statistics after convergence. Its block splitter searches
candidate positions and repeatedly selects another splittable range. Those
parse and match-finding routes are outside Columbo's scope
([`squeeze.c`](https://github.com/google/zopfli/blob/master/src/zopfli/squeeze.c),
[`blocksplitter.c`](https://github.com/google/zopfli/blob/master/src/zopfli/blocksplitter.c)).

Two Zopfli tree methods are directly relevant:

- it tries all eight enable/disable combinations for RLE symbols 16, 17, and
  18; Columbo already does this and substantially more; and
- `OptimizeHuffmanForRle` smooths true symbol counts into RLE-friendly
  pseudo-counts, builds alternate data trees, then compares complete
  tree-plus-payload cost against the true-count tree
  ([`deflate.c`](https://github.com/google/zopfli/blob/master/src/zopfli/deflate.c)).

Columbo's adjacency-aware logarithmic quantizer is a different algorithm. An
exact Zopfli pseudo-frequency candidate therefore remains a useful differential
experiment, but it should not be confused with a new Columbo invention.

Google archived its repository in October 2025. The recent
[QVXLabs fork](https://github.com/QVXLabs/zopfli) describes a fixed-point,
deterministic cost model, reusable match caches, and size-scaled iteration
counts. These are valuable encoder speed improvements, but the quality path
still depends on repeated fresh LZ77 parsing. The fork's performance and ratio
figures are upstream self-measurements, not independently verified results.

### libdeflate

libdeflate's higher compression levels cache matches, solve a minimum-cost path
over literal and match edges, build Huffman codes from the selected path, and
repeat with the new code lengths as costs. It keeps the best exact-cost pass,
can recover a better non-final path, separately considers all literals, and on
small blocks optimizes against the fixed tree
([`deflate_compress.c`](https://github.com/ebiggers/libdeflate/blob/master/lib/deflate_compress.c#L3115-L3318)).

The reusable lesson is not the match graph, which is out of scope. It is to
retain alternate states across feedback passes and judge them by exact final
cost. Columbo already follows this principle at route level; the proposed
k-best header feedback applies it to the remaining small header state space.

A later differential experiment applied libdeflate's explicit all-literals
fallback more directly. Columbo previously tested that endpoint only below a
12 KiB allocation-first gate. The retained implementation now builds literal
frequencies from decoded bytes, rejects distant candidates with one cheap legal
tree, completely prices fixed and dynamic representations, and allocates the
expanded token vector only after a strict win. Work remains bounded to dense
blocks through 80,000 decoded bytes or blocks through 1,000,000 bytes with at
most 256 source matches. In a 34-file size-spaced strict corpus it changed two
outputs with no regression: one PNG saved another 493,708 bytes and a second
saved 7 bytes against the preceding Columbo binary. The prescribed Max budget
improved the larger result by another 11 bytes over the new Default floor.

A second pass separated ratio ideas from Huffman-construction speed work:

- A merged-block warm start retained one neighbouring source table only when
  its known match-to-literal payload decisions exposed a positive estimate.
  Forty-five strict differential comparisons produced no byte or meaningful-
  bit change; the main 34-file sample was 0.86% slower, so the experiment was
  removed.
- A Max-only coarse transition scout used twelve independently selected token
  classes and adjacent 256-token windows to nominate one exactly priced split.
  Sixteen prescribed Max comparisons produced no output change and aggregate
  elapsed time increased by about 0.4%, so the scout was removed. Columbo's
  existing adaptive exact-histogram search remains the stronger boundary
  method.
- The retained speed change targets Columbo's generic Huffman builder rather
  than importing libdeflate's builder. Generic variants zero and one use one
  stable total order for both selected children, so a standard binary heap can
  reproduce their exact merge topology without rescanning every active node.
  The mixed-tie variants retain the original scan. A 768-case generated oracle
  matched the old scanning lengths exactly. On a 34-file strict differential,
  aggregate measured time fell from 97.06 to 95.32 seconds with no regression;
  one timed route completed an additional one-bit win. Three repeated controls
  improved from a 51.58-second median total to 49.85 seconds while preserving
  the same deterministic outputs, including that one-bit gain.

The retained heap code is an independently written Columbo optimization over
the project's existing tree topology. No libdeflate source expressions,
identifiers, data layouts, or control flow were reused.

A third pass evaluated the remaining execution-speed concepts one at a time:

- Safe scratch-buffer match expansion plus sliced ring updates reduced the
  median match-heavy parse microbenchmark from 26.44 to 10.30 milliseconds,
  about 61%. A bytewise oracle covered overlap periods, ring wraparound, every
  match length, and sampled distances through 32,768.
- A guarded word-at-a-time bit-buffer refill passed its aligned stored-block
  tests but was about 1% slower on the same parser control, so it was removed.
- Profiled payload decode entries now carry base values and extra-bit widths,
  consuming a match codeword and its extra field together. The 60-round parser
  median improved from 10.88 to 10.18 milliseconds, about 6.4%.
- Generic variant zero now sorts its leaves once and merges the leaf and branch
  fronts directly. Variant one retains the heap, and wrapped totals fall back
  to that heap. Mixed variant-zero/one construction improved from 16.27 to
  10.66 microseconds per tree, about 34.5%, while the scanning oracle and
  explicit wrapped-frequency cases remained identical.
- Final match emission combines each Huffman codeword and extra field in one
  buffered write. The 100,000-match emission median improved from 1.68 to 1.16
  milliseconds, about 31%.

These are isolated hot-path measurements and do not add linearly to complete
optimization time. GZIP, PNG, and zlib whole-file differentials produced
identical output at every retained step. The implementations were designed
from Columbo's model and behavioral requirements; they do not translate
libdeflate source expressions, identifiers, layouts, or control flow.

A fourth pass completed the remaining portable execution-speed experiments:

- Payload decode tables now use alphabet-specific root widths: ten bits for
  literal/length codes and eight for distance codes. Code-length codes already
  fall naturally to their seven-bit maximum. Against the previous nine/nine
  configuration, the combined setting improved the repeated real-stream parser
  median by about 2.3%. An eleven-bit literal/length root decoded a synthetic
  deep tree faster but tripled its table-build time and slowed complete parsing,
  so it was rejected.
- Root-table construction now enumerates Columbo's existing canonical
  length ranges once and expands completed prefixes with contiguous copies.
  The isolated deep-table build improved from 24.01 to 18.67 milliseconds for
  20,000 builds, about 22%, and the repeated real-stream parser control improved
  by about 0.3%. A first version that rescanned the full symbol array at every
  length was neutral to slower and was replaced.
- Two literal-run decoders were rejected. A direct-root probe repeated the next
  lookup when a literal preceded a match and slowed parsing about 5.6%; retaining
  a prefetched nonliteral still slowed parsing about 5%. Columbo's persistent
  token and frequency model does not benefit from libdeflate's output-only
  literal fast-loop shape.
- Planned emission now records its exact bit limit. Its preallocated writer
  enforces that limit and skips redundant capacity reservations, while ordinary
  growable writers retain fallible reservation. The 100,000-match control
  improved by about 3%.
- A distance-one fill reduced that isolated copy by roughly 40%, but only 2,015
  of 514,233 matches in the representative stream used distance one. Its extra
  branch slowed complete parsing about 1.4%, so the specialization was removed.
- Generic variant one now merges a sorted leaf front with reverse-ordered
  equal-frequency branch runs, improving its control from 417.82 to 227.93
  milliseconds for 20,000 trees, about 45%. Mixed-tie variants two and three
  maintain both exact total orders in paired heaps with lazy removal, reducing
  their combined 2,000-round control from 1.65 seconds to 141.57 milliseconds,
  about 91%. The scanning oracle now covers all four variants across 19-, 30-,
  and 286-symbol alphabets plus wrapped-frequency inputs.

The final 21-pair representative PNG workload took 13.246 seconds both before
and after these changes; CPU totals differed by about 0.2%, within run noise.
All eleven unique candidate outputs were byte-identical to the preceding
binary. Formatting, warning-free Clippy, and all 433 Rust tests passed. No
architecture-specific path was added.

This pass again uses libdeflate only to identify general optimization themes.
The retained Rust algorithms, state layouts, names, and control flow were
designed from Columbo's existing canonical tables, tie-order semantics, safety
rules, and exact planned-bit invariant. No libdeflate implementation text was
copied or translated.

### 7-Zip and AdvanceCOMP

7-Zip's Deflate encoder uses priced optimal parsing and a second pass whose
prices come from the first pass's tables
([`DeflateEncoder.cpp`](https://github.com/ip7z/7zip/blob/main/CPP/7zip/Compress/DeflateEncoder.cpp#L2765-L3421)).
AdvanceCOMP is a recompression suite that includes 7-Zip- and Zopfli-derived
Deflate encoders ([project source](https://github.com/amadvance/advancecomp)).

Both are useful external ceilings for complete recompression. Neither exposes
a missing post-optimization primitive that can be imported without match
discovery or raw-byte reparsing.

### ECT

[Efficient Compression Tool](https://github.com/fhanau/Efficient-Compression-Tool)
is a file optimizer that bundles modified zlib and Zopfli-derived encoder
sources. Its published size/time table demonstrates the familiar tradeoff
between more encoder work and a thinner ratio tail, but its Deflate gains come
from recompressing decoded bytes. Treat it as a comparison encoder, not as an
existing-token optimizer.

### Turtledeflate

Turtledeflate combines match enumeration, several parse-cost models, repeated
boundary refinement, and recompression of changed partitions. Columbo already
implements independent cumulative-histogram and adaptive-boundary concepts,
while its match discovery and parse diversification remain out of scope. The
source-pinned audit and exact classifications are in
[`turtledeflate-methods.md`](./turtledeflate-methods.md).

The bounded adjacent-boundary reseat and, after a diagnosed miss, one
forced-split escape are now implemented. A standalone second-basin candidate
did not improve the original guard and remains excluded; the forced escape
uses its runner-up only as an intermediate before exact replanning and one
reseat.

## Ground-truth gaps in the current source

The following distinctions are important when evaluating the proposed work:

| Source | Audited responsibility |
| --- | --- |
| [`header.rs`](../../src/deflate/header.rs) | Data-tree candidates, exact payload pricing, dynamic-header RLE and finished-tree moves |
| [`huffman.rs`](../../src/deflate/huffman.rs) | Length-limited tree builders and Columbo's adjacency-aware pseudo-frequencies |
| [`search.rs`](../../src/deflate/search.rs) | Same-distance, proven-submatch, match-family and feedback searches |
| [`stream.rs`](../../src/deflate/stream.rs) | Grouping, splitting, global boundary graph and range caches |
| [`block.rs`](../../src/deflate/block.rs) | Exact representation selection and canonical block-plan cache |

- `tree_candidates` retains up to twenty unique trees per data alphabet and
  exactly prices the literal/distance cross-product. More ordinary tree
  builders alone are unlikely to help.
- Max has two independent paired pseudo-frequency candidates. Columbo's
  adjacency quantizer rounds neighbouring counts onto a logarithmic grid. The
  Zopfli-compatible control preserves useful equal-count runs and collapses
  sufficiently long nearby-count strides to their rounded mean. Both build
  one literal/distance pair and are exact-priced against the original counts.
- Equal-frequency reassignment and greedy length swaps operate on both data
  alphabets. They change which symbols receive existing lengths; they do not
  enumerate new near-optimal length histograms.
- `plan_columbo_balanced_tree_candidate` now generates pair and quad moves for
  both data alphabets and exactly prices at most sixteen paired combinations.
  The compact route still retains its established one-block and token-count
  work bounds.
- `shortest_rle` is exact for one fixed nineteen-symbol code-length tree. The
  enclosing route then retains one feedback tree and repeats for at most four
  passes. It does not keep several near-tie RLE parses whose different symbol
  counts may build a better next tree or shorten `HCLEN`.
- Proven-submatch search solves every selected match under fixed current data
  costs, also tries a source-length-symbol-free solution, exactly prices
  individual and combined rewrites, and in Max may repeat to stability. It
  does not retain several near-tie paths per match for a global
  frequency/header beam.
- The canonical block-plan cache verifies complete token state, source-tree
  seed, and policy. A narrower route-local header cache now reuses completed
  kernels for exact trimmed literal/distance length sequences and header policy,
  even when token order differs. Payload bits remain independently priced.

## Proposed experiments

### 1. Symmetric distance-tree and paired balanced-tree moves — implemented

Columbo's pair and quad moves preserve the Kraft sum while accepting a small
payload penalty only when the complete dynamic block becomes smaller. Apply
the same transformations to the distance tree:

- **pair:** shorten one length-`L` code to `L-1` and lengthen two other
  length-`L` codes to `L+1`;
- **quad:** shorten one length-`L` code to `L-1` and lengthen four
  length-`L+1` codes to `L+2`.

Then price a small cross-product of the best literal-side and distance-side
moves. This can change the RLE run that crosses from the last transmitted
literal/length entry to the first distance entry, a legal interaction that
independent alphabet selection can miss.

Implemented bounds:

- retain the existing candidate caps and payload-delta margin initially;
- try standalone distance moves, then at most the four lowest-payload-delta
  moves from each alphabet as paired candidates;
- validate maximum code length, every used symbol, end-of-block, and complete
  tree shape before header planning; and
- keep the existing compact one-dynamic-block route: at most 4,096 tokens and
  its established source-size gates;
- in default mode, restrict paired work to non-positive combined payload
  delta. Max may use the existing positive eighteen-bit margin; and
- preserve Max's existing omission of standalone literal pair pricing on
  matched streams while still allowing distance and paired work.

Verbose source-opportunity counters now report dynamic blocks, legal bounded
literal/length moves, legal bounded distance moves, and the maximum number of
paired prices. The following outcome counters remain useful for corpus
diagnosis:

- standalone literal, standalone distance, and paired candidates priced;
- candidates rejected by payload margin or tree validity;
- payload bits added, header bits removed, exact net bits saved; and
- wins unique to the paired search.

This was selected first because it is deterministic, small, and exercised a
clear source asymmetry. Focused tests cover literal and distance pair/quad
validity, the paired cross-alphabet case, and opportunity counting.

### 2. K-best code-length-RLE feedback — explored, not adopted

For fixed literal/length and distance code lengths, the possible RLE streams
form a small acyclic graph over at most 316 decoded lengths. Columbo currently
returns one cheapest path under one fixed code-length tree. Replace the scalar
shortest path in Max with a bounded k-shortest-path variant:

1. retain the best `K` distinct RLE streams under the current code-length
   costs;
2. de-duplicate them by both token spelling and nineteen-symbol frequency
   vector;
3. build the existing code-length-tree families for each frequency vector;
4. exactly price `HCLEN`, the transmitted code-length tree, RLE symbols and
   extra bits; and
5. retain a small exact-cost beam until its fingerprint stabilizes or the pass
   cap is reached.

This is not claimed to be a globally exact joint solver. The fully coupled
objective makes the RLE edge costs depend on the tree built from the completed
path. A bounded beam is practical; a brute-force oracle can prove optimality
only for short test sequences.

Recommended initial bounds:

- `K = 2` for exact fixed-cost ties in default mode;
- `K = 4` and at most four bits of fixed-tree deficit in Max;
- retain at most eight exact `(RLE, code-length tree)` states per pass; and
- stop on a repeated state fingerprint, four passes, or the route deadline.

Suggested counters:

- headers with more than one fixed-cost RLE path in the retained window;
- distinct RLE frequency vectors produced;
- alternate paths that reduce `HCLEN` or rebuilt-tree cost;
- exact wins not reached by the current single-feedback route; and
- states discarded by beam, deficit, duplicate, and deadline gates.

Add exhaustive tests for short code-length sequences whose RLE spellings use a
small active alphabet. Enumerate every legal spelling and every complete
length-limited code-length tree for those active symbols. Assert that
production never reports a cost below the oracle and that the bounded route
retains its source candidate.

The August 2026 prototype implemented the fixed-cost path frontier and applied
it only after the selected data tree, avoiding multiplication across the
literal/distance tree grid. It confirmed that the coupling is real: a
deterministically generated header improved from 617 to 616 bits when an
equal-fixed-cost alternate spelling received another tree rebuild. The
production method was nevertheless rejected at the tested bounds:

- 20 normal-mode compact PNG comparisons produced identical final byte and
  meaningful-bit counts, while elapsed time increased from 2.67 to 4.00
  seconds;
- eight Max comparisons across compact PNG and zlib inputs produced no byte
  improvement, and the inspected verbose comparison had the same meaningful
  bit count; and
- one isolated Max balanced-tree cleanup increased from about 84 to 178
  milliseconds.

The short-sequence exhaustive oracle was retained as a test of the existing
scalar `shortest_rle` solver. Reconsider k-best feedback only if a diagnosed
real-file miss identifies a specific alternate histogram or a substantially
cheaper frontier representation removes the measured overhead.

### 3. Exact Zopfli RLE-friendly pseudo-frequencies — implemented

Columbo now independently implements Zopfli's published
`OptimizeHuffmanForRle` behavior as one additional paired literal/distance
candidate. It leaves trailing zeros alone, marks existing runs of at least five
zeros or seven equal nonzero counts, and replaces eligible nearby-count strides
with their rounded mean. The resulting trees are validated and their payload
is scored from the unmodified source frequencies through Columbo's complete
header planner.

The experiment met the Max retention threshold:

- a deterministic synthetic histogram prices at 4,366 bits, ten bits below
  Columbo's independent adjacency quantizer;
- diagnostics on compact real streams found repeated complete-plan wins of
  one to sixteen bits before later route selection; and
- in an eleven-file Max comparison against the clean baseline, three priority
  files retained final gains: `small/present.png` saved another 2 bytes / 16
  meaningful bits, `medium/Mittens.png` another 13 bytes / 107 bits, and
  `css-ig-net/barchart.png` another 9 bytes / 72 bits. The other eight files
  tied the baseline final byte and meaningful-bit result; and
- a triplicate serial Max timing of one short no-gain control increased from
  3.41 to 4.18 seconds in total, about 0.26 seconds per file. This measured
  cost is why the candidate remains Max-only.

The implementation remains one Max-only paired candidate; it does not form a
cross-product with the existing tree families. Source attribution and the
upstream Apache-2.0 provenance are retained beside the independently written
transform.

### 4. Header-aware data-tree frontier — graft explored, not adopted

Ordinary Huffman construction minimizes payload for a fixed symbol set. It
does not minimize payload plus the cost of describing its code-length
sequence. Columbo samples this larger objective with several builders,
pseudo-frequencies, assignments, swaps, and literal-side Kraft moves. A Max
frontier can generalize those samples without enumerating every tree.

If a diagnosed miss justifies returning to the frontier, seed it with the best
distinct current trees and generate bounded neighbours such as:

- the symmetric pair/quad moves above;
- one generalized Kraft exchange beyond pair/quad, selected from a
  precomputed table whose removed and added code-space weights are exactly
  equal;
- a near-optimal length histogram produced by forbidding one decision from the
  winning length-limited construction.

The first bounded prototype tested an **unused-symbol graft**: lengthen one rare
used leaf from `L` to `L+1` and assign an unused, header-adjacent symbol the
other `L+1` leaf. This preserves the Kraft sum while spending the used symbol's
frequency in payload bits. The implementation restricted targets to the
already-transmitted alphabet span, required the new leaf to join an existing
equal-length run, retained at most sixteen candidates per alphabet, and
exact-priced the complete Max header.

The mechanism won a constructed complete-header case, but did not meet the
real-file threshold. A read-only scan of 800 PNG fixtures covered 1,278 source
dynamic blocks and found no direct win. Six full Max routes likewise accepted
no graft. Running the frontier inside every Max rebuild roughly doubled one
short control, while moving it into the bounded compact cleanup restored the
runtime but still produced no accepted candidate. The production graft and its
opportunity counters were therefore removed. Reconsider it only for a
diagnosed tree shape that exhibits this exact missing move.

For each neighbour, compute the payload delta from frequencies, reject a
configurable positive margin, then exactly price the complete header. Keep a
small Pareto frontier over payload bits, header bits, `HLIT`, `HDIST`, and
`HCLEN` rather than only the current total winner.

Suggested counters:

- unique length histograms and symbol assignments reached;
- candidates at each payload-deficit band;
- header-only, payload-only, and combined exact gains; and
- wins already duplicated by another tree family.

### 5. Header-aware proven-spelling composition — implemented

The retained Max route gives each targeted original match a small menu rather
than one local path:

- the exact source match;
- the current-tree payload minimum;
- the current source-length-symbol-free path;
- the all-literal spelling.

The source spelling is implicit, so each match has at most four spellings.
Header-equivalent alternatives are removed before the beam. The implementation
composes the menus with exact literal/length frequencies, distance frequencies,
and match-extra-bit totals, and gives ranking credit to states that remove a
high trailing symbol or a source symbol whose frequency is one or two.

The production gates are:

- Max only, inside the compact M3 proven-feedback candidate;
- the M3 source parent is one block with at most 4,000 tokens and 80,000
  decoded bytes; the spelling produced by its initial floor may contain at
  most 8,000 tokens;
- two to 128 source matches;
- at most eight targeted matches and four spellings per match;
- beam width 16 and depth eight;
- states remain within 24 estimated payload bits of the current best; and
- only states rewriting at least two matches are exact-priced, under a hard
  ceiling of 32 plans.

Every beam layer deduplicates complete frequency and extra-bit state. Surviving
states are materialized with the original match distances, sent through the
ordinary complete Max block planner, and retained only on a strict exact-bit
win. The original M3 floor remains the incumbent.

The implementation was retained after a read-only scan of 800 source PNGs:
3,736 source blocks were inspected, 1,706 met the composition scan's
8,000-token/128-match ceiling, and 98 blocks improved over the existing
integrated proven floor. Complete ten-second Max A/B tests produced a unique
one-byte/six-bit gain on `imageworsener/p8tbg.png` and a same-byte/two-bit gain
on `PngSuite/f02n2c08.png`. The established eleven-file hard sample was
unchanged in bytes and meaningful bits.

The beam prioritizes states that can:

- retain the smallest estimated payload delta;
- remove the highest used literal/length or distance symbol;
- eliminate a frequency-one or frequency-two symbol;
- reduce the transmitted literal/distance alphabet spans; or
- reduce the number of active payload symbols.

This route never searches the history window: every submatch remains inside
one original match and retains its original distance. Its novelty is global
selection across several already-proven alternatives.

### 6. Boundary polishing not already covered — one reseat implemented

The global boundary graph already chooses exactly among its known cuts and
prices starting bit alignment. The missing work is cut *discovery*, not another
segmentation pass over the same nodes.

Two bounded Max experiments were evaluated:

1. retain one well-separated second minimum from the existing adaptive split
   probe cache and exactly plan it; and
2. perform one parity pass over adjacent selected blocks, join each pair, find
   one new adaptive cut across the union, and accept only the exactly smaller
   complete route.

The second-basin prototype tied the existing output on all ten selected
priority-guard files and was removed. The adjacent-boundary reseat is retained:
before broad source search, Max considers comparison floors of 2–8
alignment-independent Huffman blocks, capped at 8,192 tokens and 512 KiB
decoded. It reuses the adaptive histogram probe and canonical plan cache,
exactly prices each completed adjacent replacement, and keeps at most the
strongest single strict win. It never repeats recompression or discovers new
matches.

At an equivalent ten-second Max timeout, repeatable final-file A/B wins were
10 bytes / 80 meaningful bits on `sample_17-fs8.png`, 1 byte / 8 bits on
`bomb.png`, and 3 meaningful bits at equal bytes on `loupe-fs8.png`. The direct
comparison-floor replacements saved 366, 13, and 17 bits respectively; later
lineage work could improve those completed parents further. A serial 40-case
historical regression guard exposed no miss attributable to the route: its
reported misses either tied the clean executable, came from a Max-ineligible
plan, or were in Default mode. The original complete candidate remains the
quality floor throughout. `bomb.png` and `loupe-fs8.png` remained effectively
timing-neutral. On `sample_17-fs8.png`, the better parent admitted more
downstream lineage work and increased observed wall time from 14.6 to 19.7
seconds despite the same configured timeout; that measured cost buys the
byte-first win rather than hiding it.

### 7. Header-kernel and wider interval caching — frequency layer not adopted

Broader search should be paid for by eliminating duplicated deterministic
work. The first layer is now implemented:

- a bounded route-local header cache is keyed by the exact trimmed literal and
  distance length sequences, exhaustive policy, and RLE-mask policy. It stores
  a completed zero-payload header kernel, verifies the full key after hash
  lookup, clones it fallibly, and adds each caller's payload bits with checked
  arithmetic. Distinct token orders can therefore share the deterministic
  header search without sharing payload identity.

The cache is capped at 512 entries. Focused tests prove policy isolation,
payload-cost independence, and reuse across different token orders that miss
the canonical whole-block cache. Runtime instrumentation observed real hits;
a triplicate short Max control was neutral at 3.99 seconds before and 4.01
seconds after, with identical output.

A second prototype cached the complete fixed/dynamic planning kernel by exact
literal/length frequencies, distance frequencies, match extra-bit total,
strict and exhaustive policy, and source-tree seed. It retained at most 512
completed immutable kernels, verified the full key after hash lookup, and
never published deadline-aborted work. Focused tests proved reuse across
different token orders, policy and source-seed isolation, collision safety,
and bounded saturation.

The route-level opportunity rate was too low to retain it. Across five
targeted Max files, telemetry found only one frequency-equivalent reuse among
hundreds of route-local cache probes. The five-run Default control on
`nascar.png` remained about 0.42–0.43 seconds after warm-up with or without the
prototype. On the one file that produced a hit,
`sample_17-fs8.png`, both versions selected the same 2,992-byte / 23,929-bit
Deflate result; the measured Max wall time was 18.4 seconds without the cache
and 19.0 seconds with it. That single noisy comparison showed no speed benefit
to offset the added hashing and retained memory, so the prototype was removed.

Immutable interval sharing across sibling lineages remains conceptually safe,
but the frequency-cache result removes the evidence for implementing it now.
Do not add synchronization to the hot path unless later instrumentation finds
a materially denser duplicate-work class.

### 8. One forced-split escape — implemented

A locally losing split can expose a later winning boundary arrangement, but
unrestricted lookahead is expensive and usually unproductive. A reproducible
miss on `sample_17-fs8.png` demonstrated this case: the ordinary two-block
comparison floor in one winning lineage cost 24,279 meaningful bits; forcing
the well-separated runner-up adaptive basin initially worsened it to 24,291
bits, and one adjacent-boundary reseat then reached 24,059 bits.
Downstream compact splitting selected a 23,904-bit result. Against the
pre-change binary at the same ten-second Max timeout, the final PNG improved
from 3,943 to 3,939 bytes and its Deflate stream from 23,929 to 23,904
meaningful bits on repeated dry runs.

The retained route is deliberately narrower than a repeated pushed-split
search. It runs only in Max while time remains, starts from 2–7
alignment-independent Huffman blocks totalling at most 8,192 tokens and 512
KiB decoded bytes, and selects the largest block having at least 513 tokens
and 128 decoded bytes. The existing adaptive search keeps one sampled local
minimum at least the greater of one seventh of the token span or sixteen
tokens away from its best basin, without exceeding the existing 128-probe
budget. Columbo forces exactly that one cut, exactly replans the two children,
then tries exactly one existing adjacent-boundary reseat. The secured complete
floor remains available throughout, and the new candidate survives only on a
strict complete-plan meaningful-bit win.

The first wider dry-run sample covered the historical backlog and fifteen
additional PNG/APNG files. It found no other output difference or route
displacement, which is appropriate for a rare escape rather than a new broad
split loop.

## Methods that remain excluded

Do not add any of the following under the current product scope:

- hash-chain, binary-tree, suffix-array, or brute-force match discovery;
- alternative match distances, even if another distance is already used
  elsewhere in the block;
- extending a proven match across intervening literals by rechecking decoded
  bytes against history;
- Zopfli, libdeflate, 7-Zip, ECT, or Turtledeflate recompression;
- randomized or perturbed LZ77 parsing;
- repartitioning raw bytes and then discovering a new parse in each partition;
- sharing a dynamic tree between separate blocks, because each block must
  transmit its own tree;
- reserved-symbol, oversubscribed-tree, or incomplete-tree tricks in strict
  mode; or
- empty-block alignment tricks without an exact demonstrated net saving.

More Zopfli iterations, faster match caches, and SIMD match finders may improve
a comparison encoder, but they cannot improve Columbo without crossing the
match-discovery boundary.

## Measurement and safety protocol

Instrument opportunities before enabling a new search. Counters must
distinguish source opportunities, candidates visited, exact plans priced,
unique winners, and work stopped by each gate. Otherwise a route that merely
duplicates an existing winner can look productive.

For every experiment:

1. retain the original complete candidate;
2. compare output bytes first, then meaningful Deflate bits;
3. reparse the emitted stream and compare decoded size and SHA-256;
4. test strict and relaxed mode separately;
5. run default and Max A/B comparisons with identical time budgets;
6. record route wall time, peak retained candidate memory, cache hits, and
   deadline completion;
7. use `--dry-run` for corpus, script, benchmark, and tool invocations whenever
   an output file is not the subject of the test; and
8. treat repository fixtures as read-only inputs. If a test needs mutation,
   use a temporary copy outside `fixtures` and verify that `git status` shows
   no fixture change afterwards.

Start with the last twenty regression files and every known miss above ten
percent, then sample the whole corpus. A method should enter default mode only
after demonstrating broad wins with negligible wall-time regression. Rare
header or forced-boundary wins belong in Max even when exact comparison makes
their output safe.

## Recommended implementation order

1. Completed: add source opportunity counters for balanced-tree moves.
2. Completed: implement symmetric distance-tree moves and a tiny paired
   cross-product.
3. Completed: add the short-sequence exhaustive RLE oracle. The bounded k-best
   prototype did not meet the output/runtime threshold and was removed.
4. Completed: retain exact Zopfli pseudo-frequencies as one differential Max
   candidate after unique complete-plan and final-file wins.
5. Completed: add the bounded route-local header kernel cache and verify reuse
   across distinct token orders without changing output.
6. Completed: trial the unused-symbol graft. Its constructed win did not recur
   across 1,278 source dynamic blocks or six full Max routes, so remove it and
   defer the broader data-tree frontier until a diagnosed miss justifies it.
7. Completed: retain bounded header-aware proven-spelling composition after
   98 local block wins, one unique byte win, one unique same-byte bit win, and
   no change across the established hard sample.
8. Completed: retain one bounded adjacent-boundary reseat after byte and bit
   wins at equal timeout. The well-separated second-basin prototype produced
   no output gain on the selected priority guard and was removed.
9. Completed: trial a bounded frequency-planning kernel cache. One hit across
   five targeted Max files produced no observed runtime or output benefit, so
   remove it and leave cross-lineage interval sharing deferred.
10. Completed: retain one heavily gated forced split after a reproducible
    four-byte file win and twenty-five-bit Deflate win. The route reuses the
    existing 128-probe bound, forces only the well-separated second
    basin, permits one boundary reseat, and exact-prices the complete result.

This order resolves the cheapest untested degrees of freedom first and makes
later searches pay for themselves through measured reuse.
