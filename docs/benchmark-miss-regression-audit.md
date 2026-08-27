<!-- SPDX-License-Identifier: MIT -->

# Benchmark miss and regression audit

This audit keeps reference misses, historical Columbo-floor regressions, and
deadline-limited results separate. A positive reference delta means Columbo is
larger than the comparison program. A guard failure means only that the current
build did not reproduce an older Columbo result; it can still be substantially
smaller than DeflOpt, Defluff, or deft4j.

Private machine-readable states live under `work/`, which remains ignored by
Git. Public Markdown reports never relabel rows from an older executable as
current results.

## Evidence sets

| Evidence | Coverage | Executable SHA-256 | Finding |
| --- | ---: | --- | --- |
| DeflOpt complete journal | 1,914 rows / 957 pairs | `22fc1176…` | six strict-policy rows representing three files; no errors; Max never worse than Default |
| DeflOpt candidate family sample | 68 rows / 34 pairs | `e38b1e3c…` | two rows for one strict-policy file; no route miss or error; Max never worse than Default |
| Defluff candidate journal | all 66 pairs | `e38b1e3c…` | no miss or error |
| timed deft4j complete journal before candidate refresh | 1,621 rows | `22fc1176…` | twelve strict-policy rows and one safe PNG-preservation row; no errors |
| timed deft4j candidate family sample | 48 rows | `e38b1e3c…` | no miss or error |
| rolling priority guard | 100 unique files | `e38b1e3c…` | 96 floors pass; Max never trails current Default |

The integrated candidate is `target/release/columbo`, SHA-256
`e38b1e3c55292b2d143a4a43452c06526587fbe99546976f0895d2e4cde7a5ea`.
The complete DeflOpt journal above remains valid evidence for its recorded
binary; it has not been relabelled as a candidate result.

## Reference misses

### DeflOpt

The complete DeflOpt journal has six miss rows representing three files. All
six reach parity or better with `--strict 0`. They use compact
empty/singleton Huffman alphabets or the non-standard symbol-284 spelling of
length 258. Default Columbo intentionally remains strict, so these are output
policy differences rather than missing optimization routes.

The candidate size-spaced sample covers PNG, ZIP, and GZIP families in both
Default and Max. Its only two miss rows are Default and Max for the same
`samplelib-png/sample-green-400x300.png` input at +1 byte / +2 bits. Both
reach exact parity with `--strict 0`, so they are output-policy differences
rather than route misses. All 34 Max rows are no worse than their completed
Default result, and no row errors.

### Defluff

Defluff uses the compatibility-sensitive spellings exposed by `--strict 0`;
the comparison is therefore relaxed on both sides. All 66 candidate cases
meet or beat Defluff. Strictness is never relaxed in ordinary Default use.

### Timed deft4j

The last complete pre-candidate journal's thirteen reference misses are fully
classified: twelve reach parity or better with `--strict 0`, while
`oxipng/c2pa-signed.png` preserves an unknown unsafe-to-copy `caBX` chunk.
Columbo will not rewrite signed critical content around that chunk unless the
user explicitly requests stripping.

The candidate 48-case sample covers PNG, APNG, ZIP, GZIP, zlib, and the major
top-level fixture families. Every case meets or beats its timed deft4j
reference and none errors. A complete candidate refresh is required before
the official timed report can replace the older journal's classification.

## Matched candidate movement

The exact candidate samples were joined to the preceding complete journals by
format, source name, and mode. This compares identical files and references
rather than aggregate results from different samples.

- All 34 DeflOpt Default outputs are byte-for-byte and bit-for-bit identical.
- DeflOpt Max improves eight rows, ties 25, and loses one by 1 byte / 8 bits;
  the net movement is a 523-byte / 4,192-bit saving.
- All 66 Defluff outputs are identical to the preceding accepted run.
- Timed deft4j improves twelve rows, ties 33, and has three small ZIP
  movements totalling 2 bytes / 16 bits; the net movement is a 732-byte /
  5,849-bit saving.

Timing is effectively unchanged. On equal-timeout rows, median elapsed ratios
are 1.004× for DeflOpt Max and 1.002× for deft4j. Aggregate DeflOpt Max time is
348.91 seconds versus 348.70; aggregate deft4j time is slightly lower at
1,036.69 seconds versus 1,037.38. These sub-percent movements are scheduling
noise rather than evidence of a new speed cost.

## Hundred-file priority guard

`work/regression-guard.json` is authoritative. It retains the latest 100
unique `(format, source)` identities, and insertion deduplicates a recurring
file. The candidate run performed 108 serial trials including confirmation
reruns: 96 historical floors pass at their recorded allowance, and Max never
trails the Default result produced by the same executable.

All twenty historical regression annotations in the fresh complete DeflOpt
journal recover their older floors. Those recoveries save at least 219 bytes
and 1,743 meaningful bits relative to the immediately preceding current
results. One separate guard trades 5 bytes / 34 bits against that fresh
current result, leaving a minimum net improvement of 214 bytes / 1,709 bits.

| File | Mode | Byte loss | Bit loss | Disposition |
| --- | --- | ---: | ---: | --- |
| `oxipng/palette_should_be_reduced_with_missing.png` | Max | 0 | 1 | inherited equal-byte historical bit floor |
| `css-ig-net/sample_34-fs8.png` | Max | 4 | 28 | accepted guard trade-off; 5 bytes / 34 bits behind the fresh current result |
| `medium/LevelLoading.png` | Max+5s | 6 | 49 | inherited route basin; improved from the preceding 9-byte / 68-bit loss |
| `css-ig-net/bad-paletted.png` | Max | 6 | 45 | inherited route basin |

The accepted candidate restores the former DeflOpt regression cluster without
filename, directory, reference-score, or measured-runtime gates. In
particular, it restores the important `sample_71-fs8.png`,
`filter_0_none_sprite-frightened.png`, `sample_60-fs8.png`, and
`my-computer-off-fs8.png` floors. The latter now receives the same 120-bit
hard-boundary rescue in quiet, Verbose, and Visual modes.

The exact candidate comparison produces byte-for-byte and bit-for-bit
identical Default artifacts for all 34 sampled DeflOpt pairs. The changes
audited here are Max-only, and every sampled Max row still dominates its
completed Default row.

## Accepted general changes

The accepted rules are derived from search topology, complete-stream scoring,
and bounded resource models. They contain no corpus identity or expected
score.

### Topology-aware no-split order

The no-split route retains its deft4j-derived per-block seed, Columbo
length-family states, and adjacent source-order merges. Two- and three-block
lists expose at most two adjacent merge boundaries, so they prioritize
individual one-match pruning. Lists with four or more nonempty blocks
prioritize cumulative pruning, ensuring that local work on an early block
cannot exhaust the route window before later alignment and merge states are
visited. With sufficient time, Max prices the complementary policy as an
independent late route.

This preserves the short-list individual-pruning gains, including
`sample_69.png`, while restoring long-chain cumulative endpoints. It also
avoids forcing the result of one locally smaller block into the other route's
later alignment decisions.

### Independent topology closure

A smaller encoded sibling does not dominate another token/tree topology before
terminal tree closure. A floor-seeded, deft4j-derived, source-max, or changed
no-split parent can be slightly larger immediately yet reach the best result
after the bounded terminal methods change only its Huffman trees. Columbo now
closes such a losing independent topology before discarding it. A topology
that already wins receives the same closure once at the ordinary final Max
stage, avoiding duplicate work on the common path.

The same reasoning removes the old completed-sibling score gate from the
changed no-split dependency. That continuation still requires a genuine token
or boundary change and a strict improvement over the source, runs at most once,
and uses the existing source-max worker slot.

### Deterministic hard-boundary rescue

The bounded-depth rescue now prices the fixed Deflate-alphabet frontier for
every dynamic block rather than choosing one largest block. Admission remains
bounded to at most 1 MiB compressed, 1 MiB decoded, and 128 source blocks.
Once frequencies are known, the frontier size is independent of token count;
the existing caps bound reparse and whole-stream emission work.

This makes quiet, Verbose, and Visual reporting observational only. Crossing
the hard deadline a few milliseconds earlier can no longer change which block
receives the terminal rescue, and the optimizer never loses quality merely
because detailed progress is enabled.

### Existing accepted controls retained

The candidate retains the previously audited general controls:

- a complete Default floor is secured before timed Max work, so Max cannot
  return a worse result;
- active long-running trials finish cooperatively and forward their best
  complete incumbent inside timeout + 10% + 1 second;
- the narrow source route remains bounded to 1 MiB compressed input and 128
  nonempty blocks;
- independent deft4j and floor-seeded topologies may overlap inside the
  existing bounded worker and memory envelope;
- compact split first covers the structural cut set cheaply, then exactly
  finalizes the strongest bounded candidate;
- exact candidate identity and completed-plan caches prevent replaying the
  same token/tree state through equivalent route names.

## Accepted trade-offs

`css-ig-net/sample_34-fs8.png` is 5 bytes / 34 meaningful bits behind its
fresh pre-candidate result. Reinstating unconditional individual-first work for
every no-split list restores that isolated endpoint but again starves later
blocks in long lists. The accepted topology rule instead recovers all twenty
fresh DeflOpt historical regression floors for a minimum net 214-byte /
1,709-bit gain.

The remaining guard differences are kept visible. No filename-specific gate,
ten-second threshold, or reference score is used to hide them. A future change
must improve their general search class without sacrificing the broader
candidate gains.

## Verification

- `cargo test --release --locked`: 457 tests passed (404 library, 43 CLI, 10
  public API).
- `cargo fmt --all -- --check`: passed.
- `cargo clippy --release --all-targets --locked -- -D warnings`: passed.
- Full candidate Defluff replay: 66/66 pass with no errors.
- Candidate DeflOpt family sample: 68 rows / 34 pairs; one strict-policy file,
  no route miss or error, and Max never worse than Default.
- Candidate deft4j family sample: 48/48 reference gates pass with no errors.
- Matched samples: DeflOpt Default is identical, DeflOpt Max saves 523 bytes /
  4,192 bits net, and deft4j saves 732 bytes / 5,849 bits net with no material
  runtime movement.
- Rolling priority guard: 96/100 floors pass across 108 serial trials; all
  twenty fresh DeflOpt historical regression floors recover.
- The hard-boundary rescue reproduces the same result in quiet and detailed
  reporting.
- `git diff --check`: passed.
- Tracked source and `Cargo.toml` contain no user, repository, or temporary
  absolute path. Distribution binaries remain subject to the release path
  sanitizer; ordinary developer builds may retain toolchain paths.

Accepted routing and provenance are maintained in
`docs/routes-and-methods.md`.
