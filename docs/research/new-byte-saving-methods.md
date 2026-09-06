# New byte-saving methods worth exploring

Research date: 5 September 2026. Source baseline: `9ac02b660ca0867da15547d0d2daed3847d5cf37` **plus the existing working-tree changes**, including `optimize.rs`. The original sections record a small exploratory probe. The positive-payload swap method was subsequently [implemented and validated on 6 September](payload-header-tradeoff-validation.md); advertised literal-span search was subsequently [implemented and validated](literal-span-validation.md), while the other proposals remain research. “New” means an additional search dimension relative to the inspected Columbo implementation; worldwide novelty is not claimed.

The first retained method is **payload-cost-increasing code-length swaps**. A small probe found actual gains after Columbo's completed Default raw-stream optimization. The larger research opportunity is to optimize data-code lengths and their RLE description together.

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
| 3 | Joint data-tree and RLE dynamic programming | Algorithm proposal; not prototyped | Exact bounded-depth solver for one fixed code-length tree |
| 4 | Remove a set of symbols to simplify the header | Source-grounded extension; not prototyped | Joint removal of small interior symbol sets, with proven submatch alternatives |
| 5 | Discover cuts from alphabet changes | Source-grounded extension; not prototyped | Paired anchors around rare-symbol clusters missed by current cuts |
| Supporting research | Enumerate the tiny header tree directly | Oracle proposal; not prototyped | Establish whether bounded feedback misses a better header on final trees |

## 1. Spend payload bits on better code-length permutations

**Gap.** [`improve_one_tree_by_swaps`](../../src/deflate/header.rs) skips every swap with a positive payload delta. Existing pair/quad lengthening can spend payload bits, but changes the code-length histogram through particular Kraft-preserving moves. Swapping two existing lengths preserves that histogram and explores a different family.

For symbols `a` and `b`, swapping their lengths changes payload cost by:

`tax = (frequency[a] - frequency[b]) × (length[b] - length[a])`.

Permit a small positive tax when the new sequence is cheaper to describe. Longer equal-length runs can replace several explicit lengths with repeat tokens, and the changed RLE histogram can shorten its Huffman description. No token or distance changes.

**Measured example.** On the completed Default raw stream extracted from `basi2c16.png`, swapping distance-symbol lengths 13 and 17 adds **1 payload bit**, removes **7 header bits**, and reduces the stream from **4,071 to 4,065 meaningful bits**. Both outputs occupy 509 bytes. This is an actual fixture result.

The probe examined swaps costing 1–18 payload bits, required a reduction in adjacent length transitions, and exactly priced at most 32 proposals per alphabet. Its ranking was `tax - 3 × transitions_removed`; that is a heuristic ordering, not a bound or an acceptance test. It evaluated 3,126 candidates and found 136 locally winning swaps, yielding 21 improved complete streams.

**Further invention.** Extend the winning family to three-symbol rotations and paired literal/distance swaps. Keep bounded candidates that individually tie or lose, then price their combination. This can cross a plateau that strict-improvement single-swap search cannot cross. Equal-frequency rotations are especially attractive because their payload tax is zero. These extensions have not been measured.

**Integration, now implemented.** Run this as a sibling of a completed parent after original-match restoration. Exact-price its entire header, re-emit, and retain only a complete win. The [production report](payload-header-tradeoff-validation.md) documents the 128 KiB/128-block model cap, shared 1,024-price ceiling, Default/Max placement, and measured cost. A conservative pruning test is candidate payload plus an admissible minimum header cost versus the incumbent. A transition-count estimate must never reject a candidate as mathematically impossible.

## 2. Search the advertised literal/length span

**Gap.** `trim_literal` and `plan_for_explicit_lengths_with_cost` always trim the literal/length list to the last nonzero length, with a 257-entry minimum. Exact-source reuse can preserve another advertised span, but new explicit-length candidates do not search larger spans.

**Method.** Keep the data trees unchanged and append zero lengths to the literal/length list before concatenating it with the distance list. Search the legal advertised counts through 286. This can extend the zero run at the alphabet seam so it has a cheaper RLE spelling. The count field remains five bits; there is no direct field-width penalty for choosing a larger count.

RFC 1951 permits 257–286 literal/length entries, and its repeat codes operate on the concatenation of both length lists, including across the seam. Appended zero lengths allocate no Huffman code space and change no payload codeword. [RFC 1951 §3.2.7](https://www.rfc-editor.org/rfc/rfc1951#section-3.2.7)

**Measured result.** `basi0g04`, `basi4a16`, and `bgai4a16` each saved one meaningful bit beyond both the completed Default output and full existing header repricing. Physical bytes tied. These are three fixture inputs, including a duplicate stream pair.

**Implemented follow-up.** The [production pass](literal-span-validation.md) now enumerates every legal literal span on the completed payload/header-tradeoff parent. Across 365 distinct completed streams, it saves four meaningful bits and one physical byte; ordinary same-tree repricing ties every parent. Fixed-CL pricing misses all three wins, and a repeat-threshold shortcut misses the byte-saving case, so full header feedback is retained. These are incremental measurements after method 1, not a sum of independent probes. Distance lengths and their advertised count remain fixed.

## 3. Optimize data-tree lengths and RLE together

**Gap.** The current pipeline generates candidate data trees, then optimizes their descriptions. Bounded depths, pseudo-frequencies, permutations, and pair/quad moves explore useful trees, but they do not directly solve the combined objective over all trees in a restricted search space.

**Proposed solver.** Freeze one valid code-length Huffman tree and chosen literal/distance spans. Use dynamic programming to choose the data-code lengths and their RLE tokens together.

For a data alphabet with maximum depth `D`, represent complete Kraft capacity as `2^D` integer units. Assigning a positive length `l` consumes `2^(D-l)` units; assigning zero consumes none. An RLE edge emits one literal length or a legal repeat run. Its cost is the exact RLE codeword/extras plus the payload-frequency cost of the lengths assigned by that edge.

The state tracks the sequence position, remaining capacity in the current data alphabet, previous length, and the small compatibility state needed for usable distance-code counts. Enforce complete capacity separately at the literal/distance seam. An RLE edge crossing that seam must account for each alphabet's capacity separately while carrying the previous length through it. Positive-frequency symbols, including EOB, cannot receive zero lengths. Reserved distance symbols must never become payload symbols.

Start with small feasible ceilings, such as 8 or 9, where capacity is 256 or 512 units. Prefix frequency sums price repeat runs without rescanning their symbols. Preserve the unrestricted completed parent independently.

**What would be exact.** A complete DP would find the best data trees and RLE for the selected depths, spans, and fixed header tree. It would not establish an unrestricted Deflate optimum. Multiple header-tree seeds or an outer header-tree search enlarge coverage.

This can discover run arrangements and tree shapes that no particular package-merge tie, smoothing rule, or local move proposes. The first deliverable should be a small oracle and minimized witnesses; state counts and runtime will determine whether a production route is practical.

## 4. Remove symbol sets chosen for their header effect

**Existing overlap.** [`match_group_search`](../../src/deflate/search.rs) already expands length/distance groups individually and in a few payload-ranked prefixes. Proven resegmentation also tries source-symbol-free paths and elimination of the highest length symbol. Merely adding “group elimination” would repeat existing work.

**Additional dimension.** Choose two to four symbols because removing their complete support creates a zero run, removes an isolated nonzero code length, or simplifies a transmitted alphabet tail. Evaluate the set as a unit, even when none of its members wins individually and the set is not a prefix of payload-ranked groups.

For length-symbol removal, solve each affected original match with all forbidden length symbols excluded. Allow literals and submatches entirely within the original interval at its original distance. For distance-symbol removal, every occurrence of that distance symbol must become literals; splitting a match at the same distance does not remove that distance symbol.

One concrete target is two rare interior length symbols interrupting a long zero region. Eliminate both through proven spellings, rebuild both trees, and compare the complete header and payload. Also account for newly introduced literal symbols: deleting one alphabet feature can create a more expensive one elsewhere.

Use actual length-list structure to rank a small set menu and retain the completed parent. Test against both the existing group search and the 16-state proven-composition beam; a useful result must add coverage beyond those routes.

## 5. Discover cuts from alphabet changes

**Existing overlap.** [`choose_cuts`](../../src/deflate/stream.rs), adaptive splitting, entropy scouting, and the global boundary graph already handle many boundaries and coupled partitions. The graph can only select anchors supplied to it.

**Additional dimension.** Generate anchors from first/last occurrences of rare high symbols and localized clusters of length/distance symbols. A cut can shorten an advertised alphabet or remove several isolated tree lengths even when its entropy change is small. Two cuts around a cluster may be useful together while either alone is unhelpful.

Add a bounded number of such anchors to the existing alignment-aware graph. Preserve its original candidate path. Reuse exact histograms to price the two new boundaries jointly, including both added headers, EOBs, match cuts, and stored-block alignment where applicable.

The test must demonstrate a repeatable gain attributable to an anchor absent from the old graph. The prior entropy-scout audit found deadline-dependent apparent gains that did not reproduce, so equal work budgets matter here.

## Supporting research: search the header tree directly

`shortest_rle` finds the shortest spelling for **one fixed code-length Huffman tree**. `consider_rle` explores seeded, bounded feedback paths; its exhaustive setting is not an enumeration of every possible header tree and spelling. The earlier K-best RLE experiment was rejected on runtime and lack of final real-world gains.

A different oracle would enumerate complete, at-most-seven-bit code-length trees over the small set of RLE symbols that can occur, including optional repeat-symbol support and strict completion when necessary. For each tree, run the existing shortest-path RLE solver and add its actual HCLEN cost. Enumerate all admitted support choices if claiming exactness; an arbitrary support cap yields only a bounded oracle.

This searches the cost models directly instead of retaining many paths with repeated histograms. Begin with final headers having very small possible RLE alphabets. Use any diagnosed miss to design a cheap candidate generator, or use a proven optimum to avoid spending further header-only effort on that state. Do not reinstate broad K-best feedback without new evidence.

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

The positive-payload method has passed its [production acceptance test](payload-header-tradeoff-validation.md), and literal-span search has passed its [separate incremental validation](literal-span-validation.md). For the remaining proposals, compare additive terminal candidates against completed Default and Max outputs across PNG/APNG, GZIP, ZIP, and zlib families. Include already-optimized references, deduplicate streams when assessing generality, measure exact work and runtime, and independently decode reconstructed wrappers. Preserve every complete incumbent. Require reproducible incremental savings with acceptable cost; a synthetic witness or a wall-clock scheduling advantage alone is insufficient.
