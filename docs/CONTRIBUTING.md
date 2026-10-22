# Contributor guide

Thank you for your interest in contributing to this project! 🌱


## How to contribute

For code, documentation or general improvements, feel free to open an Issue to discuss any ideas before submitting a Pull Request.

### Keep Pull Requests focused

Keep each Pull Request focused on a single logical change. Small, atomic changes are easier to review, test, and merge.

Avoid combining unrelated fixes, refactoring, formatting, documentation changes, or new features in the same Pull Request. If changes can be reviewed and accepted independently, please submit them as separate Pull Requests.

Where possible, limit the diff to files and lines directly relevant to the change. This shows reviewers the intent and reduces the chance that unrelated changes block an otherwise useful contribution.

### AI-assisted contributions

AI tools may be used to assist with contributions, but contributors remain fully responsible for everything they submit.

Review and understand all AI-generated code, documentation, tests, and comments before submitting. Check that the contribution is correct, necessary, consistent with the existing codebase, and covered by appropriate tests.

Keep AI-assisted changes focused and minimal. Avoid large rewrites, speculative refactoring, excessive comments, or unrelated changes introduced by an AI tool. Where AI assistance was substantial, please mention this and briefly describe how it was used.

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
headers.

### Private corpus regressions

The default tests use synthetic or existing in-source regression data and work
in a fresh checkout. Tests requiring the private corpus are marked `#[ignore]`
and read their fixtures at runtime. With the corpus available locally, run all
checks, including those regressions, with:

```sh
cargo test --all-targets -- --include-ignored
```

`tests/local_corpus.rs` contains the public-API corpus regressions; four additional
raw-optimizer regressions live in `src/deflate/optimize/tests.rs`. A corpus
test run with `--ignored` or `--include-ignored` fails if its fixture is missing
rather than passing silently. The relative fixture names in these tests describe
the expected local layout under `tests/fixtures/`.

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

The script builds the release profile, which aborts on panic, with the pinned
nightly toolchain rebuilding the standard library to abort immediately, which
leaves out panic messages and their source locations. Its executable, `target/<target-triple>/release/columbo`, is
smaller and slightly faster than an ordinary `cargo build --release`. Benchmark
and time this executable, so measurements reflect what is distributed; use
another build only when a measurement needs debug information or
instrumentation.

Each run stages its executable and archive in a private temporary directory
under `target/dist/`, so concurrent builds cannot overwrite each other's files
and a failed run keeps the previous release archive. The path audit also checks
UTF-16 strings in both byte orders, such as Windows wide strings.


## Contributor License Agreement (CLA)

By submitting content to this project, you agree that:

1. **You have rights to contribute.** You own the submitted material or have the full rights and permissions to share it and to grant the licenses below.
2. **You keep ownership.** You retain copyright (and any other rights you may have) in your submission.
3. **You grant the Project Owner broad permission.** You grant the project owner (and their successors/assignees) a perpetual, worldwide, irrevocable, non-exclusive, royalty-free license to use, reproduce, modify, adapt, publish, distribute, publicly display/perform, create derivative works from, and otherwise exploit your submission **for any purpose, including commercial purposes**.
4. **Relicensing and sublicensing are allowed.** You agree the project owner may sublicense and/or relicense your submission and derivative works under **any** terms (including proprietary terms), and may include your submission in other projects or products.
5. **Patent license (if applicable).** If your submission is or may be covered by patents you control, you grant the project owner a perpetual, worldwide, irrevocable, non-exclusive, royalty-free patent license to make, have made, use, offer for sale, sell, import, and otherwise exploit the submission.
6. **No compensation; no control.** You waive any claim to compensation for your submission and agree you will not assert claims to control the project owner’s use or licensing of it consistent with this CLA.
7. **Public project license.** You understand that the project is currently shared publicly under the [MIT](../LICENSE) license, and that the project owner may also offer the project (including your submission) under different licenses now or in the future.


## Code of Conduct

Please be respectful and constructive.

All contributors are expected to follow our [Code of Conduct](./CODE_OF_CONDUCT.md). In short: Let's keep being awesome to each other! 🙏


<hr>

If you have questions, feel free to open an Issue or contact the maintainer directly.
