# Turtledeflate methods and their relevance to Columbo

This document audits Turtledeflate's current `main` source, identifies the
methods that influenced Columbo, and separates compatible Deflate-stream
optimizations from fresh compression.

The audited snapshot is commit
[`756f844d0cb0cb0accb5cca6ce08bf7a6b9a6fa1`](https://github.com/rwillenbacher/turtledeflate/tree/756f844d0cb0cb0accb5cca6ce08bf7a6b9a6fa1),
dated 9 March 2026. Links below are pinned to that commit so later upstream
changes cannot silently alter the evidence.

## Conclusions

Turtledeflate is a compressor for raw input bytes, not an optimizer for an
existing Deflate stream. Its principal compression gains come from discovering
matches, repeatedly changing the LZ77 parse, and feeding new block boundaries
back through the compressor. Those methods are outside Columbo's scope.

Columbo independently implements two related concepts also used by
Turtledeflate's block splitter:

1. cumulative symbol-frequency checkpoints at a 256-token stride; and
2. a histogram-guided, locally smoothed, coarse-to-fine split search.

The checkpoint representation is used by Columbo's general range planning.
The adaptive splitter itself is an optional `--max` route. Columbo does not
copy or translate Turtledeflate code: it retains the parsed token stream, uses
different data structures, cost models, and search constants, applies hard
work limits, and verifies a proposed boundary with its exact planner.

Turtledeflate also uses `OptimizeHuffmanForRle`, but its source explicitly
identifies that function as Google Zopfli code. Columbo's logarithmic
adjacency-aware pseudo-frequency method is original and should not be labelled
as a Turtledeflate method.

No additional Turtledeflate route should be added to normal/default Columbo
without evidence of a broad net benefit and no material speed loss. The most
plausible future `--max` experiments are retaining one second split basin and
performing one bounded adjacent-boundary reseat.

## Evidence and attribution policy

The upstream C source is authoritative for implementation details. The README
is used for the author's design and performance commentary, but active source
controls where the prose and implementation differ.

The following labels are used:

- **conceptually incorporated**: Columbo independently implements a method
  concept also used upstream, without copying or translating source code;
- **inspired**: the source informed the design, but Columbo has independent
  code, data structures, limits, costs, and acceptance rules;
- **independent or superseded**: both programs perform related work, but
  Columbo's method predates the audit, has different provenance, or covers a
  wider search;
- **candidate**: compatible with frozen existing-stream tokens but not
  incorporated; and
- **out of scope**: requires match discovery or a fresh LZ77 parse.

Generic RFC 1951 operations, such as canonical-code assignment or the existence
of code-length repeat symbols 16, 17, and 18, are not treated as uniquely
Turtledeflate-inspired.

## Product and scope

Turtledeflate describes itself as a slow Deflate compressor and says that its
results depend on repeated path tracing and precise block boundaries
([README lines 3-17](https://github.com/rwillenbacher/turtledeflate/blob/756f844d0cb0cb0accb5cca6ce08bf7a6b9a6fa1/README.md#L3-L17)).
The command-line program opens an ordinary input file and feeds raw bytes to
`turtledeflate_block`
([application lines 205-288](https://github.com/rwillenbacher/turtledeflate/blob/756f844d0cb0cb0accb5cca6ce08bf7a6b9a6fa1/src/turtledeflate_app.c#L205-L288)).
It never parses an existing Deflate stream.

Its final writer emits dynamic blocks directly; it does not compare stored,
fixed, and dynamic output representations
([block writer lines 2419-2438](https://github.com/rwillenbacher/turtledeflate/blob/756f844d0cb0cb0accb5cca6ce08bf7a6b9a6fa1/lib/turtledeflate_block.c#L2419-L2438)).

Columbo starts from a parsed Deflate stream. It may rebuild trees, change block
boundaries at legal token boundaries, and apply its documented token-preserving
or token-simplifying routes. It does not search the decoded bytes for new
matches. That distinction governs every classification below.

## High-level Turtledeflate pipeline

A simplified view of the active compressor is:

```text
for each raw-input superblock:
    start with one block

    repeat:
        for each current block:
            enumerate matches from decoded bytes
            make a greedy parse
            refine the parse under several frequency/cost models
        concatenate the resulting LZ77 tokens
        build cumulative token histograms
        split, merge, and reseat token-range boundaries
        map selected token boundaries back to raw-byte positions
        retain the best complete state or roll back a worse result
        recompress the new raw-byte partitions

    write the best token ranges as dynamic Deflate blocks
```

The outer feedback loop is visible in
[`turtledeflate_block`](https://github.com/rwillenbacher/turtledeflate/blob/756f844d0cb0cb0accb5cca6ce08bf7a6b9a6fa1/lib/turtledeflate.c#L74-L310).
The important scope boundary is the recompression step: a changed boundary
causes the raw bytes to be parsed again, so later token streams need not retain
the original matches.

## Match discovery and parse optimization

### Hash-chain match enumeration

Turtledeflate uses a rolling three-byte hash and a ring of earlier positions in
the 32 KiB window. It searches the chain for the longest match and records a
distance for every attainable sub-length
([block source lines 339-503](https://github.com/rwillenbacher/turtledeflate/blob/756f844d0cb0cb0accb5cca6ce08bf7a6b9a6fa1/lib/turtledeflate_block.c#L339-L503)).

This is fresh match discovery and is therefore out of scope for Columbo.

### Greedy and lazy seed

The initial parse takes the longest current match unless the next byte starts a
longer one. It also requires a minimum length of four for distances above 1024
([lines 552-607](https://github.com/rwillenbacher/turtledeflate/blob/756f844d0cb0cb0accb5cca6ce08bf7a6b9a6fa1/lib/turtledeflate_block.c#L552-L607)).

This selects a new LZ77 representation from raw bytes and is out of scope.

### Iterative trellis parse

For every decoded position, Turtledeflate considers a literal and every
available match sub-length. Edge costs come from either:

- quantized Shannon estimates derived from a current frequency model; or
- exact Huffman code lengths derived from that model.

The trellis keeps two incoming choices at an endpoint. An optional backtrace can
replace two primary matches with a second two-match decomposition of the same
decoded length when the alternative has favourable length symbols and no worse
length extra-bit cost
([cost and trellis lines 757-1088](https://github.com/rwillenbacher/turtledeflate/blob/756f844d0cb0cb0accb5cca6ce08bf7a6b9a6fa1/lib/turtledeflate_block.c#L757-L1088)).

These routes depend on the match set discovered from raw bytes. They are
recompression, not existing-stream optimization.

### Precision-diverse and perturbed models

The README calls the final parse slightly randomizing, but the implementation
does not use a random-number generator. It starts deterministic model paths at
different fixed-point precisions, advances their precision in stages,
eliminates identical frequency models, perturbs models toward uniform
frequencies at several weights, and finishes with exact refinement
([lines 1282-1647](https://github.com/rwillenbacher/turtledeflate/blob/756f844d0cb0cb0accb5cca6ce08bf7a6b9a6fa1/lib/turtledeflate_block.c#L1282-L1647)).

The directly pinned ranges are:

- [model perturbation lines 1282-1341](https://github.com/rwillenbacher/turtledeflate/blob/756f844d0cb0cb0accb5cca6ce08bf7a6b9a6fa1/lib/turtledeflate_block.c#L1282-L1341);
- [duplicate-model elimination lines 1344-1395](https://github.com/rwillenbacher/turtledeflate/blob/756f844d0cb0cb0accb5cca6ce08bf7a6b9a6fa1/lib/turtledeflate_block.c#L1344-L1395); and
- [precision-diverse search lines 1399-1647](https://github.com/rwillenbacher/turtledeflate/blob/756f844d0cb0cb0accb5cca6ce08bf7a6b9a6fa1/lib/turtledeflate_block.c#L1399-L1647).

The parse-changing machinery is out of scope. De-duplicating equivalent
candidates before exact evaluation is a broadly useful engineering principle,
but it is not a distinct compression method to port.

## Huffman and dynamic-header methods

### Length-limited Huffman construction

Turtledeflate builds an unconstrained frequency tree, caps overlong lengths with
a zlib-style count redistribution, reassigns the resulting length histogram in
its recorded merge order, and constructs canonical codes
([tree source lines 42-343](https://github.com/rwillenbacher/turtledeflate/blob/756f844d0cb0cb0accb5cca6ce08bf7a6b9a6fa1/lib/turtledeflate_tree.c#L42-L343)).

Rebuilding a Huffman tree for existing token frequencies is in scope. Columbo
already prices several independently sourced tree builders, including its
generic, order-sensitive, DeflOpt, Defluff, and deft4j families. The ordinary
Turtledeflate result is likely duplicated by an existing family; its
over-depth repair and leaf reassignment may still produce a unique candidate.

Status: not incorporated. Differential testing should demonstrate unique wins
before another tree family is added to any production route.

### Eight code-length-RLE masks

Turtledeflate can independently enable or disable repeat symbols 16, 17, and
18, tries all eight subsets, and keeps the shortest encoded dynamic header
([encoder lines 346-512](https://github.com/rwillenbacher/turtledeflate/blob/756f844d0cb0cb0accb5cca6ce08bf7a6b9a6fa1/lib/turtledeflate_tree.c#L346-L512),
[selection lines 2475-2486](https://github.com/rwillenbacher/turtledeflate/blob/756f844d0cb0cb0accb5cca6ce08bf7a6b9a6fa1/lib/turtledeflate_block.c#L2475-L2486)).

Columbo also evaluates the eight enable/disable masks, but that route existed
in the original Columbo C implementation and its header planner now explores a
wider family. This is independent convergence, not Turtledeflate provenance.

### RLE-friendly pseudo-frequencies are Zopfli-derived

Turtledeflate copies `OptimizeHuffmanForRle` from Google Zopfli. The file
retains Zopfli's Apache-2.0 notice and authorship
([notice lines 1-19](https://github.com/rwillenbacher/turtledeflate/blob/756f844d0cb0cb0accb5cca6ce08bf7a6b9a6fa1/lib/turtledeflate_block.c#L1-L19),
[function lines 230-335](https://github.com/rwillenbacher/turtledeflate/blob/756f844d0cb0cb0accb5cca6ce08bf7a6b9a6fa1/lib/turtledeflate_block.c#L230-L335)).
The README confirms that provenance
([lines 105-110](https://github.com/rwillenbacher/turtledeflate/blob/756f844d0cb0cb0accb5cca6ce08bf7a6b9a6fa1/README.md#L105-L110)).

Turtledeflate's final writer builds one tree from the true counts and another
from the RLE-friendly pseudo-counts. Its block-cost estimator also tries both
when the configured compression level is above seven. In either route, the
payloads are priced with the true frequencies, so a misleading pseudo-count
reduction cannot be accepted without a complete bit-cost improvement
([estimator lines 715-751](https://github.com/rwillenbacher/turtledeflate/blob/756f844d0cb0cb0accb5cca6ce08bf7a6b9a6fa1/lib/turtledeflate_block.c#L715-L751),
[writer lines 2441-2465](https://github.com/rwillenbacher/turtledeflate/blob/756f844d0cb0cb0accb5cca6ce08bf7a6b9a6fa1/lib/turtledeflate_block.c#L2441-L2465)).

Columbo's `make_columbo_rle_pseudofrequencies` does not implement that
algorithm. It uses logarithmic buckets and changes a nonzero count only when an
adjacent symbol shares its bucket. Columbo's quantizer, tree construction,
complete header search, and strict scoring are original. The correct
provenance is:

```text
Zopfli pseudo-frequency concept, also used by Turtledeflate;
Columbo-original adjacency-aware quantizer and candidate routing.
```

An exact translation of `OptimizeHuffmanForRle` would be technically in scope
and cheap enough to benchmark, but it would be a Zopfli-derived route and would
need the applicable Apache-2.0 attribution. It must not be described as a
Turtledeflate invention.

## Turtledeflate's block splitter

The block splitter is the part of Turtledeflate most relevant to Columbo.
Turtledeflate runs it over the token stream produced by its current compression
pass. Boundaries are token indices, even though some upstream variable names
call them "deflated" boundaries.

### 256-token cumulative histograms

Turtledeflate snapshots cumulative literal/length and distance frequencies
every 256 LZ77 tokens
([constant and type lines 93-98](https://github.com/rwillenbacher/turtledeflate/blob/756f844d0cb0cb0accb5cca6ce08bf7a6b9a6fa1/lib/turtledeflate.h#L93-L98),
[builder lines 1650-1687](https://github.com/rwillenbacher/turtledeflate/blob/756f844d0cb0cb0accb5cca6ce08bf7a6b9a6fa1/lib/turtledeflate_block.c#L1650-L1687)).

For an arbitrary token range, it:

1. scans from the range start to the next aligned checkpoint;
2. subtracts two cumulative checkpoints for the aligned middle; and
3. scans the unaligned tail.

It then inserts the end-of-block count and estimates the block cost
([range query and cost lines 1690-1757](https://github.com/rwillenbacher/turtledeflate/blob/756f844d0cb0cb0accb5cca6ce08bf7a6b9a6fa1/lib/turtledeflate_block.c#L1690-L1757)).

### Coarse-to-fine, locally smoothed boundary search

For a sufficiently large range, `turtledeflate_best_block_split`:

1. samples 32 evenly spaced candidate positions with the default settings;
2. prices the left and right token histograms at every sample;
3. computes a moving sum around each sampled cost, using a default radius of
   three sample slots;
4. selects the lowest smoothed basin;
5. narrows the position range to the samples eight slots either side of that
   basin;
6. repeats until the remaining range is no more than 1024 tokens; and
7. exhaustively prices every split in the final range.

If the selected basin lies near an edge, the function may save a range on the
far side of a separating maximum and search that alternate basin later.

The complete method is
[`turtledeflate_best_block_split`, lines 1791-1982](https://github.com/rwillenbacher/turtledeflate/blob/756f844d0cb0cb0accb5cca6ce08bf7a6b9a6fa1/lib/turtledeflate_block.c#L1791-L1982).
Its defaults are defined in
[`turtledeflate_api.h`, lines 48-57](https://github.com/rwillenbacher/turtledeflate/blob/756f844d0cb0cb0accb5cca6ce08bf7a6b9a6fa1/inc/turtledeflate_api.h#L48-L57)
and initialized by the application
([lines 121-134](https://github.com/rwillenbacher/turtledeflate/blob/756f844d0cb0cb0accb5cca6ce08bf7a6b9a6fa1/src/turtledeflate_app.c#L121-L134)).

### Split, merge, and boundary reseating

Turtledeflate maintains cached costs and local invalidation flags for each
current subblock. It repeatedly:

- reseats an existing boundary by joining two adjacent ranges and finding a
  better split across their union;
- chooses the globally best profitable split; and
- chooses the globally best profitable adjacent merge.

Boundary reseating alternates even and odd adjacent pairs so one pass can move
non-overlapping boundaries before the next parity is examined
([reseat lines 2019-2069](https://github.com/rwillenbacher/turtledeflate/blob/756f844d0cb0cb0accb5cca6ce08bf7a6b9a6fa1/lib/turtledeflate_block.c#L2019-L2069)).
The split, merge, and driver are at:

- [split lines 2072-2195](https://github.com/rwillenbacher/turtledeflate/blob/756f844d0cb0cb0accb5cca6ce08bf7a6b9a6fa1/lib/turtledeflate_block.c#L2072-L2195);
- [merge lines 2198-2260](https://github.com/rwillenbacher/turtledeflate/blob/756f844d0cb0cb0accb5cca6ce08bf7a6b9a6fa1/lib/turtledeflate_block.c#L2198-L2260); and
- [driver lines 2278-2324](https://github.com/rwillenbacher/turtledeflate/blob/756f844d0cb0cb0accb5cca6ce08bf7a6b9a6fa1/lib/turtledeflate_block.c#L2278-L2324).

### Forced-split escape

In early splitter states, Turtledeflate can clone the current post-search
state, force a locally losing split, reseat the resulting boundaries, and
retain a later global improvement. It stops after five consecutive forced
attempts fail to improve the best state
([lines 2331-2378](https://github.com/rwillenbacher/turtledeflate/blob/756f844d0cb0cb0accb5cca6ce08bf7a6b9a6fa1/lib/turtledeflate_block.c#L2331-L2378)).

The README says this usually wastes runtime but occasionally finds more and
better blocks
([lines 113-119](https://github.com/rwillenbacher/turtledeflate/blob/756f844d0cb0cb0accb5cca6ce08bf7a6b9a6fa1/README.md#L113-L119)).
That tradeoff conflicts with Columbo's speed-first default policy.

## Methods incorporated into Columbo

### Strided range-frequency index

Columbo's [`Composite`](../src/deflate/stream.rs) builds
`frequency_checkpoints`, and its `prefix_frequencies` and `range_frequencies`
methods answer token-range queries.

This independently implements the cumulative-checkpoint concept used by
Turtledeflate's `turtledeflate_create_global_histogram` and
`turtledeflate_get_partial_histogram`:

| Property | Turtledeflate | Columbo |
| --- | --- | --- |
| Coordinate | current compressor-token index | parsed source-token index |
| Stride | 256 tokens | 256 tokens |
| Stored state | cumulative literal/length and distance counts | those counts plus payload extra bits |
| Query | subtract aligned middle, scan both edges | reconstruct two prefixes, then subtract |
| End-of-block | added by the range-cost caller | added by `range_frequencies` |
| Allocation handling | fixed context allocation | checked optional allocation under a model budget |
| Fallback | none | direct token scan when the index does not fit |

The 256-token checkpoint concept and its use for bounded range queries were
inspired by Turtledeflate. Columbo's Rust code, representation, overflow
checks, memory gate, fallback, and extra-bit accounting are independent.

### Bounded adaptive split

Columbo's `add_adaptive_split_cut`, `coarse_to_fine_split`, and
`cached_adaptive_split_score` in
[`src/deflate/stream.rs`](../src/deflate/stream.rs) independently implement
the sample/smooth/narrow/scan concept used by
`turtledeflate_best_block_split`.

The shared skeleton is distinctive:

```text
sample evenly spaced split costs
smooth neighbouring samples
narrow around the best basin
scan the terminal range exactly
```

The implementations differ materially:

| Property | Turtledeflate default | Columbo |
| --- | ---: | ---: |
| Samples per coarse pass | 32 | 8 |
| Smoothing window | 7 values | 3 values |
| Narrowing radius | 8 sample slots | 1 sample slot |
| Terminal exhaustive width | 1024 tokens | 16 tokens |
| Probe cap | none | 128 distinct scores |
| Deadline | none | caller deadline checked throughout |
| Alternate basin | edge-triggered range stack | omitted |
| Cost candidates | dynamic only | cheap stored/fixed/dynamic estimate |
| Token source | newly compressed parse | frozen parsed source stream |
| Acceptance | splitter/outer feedback | exact Columbo replanning and a 32-bit win |
| Mode | compressor pipeline | `--max` only |

The method must therefore be described as an independent,
Turtledeflate-inspired Columbo implementation, not as a port or exact
Turtledeflate parity.

### Supporting boundary estimator

Columbo's `estimate_boundary_block_bits` in
[`src/deflate/header.rs`](../src/deflate/header.rs) supplies the cheap score
used by the adaptive search. It prices stored output, fixed output, and one
simple dynamic candidate built with a DeflOpt-derived tree.

This fills the same role as Turtledeflate's histogram block-cost estimator but
does not recreate it. Its tree, header, representation, and alignment choices
are Columbo-specific or separately attributed.

### What is not incorporated

The following similarities must not be labelled as Turtledeflate-inspired:

- Columbo's original eighth-position and 32-token split probes;
- Columbo's boundary dynamic programming and exact alignment-aware acceptance;
- the eight code-length-RLE masks already present in C Columbo;
- Columbo's adjacency-aware logarithmic pseudo-frequency quantizer;
- generic canonical Huffman construction; and
- generic candidate caching or de-duplication.

## Remaining in-scope candidates

The audit found no missing method that should be added immediately to default
mode. These experiments are compatible with a frozen token stream, listed in
recommended order.

### 1. Retain one second adaptive split basin

Turtledeflate can save a range on the other side of a separating maximum when
the first smoothed minimum lies near an edge. Columbo currently keeps only the
best candidate from its cached probe set.

A bounded Columbo experiment could retain at most one well-separated second
local minimum and submit both boundaries to exact planning. It should:

- remain `--max` only initially;
- reuse already-computed histogram scores;
- stay inside the existing deadline and total probe budget;
- de-duplicate a cut already present in the ordinary cut set; and
- accept only a strictly smaller exact complete plan.

This is the most direct missing part of Turtledeflate's split-position search.
Its runtime effect still requires whole-corpus measurement.

### 2. Perform one bounded adjacent-boundary reseat

Columbo's boundary DP can choose globally among known cuts. It does not
currently discover a new adaptive cut by joining each adjacent winning pair and
searching across their union in Turtledeflate's manner.

A conservative experiment could run one parity pass over a small winning block
list, reuse the existing range index, and exactly replan only the selected
candidate. Repeated reseating should not be copied: it would add substantial
work and duplicate parts of Columbo's boundary DP.

### 3. Differential-test Turtledeflate's tree builder

The exact over-depth repair and merge-order reassignment are technically
compatible with existing-token optimization. Before production code is added,
a standalone differential tool should compare that tree against Columbo's
existing tree-family outputs across the full frequency corpus.

Add it only if it creates unique exact wins large enough to justify another
tree candidate. There is no current evidence that it will.

### 4. Forced-split lookahead only for a diagnosed miss

Forced splits can cross a local minimum, but upstream says the route usually
wastes time. It should not be a general default-mode addition.

If a substantial, reproducible corpus miss demonstrates the need, test at most
one forced split followed by one bounded reseat in `--max`, with a strict
deadline and exact final comparison.

### Separately: exact Zopfli pseudo-frequencies

An exact `OptimizeHuffmanForRle` candidate is inexpensive and in scope for a
token-preserving optimizer, but it is a Zopfli method rather than a remaining
Turtledeflate method. Trial it as one paired literal/distance-tree candidate,
not another cross-product of every tree family. Any close translation requires
the proper Apache-2.0 notice.

## Explicitly out of scope

Do not incorporate these Turtledeflate methods into Columbo:

- hash-chain match discovery and per-length distance enumeration;
- greedy or lazy parsing of decoded bytes;
- trellis search across newly discovered matches;
- the alternative two-match backtrace;
- multi-precision parse diversification;
- frequency-model perturbation used to discover a different parse;
- exact parse refinement from raw bytes;
- recompressing partitions after a boundary change;
- raw-input superblock handling; or
- gzip creation around newly compressed data.

Adapting those methods would turn Columbo into a recompressor, contrary to its
purpose.

## Active, dormant, and proposed upstream code

Several source passages should not be mistaken for active Turtledeflate
methods:

- `turtledeflate_get_estimated_block_bits_internal____` is under `#if 0`;
- `turtledeflate_block_deflate_merge_models` has no caller;
- worst-path pruning is compiled out by `TURTLEDEFLATE_KICK_EM == 0`; and
- the outer "unstuck" retry in `turtledeflate.c` is guarded by `if (0 && ...)`.

The active forced-split escape is the separate route at block-source lines
2331-2378.

The README also suggests parallelizing superblocks, skipping unnecessary late
splitter work, and reusing unchanged subblocks
([lines 111-119](https://github.com/rwillenbacher/turtledeflate/blob/756f844d0cb0cb0accb5cca6ce08bf7a6b9a6fa1/README.md#L111-L119)).
Those are engineering suggestions, not implemented Turtledeflate methods. A
future Columbo cache based on that observation should be labelled as a Columbo
optimization inspired by the upstream note.

## Licensing

Turtledeflate is BSD 2-Clause, copyright 2022 Ralf Willenbacher
([LICENSE](https://github.com/rwillenbacher/turtledeflate/blob/756f844d0cb0cb0accb5cca6ce08bf7a6b9a6fa1/LICENSE#L1-L25)).
Columbo copies or translates none of its source code; the Rust implementation
only uses independently expressed concepts. Turtledeflate's licence is
therefore an upstream reference, not a Columbo distribution notice.

Turtledeflate's embedded `OptimizeHuffmanForRle` has separate Google Zopfli
Apache-2.0 provenance. Columbo currently uses an original quantizer rather than
a translation of that function. If the exact Zopfli route is later translated
or copied, its licensing and notice requirements must be reviewed separately.

## Primary-source index

| Source | Relevant content |
| --- | --- |
| [README](https://github.com/rwillenbacher/turtledeflate/blob/756f844d0cb0cb0accb5cca6ce08bf7a6b9a6fa1/README.md#L1-L121) | purpose, algorithm overview, timings, provenance note, and speed observations |
| [`turtledeflate.c`](https://github.com/rwillenbacher/turtledeflate/blob/756f844d0cb0cb0accb5cca6ce08bf7a6b9a6fa1/lib/turtledeflate.c#L74-L310) | outer compress/split feedback, rollback, and final state |
| [`turtledeflate_block.c` matching](https://github.com/rwillenbacher/turtledeflate/blob/756f844d0cb0cb0accb5cca6ce08bf7a6b9a6fa1/lib/turtledeflate_block.c#L339-L607) | match discovery and greedy seed |
| [`turtledeflate_block.c` parsing](https://github.com/rwillenbacher/turtledeflate/blob/756f844d0cb0cb0accb5cca6ce08bf7a6b9a6fa1/lib/turtledeflate_block.c#L757-L1647) | cost models, trellis, alternative path, and precision/model search |
| [`turtledeflate_block.c` histograms](https://github.com/rwillenbacher/turtledeflate/blob/756f844d0cb0cb0accb5cca6ce08bf7a6b9a6fa1/lib/turtledeflate_block.c#L1650-L1757) | cumulative checkpoints and token-range cost |
| [`turtledeflate_block.c` best split](https://github.com/rwillenbacher/turtledeflate/blob/756f844d0cb0cb0accb5cca6ce08bf7a6b9a6fa1/lib/turtledeflate_block.c#L1791-L1982) | coarse sampling, smoothing, narrowing, alternate basin, and final scan |
| [`turtledeflate_block.c` routing](https://github.com/rwillenbacher/turtledeflate/blob/756f844d0cb0cb0accb5cca6ce08bf7a6b9a6fa1/lib/turtledeflate_block.c#L2019-L2378) | boundary reseat, split, merge, cache invalidation, and forced lookahead |
| [`turtledeflate_tree.c`](https://github.com/rwillenbacher/turtledeflate/blob/756f844d0cb0cb0accb5cca6ce08bf7a6b9a6fa1/lib/turtledeflate_tree.c#L42-L512) | length-limited trees, canonical codes, and dynamic-header RLE |
| [`turtledeflate_api.h`](https://github.com/rwillenbacher/turtledeflate/blob/756f844d0cb0cb0accb5cca6ce08bf7a6b9a6fa1/inc/turtledeflate_api.h#L39-L72) | splitter and precision configuration |
| [`turtledeflate_app.c`](https://github.com/rwillenbacher/turtledeflate/blob/756f844d0cb0cb0accb5cca6ce08bf7a6b9a6fa1/src/turtledeflate_app.c#L107-L234) | defaults, CLI options, and raw-input entry |
| [BSD 2-Clause license](https://github.com/rwillenbacher/turtledeflate/blob/756f844d0cb0cb0accb5cca6ce08bf7a6b9a6fa1/LICENSE#L1-L25) | upstream license terms |
