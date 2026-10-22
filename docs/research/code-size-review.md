<!-- SPDX-License-Identifier: MIT -->

# Code size review

A dated log of changes that remove duplicated machine code without changing
output or speed.

## Methods

Sizes are of the release executable and of the `distribution` profile's
executable, which aborts on panic. On macOS arm64 the `__TEXT` segment grows
in 16 KiB pages, so an executable's size moves in steps; the `__text` section,
from `size -m`, gives the exact change in code.

Function sizes come from an unstripped build: the gaps between `llvm-nm -n`
symbol addresses. Duplicated inlining comes from a build with full debug
information: the `DW_TAG_inlined_subroutine` entries in its dSYM, counted per
inlined function with their address ranges. Hot code comes from sampled
profiles of 33 stratified Default inputs and of 8 inputs in Max at 10 seconds.

Equivalence uses the efficiency review's checks: public-API runs of 52 inputs
in Default with the 180-second budget and in zero-budget Max, CLI runs of the
33 Default inputs and of large APNG files compared byte for byte, and the
contributor-guide tests. Runtime is whole-file CPU over the 33 inputs, with the
builds alternating per input.

## 20 October 2026, baseline `ef91c11`

| Change | `__text` bytes | Release executable bytes |
| --- | ---: | ---: |
| Baseline | 1,596,048 | 1,926,608 |
| Shared sorts | 1,446,608 | 1,761,392 |
| Stop checks out of line | 1,323,344 | 1,645,792 |
| Cold error constructor | 1,319,184 | 1,629,216 |
| Stable sort body once per element type | 1,313,744 | 1,629,248 |
| One environment-variable read | 1,311,312 | 1,629,248 |
| Deadline checks out of line | 1,305,872 | 1,612,752 |

In all, `__text` shrank by 290,176 bytes (18.2%) and the release executable by
313,856 bytes (16.3%). The `distribution` executable fell from 1,711,616 to
1,414,304 bytes (17.4%), its `__text` from 1,508,392 to 1,227,368, and its
maximally compressed ZIP from 905,916 to 768,138 bytes.

- **Shared sorts.** 252 standard-library sort functions took 255,916 bytes,
  16% of the code: each of about 40 call sites compiled its own copy of the
  sort for its element type and comparator, at 3 to 14 KB each. Profiles show
  sorting time only in Huffman construction (0.9% of Default samples, 1.5% in
  Max). Everything else now goes through `src/sort.rs`:
  - `sort_indices_unstable` runs the standard unstable sort on a `usize` index
    slice through a `dyn` comparator. The sort's choices depend only on the
    element type and the comparison results, so for an index slice it gives
    exactly the order of `sort_unstable_by`, ties included. GZIP member order,
    ZIP entry order and APNG frame grouping use it; the last already did,
    through a local copy.
  - `SortShared::sort_shared_by` and `sort_shared_by_key` sort an index slice
    with the standard stable sort, then permute the values. A stable order is
    unique, so they equal `sort_by` and `sort_by_key`. They replace unstable
    sorts only where no two distinct values compare equal: report and view
    keys end in a report id; image jobs end in their source order; the compact
    split's cuts come from `push_cut`, which keeps one cut per decoded offset;
    ranked splits key on both fields; entropy transitions, the adaptive split
    cache and length-exchange proposals hold one entry per boundary, token and
    literal pair; and the remaining full-value sorts order integers and
    tuples, whose equal values are identical. The sort body is compiled once
    per element type, and an index buffer that cannot be allocated falls back
    to an in-place insertion sort with the same stable order.
  - Huffman construction keeps its standard sorts; the two-queue and
    reverse-front merges, which differed only in the comparator's variant, now
    share one. The six sorts of cuts by decoded offset share one function. The
    compact split's sort of cuts by distance from the target keeps its own:
    cuts on either side of the target tie, so its order depends on the
    algorithm.
- **Stop checks out of line.** `SearchStop::reached` was always inlined: 221
  copies took 117 KB, up to 1,244 bytes each, and the deadline checks it calls
  (`expired`, `soft_expired`, `route_should_stop`) were inlined at hundreds
  more sites. Every
  timed check reads the clock, so a call costs little beside it.
  `reached`, the three checks and `can_start_route` are now out of line.
- **Cold error constructor.** `Error::with_kind`, generic over the message,
  copied its string conversion into 260 call sites; it is now cold and out of
  line, so each message type has one copy.
- **One environment-variable read.** The terminal checks' nine
  `env::var_os` calls had 35 inlined copies; one out-of-line helper serves
  them.

Measured and rejected:

- Keeping the `Never` and `Always` stops inline, with only the timed checks
  out of line: 10,816 more bytes of code and a page of executable, and no
  faster (130.5 against 129.4 seconds on the 33 inputs).
- Sending the progress reports' tiny insertion sorts through the shared sort:
  768 bytes larger.

Not addressed: allocation and drop code the compiler inlines at each `Vec`
(for example, 3,256 copies of `RawVecInner::deallocate`, 38 KB); the standard
library's backtrace symbolizer (`gimli`, `addr2line`, `rustc_demangle`, about
66 KB), which its panic hook links; and the release profile's unwind tables
(`__eh_frame`, 108 KB), which `distribution` avoids by aborting on panic.

Equivalence: tests compare both shared sorts with the standard ones on random
inputs with many ties, and the in-place fallback and permutation with the
stable order. API comparisons on the 52 inputs, in Default with the 180-second
budget and in zero-budget Max, matched in all 104 runs; the 33 Default inputs
and five large APNG files matched byte for byte. Rust tests, including the
private corpus, passed in release and debug modes, with documentation and
Python tests, a locked build, formatting and all-feature Clippy with warnings
denied.

Runtime: 129.2 seconds of CPU for the baseline and 129.4 for the candidate on
the 33 inputs; on three large APNG files, medians of four alternating runs were
56.2 and 56.35, 26.7 and 26.95, and 46.95 and 47.1 seconds, while other work
varied the machine's load.

Artifacts: `work/binary-size-review-20261020/`.
