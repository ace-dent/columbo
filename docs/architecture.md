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

The library owns optimization, while the CLI owns filesystem changes. Shared
terminal code is compiled privately into both targets so the library does not
need to expose CLI implementation details as public API.

## Deflate engine

`optimize.rs` validates the complete input, schedules routes, retains the required
Default comparison floor, and selects a complete emitted candidate. A timeout
limits optional search; it does not bypass validation or produce partial output.

| Module | Responsibility |
| --- | --- |
| `bitstream` | Read and write least-significant-bit-first Deflate fields. |
| `parse`, `model` | Validate the stream and retain decoded bytes, source symbols, and block metadata. |
| `huffman` | Construct code lengths, canonical codes, and decoding tables. |
| `header` | Construct dynamic headers and account for payload and header bits. |
| `block` | Price and emit original, stored, fixed, or dynamic block representations. |
| `search` | Explore token spellings justified by the input's existing matches. |
| `stream` | Plan block merges, splits, and boundary changes. |
| `source_recode` | Run the deft4j-derived source-order recoding and merge route. |
| `restore` | Restore original match choices after other routes settle. |
| `joint` | Search payload code lengths and header run-length encoding together. |
| `symbol_set` | Price removal of small sets of payload symbols. |
| `stop` | Apply shared deadlines, grace periods, and cooperative cancellation. |

The engine never discovers new LZ77 matches. Keep source-proof checks, fallible
allocation, work limits, exact candidate pricing, and tie order visible when
changing a search. Similar-looking routes may deliberately differ in these
policies; their research provenance is described in [routes and methods](routes-and-methods.md).

## Tests, tooling, and documentation

Every unit suite lives in its module's `tests.rs` and is loaded with
`#[cfg(test)] mod tests;`, regardless of suite size. Implementation files keep
their descriptive names, while a sibling directory holds the tests: for example,
`src/format/gzip.rs` and `src/format/gzip/tests.rs`. The format coordinator
uses `src/format/mod.rs` and `src/format/tests.rs`. Library-root unit tests live
in `src/tests.rs`. This preserves access to private implementation details
without exporting test-only APIs. Keep helpers and methods used by a single
suite in its `tests.rs`. Put fixtures, assertions, and accessors used by multiple
suites in the owning module's `test_support.rs`, loaded with a `#[cfg(test)]`
module declaration. Import shared helpers from that module explicitly. For
example, header fixtures live in `src/deflate/header/test_support.rs`, and CLI
helpers live in `src/cli/test_support.rs`.

Implementation files contain test-module declarations and the minimal
instrumentation that must observe production code. The parser's test-only
trailing-empty-block counter and its `ParsedStream` field are the current
exception; fixture data and standalone test helpers belong in test files.

`tests/public_api.rs` exercises the library as an external consumer.
`tests/local_corpus.rs` holds opt-in public-API regressions backed by local files.
`tests/test_sanitize_binary_paths.py` checks path audits and atomic binary
redaction. `tests/test_build_distribution.py` checks concurrent packaging and
failure cleanup with synthetic executables and a fake compiler. Both use
temporary directories, and neither requires private fixtures. Build and
packaging scripts live in `utils/`.

Use lowercase `snake_case` for Rust modules and Python test files. Shell scripts
and documentation use descriptive hyphenated names. Keep named implementation
files such as `zip.rs` when adding tests or child modules in the sibling `zip/`
directory. Existing group roots such as `src/format/mod.rs` retain `mod.rs`.

`docs/` contains contributor material and benchmark summaries; `docs/research/`
contains method analysis and existing reference material. Private corpus files
and exploratory outputs are intentionally outside the tracked project. See the
[contributor guide](CONTRIBUTING.md) for test commands and publication boundaries.
