# Efficiency review

Initial review against `9825467` on 6 September 2026. The changes remove repeated
calculation and copying without changing the admitted search routes or their
tie-breaking rules.

## Changes

- **Dynamic-header RLE:** replace the repeated scan of 11–138-byte zero-run
  suffixes with a monotonic deque. Each endpoint enters and leaves the deque
  at most once. Equal prices retain the shortest repeat, and transitions keep
  their original order. A running count also replaces the allocated run-length
  array and its preliminary traversal.
- **Proven match splitting:** construct and price each legal submatch length
  once per solver call. For a 258-byte match this reduces those operations
  from 32,896 to 256. The shortest-path traversal, forbidden symbols, exact
  source-token preference and deadline-probe cadence remain unchanged. The
  lookup is a bounded stack array with no heap allocation.
- **ZIP processing:** borrow source payloads until a replacement exists; borrow
  the validated physical-order list; move terminal preflight metadata instead
  of cloning it. The terminal Store pass stages unchanged local records only
  when a replacement actually requires archive reconstruction.

## Focused measurements

Local macOS arm64, Rust 1.97.1, optimized builds with overflow checks enabled.
The baseline and changed functions used identical inputs and allocation/stop
helpers in a temporary harness. Values are medians of five batches; RLE batches
contained 20,000 calls and match-solver batches contained 2,000 calls. These
measure individual functions, not complete-file speedups.

| Workload | Before | After | Speedup |
| --- | ---: | ---: | ---: |
| RLE, 318 zero lengths | 63.74 µs | 8.32 µs | 7.66× |
| RLE, 318 mixed run lengths | 13.41 µs | 6.36 µs | 2.11× |
| RLE, 318 varying nonzero lengths | 2.46 µs | 2.16 µs | 1.14× |
| Match splitting, 12 bytes | 0.46 µs | 0.40 µs | 1.17× |
| Match splitting, 64 bytes | 30.83 µs | 5.99 µs | 5.15× |
| Match splitting, 258 bytes | 791.06 µs | 91.82 µs | 8.62× |

Full-file checks covered 16 inputs in Default and eight in Max: static PNG,
APNG, GZIP, ZIP, zlib and raw Deflate, including a generated 32 MiB stored ZIP.
All 24 comparisons produced byte-identical baseline and candidate outputs.
Python's zlib, gzip and zipfile decoders independently confirmed payload
identity; PNG checks also validated chunk CRCs and every image/frame stream.

Default CLI medians over three runs improved from 0.326 to 0.253 seconds for
the generated raw stream (1.29×), from 0.804 to 0.728 seconds for
`checkers_src.zip` (1.10×), and from 0.064 to 0.055 seconds for the stored ZIP
(1.16×). Other sampled files were near neutral or improved. These are local
measurements that include process startup and I/O. Max used a ten-second search
timeout and often consumed its deadline and grace, so its elapsed times do not
establish an algorithm speedup.

## Validation

- `cargo test --release`: 499 passing tests, including new exhaustive
  match-spelling checks, long-run RLE comparisons against a scanning oracle,
  and mixed changed/unchanged ZIP reconstruction in physical order.
- An additional 8,192 comparisons against the original match solver retained
  identical token spellings across all legal lengths, missing-code prices,
  forbidden symbols and the relaxed length-258 alias.
- `cargo clippy --all-targets -- -D warnings`, `cargo fmt -- --check` and
  `git diff --check` passed.
- The release executable remains 1,628,864 bytes on this host.

Temporary harnesses and detailed results are retained under the ignored
`work/efficiency-review/` directory; the original executable is retained under
`target/efficiency-review-baseline/`.

## Follow-up against `996c30e`, 9 September 2026

The earlier changes are committed in `8165633`. The follow-up inspected the
reorganized source and the new joint-tree and symbol-set searches, then
implemented two further improvements in shared code:

- **Greedy source-block merging:** retire absorbed slots in place rather than
  removing them from a vector and moving every later block. Two forward
  cursors retain the same left-to-right greedy choices; the emission pass
  skips retired slots. List maintenance is now linear. Payload and split-array
  copying during a merge are unchanged. Retired blocks are dropped immediately
  and their budget charges released at the same point as before.
- **Canonical match lengths:** replace a scan of up to 28 length families with
  a 256-byte lookup table built at compile time from the existing base lengths.
  The table preserves symbol 285 for length 258 and rejects every out-of-range
  input without allocating.

The source-list benchmark called the original and revised route directly on
identical arrays of one-byte stored blocks. It isolates list maintenance: the
public optimizer can bypass this route for wholly stored input, so these are
not whole-file speedups. Medians of five runs on the same macOS arm64 host:

| Source blocks | Before | After | Speedup |
| --- | ---: | ---: | ---: |
| 128 | 0.556 ms | 0.060 ms | 9.27× |
| 1,024 | 30.132 ms | 0.515 ms | 58.51× |
| 4,096 | 495.570 ms | 3.101 ms | 159.81× |

The original and optional working-block slots are both 3,072 bytes on this
host. Memory accounting uses the actual optional-slot size for portability.
Length encoding averaged 15.353 ns before and 1.055 ns after (14.55×), taking
the median of seven batches of 2,560,000 calls over all legal lengths.

Validation for the follow-up:

- `cargo test --release -- --include-ignored`: all 516 Rust tests passed,
  including the ten private-corpus regressions.
- All six Python utility tests passed; Clippy with warnings denied, formatting,
  and whitespace checks passed.
- 576 original/revised route comparisons retained identical complete plans
  and stop-check counts across strictness modes, alignments and interruptions.
- All 65,536 possible `u16` length inputs matched the original encoder.
- 32 API comparisons covered 16 PNG/APNG, GZIP, ZIP, zlib and raw inputs in
  Default and zero-budget Max. Output bytes and reported savings matched
  exactly, and independent decoders confirmed payload identity. Sampled
  whole-file timings were largely unchanged; these checks do not measure
  completed Max searches.
- The release executable remains 1,694,976 bytes on this host.

The follow-up harnesses and results are in `work/efficiency-review-sep9/`, with
the committed baseline executable in `target/efficiency-review-sep9-baseline/`.

## Follow-up against `625fb30`, 9 September 2026

The review of the newer joint-tree, symbol-set and paired-alphabet searches
found two further opportunities to avoid repeated work:

- **Joint-tree payload bounds:** update each suffix-table row one code length
  at a time. Its payload price is calculated once, and a shifted pair of slices
  visits only capacities where the length fits. This replaces the inner
  capacity check, repeated multiplication and indexed row lookups. Zero-length
  assignments copy the next row. The exact minima, table size, search order,
  conservative work charge and per-row stop checks are unchanged.
- **Symbol-set admission:** return as soon as the running affected-match count
  or decoded-byte total exceeds its existing limit. Both totals only increase,
  so scanning the remaining tokens cannot change rejection. The full initial
  scan charge is still reserved, preserving subsequent candidates' budgets.
  Candidates exactly at either limit remain eligible.

The paired-alphabet route already caches range estimates, prepared edges and
header kernels, and shares the established eight-alignment boundary graph.
The established coarse-to-fine split scorer also caches sampled positions.
Those caches and route policies are retained. Earlier RLE, match-length,
source-block-list and ZIP borrowing improvements remain present in this
baseline and are covered by the current test suite.

Focused measurements used extracted original/revised functions, identical
inputs and the same `SearchStop` implementation, with optimized builds,
overflow checks, fat LTO and one codegen unit on the same macOS arm64 host.
Values are medians of seven batches with alternating baseline/candidate order.
Payload-table batches used 100 calls (20,000 for the short case); complete
joint-solver batches used ten calls. Symbol admission used 20,000 calls on
8,192-token blocks. These measurements isolate the named operations.

| Workload | Before | After | Speedup |
| --- | ---: | ---: | ---: |
| Payload bounds, eight symbols, depth three | 0.466 µs | 0.266 µs | 1.75× |
| Payload bounds, 286 dense symbols, depth nine | 1,613.375 µs | 669.081 µs | 2.41× |
| Payload bounds, 286 sparse symbols, depth nine | 1,646.482 µs | 637.376 µs | 2.58× |
| Payload bounds, 32 distance positions, depth nine | 169.730 µs | 65.510 µs | 2.59× |
| Joint solver, depth three | 294.912 µs | 288.321 µs | 1.02× |
| Joint solver, depth six | 600.237 µs | 499.225 µs | 1.20× |
| Joint solver, depth nine | 67.378 ms | 66.327 ms | 1.02× |
| Symbol admission, accepted at match limit | 2.709 µs | 2.711 µs | 1.00× |
| Symbol admission, exceeds match limit | 2.705 µs | 0.122 µs | 22.21× |
| Symbol admission, exceeds byte limit | 2.653 µs | 0.056 µs | 47.50× |

The table-building improvement is larger than the complete joint-solver gain
because the state/edge search remains unchanged. Early rejection helps only
over-limit symbol candidates; the accepted-case scan was neutral in this
sample. Neither result is a whole-file speedup claim.

Validation for this follow-up:

- `cargo test --release -- --include-ignored`: all 522 Rust tests passed,
  including private-corpus regressions and both new tests.
- The new payload test checks 5,184 suffix/capacity minima against exhaustive
  assignment enumeration, including unavailable lengths, reserved symbols,
  zero-length eligibility, impossible capacities and `u32::MAX` frequencies.
- The new symbol-limit test exercises exact match and byte limits, rejection
  just beyond each limit, and an unaffected block. It checks emitted literals,
  source certificates, budget charges and stop-check counts.
- 1,800 original/revised payload-table comparisons, 5,120 joint-solver
  comparisons and 1,920 symbol-admission comparisons matched exactly. These
  include interruptions and limited budgets; complete solutions retain their
  lengths, RLE instructions and tie choices.
- All six Python utility tests, Clippy with warnings denied, formatting and
  whitespace checks passed.
- All 36 API comparisons retained exact output bytes and reported savings,
  with independent PNG/APNG, GZIP, ZIP, zlib and raw Deflate decoding. They
  cover 16 inputs in Default and zero-budget Max, plus four ten-second Max
  runs. One of those four Max runs completed; three reached their deadline,
  so they do not establish complete-search timing equivalence.
- Whole-file timings were largely unchanged. An initially slower Default
  `Load.png` sample (0.847 ms versus 1.273 ms) was followed by 30 alternating
  pairs with exact output checks: medians were 0.838 ms versus 0.807 ms, and
  the median paired candidate/baseline ratio was 0.993. The noisy initial
  sample remains in the recorded results.
- The baseline and candidate release executables are both 1,728,048 bytes.

Harness sources, logs and baseline/candidate API drivers are retained under
`work/efficiency-review-joint/`. The committed baseline build is retained under
`target/efficiency-review-joint-baseline/`.
