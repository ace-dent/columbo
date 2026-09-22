<!-- SPDX-License-Identifier: MIT -->

# Terminal time and method closure in Max

Catalogue update, 13 September: header-directed match response is R10 and
length-symbol exchange is R11. The closure described here is now R12 and
tracks eleven method inputs. The original
R1–R9 validation measurements below retain their historical scope.

11 September 2026. Baseline: `2e77f21`. **Accepted after matched validation.**
The implementation is in `src/deflate/optimize.rs`; current production gates
are catalogued in [routes and methods](../routes-and-methods.md).
The sections below preserve the 11 September evidence. The accepted
[13 September extension](#follow-up-on-13-september-2026-the-larger-max-work-class)
widens the reservation to the existing larger Max work class.

## Why a single sweep misses reachable savings

The terminal methods do not commute. A later method can change the payload
lengths, transmitted code-length tree, symbol support, tokens or block
boundaries that an earlier method priced. In particular, a joint tree/RLE
improvement can change the next iteration's header costs, and an alphabet
split can create a new, profitable joint-tree problem. Finishing the ordered
R1–R9 sweep therefore does not establish a local fixed point.

Max now revisits terminal methods after a strict complete-stream improvement,
while the existing optional deadline permits another route. The first sweep
keeps the established order. Default still performs its single R1–R5 sweep,
and the independent Default comparison endpoint and historical Max search
seed retain their established construction.

Nine score slots record the input last presented to each method. Accepted
candidates strictly decrease `(physical bytes, meaningful bits)`, so an input
score identifies its generation within this particular descent. A suffix
already evaluated on the unchanged candidate is skipped. This is not a
cross-route cache: equal-score alternative encodings in other lineages are
not considered interchangeable.

Each method retains its original size, block, support, operation and price
bounds. Additional sweeps use optional time, even if the first Default sweep
was mandatory. No new grace, worker, encoded-parent cache or stream-planner
replay is introduced. Original-match restoration continues to consult the
original source certificates, and every accepted result passes complete
emission and identity validation.

Strict improvement prevents cycles. A raw stream occupying at most L bytes
has at most 8L positive meaningful-bit scores, giving a finite descent without
an arbitrary round limit. A completed, unexpired sweep with no change establishes a
fixed point only for these bounded operators. Exhausted operation budgets,
heuristic menus, excluded work classes and compatibility policy still limit
coverage; this is not a claim of globally optimal Deflate encoding.

## Giving terminal methods time

Repeating a late sweep is ineffective if earlier token and source searches
consume its entire allowance. Public-API trials of repetition alone confirmed
this: all 20 frozen-parent Max results were identical at ten seconds, with
18 timeouts in each build. The larger frozen-parent gains below are therefore
not evidence that late repetition alone improved normal CLI output.

For stream-owning `Complete` and `CompleteThenBounded` Max work, the
11 September implementation reserved the final fifth of the original soft allowance for terminal methods
when source compressed and decoded sizes are both at most 128 KiB and the
wire stream has at most 128 blocks. These are the existing common work bounds
of R1–R5. The fraction reuses the existing primary/follow-up scheduling share;
it was not selected by testing several fractions against guard scores.

The primary phase includes elapsed parsing and Default-floor time and has no
separate grace. After the deferred compact source-split finalization, terminal
tree floors and R1–R9 use the original full deadline and grace. At a primary
phase yield, that split finalizer may use its existing cheap coarse rescue
only while the full soft deadline remains open; it does not start an exhaustive
split sweep in the reserved share. Phase expiry does not report a file timeout.
The mandatory Default floor still completes, even if it uses the allowance.

Each phase grows with configured time, preserving primary endpoints for longer
runs while giving compact header methods a chance under normal allowances.
This changes resource allocation and can lose a primary-search improvement;
complete-candidate comparison prevents larger replacements within a run, not
regression against a different schedule. The 22-file screening sample gained
73 bytes / 580 bits overall, with one loss of 1 byte / 6 bits, and measured
274.89 → 244.08 seconds. The hundred-file replay and independent generated
inputs below test whether that tradeoff extends beyond the screen. Shared container
children and larger work classes retain their earlier primary schedule.

## Portable regression witness

The existing generated coupled-swap fixture supplies literal frequencies,
source-certified matches and a stored history prefix. Its original stream
uses 37,782 bits. One terminal sweep stops at 36,729 bits. The resulting
alphabet splits expose another joint-tree improvement and another useful cut;
closure reaches at most 36,637 bits, at least 92 bits beyond one sweep.

The regression test validates the emitted stream, strict trees and actual
maximum distance, then checks that another closure invocation leaves the
bytes and meaningful bits unchanged. It also verifies that an expired Max
allowance retains precisely the ordinary terminal endpoint when Default work
is mandatory, and leaves the parent unchanged when all work is optional.
No private fixture bytes are embedded in the test.

## Controlled exploration

The private replay used 43 distinct completed Max parents from the previous
coupled-swap validation. The R2–R9 sequence first received one full sweep;
only subsequent gains are attributed to repetition. Repetition improved
27/43 parents by **189 raw bytes / 1,485 meaningful bits**, with no larger
result. These are frozen-parent gains, not additional whole-corpus benchmark
savings. This exploration omitted original-match restoration; the production
closure retains it against the original source.

The exploratory harness allowed eight sweeps and 20 seconds per parent.
Three parents were still improving on the eighth sweep, so its results are
lower bounds on further reachable savings, not fixed-point claims. Production
has no eight-sweep cutoff. Total measured time for all sweeps on these 43
parents was 48.654 seconds, including the first sweep; it is not the added
runtime of the CLI change.

## Matched hundred-file guard

The canonical hundred-file guard is unchanged. All trials use its recorded
mode and allowance, with the frozen baseline and candidate executables:

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
deft4j corpus. Historical floors have no comparable aggregate runtime. The
measured time change is a single matched observation, not a platform-wide
performance estimate. The screen overlaps this cohort and must not be added
to its savings.

The three losses against the fresh baseline are `motorcycle.png` (1 byte /
6 bits), `sample_38-fs8.png` (11 bytes / 86 bits), and `sample_53.png` (8 bytes /
64 bits), all under `css-ig-net`. They total 20 bytes / 156 bits, against gross
wins of 335 bytes / 2,683 bits. Only `sample_53.png` also misses its older guard
floor, by 1 byte / 3 bits. The inherited `medium/LevelLoading.png` residual
remains 6 bytes / 49 bits; it is outside the new reservation class.

All four smaller losses against the newer full journals are recovered:
`sample_04-fs8.png` improves that floor by 15 bytes / 121 bits, `sample_14.png`
by 20 bytes / 160 bits, `test-convertir-truecoloralpha-trns.png` by 7 bytes /
53 bits, and `small/psydk-Pink.png` by 4 bytes / 30 bits. These are the same
trials joined to different reference floors, not additional corpus savings.

Two additional serial baseline trials reproduce all three old endpoints
exactly. Two candidate confirmations reproduce `sample_38-fs8.png` at
+11 bytes / +86 bits and `sample_53.png` at +8 bytes / +64 bits. `motorcycle.png`
varies: one confirmation improves the baseline by 16 bytes / 127 bits, while
the other repeats +1 byte / +6 bits. Its 60-second trial also remains +1 / +6.
Thus the first cohort is kept intact, rather than replacing its loss with the
best repeat. Timed search order matters; monotone incumbent selection within a
run does not imply monotone output across different configured allowances.

At 60 seconds, `sample_38-fs8.png` improves on the fresh ten-second baseline
by 11 bytes / 92 bits (65.61 seconds measured), and `sample_53.png` by 18 bytes /
143 bits (65.66 seconds). Both formerly smaller endpoints remain recoverable;
the additional time is a quality/cost choice rather than a per-file default.
Relative to the candidate's ten-second results, each run spends another
53.90 seconds to recover 22 bytes / 178 bits and 26 bytes / 207 bits,
respectively. No filename-specific time allowance is built into the optimizer.
The 180-second `motorcycle.png` trial still reports +1 byte / +6 bits
(143.02 seconds measured). A better current-code result is nevertheless
witnessed by the ten-second repeat above, so it is not a structurally forbidden
endpoint. More allowance alone does not reproduce that search order. This tiny
residual is retained as a measured schedule tradeoff; no timer threshold or
filename exception is added to recover it.
The final build also recovers `medium/LevelLoading.png` at 180 seconds:
zero byte loss and five bits below its historical floor, in 195.92 seconds.
The fresh baseline needed the same allowance and took 195.87 seconds; its
60-second trial still lost 6 bytes / 49 bits. All hundred historical guard
floors are therefore witnessed in the current code: 98 at recorded allowances,
plus `sample_53.png` at 60 seconds and LevelLoading at 180 seconds. This does
not replace the matched-budget 98/100 result or its aggregate timing with
best-of-multiple-budget scores. The older APNG results are recorded below.

## Public raw Max control

The final serial, interleaved API sample contains thirteen frozen raw parents
representing PNG/APNG, GZIP and ZIP families, three generated method witnesses,
and four guard endpoints. At ten seconds, **13/20 improve and seven tie**, with
no larger result: net **83 raw bytes / 664 meaningful bits**, and measured optimizer time
218.640 → 159.662 seconds. Eighteen baseline calls and two candidate calls
report the full deadline reached. A primary phase yield alone is not a file
timeout; a non-timeout result still does not prove global optimality.

These are already-optimized parents passed through the raw public API, not a
complete wrapper corpus, and some source identities overlap other controls.
They must not be added to the hundred-file savings. In contrast, repetition
without reserved time produced identical output on all twenty inputs.

Meaningful bits are parsed independently from the emitted streams. The public
API's `bits_saved` field uses eight bits per removed physical byte when a file
shrinks, and meaningful-bit savings only when its byte length ties. Subtracting
that display metric between different byte sizes is not a meaningful-bit
measurement; the independent check caught and corrected that harness assumption.

## Default control

A serial, interleaved public-API comparison covers 365 raw streams and 39
wrapper files. All **404 outputs are byte-for-byte identical**. Neither build
reports a timeout. Aggregate optimizer time is 148.677 seconds for baseline
and 149.513 seconds for the final candidate (+0.56%); this single observation does
not establish a speed difference.

## Generated input controls

The final build also runs on deterministic generated raw streams independent
of benchmark file identities:

| Control | Byte/meaningful-bit result | Measured optimizer time, baseline → candidate |
| --- | --- | --- |
| 64 literal-only fixed streams with varied alphabets and frequencies | All outputs identical; no timeouts | 74.096 → 74.479 seconds |
| 26 fixed streams with literal history and source-certified matches | Seven wins, 19 ties, no losses; −5 bytes / −44 bits | 260.534 → 188.938 seconds |

Literal controls are serial and interleaved. The match control uses the first
26 consecutive cases recorded before the final build; candidate trials are a
separate serial batch at the same ten-second allowance. Its timing is not an
interleaved performance estimate. Eighteen baseline match calls reach the full
deadline; no candidate call does. All generated sources and outputs are
independently decoded and all reported meaningful bits are independently parsed.
These raw controls are not added to wrapper-corpus totals.

## Older APNG residuals

At the original 20-second allowance, both candidate outputs are byte-identical
to the fresh baseline. `steam-stickers_apng/2313020_361766…` remains 4 bytes /
39 bits above its old floor; `657730_102978…` remains 2 bytes / 14 bits above.
At 60 seconds, the same differences remain, with measured times of 64.44 and
64.52 seconds. All four candidate outputs validate.

These shared-frame jobs do not receive the compact stream-owner reservation.
The earlier audit records the general tree-route tradeoff that left these six
bytes after much larger aggregate APNG gains. This change does not recover
them, and a 60-second non-recovery is not proof of structural unreachability.
No full current APNG corpus is claimed from this pair of residual checks.

## Acceptance and verification

Accept the general scheduling and closure changes for their net matched-guard
saving, reduced measured Max time, unchanged Default outputs, and supporting
raw/generated controls. The fixed fraction and work bounds were not adjusted
to recover individual losses. The canonical hundred-file guard is unchanged;
its two short-budget residuals and the three fresh-baseline losses remain in
the report even where longer or repeated runs recover better results.

- `cargo test --release --locked`: 561 regular Rust tests pass.
- Both opt-in private-corpus groups pass: six integration and four library
  tests, giving 571 Rust tests in total.
- Thirteen published-tool and 189 private benchmark-harness Python tests pass.
- Clippy with all targets/features and warnings denied passes; formatting and
  diff whitespace checks pass.
- Independent zlib/PNG/GZIP/ZIP validation passes for 77 frozen probe outputs
  and both arms of the 404 Default, 20 raw Max, 64 literal and 26 match controls.
  Raw meaningful-bit counts are checked independently of the public API's
  byte-first display metric. The benchmark runners also independently validate
  every retained guard, miss, Defluff, confirmation and longer-time output.
- All 66 Defluff pairs pass; all 17 published strict miss rows reproduce,
  sixteen recover under the relaxed policy and the signed PNG is preserved.
- Final source, executable and canonical-guard hashes match the frozen manifest.

Strict descent and positive time shares remove two barriers, not every search
limit. Method-specific budgets, heuristic menus, retained parent choices and
compatibility policy still constrain the reachable set. This work establishes
bounded method closure and measured corpus improvement, not global Deflate
optimality or monotonic quality across separate timeout settings.

Private inputs, outputs, executable hashes, exact source patch, timings,
independent bit counts, confirmations and checkpoints are retained under
`work/miss-regression-20260911/`. The complete DeflOpt, deft4j and APNG journals
retain their own earlier binary identities; they were not relabelled as full
runs of this candidate.

## Follow-up on 13 September 2026: the larger Max work class

**Accepted after matched screening, complete work-class validation and the
unchanged hundred-file guard.** The reservation now covers the existing 1 MiB
source work class. The measured guard tradeoff is retained below.

R6–R9 already admit parents up to 1 MiB compressed and decoded, whereas the
11 September reservation used only the common 128 KiB R1–R5 class. An eligible
larger parent could therefore receive no optional terminal time. The extension
uses the existing Max bound for source admission, with both sizes at most
1 MiB and at most 128 source wire blocks. Max, nonzero allowance, and stream
ownership through `Complete` or `CompleteThenBounded` remain required.
This describes the 13 September admission rule. The later `ApngMax` child
share is documented in the [benchmark miss audit](../benchmark-miss-regression-audit.md#apng-terminal-share-follow-up-on-22-september-2026).

The 4/5 primary share, original terminal deadline/grace, deferred coarse split
rescue and all method budgets remain unchanged. R1–R5 still have their 128 KiB
method gates. No fraction, image dimension or filename condition was selected
from the results. Both phases grow with configured time. This is a scheduling
choice; it does not enlarge candidate menus or prove global optimality.

The pre-extension executable is
`4d4e1586a0bf877b84d1bc8ff015356b07d616197fa1cdb42e53b5ad7027dfea`
(source `c2fce04`). The production candidate is
`cd2e432fad4d2672d27e1adb4fdad36658c9a98a7dfedae53c67b7242e1e81d3`.
The initial private screen used the equivalent admission expression through
`HeaderTree.max_bytes()`, executable
`ea328f98ae6ffafe73d5fea709d449372ccc57a96964c5433596743a79f30583`;
production shares a named constant with the existing R6–R9 limits.

### Selection and normal-allowance results

Before candidate results, the screen selected the first, middle and last
compressed size in each eligible PNG family, plus the four new journal-loss
sources inside the expanded class. That gives fifteen sources across five
families. Baseline/candidate order alternates across files; all optimization
calls are serial. The screen has seven wins, four ties and four losses:
−526 bytes / −4,189 meaningful bits, with 167.54 → 160.66 seconds measured.

The production confirmation covers all 138 static PNG sources in that size
class in the complete DeflOpt journal, including the 123 outside the screen.
Each uses the pre-extension journal's recorded allowance. The comparison is
against that complete recorded baseline, not interleaved fresh pairs.

| Cohort | Wins / ties / losses | Net bytes / meaningful bits | Pre-extension → candidate time |
| --- | ---: | ---: | ---: |
| All 138 eligible-size PNGs | 85 / 30 / 23 | −1,009 / −8,114 | 1,699.48 → 1,597.55 s |
| 123 outside the screen | 78 / 26 / 19 | −341 / −2,772 | 1,531.70 → 1,437.25 s |
| Nine generated raw/zlib/GZIP controls | 2 / 7 / 0 | −1 / −4 | 107.783 → 84.245 s |

The full size-class result trades gross wins of 1,502 bytes / 12,028 bits
against gross losses of 493 bytes / 3,914 bits. Runtime falls by 6.0% in this
observation; this is not a uniform whole-file speedup claim. All 138 candidate
outputs match or beat DeflOpt, all live Max-over-Default checks pass, and their
Default byte/bit counts match the pre-extension journal throughout. Independent
PNG validation checks decoded streams, wrappers, CRCs and meaningful bits.

| Family | Sources | Net bytes / meaningful bits |
| --- | ---: | ---: |
| css-ig-net | 22 | −74 / −606 |
| large | 1 | −1 / −4 |
| medium | 65 | −980 / −7,841 |
| oxipng | 47 | +82 / +629 |
| samplelib-png | 3 | −36 / −292 |

The oxipng family is a measured loss. The positive full and held-out totals
do not establish that every workload benefits. The generated controls use
literal-skew, periodic-match and frequency-regime payloads at 192, 384 and
768 KiB, with raw, zlib and GZIP wrappers. They contain no corpus bytes.
Baseline/candidate order alternates, and independent decoding verifies every
output. Meaningful bits are parsed from emitted streams rather than inferred
from the public byte-oriented `bits_saved` result.

A separately generated APNG has two identical full-canvas RGB frames. IDAT is
its own job, so these are two optimization jobs under the unchanged `ApngMax`
shared policy, despite their identical compressed image payloads. Both builds
emit byte-identical files: 247,025 bytes / 1,974,740 aggregate meaningful bits,
in 11.646 → 11.637 seconds. Both decoded image streams and all CRCs validate.
This is a shared-policy control, not evidence for the stream-owner extension.

### Unchanged hundred-file guard

Against the historical floors, the production candidate records 90 wins,
seven ties and three losses: net −3,821 bytes / −30,617 bits in 1,195.35 seconds.
All 53 live Max-over-Default checks pass, with no validation error. The normal
allowance residuals are LevelLoading (+7 bytes / +52 bits at ten seconds),
`sample_53.png` (+1 byte / +3 bits at ten seconds), and `sample_71.png`
(+52 bytes / +422 bits at twelve seconds). The first two were already residuals
before the extension; LevelLoading adds one byte / three bits.

Against the pre-extension build's complete guard coverage, the same trials
have sixteen wins, 77 ties and seven losses, for **+126 bytes / +989 bits**.
Pre-extension time was 1,199.04 seconds; the small time difference is not a
speed claim. This adverse protective-cohort result is retained alongside the
broader size-class gain. The cohorts overlap and their totals must not be added.
No historical floor is weakened or replaced by a best-of-repeat result.

The seven losses against the pre-extension guard results are LevelLoading,
`sample_71-fs8.png`, `sample_71.png`, Death, `filter_0_for_grayscale_16.png`,
`interlaced_grayscale_16_should_be_grayscale_16.png`, and
`profile_gray_disallow_color.png`. The palette `sample_71-fs8.png` stream stays
inside the unchanged 128 KiB reservation class. Two fresh trials per executable
all reproduce its +50-byte / +394-bit loss against the earlier journal. This
diagnostic does not isolate that loss to the reservation extension.

### Longer-time recovery and remaining coverage

Thirty-two sources received 60-second trials against the strongest comparable
observed floors from the census, screen, journal losses and guard, including
the unchanged-class `sample_71-fs8.png` control. Each target is an actually
observed byte/bit pair under the same format and preservation policy; the
comparison never constructs an artificial floor from different trials.
Twenty-six recover their target, with 2,011.83 seconds measured in total.
All 31 applicable live Max-over-Default comparisons pass. The original
normal-allowance rows and aggregate timings remain unchanged.

Recoveries include Ball, `sample_59.png`, `sample_71.png`, `file09.png`,
`numbers.512.png` and `TruePNG calling-cleric.png`. The unchanged-class
`sample_71-fs8.png` beats its stronger pre-extension journal floor by 13 bytes /
106 bits in 65.72 seconds. This establishes reachable savings, while its
same-time repeats above do not isolate the original loss to the extension.

LevelLoading still loses six bytes / 48 bits at 60 seconds. The earlier
unreserved 180-second run had roughly 199 seconds of primary time including
grace. A 250-second allowance gives the new zero-grace primary phase 200
seconds, permitting a comparable primary search before terminal work. This
trial beats the historical guard floor by **one byte / eight bits**, with
270.79 seconds measured. The configured allowance and measured wall time are
reported separately because active work can finish within the existing grace.

All hundred historical guard floors now have recovery witnesses on this exact
production candidate: 97 at their recorded mode and allowance, plus:

| Guard source | Allowance | Measured time | Change against historical guard floor |
| --- | ---: | ---: | ---: |
| `css-ig-net/sample_53.png` | 60 s | 65.66 s | −25 bytes / −204 bits |
| `css-ig-net/sample_71.png` | 60 s | 65.10 s | −14 bytes / −106 bits |
| `medium/LevelLoading.png` | 250 s | 270.79 s | −1 byte / −8 bits |

These witnesses do not replace the ordinary 97/100 floor result or imply
monotone quality across allowances. The extra time recovers small amounts;
it is not a suitable default cost for every input.

Five residuals remain against the stronger observed follow-up floors after
the tested larger allowances, totalling six bytes / 45 meaningful bits:

| Source | Largest tested allowance | Residual bytes / meaningful bits |
| --- | ---: | ---: |
| `medium/09-ct-c6-c4.png` | 60 s | 0 / 2 |
| `medium/AlphaBall.png` | 60 s | 1 / 9 |
| `medium/phenix.png` | 60 s | 2 / 12 |
| `medium/road.png` | 60 s | 0 / 2 |
| `oxipng/interlaced_palette_8_should_be_palette_8.png` | 180 s | 3 / 20 |

The palette trial takes 141.80 seconds and retains the same three-byte /
20-bit residual as at 60 seconds. It provides no evidence that simply
increasing its deadline recovers the floor. A finite failed run does not
prove structural unreachability: primary lineage choices, heuristic menus and
per-invocation budgets still restrict search. More time also does not expand
hard method gates. No per-file condition or budget enlargement is introduced
to chase these remaining bits. The two older APNG residuals above belong to
the 11 September executable; this follow-up does not present them as fresh
candidate trials or include them in this five-source total.

### Reference misses and verification

The production candidate reproduces all seventeen strict reference misses;
sixteen reach parity or better in same-allowance relaxed audits. The signed
PNG retains its preservation-policy difference. Its complete Defluff replay
again has 61 wins and five ties, saving 109 bytes / 932 bits in 7.535 seconds,
with no errors.
The existing complete public journals retain their original executable
identities; partial new-binary results are kept separate.

All 571 Rust tests, including the private-corpus regressions, pass. The updated
reservation test covers both exact size limits, either size over its limit,
source block limits, stream ownership, Default mode and zero allowance.
The original phase-clock/grace and terminal-closure regression tests pass.
Locked build, formatting, all-feature Clippy with warnings denied, documentation
tests, thirteen Python utility tests and 189 private benchmark tests pass.
The package list excludes private fixtures and scratch artifacts.

### Acceptance

Accept the extension because it closes a scheduling gap across an existing
method work class, improves both the complete class and the files outside the
screen, and reduces measured aggregate runtime there. The independent generated
controls show no loss, Default comparisons pass, and all historical guard
floors have current-code witnesses. The adverse guard and oxipng totals remain
part of this decision; success on these cohorts does not establish universal
benefit. No new heuristic threshold was fitted to individual results.

Executable and source hashes match the frozen production manifest; the
canonical hundred-file guard hash is unchanged. Private selection manifests,
retained outputs, independent bit counts, trial journals, recovery witnesses
and final audit summary are under `work/miss-regression-20260913/`.

## Follow-up on 15 September 2026: header admission within the Max class

The [accepted header-admission correction](terminal-header-work-class-validation.md)
extends R2–R5 to the existing 1 MiB Max enclosing-stream class. Default and
its mandatory comparison work retain 128 KiB admission, R1 is unchanged,
and R5 keeps its 128 KiB block limit. Reservation fractions, per-method
budgets and deadlines are unchanged. Generated witnesses and no-deadline
frozen-parent probes establish the admission barrier independently of timing.

The complete 138-file PNG class saves 1,641 bytes / 13,142 bits with 2.1%
more measured runtime. The additional cohort and unchanged guard also improve
in aggregate; all hundred historical guard floors have candidate witnesses,
including five at larger allowances. The linked record retains normal-run
losses, five remaining target gaps, and the limits established by the
response-method budget probes. The dated 11 and 13 September evidence above
keeps its original executable identities and measurements.
