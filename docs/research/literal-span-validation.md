<!-- SPDX-License-Identifier: MIT -->

# Advertised literal/length span: validation

Date: 6 September 2026. Baseline: `e4400b5`, including original-match
restoration, payload/header tradeoffs and the subsequent efficiency changes.
**Accepted and implemented** as R3 in
[the route catalogue](../routes-and-methods.md#route-gate-reference).
This is a new search dimension for Columbo, not a claim of worldwide novelty.

## Why adding unused entries can save bits

A dynamic Deflate header describes the literal/length and distance code lengths
as one compressed sequence. Advertising a few additional unused literal/length
entries inserts zeros at the alphabet boundary. That can make a repeat run
cheaper or change the best code-length Huffman tree enough to shorten the
header. The data codewords remain identical.

HLIT occupies five bits for any legal count from 257 through 286, stored as
count minus 257. Zero lengths
allocate no code space, and repeat codes may cross the literal/distance seam.
These properties are specified by [RFC 1951 §3.2.7](https://www.rfc-editor.org/rfc/rfc1951#section-3.2.7).

On the completed `basi0g04` stream, changing the advertised count from 269 to
277 extends the seam's zero run from three to eleven lengths. The stream drops
from 1,294 to 1,293 meaningful bits with unchanged payload codes. This win also
needs a different code-length tree; keeping the parent's CL tree misses it.

## Implementation and limits

The pass receives the completed payload/header-tradeoff result. It tries every
legal literal count from the minimum required by the nonzero lengths through
286, except the exact parent count. A previously padded parent may therefore
also select a shorter advertised span. The distance lengths and HDIST remain
unchanged. Each candidate gets the existing full header/RLE feedback search.
This exhausts the legal count choices when time and budget permit; it does not
prove a global optimum over all Huffman trees or RLE descriptions.

The selected stream is limited to 128 KiB compressed and decoded and 128 parsed
blocks. A parser model that discarded wire blocks is ineligible. Each invocation
has an independent ceiling of 1,024 header prices, with at most 29 alternative
counts per dynamic block. No repeat-threshold heuristic removes legal counts.

Timed work starts only before the soft deadline and polls the hard stop between
prices and blocks. Completed prices survive interruption. Max's mandatory
Default floor includes the same bounded pass independently of the optional
search deadline; its historical search seed remains unchanged. Max may also
price its final selected parent while timed work is still permitted.

The two terminal header passes share emission and alignment handling. R3 keeps
the complete R2 parent available, preserves every token, distance, payload
codeword and block boundary, and adds no replay. Stored padding is regenerated
at the resulting alignment. Decoded size, CRC-32, Adler-32 and the actual maximum
distance are checked by the common candidate builder. Strict Huffman compatibility
is retained. Complete bytes, then meaningful bits, decide every replacement.

## Controlled raw-stream results

The sample contains 365 distinct completed Default streams from 25 fixture
families: 127 distinct PngSuite streams and 238 additional deterministic samples.
The current baseline was freshly regenerated; all outputs matched the previous
payload-tradeoff results byte for byte, confirming that the efficiency changes
preserved these endpoints. None of the measured Default runs timed out.

Full enumeration priced 2,490 alternative spans. The integrated production pass
reproduced every probe output exactly. Full same-tree header repricing tied all
365 parents, so every gain below is attributable to the new count choice.

| Completed raw stream | Advertised count change | Meaningful bits saved | Physical bytes saved |
| --- | --- | ---: | ---: |
| `basi0g04` | 269 → 277 | 1 | 0 |
| `basi4a16` | 274 → 275 | 1 | 0 |
| `Project1.vbp` from `kskinmkr_src.zip` | 267 → 268 | 2 | 1 |
| **Total** | | **4** | **1** |

The last case changes 3,482 bits to 3,480, crossing a physical byte boundary.
It inserts only one zero before a nonzero distance length. This is evidence
that looking only for longer repeat runs is insufficient.

Two shortcut probes were rejected: shortest RLE under the parent's or ordinary
repricer's fixed CL tree missed all three gains; a candidate list based on seam
run lengths 3, 6, 7, 10 and 11 missed the byte-saving case. Full enumeration is
small enough to retain. The pass is composed after the previously selected
payload-costly swaps; these savings must not be added to that method's earlier
independent research probe.

All control and candidate streams were reparsed and checked for decoded identity
and strict compatibility. Python zlib independently decoded both probe arms and
the production results. The complete baseline remained available in every case;
there were no regressions.

## Actual containers

Thirteen original containers were optimized through the public API. PNG/APNG
checks included chunk CRCs, image/frame payloads and relevant metadata; GZIP and
ZIP checks included independent decoders and archive tests. Extracted streams
were inspected for meaningful bits and sufficient advertised windows.

| Container | Meaningful bits saved | Physical bytes saved |
| --- | ---: | ---: |
| PngSuite `basi0g04.png` | 1 | 0 |
| PngSuite `basi4a16.png` | 1 | 0 |
| APNG `clock.png` | 6 | 0 |
| ZIP `kskinmkr_src.zip` | 2 | 1 |
| Nine unchanged controls | 0 | 0 |
| **Total** | **10** | **1** |

The unchanged controls include GZIP and zlib references: compatibility was
verified, but no gain was measured on those examples. Raw and wrapper samples
overlap and must not be added. APNG frames are correlated observations. Exact
meaningful-bit totals come from parsing outputs, because the public API reports
removed bytes × 8 when a physical file shrinks.

## Max validation

A fixed-parent check covered 25 distinct Max references: ten recorded parents
from the previous payload-tradeoff validation and fifteen eligible distinct
streams found while inspecting eight current efficiency-review Max outputs. The latter were obtained with a
10-second optional allowance. Two references improved by one bit each, with
no byte changes. Both are variants of `basi4a16`, so they are correlated evidence,
not two independent workload families. Full same-tree header repricing ties
all 25 parents; no timed search is involved in these isolated gains.

Three fresh Max A/B pairs also used a 10-second optional allowance. A test-only
gate in a disposable source copy disabled only R3 for the baseline. Both arms
used the same remaining scheduler and source.

| Fresh raw reference | Baseline bits | New bits | Outcome |
| --- | ---: | ---: | --- |
| `basi0g04` | 1,294 | 1,293 | Both reached the deadline; bytes tie |
| `Project1.vbp` ZIP member | 3,482 | 3,480 | Both reached the deadline; 436 → 435 bytes |
| Small 8x8 PNG stream | 347 | 347 | Both completed in about 5.6 seconds |

A separate fixed-parent check reproduced every fresh output byte for byte;
ordinary same-tree repricing tied all three baselines. This isolates the
observed gains from timing differences. All retained outputs were independently
decoded. The bounded mandatory Default floor is covered by regression tests.
This is focused Max evidence, not a broad completed Max benchmark or a claim
that all recorded parents were unrestricted fixed points.

## Runtime and acceptance

Old/new order alternated in batches of eight cases. Timings measure the public
API call, excluding compilation, process startup and file I/O.

| Workload | Baseline | New pass | Difference |
| --- | ---: | ---: | ---: |
| 365 raw streams | 75.787 s | 76.284 s | +0.497 s / +0.66% |
| 13 actual containers | 14.200 s | 14.159 s | −0.041 s / effectively neutral |

These are single paired-run observations, not statistical guarantees. The
isolated full probe took 0.91 seconds, including the two rejected shortcut
experiments, parsing, emission, validation and I/O. The largest measured raw
case used 44 alternative-span prices.

The gains are small and rare. The method is retained as a bounded final
optimization because the savings reproduce beyond ordinary header repricing,
include a physical byte in a real archive, preserve completed incumbents and
have low measured overhead. Larger models, HDIST search, joint span/swap products
and alternative global header solvers remain separate research.

## Tests and reproducibility

The release suite passes 504 tests: 445 library, 46 CLI and 13 public API tests.
New coverage includes the frozen one-bit witness, unchanged payload codes,
complete trees, every legal advertised count against an explicit oracle,
non-minimal distance spans, exhausted budgets, interruption after a finished
winner, the HLIT=286 limit, all eight stored-block alignments, final PNG bit
accounting and the exact PNG spelling in Max's zero-optional-time Default floor.
Formatting and Clippy with warnings denied pass.

Local evidence is retained in the ignored `work/literal-span-validation/`
directory. `source-state.json` records the baseline hashes; `probe.csv`,
`moves.csv` and `spans.csv` record full enumeration. The fresh paired outputs,
wrapper metrics, independent checks, Max records and test logs are preserved.
Reproduction helpers are `work/run-literal-span-probe.py`,
`work/run-literal-span-paired.py`, `work/verify-literal-span-results.py` and
`work/prepare-literal-span-max.py`; they require the private fixture corpus.

An initial paired attempt linked a stale library. Output comparison caught it;
that attempt is quarantined under `stale-runner-check/` and contributes no
production measurements. The reported runner was rebuilt with `cargo build`
before linking and verified against all independent probe outputs.
