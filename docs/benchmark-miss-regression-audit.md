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
| DeflOpt last complete journal | 1,914 rows / 957 pairs | `a700a360…` | six strict-policy rows representing three files; no errors; Max never worse than Default |
| Candidate family sample | 68 rows / 34 pairs | `cef3db1d…` | two rows for one strict-policy file; no route miss or error; Max never worse than Default |
| Defluff candidate journal | all 66 pairs | `cef3db1d…` | no miss or error |
| timed deft4j last complete journal | 1,621 rows | `a700a360…` | twelve strict-policy rows and one safe PNG-preservation row; no errors |
| targeted deft4j recovery | 2 affected files, 3 runs each | `cef3db1d…` | both former 9-byte regressions recover their exact older byte and bit floors |
| rolling priority guard | 100 unique files | `cef3db1d…` | 96 floors pass; no new failure; Max never trails current Default |

The integrated candidate is `target/release/columbo`, SHA-256
`cef3db1d92471ddfff1dc9b2ddd6661c7e2c37955deeb64daadb9bcf0289a79a`.
The complete DeflOpt and timed deft4j journals remain valid evidence for their
recorded preceding binary; neither has been relabelled as a candidate result.
A final DeflOpt refresh was stopped after 274 Default rows at the user's
request. Its private state is resumable, but the partial generated Markdown
was not substituted for the last complete public report.

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

The two material prior-result annotations in the last complete journal were
`oxipng/interlaced_rgb_8_should_be_palette_1.png` and
`oxipng/interlaced_rgba_8_should_be_palette_2.png`. The candidate recovers
their exact older floors: 2,302 bytes / 17,911 bits and 2,870 bytes / 22,451
bits respectively. Each recovery was reproduced three times. A complete
candidate refresh is still required before the official timed report can
replace the preceding journal's classification.

## Candidate movement

The exact candidate samples were joined to the preceding complete journals by
format, source name, and mode. This compares identical files and references
rather than aggregate results from different samples.

- All 34 sampled Default outputs are byte-for-byte and bit-for-bit identical
  to the immediately preceding source-max scheduling candidate.
- Of the 34 sampled Max outputs, 31 are exact ties, one improves by 60 bytes /
  478 bits, and two do not reproduce smaller timed endpoints by 33 bytes / 264
  bits and 109 bytes / 877 bits. Those three inputs exceed the new 16 KiB
  scheduling class, so they cannot take the changed branch; the variation is
  from deadline-limited search. Every result still passes the reference and
  Max-over-Default gates.
- The complete Defluff comparison has 58 wins and eight ties, for a net saving
  of 102 bytes / 852 meaningful Deflate bits.
- The two deft4j rows directly affected by the scheduling change recover 18
  bytes and 140 bits in total.

Aggregate sampled Default time moved from 91.70 to 98.82 seconds and Max time
from 407.70 to 409.54 seconds. A single timed sample cannot establish a speed
change; importantly, the branch is bounded to compact graphs and does not run
on the two sampled Max rows with smaller prior timed endpoints.

## Hundred-file priority guard

`work/regression-guard.json` is authoritative. It retains the latest 100
unique `(format, source)` identities, and insertion deduplicates a recurring
file. The candidate run performed 108 serial trials including confirmation
reruns: 96 historical floors pass at their recorded allowance, the same four
residual files remain, and Max never trails the Default result produced by the
same executable. The two newly recovered deft4j files are both present in the
deduplicated guard tail.

| File | Mode | Byte loss | Bit loss | Disposition |
| --- | --- | ---: | ---: | --- |
| `medium/Nutcracker.png` | Max+5s | 1 | 7 | inherited timed route basin |
| `oxipng/palette_should_be_reduced_with_missing.png` | Max | 0 | 1 | inherited equal-byte historical bit floor |
| `css-ig-net/sample_34-fs8.png` | Max | 4 | 28 | accepted timed route basin |
| `medium/LevelLoading.png` | Max+5s | 6 | 49 | inherited timed route basin |

The exact candidate comparison produces byte-for-byte and bit-for-bit
identical Default artifacts for all 34 sampled pairs. The changes audited here
are Max-only, and every sampled Max row still dominates its completed Default
row.

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

### Deferred source-max structural closure

A compact source-max result can expose a useful split topology even when that
result is not the selected timed-route winner. Under the stream-owning
`Complete` and `CompleteThenBounded` policies, Columbo now retains one such
completed parent and runs the existing bounded coarse-to-fine split closure
after ordinary timed siblings finish. The deferred work is limited to 16 KiB
compressed, 256 KiB decoded, 16 Ki tokens, and one to four nonempty non-stored
blocks. A one-block parent is valid because the dependent method creates the
second Huffman regime rather than requiring one in the source.

Shared container policies do not receive this terminal overrun. That prevents
one ZIP member, GZIP member, metadata stream, or APNG frame from consuming time
reserved for later streams. Complete candidate comparison makes the closure
additive: it cannot replace the retained floor with a larger result.

### Bounded long-match source scheduling

For a large-decoded, one-block PNG, the complete-floor beam normally owns
cache and range-materialization bandwidth before source max. Two compact graph
shapes justify overlap: at least one independent repartition run per 16 source
tokens, or at most 4,000 tokens averaging at least 224 decoded bytes per token.
The latter threshold is seven eighths of Deflate's 256-byte maximum match, so
it identifies graphs made almost entirely from long matches. Such a graph has
little literal/alphabet work per decoded byte and is cheap to price within the
existing 16 KiB compressed work class.

This is a topology and work bound, not a filename, corpus family, reference
score, or measured-runtime gate. It restores the two affected interlaced PNG
floors while leaving `oxipng/issue-59.png` on the completed-floor-first path
and preserving the dense-repartition path used by `GK1.png`.

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

The four remaining guard differences are kept visible. They total 11 bytes
and 85 meaningful bits across independent deadline-limited searches; the
largest is 6 bytes / 49 bits. No filename-specific gate, ten-second threshold,
or reference score is used to hide them. A future change must improve their
general search classes without sacrificing broader gains.

The two smaller sampled Max endpoints that were not reproduced are outside
the changed work class and still pass both the external reference and
Max-over-Default gates. They are treated as timed search variance, not as a
reason to add an unrelated corpus-specific scheduling rule.

## Verification

- `cargo test --locked`: 481 tests passed (425 library, 46 CLI, 10 public API).
- `cargo fmt --check`: passed.
- `cargo clippy --locked --all-targets -- -D warnings`: passed.
- Full candidate Defluff replay: 66/66 pass with no errors.
- Candidate DeflOpt family sample: 68 rows / 34 pairs; one strict-policy file,
  no route miss or error, and Max never worse than Default.
- The two targeted deft4j regressions recover their exact historical byte and
  bit floors in three candidate runs each.
- Matched sample: all 34 Default outputs are identical; 31 Max outputs are
  identical; every Max output dominates its current Default result.
- Rolling priority guard: 96/100 floors pass across 108 serial trials, with no
  new failure and exactly 100 unique entries.
- A final complete DeflOpt refresh was intentionally interrupted after 274
  Default rows. The last complete public report remains the authoritative
  full-corpus result until the private checkpoint is resumed.
- `git diff --check`: passed.
- Tracked source and `Cargo.toml` contain no user, repository, or temporary
  absolute path. Distribution binaries remain subject to the release path
  sanitizer; ordinary developer builds may retain toolchain paths.

Accepted routing and provenance are maintained in
`docs/routes-and-methods.md`.
