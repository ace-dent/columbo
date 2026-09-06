# Alternative formulations for Deflate post-optimization

5 September 2026. These are original research proposals developed for Columbo, with source-grounded gaps and explicit experiments. No worldwide novelty claim is made. The general ideas of equality saturation, parametric optimization, and program synthesis have prior literature; the proposed contribution is their particular formulation under Columbo's constraints. A subsequent [validation of permanent match proofs](permanent-match-proofs-validation.md) demonstrates corpus gains from a bounded form of proposal 1. The other formulations remain unvalidated as compression improvements.

The most promising architectural change is to make the original match proofs permanent, then optimize over their alternative spellings without losing those proofs. That enables both reversible search and more principled handling of alternatives whose immediate cost ties. The first experiment now supports a smaller initial implementation: restore original matches under a completed candidate's unchanged trees.

The [earlier research memo](new-byte-saving-methods.md) contains incremental candidates and measured gains. This note develops four different search formulations, plus an engine for discovering new transformations automatically.

## The permitted search space

Record each original match as a certificate `(decoded start, decoded end, distance)`. A generated match must lie wholly inside one such interval and use that certificate's distance. Existing adjacent same-distance coalescing can provide the already-permitted wider run certificate. A literal edge emits the already-known byte. An original literal or stored byte receives no match certificate merely because equal bytes exist elsewhere.

Rewriting a match as literals must not delete its original certificate. Reusing that certificate later restores an already-proven choice: it does not search history or discover a distance. No transitive equality reasoning may manufacture a different distance or extend an interval beyond the permitted certificate.

Block choices and codewords remain ordinary Deflate. Each dynamic block transmits its own trees. Strict compatibility, canonical length-258 spelling, exact byte-first/meaningful-bit comparison, and decoded identity remain required. Certificates do not authorize changes to the decoded stream.

## 1. Preserve proofs and make token search reversible

### The new representation

Keep an immutable graph over decoded positions for the lifetime of one optimization. Its edges are literal emissions and certified match emissions. Candidate token streams are paths through that graph; they are not the authority defining which paths may be tried next.

Source-derived bounds can keep graph edges implicit. A certificate of length at most 258 describes its submatch edges without materializing every possible partition. Existing same-distance runs require their own bounded treatment.

This changes what a later optimization step can see. An earlier match-to-literal rewrite may make a different tree attractive; that tree may then favor restoring some of the original matches. A reversible graph permits this mixture directly, including partial restoration across boundaries created by earlier rewrites within the same certificate.

### Concrete source gap

`ParsedBlock` and `PlannedBlock` retain the selected token/plain arrays, without a separate immutable original-match certificate map. [`parsed_block_from_plan`](../../src/deflate/search.rs) and [`parsed_from_selected_plan`](../../src/deflate/stream.rs) construct later states from the selected tokens. `solve_proven_submatch` requires a `Token::Match` as its source. A token that is now a literal does not itself expose the match that previously covered it.

Columbo retains independent original and endpoint lineages, so this is not a claim that the whole optimizer discards its original input. The gap concerns restoring selected original edges inside a later mixed state without restarting an independent lineage and rediscovering the same sequence of decisions.

### Why this could save bits

The available token choices would cease to shrink when a search path expands or subdivides a match. A useful trajectory can expand A, rebuild the tree, restore a certified portion of A, expand B, and split a block. Intermediate steps can be neutral or worse; only a completed candidate can replace the incumbent.

The related compiler idea is equality saturation: retain equivalent representations before extracting a profitable result. Deflate extraction additionally needs histograms, headers, and alignment; ordinary fixed-cost e-graph extraction is insufficient. A position/certificate DAG may be simpler than a general-purpose e-graph. [Tate et al., Equality Saturation](https://www.cs.cornell.edu/~lerner/papers/popl09.html)

### First decisive experiment

Take completed compact candidates in which source matches became literals. Carry the original certificates alongside them, freeze each candidate's current tree, and solve its certified paths again. Reprice all resulting complete blocks with Columbo's existing planner. Seek a final win that restores at least one original edge unavailable from the selected token list, and that existing endpoint lineages do not reach under equal work.

This first test isolates the value of preserving proofs before attempting global graph extraction. Stop expanding the architecture if this additional reachability does not produce reproducible wins or improve the cost of finding existing wins.

**Validation outcome:** an identical fixed-tree shortest-path solver improved 48 of 365 distinct streams when given original proofs instead of selected-token proofs, with zero losses and 156 incremental meaningful bits saved. A targeted version recovers the same aggregate bit benefit against completed Default without tree repricing. Actual wrappers and a non-timeout Max replay fixed-point witness also validate the mechanism. See the [full report](permanent-match-proofs-validation.md) for controls, byte rounding, runtime, and limits. This validates retaining proofs; it does not yet validate general reversible graph search.

## 2. Represent each proven interval by its response to possible trees

### Replace one preferred spelling with a cost function

For a fixed block layout, let `P_i` be the certified spellings of interval `i`. Each spelling has a literal/length and distance frequency vector plus its extra-bit total. For a candidate pair of payload trees `T`, define:

`S_i(T) = min over p in P_i of [payload cost of p under T]`.

The complete dynamic-block objective can then be written as:

`C(T, R) = header cost(T, R) + fixed literals/EOB cost(T) + sum_i S_i(T)`.

Here `R` is a legal encoding of the data trees' code-length lists. A spelling needing a symbol absent from `T` has infinite cost. Exact extra-bit cost is included. This expression applies to a fixed set of disjoint certified intervals and fixed block boundaries; crossing a boundary requires clipping the intervals and recomputing their response functions.

The proposal is to retain the changes in `S_i` as code prices vary, rather than retaining only the best spelling under today's tree, a source-symbol-free spelling, and a few nearby alternatives.

### Useful mathematical property

For fixed trees, payload cost is affine in a spelling's frequency/extra-bit vector. If one vector is a convex combination of others, it cannot be strictly cheaper than all of them under that tree. The header cost is the same for all spellings evaluated under that fixed tree. Therefore an exact representation of the lower cost envelope can preserve the best achievable block cost under every covered tree without retaining every path.

Absent-symbol legality does not invalidate this argument: if a nonnegative convex combination has zero frequency for a symbol, each vector with positive mixture weight also has zero frequency for it. Thus a tree valid for that combined vector is valid for those component vectors.

The property is about attainable cost. Equal-cost alternatives can have different positional effects after a split, which is why formulation 3 is separate. Nor does the argument license merging states with different future proof availability.

### How it differs from present search

`solve_proven_submatch` optimizes under one supplied price model. The composition route keeps a bounded frequency beam and a small menu of spellings. A spelling can be poor under all those seed prices but optimal under another legal tree. A parametric response representation explicitly looks for these changes of winner.

### Bounded implementation

Begin with two varying symbol-code prices and freeze the remaining prices. Enumerate their legal discrete price values, including absence when applicable, and run the certified shortest-path oracle for each profile. Retain a minimum-cost witness for every profile, deduplicated by exact state within this fixed-layout problem. Then attempt three or four variable prices only where the response actually changes.

A complete enumeration is exact for its admitted price family. Sampling price profiles is heuristic; it does not justify an unrestricted pruning claim. The full response envelope can have exponentially many pieces, so reduced dimensions, edge reuse, and measured state counts are essential.

### First decisive experiment

Find a compact interval whose response envelope exposes a spelling absent from the current menus, and whose use under a completely repriced legal tree improves the final stream. Compare against increasing the existing beam by an equal amount of work. The benefit must come from discovering a different response, not merely doing more trials.

## 3. Exchange spellings at zero current cost to improve later partitions

### The invariant

Within a fixed block and fixed trees, token order does not change encoded length when literal/length counts, distance counts, and extra-bit total are unchanged. It changes the actual bits and the positions at which costs occur.

Represent a local rewrite by its frequency/extra-bit delta. Search for pairs or small cycles of rewrites whose deltas sum to zero. These exchanges redistribute code usage across the decoded stream while preserving the current complete Huffman-block cost. They are not direct compression wins and must not be selected merely because they tie.

Evaluate an exchange together with a split, merge/reseat, or region-specific tree choice. The complete combined result is the candidate.

### A concrete exchange

Two original matches, A and B, each cover 12 bytes at distance `d`. The following two states use only submatches inside their respective certificates:

```text
State U: A uses 3@d + 9@d; B uses 12@d.
State V: A uses 12@d;       B uses 3@d + 9@d.
```

Their whole-block counts and extra bits are identical. If A and B later occupy different blocks, those blocks have different local counts. A tree favoring lengths 3 and 9 can belong on A's side while a tree favoring length 12 belongs on B's side.

The original bytes are never reordered. The exchange changes which certified spelling is assigned to each fixed decoded interval.

### Verified structural witness

[A standalone Python witness](../../work/neutral-spelling-witness.py) constructs four valid Deflate streams decoding to the same 26 bytes. All payload and header trees are complete, and zlib independently verifies each stream. The results are in [the witness output](../../work/neutral-spelling-witness.json):

| Representation | U | V |
| --- | ---: | ---: |
| One block, same tree | 1,168 bits / 146 bytes | 1,168 bits / 146 bytes |
| Same prescribed split and two prescribed trees | 2,314 bits / 290 bytes | 2,326 bits / 291 bytes |

The split makes both streams larger in this deliberately simple construction. It proves a 12-bit difference in their continuation costs, not a compression gain or a Columbo regression. In particular, equality of whole-block frequencies does not justify assuming equality under future partitions.

### Concrete source gap and proposed search

[`same_proven_composition_frequency_state`](../../src/deflate/search.rs) identifies states using global frequencies and extra bits. That is appropriate for immediate fixed-block pricing. It does not preserve all possible regional distributions for subsequent boundary search. Existing boundary routes may independently recover some of them; the witness alone does not prove they miss one.

Bucket local rewrite deltas and look for exact opposites. Add a bounded search for three-way zero sums only if pair exchanges prove useful. Record their effects on prefix histograms at a small set of potential cuts. Under proposed regional trees, rank exchanges by the difference between their left/right costs, then evaluate the exchange and boundary change together.

The necessary experiments compare equal-work runs against the existing graph's own per-edge token search. Require a final gain associated with a regional allocation the old route does not reach. This formulation is primarily a way to discover or cheaply retain useful structural alternatives; it does not expand the mathematical set of source-certified encodings.

## 4. Synthesize short headers first, then fit certified spellings to them

### Reverse the direction of construction

The ordinary direction starts with token frequencies, builds payload trees, and encodes their lengths. Instead, search a grammar of compact **serialized dynamic headers**. Each completed valid header defines a pair of payload trees. The certified graph then answers the minimum payload cost under those trees, using the response functions above where useful.

The objective is:

`min over valid header programs H of [bits(H) + shortest certified payload under trees(H)]`.

“Program” here means Deflate's existing sequence of literal code lengths and repeat instructions, encoded under its ordinary code-length Huffman tree. Nothing new is added to the wire format.

This can propose a cheap tree description that no payload-frequency seed would generate. The tokens can adapt to that header through proven spellings, and the tree need never be the payload-optimal Huffman tree for the resulting counts. Fixed and stored block incumbents remain available.

### Difference from the earlier joint-tree DP

The earlier fixed-token joint-tree proposal is now [implemented and validated](joint-tree-rle-validation.md): it fixes the tokens and one code-length price model, then chooses data-tree lengths and RLE. This formulation treats the compressed header itself as the search object and lets the certified token spelling change with it. The code-length tree, advertised spans, repeat program, data trees, and payload path can all vary within the admitted search family.

### Finite, bounded search

Start with one small block and an explicit bit target below its completed incumbent. Generate partial headers as grammar productions, tracking advertised counts, repeat legality, and Kraft capacity. Reject impossible tree completions early. Bound the remaining payload optimistically using the certified graph; a bound may underprice unassigned codes but must never overprice a possible completion.

Use exact hash verification to merge equivalent completed tree states and share their payload response. Set a fixed grammar-state budget for a production experiment. This loses completeness when exhausted, but every completed valid candidate still has an exact price and the incumbent remains intact.

Searching arbitrary short bitstrings directly would be wasteful. The interesting engineering work is generating only potentially useful valid header structures and sharing partial feasibility calculations.

### First decisive experiment

On tiny final blocks, enumerate an explicit small family of low-instruction-count headers, then optimize certified spellings under each. Seek a complete result whose header/tree pair was absent from Columbo's existing families. Compare against the earlier fixed-token joint-tree proposal and equal-work broader tree enumeration.

If this wins, enlarge the header grammar based on demonstrated omissions. Without such a witness, a general grammar search is too expensive to justify in production.

## An engine for discovering new transformations

A bounded solver can serve as an oracle behind these formulations. Ask whether a stream of at most `B - 1` meaningful bits exists when the complete incumbent costs `B`, within a precisely declared block/header/certificate domain. For byte-first comparisons or an interior replacement, formulate the actual enclosing byte/bit and alignment objective instead.

The solver can jointly choose token edges, a small number of block boundaries, Huffman lengths, advertised counts, and header RLE. For a first experiment, bound decoded length, certificate lengths, and block count tightly. A timeout means unknown. An unsatisfiable result proves only the stated bounded problem, and only if every permitted alternative in that problem was encoded correctly.

Decoded equivalence alone is insufficient: the constraints must require that every match edge has an original interval/distance certificate. Otherwise a solver would silently become an out-of-scope match finder.

When it finds a better encoding, minimize the difference into a small witness, identify the interacting choices, and turn those choices into a cheap candidate-generation rule. Validate the rule's structural preconditions and exact-price every real candidate. This could discover combinations that the named routes never propose, while keeping an expensive solver out of normal runs.

Synthesizing optimizer transformations is established outside compression; Souper is a relevant example. The proposed research contribution is a proof-restricted Deflate objective and the resulting transformation rules, not the invention of superoptimization. [Souper: A Synthesizing Superoptimizer](https://research.google/pubs/souper-a-synthesizing-superoptimizer/)

## Recommended order and acceptance

1. Bounded terminal restoration using original certificates is now implemented in `src/deflate/restore.rs`. Broader reversible search remains separate; the [validation report](permanent-match-proofs-validation.md) records the evidence and production follow-up.
2. Test a bounded neutral-exchange-plus-boundary route. Its structural premise has a verified witness; corpus value remains unknown.
3. Measure two-price response envelopes on the same intervals. Establish whether omitted spellings win after exact repricing.
4. Build the smallest header-first oracle that can jointly change a header and a certified spelling. Use its counterexamples to discover cheap rules.

The four formulations overlap in the encodings they can eventually reach; their savings must not be added as independent effects. Assess each against the current optimizer and the other formulations under equal work. Preserve complete incumbents, verify original-certificate containment and distances, and independently decode every retained result. Record whole-file bytes, meaningful bits, unique-stream coverage, and runtime. No production source was changed for this research.
