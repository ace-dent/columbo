<!-- SPDX-License-Identifier: MIT -->

# Terminal header admission in Max

**Accepted on 15 September 2026** after complete work-class validation, the
unchanged hundred-file guard, longer-time recovery and independent probes.

## Problem and rule

R2–R5 used the same 128 KiB enclosing compressed/decoded stream limit in
Default and Max. R6–R11, and the existing terminal time reservation, already
used a 1 MiB envelope. Thus a small, bounded header problem could become
ineligible merely because an unrelated block enlarged its enclosing stream.
A larger timeout cannot pass that admission test.

Max now admits R2–R5 to the existing 1 MiB envelope. The selected compressed
and decoded sizes must each fit, with at most 128 parsed blocks and no
unrepresented wire blocks. Mandatory Default comparison work retains its
128 KiB envelope, including when it is performed for Max. R1 restoration also
retains its existing envelope and certificate limits. R5 still limits each
eligible block to 128 KiB decoded and 8,192 tokens; its proposal-local limits
remain unchanged. R10/R11 keep their smaller block gates.

No method menu, shared operation/price cap, phase share, deadline, grace or
worker count changes. Existing stream-level work caps constrain repeated
header pricing independently of surrounding decoded bytes, while the common
1 MiB envelope bounds parsing and token traversal. This reuses an established
Max memory/work class. It does not assert that every larger stream should
receive every operator, or make runtime unlimited in normal operation.

## Structural evidence

A generated regression places each of four existing method witnesses after
128 KiB of stored bytes. The prefix changes the enclosing stream's size class
without changing the header problem. Max reaches the expected 1-, 1-, 7- and
4-bit improvements from R2, R3, R4 and R5. Default remains excluded; a 1 MiB
prefix also excludes Max; an already-reached stop returns no candidate.
Every accepted stream is reparsed, its identity and stored alignment are
checked, and unchanged-token methods preserve tokens. R5's rewritten tokens
are checked against the permitted local rewrite relation. There are no corpus
filenames or corpus bytes in this test or the production rule.

Five parents retained from the 13 September `cd2e432f…` build's 60-second
trials were separately closed through the current terminal sequence without
a deadline, preserving the true original source certificates. The current
code with the old admission rule changes none of them. Enlarging only the
R2–R5 enclosing envelope gives:

| Frozen parent | Before → after meaningful bits | Raw bytes saved |
| --- | ---: | ---: |
| 09 | 64,384 → 64,384 | 0 |
| AlphaBall | 192,534 → 192,519 | 2 |
| phenix | 46,507 → 46,476 | 4 |
| road | 37,507 → 37,473 | 4 |
| palette | 150,121 → 150,079 | 6 |

All ten outputs decode independently to their original source payloads.
The four improvements total 16 raw bytes / 122 meaningful bits and surpass
those parents' older follow-up targets. These are frozen-parent coverage
witnesses, not timed whole-file or corpus measurements. The unchanged 09
parent motivates a separate diagnostic of remaining method and budget limits.

## Executables and selection

The complete current benchmark executable is
`6c601870adb6cb43b6f5fb1b0c5c30357bdff8aa45bc25b13b6ff631d76cfc01`
(source `7fa3fdf`). The frozen production candidate is
`11f8f7f1090466c23d235b3c1400efd94b6ed7460c76515e9bffa53cfa271385`.
All benchmark invocations, diagnostic runs and builds are serialized. Source and
binary hashes are checked against the frozen manifest.

The first screen combines the previous size-stratified fifteen-case sample
with the five diagnostic cases. Baseline/candidate order alternates. It gives
15 improvements, five ties and no losses: −262 bytes / −2,101 meaningful bits,
with 212.39 → 210.53 seconds measured. Nineteen screen files lie inside the
138-case static PNG size class; 09 is an unchanged-admission control.

Broader validation covers all 138 static PNGs in that size class at the
complete current journal's allowances, including 119 outside the screen.
A source inventory inspects PNG image and compressed metadata streams,
APNG frames, ZIP entries and GZIP members across all 1,640 unique sources in
the two complete strict benchmark journals. Conservative inclusion on an
inventory error avoids treating an unknown stream as ineligible. It identifies
69 additional policy-matched trials beyond the original PNG class: six
DeflOpt cases (three GZIP, three ZIP) and 63 deft4j cases (58 PNG/APNG, two ZIP,
three GZIP). PNG duplicates can share evidence across runners when preservation
policies agree; ZIP stripping policies cannot. Some GZIP trials overlap across
runners, so these cohorts' totals are reported separately.

These broad comparisons use actual recorded `6c601870…` rows, not fresh
interleaved pairs. Original corpus rows remain intact when repeats improve
or larger allowances recover their endpoints. The unchanged hundred-file
guard, all seventeen strict misses with relaxed audits, all 66 Defluff pairs,
the two old APNG residuals, and nine generated raw/zlib/GZIP controls receive
separate candidate trials. Generated controls use no corpus bytes.

## Completed broad results

| Cohort | Improvements / ties / losses | Net bytes / meaningful bits | Current → candidate runtime |
| --- | ---: | ---: | ---: |
| All 138 static PNGs | 116 / 16 / 6 | −1,641 / −13,142 | 1,633.17 → 1,666.87 s |
| 119 outside the screen | 102 / 12 / 5 | −1,117 / −8,939 | 1,430.89 → 1,468.10 s |
| 69 additional policy-matched cases | 41 / 27 / 1 | −475 / −3,798 | 967.67 → 979.97 s |

The full class trades gross gains of 1,856 bytes / 14,856 bits against losses
of 215 bytes / 1,714 bits. Measured aggregate runtime increases by 33.70 seconds
(2.1%); the initial paired screen's slight time reduction is not generalized
to this larger cohort. All 138 live Default byte/bit counts match the complete
baseline, every Max-over-Default check passes, and no reference miss occurs.

| Family | Sources | Net bytes / meaningful bits |
| --- | ---: | ---: |
| css-ig-net | 22 | −246 / −1,963 |
| large | 1 | −3 / −30 |
| medium | 65 | −1,108 / −8,880 |
| oxipng | 47 | −270 / −2,160 |
| samplelib-png | 3 | −14 / −109 |

The additional 69 trials trade 476 bytes / 3,810 bits of gross gains against
one byte / 12 bits of loss on `apng-large/elephant-2.png`. Runtime increases by
12.30 seconds (1.3%). All six live Default comparisons match the recorded
baseline, no Max-over-Default failure occurs, and no new reference miss appears.
Their format/policy totals are:

| Benchmark policy and format | Trials | Net bytes / meaningful bits |
| --- | ---: | ---: |
| DeflOpt GZIP | 3 | 0 / 0 |
| DeflOpt ZIP | 3 | 0 / 0 |
| deft4j GZIP | 3 | 0 / 0 |
| deft4j PNG/APNG | 58 | −475 / −3,798 |
| deft4j ZIP | 2 | 0 / 0 |

## Unchanged hundred-file guard

The full replay gives 89 improvements, six ties and five losses against the
historical guard floors: −3,910 bytes / −31,329 meaningful bits. No floor is
weakened. Against complete current-build coverage of the same hundred guard
cases, it gives 25 improvements, 69 ties and six losses: **−172 bytes / −1,368
bits**, with 1,200.99 → 1,198.40 seconds measured. The small runtime difference
is not a speedup claim. Gross current-baseline gains of 185 bytes / 1,465 bits
outweigh losses of 13 bytes / 97 bits.

All 53 live Default byte/bit comparisons match the current complete journal,
and all Max-over-Default checks pass. The 23 guard cases whose sources lie in
the expanded class have 18 improvements and five ties, saving 152 bytes /
1,216 bits against current results. The other 77 have seven improvements,
64 ties and six losses, saving twenty bytes / 152 bits in total. Source-class
stratification helps distinguish the changed admission from timed controls;
it does not turn recorded-baseline comparisons into interleaved pairs.

| Historical-floor residual | Allowance | Bytes / meaningful bits above floor |
| --- | ---: | ---: |
| `medium/LevelLoading.png` | 10 s | +7 / +52 |
| `css-ig-net/sample_53.png` | 10 s | +2 / +9 |
| `css-ig-net/sample_71.png` | 12 s | +14 / +114 |
| `oxipng/filter_0_for_grayscale_16.png` | 10 s | +15 / +121 |
| `oxipng/interlaced_grayscale_16_should_be_grayscale_16.png` | 10 s | +3 / +26 |

The six losses against current results are `css-ig-net/Orange512.png`,
`css-ig-net/briefcase.png`, `css-ig-net/file04.png`,
`css-ig-net/sample_38-fs8.png`, `css-ig-net/sample_53.png`, and
`css-ig-net/sample_60.png`. They remain in the normal-allowance total even
when a repeat or larger allowance later recovers the endpoint.

## Reference misses and Defluff

All seventeen strict reference misses reproduce on the candidate. Sixteen
reach parity or better with the same allowance under `--strict 0`; the signed
PNG remains byte-identical to its source, preserving the unsafe-to-copy chunk.
Strict misses remain visible, and ordinary strictness is unchanged.

The complete 66-pair Defluff replay gives 61 wins and five ties, saving 109 bytes
/ 932 meaningful bits in 7.634 seconds. There are no errors or
new misses. These overlap other evidence and are not added to corpus savings.

## APNG and generated controls

The two older APNG residuals receive fresh alternating-order current/candidate
pairs at their original 20-second allowance. The first ties current output;
the second saves two meaningful bits at unchanged byte size. Their gaps to the
older APNG floors remain four bytes / 39 bits and two bytes / 14 bits. The
paired times are 22.20 → 22.20 seconds and 22.27 → 22.27 seconds. These sources
come from the separate older APNG evidence, outside the current complete-journal
inventory. Their shared frame policy received no terminal reservation in this
historical test. The later bounded `ApngMax` reservation and its independent
validation are in the [benchmark miss audit](../benchmark-miss-regression-audit.md#apng-terminal-share-follow-up-on-22-september-2026).

Nine generated raw/zlib/GZIP controls give four wins, five ties and no losses:
−24 bytes / −205 meaningful bits, with 84.292 → 85.869 seconds measured.
Independent decoding verifies both arms of every pair, and an independent
Deflate parser measures meaningful bits. Payload families are literal skew,
periodic matches and frequency regimes at 192, 384 and 768 KiB, with wrappers
rotated across families. The generated inputs contain no corpus bytes.

The whole-file improvements occur in raw and zlib controls; the three GZIP
controls tie their baselines. These controls do not establish universal gains
or global optimality.

## Residuals and greater allowances

All hundred unchanged historical guard floors have witnesses on this exact
candidate: 95 at their recorded settings, four at 60 seconds, and LevelLoading
at 250 seconds. The longer trials are:

| Guard source | Allowance | Measured runtime | Bytes / bits against historical floor |
| --- | ---: | ---: | ---: |
| `medium/LevelLoading.png` | 250 s | 270.82 s | −1 / −8 |
| `oxipng/interlaced_grayscale_16_should_be_grayscale_16.png` | 60 s | 65.70 s | −75 / −593 |
| `css-ig-net/sample_53.png` | 60 s | 65.66 s | −25 / −204 |
| `css-ig-net/sample_71.png` | 60 s | 65.05 s | −44 / −348 |
| `oxipng/filter_0_for_grayscale_16.png` | 60 s | 65.70 s | −59 / −467 |

These witnesses do not replace the 95/100 normal-allowance result. The
22 follow-ups use the strongest comparable actual byte/bit pairs from current
journals, earlier floors, fresh baseline 60-second trials and the paired APNG
checks. They never combine one trial's byte minimum with another trial's bit
minimum. Seventeen targets recover, with 1,745.54 seconds of measured candidate
runtime. Allowances and measured runtime differ because existing active-route
grace is unchanged.

All six losses in the 138-file census have candidate witnesses: five at
60 seconds and the grayscale-alpha case in the earlier normal-allowance
screen. The additional cohort's one APNG loss also recovers. Nerd matches
the baseline's stronger 60-second endpoint; its original large full-journal
loss is not replaced with that result. Fs improves from +280 bytes / +2,242
bits at 60 seconds on baseline to +1 byte / +5 bits at 180 seconds on the
candidate. Its 2.85 MB decoded stream is outside the changed header class.

Five targets remain above their floors after the tested larger allowances,
totalling eight bytes / 68 meaningful bits:

| Source | Allowance | Residual bytes / meaningful bits |
| --- | ---: | ---: |
| `css-ig-net/briefcase.png` | 60 s | +1 / +8 |
| `medium/09-ct-c6-c4.png` | 60 s | +0 / +2 |
| `medium/FsqwhPuaIAIlojU.png` | 180 s | +1 / +5 |
| `steam-stickers_apng/2313020_361766_5d0f4d4940275cbb52584af6d77f9ed2e85bd6a3.png` | 60 s | +4 / +39 |
| `steam-stickers_apng/657730_102978_7e5b38c66059d4e5cca14167e02dbcddf5091c2d.png` | 60 s | +2 / +14 |

The APNG residuals match their candidate 20-second results; their 60-second
runs take 64.44 and 64.51 seconds. Briefcase finishes its longer trial in
48.49 seconds but still misses by one byte / eight bits. Fresh alternating
original-allowance comparisons then give:

| Source / allowance | Frozen baseline gap | Candidate gap | Baseline → candidate time |
| --- | ---: | ---: | ---: |
| Briefcase / 10 s, baseline `6c601870…` | +1 byte / +8 bits | +1 byte / +8 bits | 9.31 → 9.31 s |
| Fs / 20 s, baseline `4d4e1586…` | +293 bytes / +2,346 bits | +240 bytes / +1,922 bits | 22.61 → 22.61 s |

Fs's older journal allowance was twenty seconds; the earlier fresh 21-second
comparisons used the newer journal's allowance. The candidate saves 53 bytes /
424 bits in this fresh twenty-second pair, while both frozen baselines miss
their own older endpoints. These observations do not isolate the remaining
gaps to the admission change. Original corpus rows remain intact.

### Further response-method coverage

After the enlarged-header closure, each of the five frozen parents was tested
with R10/R11 block eligibility widened to 1 MiB decoded, 8,192 tokens and
8,192 matches. Work and price budgets were tried at 1×, 4× and 16×, with an
unchanged-production-gates control for each parent. None of the twenty
no-deadline trials improves a parent. Some still exhaust their work budgets,
so those trials alone do not establish completed search of the menus.

A final diagnostic focuses on the remaining 09 parent with effectively
nonbinding budgets: 2^45 work units and 2^29 header prices per response method.
It retains their finite candidate menus and enlarged block gates. Both methods
finish with work and prices remaining, spending 1,761,211,636 and 837,564,022
work units respectively. The terminal closure takes 2.096 seconds and emits
the exact same parent bytes: 64,384 meaningful bits, still two above the older
endpoint. Independent decoding and bit parsing validate all twenty-one probes.

Increasing these two response methods' budgets alone does not recover this
parent's gap. Their heuristic tree menus, other operators' limits and primary
parent selection remain possible constraints. This does not prove that the
whole optimizer can never reach the older endpoint. No response-method gate
or budget increase is accepted from these probes, which show extra work
without extra savings on the selected parents.

A successful witness proves that its endpoint is reachable. A failed finite
run does not prove that it is unreachable. Hard admission checks establish
particular missing operator coverage; budget exhaustion and heuristic menus
limit what a completed bounded-method closure proves.

## Verification and acceptance

The frozen candidate passes 585 Rust tests, including the opt-in corpus
regressions, and 202 Python tests (13 public utility and 189 private harness
tests). Formatting, all-target/all-feature Clippy with warnings denied,
locked release build, the documentation-test command and package allowlist pass.
All benchmark runners independently validate retained outputs and meaningful
bit counts. Final checks verify all 1,640 inventoried source hashes, the frozen
source and executable manifests, all cohort counts, 197 live Default checks
across overlapping cohorts, the unchanged guard hash and all hundred guard
witnesses. The complete public journals retain their original strict sizes,
bit counts, runtimes and executable identities.

Accept the admission extension because it removes a demonstrated hard barrier
within an existing bounded work class, improves the complete class and its
119 files outside the screen, improves the additional cohort and guard, and
has no loss on the generated controls. The measured 2.1% and 1.3% runtime
increases and every normal-allowance loss remain part of the tradeoff. The
rule contains no filenames, image dimensions or thresholds fitted to
individual results. Mandatory Default work, method budgets and deadlines
remain unchanged.

The remaining eight-byte / 68-bit gap is disclosed rather than hidden by
selecting better repeats or relaxing ordinary output policy. All hundred
historical guard floors have witnesses, but this establishes bounded method
coverage and useful corpus gains, not global Deflate optimality or monotone
quality across different timeout settings. The
[route catalogue](../routes-and-methods.md) records the accepted limits.

The complete public journals retain their recorded executable identities.
Private selection manifests, original rows, output artifacts, source hashes,
probe results and follow-up records are under `work/miss-regression-20260915/`.
