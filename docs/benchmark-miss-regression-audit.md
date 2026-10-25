<!-- SPDX-License-Identifier: MIT -->

# Benchmark miss and regression audit

This dated log, newest first, separates three kinds of result: misses against
a reference program, regressions against an earlier Columbo result (a
historical floor), and deadline-limited results. A positive reference delta
means Columbo is larger. A guard failure means only that the build did not
reproduce an older Columbo result; its output can still be much smaller than
DeflOpt's, Defluff's or deft4j's.

The three 24 October 2026 entries are the current state. Entries from 30 September keep
their evidence; older ones are summarized under
[Earlier refreshes](#earlier-refreshes), and their full text is in this file's
Git history.

Conventions:

- *Quiet runs* run one executable at a time with no competing load, which
  separates code effects from benchmark-load variance.
- Journal-to-journal deltas span all intervening commits. Runtimes from
  different complete runs are observations, not isolated speedups.
- Overlapping cohorts, benchmarks and modes are never added together.
- Original corpus rows are not replaced by best-of-repeat or longer-time
  results, and guard floors are not lowered to match a result. Public reports
  never relabel rows from an older executable as current results.
- An endpoint reached by any run is reachable. A failed finite run proves
  neither global optimality nor that an endpoint is unreachable.
- *Strict misses* are references with a singleton or empty distance alphabet,
  which strict output must complete, plus the signed PNG
  `oxipng/c2pa-signed.png`. Rewriting its image data would invalidate its
  `caBX` signature chunk, so Columbo returns the file unchanged unless
  `--strip` removes the chunk.
- Defluff comparisons are relaxed on both sides: Defluff uses forms that
  Columbo allows only with `--strict 0`. Ordinary Default use is strict.
- The *hundred-file guard*, `work/regression-guard.json`, keeps historical
  floors for the latest 100 unique `(format, source)` identities; a recurring
  file is deduplicated on insertion.
- Private machine-readable states live under `work/`, which Git ignores.

<a id="deft4j-refresh-24-october"></a>

## deft4j and Defluff refresh on 24 October 2026

### Benchmark state

The timed deft4j and Defluff reports come from executable `23035349…`:
`e79a60a` with the tree-builder, RLE, bit-reader and plan-key speedups of the
[efficiency review](research/efficiency-review.md#e79a60a-24-october-2026),
whose outputs match `e79a60a` wherever a search completes. The APNG reports
were not refreshed and stay at `ab9a0cd7…` (`b831d49`). Against the timed
deft4j state of 16 October (1,621 rows from `f6ce13ca…`, `9d5f818`), with
unchanged allowances:

| Benchmark | Result |
| --- | --- |
| Timed deft4j Max, 1,621 rows | 328 smaller, 1,269 identical, 24 larger (15 in bytes); net −5,083 bytes / −40,644 bits; runtime 16,313.6 → 13,872.6 s |
| By format | PNG −4,484 bytes (312 smaller, 22 larger); GZIP −570 (all 7 smaller); ZIP −29 (9 smaller, 2 larger); zlib unchanged |
| Defluff, 66 pairs | Unchanged: −109 bytes and −938 bits against Defluff, no misses |

The largest gains are `Partnership_Card___John_Lewis_Finance.png` (−441
bytes) and `kzipmix-20200115-linux-static.tar.gz` (−423). Other applications
loaded the machine in the last half hour of the deft4j run, so every row that
grew was rerun quietly before attribution.

### Reference misses

Unchanged: the same 12 timed deft4j misses, by the same 0–1 bytes and 2–3
bits. The relaxed audit puts 11 at parity with `--strict 0`; the twelfth is
the preserved `oxipng/c2pa-signed.png`. The report's five prior-result
annotations (crossroads, Squirtle, Poliwrath, Pikachu and Psyduck) are rows
identical to the 16 October state, compared with older floors of the
hundred-file guard whose causes are recorded
[below](#source-max-replaced-parents-24-october).

### Rows that grew

All 24 rows repeat their sizes in two quiet serial reruns at their
allowances. Run once at their allowances, builds from `9d5f818` on show where
each first grew:

| First larger at | Rows | Changes in that range |
| --- | ---: | --- |
| `6c81dfa` | 9 | R18, the late terminal tree floor, adds Max work |
| `0d87ee8` | 2 | Block-plan store and code-size changes |
| `f12cf8b` | 12 | Code-length tree and Max header-planning speedups |
| This build | 1 | 1 bit on `rgb_trns_8_should_be_palette_8.png` |

Given three times their allowances, six reach or beat their earlier sizes:
`Apricot512.png` (298,494 bytes, against 298,555), `4.2.07.png` (725,411),
`5.1.09.png` (53,832), `file04.png` (284,020), `frymire.png` (343,180) and
`sample_40-fs8.png` (5,066). Fifteen still use their whole allowance at 30 to
66 seconds, so where their searches end is unknown.

The other three, `basi3p04.png`, `g8t.png` and `108-Lickitung-1.png`, stop at
about 8 of their 10 seconds, but that is Max's closing margin, not a finished
search. Given 60 seconds, `0d87ee8` completes in 20 to 22 seconds and ends at
1,319, 1,180 and 1,715 bits, the sizes `f12cf8b` and this build reach within
10 seconds; its 1,318-, 1,177- and 1,714-bit outputs were deadline cuts. The
speedups moved these rows by completing more of the same search, not by
changing it. Like Hitmonchan [below](#source-max-replaced-parents-24-october),
each search ends 1 to 3 bits above what it returns when cut at 10 seconds,
which Max's first principle rules out; they are left for a separate
investigation.

Artifacts: `work/deft4j-refresh-20261024/`.

<a id="source-max-replaced-parents-24-october"></a>

## Source-max replaced parents on 24 October 2026

### Starting state

At `f12cf8b` the hundred-file guard passes 91 of 100 floors. Seven failures
keep their recorded causes: `crossroads.png` (+5 bytes),
`074-Geodude-1.png` (+6 bits), Psyduck, Squirtle and Pikachu (1 bit each),
Poliwrath (2 bits) and `test-conversion-truecoloralpha-grayscalealpha.png`
(+2 bytes); `041-Zubat-0.png` is now 1 bit above its floor instead of 3. The
ninth, `pkmn-col/107-Hitmonchan-2.png` (+1 byte), joined the guard in the
refresh below. The reference misses are unchanged: DeflOpt's four
strict-policy floors and none against Defluff.

### A complete search ending above a cut one

Hitmonchan's Max search completes, in about 6 seconds with `f12cf8b`, and ends
at 447 bytes (1,584 bits) at every allowance. Its 446-byte floor (1,574 bits)
came from `0d87ee8` cut at 10 seconds; given 20 or 40 seconds, `0d87ee8` also
completes, in about 16 seconds, and ends at 447. A longer search ending above
a shorter one breaks Max's first principle, that more time never costs
savings.

The deadline cut original-source Max (M12) at its first complete parent,
126 tokens and 1,588 bits. Closed as a losing topology, that parent reaches
1,574 bits through the compact payload-tree floor and its balanced closure.
Given time, the route's first replay replaces it with a 127-token parent of
1,587 bits, from which the same floors find nothing (1,584 bits). Capping the
route at no replays restores 1,574 bits. `074-Geodude-1.png` is the case
recorded on [17 October](#guard-17-october): the replay takes the route from
1,854 to 1,852 bits, and the replaced parent finishes at 1,842 bits instead of
1,848. `041-Zubat-0.png` is a third: without replays the current build ends at
1,730 bits instead of 1,734.

### Accepted change

M12 keeps each complete parent its replays replace (moving the bytes, not
copying them) and, after its final candidate, closes each one the same way:
the compact balanced-tree cleanup (M4), then the terminal tree floors, kept
only if strictly smaller. Each close starts only while routes may start (G0),
so a run already at its deadline does no extra work. Any of those parents is
what a deadline would have handed over, so a longer search can no longer end
above a shorter one at a replay boundary. Ties keep the route's final
candidate, so outputs change only where a replaced parent finishes strictly
smaller, and such a parent is a new encoding, so it also clears the
source-max fixed-point claim that would let M13 skip it. The gate is the
mechanism, not a size: unbounded replays let a deadline choose the parent. The
floor-seeded Max refinement, M11 and M13 also replay to a fixed point, but no
guard floor or sample pointed at them, so they are unchanged. See
[routes and methods](routes-and-methods.md).

### Evidence

| Cohort | Result |
| --- | --- |
| Every third DeflOpt PNG whose Max run completes (176 files at their allowances, builds run side by side) | Byte-identical; CPU 1,805.5 → 1,805.0 s |
| Every second hard Pokémon sprite that completes in the timed deft4j corpus (177) | Byte-identical; CPU 956.8 → 956.6 s |
| Every sixteenth DeflOpt PNG, Max at 10 seconds, alternating serial builds (58, most deadline-bound) | Byte-identical; CPU 836.3 → 836.0 s |
| Hundred-file guard, serial at recorded allowances | 94 of 100 floors pass (91 before): Hitmonchan, Geodude and Zubat now pass; the six failures keep their recorded causes |

Hitmonchan (446 bytes, 1,574 bits), Geodude (1,842 bits) and Zubat
(1,730 bits) now meet or beat their floors at 10 and 40 seconds. Hitmonchan
takes 1.3 seconds longer because the rewritten-seed refinement (M13) starts
from the better parent; the closes themselves take about 25 ms. A probe that
capped M12 at no replays was byte-identical on the first two cohorts, so a
replaced parent rarely finishes better; where one does, it is now kept. API
comparisons on the 52 inputs, in Default with the 180-second budget and in
zero-budget Max, matched in all 104 runs, and two completing Max runs
(`036-Clefable-0.png`, `114-Tangela-1.png`) matched byte for byte with
unchanged instructions retired. A new test checks that the route keeps each
replaced parent, valid and in order. `__text` grew from 841,128 to
844,288 bytes, after keeping the bounded phase out of line (inlined, it
pushed the scheduler out of its caller for 7,868 bytes); the shipped
executable keeps its 925,616 bytes.

### Not changed

- `crossroads.png`: no replay-boundary parent reaches its 2,123-byte floor.
  Capped at zero, one or two replays and given 120 seconds, every run ends at
  2,128 bytes. The floor came from a deadline that cut the route mid-search, a
  state no complete search passes through, so it stays recorded.
- The other 18 DeflOpt Max rows that grew in the refresh below use their whole
  allowance (about 11.5 seconds for 10) and are the deadline cuts recorded
  there.
- Psyduck, Squirtle, Pikachu, Poliwrath and
  `test-conversion-truecoloralpha-grayscalealpha.png` keep their recorded
  causes; refining both early-lineage parents was rejected on
  [9 October](#guard-9-october).

Artifacts: `work/source-max-replaced-parents/`.

## DeflOpt and Defluff refresh on 24 October 2026

### Benchmark state

The DeflOpt and Defluff reports come from executable `18ba70c5…` (`f12cf8b`),
which adds the code-length tree and local-rewrite reuse (`c6f01a6`), the Max
header-planning speedups (`d72c937`, `f12cf8b`) and reporting changes. The
deft4j reports were not refreshed. Against the DeflOpt state of 23 October
(1,914 rows from `54df6aab…`):

| Benchmark | Result |
| --- | --- |
| DeflOpt Default, 957 rows | 3 smaller, none larger; net −13 bytes / −103 bits; runtime 738.1 → 519.3 s |
| DeflOpt Max, 939 rows with unchanged allowances | 178 smaller, 742 identical, 19 larger; net −1,526 bytes / −12,251 bits; runtime 8,650.8 → 7,927.4 s |
| Defluff, 66 pairs | Unchanged: −109 bytes and −938 bits against Defluff, no misses |

Every Max row is no larger than its Default row. The three smaller Default
rows (`pokemon pixel art.png` −10 bytes, `IMG_8555.png` −2,
`profile_gray_disallow_color.png` −1) differ only in their iCCP chunk: PNG
metadata probes stop after 100 milliseconds, and the faster header planner
gets further within them. Their image data is identical, and the earlier
build itself varies by a byte on `pokemon pixel art.png` between runs.

### Reference misses

Unchanged: DeflOpt keeps the four strict-policy floors of 23 October
(`sample-green-400x300.png` and `T_Grass.png`, both modes), each at parity in
the relaxed audit.

### Rows that grew

Quiet serial reruns of the 19 DeflOpt Max rows that grew in bytes or bits, at
their 10-second allowances, alternating builds of `0d87ee8` and `f12cf8b`
twice each: ten repeat, by 1 to 8 bytes (about 35 bytes in all); four vary
between runs; five tie in bytes. Given 20 seconds, the earlier build reaches
the current build's 10-second sizes on the three largest
(`grayscale_alpha_8_should_be_grayscale_trns_8.png` 67,053, `ask_02.png`
36,117, `4.2.07.png` 725,430 bytes), and the current build ties or beats it
(67,053, 36,116 and 725,420). These are deadline cuts: the output-preserving
speedups move where a 10-second search stops, as with the block-plan store on
[23 October](#rows-that-grew-23-october).

## DeflOpt and Defluff refresh on 23 October 2026

### Benchmark state

The DeflOpt and Defluff reports come from executable `54df6aab…`
(`0d87ee8`), which adds the block-plan store (`ef91c11`), the APNG R5 move
(`04d63bd`) and the code-size and build changes. The deft4j reports were not
refreshed: timed deft4j is still from `f6ce13ca…` (`9d5f818`) and the APNG
reports from `ab9a0cd7…` (`b831d49`). Against the previous DeflOpt state
(1,693 rows from `f6ce13ca…`, 221 from `ab9a0cd7…`):

| Benchmark | Result |
| --- | --- |
| DeflOpt Default, 957 rows | 172 smaller, none larger; net −2,804 bytes / −22,447 bits; runtime 880.3 → 738.1 s |
| DeflOpt Max, 932 rows with unchanged allowances | 96 smaller, 779 identical, 23 larger (33 in bytes or bits); net −446 bytes / −3,538 bits; runtime 8,694.2 → 8,570.3 s |
| Defluff, 66 pairs | Unchanged: −109 bytes and −938 bits against Defluff, no misses |

Every Max row is no larger than its Default row. Other applications shared
the machine during the DeflOpt run, so its deadline-bound rows were rerun
serially before any loss was attributed.

### Reference misses

DeflOpt keeps the same four strict-policy floors (`sample-green-400x300.png`
and `T_Grass.png`, both modes); this run's relaxed audit shows all four at
parity. Timed deft4j's 12 misses, rerun with the current executable and the
relaxed audit: 11 reach parity with `--strict 0`, ten of them differing only
by 2 or 3 bits at equal bytes, and the twelfth is the preserved signed PNG
`oxipng/c2pa-signed.png`.

<a id="rows-that-grew-23-october"></a>

### Rows that grew

Quiet serial reruns of the 36 DeflOpt Max rows that grew in bytes or bits, at
their earlier allowances, with builds of `9d5f818` and `0d87ee8`, net +31
bytes; 26 tie or favor the current build (`large/nerd.png` −21). Builds of
each commit attribute the ten that repeat:

| File | At 10 s | Cause | Given time |
| --- | ---: | --- | --- |
| `oxipng/grayscale_alpha_8_should_be_grayscale_8.png` | +29 | Block-plan store (`ef91c11`) | 92,679 against 92,689 before the store at 20 s; 92,663 against 92,670 at 60 s |
| `css-ig-net/test-conversion-truecoloralpha-grayscalealpha.png` | +5 | `ef91c11` | 51,861 for both builds at 20 and 60 s |
| `css-ig-net/sample_14.png` | +5 | `ef91c11` | 7,066 for both at 30 s |
| `css-ig-net/sample_68-fs8.png` | +10 | R18 (`6c81dfa`) | +10 at 40 and 120 s |
| `css-ig-net/Bag.png` | +4 | R18 | 70,481 for both at 30 s |
| `apple-fs8.png`, `Craig_Smith.png`, `butterfly.png` | +1 each | R18 | +1 each at 30 s |
| `medium/micropython const.png` | +4 | Deadline-bound: +13 at R18, 0 at `04d63bd` (APNG only), +4 from `ef91c11` | +1 at 30 s |
| `kensilverman-zip/pngout-20230322-mac.zip` | +1 | R18, through ZIP Max's refined Default | +1 at 30 and 60 s |

The block-plan store changes no Default output and favors Max given time; it
makes the search faster, so a 10-second deadline cuts it elsewhere. On
`grayscale_alpha_8` both builds' main-lineage route reports are identical
apart from timings; the difference is the PNG early lineage, whose quick
floor is Default cut at a fifth of the allowance (2 seconds, against this
file's 3.4-second Default) and so ends at a different parent.

The R18 losses come through the same lineage. On `sample_68-fs8.png` the
main lineage selects the same 8,891-byte stream with or without R18 (9,909
bytes as PNG); both results come from the early lineage, whose quick floor
now ends with R18 (Default 9,948 → 9,925 bytes) and whose refinement then
ends 10 bytes larger. ZIP Max's refined Default shows the same effect on
`pngout-20230322-mac.zip`: R18 shrinks the `pngout` member's Default stream
from 118,741 to 118,706 bytes, and Max's refinement of it ends at 118,698
bytes instead of 118,697; the direct Max branch (120,782) is not
competitive in either build.

Rejected: a seed policy for that quick floor which skips R18, since the
refinement ends with R18 anyway and R18 there changes only the parent. It
recovered six of these losses (`sample_68`, `Bag`, `micropython const`,
`apple-fs8`, `Craig_Smith`, `butterfly`; −18 bytes), but on an unbiased
sample, every eighth DeflOpt PNG by name (116 files, Max at 10 seconds,
alternating builds), it was 0 smaller and 2 larger, net +10 bytes, with
unchanged CPU; `css-ig-net/sample_65-fs8.png` is 9 bytes larger at 10 and 30
seconds. As on [8 October](#early-lineage-parent-8-october), the parent does
not predict the endpoint, so the losses are recorded, not fitted.

<a id="guard-23-october"></a>

### Hundred-file guard

`0d87ee8` passes 91 of 100 floors. Eight failures are those of 17 October,
with the same causes; `041-Zubat-0.png` is now 1 bit above its floor instead
of 3. The ninth, `test-conversion-truecoloralpha-grayscalealpha.png` (floor
51,859 bytes, now 51,861), is the deadline cut above: both builds end at
51,861 given 20 or 60 seconds.

## APNG and Defluff refresh on 17 October 2026

### Benchmark state

The reports come from executable `ab9a0cd7…` (`b831d49`), which adds R18 and
the first form of the APNG header fit. The Defluff report is unchanged in
bytes and bits: −109 bytes and −938 bits against Defluff, no misses. Against
the APNG reports of `e3325451…` (22 September):

| Benchmark | Result |
| --- | --- |
| APNG Max, 2,431 pairs | 2,261 smaller, 11 larger; net −577,288 bytes / −4,620,121 bits; no deft4j misses; lead over deft4j 3,072,150 → 3,649,438 bytes; runtime 50,235 → 49,977 s |
| APNG Default, 2,431 files | 2,298 smaller, 99 larger by 996 bytes (109 larger in bits); net −2,047,932 bytes / −16,385,692 bits; runtime 13,686 → 3,899 s; Default-not-faster flags 4 → 0; Max no larger than Default on every file |

Other heavy work shared the machine during the Max run (load average median
16, peak 60), so its deadline-bound rows were rerun serially before any loss
was attributed.

<a id="rows-that-grew-17-october"></a>

### Rows that grew

APNG Default: builds of `357561d`, `c3c6f60`, `9d5f818`, `6c81dfa` and
`b831d49` on the 110 rows that grew in bytes or bits place 103 of them, 1,315
bytes, at `b831d49`. Its late R2–R4 left R5 in the sweep, so R5 compared token
rewrites against an unfitted header and kept rewrites that blocked the late
fit. The accepted change moves R5 with R2–R4
([R5 follows the fit](research/apng-header-fit-validation.md#r5-follows-the-fit)):
on all 2,431 stickers it saves 261,016 bytes against `6c81dfa` with 58 files
larger, and 67,715 bytes against `b831d49`. One more row,
`4162040_4062577…`, carries 152 bytes after `IEND`; Default has preserved
such payloads since `44b26f0`, unless `--strip` is given, and the older report
predates that. Its Deflate data is smaller than before.

APNG Max: serial reruns of the 11 grown rows at their recorded allowances,
with the accepted change:

- Six reach or beat the earlier result: the three `1293230_*` rows besides
  `103418`, `1145350_430383…` (−11), `321360_1896007…` (−29) and
  `3331000_416373…` (−6). Their benchmark losses came from the shared load.
- `1293230_103418…` (+11) and `2666510_360454…` (+21) beat it at twice the
  allowance (−54, −7): deadline-limited.
- `2286780_434610…` reaches its 6,362 bytes on a rerun.
- `230410_370927…` stays at 3,409 bytes against 3,408 with every build from
  `357561d` on, at 10 and 40 seconds; the earlier result does not reproduce.
- `4162040_4062577…` is 116 bytes above, all from its preserved 152-byte
  payload.

<a id="guard-17-october"></a>

### Hundred-file guard

`b831d49` passes 92 of 100 floors, against 88 for `9d5f818`; `Gingerman.png`,
`Death.png`, both `test-conversion-truecoloralpha-*` files and `sample_25.png`
now pass. The guard has no APNG entries, so the accepted change cannot alter
it. Of the eight failures, `crossroads.png` (+5 bytes), Psyduck, Squirtle and
Pikachu (1 bit each) and Poliwrath (2 bits) keep the causes recorded on
16 October. The others:

- `paperplane-fs8.png`, +14 bytes at 10 seconds, beats its floor at 20
  seconds (9,053 bytes, 64,610 bits against 64,611).
- `074-Geodude-1.png`, +6 bits. Its floor came from `c3c6f60` at 10 seconds,
  and `c3c6f60` itself ends 6 bits above it at 20 and 40 seconds. Builds up to
  `dc68b5f` pass; from `2b40fb0`, whose header-plan reuse speeds the
  source-max route, that route has time at 10 seconds for a replay (1,854 →
  1,852 bits), and from the better parent the tree floors and R4 end at 1,848
  bits instead of 1,842. Every later build ends at 1,848 at every allowance
  from 2 to 40 seconds. It is the parent path dependence recorded for
  `crossroads.png`.
- `041-Zubat-0.png`, +3 bits. `c3c6f60`, which set the floor, now misses it by
  3 bits as well; the current build reaches 1,734 bits, 1 above, at 40
  seconds.

## R15–R17 and D4 on 16 October 2026

### Benchmark state

All four reports come from executable `f6ce13ca…` (`9d5f818`), which adds R15
entropy cuts and their header-aware fallback, R16 terminal merges, R17
terminal token replay, and D4 with its length-family chains, narrow search
and re-spell from literals. Against the `3ceab7ae…` reports at the same
allowances:

| Benchmark | Result |
| --- | --- |
| Defluff, 66 pairs | Bytes unchanged: −109 against Defluff, no misses; `047-Parasect.png` 6 bits smaller |
| DeflOpt Default, 957 rows | 337 smaller, 620 identical, none larger; net −191,908 bytes / −1,535,205 bits; runtime 1,286.1 → 876.2 s |
| DeflOpt Max, 904 rows with unchanged allowances | 282 smaller, 609 identical, 13 larger; net −21,485 bytes / −171,813 bits; runtime 8,717.6 → 8,374.4 s |
| Timed deft4j, 1,621 pairs | 390 smaller, 1,213 identical, 18 larger; net −111,035 bytes / −888,259 bits; runtime 17,427.9 → 16,313.6 s |

The largest gains are the Partnership card (−60,156 bytes Default, −57,413
timed deft4j), `floor pattern.png`, `kzipmix-20200115-linux-static.tar.gz`
(−16,888) and `download_webp`. Every Max row is no larger than its Default
row. The runtimes come from separate complete runs and are observations, not
isolated speedups.

### Reference misses

Unchanged: `samplelib-png/sample-green-400x300.png` and `small/T_Grass.png`
in both DeflOpt modes, and `sample-green` and
`pkmn-col-hard/066-Machop-2.png` in timed deft4j, one byte each. All are
strict-policy floors: each reference sends a single one-bit distance code or
an empty distance alphabet, which strict output must complete. With
`--strict 0` the current executable matches or beats every reference in
Default and Max (`sample-green` 360 = 360, `T_Grass` 443 < 444, `Machop-2`
449 = 449 bytes).

### Rows that grew

34 Max rows grew in bytes or bits across DeflOpt Max and timed deft4j. Quiet
serial reruns of both executables at each recorded allowance, two repeats
each, net +27 bytes over both repeats; half the losses do not repeat or
reverse (`sample_39-fs8.png` −11, `284.png` −15, `filter_0_for_rgba_16.png`
−25 at its earlier 19-second allowance). Builds of every intermediate commit
attribute the repeating ones at 10 seconds to the code-length tree speedup
(`dc68b5f`: `sample_60.png` +10, `crossroads.png` +5), the header-plan store
(`2b40fb0`: `paperplane-fs8.png` +16, three oxipng files 1–3) and R15
(`kpic_src.zip` +1). Neither speedup changes an output given time: both are
deadline paths. At 60 seconds every repeating loss ties or reverses
(`sample_60.png` −6, `interlaced_grayscale_16_should_be_grayscale_8.png` −8)
except two:

- `css-ig-net/Apple512.png`, +36 bytes, stable at 60 and 240 seconds, from
  `9d5f818`. Both lineages of single-image PNG Max select the same main
  stream; the loss is the [early-lineage parent effect](#early-lineage-parent-8-october)
  again. The quick Default parent now includes D4's re-spelled endpoint
  (Default 224,375 → 221,438 bytes), and its `Established` refinement ends at
  218,338 bytes of image data instead of 218,302.
- `kensilverman-zip/kpic_src.zip`, +1 byte, stable, from R15 through ZIP
  Max's refined Default.

Rejected alternative: leaving D4 out of the early lineage's quick parent,
which the main lineage's mandatory floor already covers. It restored
Apple512 (221,245 bytes) and was neutral on 116 D4-eligible validation PNGs
in Max at 10 seconds (8 smaller, 10 larger, net 0), but on the 130 benchmark
PNGs whose Default output shrank and that decode to at least 128 KiB it was
11 smaller and 13 larger, net +220 bytes (`Cherry512.png` +113,
`file05.png` +155). The refined D4 parent helps more than it hurts, so the
two losses are accepted against the corpus gains.

<a id="guard-16-october"></a>

### Hundred-file guard

88 of 100 floors pass at their recorded allowances; the two repeating losses
of 13 October (`grayscale_alpha_8`, `skinsrc.zip`) now pass. The previous
benchmark executable (`c3c6f60`) fails 11 of the same 12 floors and is equal
or further from nine of them (`paperplane-fs8.png` +48 against +1 now,
`test-conversion-truecoloralpha-grayscalealpha.png` +10 against +2). Psyduck,
Squirtle and Pikachu keep their one-bit early-lineage cause, and Poliwrath's
2-bit floor is timing-dependent. At four times the recorded allowance, six of
the eight byte failures reach or beat their floors: `paperplane-fs8.png`
9,053 = 9,053, `Gingerman.png` 80,823 < 80,828, `Death.png` 21,056 < 21,057,
`sample_71.png` 52,573 < 52,594, `test-conversion-truecoloralpha-paletted.png`
2,996 < 3,004 and `sample_25.png` 11,798 < 11,806. The others:

- `crossroads.png` (floor 2,123): every build's complete Max search, 48
  seconds, ends at 2,128. At 10 seconds the deadline cuts the source-max
  route at 16,381 bits and the terminal methods take that parent to 16,215;
  given time the route reaches a better parent, 16,358 bits, which finishes
  at 16,221. A better parent finishing worse is the path dependence recorded
  for the early lineage; only refining both parents would recover it.
- `test-conversion-truecoloralpha-grayscalealpha.png` stays 2 bytes above its
  floor at 40 seconds.

## Smoothed-tree finish and Huffman repair on 13 October 2026

### Benchmark state

All three reports come from executable `3ceab7ae…` (`c3c6f60`), which adds
R14, the repeated tree finish and the corrected length repair of the generic
Huffman builder. Against the `2825c48f…` reports at the same allowances:

| Benchmark | Result |
| --- | --- |
| Defluff, 66 pairs | Unchanged: 61 wins and five ties, −109 bytes / −932 bits; no misses |
| DeflOpt Default, 957 rows | 252 smaller, 705 identical, none larger; net −30,170 bytes / −241,370 bits; runtime 1,254.3 → 1,286.1 s (+2.5%) |
| DeflOpt Max, 937 rows with unchanged allowances | 148 smaller, 757 identical, 32 larger; net −1,658 bytes / −13,303 bits; runtime 9,210.2 → 9,226.0 s |
| Timed deft4j, 1,621 pairs | 211 smaller, 1,362 identical, 48 larger; net −6,038 bytes / −48,332 bits; runtime 17,430.6 → 17,427.9 s |

The misses are unchanged: four DeflOpt rows on two files and twelve deft4j
rows. All strict misses reach parity in the relaxed audit; Max `T_Grass.png`
narrows from 2 bits to 1.

### Rows that grew

The 68 Max rows that grew in either benchmark were rerun as quiet pairs, two
repeats per executable. Together they favour the new executable, net
−482 bytes over both repeats; `oxipng/interlaced_odd_width.png` alone is
2,196 smaller. About half the losses do not repeat. Clean builds of each
commit attribute the repeating ones:

| File | Loss at 10 s | Cause |
| --- | ---: | --- |
| `css-ig-net/Peach512.png` | +138 bytes, also at 145 s | Huffman length repair (`8336d3a`) |
| `css-ig-net/Sock.png` | +57 | Huffman length repair; 2 bytes smaller at 30 s |
| `oxipng/grayscale_alpha_8_should_be_grayscale_alpha_8.png` | +200 | Repeated tree finish (+174) and R14 (+26), through the PNG early lineage |
| `medium-zip/skinsrc.zip` | +25 (guard +20) | Repeated tree finish, through ZIP Max's refined Default |
| `large/nerd.png` and smaller rows | mixed | Deadline-bound; vary between repeats |

**Length repair.** When an unlimited Huffman tree exceeded its depth limit,
the generic builder gave the longest repaired codes to the most frequent
symbols. Giving them the shortest codes, as zlib does, is strictly better for
every tree it builds. Default output is identical on all 957 DeflOpt
fixtures. In Max, the builder supplies one of the 7-bit code-length trees
that the exhaustive header search tries, and its feedback passes now settle
elsewhere: on Peach512 the bounded no-split route ends 178 bytes larger.
Paired Max runs of the commit alone, at the recorded allowances:

| Sample | Smaller / larger | Net | CPU |
| --- | --- | ---: | --- |
| 131 deft4j cases | 24 / 24 | −404 bytes / −3,238 bits | 7,389 → 7,315 s |
| 96 DeflOpt every-tenth Max cases | 1 / 3 | +156 bytes / +1,236 bits, of which Peach512 +148 | 1,717 → 1,714 s |

The correction is kept. Peach512's loss is a stable different basin rather
than a timing effect.

**Parent basins.** The repeated finish makes Default endpoints smaller, and
two lineages refine such an endpoint through `Established`: the PNG early
lineage and ZIP Max's refined Default. On `skinsrc.zip`, member 2's Default
falls from 106,980 to 106,975 bytes, but its refinement ends at 106,873
instead of 106,848. This is the effect recorded on 8 and 9 October. Withholding
finishing steps from such a parent, or refining both parents, lost on the
corpus in each form tested, so no change is made.

### Hundred-file guard

89 of 100 floors pass at their recorded allowances. After this refresh the
deft4j runner added five floors. Two are the repeating losses above:
`grayscale_alpha_8` (+200 bytes) and `skinsrc.zip` (+20). Two predate this
range: no build since `c28f1ad` reaches `medium/Gingerman.png`'s 80,828 bytes
(all give 80,831) or `test-convertir-truecoloralpha-trns.png`'s 9,635 bytes
(all give 9,636–9,637). `medium/Death.png` (+6) varies between repeats from
R14 onward. The six earlier failures keep their recorded causes: Psyduck,
Squirtle and Pikachu by one bit through the early lineage, Poliwrath's
timing-dependent floor, and the two timed cases, which reach their floors
with more time.

## Iterated slide and distance ladder on 9 October 2026

### Benchmark state

All three reports come from executable `069e236f…`, which adds the iterated
boundary slide and R13, the distance-alphabet ladder. Against the
`b4c4329b…` reports at the same allowances:

| Benchmark | Result |
| --- | --- |
| DeflOpt Default, 957 rows | 72 smaller, 885 identical, none larger; net −50,966 bytes; runtime 1,214.7 → 1,248.0 s (+2.7%) |
| DeflOpt Max, 943 rows with unchanged allowances | 97 smaller, 832 identical, 14 larger; net −1,536 bytes / −12,345 bits; runtime 9,314.4 → 9,310.2 s |
| Timed deft4j, 1,621 pairs | 158 smaller, 1,424 identical, 39 larger; net −1,207 bytes / −9,598 bits; runtime 17,429.9 → 17,448.2 s |
| Defluff, 66 pairs | Unchanged: 61 wins and five ties, −109 bytes / −932 bits |

Misses are unchanged: four DeflOpt rows on two files and twelve deft4j rows,
all strict misses.

<a id="rows-that-grew-9-october"></a>

### Rows that grew

Quiet runs of both executables at each recorded allowance:

- `oxipng/interlaced_odd_width.png`, the largest deft4j loss (+2,184 bytes at
  37 seconds), is 24 bytes smaller with the new executable.
- The other 38 larger deft4j rows net −1 byte / +7 bits: 7 smaller, 24
  identical and 7 larger. Only two losses repeat: `054-Psyduck-1.png` and
  `007-Squirtle.png`, one bit each.
- Of the four largest DeflOpt Max losses, only `css-ig-net/Apricot512.png`
  repeats: +361 bytes at 10 and 15 seconds. `BNDT…png` is smaller in quiet
  runs; `nerd.png` and `profile_gray_disallow_color.png` vary in both
  executables.

All three repeating losses are the
[early-lineage parent effect](#early-lineage-parent-8-october) of 8 October. R13 also
improves the PNG early lineage's quick floor (Apricot512 300,389 → 300,284
bytes; Psyduck 1,567 → 1,566 bits). The `Established` refinement then ends
worse from that better parent (Apricot512: 296,654 instead of 296,293 bytes of
Deflate). The main lineage differs only by timing and loses to the early
lineage in both executables. At 20 or 30 seconds the new executable is
10 bytes smaller (298,967 against 298,977).

The four early-lineage losses are accepted against the corpus wins. Rejected
alternatives:

- **Skip the quick floor's terminal finish.** Skipping R1c there lost
  3,156 bytes over 131 paired cases on 8 October. Skipping only R13 restored
  all three files, but over the same cases it was 4 smaller and 20 larger, net
  +1,148 bytes (+469 on `grayscale_alpha_8`, +586 on
  `download_webp__260×280_.png`); the DeflOpt every-tenth Max sample was
  neutral. R13's literal spellings lose matches for every descendant (Columbo
  never searches for new matches), but lost matches do not predict the better
  parent, so R13 stays.
- **Refine both parents.** Refining the unfinished quick floor (without R1c
  and R13) concurrently with the finished one recovered Apricot512
  (−637 bytes) and the Psyduck, Squirtle and Pikachu guard floors at unchanged
  wall time. The refinements compete for cores: over the 131 deft4j cases it
  was 12 smaller and 15 larger, net +3,183 bytes with 15% more CPU; on the
  DeflOpt sample, 2 smaller and 11 larger, net +149 bytes with 17% more
  CPU. Time-bound files lose most: `interlaced_odd_width.png`
  +2,338 bytes, `FsqwhPuaIAIlojU.png` +828 and `nerd.png` +127.
- **Refine the unfinished floor only with leftover allowance.** Psyduck,
  Squirtle and Apricot512 refine until the deadline, so only Pikachu recovered
  (2,090 → 2,089 bits), for 2.9 seconds more wall time on that file. One bit
  does not justify extra wall time on every image that finishes early.

<a id="guard-9-october"></a>

### Hundred-file guard

The new executable passes 93 of 100 floors at recorded allowances. Psyduck and
Squirtle fail by one bit each, as above. The two timed cases reach their floors
with more time: `css-ig-net/sample_61-fs8.png` gives 9,731 bytes at 60 seconds
(floor 9,738) and `medium/LevelLoading.png` 222,662 at 250 seconds (floor
222,666). A clean-build bisect of the three `pkmn` floors set on 22 September
corrects the 8 October attribution of all three to R1b:

| File | Floor | HEAD | Cause |
| --- | ---: | ---: | --- |
| `pkmn-bw/000-Logo-2.png` | 1,059 bits | 1,064 | R1b (`344a76a`); the earlier build keeps 1,059 at 60 seconds |
| `pkmn-bw-hard/062-Poliwrath-1.png` | 1,118 | 1,120 | Timing: `85bf3d7` gives 1,118 at 10 seconds but 1,120 at 30, and `b4a8249`, a speed-only change, moves the 10-second result |
| `pkmn-col-hard/025-Pikachu-2.png` | 2,089 | 2,090 | R1c (`bf2fe12`); the earlier build keeps 2,089 at 60 seconds |

### Strict completion as a final competitor

Logo-2 loses in Max's mandatory Default endpoint, which can become Max's
terminal parent. R1b ran there before R2–R5 and Max's R6–R9 strengthening,
and its uniform complete distance tree changed the header those searches start
from. The old path saved 37 bits through R6 and a final literal/length span;
with R1b first, R6 found nothing and the endpoint settled at 32.

**Accepted.** When R1b wins on a mandatory endpoint, R2–R5 and R6–R9 start from
the endpoint without R1b. Default's exact endpoint (R1b, then R2–R5, R1c and
R13) competes only after Max's terminal searches, which apply R1b to Max's own
incumbent, so Max never trails Default. R2–R5 run twice only when R1b wins.
R1c already followed this rule. It applies to the PNG, standalone,
`SharedExact` and `ApngMax` endpoints.

Only streams where R1b wins can change. A Default scan of every PNG in R1b's
128 KiB size class and every GZIP, ZIP and zlib fixture found 37: 12
single-image PNGs, 9 APNG stickers and 16 ZIP archives. Paired Max runs at
10 seconds:

| Cohort | Result |
| --- | --- |
| 12 single-image PNGs, two repeats | 4 smaller, 8 identical, none larger, every repeat identical: `000-Logo-2.png` 461 → 460 bytes (1,064 → 1,055 bits, below its 1,059 floor), both `060-Poliwag-2.png` −2 bits, `T_Grass.png` −1 bit |
| 16 ZIP archives, two repeats | Identical; their Max members do not build this endpoint |
| 9 APNG stickers, three repeats | Mean +0.7 bytes in total; individual files move up to ±8 bytes between repeats of the same executable |

Rejected first: moving R1b to the end of the terminal sweep changed none of the
three files and made `T_Grass.png` Default one byte larger; removing it from
the sweep alone left Logo-2 at 1,064, which located the effect in the
mandatory endpoint. The unit test
`strict_completion_competes_only_after_max_searches_its_parent` checks that
the competitor equals Default's output, that Max's parent keeps the planned
`[1, 1]` completion, and that Max does not trail Default.

The guard then passes 94 of 100; the six failures are Psyduck, Squirtle,
Poliwrath, Pikachu and the two timed cases. Pikachu's is also the early-lineage
parent effect: both executables reach 2,090 bits in the main lineage, while
the early lineage refines a quick floor that R1c slid to 2,092 bits and finds
nothing; the pre-R1c build reached 2,089 from the unslid floor.

All three reports refreshed with the accepted executable, `2825c48f…`, against
the `069e236f…` states:

| Benchmark | Result |
| --- | --- |
| Defluff, 66 pairs | Unchanged: 61 wins and five ties, −109 bytes / −932 bits; no misses |
| DeflOpt Default, 957 rows | Identical; runtime 1,248.0 → 1,254.3 s |
| DeflOpt Max, 939 rows with unchanged allowances | 19 smaller, 892 identical, 28 larger; net −132 bytes / −1,025 bits; runtime 9,267.6 → 9,271.0 s |
| Timed deft4j, 1,621 pairs | 52 smaller, 1,532 identical, 37 larger; net +32 bytes / +232 bits; runtime 17,448.2 → 17,430.6 s |

The 37 R1b files move exactly as in the paired runs: Logo-2 −1 byte / −9 bits,
each Poliwag-2 −2 bits and T_Grass −1 bit in both benchmarks, net −1 byte /
−12 bits per benchmark. Their one other moved row,
`8x8-zip/Architecture.playdate-pulp.zip` (+2 bits), gives 72,567 bits with
both executables in quiet runs at its 13-second allowance. Every other moved
row, mostly APNGs, is a file the change cannot reach. Quiet paired runs of the
37 deft4j rows that grew favor the accepted executable (14 smaller,
15 identical, 8 larger; net −149 bytes / −1,186 bits), so the journal's
+32 bytes is benchmark-load variance. Misses are unchanged: four DeflOpt and
twelve deft4j strict misses, all at parity in the relaxed audit except the
signed PNG.

## Linear finalization share on 8 October 2026

### Starting state

Before this change, the complete DeflOpt journal (`c2678492…`) had 1,914 rows,
four strict misses on two files, no errors, and no Max row worse than Default.
Defluff passed all 66 pairs: 61 wins and five ties, saving 109 bytes /
932 bits. The timed deft4j report dated from 22 September (`64dad99f…`). All
three are refreshed below with the accepted executable, `b4c4329b…`.

### DeflOpt misses are strict-policy floors

Exact header inspection reproduces both references' bit counts. Both gaps are
the cost of strict output, not missed routes; `--strict 0` beats both
references.

- `samplelib-png/sample-green-400x300.png` has identical payloads. The
  reference sends one 1-bit distance code (HDIST 1); strict output must send a
  complete tree, `[1, 1]`, whose extra code-length entry costs 2 bits, the
  least any complete tree can add.
- `small/T_Grass.png` is literal-only. DeflOpt's literal tree is the
  Huffman-optimal tree, 2,513 payload bits; Columbo's trades 7 payload bits for
  a shorter header and, with the empty distance alphabet relaxed output may
  send, totals 2,774 bits, ten fewer than DeflOpt. Strict completion adds
  14 bits (cheapest: the uniform depth-3 tree); the Huffman tree with any
  strict completion needs at least 2,801 bits.

### Regressions against the 29 September journal

No Default row is worse in any format; Default PNG improves 366 rows and ties
559, saving 13,625 bytes. Twenty Max rows are worse, by 336 bytes in total,
against a net Max saving of 2,252 bytes. Serial reruns of the ten largest with
builds of the 29 September source (`ba00f08`) and the current source, two
repeats each at the recorded allowance, tie or favor the current build except
one `Apricot512.png` repeat 9 bytes larger. The journal losses are deadline
variance under benchmark load: the journal's `download_webp__260×280_.png` Max
row is 208,849 bytes, while quiet runs of the same executable give 208,734.

### Hundred-file guard

Both the current and accepted executables pass 98 of 100 floors at recorded
allowances. The two timed cases reach their floors with more time
(`css-ig-net/sample_61-fs8.png` 9,731 bytes at 60 seconds, floor 9,738;
`medium/LevelLoading.png` 222,662 at 250 seconds, floor 222,666), and on both
the current build beats the 29 September build at 10 and 60 seconds.

### Accepted change

Max streams outside the 1 MiB terminal header class reserved no terminal time,
so primary routes ran through their grace to the hard boundary and R1c, which
has no size class, never started on the final incumbent. The PNG early lineage
(an `Established` continuation) had no share at all, and a Max sweep whose last
method changed the candidate skipped R1c even when no further sweep could
start. Default already slides these streams, so Max missed a systematic saving
on the largest files.

- R1c is linear finalization, like the bounded-depth tree floor: it may start
  until the hard stop it polls. In Max it waits for a settled sweep only while
  another sweep could start.
- `Complete` and `CompleteThenBounded` streams and APNG Max children outside
  the class, and `Established` continuations outside it, reserve a
  finalization share: primary routes keep 19/20 of the allowance and their
  phase grace, so they end at least 1/20 before the hard boundary.

Rejected before the full run: copying the search share's zero phase grace cost
about 2.3 seconds of search on a 10-second allowance (`large/nerd.png`
263 bytes larger in three of three runs). It also returned single PNG images
before the file deadline, so an early-lineage timeout started a reclaim pass
whose mandatory floor overran a 14-second allowance to 22 seconds.

| Cohort | Result |
| --- | --- |
| 15 large PNG streams at their DeflOpt Max allowances, two alternating repeats | All 15 improve on average, saving 5,513 bytes; every run stays within its hard boundary |
| DeflOpt Max sample, every tenth case (96 PNG/ZIP/GZIP) | 22 smaller, 73 identical, one 5 bytes larger (`nerd.png`, whose repeats average a 169-byte saving); net −483 bytes / −3,856 bits; wall 954.3 → 953.3 s |
| Hundred-file guard | Identical to the current executable: 98 of 100 at recorded allowances |
| Timed deft4j, 1,621 pairs, against the 22 September journal at the same allowances | 483 smaller, 1,127 identical, 11 larger; net −27,168 bytes / −217,403 bits; runtime 17,721.6 → 17,429.9 s; misses 13 → 12, all strict-policy floors except the preserved signed PNG |
| DeflOpt, 1,914 rows, against the `c2678492…` journal | Default identical on all 957 rows. Max 242 smaller, 701 identical, 14 larger; net −11,602 bytes / −92,826 bits; Max runtime 9,634.1 → 9,596.9 s. The same four strict misses remain; the relaxed audit, included in this run, shows all four reach parity. No Max row is worse than Default |
| Defluff, 66 pairs | Unchanged: 61 wins and five ties, −109 bytes / −932 bits |

<a id="early-lineage-parent-8-october"></a>

### Rows that grew

Ten of the eleven larger deft4j rows are not regressions of this change: quiet
paired runs of HEAD and the accepted build, two repeats each, tie or favor the
accepted build. Three of them, `pkmn` losses of 1, 2 and 5 bits, are
deterministic, finish in about a second, and cost no bytes. They were first
attributed here to R1b; the 9 October bisect traced them to R1b, R1c and a
timing-dependent floor.

`oxipng/grayscale_alpha_8_should_be_grayscale_alpha_8.png` is a real,
deterministic loss at 10 seconds, 80,094 → 80,637 bytes; at 60 seconds HEAD
and the accepted build both reach 80,014. The PNG early lineage's quick floor
ends with R1c, and its `Established` refinement starts from that slid parent.
From the unslid parent the floor-seeded route finds nothing, and the lineage
reaches 80,031 bytes of Deflate through deft4j-derived refinement. From the
slid parent that route finds 46 bytes first, the lineage follows it, and
reaches only 80,574.

A `CompleteParent` policy omitting R1c from that quick floor restored the file
but was rejected. Over 131 paired cases (every larger row, the 40 largest
wins, 40 further wins and 40 unchanged rows) it was 9 smaller and 20 larger,
net +3,156 bytes, losing 2,452 bytes on `oxipng/interlaced_odd_width.png` and
1,013 on `medium/FsqwhPuaIAIlojU.png`. On the first, both parents continue
through deft4j-derived refinement, but the slid parent's deft4j-derived source
starts 6,554 bytes smaller. Parent size does not predict the better endpoint,
so the loss is recorded, not fitted.

The DeflOpt refresh shows the same two kinds. Of its 14 larger Max rows,
`grayscale_alpha_8` (+543 bytes) and `css-ig-net/Apricot512.png` (+64) are
deterministic parent-choice losses: Apricot512's early lineage refines a slid
300,389-byte quick floor instead of the unslid 300,470-byte one and ends
179 bytes larger. Quiet paired runs tie or favor the accepted build on the
other twelve. The deft4j runner then rotated `grayscale_alpha_8` and the three
`pkmn` files into the hundred-file guard at their earlier floors, which the
accepted build misses for these reasons.

## Strict distance completion on 30 September 2026

<a id="reference-misses-30-september"></a>

### Reference misses

The remaining misses are four DeflOpt rows (two files) and 13 timed-deft4j
rows. Apart from the signed PNG, an independent header inspector shows that
every reference uses an RFC 1951 degenerate distance tree that strict mode
completes: a single one-bit code (half the code space) in 13 rows, or an empty
alphabet in the three `small/T_Grass.png` rows (two reference files).

A singleton needs at least one more code-length symbol, which costs 2–4 bits in
each reference's own header (the code-length code for value 1). Columbo's
singleton gaps are 2–3 bits, at or below that floor, so they are the cost of
the strict policy. The empty case had slack: planning always completed it as
`[1, 1]`, whose code-length symbol 1 sits at position 18 of RFC 1951's
code-length order and raised the number of code-length codes (HCLEN + 4) from
14 to 18.

### Accepted change

Terminal step R1b re-prices each finished strict block whose tokens use at
most one distance symbol with four uniform complete distance trees (depths
1–4). Any complete code covering the used symbol decodes the same payload, so
the choice is exact header cost plus the used symbol's payload change. See
[routes and methods](routes-and-methods.md) for its gates. Adding the same
candidates to the central dynamic planner produced the same wins, but that
planner prices every trial block, so median CPU time on affected small files
rose about 2.4× (`T_Grass` 0.086 → 0.207 s). The terminal step prices each
finished degenerate block once.

### Evidence

Paired concurrent runs of the pre-change and candidate executables compare CPU
time rather than wall time; timed Max trials ran only without competing load.

| Cohort | Result |
| --- | --- |
| DeflOpt corpus Default, 957 files | 943 identical; 13 smaller (PNG and ZIP), saving 18 bytes / 141 meaningful bits; one +1-byte row in a time-sliced metadata stream did not reproduce in five paired repeats. CPU 2,142.3 → 2,143.4 s (+0.05%). |
| Hundred-file guard, Max at recorded allowances | 91 identical; 3 ZIPs smaller by 6 bytes / 53 bits; 6 apparent losses. Paired triple repeats make four byte-identical; R1b finds no candidate in the other two, which vary in both binaries. Both binaries pass the same 92 floors. |
| Timed deft4j strict misses, Max | `T_Grass` moves from +4 to −1 meaningful bit against its reference, resolving the miss. The eleven singleton misses are unchanged. |
| DeflOpt `T_Grass`, strict | The gap narrows from 15 to 2 bits; it remains one byte because 2 bits cross a byte boundary. |

### More time and reachability

The eight guard floors missed at normal allowances, and the six DeflOpt Max
rows flagged as losing more than 10% of their earlier lead, were rerun with
the candidate at 60 seconds (300 seconds for the 136-second `BNDT…` case).

| Source | Longer result vs historical floor or published Max |
| --- | --- |
| `medium/BNDT_on_X…png` (300 s) | −91 bytes / −730 bits |
| `oxipng/interlaced_grayscale_16_should_be_grayscale_16.png` | −75 / −593 |
| `oxipng/filter_0_for_grayscale_16.png` | −59 / −467 |
| `css-ig-net/sample_71.png` | −44 / −351 |
| `css-ig-net/sample_53.png` | −25 / −204 |
| `css-ig-net/sample_25.png` | −7 / −56 |
| `css-ig-net/sample_61-fs8.png` | −5 / −39 |
| `small-zip/Alleyway (EMU).zophar.zip` | −4 / −32 |
| `small-zip/kskinmkr_src.zip` | 0 / 0 |
| `kensilverman-gz/pngout-20200115-bsd.tar.gz` | 0 / 0 |
| `medium/LevelLoading.png` | +6 / +43 at 60 s; the earlier 250-second witness reaches its floor |
| `css-ig-net/my-computer-on-fs8.png` | +3 / +26 at 60 and 180 s |

Every route remains reachable. `my-computer-on-fs8.png` is time-nonmonotonic,
not a code regression: the candidate reaches the published 13,658 bytes in
three of three 10-second runs, R1b does not run on it, and the pre-change
executable also gives 13,661 bytes at 180 seconds, where a different lineage
wins an intermediate comparison. It is recorded, not fitted with a
file-specific rule. Private paired records and harness scripts are under
`work/strict-completion-20260930/`.

## Earlier refreshes

Counts are smaller / identical / larger rows against the baseline, or against
the historical floors for the guard; negative deltas mean smaller output.

| Date | Executable (baseline) | Headline result | Accepted change |
| --- | --- | --- | --- |
| 26 September 2026 | `9b0b6fa3…` (`e3325451…`) | Two APNGs recurred as Default speed-gate flags because Default scheduled their 240 and 100 image streams serially. Afterward a 29-file Default holdout kept identical bytes and bits in 80.95 s instead of 279.61 s, and 30 APNG-named deft4j cases saved 494 bytes / 4,057 bits | Bounded multi-image Default work runs on up to eight worker lanes, limited by the CPU count ([multithreading](routes-and-methods.md#multithreading)) |
| 22 September 2026 | `e3325451…` (`64dad99f…`) | Both APNG residuals beat their older targets at 60 seconds | Eligible `ApngMax` children reserve 1/20 of their allowance for terminal methods ([details](#apng-terminal-share-follow-up-on-22-september-2026)) |
| 22 September 2026 | `64dad99f…` (`6c601870…`) | Default identical; DeflOpt Max 147 / 776 / 34, −7,335 bytes / −58,679 bits; timed deft4j 204 / 1,390 / 27, −2,104 / −16,765; seventeen strict misses; every guard floor reached, 96 at normal allowances and four with more time | None; speed-only changes |
| 15 September 2026 | `6c601870…` (`4d4e1586…`; deft4j `e3bd1f72…`) | DeflOpt Max 117 / 766 / 74, +5,416 bytes / +43,326 bits; timed deft4j 613 / 957 / 51, −3,606 / −29,089, with 15.4% less runtime; guard 89 / 7 / 4 | Max allows R2–R5 in its 1 MiB size class (`11f8f7f1…`): 138 static PNGs 116 / 16 / 6, −1,641 bytes / −13,142 bits ([validation](research/terminal-header-work-class-validation.md)) |
| 13 September 2026 | `4d4e1586…` (`c3f63f77…`) | DeflOpt Max 498 / 443 / 16, −6,320 bytes / −50,674 bits, with 15.9% less time at matched allowances; Default identical; guard 89 / 9 / 2 | Terminal reservation extended from the 128 KiB R1–R5 class to the 1 MiB Max size class (`cd2e432f…`): 138 static PNGs 85 / 30 / 23, −1,009 bytes / −8,114 bits, 6.0% less runtime ([validation](research/terminal-closure-validation.md#follow-up-on-13-september-2026-the-larger-max-work-class)) |
| 11 September 2026 | Baseline `a3d5c69d…` | Seventeen strict misses reproduce, sixteen at relaxed parity plus the signed PNG; guard 99 of 100 | Terminal scheduling and closure correction: the guard replay saves 315 bytes / 2,527 bits against the baseline with 8.3% less trial time ([validation](research/terminal-closure-validation.md)) |
| Through 8 September 2026 | `c5036306…`; complete journals `a700a360…` | DeflOpt six strict-policy rows on three files; timed deft4j twelve strict-policy rows plus the signed PNG; APNG doubled-time pass recovers 68 of 70 prior misses; guard 97 of 100 | The [accepted general changes](#accepted-general-changes) |

## APNG terminal-share follow-up on 22 September 2026

A long primary route could finish on a useful parent with no time left for
short terminal header passes. Eligible `ApngMax` children therefore reserve the
last 1/20 of their soft allowance for terminal methods and leave 19/20 for
primary routes. The rule uses the existing Max size class (≤1 MiB
compressed/decoded, ≤128 source blocks), adds no worker or file-wide allowance,
and keeps the complete Default file floor. Candidate `e3325451…` improves both older APNG
targets at their original 60-second allowances against `64dad99f…`. On the 30
APNG-named timed deft4j cases at recorded allowances, 26 improve, two tie and
two lose in file bytes, a net 2,866 bytes / 22,867 bits saved; no other
benchmark or guard source has APNG frames.

| Source prefix | `64dad99f…` bytes / bits | Candidate bytes / bits | Candidate vs older target |
| --- | ---: | ---: | ---: |
| `2313020_361766…` | 65,394 / 517,985 | 65,330 / 517,483 | −60 bytes / −463 bits |
| `657730_102978…` | 172,311 / 1,371,475 | 172,231 / 1,370,827 | −78 bytes / −634 bits |

## Accepted general changes

These rules, accepted through 8 September 2026, derive from search topology,
complete-stream scoring and bounded resource models, with no corpus identity
or expected score. The [route catalog](routes-and-methods.md) describes their
current form.

- **Topology-aware no-split order (M2).** Two or three blocks, with at most
  two adjacent merge boundaries, run individual one-match pruning first; four
  or more nonempty blocks run cumulative pruning first, so work on an early
  block cannot exhaust the route window before later alignment and merge
  states. With enough time, Max prices the complementary policy as an
  independent late route.
- **Independent topology closure.** A slightly larger floor-seeded,
  deft4j-derived, source-max or changed no-split parent can still win once the
  bounded terminal methods change only its Huffman trees. A losing topology is
  therefore closed before it is discarded, and a winning one once, at the
  final Max stage. For the same reason M2c has no completed-sibling score
  gate.
- **Deterministic hard-boundary rescue.** The bounded-depth rescue prices every
  dynamic block, not only the largest, within ≤1 MiB compressed, ≤1 MiB
  decoded and ≤128 source blocks; with frequencies known, the frontier size is
  independent of token count. A deadline crossed a few milliseconds earlier,
  for example with detailed progress enabled, therefore cannot change which
  block is rescued.
- **Deferred source-max structural closure (M10d).** Under `Complete` and
  `CompleteThenBounded`, one compact completed source-max parent is kept even
  when it is not the timed winner, because it can expose a useful split
  topology; the split closure runs on it after the timed siblings (≤16 KiB
  compressed, ≤256 KiB decoded, ≤16 Ki tokens, one to four nonempty non-stored
  blocks). Shared container policies omit it, so no ZIP member, GZIP member,
  metadata stream or APNG frame can use time reserved for later streams.
- **Bounded long-match source scheduling (M8).** On a large one-block PNG, the
  complete-floor beam normally has cache and range-materialization bandwidth
  to itself before source max runs. Two compact graph shapes let them overlap:
  at least one repartition run per 16 source tokens, or ≤4,000 tokens
  averaging ≥224 decoded bytes per token. The second threshold, seven eighths
  of 256 bytes, selects graphs of almost only long matches, which are cheap to
  price within the 16 KiB compressed size class.
- **Fair APNG independent roots.** A frame's proportional time share can be
  used up by a floor continuation or direct-deft refinement, and parent size
  cannot predict which topology reaches the better endpoint. Under bounded
  `ApngMax`, original-source Max therefore runs concurrently beside either, and
  the retained incumbent still gates selection. The bounded single-image PNG
  policy uses the same overlap. At doubled time this recovered
  68 of the 70 prior APNG byte-and-bit floors and was 1,800 bytes /
  14,440 meaningful bits smaller in aggregate than the previous binary.

Retained controls: a complete Default floor before timed Max work; cooperative
finish within timeout + 10% + 1 second; the narrow source route's bound of
1 MiB compressed input and 128 nonempty blocks; overlap of independent deft4j
and floor-seeded topologies within the bounded worker and memory envelope; a
cheap pass over compact split's structural cut set before exact finalization;
and candidate identity and completed-plan caches against replaying equivalent
states.

## Accepted trade-offs

Current:

- **Early-lineage parent choice.** The PNG early lineage's quick floor ends
  with R1c and R13, and its `Established` refinement can end worse from that
  better parent: `grayscale_alpha_8` on 8 October, and Apricot512, Psyduck,
  Squirtle and Pikachu on 9 October. Parent size does not predict the better
  endpoint, and each fix tried cost more bytes or time than it recovered
  ([8 October](#early-lineage-parent-8-october), [9 October](#rows-that-grew-9-october)).
- **Timing.** Poliwrath's floor depends on timing, `sample_61-fs8.png` and
  `LevelLoading.png` reach theirs only with more time
  ([guard](#guard-9-october)), and `my-computer-on-fs8.png` is
  time-nonmonotonic ([30 September](#more-time-and-reachability)). None gets a
  file-specific rule.
- **Source-max replay parent.** `crossroads.png` and `074-Geodude-1.png` end
  better from a source-max parent that the deadline cut short than from the
  replayed one; only refining both parents would recover them
  ([16 October](#guard-16-october), [17 October](#guard-17-october)).
- **APNG fit order.** With R2–R5 after R18, R5's late rewrites miss the tree
  floors and refit that the sweep order gave them: `apng-medium/ball.png` is
  50 bytes above `6c81dfa`, against 261,016 bytes saved on the sticker corpus;
  a refit after R5 recovered too little for its cost
  ([17 October](#rows-that-grew-17-october)).
- **Strict policy.** The strict misses are the cost of strict output; relaxed
  output reaches parity on all but the signed PNG
  ([DeflOpt](#deflopt-misses-are-strict-policy-floors),
  [deft4j](#reference-misses-30-september)).

Accepted on 8 September 2026 and since superseded:

- Three guard residuals totaling 11 bytes / 84 meaningful bits, the largest
  `medium/LevelLoading.png` at 6 bytes / 49 bits. By 11 September only
  LevelLoading remained.
- Two APNG general-route trade-offs, `2313020_361766…` 4 bytes / 39 bits and
  `657730_102978…` 2 bytes / 14 bits above the previous binary. Restoring them
  by changing general Huffman/tree pricing caused five rolling-guard
  regressions; the
  [22 September terminal share](#apng-terminal-share-follow-up-on-22-september-2026)
  took both below their older targets.
- Two sampled Max endpoints that were not reproduced lie outside the changed
  size class and still pass the reference and Max-over-Default gates: timed
  search variance, not a reason for a corpus-specific rule.

## Verification

Each dated entry records its own checks. The 8 September integrated candidate
passed:

- `cargo test --locked`: 507 tests (448 library, 46 CLI, 13 public API);
- `cargo fmt --check`, `cargo clippy --locked --all-targets -- -D warnings`
  and `git diff --check`.

Tracked source and `Cargo.toml` contain no user, repository or temporary
absolute path. Distribution binaries remain subject to the release path
audit; ordinary developer builds may retain toolchain paths.

Accepted routing and provenance are maintained in
[routes and methods](routes-and-methods.md).
