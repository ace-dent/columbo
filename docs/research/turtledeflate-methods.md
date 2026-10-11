# Turtledeflate methods and their relevance to Columbo

This audit of Turtledeflate's source identifies the methods that influenced
Columbo and separates those that apply to an existing Deflate stream from fresh
compression.

The audited snapshot is commit
[`756f844d0cb0cb0accb5cca6ce08bf7a6b9a6fa1`](https://github.com/rwillenbacher/turtledeflate/tree/756f844d0cb0cb0accb5cca6ce08bf7a6b9a6fa1),
dated 9 March 2026. Links below are pinned to that commit.

## Conclusions

Turtledeflate compresses raw input bytes; it does not optimize an existing
Deflate stream. Most of its gains come from discovering matches, repeatedly
changing the LZ77 parse, and feeding new block boundaries back through the
compressor. Those methods are outside Columbo's scope.

Columbo independently implements two concepts from Turtledeflate's block
splitter:

1. cumulative symbol-frequency checkpoints at a 256-token stride; and
2. a histogram-guided, locally smoothed, coarse-to-fine split search.

The checkpoints serve Columbo's general range planning; the adaptive split is a
Max-only route. Max mode also runs one bounded adjacent-boundary reseat and one
forced-split escape, much smaller counterparts of Turtledeflate's boundary
reseating and forced splits. Columbo copies or translates no Turtledeflate
code ([licensing](#licensing)).

Turtledeflate's `OptimizeHuffmanForRle` is Google Zopfli code, as its source
states. Columbo's logarithmic, adjacency-aware pseudo-frequency method is
original and is not a Turtledeflate method.

The one remaining in-scope candidate is a
[differential test of Turtledeflate's tree builder](#remaining-in-scope-candidate).

## Evidence and attribution policy

The upstream C source is authoritative. The README gives the author's design
and performance commentary; where it differs from the active source, the
source wins.

Classification labels:

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

Generic RFC 1951 operations, such as canonical-code assignment or the
code-length repeat symbols 16, 17 and 18, are not treated as
Turtledeflate-inspired.

## Product and scope

Turtledeflate describes itself as a slow Deflate compressor whose results
depend on repeated path tracing and precise block boundaries
([README lines 3-17](https://github.com/rwillenbacher/turtledeflate/blob/756f844d0cb0cb0accb5cca6ce08bf7a6b9a6fa1/README.md#L3-L17)).
The command-line program feeds the raw bytes of an ordinary input file to
`turtledeflate_block`
([application lines 205-288](https://github.com/rwillenbacher/turtledeflate/blob/756f844d0cb0cb0accb5cca6ce08bf7a6b9a6fa1/src/turtledeflate_app.c#L205-L288))
and never parses an existing Deflate stream. Its final writer emits dynamic
blocks directly, without comparing stored, fixed and dynamic representations
([block writer lines 2419-2438](https://github.com/rwillenbacher/turtledeflate/blob/756f844d0cb0cb0accb5cca6ce08bf7a6b9a6fa1/lib/turtledeflate_block.c#L2419-L2438)).

Columbo starts from a parsed Deflate stream and never searches the decoded
bytes for new matches ([scope](future-deflate-methods.md#scope-and-invariants)).
That distinction governs every classification below.

## High-level Turtledeflate pipeline

A simplified view of the active compressor:

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

The outer feedback loop is in
[`turtledeflate_block`](https://github.com/rwillenbacher/turtledeflate/blob/756f844d0cb0cb0accb5cca6ce08bf7a6b9a6fa1/lib/turtledeflate.c#L74-L310).
The scope boundary is the recompression step: a changed boundary causes the
raw bytes to be parsed again, so later token streams need not keep the original
matches.

## Match discovery and parse optimization

All methods in this section select a new LZ77 representation from raw bytes
and are out of scope.

### Hash-chain match enumeration

Turtledeflate uses a rolling three-byte hash and a ring of earlier positions in
the 32 KiB window. It searches the chain for the longest match and records a
distance for every attainable sub-length
([block source lines 339-503](https://github.com/rwillenbacher/turtledeflate/blob/756f844d0cb0cb0accb5cca6ce08bf7a6b9a6fa1/lib/turtledeflate_block.c#L339-L503)).

### Greedy and lazy seed

The initial parse takes the longest current match unless the next byte starts a
longer one. It also requires a minimum length of four for distances above 1024
([lines 552-607](https://github.com/rwillenbacher/turtledeflate/blob/756f844d0cb0cb0accb5cca6ce08bf7a6b9a6fa1/lib/turtledeflate_block.c#L552-L607)).

### Iterative trellis parse

For every decoded position, Turtledeflate considers a literal and every
available match sub-length. Edge costs come from either:

- quantized Shannon estimates derived from a current frequency model; or
- exact Huffman code lengths derived from that model.

The trellis keeps two incoming choices at an endpoint. An optional backtrace can
replace two primary matches with a second two-match decomposition of the same
decoded length when the alternative has favorable length symbols and no worse
length extra-bit cost
([cost and trellis lines 757-1088](https://github.com/rwillenbacher/turtledeflate/blob/756f844d0cb0cb0accb5cca6ce08bf7a6b9a6fa1/lib/turtledeflate_block.c#L757-L1088)).

### Precision-diverse and perturbed models

The README calls the final parse slightly randomizing, but the implementation
uses no random-number generator. It starts deterministic model paths at
different fixed-point precisions, advances their precision in stages,
eliminates identical frequency models, perturbs models toward uniform
frequencies at several weights, and finishes with exact refinement:
[model perturbation lines 1282-1341](https://github.com/rwillenbacher/turtledeflate/blob/756f844d0cb0cb0accb5cca6ce08bf7a6b9a6fa1/lib/turtledeflate_block.c#L1282-L1341),
[duplicate-model elimination lines 1344-1395](https://github.com/rwillenbacher/turtledeflate/blob/756f844d0cb0cb0accb5cca6ce08bf7a6b9a6fa1/lib/turtledeflate_block.c#L1344-L1395) and
[precision-diverse search lines 1399-1647](https://github.com/rwillenbacher/turtledeflate/blob/756f844d0cb0cb0accb5cca6ce08bf7a6b9a6fa1/lib/turtledeflate_block.c#L1399-L1647).

## Huffman and dynamic-header methods

### Length-limited Huffman construction

Turtledeflate builds an unconstrained frequency tree, caps overlong lengths with
a zlib-style count redistribution, reassigns the resulting length histogram in
its recorded merge order, and constructs canonical codes
([tree source lines 42-343](https://github.com/rwillenbacher/turtledeflate/blob/756f844d0cb0cb0accb5cca6ce08bf7a6b9a6fa1/lib/turtledeflate_tree.c#L42-L343)).

Rebuilding a Huffman tree for existing token frequencies is in scope. Columbo
already prices several independently sourced tree builders (generic,
order-sensitive, DeflOpt, Defluff and deft4j families), and one of them
probably duplicates the ordinary Turtledeflate result. Its over-depth repair
and leaf reassignment may still produce a unique candidate.

Status: not incorporated; see
[Remaining in-scope candidate](#remaining-in-scope-candidate).

### Eight code-length-RLE masks

Turtledeflate can independently enable or disable repeat symbols 16, 17 and
18, tries all eight subsets, and keeps the shortest encoded dynamic header
([encoder lines 346-512](https://github.com/rwillenbacher/turtledeflate/blob/756f844d0cb0cb0accb5cca6ce08bf7a6b9a6fa1/lib/turtledeflate_tree.c#L346-L512),
[selection lines 2475-2486](https://github.com/rwillenbacher/turtledeflate/blob/756f844d0cb0cb0accb5cca6ce08bf7a6b9a6fa1/lib/turtledeflate_block.c#L2475-L2486)).

Columbo also evaluates the eight masks, but that route comes from the original
Columbo C implementation, and Columbo's header planner explores a wider family.
This is independent convergence, not Turtledeflate provenance.

### RLE-friendly pseudo-frequencies are Zopfli-derived

Turtledeflate copies `OptimizeHuffmanForRle` from Google Zopfli, keeping
Zopfli's Apache-2.0 notice and authorship
([notice lines 1-19](https://github.com/rwillenbacher/turtledeflate/blob/756f844d0cb0cb0accb5cca6ce08bf7a6b9a6fa1/lib/turtledeflate_block.c#L1-L19),
[function lines 230-335](https://github.com/rwillenbacher/turtledeflate/blob/756f844d0cb0cb0accb5cca6ce08bf7a6b9a6fa1/lib/turtledeflate_block.c#L230-L335));
the README confirms this
([lines 105-110](https://github.com/rwillenbacher/turtledeflate/blob/756f844d0cb0cb0accb5cca6ce08bf7a6b9a6fa1/README.md#L105-L110)).

Turtledeflate's final writer builds one tree from the true counts and another
from the RLE-friendly pseudo-counts; its block-cost estimator also tries both
above compression level seven. Both price payloads with the true frequencies,
so a misleading pseudo-count reduction cannot win without a complete bit-cost
improvement
([estimator lines 715-751](https://github.com/rwillenbacher/turtledeflate/blob/756f844d0cb0cb0accb5cca6ce08bf7a6b9a6fa1/lib/turtledeflate_block.c#L715-L751),
[writer lines 2441-2465](https://github.com/rwillenbacher/turtledeflate/blob/756f844d0cb0cb0accb5cca6ce08bf7a6b9a6fa1/lib/turtledeflate_block.c#L2441-L2465)).

Columbo's `make_columbo_rle_pseudofrequencies` is a different algorithm: it
rounds counts onto a logarithmic grid and changes a nonzero count only when an
adjacent symbol shares its bucket. Its provenance is the Zopfli pseudo-frequency
concept, also used by Turtledeflate, with a Columbo-original adjacency-aware
quantizer and candidate routing.

Columbo separately implements the published `OptimizeHuffmanForRle` behavior
as `make_zopfli_rle_pseudofrequencies` in
[`src/deflate/huffman.rs`](../../src/deflate/huffman.rs). That is a Zopfli
method, not a Turtledeflate one; the
[method catalog](../routes-and-methods.md#block-huffman-and-token-methods)
lists where it runs.

## Turtledeflate's block splitter

The block splitter is the part of Turtledeflate most relevant to Columbo. It
runs over the token stream produced by the current compression pass.
Boundaries are token indices, even though some upstream variable names call
them "deflated" boundaries.

### 256-token cumulative histograms

Turtledeflate snapshots cumulative literal/length and distance frequencies
every 256 LZ77 tokens
([constant and type lines 93-98](https://github.com/rwillenbacher/turtledeflate/blob/756f844d0cb0cb0accb5cca6ce08bf7a6b9a6fa1/lib/turtledeflate.h#L93-L98),
[builder lines 1650-1687](https://github.com/rwillenbacher/turtledeflate/blob/756f844d0cb0cb0accb5cca6ce08bf7a6b9a6fa1/lib/turtledeflate_block.c#L1650-L1687)).

For an arbitrary token range, it:

1. scans from the range start to the next aligned checkpoint;
2. subtracts two cumulative checkpoints for the aligned middle; and
3. scans the unaligned tail.

It then adds the end-of-block count and estimates the block cost
([range query and cost lines 1690-1757](https://github.com/rwillenbacher/turtledeflate/blob/756f844d0cb0cb0accb5cca6ce08bf7a6b9a6fa1/lib/turtledeflate_block.c#L1690-L1757)).

### Coarse-to-fine, locally smoothed boundary search

For a sufficiently large range, `turtledeflate_best_block_split`:

1. samples 32 evenly spaced candidate positions with the default settings;
2. prices the left and right token histograms at every sample;
3. computes a moving sum around each sampled cost, using a default radius of
   three sample slots;
4. selects the sample with the lowest smoothed cost;
5. narrows the position range to the samples eight slots either side of that
   sample;
6. repeats until the remaining range is no more than 1024 tokens; and
7. exhaustively prices every split in the final range.

If the selected minimum lies near an edge, the function may save the range on
the far side of a separating maximum and later search that alternate local
minimum.

The complete method is
[`turtledeflate_best_block_split`, lines 1791-1982](https://github.com/rwillenbacher/turtledeflate/blob/756f844d0cb0cb0accb5cca6ce08bf7a6b9a6fa1/lib/turtledeflate_block.c#L1791-L1982).
Its defaults are defined in
[`turtledeflate_api.h`, lines 48-57](https://github.com/rwillenbacher/turtledeflate/blob/756f844d0cb0cb0accb5cca6ce08bf7a6b9a6fa1/inc/turtledeflate_api.h#L48-L57)
and initialized by the application
([lines 121-134](https://github.com/rwillenbacher/turtledeflate/blob/756f844d0cb0cb0accb5cca6ce08bf7a6b9a6fa1/src/turtledeflate_app.c#L121-L134)).

### Split, merge, and boundary reseating

Turtledeflate keeps cached costs and local invalidation flags for each current
subblock. It repeatedly:

- reseats an existing boundary by joining two adjacent ranges and finding a
  better split across their union;
- chooses the globally best profitable split; and
- chooses the globally best profitable adjacent merge.

Reseating alternates even and odd adjacent pairs, so one pass can move
non-overlapping boundaries before the next parity is examined
([reseat lines 2019-2069](https://github.com/rwillenbacher/turtledeflate/blob/756f844d0cb0cb0accb5cca6ce08bf7a6b9a6fa1/lib/turtledeflate_block.c#L2019-L2069)).
The split, merge, and driver are at:

- [split lines 2072-2195](https://github.com/rwillenbacher/turtledeflate/blob/756f844d0cb0cb0accb5cca6ce08bf7a6b9a6fa1/lib/turtledeflate_block.c#L2072-L2195);
- [merge lines 2198-2260](https://github.com/rwillenbacher/turtledeflate/blob/756f844d0cb0cb0accb5cca6ce08bf7a6b9a6fa1/lib/turtledeflate_block.c#L2198-L2260); and
- [driver lines 2278-2324](https://github.com/rwillenbacher/turtledeflate/blob/756f844d0cb0cb0accb5cca6ce08bf7a6b9a6fa1/lib/turtledeflate_block.c#L2278-L2324).

### Forced-split escape

In early splitter states, Turtledeflate can clone the current post-search
state, force a locally losing split, reseat the resulting boundaries, and keep
a later global improvement. It stops after five consecutive forced attempts
fail to improve the best state
([lines 2331-2378](https://github.com/rwillenbacher/turtledeflate/blob/756f844d0cb0cb0accb5cca6ce08bf7a6b9a6fa1/lib/turtledeflate_block.c#L2331-L2378)).

The README says this usually wastes runtime but occasionally finds more and
better blocks
([lines 113-119](https://github.com/rwillenbacher/turtledeflate/blob/756f844d0cb0cb0accb5cca6ce08bf7a6b9a6fa1/README.md#L113-L119)).
That tradeoff conflicts with Columbo's speed-first Default mode; Columbo's
bounded Max version is under
[Bounded reseat and forced-split escape](#bounded-reseat-and-forced-split-escape).

## Methods incorporated into Columbo

### Strided range-frequency index

Columbo's [`Composite`](../../src/deflate/stream.rs) builds
`frequency_checkpoints`, and its `prefix_frequencies` and `range_frequencies`
methods answer token-range queries. This independently implements the
cumulative-checkpoint concept of Turtledeflate's
`turtledeflate_create_global_histogram` and
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

`range_frequencies` also counts tokens directly when the range is no longer
than its two partial prefixes. The 256-token checkpoints and their use for
bounded range queries are Turtledeflate-inspired; Columbo's code,
representation, overflow checks, memory gate, fallback and extra-bit
accounting are independent.

### Bounded adaptive split

Columbo's `add_adaptive_split_cut`, `adaptive_histogram_split_search`,
`coarse_to_fine_split_search`, and `cached_adaptive_split_score` in
[`src/deflate/stream.rs`](../../src/deflate/stream.rs) independently implement
the sample/smooth/narrow/scan skeleton of `turtledeflate_best_block_split`:

```text
sample evenly spaced split costs
smooth neighboring samples
narrow around the best smoothed sample
scan the terminal range exactly
```

The implementations differ:

| Property | Turtledeflate default | Columbo |
| --- | ---: | ---: |
| Samples per coarse pass | 32 | 8 |
| Smoothing window | 7 values | 3 values |
| Narrowing radius | 8 sample slots | 1 sample slot |
| Terminal exhaustive width | 1024 tokens | 16 tokens |
| Probe cap | none | 128 distinct scores |
| Deadline | none | caller deadline checked throughout |
| Alternate local minimum | edge-triggered range stack | omitted |
| Cost candidates | dynamic only | cheap stored/fixed/dynamic estimate |
| Token source | newly compressed parse | frozen parsed source stream |
| Acceptance | splitter/outer feedback | exact Columbo replanning and a 32-bit win |
| Mode | compressor pipeline | `--max` only |

Columbo also picks one well-separated runner-up local minimum from the probes
it has already scored, used only by the forced-split escape.

### Supporting boundary estimator

Columbo's `estimate_boundary_block_bits` in
[`src/deflate/header.rs`](../../src/deflate/header.rs) supplies the cheap
Huffman score for the adaptive search. It prices the fixed tree and one simple
dynamic candidate: DeflOpt variant-zero heap trees with one greedy
code-length RLE spelling. Its caller, `estimate_histogram_range_bits` in
[`src/deflate/stream.rs`](../../src/deflate/stream.rs), takes the smaller of
that score and the stored-block size.

This fills the role of Turtledeflate's histogram block-cost estimator without
recreating it; its tree, header, representation and alignment choices are
Columbo-specific or separately attributed.

### Bounded reseat and forced-split escape

Max mode runs two boundary routes in
[`src/deflate/stream.rs`](../../src/deflate/stream.rs), each far more bounded
than its Turtledeflate counterpart. Both accept only a strictly smaller
complete plan.

- `plan_selected_huffman_boundary_reseat` runs the adaptive split search over
  each adjacent pair's union and keeps only the single best reseat;
  Turtledeflate reseats repeatedly, alternating even and odd pairs.
- `plan_forced_split_boundary_escape` forces one runner-up cut in the largest
  eligible block, taken from `secondary_adaptive_split_cut` without a second
  probe budget, plans both children exactly, then tries one reseat;
  Turtledeflate repeats forced splits and reseating until five consecutive
  attempts fail.

[Routes and methods](../routes-and-methods.md#inner-stream-planner-gates) gives
the exact gates and catalogs the reseat as a Columbo method and the escape as a
Turtledeflate-inspired concept.

### What is not incorporated

These similarities must not be labeled Turtledeflate-inspired:

- Columbo's original eighth-position and 32-token split probes;
- Columbo's boundary dynamic programming and exact alignment-aware acceptance;
- the eight code-length-RLE masks already present in C Columbo;
- Columbo's adjacency-aware logarithmic pseudo-frequency quantizer;
- generic canonical Huffman construction; and
- generic candidate caching or deduplication.

## Remaining in-scope candidate

The audit found no method to add to Default mode. Of its four experiments that
keep the existing token stream, Columbo's Max mode implements three in bounded
form (see
[Bounded reseat and forced-split escape](#bounded-reseat-and-forced-split-escape)):

- a second adaptive split minimum, used only as the forced-split escape's
  runner-up cut; a standalone second-minimum route changed no outputs and was
  not adopted
  ([rejected experiments](future-deflate-methods.md#rejected-or-deferred-experiments));
- one adjacent-boundary reseat; and
- one forced split followed by one reseat.

The fourth remains open: a differential test of Turtledeflate's tree builder,
whose over-depth repair and merge-order reassignment are compatible with
existing-token optimization. A standalone tool should first compare that tree
with Columbo's tree-family outputs across the full frequency corpus. Add it
only if it produces unique exact wins large enough to justify another tree
candidate.

## Out of scope

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

Adapting them would turn Columbo into a recompressor.

## Active, dormant, and proposed upstream code

These source passages are not active Turtledeflate methods:

- `turtledeflate_get_estimated_block_bits_internal____` is under `#if 0`;
- `turtledeflate_block_deflate_merge_models` has no caller;
- worst-path pruning is compiled out by `TURTLEDEFLATE_KICK_EM == 0`; and
- the outer "unstuck" retry in `turtledeflate.c` is guarded by `if (0 && ...)`.

The active forced-split escape is the separate route at block-source lines
2331-2378.

The README also suggests parallelizing superblocks, skipping unnecessary late
splitter work, and reusing unchanged subblocks
([lines 111-119](https://github.com/rwillenbacher/turtledeflate/blob/756f844d0cb0cb0accb5cca6ce08bf7a6b9a6fa1/README.md#L111-L119)).
These are suggestions, not implemented methods; a Columbo cache based on them
should be labeled a Columbo optimization inspired by the upstream note.

## Licensing

Turtledeflate is BSD 2-Clause, copyright 2022 Ralf Willenbacher
([LICENSE](https://github.com/rwillenbacher/turtledeflate/blob/756f844d0cb0cb0accb5cca6ce08bf7a6b9a6fa1/LICENSE#L1-L25)).
Columbo copies or translates none of its source code and uses only
independently expressed concepts, so the license is an upstream reference, not
a Columbo distribution notice. Columbo's independent implementation of the
embedded Zopfli transform records Zopfli's Apache-2.0 authorship beside the
code.

## Primary-source index

| Source | Relevant content |
| --- | --- |
| [README](https://github.com/rwillenbacher/turtledeflate/blob/756f844d0cb0cb0accb5cca6ce08bf7a6b9a6fa1/README.md#L1-L121) | purpose, algorithm overview, timings, provenance note, and speed observations |
| [`turtledeflate.c`](https://github.com/rwillenbacher/turtledeflate/blob/756f844d0cb0cb0accb5cca6ce08bf7a6b9a6fa1/lib/turtledeflate.c#L74-L310) | outer compress/split feedback, rollback, and final state |
| [`turtledeflate_block.c` matching](https://github.com/rwillenbacher/turtledeflate/blob/756f844d0cb0cb0accb5cca6ce08bf7a6b9a6fa1/lib/turtledeflate_block.c#L339-L607) | match discovery and greedy seed |
| [`turtledeflate_block.c` parsing](https://github.com/rwillenbacher/turtledeflate/blob/756f844d0cb0cb0accb5cca6ce08bf7a6b9a6fa1/lib/turtledeflate_block.c#L757-L1647) | cost models, trellis, alternative path, and precision/model search |
| [`turtledeflate_block.c` histograms](https://github.com/rwillenbacher/turtledeflate/blob/756f844d0cb0cb0accb5cca6ce08bf7a6b9a6fa1/lib/turtledeflate_block.c#L1650-L1757) | cumulative checkpoints and token-range cost |
| [`turtledeflate_block.c` best split](https://github.com/rwillenbacher/turtledeflate/blob/756f844d0cb0cb0accb5cca6ce08bf7a6b9a6fa1/lib/turtledeflate_block.c#L1791-L1982) | coarse sampling, smoothing, narrowing, alternate local minimum, and final scan |
| [`turtledeflate_block.c` routing](https://github.com/rwillenbacher/turtledeflate/blob/756f844d0cb0cb0accb5cca6ce08bf7a6b9a6fa1/lib/turtledeflate_block.c#L2019-L2378) | boundary reseat, split, merge, cache invalidation, and forced lookahead |
| [`turtledeflate_tree.c`](https://github.com/rwillenbacher/turtledeflate/blob/756f844d0cb0cb0accb5cca6ce08bf7a6b9a6fa1/lib/turtledeflate_tree.c#L42-L512) | length-limited trees, canonical codes, and dynamic-header RLE |
| [`turtledeflate_api.h`](https://github.com/rwillenbacher/turtledeflate/blob/756f844d0cb0cb0accb5cca6ce08bf7a6b9a6fa1/inc/turtledeflate_api.h#L39-L72) | splitter and precision configuration |
| [`turtledeflate_app.c`](https://github.com/rwillenbacher/turtledeflate/blob/756f844d0cb0cb0accb5cca6ce08bf7a6b9a6fa1/src/turtledeflate_app.c#L107-L234) | defaults, CLI options, and raw-input entry |
| [BSD 2-Clause license](https://github.com/rwillenbacher/turtledeflate/blob/756f844d0cb0cb0accb5cca6ce08bf7a6b9a6fa1/LICENSE#L1-L25) | upstream license terms |
