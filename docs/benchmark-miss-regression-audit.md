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
| DeflOpt | 1,914 | 15 | 9 | 15 strict-policy rows | 10 | 0 |
| Defluff (`--strict 0`) | 66 | 0 | 0 | — | 0 | 0 |
| timed deft4j | 1,621 | 22 | 22 | 20 strict-policy rows; 1 PNG preservation-policy row; 1 fixed engine row | 11 | 0 |
| Combined | 3,601 | 37 | 29 | 35 strict-policy rows; 1 PNG preservation-policy row; 1 fixed engine row; no unclassified row | 19 unique | 0 |

The accepted candidate fixes the recorded
`oxipng/profile_gray_disallow_color.png` engine miss, leaving 21 timed-deft4j
differences in 21 files. Every remaining difference has a confirmed strict or
safe-preservation policy explanation.

The complete DeflOpt report and deft4j miss replays use final-build binary
SHA-256 `c8eea2e26a2a6300a3212fbf843a60fc27d5ed4272d5e03430c5631eb2716a44`.
The complete like-for-like Defluff refresh uses the current distribution
binary SHA-256
`8ae993583b7ea95291e5eb185d7ea7d8f60b639a96ed3e94f538c75283003261`.
The accepted PNG and ZIP fixes were validated with current distribution binary
SHA-256 `29d432780240255ab15cfe61c2a3f98a87392c51cdbc50617dbba336b6c349f6`;
their focused and broad samples are recorded separately so the complete
journals do not mix executable hashes.

## DeflOpt misses

Every DeflOpt miss reaches parity or better when rerun with `--strict 0`.
Default mode must remain strictly compliant by default, so these are documented
policy differences rather than search failures. Deltas below are the largest
strict delta recorded for the file; positive means Columbo is larger.

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

The complete final-build refresh covers all 15 rows above. Max was never worse
than its completed Default result, and every strict miss reached parity or
better in a same-binary relaxed audit.

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

Across all 66 files, relaxed Columbo is 27 bytes and 273 meaningful Deflate
bits smaller than Defluff in aggregate.

## Timed deft4j misses

The recorded journal contains 22 misses. The accepted metadata-floor schedule
fixes `oxipng/profile_gray_disallow_color.png`: the current result is 36,584
bytes / 291,812 meaningful bits, one byte / two bits smaller than deft4j.
The following table lists the 21 remaining differences. Twenty reach parity
or better with `--strict 0`. The remaining file,
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
| `medium/test-conversion-truecoloralpha-grayscalealpha.png` | 10 | 0 | 2 | strict policy |
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
| `small/text.png` | 10 | 1 | 10 | strict policy |
| `samplelib-zip/sample-simple.zip` | 10 | 2 | 12 | strict policy |
| `small-zip/schematic-symbol-and-pcb-footprint.zip` | 10 | 3 | 21 | strict policy |
| `small-zip/testmake.zip` | 10 | 1 | 6 | strict policy |

The final-build replay covers every row in this table under its recorded
timeout. All 20 strict-policy rows again reached parity or better in the
same-binary relaxed audit. `small/text.png` improved from the historical
1-byte / 10-bit miss to a same-byte 3-bit strict difference; the table retains
the report snapshot so its provenance remains explicit.

## Prior-result regressions

The benchmark journals contain 10 DeflOpt and 11 deft4j rows whose advantage
over the same reference fell by more than 10%. Defluff has none. Two files
occur in both larger journals, leaving 19 unique files. These annotations
compare Columbo with an older Columbo result; they are not reference misses
unless the file also appears above. They are an investigation backlog, not a
reason to interrupt a complete benchmark refresh.

| File | Journal | Current disposition |
| --- | --- | --- |
| `css-ig-net/barchart.png` | DeflOpt | Current run saves 14 bits, 2 fewer than its previous result. |
| `css-ig-net/compose.png` | DeflOpt | Current run remains 16 bytes / 131 bits ahead of DeflOpt, but lost 2 bytes / 15 bits of prior advantage. |
| `css-ig-net/sample_21-fs8.png` | deft4j | Current guard passes after the compact-split parent-order change. |
| `css-ig-net/sample_33-fs8.png` | deft4j | Current guard passes after the same general ordering rule. |
| `css-ig-net/test-convertir-truecoloralpha-trns.png` | deft4j | Current guard ties its historical file size and is 4 bits above its floor. |
| `medium/loupe-fs8.png` | DeflOpt | Current run saves 1 byte / 14 bits, losing 1 byte / 4 bits of prior advantage. |
| `medium/menu.png` | DeflOpt | Current guard passes. |
| `medium/te_syntax.png` | DeflOpt | Current guard passes after the metadata-floor scheduling change. |
| `medium/284.png` | deft4j | Current guard is 1 byte / 3 bits above its historical floor. |
| `medium/Mittens.png` | deft4j | Current guard is 1 bit above its historical floor. |
| `oxipng/grayscale_8_should_be_palette_8.png` | DeflOpt | Current run remains 420 bytes / 3,364 bits ahead of DeflOpt, but lost 84 bytes / 667 bits of prior advantage. |
| `oxipng/palette_8_should_be_palette_8.png` | deft4j | Current guard is 15 bytes / 123 bits above its historical floor; the rejected competing-lineage experiment caused much larger broad losses. |
| `oxipng/rgba_16_should_be_palette_2.png` | deft4j | Current guard is 6 bytes / 51 bits above its historical floor; it shares the same competing-lineage trade-off. |
| `pkmn-bw/000-Logo-2.png` | both | Current DeflOpt run saves 4 bits, 2 fewer than its previous result; the guard records the same 2-bit floor difference. |
| `small/carwheel.png` | DeflOpt | Current guard passes. |
| `small/check.png` | DeflOpt | Current guard passes. |
| `small/present.png` | both | Current DeflOpt run saves 15 bits, 2 fewer than its previous result; the guard records the same 2-bit floor difference. |
| `8x8-zip/Grid.playdate-pulp.zip` | deft4j | Current guard passes. |
| `8x8-zip/Symbols.playdate-pulp.zip` | deft4j | Current guard passes. |

The previous full guard run predated the accepted fixes and failed 19 of its 40
files. The final-build rerun reduced that to 13 of 40. All three repaired
compact-split files, both uniform ZIP files, all three priority ZIP canaries,
`medium/loupe-fs8.png`, and `large/nerd.png` passed.

## Accepted general fixes

The current source changes avoid filenames, corpus identities, and measured
elapsed-time gates:

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
  Detailed modes wrap the same work with stream-labelled events; they do not
  serialize ZIP/APNG workers or replace a route with a cheaper schedule.
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

The remaining historical-floor differences are retained as priority cases,
but accepted for this build where the only identified common reversal caused
far larger losses elsewhere. No filename, corpus identity, or measured
ten-second timing gate was added to recover them.

## Forty-file priority guard

`work/regression-guard.json` is authoritative. It contains exactly 40 unique
names; insertion deduplicates each physical source by format and name, so a
recurring file does not consume more than one slot. The current set is:

- **css-ig-net:** `die.png`, `sample_56-fs8.png`, `scooter.png`,
  `sample_28.png`, `Banana512.png`, `biker.png`, `Orange512.png`,
  `barchart.png`, `sample_17-fs8.png`, `sample_21-fs8.png`,
  `sample_33-fs8.png`, `test-convertir-truecoloralpha-trns.png`,
  `compose.png`, `sample_14.png`, and `sample_59.png`.
- **oxipng:** `interlaced_rgba_16_should_be_palette_1.png`,
  `palette_8_should_be_palette_8.png`, `rgba_16_should_be_palette_2.png`,
  `grayscale_8_should_be_palette_8.png`, and
  `profile_gray_disallow_color.png`.
- **PNG/APNG size families:** `large/nerd.png`, `medium/loupe-fs8.png`,
  `medium/Gingerman.png`, `medium/menu.png`, `medium/te_syntax.png`,
  `medium/Matrix.png`, `medium/284.png`, `medium/Mittens.png`,
  `small/carwheel.png`, `small/check.png`, `small/present.png`,
  `pkmn-bw/000-Logo-2.png`, `apng-medium/Lotus-buddha-APNG-animation.png`,
  and `apng-medium/Biker.png`.
- **ZIP:** `small-zip/Alleyway (EMU).zophar.zip`,
  `small-zip/wp-hide-dashboard.2.2.zip`,
  `small-zip/WindowsBatchFileMarkup.tmbundle.zip`,
  `8x8-zip/Grid.playdate-pulp.zip`,
  `8x8-zip/Symbols.playdate-pulp.zip`, and `medium-zip/samples.zip`.

The latest exact-candidate replay checked all 40 files in 64 serial trials,
including confirmation reruns. Twenty-eight floors passed and the following
12 differences remained. `medium/Matrix.png`, `medium-zip/samples.zip`,
`medium/te_syntax.png`, and both APNG canaries passed. A zero byte loss denotes
a meaningful-bit difference within the same file size.

| File | Byte loss | Bit loss |
| --- | ---: | ---: |
| `css-ig-net/sample_59.png` | 27 | 218 |
| `css-ig-net/sample_14.png` | 4 | 28 |
| `oxipng/grayscale_8_should_be_palette_8.png` | 84 | 667 |
| `css-ig-net/compose.png` | 2 | 15 |
| `small/present.png` | 0 | 2 |
| `pkmn-bw/000-Logo-2.png` | 0 | 2 |
| `oxipng/rgba_16_should_be_palette_2.png` | 6 | 51 |
| `oxipng/palette_8_should_be_palette_8.png` | 15 | 123 |
| `medium/Mittens.png` | 0 | 1 |
| `medium/284.png` | 1 | 3 |
| `css-ig-net/test-convertir-truecoloralpha-trns.png` | 0 | 4 |
| `css-ig-net/barchart.png` | 0 | 2 |

## Validation status

- The current candidate passes all 352 Rust tests, `cargo fmt --check`, strict
  all-target Clippy, the 40-case guard structure check, and `git diff --check`.
- All 164 private benchmark-tool tests pass. Dynamic discovery confirms 957
  eligible DeflOpt pairs and 1,621 timed-deft4j pairs, including the 12 reviewed
  `steam-shop_apng` additions.
- The complete 66-file Defluff rerun has no error or prior-result regression.
  The two former strict-mode differences are independently confirmed policy
  cases; aggregate relaxed results are 27 bytes / 273 bits smaller than
  Defluff.
- The complete final-build DeflOpt refresh contains exactly 1,914 current-hash
  rows for all 957 eligible pairs in Default and Max: no duplicate, missing,
  orphan, error, stale-hash, or Max-below-Default row. Default totals 385,418
  bytes / 3,056,651 bits smaller than DeflOpt; Max totals 586,956 bytes /
  4,669,051 bits smaller. All 15 miss rows reach parity or better in their
  recorded relaxed audit, and all 10 confirmed prior-result regressions are
  retained in the public report for later investigation.
- The final 22-row timed-deft4j family sample has no error and totals 1,735
  bytes / 13,912 bits smaller. Its only two misses are strict-policy
  differences: relaxed same-build audits reach parity or better on
  `8x8-png/waves.png` and
  `small-zip/schematic-symbol-and-pcb-footprint.zip`.
- A separate final-build replay covers all 21 timed-deft4j miss files. Twenty
  are confirmed strict-policy differences; the remaining `c2pa-signed.png`
  result is the validated unsafe ancillary-chunk preservation policy.
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
- The current priority guard performed 64 serial trials including
  confirmations: 28 of its 40 historical floors passed, and its 12 differences
  are enumerated above. The guard remains structurally valid at 40 unique
  files with no duplicate physical source.
