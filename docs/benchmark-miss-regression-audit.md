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
