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

| Evidence | Coverage | Executable SHA-256 | Current finding |
| --- | ---: | --- | --- |
| DeflOpt full journal | 1,914 rows / 957 pairs | `c09b1fc3…` | 14 strict-policy rows; no errors |
| DeflOpt current family sample | 36 rows / 18 pairs | `9779c011…` | no miss, no error, Max never worse than Default |
| Defluff relaxed journal | all 66 pairs | `3af16f7a…` | no miss, error, or prior-result regression; 46 outputs improve and none regress |
| timed deft4j full journal | 1,621 rows | `ef63e322…` | 17 strict-policy rows and one safe PNG-preservation row; no errors |
| timed deft4j current family sample | 25 rows | `9779c011…` | one known strict-policy miss; no errors |
| timed deft4j current PngSuite group | all 161 paired files | `9779c011…` | three strict-policy rows; no errors; 38 bytes / 298 bits smaller than the old journal |
| Steam APNG stopped full journal | 2,431 rows | `f287245e…` | historical evidence only: 12 misses and 54 errors |
| Steam APNG current scoped replay | 12 former misses in Max and Default | `fa9b3559…` | no reference miss, Default/Max gate failure, or format error |
| Priority guard | 100 unique files | `7ae764de…` | 91 floors pass at their recorded allowance; two more recover with longer time; Max never trails current Default |

The current candidate is `target/release/columbo`, SHA-256
`7ae764def6e6adb16ab76227a2567d41036776501c1dd92ffbd5619c62131140`.
The complete Defluff journal and the DeflOpt, deft4j, PngSuite, and scoped APNG
samples above retain their recorded executable hashes. The priority guard is
the current-source evidence; older journals have not been relabelled as
current results.

## Reference misses

### DeflOpt

The complete DeflOpt journal has 14 miss rows representing nine files. Every
one reached parity or better in the earlier `--strict 0` audit. They use compact
empty/singleton Huffman alphabets or the non-standard symbol-284 spelling of
length 258. Default Columbo intentionally remains strict, so these are output
policy differences rather than missing optimization routes.

The current size-spaced family sample covers PNG, ZIP, and GZIP families in
both Default and Max. All 36 rows meet DeflOpt parity, Max is never worse than
its completed Default result, and no row errors. Against the preceding sample,
aggregate movement is -46 bytes / -367 meaningful bits. Two Max rows improve;
one equal-byte ZIP row is one bit longer on two isolated confirmations. The
full historical and current hashes remain explicitly distinct until a complete
refresh finishes.

### Defluff

Defluff uses the compatibility-sensitive spellings exposed by
`--strict 0`; the comparison is therefore relaxed on both sides. All 66 cases
complete. Columbo is equal on eight, strictly better on 58, and smaller by 102
bytes / 852 meaningful bits in aggregate. Compared with the preceding accepted
run, 46 outputs improve by 70 bytes / 534 bits in aggregate and none regress.
Strictness is never relaxed in ordinary Default use.

### Timed deft4j

The old complete journal's 18 reference misses are fully classified: 17 are
strict-output-policy differences, while `oxipng/c2pa-signed.png` preserves an
unknown unsafe-to-copy `caBX` chunk. Columbo will not rewrite signed critical
content around that chunk unless the user explicitly requests stripping.

The current 25-case sample covers every wrapper and top-level family. Its only
reference miss is `8x8-png/waves.png` at +1 byte / +4 bits, the same known
strict-policy case; relaxed Max reaches equal bytes and two fewer bits. Against
the preceding sample it is smaller by 211 bytes / 1,701 meaningful bits in
aggregate. At their normal allowances, `Animated.png` and `bored.png` were one
and 28 bytes above the preceding sample. Longer isolated runs prove these are
timing-basin losses rather than removed endpoints: at 30 and 60 seconds they
reach 5,945 bytes / 41,923 bits and 246,079 bytes / 1,949,516 bits, improving
the preceding floors by 6 bytes / 35 bits and 13 bytes / 141 bits respectively.

Because ten retained guard differences form one PngSuite cluster, the exact
current binary was also run over all 161 paired files in that group. Columbo is
strictly better than timed deft4j on 114, equal on 44, and smaller by 3,332
bytes / 26,603 meaningful bits in aggregate. The other three rows differ only
under strict output: `basi0g01.png` by three bits and `cs5n3p08.png` plus
`cs8n3p08.png` by two bits each; all reach parity or better with `--strict 0`.
There are no errors. Against the older complete journal, 157 outputs are
identical, three improve, and only `g10n3p04.png` is four bits longer at equal
file size, yielding a net 38-byte / 298-bit improvement.

### Steam Stickers APNG

The old full APNG state is retained only as historical diagnosis. The
benchmarked optimizer candidate was replayed on all twelve old misses; every
one beats the same timed deft4j reference. The aggregate advantage is 12,335
bytes / 98,686 bits.

Those twelve files also pass the Default/Max policy gates: every Default run is
faster, and every Max artifact is no worse in both file bytes and aggregate
meaningful bits. Default saves 14,700 bytes / 117,532 bits from the sources in
aggregate; Max retains a strict quality lead for every row.

All 54 former error fixtures now complete in Default with identical decoded
image/frame streams. Fifty-two contain the same invalid exporter vestige: an
RGBA PNG with a palette-sized `tRNS` after a valid suggested `PLTE`. Columbo
removes only that specification-forbidden shape. Two have bytes after `IEND`,
which are outside the PNG datastream and are discarded. The private comparator
normalizes only the exact RGBA/PLTE/`tRNS` signature; its unit tests prove that
missing palettes and oversized transparency data remain errors. One Max file
from each of fourteen structural families also completes and beats deft4j,
by 20,169 bytes / 151,865 bits in aggregate.

## Hundred-file priority guard

`work/regression-guard.json` remains authoritative. It contains the latest 100
unique `(format, source)` identities; insertion deduplicates a recurring file.
The expanded guard currently covers 71 deft4j cases and 29 DeflOpt cases across
96 PNG and four ZIP inputs. The complete current-source run performed 118
serial trials, including confirmation reruns: 91 historical floors pass at
their recorded allowance and nine remain. Every Max+5s row equals or beats the
Default result produced by the same executable. The failures below compare
only with older Columbo peaks.

A separate old-HEAD/current-source comparison produced byte-for-byte identical
Default artifacts for a five-file stride through the guard plus explicit PNG,
ZIP, GZIP, and zlib coverage: 22 of 22 outputs matched. The Max-floor changes
therefore do not alter Default routing or output.

| File | Mode | Byte loss | Bit loss | Disposition |
| --- | --- | ---: | ---: | --- |
| `medium/LevelLoading.png` | Max+5s | 9 | 68 | older route basin; not recovered at 60 s |
| `small/check.png` | Max+5s | 1 | 1 | recovered at 20 s |
| `pkmn-bw/000-Logo-2.png` | Max | 0 | 2 | equal bytes; historical bit floor |
| `css-ig-net/bad-paletted.png` | Max | 6 | 45 | older route basin; not recovered at 60 s |
| `css-ig-net/Mango512.png` | Max | 111 | 888 | recovered at 32 s |
| `PngSuite/cdhn2c08.png` | Max | 2 | 12 | older route basin; not recovered at 60 s |
| `PngSuite/bgyn6a16.png` | Max | 12 | 97 | identical raw stream/floor with `basn` and `bgan` |
| `PngSuite/bgan6a16.png` | Max | 12 | 97 | identical raw stream/floor with `basn` and `bgyn` |
| `PngSuite/basn6a16.png` | Max | 12 | 97 | identical raw stream/floor with `bgan` and `bgyn` |

The accepted narrow-continuation changes newly recover four substantial guards
without a filename or timing gate:

- `medium/BNDT_on_X____stack__https___t_co_3VRfGVC7bX____X.png` reaches its
  large-model historical floor after raising the narrow route's paired
  compressed-size ceiling from 512 KiB to 1 MiB;
- `css-ig-net/file04.png` reaches its floor after keeping the independent
  deft4j refinement live beside a long floor-seeded continuation;
- `css-ig-net/Mango512.png` reaches 140,112 bytes / 1,097,291 bits, improving
  its floor by 6 bytes / 49 bits;
- `css-ig-net/Sock.png` reaches 85,956 bytes / 686,617 bits, improving its
  floor by 32 bytes / 258 bits.

Earlier accepted changes also retain these important guards at their normal
allowance:

- `css-ig-net/sample_69.png`: 50,036 bytes / 399,784 bits, improving its
  historical floor after restoring complementary individual pruning to the
  source-ordered no-split route;
- `oxipng/rgba_16_should_be_palette_2.png`: 3,751 bytes / 29,500 bits;
- `css-ig-net/sample_71-fs8.png`: 11,013 bytes / 80,136 bits in three repeated
  runs;
- `css-ig-net/dossier-green-normal-fs8.png`: 16,384 bytes / 121,888 bits;
- `small/check.png`: at or below its 1,586-byte / 9,008-bit guard floor;
- `css-ig-net/sample_38-fs8.png`: at or below its 6,220-byte / 42,190-bit guard
  floor;
- `medium/Death.png`: 21,099 bytes / 165,113 bits at ten seconds, improving the
  historical 21,101-byte / 165,133-bit floor;
- `css-ig-net/motorcycle.png`: 5,716 bytes / 44,923 bits at ten seconds,
  reproducing its historical compact-split floor inside the critical envelope.

Longer trials distinguish deadline pressure from route removal. Besides the
ZIP case above, historical `small/bomb.png` and `medium/loupe-fs8.png` floors
recover or improve with 30 seconds. The current `small/check.png` miss recovers
at 20 seconds and `Mango512.png` recovers at 32 seconds. Three independent
60-second trials do not recover `LevelLoading.png`, `000-Logo-2.png`,
`bad-paletted.png`, `cdhn2c08.png`, or the shared `basn` stream family, so none
is described as time-limited.

The three `basn`/`bgan`/`bgyn` fixtures contain the same raw Deflate stream and
therefore represent one structural miss, not three independent optimizer
failures. The current build produces 3,415 bytes / 26,688 bits for
`basn6a16.png` at both ten and 60 seconds, still above the 3,403-byte /
26,591-bit recorded floor. Strictness does not change that result.

The PngSuite floors came from an unretained experimental executable. Rebuilding
every unique reachable v0.4 source snapshot did not reproduce them. Retained
older builds range from 3,432 bytes / 26,818 bits to 3,425 bytes / 26,762 bits;
the current invariant build reaches 3,415 bytes / 26,688 bits. The guard
remains useful evidence that a search
basin may have been lost, but more time, relaxed output, repeated current
passes, and every reconstructable historical source have not recovered it.

## Accepted general changes

The accepted rules contain no filename, directory, reference score, or
measured ten-second special case.

### Priority boundary graph

Once a complete incumbent protects source-order work, a compact one-block
source or its first two-block replay can price the independent global boundary
graph first when it has at most 8,192 tokens, at least 100,000 decoded bytes,
and at least two same-distance repartition runs. The graph is the only route
that combines distant cut anchors; larger block lists keep source order first
because their graph grows much faster. Sufficient-time route coverage is
unchanged. This restores the RGBA palette case at ten seconds without changing
its interlaced sibling or creating a new priority-guard regression.

### Best-first floor-seeded continuation

At a bounded-route rejoin, an unfinished floor-seeded endpoint continues before
weaker complete parents only when it strictly beats the ordinary floor and no
deft4j, narrow, or source-Max sibling beats it. This is dependency ordering,
not pruning: the incumbent remains complete and sufficient time still admits
every sibling. It restores `sample_71-fs8.png` at ten seconds and improves two
DeflOpt-family rows plus several deft4j/APNG rows.

### Graceful coarse-to-fine compact split

The compact-split route no longer performs an untimed complete sweep after the
critical file deadline. It first covers every structural cut with the fast
ordinary Huffman planner, exhaustively finalizes only the winning
alignment-independent suffix, and then spends remaining hard time on the exact
Max sweep. Every stage forwards a complete parent or descendant. Unlimited
tests prove that the exact route is never lost; at ten seconds this keeps
`sample_24-fs8.png` at 36,097 bits while reducing its elapsed optimization from
about 17.1 to 11.8 seconds and restores `dossier-green-normal-fs8.png` exactly.
If the hard boundary is already reached before the first cut, the one stream
owning file-level grace ordinary-prices at most fourteen eighth cuts on the
largest eligible block and exact-prices only the strongest child pair. This
recovers `motorcycle.png` from 5,720 bytes / 44,953 bits to its historical
5,716-byte / 44,923-bit floor in about 11.8 seconds; the former six-exact-trial
rescue needed about 13 seconds. Zero-grace container children and route-window
yields cannot run the rescue, so APNG/ZIP stream counts cannot multiply grace.

### No-split route ownership

The no-split route retains its deft4j-derived per-block seed, Columbo cumulative
length-family states, and adjacent source-order merges. Source max now owns the
individual one-match pruning family exclusively. Removing that duplicated
walk lets no-split reach later blocks: `Death.png` reaches 165,113 bits at ten
seconds and 165,101 bits at thirty seconds, versus the 165,133-bit historical
floor. More time therefore recovers the remaining short-budget bits without a
file-specific rule.

### Bounded narrow-route model

The narrow source route now accepts up to 1 MiB of compressed input while
retaining its 128-nonempty-block ceiling. The route is linear in the source
block list, and its per-block candidate storage remains below the existing
64 MiB parallel-model class under those paired bounds. This is a resource-model
extension rather than a corpus threshold; it restores the large `BNDT` guard
without admitting unbounded state growth.

### Independent deft4j refinement overlap

A strong floor-seeded endpoint does not dominate an independent deft4j-derived
topology merely because its current encoded score is smaller. When the bounded
parallel work class is available, Columbo therefore keeps the deft4j refinement
live beside the floor-seeded continuation. It reuses the historical
at-most-three-worker envelope and one file deadline rather than adding time or
memory. A transformed direct-deft parent may receive one no-split refinement
and one Max continuation; header-only changes do not trigger either. This
restores `file04.png` without delaying the winning floor lineage.

### Non-dominated changed-parent continuation

When the narrow source route emits a strict score improvement with genuinely
different tokens or block boundaries, Columbo may run the distinct no-split
transformation once more. It does so only while no completed floor,
floor-seeded, deft4j, source-Max, or exact-Default sibling strictly beats that
parent. The dependency takes the source-Max worker slot, preserving the prior
worker and model limits; source Max remains eligible later if time remains.
This score/topology rule improves `Mango512.png` and `Sock.png`, and longer APNG
checks show that it has delayed rather than removed the older `Animated.png`
and `bored.png` endpoints.

### Narrow PNG invalid-exporter repair

The runtime accepts and removes only a structurally consistent palette-shaped
`tRNS` vestige from RGBA, and ignores bytes after the terminating `IEND`.
Decoded streams remain mandatory validation evidence. Other invalid `tRNS`
forms and rewrite-sensitive unknown ancillary combinations remain rejected.

## Rejected or accepted trade-offs

Separating the all-literal endpoint into another full lineage recovers the
50-byte `LevelLoading.png` floor, but broad controls lose roughly 9.5 KiB on a
floor-pattern case and 3.6 KiB on `Partnership.png`, or nearly double runtime
under the less damaging ordering. That experiment was rejected. The current
50-byte / 397-bit local loss is accepted because the integrated endpoint opens
much larger, general gains elsewhere.

The remaining isolated differences range from equal-byte bit floors through a
6-byte / 45-bit `bad-paletted.png` loss. Reintroducing a second unconditional
complete no-split lineage can recover some older Zopfli-sensitive basins, but
duplicates substantial Max work and reverses much larger wins. The three
22-byte PngSuite rows are one identical-stream structural cluster whose
historical executable cannot be reconstructed; neither extra time nor repeated
current passes recover its floor. These cases remain priority guards rather
than being hidden by file-specific scheduling.

## Verification

- `cargo test --release --all-targets`: 429 tests passed (379 library, 43 CLI,
  7 public API).
- `cargo fmt --all -- --check`: passed.
- `cargo clippy --release --all-targets -- -D warnings`: passed.
- Private benchmark-tool discovery: 175 tests passed, including the narrowed
  APNG comparator and 100-entry guard checks. The guard's structural preflight
  validates all 100 unique cases.
- Full current Defluff replay: 66/66 pass.
- Current DeflOpt family sample: 36/36 reference gates pass; Max never worse.
- Current deft4j family sample: only the known strict-policy `waves.png` row.
- Current PngSuite group: all 161 paired cases complete; three strict-policy
  rows reach relaxed parity, and no row errors.
- The exact current build reproduces all 61 outputs in those two samples and
  passes the `BNDT`, `file04`, `Mango512`, and `Sock` guard floors in four
  serial runs.
- Current APNG scoped checks: 12/12 former misses pass; 54/54 former errors
  validate; 14/14 Max family representatives beat deft4j.
- `git diff --check`: passed.
- Tracked source and `Cargo.toml` contain no user, repository, or temporary
  absolute path. The distribution-path auditor passes both retained platform
  executables; ordinary developer builds are not distribution artifacts and
  may retain toolchain paths until the release sanitizer runs.

Accepted routing and provenance are maintained in
`docs/routes-and-methods.md`.
