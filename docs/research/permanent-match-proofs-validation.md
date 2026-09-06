# Permanent original-match proofs: validation

5 September 2026. Baseline: `dbe148efbf60581f98d4d6bc2f4c0c58d21a1515`.
This validates a small, practical form of the first [alternative formulation](novel-deflate-formulations.md): retain original match certificates and use them to restore profitable matches in a completed candidate. The initial prototype ran in a disposable source snapshot. The bounded fixed-tree variant has subsequently been implemented in production, as recorded below.

**Decision: implement the bounded fixed-tree variant.** Original certificates produced improvements that an otherwise identical search using only the selected tokens could not produce. A targeted pass recovered most of the relevant benefit without changing any trees or headers. A general reversible graph optimizer remains untested and is not needed to obtain these initial gains.

## Production follow-up

The implementation is in [`src/deflate/restore.rs`](../../src/deflate/restore.rs), with terminal scheduling in [`optimize.rs`](../../src/deflate/optimize.rs). It runs in Default and Max, in strict and relaxed modes, without a new option. Max's mandatory Default comparison endpoint includes the pass; its historical search seed remains independent. Ordinary calls start it only while the soft deadline permits another route, then poll the hard stop during search. Completed interval repairs survive interruption, with untouched parent plans for the remainder.

The production work bounds are 128 KiB compressed and decoded per stream, 128 parsed blocks per layout, 16 Ki selected tokens per searched block, 4,096 decoded bytes per clipped certificate, and 2^20 DP edge evaluations per stream. Fallible allocation abandons optional plans safely. The pass preserves transmitted trees and headers, skips parent layouts whose redundant empty blocks were discarded by parsing, and records the actual output distance for wrapper reconstruction. It accepts only complete byte/meaningful-bit wins after emission and decoded-identity validation. [R1 in the route catalogue](../routes-and-methods.md#route-gate-reference) records the complete admission policy.

The production A/B comparison reproduced **all 365 reference outputs exactly**: 48 improved streams, 18 bytes and 156 meaningful bits saved, with no regressions. The 11 complete-container cases also reproduced the prototype's ten improvements, totaling six bytes and 63 meaningful bits. Independent zlib, PNG, GZIP, and ZIP checks passed, including advertised-window validation.

An initial sequential raw-sample timing was 78.25 seconds before versus 80.45 seconds after. A second comparison alternated the baseline/candidate order for each identical input in persistent processes: **78.87 seconds before versus 78.68 seconds after**. Two container repetitions took 11.025/11.015 seconds before and 10.975/10.978 seconds after. These runs do not establish a speedup; they found no repeatable material slowdown on this sample. Full-corpus and larger-input performance remain outside this measurement.

All **489 tests** pass (432 library, 46 CLI, 11 public API). New coverage checks the exact 1,740-to-1,737-bit Max witness, unchanged trees, certificate containment and distance, block clipping, absent codes, bounded interruption, stored alignment, and exhaustive comparison with 448 small abstract price problems. The public API test verifies the PNG byte saving, unchanged results with reporting enabled, and Max's improved Default floor even with zero optional-search time. `cargo fmt --check` and `cargo clippy --locked --all-targets -- -D warnings` also pass.

Production measurements and exact outputs are retained locally in [`work/restoration-implementation/`](../../work/restoration-implementation/), with [aggregate savings](../../work/restoration-implementation/summary.json), [raw bit accounting](../../work/restoration-implementation/raw-validation.json), [container bit accounting](../../work/restoration-implementation/wrappers-validation.json), and [the test log](../../work/restoration-full-tests.log). The following sections retain the original research controls and their limitations.

## Mechanism and scope

An original match certifies `(decoded start, decoded end, distance)`. Keep that certificate even if optimization replaces its match with literals. After later token/tree decisions, some certified matches can become cheaper again under the final tree. Recover those choices without searching history.

For example, the Default output of the stream in `PngSuite/f00n0g08.png` costs 1,865 meaningful bits. Restoring its original match covering decoded positions `[312, 330)` at distance 34 reduces it to 1,864 bits, with exactly the same header and trees. The corresponding complete PNG becomes one byte smaller because the change crosses a byte boundary.

The source gap is local, not total loss of the input. [`consider_proven_submatches`](../../src/deflate/search.rs) works from the current best tokens, `solve_proven_submatch` requires a match token, and `parsed_block_from_plan` and [`parsed_from_selected_plan`](../../src/deflate/stream.rs) build later states from selected tokens. Separate original lineages still exist. What those selected states do not expose is an independent certificate for a match that has become literals.

The prototype:

1. Extracts certificates from the original stream, coalescing only directly adjacent matches with the same distance, as already permitted by Columbo.
2. Clips certificates to the completed candidate's block boundaries. Original literal/stored bytes gain no certificates.
3. Finds the cheapest spelling using literal edges and submatches of lengths 3–258 wholly within a certificate, at its original distance. Missing Huffman symbols have infinite cost; match lengths use canonical encoding.
4. Emits and checks the entire stream, accepting only a strict improvement in `(physical bytes, meaningful bits)`. Stored-block padding is included.

The **targeted fixed-tree variant** examines only certificate ranges interrupted by literal gaps in the selected stream. It requires a strict payload saving that restores at least one match unavailable from the selected proofs. It keeps block types, boundaries, transmitted code lengths, and header RLE unchanged. It skips a clipped interval if its endpoints are not current token boundaries. This is a bounded restoration pass, not an exhaustive search of all future transformations.

## Controlled Default experiment

The sample contains **365 distinct raw Deflate streams**, deduplicated by SHA-256, from 270 container paths in 25 fixture families. It starts with 127 distinct valid top-level PngSuite streams, then adds 238 deterministically sampled PNG/APNG, GZIP, ZIP, and zlib streams. APNG frames and ZIP members can share a source container; these are not 365 independent workloads.

Added samples have at most 128 KiB decoded data. One original PngSuite stream has 196,864 decoded bytes. The probe leaves stored blocks unchanged and searches only blocks with at most 128 KiB decoded data and 16,384 selected tokens. Every baseline was optimized as standalone Raw in Default mode; none timed out. In 191 streams, some bytes covered by an original match had become literals.

| Experiment | Comparison | Stream wins / losses | Physical bytes saved | Meaningful bits saved |
| --- | --- | ---: | ---: | ---: |
| Full shortest path, fixed trees | Original proofs versus selected proofs; same solver and final trees | 48 / 0 | 19 | 156 |
| Targeted restoration, fixed trees | Original proofs versus completed Default | 48 / 0 | 18 | 156 |
| Targeted restoration, with tree repricing | Original proofs versus completed Default | 48 / 0 | 46 | 372 |

These rows are alternative experiments; **do not add their savings**. Full shortest-path search can also improve ordinary resegmentation available from selected proofs. Its control accounts for that effect. Different parent bit positions explain the 19-versus-18 byte rounding despite identical incremental bit totals.

The first row is the decisive causal comparison: only the source of legal match choices changes. There is no additional tree search in either arm. All 48 incremental winners restore original matches. The targeted variant also restores original matches in every winner, improves streams in 13 fixture families, and makes 16 of the 365 streams physically smaller. Parsed header/tree/RLE objects and block types/counts were compared and remained identical across all 365 targeted outputs.

The effects are small in absolute size. They demonstrate a missing choice, not a large compression-ratio improvement or an unrestricted optimum.

### Runtime and wider search

Five warm repetitions of the targeted fixed-tree pass across all 365 streams took **50.6–52.4 ms**, median **50.7 ms**. The selected-proof control, which finds nothing to restore, took 33.0–35.0 ms. Median paired difference was **16.0 ms across the sample**. The targeted run visited 809,447 DP edges and invoked no tree repricing.

These times include the pass's complete emission, reparse, decoded-identity checks, and scope checks. They exclude loading, initial source/parent parsing, and certificate construction. They are prototype timings, not measured end-to-end production overhead. The full fixed-tree causal comparison took about 6.8 seconds per arm because it solves every eligible position rather than only interrupted certificate ranges.

The targeted variant with tree repricing took about 2.76 seconds and priced 50 candidates. Its extra savings may justify a separate higher-effort experiment, but the cheaper fixed-tree variant is the better initial integration candidate.

A broader four-round tree-feedback experiment produced 47 wins and one one-bit loss **relative to the equally strengthened selected-proof control**, net 17 bytes / 148 bits. Neither arm grew its completed Default parent. The loss on `exif2c08` shows that enlarging the legal path set does not guarantee a better result from a bounded adaptive search: it can select a different feedback path. A future adaptive route must preserve the ordinary control as a sibling candidate. Fixed-tree shortest-path search avoids that feedback ambiguity.

## Actual container validation

A test-only terminal hook ran the targeted fixed-tree pass through Columbo's real wrapper code. Each baseline and candidate used the same binary, Default options, and `Format::Auto`; only the hook differed. The hook also recalculates the maximum emitted distance so zlib window advertisement remains valid when a restored match needs more history than the selected parent.

The 11 cases deliberately include observed winners and one zlib tie. They are a conformance check, not an unbiased estimate of whole-corpus yield. All runs completed without timeout.

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

Ten complete containers improve in bytes or meaningful bits, totaling **6 bytes and 63 meaningful bits**. Five are physically smaller. These savings overlap the raw experiments and must not be added to them.

Independent validation passed with zlib decoding, `pngcheck`, `gzip -t`, and `unzip -tqq`. Additional checks compared decoded frame/member data, ZIP member CRC/content and preserved metadata, PNG chunk CRCs and frame/control metadata, strict Huffman compatibility, and the advertised zlib window against actual emitted distances. The hook's reparsing is research scaffolding; it does not establish production runtime.

## Focused Max and replay checks

Ten selected PngSuite inputs were also run with Max and a 180-second timeout option. Seven reported timeout, with observed wall time around 199 seconds. Their results describe bounded runs, not completed Max search. Three completed normally.

The same targeted fixed-tree pass improves six of these ten parents, saving one physical byte and 14 meaningful bits. Two gains are on non-timeout parents:

| Stream | Completed Max | Original-proof restoration, same trees | Another existing Max run on the Max output |
| --- | ---: | ---: | ---: |
| `f00n0g08` | 1,740 bits | **1,737 bits** | 1,740 bits, byte-for-byte unchanged |
| `s33i3p04` | 1,854 bits | 1,851 bits | **1,848 bits** |

Neither replay timed out. `f00n0g08` is the strongest witness: an existing Max replay reaches a byte fixed point, while the original certificates still expose a three-bit improvement. `s33i3p04` is a useful counterexample to overclaiming: ordinary replay beats this restoration result. It should not be presented as evidence of a gain beyond replay.

The other four targeted Max wins are timeout cases: `s39i3p04` saves two bits, `tbrn2c08` one, `tp0n2c08` three, and `z03n2c08` two. A broad repricing variant saves 23 bits against the same ten Max parents, including generic extra-search effects. It is not the recommended initial route. Every Max/replay output was independently decoded and compared with its source.

## Verification and practical recommendation

Seven focused checks passed. They include exhaustive shortest-path comparison against all admitted spellings in 1,344 small abstract price/interval cases, certificate clipping across block boundaries, rejection of wrong distances and interval overrun, absent symbols, literal-only fragments, distances through 32,768, lengths 3 and 258, and unchanged frozen headers. A stored-alignment case demonstrates that four saved payload bits can disappear into padding; the complete incumbent is retained when the full result ties.

The production follow-up implements the recommended small terminal candidate generator using the original parsed matches already retained by the raw optimizer. It starts with interrupted certificate ranges and the final transmitted trees, keeps the complete parent available, propagates actual output distance requirements, and enforces deadline and work limits. There is no need to materialize a general graph or preserve every intermediate token stream for this initial form.

Larger inputs, full-corpus Max coverage, and interaction with the [earlier header experiments](new-byte-saving-methods.md) remain unmeasured. The original prototype uses test-only unchecked operations; production uses the separate bounded implementation above. No worldwide novelty claim is made: the demonstrated contribution is additional legal reachability within the inspected Columbo implementation.

## Local reproduction and evidence

The private fixture corpus and generated payloads remain outside version control. Local research files are in the ignored `work/` directory:

- [Probe and focused checks](../../work/permanent-proof-probe.rs), [snapshot runner](../../work/run-permanent-proof-probe.py), and [mixed-corpus preparation](../../work/prepare-permanent-proof-mixed.py).
- [Container runner](../../work/run-permanent-proof-wrappers.py) and [independent container verifier](../../work/verify-permanent-proof-wrappers.py).
- [Aggregate results](../../work/permanent-proof-validation/summary.json), [source revision/hashes](../../work/permanent-proof-validation/source-state.json), and [deduplicated manifest](../../work/permanent-proof-validation/mixed-manifest.json).
- [Fixed-tree causal comparison](../../work/permanent-proof-validation/frozen-full-validation.csv), [targeted fixed-tree results](../../work/permanent-proof-validation/mixed-frozen.csv), and [five timing repetitions](../../work/permanent-proof-validation/frozen-repeat.csv).
- [Complete-container results](../../work/permanent-proof-validation/wrapper-validation.json), [Max parents](../../work/permanent-proof-validation/max-validation.csv), [fixed-tree Max restoration](../../work/permanent-proof-validation/max-frozen-validation.csv), and [existing Max replay](../../work/permanent-proof-validation/replay-validation.csv).
- [Restored interval traces](../../work/permanent-proof-validation/restoration-traces.txt) and [final check log](../../work/permanent-proof-validation/permanent-proof-final-checks.log).

The evidence directory also preserves source/baseline/candidate raw streams, generated containers, script snapshots, and an artifact hash manifest, so validation does not depend on the disposable directory surviving. Source hashes were checked at the end of the prototype experiment, before implementation; all production files then matched the baseline.

The snapshot runner first builds the PngSuite sample. The mixed preparation script adds the other families; rerun against that snapshot to populate their Default parents. `COLUMBO_PROOF_FAST=1` selects targeted restoration and `COLUMBO_PROOF_FROZEN=1` disables tree repricing. Both are required for the recommended variant; use only `FROZEN` for the full-DP causal control. Unset both for broad feedback. The runner's `--frozen` flag sets both. Save each variant's outputs before another run, because the active result CSV and output arms are overwritten. Use separate parent directories for Default and Max: the mode flag does not invalidate existing cached parents. The Max experiment used fresh Max parents, then reused those exact outputs for restoration variants. Cached parent timing fields measure loading, not a fresh optimizer run.
