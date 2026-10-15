# Codebase guide

Columbo has two Cargo targets: a reusable library and a command-line executable.
The public library interface is in `src/lib.rs`, with configuration in
`src/options.rs` and error categories in `src/error.rs`. Implementation modules
remain private to the crate.

## Source layout

| Path | Responsibility |
| --- | --- |
| `src/main.rs` | Start the CLI and return its exit status. |
| `src/cli/mod.rs` | Process input files sequentially through reading, optimization, and writing. |
| `src/cli/arguments.rs` | Parse OS-native arguments, validate combinations, and render help. |
| `src/cli/files.rs` | Read within resource limits and stage atomic file replacements. |
| `src/cli/report.rs` | Render per-file results and select the appropriate output channel. |
| `src/format/` | Detect and validate PNG/APNG, GZIP, ZIP, and zlib wrappers; schedule embedded streams and rebuild containers. |
| `src/format/deadline.rs` | Share file-wide time limits and scale embedded-stream search allowances. |
| `src/format/png/chunks.rs` | Parse, validate, and encode PNG/APNG chunks; calculate decoded image sizes. |
| `src/deflate/` | Parse, price, search, and emit structurally equivalent Deflate streams. |
| `src/checksum.rs` | Incremental CRC-32 and Adler-32 checksums. |
| `src/progress/` | Coordinate physical stream order and render verbose or visual route reports. |
| `src/terminal/` | Share terminal capability checks, text formatting, and spinner behavior between the library and CLI. |

The library performs optimization, while the CLI performs filesystem changes.
Shared terminal code is compiled privately into both targets so the library does
not need to expose CLI implementation details as public API.

## Deflate engine

`optimize.rs` holds the route builders, eligibility rules, and candidate
comparison. Its child module `optimize/schedule.rs` validates one raw stream,
then runs the route phases in a fixed order: each phase reads a shared route
context and updates the complete candidates carried between phases. Max
retains the Default comparison floor. A timeout limits optional search; it
never skips validation or produces partial output.

| Module | Responsibility |
| --- | --- |
| `bitstream` | Read and write least-significant-bit-first Deflate fields. |
| `parse`, `model` | Validate the stream and retain decoded bytes, source symbols, and block metadata. |
| `huffman` | Construct length-limited code lengths, canonical codes, and decoding tables. |
| `header` | Construct dynamic headers and count header and payload bits, reusing completed header plans per thread. Child modules hold Max code-length searches: `tree` (R7), `rotate` (R8), `coupled` (R9), `response` (R10), and `response/exchange` (R11). |
| `block` | Price and emit original, stored, fixed, or dynamic block representations, with a route-local plan cache. |
| `minima` | Share range-minimum lookups between searches over match-length families. |
| `search` | Change token spelling within existing matches. |
| `stream` | Plan block merges, splits, and boundary changes; `stream/alphabet` finds alphabet-support cuts (R6), `stream/entropy_cut` cuts blocks where their symbol statistics change (R15), and `stream/merge` merges neighbors after the finish (R16). |
| `source_recode` | Run the deft4j-derived source-order recoding and merge route. |
| `restore` | Restore original match choices after other routes settle (R1). |
| `joint` | Search payload code lengths and header run-length encoding together (R4). |
| `symbol_set` | Price removal of small sets of payload symbols (R5). |
| `slide` | Slide block boundaries under the transmitted Huffman trees (R1c). |
| `distance_band` | Collapse a block's distance alphabet to a kept set of distance symbols (R13). |
| `stop` | Apply shared deadlines, grace periods, and cooperative cancellation. |
| `optimize` | Build, schedule, and compare complete candidates, as described above. |

The engine never discovers new LZ77 matches. Keep source-proof checks, fallible
allocation, work limits, exact candidate pricing, and tie order visible when
changing a search. Similar routes can differ in these policies by design;
[routes and methods](routes-and-methods.md) describes each route and its
provenance.

## Terms

Code comments and the route documentation rely on these project terms.

- **Route**: one optimization strategy that builds a complete candidate, such
  as the ordinary planner, the deft4j-derived source route, or a terminal method.
- **Candidate**: a complete emitted stream that competes with the incumbent.
  Candidates are ranked by byte length, then meaningful bits; an exact tie
  keeps the incumbent.
- **Meaningful bits**: a Deflate stream's bit length through its final block,
  excluding the padding in the last byte.
- **Floor**: a complete candidate retained before optional search, which later
  routes must beat. Max keeps the Default comparison floor so that, given
  enough time, it never returns a worse result than Default.
- **Lineage**: a sequence of candidates derived from one starting candidate.
  Each lineage is retained independently until the final comparison.
- **Spelling**: the choice of tokens (literals, matches, and how a match is
  split) that encode the same decoded bytes.
- **Proven submatch**: a shorter match at an existing match's distance, inside
  that match's decoded interval; the source match proves those bytes repeat.
- **Sweep**: one ordered pass of the terminal methods (R1–R15) over the
  incumbent. Max repeats the sweep (R12) while it improves and time remains;
  R16 and R17 follow the settled sweeps.
- **Default / Max**: the two search modes. Max (`--max`, `Options::exhaustive`)
  adds slower routes.
- **Strict mode**: `--strict 1`, the default (`Options::strict`). Output uses
  complete Huffman alphabets and the standard length-258 code, symbol 285.
- **Deadline and grace**: the soft deadline stops new routes from starting;
  active work may continue for a grace of 10% of the timeout plus one second,
  until the hard stop.
- **Route labels**: D1–D3, G0, R1–R17, M0–M13, S0, and their suffixed variants
  name gates and routes in the
  [route-gate reference](routes-and-methods.md#route-gate-reference).

## Tests, tooling, and documentation

Every unit suite, whatever its size, lives in its module's `tests.rs`, loaded
with `#[cfg(test)] mod tests;`, so tests can reach private items without
test-only exports. A named implementation file keeps its name, and a sibling
directory holds its tests and child modules: for example, `src/format/gzip.rs`
and `src/format/gzip/tests.rs`. Group roots keep `mod.rs`, with tests beside
it: `src/format/mod.rs` and `src/format/tests.rs`. Library-root unit tests live
in `src/tests.rs`. Helpers used by one suite stay in its `tests.rs`. Fixtures,
assertions, and accessors shared by several suites go in the module's
`test_support.rs`, also under `#[cfg(test)]`, and are imported explicitly: for
example, `src/deflate/header/test_support.rs` and `src/cli/test_support.rs`.

Implementation files hold only test-module declarations and the minimal
test-only instrumentation that must observe production code, such as the
parser's trailing-empty-block counter (a `ParsedStream` field) and the file writer's
checkpoints after snapshot comparison and publication, which make the
concurrent-edit, force-kill, and sync-failure tests deterministic.

`tests/public_api.rs` exercises the library as an external consumer.
`tests/cli_output.rs` runs the CLI executable to check report streams and
`--out` validation. `tests/local_corpus.rs` holds opt-in public-API regressions
backed by local files. `tests/test_sanitize_binary_paths.py` checks path audits
and atomic binary redaction. `tests/test_build_distribution.py` checks
concurrent packaging and failure cleanup with synthetic executables and a fake
compiler. Both use temporary directories, and neither requires private
fixtures. Build and packaging scripts live in `utils/`.

See [file safety](file-safety.md) for the write protocol, interruption behavior,
and the limits of the portable filesystem implementation.

Use lowercase `snake_case` for Rust modules and Python test files, and
descriptive hyphenated names for shell scripts and documentation.

`docs/` contains contributor material, design notes, and benchmark summaries;
`docs/research/` contains method analysis and existing reference material.
Private corpus files and exploratory outputs stay outside the tracked project.
See the [contributor guide](CONTRIBUTING.md) for test commands and publication
boundaries.
