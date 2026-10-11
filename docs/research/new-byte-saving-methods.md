# New byte-saving methods worth exploring

Research date: 5 September 2026. Source baseline: `9ac02b660ca0867da15547d0d2daed3847d5cf37` **plus the existing working-tree changes**, including `optimize.rs`. This memo proposed six methods and records the small exploratory probe behind them. All six are implemented, the first on 6 September, with the extensions in the [status table](#status); each method's validation report is the authority for its design, bounds and measurements. Extensions beyond those bounded implementations remain research. “New” means a search dimension absent from the inspected Columbo implementation, not worldwide novelty.

[Alternative search formulations](novel-deflate-formulations.md) develops four more radical approaches. R1 ([permanent match proofs](permanent-match-proofs-validation.md)) and R10/R11 (parts of header-first synthesis) come from it; its broader proposals remain unvalidated, and their possible savings must not be added to this memo's header experiments.

## Scope and objective

Every proposal preserves the decoded bytes, uses existing tokens or already-proven same-distance submatches, and keeps Columbo's strict Huffman compatibility rules. No history search, new distance, image refiltering or external recompression is involved ([scope boundary](../routes-and-methods.md#scope-boundary)). For a dynamic block the objective is:

`complete bits = payload codewords + match extra bits + EOB + block/header fields + encoded tree description`.

Improving one term can worsen another. The proposals therefore target combinations the baseline omitted, rather than treating a payload-optimal Huffman tree as optimal for the whole block. Acceptance compares emitted stream/container bytes first, then meaningful Deflate bits, including alignment effects.

## Status

| Proposal | Route | Method | Evidence | Validation |
| --- | --- | --- | --- | --- |
| 1 | R2 | Spend payload bits on better code-length permutations | 108/365 distinct completed Default streams improved; 607 incremental bits beyond full header repricing | [Production validation and bounds](payload-header-tradeoff-validation.md) |
| 2 | R3 | Search the advertised literal/length span | After the payload/header tradeoff pass, three distinct raw streams saved four meaningful bits and one byte | [Production validation and bounds](literal-span-validation.md) |
| 3 | R4 | Joint data-tree and RLE dynamic programming | After R3, 160/365 distinct completed Default streams save 3,344 bits and 419 bytes | [Production solver, oracle, bounds and validation](joint-tree-rle-validation.md) |
| 4 | R5 | Remove a set of symbols to simplify the header | After R4, 27/365 streams save 860 bits and 113 bytes; nine streams retain 49 bits beyond combined existing-route/single-symbol controls | [Production validation and bounds](symbol-set-validation.md) |
| 5 | R6, Max only | Discover cuts from alphabet changes | Nine real streams retain 377 bits beyond old anchors with Max table prices; fresh Max A/Bs improve six of eight cases | [Production bounds, cost policy and validation](alphabet-boundary-validation.md) |
| 6 | R7, Max only | Joint code-length tree and RLE search | Exact fixed-tree/spans solver; 22 Default parents and 20 frozen Max parents improve beyond full header repricing | [Solver, oracle and validation](code-length-tree-validation.md) |
| 1, extension | R8, Max only | Rotate three payload code lengths together | 30 Default-plus-R7 parents and 29 frozen Max parents improve; pair-swap barriers and seven public Max wins | [Bounds, controls and validation](code-length-rotation-validation.md) |
| 1, extension | R9, Max only | Couple literal/length and distance swaps | 43 controlled Default parents and 42 controlled Max parents improve; a real single-swap local optimum saves three bits | [Bounds, controls and validation](coupled-code-length-swaps-validation.md) |
| [Header-first synthesis](novel-deflate-formulations.md) | R10, Max only | Fit certified match spellings to proposed trees | Five of seven frozen Max parents improve; two joint moves beat both broader single-tree enumeration and the unchanged-tree response | [Bounds, controls and validation](header-response-validation.md) |
| Header-first synthesis | R11, Max only | Reassign length codes while repairing their certified payload | Seven of eight frozen Max fixed points save eighteen bits and one byte; five wins leave the reassigned code unused | [Bounds, controls and validation](length-symbol-exchange-validation.md) |

R2–R5 run in Default and Max. The [route catalog](../routes-and-methods.md#route-gate-reference) gives each route's current gates and bounds.

## 1. Spend payload bits on better code-length permutations

Swap two symbols' code lengths even when the payload grows by `tax = (frequency[a] - frequency[b]) × (length[b] - length[a])`, if the new length sequence is cheaper to describe in the header; the histogram of lengths is unchanged. Implemented as R2 ([validation](payload-header-tradeoff-validation.md)), and extended by three-symbol rotations (R8) and coupled literal/length and distance swaps (R9).

## 2. Search the advertised literal/length span

Append zero lengths to the literal/length list, up to the 286 entries [RFC 1951 §3.2.7](https://www.rfc-editor.org/rfc/rfc1951#section-3.2.7) allows, so the zero run at the seam with the distance list can get a cheaper RLE spelling; no payload codeword changes. Implemented as R3 ([validation](literal-span-validation.md)).

## 3. Optimize data-tree lengths and RLE together

For one fixed code-length tree and fixed spans, a dynamic program over Kraft capacity chooses the data-code lengths and their RLE tokens together. Implemented as R4 ([validation](joint-tree-rle-validation.md)).

## 4. Remove symbol sets chosen for their header effect

Remove two to four length or distance symbols as a unit when removing their support creates a zero run, removes an isolated nonzero code length or simplifies a transmitted alphabet tail, even if no member wins alone. Affected matches become literals or proven submatches at their original distance. Implemented as R5 ([validation](symbol-set-validation.md)).

## 5. Discover cuts from alphabet changes

Add block-boundary anchors at the first and last occurrences of rare high symbols and around clusters of length/distance symbols to the alignment-aware boundary graph. Implemented in Max as R6 ([validation](alphabet-boundary-validation.md)); a Default placement was rejected for its runtime cost.

## 6. Search the header tree directly

For fixed data-code lengths and spans, enumerate every useful complete seven-bit code-length tree jointly with its RLE through a 128-unit Kraft-capacity dynamic program. Implemented in Max as R7 ([validation](code-length-tree-validation.md)).

## Probe results and limits

The probe extracted zlib-valid IDAT streams from 174 top-level PngSuite PNG inputs, representing **127 distinct input streams**. It optimized each as a **standalone raw stream in Default mode**, then inspected 126 final dynamic blocks. This does not reproduce PNG-specific scheduling or constitute a Max comparison.

| Probe | Complete-stream wins | Physical bytes saved | Meaningful bits saved |
| --- | ---: | ---: | ---: |
| Literal-span extension | 3 | 0 | 3 |
| Positive-payload swaps versus completed Default | 21 | 15 | 129 |
| Swap contribution beyond existing full header repricing | 21 | 14 | 120 |
| Strict filler-code relocation | 0 | 0 | 0 |

The swap probe examined swaps costing 1–18 payload bits that reduced adjacent length transitions, and exactly priced at most 32 proposals per alphabet, ranked by the heuristic `tax - 3 × transitions_removed` (an ordering, not a bound or acceptance test). It evaluated 3,126 candidates and found 136 locally winning swaps. The literal-span wins were `basi0g04`, `basi4a16` and `bgai4a16`, one meaningful bit each beyond both the completed Default output and full existing header repricing, with physical bytes tied; they include a duplicate stream pair.

The swap total includes 9 bits already recoverable by the existing full header repricer on `basi6a16`; the third row removes that contribution. Rows are alternative comparisons, not additive savings. Keeping the best measured method per input yields **22 improved inputs, 15 bytes and 130 meaningful bits**, representing 18 distinct winning output streams. Related and duplicate fixtures prevent treating these counts as independent workload evidence.

Every emitted rewrite was reparsed by Columbo, checked for strict tree compatibility, and compared for decoded identity. Python's independent zlib decoder verified the 22 retained candidate outputs against the original streams. The probe emitted no PNG containers and did not change production source. The isolated swap test took 1.05 seconds, including reads, parsing, candidate generation, 3,126 complete header prices, emission and checks, but not compilation or Default baseline generation; this is a single-run observation, not a production overhead measurement.

**Filler relocation found nothing.** It tried all 435 pairs of usable one-bit distance-code positions for literal-only blocks, or all 29 alternate filler positions for a singleton used distance alphabet. Across 14 eligible blocks it produced no gain; with the earlier broad unused-symbol graft also showing no real gains, the idea stays low priority.

### Reproduction

The local exploratory files are the driver (`work/run-header-invention-probe.py`), the injected Rust tests (`work/header-invention-probe.rs`), the run log (`work/header-invention-probe.log`), span/filler results (`work/header-invention-probe-results.csv`), and swap results (`work/header-invention-swap-results.csv`). They live in the repository's ignored `work` area and require the private fixture corpus. The driver copies source into a temporary directory before injecting tests.

Run `python3 work/run-header-invention-probe.py` from the repository. The code inspected for these results had these SHA-256 values:

```text
header.rs    26c21de89101024484740eb00701f9f360ef57948fcbb89828824e4e56af2a97
search.rs    63cfcb13590e3c07a53715f70d5813eeff2454340337fd6a3e6b88ba9d7a5c58
optimize.rs  906ca286df6ebe4cdc8cc8320ce22d3d7e1d85a04546259d461afcb4a6637417
```

Further proposals follow the [validation policy](future-deflate-methods.md#validation-policy).
