# Contributor guide

Thank you for your interest in contributing to this project! 🌱


## How to contribute

For code, documentation or general improvements, feel free to open an Issue to discuss any ideas before submitting a Pull Request.

## Build and test

The library and CLI use Rust 2021, support Rust 1.73 or newer, and have no
third-party crate dependencies. Run these commands from the repository root:

```sh
cargo build --locked
cargo fmt --all -- --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-targets
cargo test --doc
python3 -m unittest discover -s tests -p 'test_*.py'
```

The Python checks require Python 3.10 or newer and use only its standard library.
Packaging checks also require a POSIX shell and `zip`; they skip when these are
unavailable and use a fake compiler rather than building a release.
See the [codebase guide](architecture.md) before adding a module or moving files.
Use four spaces for Python and shell indentation.

### Unit test layout

Put each module's unit tests in its own `tests.rs` file, including small suites,
and declare it with `#[cfg(test)] mod tests;`. Preserve descriptive implementation
filenames: for example, `src/format/gzip.rs` and `src/format/gzip/tests.rs`.
Keep helpers and methods used by a single suite in its `tests.rs`. Shared helpers,
fixtures, and accessors belong in a module-local `test_support.rs`, enabled only
under `#[cfg(test)]`. Keep only necessary instrumentation in implementation
files. See the [codebase guide](architecture.md) for the full layout.

### Rust formatting and comments

Run `cargo fmt --all` before submitting changes. The root `rustfmt.toml` sets
four-space indentation, LF line endings, and a 100-column code width.

Use `//!` for crate or module documentation at the start of the file. Use `///`
for descriptions of public and private items, including constants, functions,
types, fields, and variants. Use `//` for implementation notes inside function
bodies, including notes on local declarations, and for file-level license
headers. The markers are contiguous: `// !` is an ordinary comment, not module
documentation.

Put one space after the marker, align comments with the code they describe, and
write prose in complete sentences. Wrap standalone prose comments to 80 columns,
including indentation; stable `rustfmt` does not reflow comments. Preserve
Markdown structure, code examples, URLs, and identifiers when wrapping. Keep
SPDX license headers in their standard form.

### Private corpus regressions

The default tests use synthetic or existing in-source regression data and work
in a fresh checkout. Tests requiring the private corpus are marked `#[ignore]`
and read their fixtures at runtime. With the corpus available locally, run all
checks, including those regressions, with:

```sh
cargo test --all-targets -- --include-ignored
```

`tests/local_corpus.rs` contains the public-API corpus regressions; four additional
raw-optimizer regressions live in `src/deflate/optimize/tests.rs`. An explicitly
requested corpus test fails if its fixture is missing rather than silently
passing. The relative fixture names in these tests describe the expected local
layout under `tests/fixtures/`.

Corpus files may be copyrighted and must not be added to the public repository.
Keep `tests/fixtures/`, `work/`, `target/`, and `benchmark-results/` local; Git
ignores them and the Cargo package allowlist excludes them. Do not embed private
fixture bytes or absolute machine paths in new source, logs, or documentation.
Use generated fixtures for portable regression tests. Before sharing changes,
inspect the staged diff and `cargo package --list --allow-dirty`.

### Distribution executables

`utils/build-distribution.sh <target-triple>` builds with the pinned distribution
toolchain, remaps build paths, checks the final executable with
`utils/sanitize-binary-paths.py`, and creates an archive under `target/dist/`.
Use this script for distributable binaries; ordinary local Cargo builds can
retain compiler or checkout paths in diagnostics. The script reports required
toolchains, targets, and tools when they are unavailable.

Each packaging invocation stages its executable and archive in a private,
temporary directory under `target/dist/`. Concurrent architecture builds cannot
overwrite each other's staged files, and failed packaging preserves the previous
release archive. The path audit checks filesystem strings and both UTF-16 byte
orders, including Windows wide strings.


## Contributor License Agreement (CLA)

By submitting content to this project, you agree that:

1. **You have rights to contribute.** You own the submitted material or have the full rights and permissions to share it and to grant the licenses below.
2. **You keep ownership.** You retain copyright (and any other rights you may have) in your submission.
3. **You grant the Project Owner broad permission.** You grant the project owner (and their successors/assignees) a perpetual, worldwide, irrevocable, non-exclusive, royalty-free license to use, reproduce, modify, adapt, publish, distribute, publicly display/perform, create derivative works from, and otherwise exploit your submission **for any purpose, including commercial purposes**.
4. **Relicensing and sublicensing are allowed.** You agree the project owner may sublicense and/or relicense your submission and derivative works under **any** terms (including proprietary terms), and may include your submission in other projects or products.
5. **Database rights (if applicable).** To the extent your submission is protected by database rights (including _sui generis_ database rights), you grant the project owner the same license over those rights.
6. **Patent license (if applicable).** If your submission is or may be covered by patents you control, you grant the project owner a perpetual, worldwide, irrevocable, non-exclusive, royalty-free patent license to make, have made, use, offer for sale, sell, import, and otherwise exploit the submission.
7. **No compensation; no control.** You waive any claim to compensation for your submission and agree you will not assert claims to control the project owner’s use or licensing of it consistent with this CLA.
8. **Public project license.** You understand that the project is currently shared publicly under the [MIT](../LICENSE) license, and that the project owner may also offer the project (including your submission) under different licenses now or in the future.


## Code of Conduct

Please be respectful and constructive.

All contributors are expected to follow our [Code of Conduct](./CODE_OF_CONDUCT.md). In short: Let's keep being awesome to each other! 🙏


<hr>

If you have questions, feel free to open an Issue or contact the maintainer directly.
