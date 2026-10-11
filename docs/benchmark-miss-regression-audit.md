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

The misses are unchanged: four DeflOpt rows on two files and twelve deft4j
rows, all strict-policy floors except the preserved signed PNG.

### Rows that grew

Quiet serial pairs of the two executables at each recorded allowance
separate code effects from benchmark-load variance.

- `oxipng/interlaced_odd_width.png`, the largest deft4j loss (+2,184 bytes at
  37 seconds), is 24 bytes smaller with the new executable when run alone.
- The other 38 larger deft4j rows net −1 byte / +7 bits: 7 smaller, 24
  identical and 7 larger. Only two losses repeat: `054-Psyduck-1.png` and
  `007-Squirtle.png`, one bit each.
- Of the four largest DeflOpt Max losses, only `css-ig-net/Apricot512.png`
  repeats: +361 bytes at 10 and 15 seconds. `BNDT…png` is smaller in quiet
  runs, and `nerd.png` and `profile_gray_disallow_color.png` vary in both
  executables.

The three repeating losses have one cause, the early-lineage parent effect
recorded on 8 October. R13 now also improves the PNG early lineage's quick
floor: Apricot512's falls from 300,389 to 300,284 bytes and Psyduck's from
1,567 to 1,566 bits. The `Established` refinement then ends worse from the
better parent: 296,654 instead of 296,293 bytes of Deflate on Apricot512. Its
main lineage differs only by timing and loses to the early lineage in both
executables. With 20 or 30 seconds the new executable is 10 bytes smaller
(298,967 against 298,977), so the route stays reachable.

Withholding the quick floor's terminal finish only reverses the coin.
Withholding R1c there lost 3,156 bytes over 131 paired cases on 8 October.
Withholding only R13 was tested on its own premise: Columbo never searches for
new matches, so matches the ladder spells as literals in a parent are lost to
every descendant. It restored all three files, but over the same 131 cases it
was 4 smaller and 20 larger, net +1,148 bytes, including +469 on
`grayscale_alpha_8` and +586 on `download_webp__260×280_.png`; the DeflOpt
every-tenth Max sample was neutral. Lost matches do not predict the basin, so
no change is made there.

Hedging instead of choosing was also tested, on the principle the main
lineage already applies to its continuations: score order between complete
parents does not prove order between their endpoints. A prototype built the
unfinished quick floor (without R1c and R13) beside the finished one and
refined both concurrently, keeping the better endpoint. It recovered
Apricot512 (−637 bytes) and the Psyduck, Squirtle and Pikachu guard floors at
unchanged wall time. Over the same 131 deft4j cases it was 12 smaller and 15
larger, net +3,183 bytes with 15% more CPU; on the DeflOpt sample, 2 smaller
and 11 larger, net +149 bytes with 17% more CPU. The two refinements compete
for cores, and time-bound files lose most: `interlaced_odd_width.png`
+2,338 bytes, `FsqwhPuaIAIlojU.png` +828 and `nerd.png` +127. Rejected.

A work-conserving form avoids that competition: refine the unfinished quick
floor only after the first refinement finishes with allowance left, as the
reclaim passes spend real leftover time. Time-bound files cannot change. But
Psyduck, Squirtle and Apricot512 also refine until the deadline, so only
Pikachu was recovered (2,090 → 2,089 bits), at 2.9 seconds more wall time on
that file. One bit does not justify extra wall time on every early-finishing
image, so this variant was rejected as well. The four early-lineage losses
are accepted against the corpus wins.

### Hundred-file guard

The new executable passes 93 of 100 floors at their recorded allowances.
Psyduck and Squirtle fail by one bit each for the reason above. The two timed
cases reach their floors with more time: `css-ig-net/sample_61-fs8.png` gives
9,731 bytes at 60 seconds (floor 9,738) and `medium/LevelLoading.png` 222,662
at 250 seconds (floor 222,666).

The three `pkmn` floors set on 22 September were bisected with clean builds of
each commit; the 8 October attribution of all three to R1b was wrong.

| File | Floor | HEAD | Cause |
| --- | ---: | ---: | --- |
| `pkmn-bw/000-Logo-2.png` | 1,059 bits | 1,064 | R1b (`344a76a`); the earlier build keeps 1,059 at 60 seconds |
| `pkmn-bw-hard/062-Poliwrath-1.png` | 1,118 | 1,120 | Timing: `85bf3d7` gives 1,118 at 10 seconds but 1,120 at 30, and `b4a8249`, a speed-only change, moves the 10-second result |
| `pkmn-col-hard/025-Pikachu-2.png` | 2,089 | 2,090 | R1c (`bf2fe12`); the earlier build keeps 2,089 at 60 seconds |

### Strict completion as a final competitor

Logo-2's loss is in Max's mandatory Default endpoint. R1b ran first, before
R2–R5 and before Max's R6–R9 strengthening, and that endpoint can become Max's
terminal parent. On Logo-2 the old path reached 37 saved bits through R6 and a
final literal/length span; with R1b adopted first, R6 found nothing and the
endpoint settled at 32. R1b fits the current state, a uniform complete
distance tree chosen for the current payload, which changes the header the
later searches start from. R1c already follows the matching rule: Max keeps
the slid endpoint only as a final competitor.

**Accepted.** When R1b wins on a mandatory endpoint, its R2–R5 finish and
R6–R9 strengthening now start from the endpoint without R1b. Default's exact
endpoint, R1b then R2–R5, R1c and R13, is kept only as a final competitor,
compared after Max's terminal searches, which apply R1b to Max's own
incumbent. Max therefore never trails Default. R2–R5 run twice only when R1b
wins. The same rule applies to the PNG, standalone, `SharedExact` and
`ApngMax` endpoints.

Only streams where R1b wins can change, so the complete affected set was
measured. A Default scan of every PNG within R1b's 128 KiB class and every
GZIP, ZIP and zlib fixture found 37 such files: 12 single-image PNGs, 9 APNG
stickers and 16 ZIP archives. Paired Max runs at 10 seconds, two or three
repeats each:

| Cohort | Result |
| --- | --- |
| 12 single-image PNGs, two repeats | 4 smaller, 8 identical, none larger, every repeat identical: `000-Logo-2.png` 461 → 460 bytes (1,064 → 1,055 bits, below its 1,059 floor), both `060-Poliwag-2.png` −2 bits, `T_Grass.png` −1 bit |
| 16 ZIP archives, two repeats | Identical; their Max members do not build this endpoint |
| 9 APNG stickers, three repeats | Mean +0.7 bytes in total; individual files move up to ±8 bytes between repeats of the same executable |

Two variants were rejected first. Moving R1b to the end of the terminal sweep
changed none of the three files and made `T_Grass.png` Default one byte
larger. Removing it from the sweep alone left Logo-2 at 1,064, which located
the effect in the mandatory endpoint. The new unit test
`strict_completion_competes_only_after_max_searches_its_parent` checks that
the competitor equals Default's output, that Max's parent keeps the planned
`[1, 1]` completion, and that Max does not trail Default.

With the accepted change the hundred-file guard passes 94 of 100: Logo-2 now
passes, and the six remaining failures are Psyduck, Squirtle, Poliwrath,
Pikachu and the two timed cases, for the causes above. Pikachu's is the
early-lineage parent effect too: both executables reach 2,090 bits in the main
lineage, while the early lineage refines a quick floor that R1c slid to 2,092
bits and finds nothing; the pre-R1c build reached 2,089 from the unslid floor.

All three reports were then refreshed with the accepted executable,
`2825c48f…`, and compared with the `069e236f…` states:

| Benchmark | Result |
| --- | --- |
| Defluff, 66 pairs | Unchanged: 61 wins and five ties, −109 bytes / −932 bits; no misses |
| DeflOpt Default, 957 rows | Identical; runtime 1,248.0 → 1,254.3 s |
| DeflOpt Max, 939 rows with unchanged allowances | 19 smaller, 892 identical, 28 larger; net −132 bytes / −1,025 bits; runtime 9,267.6 → 9,271.0 s |
| Timed deft4j, 1,621 pairs | 52 smaller, 1,532 identical, 37 larger; net +32 bytes / +232 bits; runtime 17,448.2 → 17,430.6 s |

Only the 37 files where R1b wins can change. Their rows show exactly the
paired results: Logo-2 −1 byte / −9 bits, each Poliwag-2 −2 bits and T_Grass
−1 bit in both benchmarks, net −1 byte / −12 bits per benchmark. The one other
affected row, `8x8-zip/Architecture.playdate-pulp.zip` (+2 bits), gives 72,567
bits with both executables in quiet runs at its 13-second allowance. Every
other moved row is a file the change cannot reach, mostly APNGs. Quiet paired
runs of the 37 deft4j rows that grew, one per executable at each recorded
allowance, favour the accepted executable: 14 smaller, 15 identical and 8
larger, net −149 bytes / −1,186 bits. The deft4j journal's +32 bytes is
therefore benchmark-load variance. The misses are unchanged: four DeflOpt rows and twelve deft4j rows,
all strict-policy floors that reach parity in the relaxed audit except the
preserved signed PNG.

## Linear finalization share on 8 October 2026

### Current benchmark state

Before this change, the complete DeflOpt journal from executable
`c2678492…` had 1,914 rows, four strict misses representing two files, no
errors, and no Max row worse than its Default row. The Defluff comparison
passed all 66 pairs: 61 wins and five ties, saving 109 bytes / 932 bits. The
timed deft4j report was last completed on 22 September by `64dad99f…`. All
three reports are refreshed below with the accepted executable, `b4c4329b…`.

### DeflOpt misses are strict-policy floors

An exact header inspection reproduces both references' bit counts.

- `samplelib-png/sample-green-400x300.png` has identical payloads. The
  reference sends one 1-bit distance code (HDIST 1); strict output must send a
  complete tree, `[1, 1]`. The extra code-length entry costs 2 bits, the least
  any complete tree can add.
- `small/T_Grass.png` is literal-only. DeflOpt's literal tree is the
  Huffman-optimal tree, 2,513 payload bits; Columbo's trades 7 payload bits for
  a shorter header. With an empty distance alphabet, as relaxed output may
  send, Columbo's tree totals 2,774 bits, ten fewer than DeflOpt. Strict
  completion adds 14 bits, the cheapest being the uniform depth-3 tree; the
  Huffman tree with any strict completion needs at least 2,801 bits.

Both gaps are the cost of strict output, not missed routes; `--strict 0`
beats both references.

### Regressions against the 29 September journal

No Default row is worse in any format: Default PNG improves 366 rows and ties
559, saving 13,625 bytes. Twenty Max rows are worse, by 336 bytes in total,
against a net Max saving of 2,252 bytes. The ten largest were rerun serially
with executables built from the 29 September source (`ba00f08`) and the
current source, two repeats each at the recorded allowance. The current build
ties or wins on all ten, except one `Apricot512.png` repeat 9 bytes larger.
The worse journal rows came from deadline variance under benchmark load: the
journal's `download_webp__260×280_.png` Max row is 208,849 bytes, while quiet
runs of the same executable give 208,734.

### Hundred-file guard

Both the current and accepted executables pass 98 of 100 floors at their
recorded allowances. The other two are timed cases. `css-ig-net/sample_61-fs8.png`
reaches 9,731 bytes at 60 seconds (floor 9,738) and `medium/LevelLoading.png`
222,662 at 250 seconds (floor 222,666). On both, the current build beats the
29 September build at 10 and 60 seconds.

### Accepted change

R1c has no size class, but Max streams outside the 1 MiB terminal header class
reserved no terminal time: primary routes ran through their grace to the hard
boundary, so the slide never started on the final incumbent. The PNG early
lineage, an `Established` continuation, had no share at all, and a Max sweep
whose last method changed the candidate skipped R1c even when no further sweep
could start. Default already slides these streams, so Max left a systematic
saving behind on exactly the largest files.

- R1c is now linear finalization, like the bounded-depth tree floor: it may
  start until the hard stop it polls. In Max it waits for a settled sweep only
  while another sweep could start.
- Owners and APNG Max children outside the class, and `Established`
  continuations outside it, reserve a finalization share. Primary routes keep
  19/20 of the allowance and their phase grace, so they end at least 1/20
  before the allowance's hard boundary.

One variant was rejected before the full run. Copying the search share's
zero phase grace cost about 2.3 seconds of search on a 10-second allowance:
`large/nerd.png` became 263 bytes larger in three of three runs. It also
returned single PNG images before the file deadline, which let an
early-lineage timeout start a reclaim pass whose mandatory floor overran a
14-second allowance to 22 seconds. Keeping the phase grace removes both
effects.

| Cohort | Result |
| --- | --- |
| 15 large PNG streams at their DeflOpt Max allowances, two alternating repeats | All 15 improve on average, saving 5,513 bytes; every run stays within its hard boundary |
| DeflOpt Max sample, every tenth case (96 PNG/ZIP/GZIP) | 22 smaller, 73 identical, one 5 bytes larger (`nerd.png`, whose repeats average a 169-byte saving); net −483 bytes / −3,856 bits; wall 954.3 → 953.3 s |
| Hundred-file guard | Identical to the current executable: 98 of 100 at recorded allowances |
| Timed deft4j, 1,621 pairs, against the 22 September journal at the same allowances | 483 smaller, 1,127 identical, 11 larger; net −27,168 bytes / −217,403 bits; runtime 17,721.6 → 17,429.9 s; misses 13 → 12, all strict-policy floors except the preserved signed PNG |
| DeflOpt, 1,914 rows, against the `c2678492…` journal | Default identical on all 957 rows. Max 242 smaller, 701 identical, 14 larger; net −11,602 bytes / −92,826 bits; Max runtime 9,634.1 → 9,596.9 s. The same four strict misses remain; the relaxed audit, included in this run, shows all four reach parity. No Max row is worse than Default |
| Defluff, 66 pairs | Unchanged: 61 wins and five ties, −109 bytes / −932 bits |

### Rows that grew

Ten of the eleven larger deft4j rows are not regressions of this change. Quiet
paired runs of HEAD and the accepted build, two repeats each at the recorded
allowance, tie or favour the accepted build on all ten, so most journal losses
were benchmark-load variance. Three of them, the `pkmn` bit losses of 1, 2
and 5 bits, are deterministic and finish in about a second, so more time
cannot change them. They were attributed here to R1b; the clean bisect on
9 October found R1b, R1c and a timing-dependent floor instead (see the
9 October section). They cost no bytes.

`oxipng/grayscale_alpha_8_should_be_grayscale_alpha_8.png` is a real,
deterministic loss at 10 seconds: 80,094 → 80,637 bytes. At 60 seconds HEAD
and the accepted build both reach 80,014. The PNG early lineage's quick floor
now ends with R1c, and its `Established` refinement starts from that slid
parent. From the unslid parent the floor-seeded route finds nothing, so the
lineage continues through deft4j-derived refinement and reaches 80,031 bytes
of Deflate. From the slid parent floor-seeded finds 46 bytes first; the lineage
continues it instead and reaches only 80,574.

A `CompleteParent` policy that omitted R1c from that quick floor restored the
file but was rejected. Over 131 paired cases (every larger row, the 40 largest
wins, 40 further wins and 40 unchanged rows) it was 9 smaller and 20 larger,
net +3,156 bytes. `oxipng/interlaced_odd_width.png` lost 2,452 bytes and
`medium/FsqwhPuaIAIlojU.png` 1,013. On the first, both parents continue
through deft4j-derived refinement, but the slid parent's deft4j-derived source
starts 6,554 bytes smaller. Which parent leads to the better basin is not
predictable from its size, so the loss is recorded rather than fitted.

The DeflOpt refresh shows the same two kinds. Of its 14 larger Max rows,
`grayscale_alpha_8` (+543 bytes) and `css-ig-net/Apricot512.png` (+64) are
deterministic parent-basin losses. Apricot512's early lineage now refines a
slid 300,389-byte quick floor instead of the unslid 300,470-byte one, and the
same deft4j-derived refinement ends 179 bytes larger. Quiet paired runs tie or
favour the accepted build on the other twelve.

After the refresh the deft4j runner rotated four of these rows into the
hundred-file guard, `grayscale_alpha_8` and the three `pkmn` files, at their
earlier floors. The accepted build misses those four floors for the reasons
above.

## Strict distance completion on 30 September 2026

### Reference misses

The remaining reference misses are the four DeflOpt rows (two files) and 13
timed-deft4j rows. One deft4j row is the signed PNG whose unknown
unsafe-to-copy chunk requires byte-identical preservation. An independent
header inspector shows that every other reference uses an RFC 1951 degenerate
distance tree that strict mode deliberately completes: a single one-bit code
(half the code space) in 13 references, or an empty alphabet in the two
`small/T_Grass.png` references.

For a singleton, strict output needs at least one more code-length symbol. In
each reference's own header that costs 2–4 bits (the code-length code for
value 1). Columbo's singleton gaps are 2–3 bits, already at or below that
floor, so they are the cost of the strict policy rather than search misses.
The empty case had slack: planning always completed it as `[1, 1]`, whose
code-length symbol 1 sits at position 18 of RFC 1951's code-length order and
forced HCLEN from 14 to 18.

### Accepted change

The new terminal step R1b re-prices each finished strict block whose tokens
use at most one distance symbol with four uniform complete distance trees
(depths 1–4). Any complete code covering the used symbol decodes the same
payload, so the choice is exact header cost plus the used symbol's payload
change. See [routes and methods](routes-and-methods.md) for its gates.

The first placement added the same candidates to the central dynamic planner.
It produced the same wins but, because that planner prices every trial block,
median CPU time on affected small files rose about 2.4× (`T_Grass`
0.086 → 0.207 s). The accepted terminal placement prices each finished
degenerate block once.

### Evidence

Measurements used paired concurrent runs of the pre-change and candidate
executables, compared CPU time rather than wall time, and ran timed Max
trials only when no competing workload was active.

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

Every route remains reachable. `my-computer-on-fs8.png` is not a code
regression: the candidate reaches the published 13,658 bytes in three of three
10-second runs, R1b does not run on it, and the pre-change executable also
gives 13,661 bytes at 180 seconds. It is a time-nonmonotonic case, where a
longer allowance lets a different lineage win an intermediate comparison. It
is recorded, not fitted with a file-specific rule.

The complete DeflOpt, deft4j and APNG journals were not rerun with this
executable; their public reports remain the earlier complete results. Private
paired records and harness scripts are under `work/strict-completion-20260930/`.

## Bounded APNG Default scheduling follow-up on 26 September 2026

The complete 2,431-case APNG Max and Default journals use executable
`e3325451…`. Max has no deft4j misses or Default-quality failures. Default
has four speed-gate flags. Fresh serial controls show that two are run-to-run
timing variation: Default finishes before Max when retried. The other two
recur at 34.22 versus 24.16 seconds and 53.61 versus 40.22 seconds. They
contain 240 and 100 unique image streams respectively; Default scheduled
them serially while bounded Max used independent image-worker lanes.

The accepted change schedules bounded multi-image Default work on up to the
smaller of eight and the available CPU count worker lanes when at least two
CPUs are available. Each lane processes a small-to-large slice. The serial
scheduler's source-weighted timeout formula and the `ApngDefault` search
policy remain;
the complete Default sibling raced by bounded Max uses the same schedule, so
Max still has a full-file Default quality floor. Existing 8 MiB compressed
and 64 MiB decoded work bounds, duplicate-frame grouping, validation,
decoded-byte budget, and serial fallback continue to apply. This gate follows
independent stream structure and bounded work, not any corpus filename or
observed benchmark result. The final executable is `9b0b6fa3…`.

At the four original flags, fresh Default runs on that executable finish in
2.64, 4.90, 9.80, and 34.51 seconds, all before their respective Max runs.
They reproduce the frozen build's output byte and meaningful-bit counts.
Across these same four paired Max runs, the candidate loses 90 bytes / 711
meaningful bits against fresh frozen controls while Max time is essentially
unchanged (93.07 → 93.15 seconds). A separate eight-file, timing-stratified
paired Max holdout loses a net 62 bytes / 502 bits (204.01 → 203.14 seconds),
with two improvements and six losses. All 12 candidate Max outputs remain no
worse than Default in both metrics and beat their deft4j references. These
small timed-search losses are part of the tradeoff, not hidden as ties.

The 29-file Default holdout selects five files from each runtime quintile
with a fixed seed, then includes the four speed-gate files. A paired trial of
the frozen build against a code-equivalent standalone Default prototype
produced identical bytes and bits on every file and reduced total observed
runtime from 279.61 to 99.07 seconds; all 29 candidate runs were faster.
The final executable also gives identical bytes and bits on all 29, with
80.95 seconds observed total versus those frozen controls' 279.61 seconds;
all 29 remain faster. The two candidate timing totals were recorded in
separate runs, so their difference is timing variation rather than a claimed
second scheduling improvement. These results are kept separately from the
complete 2,431-row journals. The full APNG corpus has **not** been rerun on
`9b0b6fa3…`, so no full-corpus runtime or compression claim is inferred from
these samples.

All 30 APNG-named cases in the regular timed deft4j cohort were also rerun at
their recorded allowances on `9b0b6fa3…` against the earlier `e3325451…`
rows. Exactly 29 have APNG frames. Twenty-four improve, four tie, and two
lose in file bytes, for a net 494 bytes / 4,057 meaningful bits saved. There
are no deft4j reference misses. Observed Max time is 592.48 → 589.70
seconds across different runs, so that small difference is not an isolated
speedup measurement. The remaining 1,592 regular deft4j sources cannot
enter the changed multi-image schedule; the full 1,621-row corpus was not
rerun on this executable.

The historical `f287245e…` APNG Max state and complete `e3325451…` Max state
overlap on 2,377 successful cases: 2,331 improve, one ties, and 45 lose in
file bytes, for a net 1,057,014 bytes / 8,456,569 meaningful bits saved.
Gross losses total 9,586 bytes. Their recorded total times are 53,886.53
and 49,329.72 seconds respectively, from different runs. On the largest
historical Max loss, a fresh 36-second old/current comparison yields
1,233,270 / 1,234,928 bytes. Giving the current frozen build 144 seconds
instead yields 1,231,247 bytes / 9,799,916 bits, beating its old historical
target by 1,669 bytes / 13,376 bits. This witness shows that the route is
reachable with more time in that case; it does not erase the normal-allowance
loss or prove that every historical target is reachable.

The prior DeflOpt, Defluff, and 100-file guard audits below remain the
applicable full-corpus evidence. None of their sources has APNG `fdAT` frames,
so none can enter this new multi-image schedule. The regular deft4j cohort
has the separately rerun framed APNG sources above. The complete Steam
stickers APNG benchmark reports still describe `e3325451…`; private paired
trials, extended run, and output comparisons are under
`work/apng-goal-20260926/`. Debug and release Rust suites,
`cargo fmt --check`, and a verbose-versus-quiet APNG output identity check
pass on the final source and executable.

## APNG terminal-share follow-up on 22 September 2026

The five stronger historical Columbo targets were first retried on executable
`64dad99f…` with larger allowances, without changing its source. The two
DeflOpt targets still miss: `briefcase.png` is +1 byte / +8 bits after
180 seconds, and `09-ct-c6-c4.png` is +2 bits after 180 seconds. The former
chooses a different subset of source-certified four-byte matches; the latter
moves a two-block boundary by 149 decoded bytes and changes both blocks'
tokens. A header-only patch cannot recover either output. `FsqwhPuaIAIlojU.png`
does recover at 360 seconds, finishing 21 bytes / 165 bits below its older
target. Its older and normal-current outputs have 50 and 52 blocks respectively.

The first older APNG target recovers at 180 seconds on `64dad99f…`, finishing
9 bytes / 64 bits below the target. Its repeated frame is 25 bits smaller at
each of four copies than in the 60-second output. The second older APNG target
remains +2 bytes / +14 bits at 180 seconds, byte-identical to its 60-second
output. Its historical winning frame splits after 32,101 decoded bytes;
the current parent splits after 36,627. The older `f287245e…` executable
reproduces both historical APNG targets. Its first output predates Columbo's
smallest-sufficient zlib-window normalization and retains one unnecessarily
wide frame window. That wrapper choice does not change its Deflate bit count
or explain the missing compression choice.

The diagnosed APNG scheduling gap has a format-independent time basis within
each child stream: a long primary route can finish on a useful parent with no
time left for short terminal header passes. For eligible `ApngMax` children,
the working-tree change reserves the last 1/20 of the child's assigned soft
allowance for terminal methods and leaves 19/20 for primary routes. It uses
the existing ≤1 MiB compressed/decoded and ≤128-source-block Max work class,
adds no worker or new file-wide allowance, and retains the complete Default
file floor. Candidate executable SHA-256 is
`e3325451aac96fdc66b313e648898a31a9f8e076352b13cde94d52d42d56d8f9`.

Both APNG targets improve at their original 60-second allowances:

| Source prefix | `64dad99f…` bytes / bits | Candidate bytes / bits | Candidate vs older target |
| --- | ---: | ---: | ---: |
| `2313020_361766…` | 65,394 / 517,985 | 65,330 / 517,483 | −60 bytes / −463 bits |
| `657730_102978…` | 172,311 / 1,371,475 | 172,231 / 1,370,827 | −78 bytes / −634 bits |

Fresh serial controls reproduced those byte and bit counts for both executables
at the same allowances. In those controls, candidate wall times were 55.82
versus 64.44 seconds and 60.22 versus 64.51 seconds. One deliberately checked
short-allowance loss, `apng-medium/Dharma_Wheelmmm-APNG-animation2.png`,
measured 76,422 / 600,269 at 12.54 seconds on `64dad99f…` and
76,443 / 600,426 at 10.04 seconds on the candidate. The reservation can
therefore trade some primary search quality for earlier terminal work; the
individual loss is retained, not hidden by the aggregate.
The candidate's Max outputs also dominate fresh Default outputs of
65,550 / 519,250 and 172,566 / 1,373,519 bytes / bits respectively.

All 30 APNG-named cases in the regular timed deft4j journal were rerun at their
recorded allowances with output identity, PNG validity and zlib-window checks.
Against the earlier complete `64dad99f…` journal, 26 improve, two tie and two
lose in file bytes; 25 improve, two tie and three lose in meaningful bits.
The net is 2,866 bytes / 22,867 bits saved, with observed total runtime
661.23 → 592.48 seconds. Those runtime observations are from different full
cohort runs, not a controlled speedup measurement. Exactly 29 of the 30
sources contain APNG frames. A source inspection found no APNG frames among
the other 1,592 timed deft4j sources, any DeflOpt source, or any of the 100
canonical guard sources, so those sources cannot enter the changed `ApngMax`
branch. The full 1,621-row deft4j corpus was not rerun on the candidate.

Debug and release Rust tests and `cargo fmt --check` pass on the candidate. The 17
strict reference misses remain policy cases: 13 references use singleton
distance trees, three use empty distance alphabets, and the signed PNG must
stay byte-identical. No strictness rule or Default route changed. Defluff's
61 wins and five ties remain outside this APNG-only change. The private
block/token comparisons, extended trials, candidate rows and paired controls
are under `work/miss-regression-20260922/`.

## Refresh on 22 September 2026

The current checkout retains the accepted Max header admission rule described
below. Subsequent changes accelerated Huffman construction, same-distance
partitioning and original-match restoration without changing their stated
search limits. All three newly completed benchmark journals use executable
SHA-256 `64dad99f0de94bcdbaf9b00fe2a4351b0043f468437ee68736cec98a4431c34c`.
The 89 source files match the validated build's source manifest, and all 1,640
inventoried benchmark sources match their 15 September hashes. Reference names,
bytes and meaningful-bit counts also match the frozen complete journals.

### Complete current journals

The table compares the current executable with the earlier complete
`6c601870…` executable. A negative size delta means the current result is
smaller. DeflOpt's Max allowance depends on measured Default time; 36 of its
957 allowances changed. The same-allowance subset separates those rows.

| Cohort | Improvements / ties / losses | Net bytes / meaningful bits | Earlier → current measured runtime |
| --- | ---: | ---: | ---: |
| DeflOpt Default, 957 pairs | 0 / 957 / 0 | 0 / 0 | 1,395.78 → 1,270.97 s |
| DeflOpt Max, 957 pairs | 147 / 776 / 34 | −7,335 / −58,679 | 9,752.64 → 9,690.18 s |
| DeflOpt Max, 921 pairs at identical allowances | 130 / 760 / 31 | −7,525 / −60,198 | 9,056.78 → 9,055.83 s |
| Timed deft4j, 1,621 pairs at identical allowances | 204 / 1,390 / 27 | −2,104 / −16,765 | 17,720.46 → 17,721.63 s |
| Defluff, 66 pairs | 0 / 66 / 0 | 0 / 0 | 13.38 → 8.12 s |

Default's output byte/bit counts are identical throughout. Against the earlier
build, DeflOpt Max trades 8,670 bytes / 69,348 bits of gross gains against
1,335 bytes / 10,669 bits of losses; deft4j trades 2,455 bytes / 19,576 bits
of gains against 351 bytes / 2,811 bits of losses. Defluff still has 61 wins
and five ties against its reference, saving 109 bytes / 932 bits. Runtimes are
observations from different complete runs, not isolated speedup measurements.
The benchmark policies and overlapping sources prevent adding these totals as
one independent corpus.

Within the previously inventoried 138-file static PNG Max class, the current
build has 119 improvements, 16 ties and three losses against `6c601870…`,
saving 1,851 bytes / 14,782 bits. The 122 rows at identical allowances save
1,676 bytes / 13,381 bits. The 69 additional policy-matched cases have 38
improvements, 27 ties and four losses, saving 443 bytes / 3,581 bits; 68 retain
identical allowances. These are overlapping selections from the complete
journals, not additions to the table above. They support the general Max work
class without replacing its [original admission proof](research/terminal-header-work-class-validation.md).
Measured time is 1,633.17 → 1,642.41 seconds in the 138-file class and
967.67 → 977.42 seconds in the additional 69 cases; allowances differ for
some rows as described above.

### Strict misses and hundred-file guard

The same seventeen strict reference misses remain, with identical strict
byte/bit counts. Fresh `--strict 0` audits on this executable reach reference
parity or better for sixteen at their recorded allowances; the signed PNG
remains byte-identical to its source, including the unknown unsafe-to-copy
chunk. The [DeflOpt](deflopt-benchmark.md) and
[deft4j](deft4j-timed-benchmark.md) reports now attach those current-executable
audits while preserving every original strict row. Strict misses are not
relabeled as wins.

All hundred canonical guard cases have exact source, mode and allowance
coverage in the current complete journals. At normal allowances, 90 improve,
six tie and four miss their unchanged historical floors: net savings of
3,935 bytes / 31,534 bits. Against the earlier `6c601870…` full-journal
coverage of those same hundred cases, the current build saves 197 bytes /
1,573 bits, with 1,200.99 → 1,197.34 seconds measured. This small time
difference is not an isolated speedup claim. The four historical residuals
and longer-time witnesses are:

| Source | Normal-allowance gap, bytes / bits | Longer allowance and measured runtime | Longer result vs historical floor, bytes / bits |
| --- | ---: | ---: | ---: |
| `css-ig-net/sample_53.png` | +1 / +3 | 60 s / 65.65 s | −25 / −204 |
| `css-ig-net/sample_71.png` | +14 / +114 | 60 s / 65.10 s | −44 / −351 |
| `oxipng/filter_0_for_grayscale_16.png` | +15 / +121 | 60 s / 65.71 s | −59 / −467 |
| `medium/LevelLoading.png` | +7 / +52 | 250 s / 270.80 s | −1 / −8 |

Thus all hundred historical floors have witnesses on the current executable:
96 at normal settings and four after more time. The longer results do not
replace normal-run losses or runtime. The canonical guard file is unchanged.

### Timing-sensitive examples and stronger targets

Three large journal losses received fresh trials using frozen executables and
the same source/reference policy. Kiwi512's earlier 10-second journal result
is 389,163 bytes / 3,089,703 bits. Fresh 10-second runs of both executables
give 389,163 bytes / 3,089,697 bits; the current build repeats that result at
60 seconds. The worse current journal row does not reproduce as a build
difference in this pair.

For `floor pattern.png`, the old journal used 21 seconds and the new one 18.
At a fresh 21 seconds, the current executable beats the earlier executable by
48 bytes / 389 bits, although both miss the earlier journal's best result.
At 60 seconds, the current executable beats that earlier journal result by
552 bytes / 4,416 bits. A fresh 30-second pair for
`csgoChat_128_chickendance.png` likewise favors the current executable by
29 bytes / 234 bits despite its larger current journal row. Its 60-second
trial beats the earlier journal result by 204 bytes / 1,621 bits. These
examples show that a journal-to-journal loss is not automatically a causal
regression in the changed code; all original rows remain in the aggregates.

The five stronger comparable targets left open on 15 September were repeated
on the current executable at their previous longer allowances. None recovers:
Briefcase remains +1 byte / +8 bits at 60 seconds, 09 remains +2 bits at
60 seconds, Fs remains +1 byte / +5 bits at 180 seconds, and the two older
APNG cases remain +4 bytes / +39 bits and +2 bytes / +14 bits at 60 seconds.
Their combined gap is eight bytes / 68 meaningful bits. Briefcase already
passes its separate canonical guard floor. A failed finite run does not prove
that a whole-optimizer endpoint is unreachable; the prior completed-budget
probe only rules out increasing R10/R11 budgets alone for its frozen 09 parent.

The latest committed source has already passed 591 Rust tests in debug and
release, 13 Python tests, contributor checks and 50 exact-output API
comparisons, as recorded in the [efficiency review](efficiency-review.md).
No production code or route limit changed in this refresh. Private complete
comparison, paired-run, longer-time, source-hash and relaxed-audit records
are under `work/miss-regression-20260922/`.

## Refresh on 15 September 2026

All three complete journals now use source `7fa3fdf`, executable SHA-256
`6c601870adb6cb43b6f5fb1b0c5c30357bdff8aa45bc25b13b6ff631d76cfc01`:
1,914 DeflOpt rows, 1,621 timed deft4j rows and 66 Defluff pairs. The following
comparison spans the intervening changes, including the larger terminal
reservation, header-directed match response and length-symbol exchange. It
does not isolate the effect of any one method.

### Complete-journal comparison

| Cohort | Improvements / ties / losses | Net bytes / meaningful bits | Earlier → current runtime |
| --- | ---: | ---: | ---: |
| DeflOpt Default, 957 pairs | 0 / 957 / 0 | 0 / 0 | 1,241.92 → 1,395.78 s |
| DeflOpt Max, 957 pairs | 117 / 766 / 74 | +5,416 / +43,326 | 9,731.27 → 9,752.64 s |
| DeflOpt Max, 924 pairs at identical allowances | 104 / 755 / 65 | +5,515 / +44,093 | 9,136.95 → 9,085.84 s |
| Timed deft4j, 1,621 pairs at identical allowances | 613 / 957 / 51 | −3,606 / −29,089 | 20,938.39 → 17,720.46 s |
| Defluff, 66 pairs | 0 / 66 / 0 | 0 / 0 | 8.448 → 13.376 s |

The preceding DeflOpt and Defluff journals use `4d4e1586…`; the preceding
deft4j journal uses `e3bd1f72…`, as recorded in the 13 September refresh.
Thirty-three DeflOpt Max allowances changed because they are derived from
measured Default runtime. Default byte/bit counts match throughout; this is
not a byte-identity claim. The deft4j aggregate runtime falls by 15.4% in these
observations. Corpus overlap and different runner policies prevent adding
the benchmark totals as independent savings.

Against their respective reference programs, current DeflOpt Default saves
948,923 bytes / 7,564,667 bits, DeflOpt Max saves 1,112,748 bytes / 8,875,321 bits,
and timed deft4j saves 831,863 bytes / 6,655,089 bits. Defluff remains at 61 wins
and five ties, saving 109 bytes / 932 bits. There are no validation errors or
Max-over-Default failures in the complete journals.

### Reference misses and the unchanged guard

All seventeen strict reference misses reproduce on this executable. Sixteen
reach parity or better with `--strict 0` at the same allowance. Reference-file
hashes match the independent header audit: thirteen use singleton distance
alphabets and three use empty distance alphabets. The signed PNG retains the
unknown unsafe-to-copy `caBX` chunk and is emitted byte-identically. Four
DeflOpt and thirteen deft4j relaxed audits are attached to the complete
journals without changing their recorded strict measurements; strict misses
remain visible.

All hundred historical guard cases have exact source/mode/allowance coverage
in the complete current journals. Cross-runner reuse is restricted to PNG;
ZIP metadata policies remain separate. Against the unchanged historical
floors, 89 improve, seven tie and four lose, for net savings of 3,738 bytes /
29,961 bits. The ordinary-allowance residuals are:

| Source | Allowance | Residual bytes / meaningful bits |
| --- | ---: | ---: |
| `medium/LevelLoading.png` | 10 s | +7 / +52 |
| `oxipng/interlaced_grayscale_16_should_be_grayscale_16.png` | 10 s | +6 / +48 |
| `css-ig-net/sample_71.png` | 12 s | +52 / +420 |
| `oxipng/filter_0_for_grayscale_16.png` | 10 s | +19 / +153 |

No guard floor is weakened. The canonical guard SHA-256 remains
`50cad1ce6c9a0fa195ddd81d7588ee1ed5dc0255fc7172fe1f7aac984eb8a665`.

### Repeats, extra time and structural coverage

Six of the larger journal losses received fresh, alternating-order trials
of `4d4e1586…` and `6c601870…` at the recorded allowances. Their current-build
net loss is 310 bytes / 2,483 bits, with 160.76 → 161.89 seconds measured.
BNDT and Partnership beat their older journal floors in both builds; Matrix
matches its floor in both. Nerd's 5,510-byte / 44,080-bit full-journal loss
shrinks to 30 bytes / 242 bits in the fresh current trial. The original
full-journal loss remains in the table above.

Fs initially repeats a 280-byte / 2,242-bit loss only in the current build.
A further verbose trial of each frozen executable produces that same loss
in both, in approximately 24.32 seconds at a 21-second allowance. Its decoded
stream exceeds 1 MiB; it cannot enter the small terminal-header work class.
These observations establish timing sensitivity but do not isolate the
regression to an individual code change.

Twelve selected current-build cases received 60-second trials against actual,
comparable observed byte/bit floors. Five recover: Nerd, `sample_71.png`,
the two grayscale guard residuals and `interlaced_odd_width.png`. The trials
take 752.45 seconds in total. LevelLoading, Fs and the five small residuals
listed in the 13 September follow-up remain above their targets. These
longer-time trials do not replace the normal-allowance measurements.

A separate frozen-parent probe retains the original source certificates and
runs the terminal closure without a deadline. The existing method gates
leave all five small residual parents unchanged. Admitting R2–R5 to the
existing 1 MiB Max envelope, with all work, price and block-local caps held
constant, saves 15, 31, 34 and 42 meaningful bits on AlphaBall, phenix, road
and the palette parent respectively: sixteen raw bytes / 122 bits in total.
Independent decoding verifies every probe output. The 09 parent is unchanged.
This demonstrates a specific method-admission barrier on those frozen parents;
it is not a full-corpus gain or proof that other routes could never reach
their improved endpoints.

### Accepted Max header admission correction

Max now admits R2–R5 to the existing 1 MiB enclosing compressed/decoded work
class. Mandatory Default work keeps its 128 KiB envelope, R1 restoration is
unchanged, and R5 retains its 128 KiB block limit. All method budgets, menus,
phase shares, deadlines and grace remain unchanged. The accepted executable is
`11f8f7f1090466c23d235b3c1400efd94b6ed7460c76515e9bffa53cfa271385`.
A generated stored-prefix test proves that an unrelated block no longer
excludes the four bounded header problems in Max.

| Candidate comparison against `6c601870…` | Improvements / ties / losses | Net bytes / meaningful bits | Baseline → candidate runtime |
| --- | ---: | ---: | ---: |
| Complete 138-file static PNG work class | 116 / 16 / 6 | −1,641 / −13,142 | 1,633.17 → 1,666.87 s |
| 119 files outside the initial screen | 102 / 12 / 5 | −1,117 / −8,939 | 1,430.89 → 1,468.10 s |
| 69 additional policy-matched cases | 41 / 27 / 1 | −475 / −3,798 | 967.67 → 979.97 s |
| Unchanged hundred-file guard | 25 / 69 / 6 | −172 / −1,368 | 1,200.99 → 1,198.40 s |
| Nine generated raw/zlib/GZIP controls | 4 / 5 / 0 | −24 / −205 | 84.292 → 85.869 s |

The complete work-class and additional-cohort comparisons use recorded
baseline rows, while the initial screen and generated controls alternate fresh
baseline/candidate order. The initial twenty-case screen has fifteen wins,
five ties and no losses: −262 bytes / −2,101 bits, with 212.39 → 210.53 seconds.
The additional cohort follows an inventory of all 1,640 unique sources in the
two complete strict journals, including container substreams. Its ZIP and
GZIP trials tie; its gains come from PNG/APNG. All five PNG families improve
in aggregate. Overlapping trials and preservation policies prevent adding
these cohort totals as independent corpus savings.

The full PNG class trades 1,856 bytes / 14,856 bits of gross wins against
215 bytes / 1,714 bits of losses, with 2.1% more measured runtime. The
additional cohort uses 1.3% more time. These costs and all losses are retained
in the acceptance decision. All 197 live Default comparisons across the
overlapping cohorts match baseline byte/bit counts; all Max-over-Default
checks pass, and no new reference miss appears.

Against the historical guard floors, the candidate has 89 improvements,
six ties and five losses: −3,910 bytes / −31,329 bits. All hundred floors
have witnesses on this candidate: 95 at their original settings, four at
60 seconds, and LevelLoading at 250 seconds. The last improves its historical
floor by one byte / eight bits in 270.82 seconds. These witnesses do not
replace the normal 95/100 result or its runtime.

Twenty-two selected longer-time trials recover seventeen stronger observed
targets in 1,745.54 seconds. All six PNG-census losses have recovery witnesses,
as does the additional cohort's APNG loss. Nerd matches its stronger baseline
60-second result. Fs's 280-byte / 2,242-bit gap shrinks to one byte / five bits
at 180 seconds, taking 195.16 seconds. Five residual targets remain, totalling
eight bytes / 68 bits: Briefcase (+1 byte / +8 bits), 09 (+2 bits), Fs
(+1 byte / +5 bits), and the two older APNG cases (+4 bytes / +39 bits and
+2 bytes / +14 bits).

Fresh original-allowance comparisons do not isolate the Briefcase or Fs gaps
to this patch. At ten seconds, both `6c601870…` and the candidate miss
Briefcase's older floor by one byte / eight bits. At Fs's older twenty-second
setting, both `4d4e1586…` and the candidate miss the old floor; the candidate
is 53 bytes / 424 bits smaller in that fresh pair. Original corpus results
are not replaced with better repeats.

Widening the two response methods' block eligibility and raising their budgets
through 16× yields no further gain on the five frozen parents. A focused 09
trial then completes those finite menus with effectively nonbinding budgets
and still emits identical parent bytes. Simply increasing R10/R11 budgets
does not recover that parent's two-bit gap. Other method limits, heuristic
menus and parent choices remain possible constraints; this is not proof of
global optimality or of an unreachable whole-optimizer endpoint.

The candidate's seventeen strict reference misses remain unchanged: sixteen
reach relaxed-policy parity and the signed PNG is byte-identical. Its full
Defluff replay gives 61 wins and five ties, saving 109 bytes / 932 bits in
7.634 seconds. All 585 Rust and 202 Python tests pass, along with formatting,
Clippy, locked builds, package checks and independent output validation.
Final source, executable, fixture and guard hashes are verified.

The [route catalogue](routes-and-methods.md) and
[complete validation record](research/terminal-header-work-class-validation.md)
record the accepted rule, theoretical basis, selection, runtime costs,
remaining limits and recovery witnesses. Private artifacts are under
`work/miss-regression-20260915/`. Complete public journals retain their
`6c601870…` identities and original measurements.

## Refresh on 13 September 2026

The audited pre-extension source is `c2fce04`, executable SHA-256
`4d4e1586a0bf877b84d1bc8ff015356b07d616197fa1cdb42e53b5ad7027dfea`.
The complete DeflOpt and Defluff journals were rerun with this executable on
12 September. The complete 1,621-row deft4j journal still belongs to executable
`e3bd1f722b68cd8f3892a90f2f78473c29bde815cddb76eacde273c65af73364`;
its thirteen misses were freshly checked, without replacing that complete
journal with a partial run or relabelling its other rows.
The complete-journal comparisons and initial confirmations below describe
that pre-extension build; the accepted extension has its own validation below.

### Complete-journal comparison

The earlier complete DeflOpt journal used executable
`c3f63f77748dcca26756f5fd2fa67d3a9598e45cd7a9a7cdd3917ac54ad832ae`.
Comparison with the new complete journal gives:

| Cohort | Improvements / ties / losses | Net bytes / meaningful bits | Earlier → newer runtime |
| --- | ---: | ---: | ---: |
| Default, all 957 pairs | 0 / 957 / 0 | 0 / 0 | 1,222.76 → 1,241.92 s |
| Max, all 957 pairs | 498 / 443 / 16 | −6,320 / −50,674 | 11,519.45 → 9,731.27 s |
| Max, 945 pairs at identical allowances | 491 / 438 / 16 | −6,114 / −49,022 | 11,283.89 → 9,494.49 s |

Twelve Max allowances changed because they are derived from measured Default
runtime. The matched-allowance subset saves 6,114 bytes / 49,022 bits while
using 15.9% less aggregate time. These are complete-journal observations
spanning intervening commits, not an isolated measurement of one patch.
Default sizes and meaningful-bit counts match throughout; that does not by
itself establish byte-for-byte identity of the emitted files.

The full Max comparison has gross wins of 6,969 bytes / 55,884 bits and gross
losses of 649 bytes / 5,210 bits. All 957 current Max/Default comparisons pass.
Against DeflOpt itself, Default saves 948,923 bytes / 7,564,667 bits and Max
saves 1,118,164 bytes / 8,918,647 bits. The modes overlap and must not be added
as separate corpus savings. The current complete Defluff result remains
61 wins and five ties, saving 109 bytes / 932 bits in 8.45 seconds, with no
validation error.

### Misses and historical floors

All seventeen strict reference misses reproduce on the audited executable.
Sixteen reach parity or better with `--strict 0` at the same allowance.
Independent reference-header inspection again finds thirteen singleton and
three empty distance alphabets. The remaining signed PNG contains the unknown
unsafe-to-copy `caBX` chunk; its emitted source is byte-identical. The four
DeflOpt relaxed audits are attached to the current journal without changing
any recorded strict size, bit count or runtime. Strict misses remain labelled
as strict misses even when the policy difference is explained.

All hundred unchanged historical guard floors are covered by this executable:
85 source/mode/allowance matches in the complete DeflOpt journal and fifteen
additional targeted replays. Cross-runner reuse is restricted to PNG, whose
CLI metadata policy agrees; deft4j ZIP cases receive their own trials because
the DeflOpt ZIP runner strips metadata. There are 89 improvements, nine ties
and two losses, for net savings of 3,947 bytes / 31,606 bits. The residuals are
again `medium/LevelLoading.png` at +6 bytes / +49 bits and
`css-ig-net/sample_53.png` at +1 byte / +3 bits, both at ten seconds. Aggregate
trial time is 1,199.04 seconds; these historical floors have no comparable
aggregate runtime. No guard floor was weakened.

### Confirmation of journal losses

All sixteen new full-journal Max losses received fresh trials with both the
frozen `2e77f21` pre-fix executable and the audited executable at the recorded
allowance. Ball, Kiwi, Mango and BNDT recover their old journal floors in the
new-build repeat. Motorcycle beats its older floor by 16 bytes / 127 bits.
Other repeats vary: `file09.png`, for example, changes from +1 byte / +5 bits
in the full journal to +80 bytes / +637 bits in the fresh new-build trial.
The frozen pre-fix executable also misses several old journal floors.

On this deliberately loss-selected subset, the new build loses a net 48 bytes /
390 bits against the fresh pre-fix trials. That result is retained alongside
the full-corpus gain; confirmation and longer-time results do not replace
original corpus rows with best-of-repeat scores. An endpoint reached at the
same allowance is demonstrably reachable even when a different timed run
misses it. Conversely, failure in a finite longer run cannot prove that a
route is structurally unreachable.

### Accepted extension to the existing Max work class

The terminal reservation now covers sources whose compressed and decoded
sizes are each at most 1 MiB, reusing the existing R6–R9 work class instead of
the narrower 128 KiB R1–R5 class. This closes a scheduling gap: already-admitted
terminal methods could otherwise receive no optional time. Stream ownership,
the 128-source-block limit, the 4/5 primary share, method budgets and original
deadline/grace remain unchanged. R1–R5 retain their 128 KiB method gates.
The production executable is
`cd2e432fad4d2672d27e1adb4fdad36658c9a98a7dfedae53c67b7242e1e81d3`.

Across all 138 static PNG sources in the expanded size class, at the recorded
pre-extension allowances, the candidate has 85 wins, 30 ties and 23 losses:
**−1,009 bytes / −8,114 meaningful bits**, with measured runtime changing from
1,699.48 to 1,597.55 seconds (−6.0%). The 123 sources outside the initial screen
save 341 bytes / 2,772 bits. This complete-class comparison uses the recorded
pre-extension journal, not interleaved fresh pairs. All 138 live Max-over-Default
checks pass, Default byte/bit counts match, and no new DeflOpt miss appears.
Nine generated raw/zlib/GZIP controls give two wins and seven ties; a generated
two-job APNG under the unchanged shared policy emits byte-identical output.

The unchanged hundred-file guard records 90 wins, seven ties and three losses
against its historical floors: −3,821 bytes / −30,617 bits in 1,195.35 seconds.
Against the pre-extension guard results, however, it loses a net **126 bytes /
989 bits**. The expanded class's oxipng family also loses 82 bytes / 629 bits.
These adverse results are retained in the acceptance decision. The overlapping
cohorts must not be added, and guard floors are not weakened.

Of 32 targeted 60-second follow-ups against comparable observed floors,
26 recover. All hundred historical guard floors have witnesses on this
candidate: 97 at their recorded settings, `sample_53.png` and `sample_71.png`
at 60 seconds, and LevelLoading at 250 seconds. The last beats its historical
floor by one byte / eight bits in 270.79 seconds. These expensive recoveries
do not replace the normal-allowance results. Five other small gaps remain
against the follow-up floors, totalling six bytes / 45 bits; the largest,
three bytes / 20 bits, persists at 180 seconds. The earlier two APNG residuals
remain separately dated evidence, not fresh trials of this executable.

All seventeen strict reference misses still reproduce, sixteen reach parity
or better under the relaxed policy, and the signed PNG retains its preservation
difference. The full 66-pair Defluff replay again gives 61 wins and five ties,
saving 109 bytes / 932 bits in 7.535 seconds. All 571 Rust and 202 Python tests
pass, along with Clippy, formatting and independent output validation.
The [route catalogue](routes-and-methods.md) and
[full validation record](research/terminal-closure-validation.md#follow-up-on-13-september-2026-the-larger-max-work-class)
document the accepted rule, selection, tradeoffs and remaining limits.
Complete public journals keep their earlier executable identities.

### More time and method coverage

Increasing the allowance lengthens both primary and terminal phases; it does
not expand method admission, candidate menus or per-invocation work budgets.
For example, R1–R5 still exclude streams above their 128 KiB compressed/decoded
class, and R6–R9 exclude streams above 1 MiB, regardless of timeout. A longer
run can nevertheless improve such a file through other admitted routes.

A successful longer-time or same-time repeat proves that its particular
endpoint remains reachable. An unsuccessful finite run proves neither global
optimality nor structural unreachability of the endpoint. The closure proof
applies to strict descent through the bounded operators actually evaluated;
budget exhaustion and heuristic menus limit that statement. The goal of
recovering every possible bit would require additional coverage work, not
merely a larger timeout. No image dimensions, filenames or corpus-specific
thresholds enter the reservation rule.

Private snapshots, executable and source identities, guard coverage joins,
retained outputs and current reference-header inspection are under
`work/miss-regression-20260913/`.

## Refresh on 11 September 2026

The fresh baseline is source `2e77f21`, executable SHA-256
`a3d5c69d3ab60d7c319313abfe4d116e11b81bbe8994b1a2ed9d0b32dcaea336`.
The complete journals below retain their own executable identities; this
refresh does not relabel them as fresh full-corpus runs.

- All 17 published DeflOpt/deft4j miss rows reproduce. Independent header
  inspection finds singleton distance alphabets in thirteen reference rows
  and empty distance alphabets in three. All sixteen reach parity or better
  with `--strict 0` at the same allowance. The remaining row is
  `oxipng/c2pa-signed.png`, which preserves the unknown unsafe-to-copy `caBX`
  chunk. These are compatibility or preservation-policy differences.
- Fresh full Defluff replay: 66/66 pass, with 61 wins and five exact ties;
  net savings are 109 bytes and 932 meaningful bits.
- Fresh rolling guard: 99/100 historical floors pass, comprising 85 wins,
  14 exact ties and one residual. Net savings are 3,645 bytes and 29,184 bits.
  All 53 live Max/Default comparisons pass, and no validation error occurs.
  The 100 trials take 1,308.90 seconds, plus 60.13 seconds for their live
  Default comparisons. Historical guard floors have no comparable aggregate
  runtime, so those savings are not presented as a measured speed improvement.
- Joining the same 100 trials to the newer complete journals at identical
  allowances gives a net 68-byte / 527-bit improvement. Trial time changes
  from 1,306.81 to 1,308.90 seconds. Four small timed losses remain against
  those newer endpoints: three bit-only losses totalling four bits and one
  one-byte / fourteen-bit loss. These are separate from the older guard floors.

The one guard residual, `medium/LevelLoading.png`, loses six bytes / 49 bits
at ten seconds. Sixty seconds produces the same result. At 180 seconds it
matches the historical byte floor and improves it by five meaningful bits;
measured runtime is 195.87 seconds, within that allowance's active-route grace.
This endpoint remains reachable. Recovering it does not justify imposing that
runtime on every ordinary invocation.

The completed DeflOpt journal also gives the broader runtime context:
Default saves 948,923 bytes / 7,564,667 bits against the reference in 1,222.76
seconds; Max saves 1,111,844 bytes / 8,867,973 bits in 11,519.45 seconds.
Thus Max saves another 162,921 bytes / 1,303,306 bits for about 9.4 times the
aggregate Default runtime. These overlapping mode totals must not be added
as independent corpus savings.

The [terminal scheduling and closure correction](research/terminal-closure-validation.md)
is accepted after matched validation. Its hundred-file replay improves on this
fresh baseline by **315 bytes / 2,527 bits**, with trial time changing from
**1,308.90 to 1,200.59 seconds** (−8.3%). It produces 45 improvements, 52 ties
and three losses against baseline; all 53 live Max-over-Default checks pass.
Against the older guard floors, 89 improve, nine tie and two remain larger:
`medium/LevelLoading.png` at +6 bytes / +49 bits and `css-ig-net/sample_53.png`
at +1 byte / +3 bits. Net savings against those floors are 3,960 bytes /
31,711 bits. No floor was weakened, and no validation error occurred.

All four smaller regressions against the newer complete journals now recover.
The three losses against the fresh baseline total 20 bytes / 156 bits, against
gross wins of 335 bytes / 2,683 bits. The final candidate also passes all 66
Defluff pairs with the same 109-byte / 932-bit net saving as baseline. These
cohorts overlap and must not be added together. This is a matched protective
cohort, not a fresh complete DeflOpt/deft4j journal or a general speed estimate.

All hundred historical floors remain reachable in the final code: 98 pass at
their recorded allowances, `sample_53.png` recovers at 60 seconds, and
LevelLoading recovers at 180 seconds (matching bytes and improving its old
floor by five bits in 195.92 seconds). The two material fresh-baseline losses,
`sample_38-fs8.png` and `sample_53.png`, both beat that baseline at 60 seconds.
The one-byte / six-bit `motorcycle.png` loss persists at 60 and 180 seconds,
but a separate ten-second confirmation beats baseline by 16 bytes / 127 bits.
This is evidence of timed search-order sensitivity, not an unreachable better
encoding. The original matched cohort is not replaced with best-of-repeat or
longer-time scores.

The two older APNG residuals retain exactly the fresh baseline bytes at
20 seconds and remain +4 bytes / +39 bits and +2 bytes / +14 bits at 60 seconds.
Those shared-frame jobs do not receive the new terminal reservation. They
remain measured tradeoffs; these trials do not prove structural unreachability.

Validation passes: 571 Rust tests including ten opt-in local-corpus tests,
202 Python tests, Clippy, formatting, whitespace and independent decoder checks.
All 404 paired Default outputs are identical. Twenty raw Max controls add
13 wins and seven ties, with no losses; independent generated controls also
show no loss. The method-specific search bounds remain, so this is a corpus
improvement rather than a proof of globally optimal Deflate output.

The private checkpoints, retained outputs, executable identities, and repeat
and longer-time trials are under `work/miss-regression-20260911/`.

## Historical evidence through 8 September 2026

The following sections describe the preceding routing audit and its recorded
binaries. Their candidate hashes, counts and residuals are historical; the
refresh above records the newly reproduced results.

## Evidence sets

| Evidence | Coverage | Executable SHA-256 | Finding |
| --- | ---: | --- | --- |
| DeflOpt last complete journal | 1,914 rows / 957 pairs | `a700a360…` | six strict-policy rows representing three files; no errors; Max never worse than Default |
| Candidate family sample | 68 rows / 34 pairs | `cef3db1d…` | two rows for one strict-policy file; no route miss or error; Max never worse than Default |
| Defluff candidate journal | all 66 pairs | `4eaf3355…` | no miss or error |
| timed deft4j last complete journal | 1,621 rows | `a700a360…` | twelve strict-policy rows and one safe PNG-preservation row; no errors |
| targeted deft4j recovery | 2 affected files, 3 runs each | `cef3db1d…` | both former 9-byte regressions recover their exact older byte and bit floors |
| APNG doubled-time regression pass | 70 prior misses | `c5036306…` | 68 strict byte-and-bit recoveries; two accepted route trade-offs; no errors; net 1,800 bytes smaller than the previous binary |
| rolling priority guard | 100 unique files | `c5036306…` | 97 floors pass; no new failure; Max never trails current Default |

The integrated candidate is `target/release/columbo`, SHA-256
`c50363060e07b258667a8049db8ab133f011214a1897c2d96da548a450aea825`.
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

## Earlier candidate movement

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
- The latest complete Defluff comparison has 58 wins and eight ties, for a net
  saving of 108 bytes / 918 meaningful Deflate bits.
- The two deft4j rows directly affected by the scheduling change recover 18
  bytes and 140 bits in total.

Aggregate sampled Default time moved from 91.70 to 98.82 seconds and Max time
from 407.70 to 409.54 seconds. A single timed sample cannot establish a speed
change; importantly, the branch is bounded to compact graphs and does not run
on the two sampled Max rows with smaller prior timed endpoints.

## Hundred-file priority guard

`work/regression-guard.json` is authoritative. It retains the latest 100
unique `(format, source)` identities, and insertion deduplicates a recurring
file. The integrated run performed 106 serial trials including confirmation
reruns: 97 historical floors pass at their recorded allowance, three inherited
residual files remain, and Max never trails the Default result produced by the
same executable. A previously inherited one-bit residual for
`oxipng/palette_should_be_reduced_with_missing.png` now passes. The five new
failures exposed by the rejected general Huffman/tree-routing experiment also
pass after retaining their established general routes.

| File | Mode | Byte loss | Bit loss | Disposition |
| --- | --- | ---: | ---: | --- |
| `medium/Nutcracker.png` | Max+5s | 1 | 7 | inherited timed route basin |
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

### Fair APNG independent roots

An APNG frame owns only the proportional wall window assigned by the outer
file scheduler. A selected floor continuation can consume that entire window,
and a direct-deft refinement can likewise leave no serial time for the
independent original-source Max root. Immediate parent size cannot prove the
order of endpoints reached by different token and boundary topologies.

Under bounded `ApngMax`, original-source Max therefore receives a concurrent
share beside either dependency. The retained complete incumbent still gates
selection, so parallel work cannot enlarge the output. Each independent root
receives positive work as the allowance grows, eliminating structural route
starvation without a filename, corpus score, or measured-runtime threshold.
The same direct-deft/source overlap applies to the existing bounded
single-image PNG policy; other shared container members retain their serial
resource schedule.

At doubled time, the integrated candidate recovers 68 of the 70 prior APNG
byte-and-bit floors and is 1,800 bytes / 14,440 meaningful bits smaller in
aggregate than the previous binary. Three directly affected starvation
witnesses improve by 7, 49, and 30 bytes. The two retained losses are general
tree-route trade-offs of 4 and 2 bytes; changing those shared methods recovered
the six APNG bytes but created five protected PNG regressions totalling 54
bytes, so that experiment was rejected.

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

The three remaining guard differences are kept visible. They total 11 bytes
and 84 meaningful bits across independent deadline-limited searches; the
largest is 6 bytes / 49 bits. No filename-specific gate, ten-second threshold,
or reference score is used to hide them. A future change must improve their
general search classes without sacrificing broader gains.

The APNG doubled-time pass retains two additional general-route trade-offs:
`2313020_361766…` is 4 bytes / 39 bits above the previous binary, and
`657730_102978…` is 2 bytes / 14 bits above it. Both remain substantially
smaller than the pre-fix baseline in aggregate context, and all 70 cases
produce a net 1,800-byte win. Restoring those two endpoints by changing
general Huffman/tree pricing caused the five larger rolling-guard regressions
above; running both complete token planners would impose a broad Max-time cost
for a six-byte local recovery.

The two smaller sampled Max endpoints that were not reproduced are outside
the changed work class and still pass both the external reference and
Max-over-Default gates. They are treated as timed search variance, not as a
reason to add an unrelated corpus-specific scheduling rule.

## Verification

- `cargo test --locked`: 507 tests passed (448 library, 46 CLI, 13 public API).
- `cargo fmt --check`: passed.
- `cargo clippy --locked --all-targets -- -D warnings`: passed.
- Latest full Defluff replay (`4eaf3355…`): 66/66 pass with no errors.
- Candidate DeflOpt family sample: 68 rows / 34 pairs; one strict-policy file,
  no route miss or error, and Max never worse than Default.
- The two targeted deft4j regressions recover their exact historical byte and
  bit floors in three candidate runs each.
- Matched sample: all 34 Default outputs are identical; 31 Max outputs are
  identical; every Max output dominates its current Default result.
- Integrated APNG doubled-time pass: 68/70 strict floors recovered, both
  residuals logged, no errors, and an aggregate 1,800-byte / 14,440-bit win
  over the previous binary.
- Rolling priority guard: 97/100 floors pass across 106 serial trials, with no
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
