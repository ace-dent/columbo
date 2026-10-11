# Alternative formulations for Deflate post-optimization

5 September 2026. Research proposals for Columbo, each with a source gap and a decisive experiment. Equality saturation, parametric optimization and program synthesis have prior literature; the proposed contribution is their formulation under Columbo's constraints. Source gaps describe the code on that date.

Status:

- Proposal 1: a bounded form is implemented as R1 ([validation](permanent-match-proofs-validation.md)), restoring original matches under a completed candidate's unchanged trees.
- Proposal 4: bounded tree-first instances are implemented in Max as R10 ([validated match-response gains](header-response-validation.md)) and R11 ([validated length-symbol-exchange gains](length-symbol-exchange-validation.md)).
- The broader response-envelope (2), neutral-exchange (3) and serialized-header-grammar (4) formulations remain unvalidated as compression improvements.

The most promising change is to make original match proofs permanent, so search can try alternative spellings without losing them; this allows reversible search and better handling of alternatives whose immediate cost ties. The [earlier research memo](new-byte-saving-methods.md) holds incremental candidates and measured gains.

## The permitted search space

Record each original match as a certificate `(decoded start, decoded end, distance)`. A generated match must lie wholly inside one such interval and use that certificate's distance. Existing adjacent same-distance coalescing can provide the already-permitted wider run certificate. A literal edge emits the already-known byte. An original literal or stored byte gets no match certificate merely because equal bytes exist elsewhere.

Rewriting a match as literals must not delete its certificate. Reusing the certificate later restores an already-proven choice; it does not search history or discover a distance. No transitive equality reasoning may manufacture a different distance or extend an interval beyond its certificate.

Block choices and codewords remain ordinary Deflate, and each dynamic block transmits its own trees. Strict compatibility, canonical length-258 spelling, exact byte-first/meaningful-bit comparison and decoded identity remain required. Certificates never authorize changes to the decoded stream.

## 1. Preserve proofs and make token search reversible

### The new representation

Keep an immutable graph over decoded positions for the lifetime of one optimization. Its edges are literal emissions and certified match emissions. Candidate token streams are paths through that graph; they do not decide which paths may be tried next.

Source-derived bounds can keep graph edges implicit: a certificate of length at most 258 describes its submatch edges without materializing every partition. Existing same-distance runs need their own bounded treatment.

An earlier match-to-literal rewrite may make a different tree attractive, and that tree may favor restoring some original matches. A reversible graph permits this directly, including partial restoration across boundaries that earlier rewrites created within one certificate.

### Concrete source gap

`ParsedBlock` and `PlannedBlock` retain the selected token/plain arrays, without a separate immutable original-match certificate map. [`parsed_block_from_plan`](../../src/deflate/search.rs) and [`parsed_from_selected_plan`](../../src/deflate/stream.rs) construct later states from the selected tokens. `solve_proven_submatch` requires a `Token::Match` as its source, so a token that is now a literal does not expose the match that previously covered it.

Columbo keeps independent original and endpoint lineages, so the optimizer as a whole does not lose its input. The gap is restoring selected original edges inside a later mixed state without restarting a lineage and rediscovering the same decisions. R1 recovers the certificates from the original parse for its terminal pass; intermediate search states still do not carry them.

### Why this could save bits

Token choices would no longer shrink as a search path expands or subdivides matches. A useful trajectory can expand A, rebuild the tree, restore a certified part of A, expand B, and split a block. Intermediate steps may be neutral or worse; only a completed candidate can replace the incumbent.

The related compiler idea is equality saturation: retain equivalent representations before extracting a profitable result. Deflate extraction also needs histograms, headers and alignment, so ordinary fixed-cost e-graph extraction is insufficient. A position/certificate DAG may be simpler than a general-purpose e-graph. [Tate et al., Equality Saturation](https://www.cs.cornell.edu/~lerner/papers/popl09.html)

### First decisive experiment

For completed compact candidates in which source matches became literals, carry the original certificates, freeze the current tree, re-solve the certified paths and reprice the complete blocks. Seek a final win that restores an original edge absent from the selected tokens and unreached by existing endpoint lineages under equal work. This tests preserved proofs alone, before any global graph extraction; stop if it neither produces reproducible wins nor makes existing wins cheaper to find.

**Validation outcome:** given original proofs instead of selected-token proofs, an otherwise identical fixed-tree shortest-path solver improved 48 of 365 distinct streams, with zero losses and 156 incremental meaningful bits saved. A targeted version recovers the same aggregate bit benefit against completed Default without tree repricing. Actual wrappers and a non-timeout Max replay fixed-point witness also validate the mechanism. The [full report](permanent-match-proofs-validation.md) gives controls, byte rounding, runtime and limits. This validates retaining proofs, not general reversible graph search.

## 2. Represent each proven interval by its response to possible trees

### Replace one preferred spelling with a cost function

For a fixed block layout, let `P_i` be the certified spellings of interval `i`. Each spelling has a literal/length and distance frequency vector plus its extra-bit total. For a candidate pair of payload trees `T`, define:

`S_i(T) = min over p in P_i of [payload cost of p under T]`.

The complete dynamic-block objective can then be written as:

`C(T, R) = header cost(T, R) + fixed literals/EOB cost(T) + sum_i S_i(T)`.

Here `R` is a legal encoding of the data trees' code-length lists. A spelling needing a symbol absent from `T` has infinite cost, and exact extra-bit cost is included. The expression applies to a fixed set of disjoint certified intervals and fixed block boundaries; crossing a boundary requires clipping the intervals and recomputing their response functions.

The proposal keeps how `S_i` changes as code prices vary, instead of only the best spelling under the current tree, a source-symbol-free spelling and a few nearby alternatives.

### Useful mathematical property

For fixed trees, payload cost is affine in a spelling's frequency/extra-bit vector. If one vector is a convex combination of others, it cannot be strictly cheaper than all of them under that tree. The header cost is the same for every spelling under that fixed tree. An exact representation of the lower cost envelope therefore preserves the best achievable block cost under every covered tree without retaining every path.

Absent-symbol legality does not break this argument: if a nonnegative convex combination has zero frequency for a symbol, so does each vector with positive weight in it. A tree valid for the combined vector is therefore valid for those component vectors.

The property concerns attainable cost only. Equal-cost alternatives can have different positional effects after a split, which is why formulation 3 is separate. Nor does the argument allow merging states with different future proof availability.

### How it differs from present search

`solve_proven_submatch` optimizes under one supplied price model. The composition route keeps a bounded frequency beam and a small menu of spellings. A spelling can be poor under all those seed prices but optimal under another legal tree; a parametric response representation looks for these changes of winner.

### Bounded implementation

Begin with two varying symbol-code prices and freeze the rest. Enumerate their legal discrete values, including absence where applicable, and run the certified shortest-path oracle for each profile. Keep a minimum-cost witness for every profile, deduplicated by exact state within this fixed-layout problem. Try three or four variable prices only where the response changes.

A complete enumeration is exact for the price family it covers. Sampling price profiles is heuristic and does not justify an unrestricted pruning claim. The full response envelope can have exponentially many pieces, so reduced dimensions, edge reuse and measured state counts are essential.

### First decisive experiment

Find a compact interval whose response envelope exposes a spelling absent from the current menus, and whose use under a completely repriced legal tree improves the final stream. Compare against enlarging the existing beam by the same work. The benefit must come from a different response, not from more trials.

## 3. Exchange spellings at zero current cost to improve later partitions

### The invariant

Within a fixed block and fixed trees, token order does not change encoded length when literal/length counts, distance counts and extra-bit total are unchanged. It changes the actual bits and where costs occur.

Represent a local rewrite by its frequency/extra-bit delta, and search for pairs or small cycles of rewrites whose deltas sum to zero. These exchanges redistribute code usage across the decoded stream while preserving the current complete Huffman-block cost. They are not direct compression wins and must not be selected merely because they tie.

Evaluate an exchange together with a split, merge/reseat or region-specific tree choice; the complete combined result is the candidate.

### A concrete exchange

Two original matches, A and B, each cover 12 bytes at distance `d`. The following two states use only submatches inside their respective certificates:

```text
State U: A uses 3@d + 9@d; B uses 12@d.
State V: A uses 12@d;       B uses 3@d + 9@d.
```

Their whole-block counts and extra bits are identical. If A and B later fall in different blocks, those blocks have different local counts: a tree favoring lengths 3 and 9 can belong on A's side and a tree favoring length 12 on B's side.

The original bytes are never reordered; the exchange only changes which certified spelling each fixed decoded interval uses.

### Verified structural witness

A standalone Python witness (`work/neutral-spelling-witness.py`) constructs four valid Deflate streams decoding to the same 26 bytes. All payload and header trees are complete, and zlib independently verifies each stream. The results are in the witness output (`work/neutral-spelling-witness.json`):

| Representation | U | V |
| --- | ---: | ---: |
| One block, same tree | 1,168 bits / 146 bytes | 1,168 bits / 146 bytes |
| Same prescribed split and two prescribed trees | 2,314 bits / 290 bytes | 2,326 bits / 291 bytes |

The split makes both streams larger in this simple construction. It proves a 12-bit difference in continuation cost, not a compression gain or a Columbo regression: equal whole-block frequencies do not imply equal cost under later partitions.

### Concrete source gap and proposed search

[`same_proven_composition_frequency_state`](../../src/deflate/search.rs) identifies states by global frequencies and extra bits. That suits immediate fixed-block pricing but does not preserve every regional distribution for later boundary search. Existing boundary routes may recover some of them; the witness alone does not prove they miss one.

Bucket local rewrite deltas and look for exact opposites. Add a bounded search for three-way zero sums only if pair exchanges prove useful. Record their effects on prefix histograms at a small set of potential cuts. Under proposed regional trees, rank exchanges by the difference between their left/right costs, then evaluate the exchange and boundary change together.

Experiments compare equal-work runs against the existing graph's per-edge token search and require a final gain from a regional allocation the old route does not reach. This formulation discovers or cheaply retains structural alternatives; it does not enlarge the set of source-certified encodings.

## 4. Synthesize short headers first, then fit certified spellings to them

### Reverse the direction of construction

The ordinary direction starts with token frequencies, builds payload trees and encodes their lengths. Instead, search a grammar of compact **serialized dynamic headers**. Each completed valid header defines a pair of payload trees. The certified graph then gives the minimum payload cost under those trees, using the response functions above where useful.

The objective is:

`min over valid header programs H of [bits(H) + shortest certified payload under trees(H)]`.

“Program” here means Deflate's existing sequence of literal code lengths and repeat instructions, encoded under its ordinary code-length Huffman tree. Nothing new is added to the wire format.

This can propose a cheap tree description that no payload-frequency seed would generate. The tokens adapt to that header through proven spellings, and the tree need never be the payload-optimal Huffman tree for the resulting counts. Fixed and stored block incumbents remain available.

### Difference from the earlier joint-tree DP

The earlier fixed-token joint-tree proposal is implemented as R4 ([validation](joint-tree-rle-validation.md)): it fixes the tokens and one code-length price model, then chooses data-tree lengths and RLE. This formulation makes the compressed header itself the search object and lets the certified token spelling change with it. The code-length tree, advertised spans, repeat program, data trees and payload path can all vary within the chosen search family.

### Bounded tree-first follow-up

The [header-directed response pass](header-response-validation.md) (R10,
13 September) proposes valid parent-tree swaps and fits exact certified
spellings to them, keeping the proposed tree even when it is not
frequency-optimal for the new tokens. The
[length-symbol exchange](length-symbol-exchange-validation.md) (R11) moves a
used length code to an absent symbol before solving the payload. Five of its
seven Max improvements leave the new code unused, because the old tree shape
with a differently placed spare code has a cheaper header.

Both are limited joint tree/spelling moves. They do not synthesize serialized
header programs, enumerate an unrestricted response envelope or prove the
larger formulation above.

### Finite, bounded search

Start with one small block and an explicit bit target below its completed incumbent. Generate partial headers as grammar productions, tracking advertised counts, repeat legality and Kraft capacity, and reject impossible tree completions early. Bound the remaining payload optimistically using the certified graph; a bound may underprice unassigned codes but must never overprice a possible completion.

Merge equivalent completed tree states with exact hash verification and share their payload response. A production experiment needs a fixed grammar-state budget. Exhausting it loses completeness, but every completed valid candidate still has an exact price and the incumbent remains intact.

### First decisive experiment

On tiny final blocks, enumerate a small explicit family of low-instruction-count headers, then optimize certified spellings under each. Seek a complete result whose header/tree pair is absent from Columbo's existing families. Compare against R4 and equal-work broader tree enumeration.

If this wins, enlarge the header grammar based on demonstrated omissions. Without such a witness, a general grammar search is too expensive for production.

## An engine for discovering new transformations

A bounded solver can serve as an oracle behind these formulations. Ask whether a stream of at most `B - 1` meaningful bits exists when the complete incumbent costs `B`, within a precisely declared block/header/certificate domain. For byte-first comparisons or an interior replacement, formulate the actual enclosing byte/bit and alignment objective instead.

The solver can jointly choose token edges, a few block boundaries, Huffman lengths, advertised counts and header RLE. For a first experiment, bound decoded length, certificate lengths and block count tightly. A timeout means unknown. An unsatisfiable result proves only the stated bounded problem, and only if every permitted alternative in it was encoded correctly.

Decoded equivalence alone is insufficient: the constraints must require an original interval/distance certificate for every match edge, or the solver silently becomes an out-of-scope match finder.

When it finds a better encoding, minimize the difference into a small witness, identify the interacting choices and turn them into a cheap candidate-generation rule. Validate the rule's structural preconditions and exact-price every real candidate. This can find combinations the named routes never propose while keeping an expensive solver out of normal runs.

Synthesizing optimizer transformations is established outside compression; Souper is an example. The proposed contribution is a proof-restricted Deflate objective and the resulting transformation rules, not superoptimization itself. [Souper: A Synthesizing Superoptimizer](https://research.google/pubs/souper-a-synthesizing-superoptimizer/)

## Recommended order and acceptance

1. Bounded terminal restoration from original certificates is implemented as R1 in `src/deflate/restore.rs` ([validation](permanent-match-proofs-validation.md)). Broader reversible search remains open.
2. Test a bounded neutral-exchange-plus-boundary route. Its structural premise has a verified witness; corpus value remains unknown.
3. Measure two-price response envelopes on the same intervals. Establish whether omitted spellings win after exact repricing.
4. Build the smallest header-first oracle that can jointly change a header and a certified spelling. Use its counterexamples to discover cheap rules.

The four formulations overlap in the encodings they can eventually reach, so their savings must not be added as independent effects. Assess each against the current optimizer and the other formulations under equal work. Preserve complete incumbents, verify original-certificate containment and distances, and independently decode every retained result. Record whole-file bytes, meaningful bits, unique-stream coverage and runtime.
