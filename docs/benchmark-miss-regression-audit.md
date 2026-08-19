<!-- SPDX-License-Identifier: MIT -->

# Benchmark miss and regression audit

This audit combines the DeflOpt, Defluff, and timed deft4j benchmark journals.
It keeps three different questions separate:

1. Is a reference smaller because it emits a relaxed Deflate spelling that
   strict Columbo intentionally rejects?
2. Is a reference using a container rewrite that Columbo cannot safely copy?
3. Did a newer Columbo search schedule lose a result that an older Columbo
   build had already found?

The detailed row data remains in `docs/deflopt-benchmark.md`,
`docs/defluff-benchmark.md`, and `docs/deft4j-timed-benchmark.md`. The private
machine-readable sources are `work/deflopt-benchmark.json`,
`work/defluff-benchmark.json`, `work/deft4j-timed-benchmark.json`, and
`work/regression-guard.json`. Final-build broad samples and complete miss
replays are retained under `work/*-final-broad-sample.json` and
`work/*-final-miss-audit.json`; `/work/` remains ignored by Git.

## Snapshot summary

| Benchmark | Completed rows | Recorded miss rows | Unique recorded miss files | Current disposition | Prior-result regression files | Errors |
| --- | ---: | ---: | ---: | --- | ---: | ---: |
| DeflOpt | 1,914 | 14 | 9 | 14 strict-policy rows | 11 | 0 |
| Defluff (`--strict 0`) | 66 | 0 | 0 | — | 0 | 0 |
| timed deft4j | 1,621 | 18 | 18 | 17 strict-policy rows; 1 PNG preservation-policy row | 9 | 0 |
| Combined | 3,601 | 32 | 25 | 31 strict-policy rows; 1 PNG preservation-policy row; no unclassified row | 12 unique | 0 |

Every recorded reference difference has a confirmed strict or safe-preservation
policy explanation. There is no unclassified optimization-engine miss in the
three complete journals.

The complete DeflOpt and like-for-like Defluff refreshes use the current
distribution binary SHA-256
`5bac688c214e576cdfb54bc04b463c8e102a7f3f46f545f06205c5daea8db48e`.
The complete timed-deft4j journal and its miss replays use the earlier binary
SHA-256
`ef63e3228ee65b70fd14aa0fea4345af70893e3535928a572ec81003fa5ce44c`;
the accepted candidate and 40-file guard use
`9f8262d6cd667295b2863d4835d7b69d777514ba94b31d53a19992568fff839e`.
These results are kept separate so executable hashes are not mixed within a
journal.
The changed-parent no-split candidate is intentionally kept in separate sample
state while it is validated, so the complete journals do not mix executable
hashes.

## DeflOpt misses

Every DeflOpt miss reached parity or better in the earlier `--strict 0`
classification audit. Default mode must remain strictly compliant by default,
so these are documented policy differences rather than search failures. The
latest complete run records the same 14 strict deltas for later investigation
without rerunning relaxed audits. Deltas below are the largest strict delta
recorded for the file; positive means Columbo is larger.

| File | Affected modes | Bytes | Bits | Relaxed audit |
| --- | --- | ---: | ---: | --- |
| `css-ig-net/sample_40.png` | Default | 0 | 2 | parity |
| `medium/global.png` | Default | 0 | 2 | parity |
| `medium/tiger.png` | Default, Max | 0 | 2 | parity |
| `pkmn-bw/023-Ekans-2.png` | Default, Max | 1 | 5 | parity or better |
| `pkmn-bw/060-Poliwag-2.png` | Default, Max | 1 | 4 | better |
| `pkmn-bw/132-Ditto-2.png` | Default, Max | 0 | 5 | parity or better |
| `samplelib-png/sample-green-400x300.png` | Default, Max | 1 | 2 | parity |
| `small/T_Grass.png` | Default, Max | 2 | 15 | parity or better |
| `small/profle.png` | Default | 1 | 1 | parity |

The complete current-binary refresh covers all 15 rows above, and Max was never
worse than its completed Default result. The prior relaxed audit remains the
classification evidence; it is not represented as current-hash row metadata.

## Defluff comparison

Defluff permits compatibility-sensitive empty or singleton Huffman alphabets,
so its benchmark now invokes Columbo with `--strict 0` for a like-for-like
comparison. Default Columbo remains strict and continues to emit complete
Huffman codes for compatibility with old Windows Explorer decoders.

The complete relaxed run has 66 rows, no errors, no prior-result regressions,
and no misses. The two former strict-policy differences are now resolved in
the benchmark: `pkmn-bw-hard/023-Ekans-2.png` is equal in file bytes and one
Deflate bit better, while `pkmn-bw-hard/060-Poliwag-2.png` is one byte and
seven bits better than Defluff.

Across all 66 files, relaxed Columbo is 32 bytes and 318 meaningful Deflate
bits smaller than Defluff in aggregate.

## Timed deft4j misses

The recorded journal contains 18 misses. Seventeen reach parity or better with
`--strict 0`. The remaining file,
`oxipng/c2pa-signed.png`, contains the unknown unsafe-to-copy ancillary chunk
`caBX`. Columbo preserves the complete source unless stripping is explicitly
requested; copying that chunk after changing critical image data would not be a
safe optimization target.

| File | Timeout | Bytes | Bits | Classification |
| --- | ---: | ---: | ---: | --- |
| `8x8-png/symbols.png` | 10 | 0 | 3 | strict policy |
| `8x8-png/waves.png` | 10 | 1 | 4 | strict policy |
| `PngSuite/basi0g01.png` | 10 | 0 | 3 | strict policy |
| `PngSuite/cs5n3p08.png` | 10 | 0 | 2 | strict policy |
| `PngSuite/cs8n3p08.png` | 10 | 0 | 2 | strict policy |
| `oxipng/c2pa-signed.png` | 10 | 0 | 2 | safe preservation policy |
| `pkmn-bw-hard/029-Nidoran♀-2.png` | 10 | 0 | 3 | strict policy |
| `pkmn-bw-hard/066-Machop-0.png` | 10 | 0 | 3 | strict policy |
| `pkmn-bw-hard/149-Dragonite-0.png` | 10 | 0 | 3 | strict policy |
| `pkmn-col-hard/066-Machop-2.png` | 10 | 1 | 3 | strict policy |
| `pkmn-col/022-Fearow-2.png` | 10 | 0 | 3 | strict policy |
| `samplelib-png/sample-blue-200x200.png` | 10 | 0 | 2 | strict policy |
| `samplelib-png/sample-green-200x200.png` | 10 | 0 | 2 | strict policy |
| `samplelib-png/sample-green-400x300.png` | 10 | 1 | 2 | strict policy |
| `samplelib-png/sample-red-200x200.png` | 10 | 0 | 2 | strict policy |
| `small/T_Grass.png` | 10 | 1 | 12 | strict policy |
| `small/text.png` | 10 | 0 | 3 | strict policy |
| `samplelib-zip/sample-simple.zip` | 10 | 1 | 5 | strict policy |

The final-build replay covers every row in this table under its recorded
timeout. All 17 strict-policy rows reach parity or better in the relaxed audit.

## Prior-result regressions

The benchmark journals contain 11 DeflOpt and 9 deft4j files whose advantage
over the same reference fell by more than 10%. Defluff has none. Eight files
occur in both larger journals, leaving 12 unique files. These annotations
compare Columbo with an older Columbo result; they are not reference misses
unless the file also appears above. They are an investigation backlog, not a
reason to interrupt a complete benchmark refresh.

| File | Journal | Current disposition |
| --- | --- | --- |
| `PngSuite/g25n2c08.png` | both | Candidate remains one byte / seven bits above its historical floor. |
| `css-ig-net/Orange512.png` | both | Fixed: candidate is 732 bytes / 5,851 bits below its historical floor. |
| `css-ig-net/barchart.png` | DeflOpt | Candidate remains two bits above its historical floor. |
| `css-ig-net/briefcase.png` | both | Candidate remains one byte / five bits above its historical floor. |
| `css-ig-net/caution.png` | both | Candidate remains one byte / one bit above its historical floor. |
| `medium/loupe-fs8.png` | DeflOpt | Ten-second candidate is one bit above its floor; 30 seconds improves the floor by 4 bytes / 33 bits. |
| `oxipng/grayscale_2_should_be_grayscale_1.png` | deft4j | Candidate remains four bits above its historical floor. |
| `oxipng/grayscale_8_should_be_palette_8.png` | DeflOpt | Fixed: candidate is 39 bytes / 311 bits below its historical floor. |
| `pkmn-bw/000-Logo-2.png` | both | Candidate remains two bits above its historical floor. |
| `small/bomb.png` | both | Ten-second candidate is five bits above its floor; 30 seconds reaches exact parity. |
| `small/check.png` | both | Fixed by retaining the ordinary no-split parent before refinement. |
| `small/news.png` | both | Candidate remains three bits above its historical floor. |

Earlier guard generations failed 19 and then 13 of 40 files. The current
changed-parent result reduces the backlog to 10 while retaining the repaired
compact-split, ZIP, APNG, and large-image canaries.

## Accepted general fixes

The current source changes avoid filenames, corpus identities, and measured
elapsed-time gates:

- **Changed-parent no-split refinement:** the bounded M2 route keeps its
  complete ordinary Zopfli-enabled result. When emitting that result changes a
  block boundary or token spelling, Columbo reparses it once through the
  Default planner inside the existing file-wide hard grace. Header-only
  rewrites skip the replay, Arc-identical token arrays are recognized without
  scanning, and no alternate pre-Zopfli topology search is duplicated. This
  recovers `Orange512.png` by 1,072 bytes / 8,571 bits relative to the current
  journal result and `grayscale_8_should_be_palette_8.png` by 123 bytes / 978
  bits. Two-run wall-clock checks were unchanged on the grayscale and control
  cases; Orange completed in about 10.53 seconds instead of 11.89 seconds.
  The rule is based on exposing a genuinely new planner state rather than a
  corpus identity or elapsed-time threshold.
- **PNG metadata floor preservation:** bounded Max runs complete affordable
  non-Max metadata floors before image routes can consume the shared deadline.
  This restores `small/text.png` Max to Default parity. When time remains,
  cached floors continue through `Established` Max routes without rebuilding
  Default or charging their decoded data twice, so exhaustive opportunities
  remain available. Once those mandatory floors are complete, the dominant
  image search receives the full remaining allowance; optional metadata Max
  refinement uses only time genuinely left afterward. This recovered 189
  bytes and 1,515 meaningful bits on `medium/Matrix.png` without changing the
  supported metadata, APNG, or large-image guard floors.
- **Observational reporting:** Default, verbose, and visual modes share route
  gates, deadlines, candidate order, memory policy, and worker parallelism.
  Detailed modes cache stream-labelled reports and emit them as the ordered
  physical-stream prefix becomes final; they do not serialize ZIP/APNG workers
  or replace a route with a cheaper schedule.
- **Compact-split parent priority:** distinct complete parents are ordered by
  their actual byte count and meaningful bits. Every parent remains eligible
  when Max has sufficient time, because split gains are not assumed monotone.
- **Uniform ZIP member scheduling:** archives with at least eight similarly
  distributed Deflate members use balanced, independent worker slices and a
  single outer archive lineage. The tested 14-file `8x8-zip` group improved by
  103 bytes and 910 bits in aggregate relative to the recorded report, with no
  deft4j miss.
- **Non-duplicating ZIP lineages:** for bounded nonuniform archives, the caller
  owns complete Default while the parallel original-source Max branch starts
  from `Established` and does not rebuild an ordinary floor. Uniform member
  sets use their balanced member workers instead, and stored-only archives
  skip the second lineage.
- **Bounded ZIP Default-floor concurrency:** when that nonuniform archive race
  contains at least two independent Deflate members, its mandatory Default
  sibling uses up to eight bounded member lanes. This leaves more of the same
  file deadline for refinement; the direct Max sibling is unchanged. The
  workers share one wall-clock deadline and grace boundary, so this does not
  restore the older accidental behavior in which every serial child received
  a fresh global grace period. The exact candidate is 11 bytes / 89 bits
  smaller than timed deft4j on `skinsrc.zip`. Its eight-row DeflOpt ZIP sample
  has no miss or prior-result regression and is 2 bytes / 14 bits smaller in
  aggregate than the older broad-sample floor.

An experiment that replaced the PNG transformed lineage with only the direct
Default lineage was rejected: although it recovered several small historical
floors, it lost 8,149 bytes on `large/nerd.png` and 524 bytes on
`css-ig-net/Apple512.png`. This is the kind of broad net loss the guard and
family samples are intended to prevent.

Shortening the transformed lineage's ordinary-parent share from one fifth to
one tenth was also rejected. It beat the transient `sample_59.png` historical
floor by 4 bytes and 31 bits, confirming that a deliberately weaker parent can
open a distinct Max basin. The same general schedule repeatedly lost the full
189-byte / 1,515-bit `medium/Matrix.png` repair and put the APNG `Biker.png`
canary 8 bytes / 80 bits above its floor. Direct lineage traces explain the
trade-off: `sample_59.png` benefits from the earlier, weaker parent, whereas
`Matrix.png` needs the one-fifth parent before its best descendant becomes
reachable. No corpus-size threshold was introduced to choose between them.

Historical replay also isolates the remaining Zopfli-sensitive cluster.
`sample_17-fs8.png` reaches 3,949 bytes / 23,981 bits through `a9e5879`; the
first loss is `847e0a3`, which introduced the exact Zopfli RLE-friendly tree,
and later commits retain the 3,953-byte / 24,009-bit basin. Removing Zopfli
globally restores that and several 1–7-bit floors but loses newer, materially
stronger lineages. Two additive approximations were therefore tested: retain
the exact pre-Zopfli tree as another transformation seed, and retain both
already-built Defluff-derived feedback trees as transformation seeds. Neither
recovered `sample_17-fs8.png` or `small/bomb.png`; both added work. Longer Max
trials distinguish deadline pressure from a lost route lineage. At 30 seconds,
the current engine reaches `bomb.png`'s exact 9,537-bit floor and improves
`loupe-fs8.png` beyond its historical floor by 4 bytes / 33 bits. The other
eight differences do not recover: `sample_17-fs8.png` remains in the same
3,953-byte basin at 24,010 bits in that bounded run, and the seven smaller
cases retain their 1–7-bit differences. Reproducing the former historical
lineage requires a complete second no-split state lineage, which is rejected
here because it duplicates substantial Max work for at most four bytes in the
known guard set.

The remaining historical-floor differences are retained as priority cases,
but accepted for this build where the only identified common reversal caused
far larger losses elsewhere. No filename, corpus identity, or measured
ten-second timing gate was added to recover them.

## Forty-file priority guard

`work/regression-guard.json` is authoritative. It contains exactly 40 unique
names; insertion deduplicates each physical source by format and name, so a
recurring file does not consume more than one slot. The current set is:

- **css-ig-net:** `Orange512.png`, `barchart.png`, `briefcase.png`,
  `caution.png`, `sample_17-fs8.png`, `sample_21-fs8.png`,
  `sample_33-fs8.png`, `test-convertir-truecoloralpha-trns.png`,
  `compose.png`, `sample_14.png`, and `sample_59.png`.
- **oxipng:** `palette_8_should_be_palette_8.png`,
  `rgba_16_should_be_palette_2.png`,
  `grayscale_8_should_be_palette_8.png`,
  `grayscale_8_should_be_grayscale_1.png`,
  `grayscale_2_should_be_grayscale_1.png`, and
  `profile_gray_disallow_color.png`.
- **PNG/APNG size families:** `large/nerd.png`, `medium/loupe-fs8.png`,
  `medium/Gingerman.png`, `medium/menu.png`, `medium/te_syntax.png`,
  `medium/Matrix.png`, `medium/284.png`, `medium/Mittens.png`,
  `small/bomb.png`, `small/carwheel.png`, `small/check.png`, `small/news.png`,
  `small/present.png`, `PngSuite/g25n2c08.png`,
  `pkmn-bw/000-Logo-2.png`, `apng-medium/Lotus-buddha-APNG-animation.png`,
  and `apng-medium/Biker.png`.
- **ZIP:** `small-zip/Alleyway (EMU).zophar.zip`,
  `small-zip/wp-hide-dashboard.2.2.zip`,
  `small-zip/WindowsBatchFileMarkup.tmbundle.zip`,
  `8x8-zip/Grid.playdate-pulp.zip`,
  `8x8-zip/Symbols.playdate-pulp.zip`, and `medium-zip/samples.zip`.

The changed-parent candidate checked all 40 files in 60 serial trials,
including confirmation reruns. Thirty floors passed and the following ten
differences remained. `Orange512.png`, `small/check.png`, and
`grayscale_8_should_be_palette_8.png` now pass. A zero byte loss denotes a
meaningful-bit difference within the same file size.

| File | Byte loss | Bit loss |
| --- | ---: | ---: |
| `small/bomb.png` | 0 | 5 |
| `medium/loupe-fs8.png` | 0 | 1 |
| `css-ig-net/caution.png` | 1 | 1 |
| `PngSuite/g25n2c08.png` | 1 | 7 |
| `small/news.png` | 0 | 3 |
| `css-ig-net/briefcase.png` | 1 | 5 |
| `oxipng/grayscale_2_should_be_grayscale_1.png` | 0 | 4 |
| `pkmn-bw/000-Logo-2.png` | 0 | 2 |
| `css-ig-net/sample_17-fs8.png` | 4 | 28 |
| `css-ig-net/barchart.png` | 0 | 2 |

## Validation status

- All 380 Rust tests pass, including changed-parent admission and no-split
  decoded-byte preservation. `cargo fmt --check`, strict all-target Clippy,
  the 40-case guard structure check, and `git diff --check` also pass.
- All 164 private benchmark-tool tests pass. Dynamic discovery confirms 957
  eligible DeflOpt pairs and 1,621 timed-deft4j pairs.
- The complete 66-file Defluff rerun has no error or prior-result regression.
  Aggregate relaxed results are 32 bytes / 318 bits smaller than Defluff.
- The complete current-build DeflOpt refresh contains exactly 1,914 current-hash
  rows for all 957 eligible pairs in Default and Max: no duplicate, missing,
  orphan, error, stale-hash, or Max-below-Default row. Default totals 385,419
  bytes / 3,056,660 bits smaller than DeflOpt; Max totals 587,128 bytes /
  4,670,417 bits smaller. All 14 strict-policy miss rows and all 11 confirmed
  prior-result regressions are retained in the public report for later
  investigation; earlier relaxed audits provide their policy classification.
- The changed-parent broad samples cover 36 DeflOpt rows (18 source pairs) and
  25 timed-deft4j rows across PNG, ZIP, GZIP, and zlib families. Neither sample
  records a prior-result regression; every DeflOpt row meets parity. The sole
  deft4j miss is `8x8-png/waves.png`, which reaches parity under `--strict 0`.
- The exact current-binary ZIP replays cover eight DeflOpt rows and five timed
  deft4j rows without an error or unclassified reference miss. Against the
  pre-fix candidate, the timed-deft4j ZIP set saves another 9 bytes / 78 bits;
  its sole miss is the unchanged 3-byte / 21-bit strict-policy case. The
  32-row DeflOpt and 22-row deft4j broad samples, combining these ZIP results
  with the accepted PNG repair, are respectively 2 bytes / 14 bits and 7
  bytes / 57 bits smaller than the older candidate samples in aggregate.
- `medium-zip/skinsrc.zip`, which exposed duplicate floor work in an interim
  schedule, is now 11 bytes / 89 bits smaller than timed deft4j under one
  correctly shared grace boundary. `kensilverman-zip/kzipmix-20230322-mac.zip`
  Max improves its completed Default floor by 13 bytes / 100 bits.
- All six ZIP cases in the 40-file priority guard pass with the exact current
  distribution binary in six serial runs.
- The current priority guard performed 60 serial trials including
  confirmations: 30 of its 40 historical floors passed, and its 10 differences
  are enumerated above. The guard remains structurally valid at 40 unique
  files with no duplicate physical source.
