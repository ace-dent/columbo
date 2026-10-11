# Permanent original-match proofs: validation

5 September 2026. Baseline: `dbe148efbf60581f98d4d6bc2f4c0c58d21a1515`.
This validates a small, practical form of the first [alternative formulation](novel-deflate-formulations.md): retain original match certificates and use them to restore profitable matches in a completed candidate. A prototype ran in a disposable source snapshot; the bounded fixed-tree variant was then implemented in production.

**Decision: implement the bounded fixed-tree variant.** Original certificates produced improvements that an otherwise identical search using only the selected tokens could not. A targeted pass recovered most of the benefit without changing any trees or headers. A general reversible graph optimizer remains untested and is not needed for these gains.

## Production follow-up

The implementation is in [`src/deflate/restore.rs`](../../src/deflate/restore.rs), with terminal scheduling in [`optimize.rs`](../../src/deflate/optimize.rs). It runs in Default and Max, in strict and relaxed modes; [R1 in the route catalog](../routes-and-methods.md#route-gate-reference) gives the current method, eligibility rules and work bounds.

The production A/B comparison reproduced **all 365 reference outputs exactly**: 48 improved streams, 18 bytes and 156 meaningful bits saved, with no regressions. The 11 complete-container cases reproduced the prototype's ten improvements, totaling six bytes and 63 meaningful bits. Independent zlib, PNG, GZIP and ZIP checks passed, including advertised-window validation.

A sequential raw-sample timing was 78.25 seconds before and 80.45 seconds after. A second comparison, alternating baseline/candidate order for each input in persistent processes, measured **78.87 seconds before and 78.68 seconds after**. Two container repetitions took 11.025/11.015 seconds before and 10.975/10.978 seconds after. These runs show no repeatable slowdown or speedup on this sample; full-corpus and larger-input performance were not measured.

All **489 tests** pass (432 library, 46 CLI, 11 public API). New coverage includes the exact 1,740-to-1,737-bit Max witness, certificate containment and distance, block clipping, stored alignment, and exhaustive comparison with 448 small abstract price problems. The public API test verifies the PNG byte saving and Max's improved Default floor even with zero optional-search time.

Production measurements and exact outputs are in `work/restoration-implementation/`: aggregate savings (`summary.json`), raw and container bit accounting (`raw-validation.json`, `wrappers-validation.json`) and the test log `work/restoration-full-tests.log`. The sections below keep the original research controls.

## Mechanism and scope

An original match certifies `(decoded start, decoded end, distance)`, and the certificate survives when optimization replaces the match with literals. Under the final tree some certified matches can become cheaper again, and they can be recovered without searching history. Before this change, selected states carried no certificate for a match that had become literals.

For example, the Default output of the stream in `PngSuite/f00n0g08.png` costs 1,865 meaningful bits. Restoring its original match covering decoded positions `[312, 330)` at distance 34 reduces it to 1,864 bits with the same header and trees, and the complete PNG becomes one byte smaller.

The prototype clips certificates to the candidate's block boundaries, finds the cheapest spelling from literals and submatches of lengths 3–258 inside a certificate at its original distance, and accepts only a strict complete-stream improvement. The **targeted fixed-tree variant** examines only certificate ranges interrupted by literal gaps, requires at least one restored match unavailable from the selected proofs, and keeps blocks, code lengths and header RLE unchanged. It is a bounded restoration pass, not an exhaustive search.

## Controlled Default experiment

The sample contains **365 distinct raw Deflate streams** (SHA-256) from 270 container paths in 25 fixture families: 127 valid top-level PngSuite streams and 238 deterministically sampled PNG/APNG, GZIP, ZIP and zlib streams, some sharing a container. Added samples have at most 128 KiB decoded data; one PngSuite stream has 196,864 decoded bytes. The probe searches only blocks with at most 128 KiB decoded data and 16,384 selected tokens. Every baseline was optimized as standalone Raw in Default mode, with no timeouts. In 191 streams, some bytes covered by an original match had become literals.

| Experiment | Comparison | Stream wins / losses | Physical bytes saved | Meaningful bits saved |
| --- | --- | ---: | ---: | ---: |
| Full shortest path, fixed trees | Original proofs versus selected proofs; same solver and final trees | 48 / 0 | 19 | 156 |
| Targeted restoration, fixed trees | Original proofs versus completed Default | 48 / 0 | 18 | 156 |
| Targeted restoration, with tree repricing | Original proofs versus completed Default | 48 / 0 | 46 | 372 |

These rows are alternative experiments; **do not add their savings**. Different parent bit positions explain the 19-versus-18 byte rounding despite identical bit totals.

The first row is the decisive causal comparison: only the source of legal match choices changes, with no tree search in either arm. All 48 winners restore original matches. The targeted variant also restores original matches in every winner, improves streams in 13 fixture families, and makes 16 of the 365 streams physically smaller, with headers, trees, block types and counts unchanged in all 365 outputs. The effects are small: they show a missing choice, not a large compression-ratio improvement.

### Runtime and wider search

Five warm repetitions of the targeted fixed-tree pass across all 365 streams took **50.6–52.4 ms**, median **50.7 ms**; the selected-proof control, which finds nothing to restore, took 33.0–35.0 ms, a median paired difference of **16.0 ms across the sample**. The targeted run visited 809,447 DP edges and no tree repricing. These prototype times include emission, reparse and checks but exclude loading, initial parsing and certificate construction. The full fixed-tree causal comparison took about 6.8 seconds per arm. The variant with tree repricing took about 2.76 seconds and priced 50 candidates; its extra savings may justify a separate experiment.

A broader four-round tree-feedback experiment produced 47 wins and one one-bit loss **relative to the equally strengthened selected-proof control**, net 17 bytes / 148 bits. Neither arm grew its completed Default parent. The loss on `exif2c08` shows that a bounded adaptive search can select a worse feedback path from a larger legal path set, so a future adaptive route must keep the ordinary control as a sibling candidate.

## Actual container validation

A test-only terminal hook ran the targeted fixed-tree pass through Columbo's real wrapper code, with the same binary, Default options and `Format::Auto` in both arms. The hook also recalculates the maximum emitted distance so the zlib window stays valid when a restored match needs more history than the selected parent.

The 11 cases were chosen to include observed winners and one zlib tie: a conformance check, not an estimate of whole-corpus yield. No run timed out.

| Container | Completed Default bytes | With restoration | Meaningful bits saved |
| --- | ---: | ---: | ---: |
| `PngSuite/basi4a16.png` | 2,828 | 2,827 | 3 |
| `PngSuite/f00n0g08.png` | 297 | 296 | 1 |
| `PngSuite/s33i3p04.png` | 377 | 377 | 3 |
| `PngSuite/tbbn2c16.png` | 2,029 | 2,028 | 4 |
| `apng-medium/clock.png` | 23,608 | 23,608 | 4 |
| `steam-shop_apng/happy.png` | 125,793 | 125,793 | 10 |
| `kensilverman-gz/kzipmix-20200115-bsd.tar.gz` | 52,833 | 52,832 | 12 |
| `kensilverman-zip/kwincheat.zip` | 17,496 | 17,494 | 17 |
| `kensilverman-zip/rekzip.zip` | 372 | 372 | 6 |
| `samplelib-zip/sample-project.zip` | 1,467 | 1,467 | 3 |
| `oxipng-zlib/XYB.icc.zlib` | 353 | 353 | 0 |

Ten complete containers improve in bytes or meaningful bits, totaling **6 bytes and 63 meaningful bits**; five are physically smaller. These savings overlap the raw experiments and are not additive.

Independent validation passed with zlib decoding, `pngcheck`, `gzip -t` and `unzip -tqq`, plus comparisons of decoded data, metadata, CRCs, strict Huffman compatibility and the advertised zlib window against actual emitted distances.

## Focused Max and replay checks

Ten selected PngSuite inputs were also run with Max and a 180-second timeout. Seven timed out after about 199 seconds of wall time; three completed. The targeted fixed-tree pass improves six of the ten parents, saving one physical byte and 14 meaningful bits. Two gains are on non-timeout parents:

| Stream | Completed Max | Original-proof restoration, same trees | Another existing Max run on the Max output |
| --- | ---: | ---: | ---: |
| `f00n0g08` | 1,740 bits | **1,737 bits** | 1,740 bits, byte-for-byte unchanged |
| `s33i3p04` | 1,854 bits | 1,851 bits | **1,848 bits** |

Neither replay timed out. `f00n0g08` is the strongest witness: an existing Max replay reaches a byte fixed point, while the original certificates still expose a three-bit improvement. On `s33i3p04` ordinary replay beats restoration, so it is not evidence of a gain beyond replay.

The other four targeted Max wins are timeout cases: `s39i3p04` saves two bits, `tbrn2c08` one, `tp0n2c08` three and `z03n2c08` two. A broad repricing variant saves 23 bits against the same ten Max parents, including generic extra-search effects; it was not recommended as the first route. Every Max and replay output was independently decoded and compared with its source.

## Verification and limits

Focused checks compared the shortest path against all permitted spellings in 1,344 small abstract price/interval cases and covered certificate clipping, wrong distances, interval overrun, absent symbols, distances through 32,768 and lengths 3 and 258. A stored-alignment case shows that four saved payload bits can disappear into padding; the complete incumbent is kept when the full result ties.

Production reuses the original parsed matches the raw optimizer already retains. Larger inputs, full-corpus Max coverage, and interaction with the [earlier header experiments](new-byte-saving-methods.md) remain unmeasured.

## Local reproduction and evidence

The private fixture corpus and generated payloads stay outside version control. Research files are in the ignored `work/` directory:

- Scripts: probe and focused checks (`work/permanent-proof-probe.rs`), snapshot runner (`work/run-permanent-proof-probe.py`), mixed-corpus preparation (`work/prepare-permanent-proof-mixed.py`), container runner (`work/run-permanent-proof-wrappers.py`) and container verifier (`work/verify-permanent-proof-wrappers.py`).
- Results in `work/permanent-proof-validation/`: aggregates (`summary.json`), source revision and hashes (`source-state.json`), deduplicated manifest (`mixed-manifest.json`), fixed-tree causal comparison (`frozen-full-validation.csv`), targeted fixed-tree results (`mixed-frozen.csv`), timing repetitions (`frozen-repeat.csv`), containers (`wrapper-validation.json`), Max parents (`max-validation.csv`), fixed-tree Max restoration (`max-frozen-validation.csv`), Max replay (`replay-validation.csv`), restored interval traces (`restoration-traces.txt`) and the final check log (`permanent-proof-final-checks.log`).

The evidence directory also keeps source, baseline and candidate raw streams, generated containers, script snapshots and an artifact hash manifest.

The snapshot runner builds the PngSuite sample; the mixed preparation script adds the other families, and a rerun populates their Default parents. `COLUMBO_PROOF_FAST=1` selects targeted restoration and `COLUMBO_PROOF_FROZEN=1` disables tree repricing. The recommended variant needs both (the runner's `--frozen` flag sets both); the full-DP causal control uses only `FROZEN`, and broad feedback unsets both. Each run overwrites the result CSV and output arms, so save them first. Use separate parent directories for Default and Max, because the mode flag does not invalidate cached parents. Cached parent timing fields measure loading, not optimization.
