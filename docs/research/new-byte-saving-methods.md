# New byte-saving methods worth exploring

Research date: 5 September 2026. Source baseline: `9ac02b660ca0867da15547d0d2daed3847d5cf37` **plus the existing working-tree changes**, including `optimize.rs`. The original sections record a small exploratory probe. The positive-payload swap method was subsequently [implemented and validated on 6 September](payload-header-tradeoff-validation.md); advertised literal-span search was subsequently [implemented and validated](literal-span-validation.md), and joint payload-tree/RLE search is now [implemented and validated](joint-tree-rle-validation.md). Symbol-set removal is now [implemented and validated](symbol-set-validation.md). Alphabet-boundary search is now [implemented and validated in Max](alphabet-boundary-validation.md). Joint code-length tree/RLE search is now [implemented and validated in Max](code-length-tree-validation.md). Three-symbol code-length rotations are now [implemented and validated in Max](code-length-rotation-validation.md). The remaining extensions are research. “New” means an additional search dimension relative to the inspected Columbo implementation; worldwide novelty is not claimed.

The first retained method is **payload-cost-increasing code-length swaps**. A small probe found actual gains after Columbo's completed Default raw-stream optimization. Joint optimization of data-code lengths and their RLE description is now retained as a bounded terminal pass.

A subsequent request for more radical approaches is covered in [alternative search formulations](novel-deflate-formulations.md): permanent match proofs, parametric spelling responses, cost-neutral regional exchanges, and header-first synthesis. [Permanent match proofs have now been validated](permanent-match-proofs-validation.md): a targeted pass with unchanged trees improves 48 of 365 distinct completed Default streams, with actual-container and focused Max evidence. The other radical proposals remain unvalidated; their possible savings must not be added to this note's header experiments.

## Scope and objective

Every proposal preserves the decoded bytes, uses existing tokens or already-proven same-distance submatches, and retains Columbo's strict Huffman compatibility rules. No history search, new distance, image refiltering, or external recompression is involved. The production scope is described in [routes and methods](../routes-and-methods.md#scope-boundary).

For a dynamic block, the objective is:

`complete bits = payload codewords + match extra bits + EOB + block/header fields + encoded tree description`.

Improving one term can worsen another. The proposals below target combinations omitted by current candidate generation, rather than treating a payload-optimal Huffman tree as a complete optimum. Final acceptance must still compare emitted stream/container bytes first, then meaningful Deflate bits, including alignment effects.

## Priority order

| Priority | Method | Evidence now | Next experiment |
| --- | --- | --- | --- |
| Implemented | Spend payload bits on better code-length permutations | 108/365 distinct completed Default streams improved; 607 incremental bits beyond full header repricing | [Production validation and bounds](payload-header-tradeoff-validation.md) |
| Implemented | Search the advertised literal/length span | After the payload/header tradeoff pass, three distinct raw streams saved four meaningful bits and one byte | [Production validation and bounds](literal-span-validation.md) |
| Implemented | Joint data-tree and RLE dynamic programming | After R3, 160/365 distinct completed Default streams save 3,344 bits and 419 bytes | [Production solver, oracle, bounds and validation](joint-tree-rle-validation.md) |
| Implemented | Remove a set of symbols to simplify the header | After R4, 27/365 streams save 860 bits and 113 bytes; nine streams retain 49 bits beyond combined existing-route/single-symbol controls | [Production validation and bounds](symbol-set-validation.md) |
| Implemented in Max | Discover cuts from alphabet changes | Nine real streams retain 377 bits beyond old anchors with Max table prices; fresh Max A/Bs improve six of eight cases | [Production bounds, cost policy and validation](alphabet-boundary-validation.md) |
| Implemented in Max | Joint code-length tree and RLE search | Exact fixed-tree/spans solver; 22 Default parents and 20 frozen Max parents improve beyond full header repricing | [Solver, oracle and validation](code-length-tree-validation.md) |
| Implemented in Max | Rotate three payload code lengths together | 30 Default-plus-R7 parents and 29 frozen Max parents improve; pair-swap barriers and seven public Max wins | [Bounds, controls and validation](code-length-rotation-validation.md) |

## 1. Spend payload bits on better code-length permutations

**Gap.** [`improve_one_tree_by_swaps`](../../src/deflate/header.rs) skips every swap with a positive payload delta. Existing pair/quad lengthening can spend payload bits, but changes the code-length histogram through particular Kraft-preserving moves. Swapping two existing lengths preserves that histogram and explores a different family.

For symbols `a` and `b`, swapping their lengths changes payload cost by:

`tax = (frequency[a] - frequency[b]) × (length[b] - length[a])`.

Permit a small positive tax when the new sequence is cheaper to describe. Longer equal-length runs can replace several explicit lengths with repeat tokens, and the changed RLE histogram can shorten its Huffman description. No token or distance changes.

**Measured example.** On the completed Default raw stream extracted from `basi2c16.png`, swapping distance-symbol lengths 13 and 17 adds **1 payload bit**, removes **7 header bits**, and reduces the stream from **4,071 to 4,065 meaningful bits**. Both outputs occupy 509 bytes. This is an actual fixture result.

The probe examined swaps costing 1–18 payload bits, required a reduction in adjacent length transitions, and exactly priced at most 32 proposals per alphabet. Its ranking was `tax - 3 × transitions_removed`; that is a heuristic ordering, not a bound or an acceptance test. It evaluated 3,126 candidates and found 136 locally winning swaps, yielding 21 improved complete streams.

**Implemented extension.** Three-symbol rotations now have a bounded
[Max-only implementation and validation](code-length-rotation-validation.md).
The method preserves each alphabet's length histogram, support and advertised
span, and prices both orientations of selected cycles together. A generated
fixture saves one bit beyond every fully repriced pair swap; real controls
also demonstrate cycles whose first pair steps all tie or lose. The
production pass improves 30 of 365 Default-plus-R7 parents and 29 of 31 freshly
frozen Max parents. Public Max comparisons improve seven of 12 selected cases
while all 404 Default outputs remain byte-identical. Paired literal/distance
swaps remain a separate unvalidated extension.

**Integration, now implemented.** Run this as a sibling of a completed parent after original-match restoration. Exact-price its entire header, re-emit, and retain only a complete win. The [production report](payload-header-tradeoff-validation.md) documents the 128 KiB/128-block model cap, shared 1,024-price ceiling, Default/Max placement, and measured cost. A conservative pruning test is candidate payload plus an admissible minimum header cost versus the incumbent. A transition-count estimate must never reject a candidate as mathematically impossible.

## 2. Search the advertised literal/length span

**Gap.** `trim_literal` and `plan_for_explicit_lengths_with_cost` always trim the literal/length list to the last nonzero length, with a 257-entry minimum. Exact-source reuse can preserve another advertised span, but new explicit-length candidates do not search larger spans.

**Method.** Keep the data trees unchanged and append zero lengths to the literal/length list before concatenating it with the distance list. Search the legal advertised counts through 286. This can extend the zero run at the alphabet seam so it has a cheaper RLE spelling. The count field remains five bits; there is no direct field-width penalty for choosing a larger count.

RFC 1951 permits 257–286 literal/length entries, and its repeat codes operate on the concatenation of both length lists, including across the seam. Appended zero lengths allocate no Huffman code space and change no payload codeword. [RFC 1951 §3.2.7](https://www.rfc-editor.org/rfc/rfc1951#section-3.2.7)

**Measured result.** `basi0g04`, `basi4a16`, and `bgai4a16` each saved one meaningful bit beyond both the completed Default output and full existing header repricing. Physical bytes tied. These are three fixture inputs, including a duplicate stream pair.

**Implemented follow-up.** The [production pass](literal-span-validation.md) now enumerates every legal literal span on the completed payload/header-tradeoff parent. Across 365 distinct completed streams, it saves four meaningful bits and one physical byte; ordinary same-tree repricing ties every parent. Fixed-CL pricing misses all three wins, and a repeat-threshold shortcut misses the byte-saving case, so full header feedback is retained. These are incremental measurements after method 1, not a sum of independent probes. Distance lengths and their advertised count remain fixed.

## 3. Optimize data-tree lengths and RLE together

**Gap addressed.** The preceding pipeline generates candidate data trees, then optimizes their descriptions. Bounded depths, pseudo-frequencies, permutations, and pair/quad moves explore useful trees, but they do not directly solve the combined objective over all trees in a restricted search space.

**Implemented solver.** Freeze one valid code-length Huffman tree and chosen literal/distance spans. Use dynamic programming to choose the data-code lengths and their RLE tokens together.

For a data alphabet with maximum depth `D`, represent complete Kraft capacity as `2^D` integer units. Assigning a positive length `l` consumes `2^(D-l)` units; assigning zero consumes none. An RLE edge emits one literal length or a legal repeat run. Its cost is the exact RLE codeword/extras plus the payload-frequency cost of the lengths assigned by that edge.

The state tracks the sequence position, remaining capacity in the current data alphabet and previous length. The implemented domain forces reserved distance positions to zero; complete Kraft capacity with positive lengths of at least one bit guarantees at least two usable distance codes without another state dimension. Enforce complete capacity separately at the literal/distance seam. An RLE edge crossing that seam must account for each alphabet's capacity separately while carrying the previous length through it. Positive-frequency symbols, including EOB, cannot receive zero lengths. Reserved distance symbols must never become payload symbols.

The production ceiling is nine bits, giving 512 capacity units; absent direct CL length symbols can reduce that ceiling exactly. Prefix frequency sums price repeat runs without rescanning their symbols. Preserve the unrestricted completed parent independently.

**What would be exact.** A complete DP would find the best data trees and RLE for the selected depths, spans, and fixed header tree. It would not establish an unrestricted Deflate optimum. Multiple header-tree seeds or an outer header-tree search enlarge coverage.

This can discover run arrangements and tree shapes that no particular package-merge tie, smoothing rule, or local move proposes. The [production validation](joint-tree-rle-validation.md) includes exhaustive small-tree oracles and crossing-repeat witnesses. Independent payload and header suffix minima give admissible pruning. The pass shares 2^27 work units across a stream, uses less than 24 MiB of scratch per solver, and preserves the completed parent. It saves 419 bytes on 365 distinct completed Default raw streams and 946 bytes on twenty targeted containers; these overlapping samples must not be added. The measured API overhead was 6.1% and 12.7%, respectively, with larger individual outliers recorded in the report.

## 4. Remove symbol sets chosen for their header effect

**Existing overlap.** [`match_group_search`](../../src/deflate/search.rs) already expands length/distance groups individually and in a few payload-ranked prefixes. Proven resegmentation also tries source-symbol-free paths and elimination of the highest length symbol. Merely adding “group elimination” would repeat existing work.

**Additional dimension.** Choose two to four symbols because removing their complete support creates a zero run, removes an isolated nonzero code length, or simplifies a transmitted alphabet tail. Evaluate the set as a unit, even when none of its members wins individually and the set is not a prefix of payload-ranked groups.

For length-symbol removal, solve each affected original match with all forbidden length symbols excluded. Allow literals and submatches entirely within the original interval at its original distance. For distance-symbol removal, every occurrence of that distance symbol must become literals; splitting a match at the same distance does not remove that distance symbol.

One concrete target is two rare interior length symbols interrupting a long zero region. Eliminate both through proven spellings, rebuild both trees, and compare the complete header and payload. Also account for newly introduced literal symbols: deleting one alphabet feature can create a more expensive one elsewhere.

Use actual length-list structure to rank a small set menu and retain the completed parent. Test against both the existing group search and the 16-state proven-composition beam; a useful result must add coverage beyond those routes.

**Implemented follow-up.** R5 now ranks intervals of used length/distance symbols and mixed sets, keeps eight proposals, and prices their complete token/table plans with one feedback attempt. Every submatch retains its original interval and distance. Shared stream caps are 2^25 rewrite-work units and 128 spelling prices. The [validation report](symbol-set-validation.md) records 113 additional raw bytes, 174 bytes on overlapping selected containers, nine witnesses beyond combined existing-route/single-symbol controls, fixed and fresh Max checks, and measured API overhead of 9.9% and 12.3%.

## 5. Discover cuts from alphabet changes

**Existing overlap.** [`choose_cuts`](../../src/deflate/stream.rs), adaptive splitting, entropy scouting, and the global boundary graph already handle many boundaries and coupled partitions. The graph can only select anchors supplied to it.

**Additional dimension.** Generate anchors from first/last occurrences of rare high symbols and localized clusters of length/distance symbols. A cut can shorten an advertised alphabet or remove several isolated tree lengths even when its entropy change is small. Two cuts around a cluster may be useful together while either alone is unhelpful.

Add a bounded number of such anchors to the existing alignment-aware graph. Preserve its original candidate path. Reuse exact histograms to price the two new boundaries jointly, including both added headers, EOBs, match cuts, and stored-block alignment where applicable.

The test must demonstrate a repeatable gain attributable to an anchor absent from the old graph. The prior entropy-scout audit found deadline-dependent apparent gains that did not reproduce, so equal work budgets matter here.

**Implemented follow-up.** R6 ranks support intervals, prices eight pairs, then adds their endpoints to the existing eight-alignment graph. Shared stream caps are 2^26 work units and 4,096 range prices. [Validation](alphabet-boundary-validation.md) demonstrates additional anchors beyond Max-priced old-anchor controls and direct gains on freshly frozen Max parents. The Default-enabled experiment saved 539 raw bytes but added 12.4% runtime, and selected containers added 27.7%; that placement was rejected. The retained Max-only search uses existing Max windows, preserves the historical seed and complete candidates, and leaves all 404 measured Default outputs unchanged. Fresh strict Max A/Bs improve six of eight cases; relaxed Max and Defluff checks pass.

## 6. Search the header tree directly

**Gap.** `shortest_rle` finds the cheapest spelling for one fixed code-length
Huffman tree. Seeded, bounded feedback does not enumerate every joint tree and
spelling. The earlier K-best RLE experiment lacked final real-world gains;
reinstating that path beam without new evidence remains unwarranted.

**Implemented formulation.** Factor the fixed data-length list into maximal
runs. Enumerate repeat-16's code length, compute each positive value's run cost
at every possible literal code price, and solve a 128-unit Kraft-capacity DP.
Enumerating the remaining zero/repeat code lengths selects a capacity state;
exact zero-run prices complete the objective. HCLEN is fixed by mandatory
positive values. This covers every useful complete seven-bit CL tree for the
fixed data trees and advertised spans, including optional repeat support.

The [validation report](code-length-tree-validation.md) compares the solver
with exhaustive tree enumeration, existing full header repricing, and freshly
captured Max parents. A bounded terminal Max implementation preserves every
completed incumbent, shares 2^24 work units per stream, and leaves Default
unchanged. Public-API checks improve 10 of 13 selected strict Max cases and three of four relaxed Max cases, with no regressions; all 404 Default outputs remain byte-identical. This is a new joint
cost-model search within Columbo; it is not a claim of global Deflate
optimality or worldwide novelty.

## Probe results and limits

The probe extracted zlib-valid IDAT streams from 174 top-level PngSuite PNG inputs, representing **127 distinct input streams**. It optimized each as a **standalone raw stream in Default mode**, then inspected 126 final dynamic blocks. This does not reproduce PNG-specific scheduling or constitute a Max comparison.

| Probe | Complete-stream wins | Physical bytes saved | Meaningful bits saved |
| --- | ---: | ---: | ---: |
| Literal-span extension | 3 | 0 | 3 |
| Positive-payload swaps versus completed Default | 21 | 15 | 129 |
| Swap contribution beyond existing full header repricing | 21 | 14 | 120 |
| Strict filler-code relocation | 0 | 0 | 0 |

The swap total includes 9 bits already recoverable by the existing full header repricer on `basi6a16`; the third row removes that contribution. Rows are alternative comparisons, not additive savings. Keeping the best measured method per input yields **22 improved inputs, 15 bytes and 130 meaningful bits**, representing 18 distinct winning output streams. Related and duplicate fixtures prevent treating these counts as independent workload evidence.

Every emitted rewrite was reparsed by Columbo, checked for strict tree compatibility, and compared for decoded identity. Python's independent zlib decoder verified the 22 retained candidate outputs against the original streams. No PNG containers were emitted by this probe. Production source was not changed.

The isolated swap test took 1.05 seconds, including reads, parsing, candidate generation, 3,126 complete header prices, emission, and checks. Compilation and Default baseline generation were excluded. This is a single-run observation, not a production overhead measurement.

Filler relocation tried all 435 pairs of usable one-bit distance-code positions for literal-only blocks, or all 29 alternate filler positions for a singleton used distance alphabet. Across 14 eligible blocks it produced no gain. Keep it low priority; the earlier broad unused-symbol graft also failed to show real gains. The worthwhile initial result is the positive-payload swap family.

### Reproduction and next acceptance test

The local exploratory files are [the driver](../../work/run-header-invention-probe.py), [the injected Rust tests](../../work/header-invention-probe.rs), [the run log](../../work/header-invention-probe.log), [span/filler results](../../work/header-invention-probe-results.csv), and [swap results](../../work/header-invention-swap-results.csv). They live in the repository's intentionally ignored `work` area and require the private fixture corpus. The driver copies source into a temporary directory before injecting tests.

Run `python3 work/run-header-invention-probe.py` from the repository. The code inspected for these results had these SHA-256 values:

```text
header.rs    26c21de89101024484740eb00701f9f360ef57948fcbb89828824e4e56af2a97
search.rs    63cfcb13590e3c07a53715f70d5813eeff2454340337fd6a3e6b88ba9d7a5c58
optimize.rs  906ca286df6ebe4cdc8cc8320ce22d3d7e1d85a04546259d461afcb4a6637417
```

The positive-payload method has passed its [production acceptance test](payload-header-tradeoff-validation.md), literal-span search has passed its [separate incremental validation](literal-span-validation.md), joint tree/RLE search has passed its [incremental validation](joint-tree-rle-validation.md), symbol-set removal has passed its [incremental validation](symbol-set-validation.md), alphabet-boundary search has passed its [Max-only validation](alphabet-boundary-validation.md), joint code-length tree/RLE search has passed its [Max-only validation](code-length-tree-validation.md), and three-symbol rotations have passed their [Max-only validation](code-length-rotation-validation.md). For the remaining proposals, compare additive terminal candidates against completed Default and Max outputs across PNG/APNG, GZIP, ZIP, and zlib families. Include already-optimized references, deduplicate streams when assessing generality, measure exact work and runtime, and independently decode reconstructed wrappers. Preserve every complete incumbent. Require reproducible incremental savings with acceptable cost; a synthetic witness or a wall-clock scheduling advantage alone is insufficient.
