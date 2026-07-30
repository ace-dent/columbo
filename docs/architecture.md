# Columbo architecture for newcomers

This document is a guide to the Rust codebase. It explains the module layout,
the data flow from input to output, the key invariants, and a recommended
reading order for someone who wants to contribute code or research. It does
not duplicate the conceptual overview of optimization methods, which lives in
[`routes-and-methods.md`](routes-and-methods.md); this document complements
that one with code-level orientation.

## Source tree

```
src/
├── lib.rs              Public API surface (`optimize`, `Options`, `Optimization`).
├── main.rs             CLI entry point and `Options` construction.
├── options.rs          `Format` enum and `Options` struct definitions.
├── error.rs            `Error` and `Result` types.
├── progress.rs         CLI/progress reporting (`--verbose`, `--dry-run`).
├── checksum.rs         CRC32 and Adler-32 helpers.
│
├── format/             Container detection and walk.
│   ├── mod.rs          Detection cascade, `SearchDeadline`, dispatch.
│   ├── gzip.rs         RFC 1952 walking, header parsing, member reassembly.
│   ├── png.rs          PNG chunk walk, IDAT/fdAT/iCCP/iTXt/zTXt handling.
│   ├── zip.rs          Central-directory walk, method-8 entry selection.
│   └── zlib.rs         RFC 1950 wrapper handling.
│
└── deflate/            Deflate stream optimization (the core engine).
    ├── mod.rs          Public re-exports for siblings and tests.
    ├── bitstream.rs    Bit I/O (LSB-first, RFC 1951).
    ├── block.rs        Block emit (`emit_block`).
    ├── parse.rs        Stream parsing into `ParsedStream` (the immutable source model).
    ├── model.rs        Data structures: `ParsedBlock`, `Token`, `Representation`, etc.
    ├── huffman.rs      Length-limited Huffman construction (Package-Merge-derived).
    ├── header.rs       Dynamic-header packing and RLE strategies.
    ├── optimize.rs     Top-level route scheduler (`optimize_raw`) and bounded runners.
    ├── stream.rs       Stream-level route implementations (split, merge, grouping, replay).
    ├── search.rs       Token-level search (submatches, sibling tables, segmenter).
    └── deft4j.rs       Bounded deft4j-inspired source route.
```

`src/deflate/` is roughly 24,000 LOC of optimization code. The non-deflate
modules (`format/`, `lib.rs`, `options.rs`, `error.rs`, `progress.rs`,
`checksum.rs`) are mostly thin wrappers around the engine.

## Public API

The public surface lives in `src/lib.rs`. Two entry points matter:

```rust
pub fn optimize(input: &[u8], format: Format, options: &Options) -> Result<Optimization>
```

`optimize` is the only call anyone needs. `Format` selects the wrapper
(Auto/Raw/Png/Zlib/Gzip/Zip); `Options` controls `--max`, `--strict`,
`--verbose`, `--strip-metadata`, and the timeout/byte limits. See
`src/options.rs` for the complete field set.

The result type:

```rust
pub struct Optimization {
    pub data: Vec<u8>,       // Selected output bytes.
    pub bits_saved: u64,     // bytes_saved * 8 OR deflate-bits saved when bytes equal.
    pub timed_out: bool,     // Whether search hit the deadline.
}
```

`Optimization::from_metrics` (private) compares the source size, the source
deflate-bit total, and the new deflate-bit total to produce a `bits_saved`
value that is consistent with the no-growth guarantee.

## Data flow

```
input bytes
    │
    ▼
format::optimize(input, format, options)
    │  detect format (if Auto) ─── PNG signature → GZIP magic → ZIP structure → zlib header → raw
    │  enforce max_input_bytes
    │  create SearchDeadline (file-wide shared timeout)
    │
    ▼
[Raw] format/dispatch
    │  PNG →  png::optimize  → walks chunks, calls deflate::optimize_raw per stream
    │  ZIP →  zip::optimize   → walks central directory, calls deflate::optimize_raw per method-8 entry
    │  Zlib → zlib::optimize  → strips header/trailer, calls deflate::optimize_raw
    │  Gzip → gzip::optimize  → walks members, calls deflate::optimize_raw per member
    │  Raw  → deflate::optimize_raw directly
    │
    ▼
deflate::optimize_raw(stream, options_with_remaining_budget)
    │  parse: parse_stream → ParsedStream (decoded bytes, blocks, tokens, source dynamic trees)
    │  build initial candidates (original-strict, original-relaxed, source-cleanup)
    │  schedule route dispatch:
    │     - bounded-floor prebuild (when small enough)
    │     - default routes (re-segmentation, grouping, split probes, inside-match polish)
    │     - max-mode routes (wider tokens, source-max, replay, adaptive split)
    │  for each candidate:
    │     - plan: encode at current alignment → complete deflate bits
    │     - compare strictly to retained best
    │  pick smallest valid candidate
    │
    ▼
Optimization {
    data: encoded_stream,
    bits_saved: bytes_saved*8 OR meaning_bits_diff,
    timed_out: bool,
}
```

## Key invariants

Several invariants are not auto-enforced by the type system but are required
for correctness. `cargo test` exercises all of them; changes that violate
them break tests before they break users.

1. **Decoded byte equality**. Every candidate must decode to the same byte
   stream as the source. Checked by `parsed_model_bytes` and `raw_stream_decodes_to`
   in `deflate/parse.rs`.

2. **Deflate-format validity**. Every emitted bit sequence must be a valid
   Deflate stream accepted by independent decoders (zlib, libdeflate,
   references listed in `routes-and-methods.md`). The integration tests in
   `tests/public_api.rs` round-trip every public corpus file.

3. **No-growth guarantee (default) and relaxed-mode escape**. Default mode
   never returns an output that is larger than the input. Relaxed mode may
   only grow when strict compatibility requires it (canonicalizing length-258
   aliases, completing or singleton-fying incompatible dynamic alphabets).
   The `bits_saved == 0` invariant covers the equal-byte case where strict
   zero-padding still requires a Deflate-bit win before reporting savings.

4. **Strict `<` winner comparison**. Candidate retention always uses strict
   strict-improvement comparison. Equal-cost candidates never displace the
   incumbent, which makes enumeration order observable in tests.

5. **Determinism**. The same input and the same `Options` produce the same
   output on every run. No clocks with meaningful resolution are read in
   the optimization path; only the elapsed deadline is sampled.

6. **`!unsafe_code`**. The crate forbids `unsafe` at the top level.

7. **No panics on attacker-controlled input**. Every allocation that depends
   on input bytes goes through `try_vec_with_capacity` /
   `try_append_bytes` helpers in `format/mod.rs`. Recovery failures are
   returned as `Error`, not panics.

## Reading order for newcomers

Suggested order, ~30 minutes for someone comfortable with Rust:

1. **`src/lib.rs`** — `optimize()` is the only external entry. Read it.
2. **`src/options.rs`** — `Format` and `Options` are the only inputs.
3. **`src/format/mod.rs`** — Detection cascade, dispatch, and the
   `SearchDeadline` plumbing. This is the layer that handles multi-stream
   containers with a shared timeout.
4. **`src/deflate/parse.rs`** then `src/deflate/model.rs` — together, these
   define the immutable source model. `ParsedStream` is the central data
   structure that every later route consumes.
5. **`src/deflate/bitstream.rs`** — bit-level I/O. Not strictly required
   reading, but useful when a bug lands in encoding.
6. **`src/deflate/huffman.rs`** and `src/deflate/header.rs` — the two
   tables that determine payload bit cost. Length-limited Huffman
   construction and dynamic-header RLE are the most concentrated
   algorithmic content.
7. **`src/deflate/optimize.rs`** — top-level route scheduler. The function
   `optimize_raw` (≈300 LOC plus inline helpers) is the entry point. Read it
   to see the order in which routes try to improve a candidate.
8. **`src/deflate/stream.rs`** — concrete stream-level routes (split, merge,
   grouping, fragmented replay). Many of these are pure planning functions
   that return a `PlannedStream` for the price function in `optimize.rs`.
9. **`src/deflate/search.rs`** — token-level search. Submatches, sibling
   tables, segmenter. Most of the slow routes spend time here.
10. **`src/deflate/block.rs`** and **`src/deflate/deft4j.rs`** — block-level
    emission and the bounded deft4j route.

The `format/png.rs`, `format/zip.rs`, `format/gzip.rs`, `format/zlib.rs`
modules are concrete wrappers around the engine. They are read-once modules
— once you understand the engine, their containers are mostly CRCs and
chunk walks.

## Where to add new functionality

| You want to ... | Look in | Modification shape |
|---|---|---|
| Tweak a per-block cost or representation | `src/deflate/optimize.rs` (the route scheduler) and `src/deflate/block.rs` (emission) | Add a candidate struct, generate it in the relevant route, price it via the existing `price_*` helpers, and let the strict `<` winner replace it. |
| Add a new token-level optimization | `src/deflate/search.rs` + `src/deflate/model.rs` | Introduce a new `Token` variant if needed, then a `find_*` function plus a corresponding `plan_*` route that emits a `PlannedBlock`. |
| Add a new container | `src/format/` | Add a new submodule with `has_recognizable_structure` (or equivalent) and an `optimize` function. Wire the dispatch in `src/format/mod.rs`. |
| Change a default mode versus `exhaustive` boundary | `src/options.rs` (`Options::exhaustive`) and the per-route checks in `src/deflate/optimize.rs` and `src/deflate/search.rs` | Routes read `options.exhaustive` or the deadline. The pattern is to keep the existing bounded path unchanged and add a wider sibling only when the flag is set. |
| Change strict-mode behaviour | `src/options.rs` (`Options::strict`) and the per-candidate code that decides whether to canonicalize | Strict mode is the safe default; relaxed is an opt-in for compatibility risk. Most compatibility-edge handling lives in `src/deflate/optimize.rs`. |
| Add a benchmark | `tests/benchmark.rs` and any JSON corpus under `tests/fixtures/` | Follow the existing benchmark harness. Use `criterion` for statistical timing. |

## Testing

- `cargo test` runs all unit and integration tests. 284 tests pass on the
  current `main` (count varies as routes are added).
- `tests/public_api.rs` is the integration boundary: it round-trips the
  published corpus through `optimize` and verifies both decoded equality
  and byte-level behaviour.
- `cargo test --doc` runs the doctest examples in `src/lib.rs` and
  `src/deflate/model.rs`.
- `tests/benchmark.rs` provides benchmarks against the benchmark corpus.
- `CONTRIBUTING.md` describes the merging workflow and the GNU/Linux
  validation expectations.

## Performance contract

- `optimize_raw` is bounded by `Options::timeout` (default 180s, clamp
  10s–4000s). Active routes may finish their current candidate within
  10% + 1s of the deadline.
- `Options::max_input_bytes` and `Options::max_decoded_bytes` are
  conservative safety limits (default 1 GiB each). Lower them for
  untrusted callers.
- Peak memory is bounded by the input size plus one cached
  Huffman kernel per planned candidate, capped at 512 entries × 16 MiB.

## Reproduction of research

The implementation is a clean-room reimplementation informed by the
research documents under `docs/research/`:

- `deflopt-methods.md` — binary RE of DeflOpt 2.07.
- `defluff-methods.md` — binary RE of defluff 0.3.2.
- `deft4j-methods.md` — source RE of deft4j β17.
- `turtledeflate-methods.md` — source audit of Turtledeflate.
- `block-splitting.md` — design for the block-splitting route.
- `design-v1.md` — initial Rust design proposal.

If you want to revisit a design decision, start in the relevant research
document. The `Attribution` section of `routes-and-methods.md` names the
original author and the canonical link for each technique.
