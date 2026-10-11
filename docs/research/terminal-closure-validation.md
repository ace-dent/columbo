<!-- SPDX-License-Identifier: MIT -->

# Terminal time and method closure in Max

11 September 2026. Baseline: `2e77f21`. **Accepted after matched validation.**
The implementation is in `src/deflate/optimize.rs`; the R12 and G0t rows in
[routes and methods](../routes-and-methods.md) describe the current method. The
accepted [13 September extension](#follow-up-on-13-september-2026-the-larger-max-work-class)
widens the time reservation to the existing larger Max size class.

On 13 September, header-directed match response became R10, length-symbol
exchange R11, and this closure R12. The R1–R9 measurements below keep their
original scope.

## Why a single sweep misses reachable savings

The terminal methods do not commute: a later method can change the lengths,
code-length tree, support, tokens or boundaries that an earlier method priced.
A joint tree/RLE improvement can change the next iteration's header costs, and
an alphabet split can create a new, profitable joint-tree problem, so one
ordered R1–R9 sweep is not a local fixed point. Max therefore repeats the
terminal methods after a strict complete-stream improvement; Default keeps its
single sweep. Strict improvement makes the descent finite: a raw stream of at
most L bytes has at most 8L positive meaningful-bit scores.

## Giving terminal methods time

Repeating a late sweep does nothing if earlier searches use the whole
allowance: with repetition alone, all 20 frozen-parent Max results were
identical at ten seconds, with 18 timeouts in each build. The frozen-parent
gains below are therefore not evidence that repetition alone improves normal
CLI output.

The 11 September rule reserved the final fifth of the soft allowance for
terminal methods under `Complete` and `CompleteThenBounded` when the source is
at most 128 KiB compressed and decoded with at most 128 wire blocks. The
fraction reuses the existing primary/follow-up share; it was not tuned against
guard scores.

The reservation can lose a primary-search improvement against a different
schedule. The 22-file screen gained 73 bytes / 580 bits overall, with one loss
of 1 byte / 6 bits, and measured 274.89 → 244.08 seconds; the hundred-file
replay and generated inputs below test whether that holds beyond the screen.
Shared container children and larger size classes kept their earlier schedule.

## Portable regression witness

The generated coupled-swap fixture's original stream uses 37,782 bits, and one
terminal sweep stops at 36,729. The resulting alphabet splits expose another
joint-tree improvement and another useful cut; closure reaches at most 36,637
bits, at least 92 bits beyond one sweep. The test also checks that another
closure changes nothing and that an expired Max allowance keeps the ordinary
terminal endpoint. It embeds no private fixture bytes.

## Controlled exploration

The private replay used 43 distinct completed Max parents from the coupled-swap
validation. R2–R9 first received one full sweep, and only later gains are
attributed to repetition: 27/43 parents improved by **189 raw bytes / 1,485
meaningful bits**, with no larger result. These are frozen-parent gains, not
whole-corpus savings. The exploration omitted original-match restoration,
which production closure keeps.

The harness allowed eight sweeps and 20 seconds per parent. Three parents were
still improving on the eighth sweep, so the results are lower bounds;
production has no eight-sweep cutoff. All sweeps on the 43 parents took
48.654 seconds, including the first; this is not the CLI's added runtime.

## Matched hundred-file guard

All trials use the unchanged canonical hundred-file guard's recorded mode and
allowance, with the frozen baseline and candidate executables:

| Measure | Result |
| --- | ---: |
| Candidate executable SHA-256 | `644a72ce4fa85729c3dd07d6b3aaa5210c1b5250f36d30858c32b91528cdcd64` |
| Improvement / exact tie / loss against fresh baseline | 45 / 52 / 3 |
| Net change against fresh baseline | −315 bytes / −2,527 meaningful bits |
| Baseline → candidate trial time | 1,308.90 → 1,200.59 seconds (−8.3%) |
| Additional live Default comparison time | 60.13 → 60.41 seconds |
| Historical floor improvement / tie / loss | 89 / 9 / 2 |
| Net change against historical floors | −3,960 bytes / −31,711 meaningful bits |
| Live Max-over-Default comparisons | 53/53 pass |
| Validation errors | 0 |

These are serial trials on a protective cohort, not a fresh complete DeflOpt or
deft4j corpus, and the time change is one matched observation. The screen
overlaps this cohort and is not additive to it.

The three losses against the fresh baseline, all under `css-ig-net`, are
`motorcycle.png` (1 byte / 6 bits), `sample_38-fs8.png` (11 bytes / 86 bits)
and `sample_53.png` (8 bytes / 64 bits): 20 bytes / 156 bits against gross wins
of 335 bytes / 2,683 bits. Only `sample_53.png` also misses its older guard
floor, by 1 byte / 3 bits. The inherited `medium/LevelLoading.png` residual
remains 6 bytes / 49 bits, outside the new reservation class.

All four smaller losses against the newer full journals are recovered:
`sample_04-fs8.png` beats that floor by 15 bytes / 121 bits, `sample_14.png`
by 20 bytes / 160 bits, `test-convertir-truecoloralpha-trns.png` by 7 bytes /
53 bits, and `small/psydk-Pink.png` by 4 bytes / 30 bits. These are the same
trials joined to different floors, not extra savings.

Two more serial baseline trials reproduce all three old endpoints. Two
candidate confirmations reproduce `sample_38-fs8.png` at +11 bytes / +86 bits
and `sample_53.png` at +8 bytes / +64 bits. `motorcycle.png` varies: one
confirmation beats the baseline by 16 bytes / 127 bits, the other repeats
+1 byte / +6 bits, and its 60-second trial also stays +1 / +6. The first
cohort is kept as measured rather than replaced by the best repeat.

At 60 seconds, `sample_38-fs8.png` beats the fresh ten-second baseline by
11 bytes / 92 bits (65.61 seconds measured) and `sample_53.png` by 18 bytes /
143 bits (65.66 seconds). Relative to the candidate's ten-second results, each
spends another 53.90 seconds to recover 22 bytes / 178 bits and 26 bytes /
207 bits respectively. The 180-second `motorcycle.png` trial still reports
+1 byte / +6 bits (143.02 seconds measured), although the ten-second repeat
above shows a better result is reachable. No per-file exception is added for
these residuals.

At 180 seconds the final build also recovers `medium/LevelLoading.png`: no
byte loss and five bits below its historical floor, in 195.92 seconds. The
fresh baseline needed the same allowance (195.87 seconds); its 60-second trial
still lost 6 bytes / 49 bits. All hundred historical guard floors are
therefore witnessed in the current code: 98 at recorded allowances, plus
`sample_53.png` at 60 seconds and LevelLoading at 180 seconds. This does not
replace the matched-budget 98/100 result or its timing.

## Public raw Max control

The final serial, interleaved API sample has thirteen frozen raw parents from
PNG/APNG, GZIP and ZIP families, three generated method witnesses and four
guard endpoints. At ten seconds **13/20 improve and seven tie**, with no larger
result: net **83 raw bytes / 664 meaningful bits**, with optimizer time
218.640 → 159.662 seconds. Eighteen baseline calls and two candidate calls
reach the full deadline. Repetition without reserved time produced identical
output on all twenty inputs. Some sources overlap other controls, so these are
not added to the hundred-file savings.

Meaningful bits are parsed from the emitted streams; the public API's
`bits_saved` field counts eight bits per removed byte when a file shrinks.

## Default control

A serial, interleaved public-API comparison of 365 raw streams and 39 wrapper
files gives **404 byte-identical outputs**, with no timeout. Optimizer time is
148.677 seconds for the baseline and 149.513 for the candidate (+0.56%), a
single observation that does not establish a speed difference.

## Generated input controls

The final build also runs on deterministic generated raw streams, independent
of benchmark file identities:

| Control | Byte/meaningful-bit result | Measured optimizer time, baseline → candidate |
| --- | --- | --- |
| 64 literal-only fixed streams with varied alphabets and frequencies | All outputs identical; no timeouts | 74.096 → 74.479 seconds |
| 26 fixed streams with literal history and source-certified matches | Seven wins, 19 ties, no losses; −5 bytes / −44 bits | 260.534 → 188.938 seconds |

Literal controls are serial and interleaved. The match control uses the first
26 consecutive cases recorded before the final build, with candidate trials
run as a separate serial batch at the same ten-second allowance, so its timing
is not an interleaved estimate. Eighteen baseline match calls reach the full
deadline; no candidate call does.

## Older APNG residuals

At the original 20-second allowance, both candidate outputs are byte-identical
to the fresh baseline: `steam-stickers_apng/2313020_361766…` stays 4 bytes /
39 bits above its old floor and `657730_102978…` 2 bytes / 14 bits. At
60 seconds the same differences remain, in 64.44 and 64.52 seconds measured.
These shared-frame jobs do not get the reservation, which applies only to
`Complete` and `CompleteThenBounded` streams. The earlier audit records the
tree-route tradeoff that left these six bytes after much larger aggregate APNG
gains.

## Acceptance and verification

The scheduling and closure changes are accepted for their net matched-guard
saving, lower measured Max time, unchanged Default outputs and supporting raw
and generated controls. The fixed fraction and work bounds were not tuned to
recover individual losses.

- 571 Rust tests pass, including both opt-in private-corpus groups; 13
  published-tool and 189 private benchmark-harness Python tests pass.
- Independent zlib/PNG/GZIP/ZIP validation passes for 77 frozen probe outputs
  and both arms of the 404 Default, 20 raw Max, 64 literal and 26 match
  controls, and for every retained guard, miss, Defluff, confirmation and
  longer-time output.
- All 66 Defluff pairs pass; all 17 published strict miss rows reproduce,
  sixteen recover under the relaxed policy and the signed PNG is preserved.
- Final source, executable and canonical-guard hashes match the frozen manifest.

Method budgets, heuristic menus, retained parent choices and compatibility
policy still constrain the reachable set. This establishes bounded method
closure and measured corpus improvement, not global Deflate optimality or
monotone quality across timeout settings.

Private inputs, outputs, executable hashes, exact source patch, timings,
independent bit counts, confirmations and checkpoints are under
`work/miss-regression-20260911/`. The complete DeflOpt, deft4j and APNG
journals keep their earlier binary identities.

## Follow-up on 13 September 2026: the larger Max work class

**Accepted after matched screening, complete size-class validation and the
unchanged hundred-file guard.** The reservation now covers the existing 1 MiB
source size class.

R6–R9 already accepted parents up to 1 MiB compressed and decoded, but the
11 September reservation used only the 128 KiB R1–R5 limit, so an eligible
larger parent could get no optional terminal time. The extension uses the
existing Max bound: both sizes at most 1 MiB and at most 128 source wire
blocks. The later `ApngMax` child share is documented in the
[benchmark miss audit](../benchmark-miss-regression-audit.md#apng-terminal-share-follow-up-on-22-september-2026).
The primary share, deadlines, method budgets and R1–R5 gates are unchanged, and
no condition was chosen from the results.

The pre-extension executable is
`4d4e1586a0bf877b84d1bc8ff015356b07d616197fa1cdb42e53b5ad7027dfea`
(source `c2fce04`). The production candidate is
`cd2e432fad4d2672d27e1adb4fdad36658c9a98a7dfedae53c67b7242e1e81d3`.
The initial private screen used an equivalent eligibility expression through
`HeaderTree.max_bytes()`, executable
`ea328f98ae6ffafe73d5fea709d449372ccc57a96964c5433596743a79f30583`.

### Selection and normal-allowance results

Before any candidate results, the screen selected the first, middle and last
compressed size in each eligible PNG family, plus the four new journal-loss
sources inside the expanded class: fifteen sources across five families, run
serially in alternating order. It gives seven wins, four ties and four losses:
−526 bytes / −4,189 meaningful bits, with 167.54 → 160.66 seconds measured.

The production confirmation covers all 138 static PNG sources in that size
class in the complete DeflOpt journal, 123 of them outside the screen. Each
uses the pre-extension journal's recorded allowance and is compared with that
recorded baseline, not with fresh interleaved pairs.

| Cohort | Wins / ties / losses | Net bytes / meaningful bits | Pre-extension → candidate time |
| --- | ---: | ---: | ---: |
| All 138 eligible-size PNGs | 85 / 30 / 23 | −1,009 / −8,114 | 1,699.48 → 1,597.55 s |
| 123 outside the screen | 78 / 26 / 19 | −341 / −2,772 | 1,531.70 → 1,437.25 s |
| Nine generated raw/zlib/GZIP controls | 2 / 7 / 0 | −1 / −4 | 107.783 → 84.245 s |

The full class trades gross wins of 1,502 bytes / 12,028 bits against gross
losses of 493 bytes / 3,914 bits, and runtime falls 6.0% in this observation.
All 138 candidate outputs match or beat DeflOpt, all live Max-over-Default
checks pass, and Default byte/bit counts match the pre-extension journal.

| Family | Sources | Net bytes / meaningful bits |
| --- | ---: | ---: |
| css-ig-net | 22 | −74 / −606 |
| large | 1 | −1 / −4 |
| medium | 65 | −980 / −7,841 |
| oxipng | 47 | +82 / +629 |
| samplelib-png | 3 | −36 / −292 |

The oxipng family is a measured loss, so the positive totals do not show that
every workload benefits. The generated controls use literal-skew,
periodic-match and frequency-regime payloads at 192, 384 and 768 KiB with raw,
zlib and GZIP wrappers and no corpus bytes.

A generated APNG with two identical full-canvas RGB frames gives two
optimization jobs under the unchanged `ApngMax` shared policy. Both builds emit
byte-identical files, 247,025 bytes / 1,974,740 aggregate meaningful bits, in
11.646 → 11.637 seconds. This is a shared-policy control, not evidence for the
extension.

### Unchanged hundred-file guard

Against the historical floors, the production candidate records 90 wins, seven
ties and three losses: net −3,821 bytes / −30,617 bits in 1,195.35 seconds,
with all 53 live Max-over-Default checks passing and no validation error. The
normal-allowance residuals are LevelLoading (+7 bytes / +52 bits at ten
seconds), `sample_53.png` (+1 byte / +3 bits at ten seconds) and
`sample_71.png` (+52 bytes / +422 bits at twelve seconds). The first two were
already residuals before the extension; LevelLoading adds one byte / three
bits.

Against the pre-extension build's complete guard coverage, the same trials
have sixteen wins, 77 ties and seven losses, **+126 bytes / +989 bits**;
pre-extension time was 1,199.04 seconds. This adverse protective-cohort result
stands beside the broader size-class gain; the cohorts overlap and are not
additive.

The seven losses are LevelLoading, `sample_71-fs8.png`, `sample_71.png`, Death,
`filter_0_for_grayscale_16.png`,
`interlaced_grayscale_16_should_be_grayscale_16.png` and
`profile_gray_disallow_color.png`. The palette `sample_71-fs8.png` stream is
inside the unchanged 128 KiB reservation class; two fresh trials per
executable all reproduce its +50-byte / +394-bit loss against the earlier
journal, so this diagnostic does not attribute that loss to the extension.

### Longer-time recovery and remaining coverage

Thirty-two sources got 60-second trials against the strongest comparable
observed floors from the census, screen, journal losses and guard, including
the unchanged-class `sample_71-fs8.png` control. Each target is an observed
byte/bit pair from a single trial. Twenty-six recover their target, in
2,011.83 seconds measured in total, and all 31 applicable live
Max-over-Default comparisons pass.

Recoveries include Ball, `sample_59.png`, `sample_71.png`, `file09.png`,
`numbers.512.png` and `TruePNG calling-cleric.png`. The unchanged-class
`sample_71-fs8.png` beats its stronger pre-extension journal floor by
13 bytes / 106 bits in 65.72 seconds.

LevelLoading still loses six bytes / 48 bits at 60 seconds. The earlier
unreserved 180-second run had roughly 199 seconds of primary time including
grace; a 250-second allowance gives the new zero-grace primary phase
200 seconds, a comparable primary search before terminal work. That trial
beats the historical guard floor by **one byte / eight bits**, in
270.79 seconds measured.

All hundred historical guard floors now have recovery witnesses on this exact
production candidate: 97 at their recorded mode and allowance, plus:

| Guard source | Allowance | Measured time | Change against historical guard floor |
| --- | ---: | ---: | ---: |
| `css-ig-net/sample_53.png` | 60 s | 65.66 s | −25 bytes / −204 bits |
| `css-ig-net/sample_71.png` | 60 s | 65.10 s | −14 bytes / −106 bits |
| `medium/LevelLoading.png` | 250 s | 270.79 s | −1 byte / −8 bits |

These witnesses do not replace the ordinary 97/100 result; the small extra
recoveries do not justify the time as a default.

Five residuals remain against the stronger follow-up floors after the tested
larger allowances, totaling six bytes / 45 meaningful bits:

| Source | Largest tested allowance | Residual bytes / meaningful bits |
| --- | ---: | ---: |
| `medium/09-ct-c6-c4.png` | 60 s | 0 / 2 |
| `medium/AlphaBall.png` | 60 s | 1 / 9 |
| `medium/phenix.png` | 60 s | 2 / 12 |
| `medium/road.png` | 60 s | 0 / 2 |
| `oxipng/interlaced_palette_8_should_be_palette_8.png` | 180 s | 3 / 20 |

The palette trial takes 141.80 seconds and keeps the same three-byte / 20-bit
residual as at 60 seconds: more time does not widen hard method gates. The two
older APNG residuals above belong to the 11 September executable and are not in
this total.

### Reference misses and verification

The candidate reproduces all seventeen strict reference misses; sixteen reach
parity or better in same-allowance relaxed audits, and the signed PNG keeps
its preservation-policy difference. The complete Defluff replay again has
61 wins and five ties, saving 109 bytes / 932 bits in 7.535 seconds, with no
errors.

All 571 Rust tests, including the private-corpus regressions, pass; the
updated reservation test covers both exact size limits, source block limits,
floor policy, Default mode and zero allowance. Thirteen Python utility tests
and 189 private benchmark tests pass.

### Acceptance

The extension is accepted because it closes a scheduling gap across an
existing method size class, improves both the complete class and the files
outside the screen, and lowers measured aggregate runtime there. The adverse
guard and oxipng totals remain part of the decision, and no threshold was
fitted to individual results.

Executable and source hashes match the frozen production manifest; the
canonical hundred-file guard hash is unchanged. Private selection manifests,
retained outputs, independent bit counts, trial journals, recovery witnesses
and the final audit summary are under `work/miss-regression-20260913/`.

## Follow-up on 15 September 2026: header eligibility within the Max size class

The [accepted header-eligibility correction](terminal-header-work-class-validation.md)
extends R2–R5 to the existing 1 MiB Max enclosing-stream class. The complete
138-file PNG class saves 1,641 bytes / 13,142 bits with 2.1% more measured
runtime; the linked record has the full evidence, losses and remaining gaps.
