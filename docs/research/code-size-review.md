<!-- SPDX-License-Identifier: MIT -->

# Code size review

A dated log of changes that remove machine code without changing optimized
output or speed.

## Methods

Sizes are of the release executable and of the shipped executable that
`utils/build-distribution.sh` builds: the release profile on the pinned
nightly toolchain, which rebuilds the standard library to abort immediately on
panic, so it carries no unwind tables, backtrace printer, panic messages or
their source locations. (Before 22 October the script used a separate `distribution` profile
that only set `panic = "abort"`.) On macOS arm64 the `__TEXT` segment grows
in 16 KiB pages, so an executable's size moves in steps; the `__text` section,
from `size -m`, gives the exact change in code.

Function sizes come from an unstripped build: the gaps between `llvm-nm -n`
symbol addresses. Duplicated inlining comes from a build with full debug
information: the `DW_TAG_inlined_subroutine` entries in its dSYM, counted per
inlined function with their address ranges. Hot code comes from sampled
profiles of 33 stratified Default inputs and of 8 inputs in Max at 10 seconds.
Since the `d5897ec` pass, these builds use the shipped configuration with only
symbols or debug information added (the unstripped `__text` equals the
shipped one), and each candidate is measured alone: a build adds just that
change.

Equivalence uses the efficiency review's checks: public-API runs of 52 inputs
in Default with the 180-second budget and in zero-budget Max, CLI runs of the
33 Default inputs and of large APNG files compared byte for byte, and the
contributor-guide tests. Runtime is whole-file CPU over the 33 inputs, with the
builds alternating per input, and instructions retired, which do not depend on
the machine's load: medians of three runs of a Max run that completes its
search (`036-Clefable-0.png`, 150-second budget) and of `bored.png` and
`elephant-2.png` in Default.

## 23 October 2026, baseline `c6f01a6`

| Change | `__text` bytes | Shipped executable bytes |
| --- | ---: | ---: |
| Baseline | 909,776 | 993,120 |
| Visual mode removed | 852,544 | 942,032 |

`__text` shrank by 57,232 bytes (6.3%) and the shipped executable by 51,088
(5.1%, three 16 KiB pages); its release archive fell from 556,091 to 518,648
bytes (6.7%).

- **Visual mode removed.** `--visual` drew each stream's block map with its
  source and output rows. Its renderer (`src/progress/visual.rs`, 1,478
  lines) is gone, with the reporter branches that built its status text and
  the two block reports that only it read: the source stream's, taken
  before any route runs, and a copy of the block report in every candidate
  update. Verbose reports still print the final block plan, from the selected
  candidate or, when the source is kept, from the source. The public
  `Options::visual` field is removed.
- **No process spawning.** To size its cards, the renderer ran `stty size` on
  `/dev/tty`. It was Columbo's only `std::process::Command`, so it compiled
  the standard library's process-spawning code: removing only that call, with
  visual mode kept, saves 18,644 bytes of `__text` and 18,128 of executable.

Equivalence: the change touches only reporting, so the public-API runs were
not repeated. The 33 Default inputs matched byte for byte, as did `bored.png`,
`elephant-2.png` and the completing Max run. Verbose reports of nine PNG,
APNG, ZIP, GZIP and zlib inputs, in Default and in Max with a 10-second
budget, matched line for line with durations masked, except two Max runs that
reached their deadline (`Animated.png` and `Checked.playdate-pulp.zip`),
which start different routes from run to run: the baseline build differs from
itself in the same way. Rust tests, including the private corpus, passed in
release mode and the ordinary suite in debug mode, with documentation and
Python tests, a locked build, formatting and all-feature Clippy with warnings
denied. The tests of visual mode are gone; tests it shared with verbose mode
now cover verbose mode alone.

Runtime: 99.8 seconds of CPU for the baseline and 100.2 for the candidate on
the 33 inputs. Medians of three runs retired 105.28 and 105.35 billion
instructions on the completing Max run, 30.77 and 30.76 billion on
`bored.png`, and 37.45 and 37.46 billion on `elephant-2.png`.

Artifacts: `work/binary-size-review-20261023/`.

## 22 October 2026, baseline `d5897ec`

| Change | `__text` bytes | Shipped executable bytes |
| --- | ---: | ---: |
| Baseline | 963,940 | 1,058,784 |
| Alphabet, token and plan helpers out of line | 926,848 | 1,009,552 |
| One leaf-key sort for overflow repair and package merge | 917,844 | 1,009,536 |
| More helpers out of line, one token-list comparison | 907,836 | 993,120 |

`__text` shrank by 56,104 bytes (5.8%) and the shipped executable by 65,664
(6.2%, four 16 KiB pages); its release archive fell from 584,543 to 554,893
bytes (5.1%).

- **Helpers out of line.** 28 helpers were inlined at up to 26 call sites
  each; each loops over an alphabet, a code, or a token or plan list, plans a
  whole block or header, or is cold report or usage code. Among them are
  trimming code lengths (26 copies, up to 468 bytes), counting token
  frequencies (21), checking a code's shape (14 and 25), planning tokens and
  headers, CRC-32, and the format wrappers' fallible appends. Each is now one
  copy, and a call costs little beside its loop. Helpers priced per token or
  symbol (`EntropyScoutCode::token_bits`, `slide::price`,
  `restore::token_cost`, `code_or_unseen`) and `stored_block_bits`, which the
  boundary search calls in its inner loop, stay inline.
- **One token-list comparison.** The 19 comparisons of whole token lists each
  inlined the element loop, up to 204 bytes. They now call `same_tokens`,
  which keeps the shortcut that `Arc` comparisons of `Eq` values take: a list
  compared with itself is equal without a scan.
- **One leaf-key sort.** Overflow repair (`limit_generic_lengths`) and package
  merge each compiled the standard unstable sort for their own comparator.
  Both order leaves by frequency and then by a distinct symbol or leaf index,
  a total order, so the result does not depend on the sort algorithm. Each now
  packs that order into a `u64` key, and both sort through one instance. A
  test compares the key orders with the comparator sorts on random
  frequencies with many ties.

Measured and rejected:

- The same keys for the two-queue merges (6,576 bytes) and Defluff's builder
  (3,864 bytes): each tree needs a key buffer, and allocating it added 0.7%
  and 1.3% to the completing Max run's instructions, and zeroing one on the
  stack 0.2% and 0.3%. That run builds many trees.
- Out of line, these grew the code: the header's `token_bits` (4,448 bytes),
  `Representation::try_clone` (1,156), `new_token_candidate` (240) and
  `new_token_candidate_with_limit` (100).
- Build options from oxipng issues #571 and #806 that Columbo does not use:
  `-Zlocation-detail=none` is redundant with immediate abort (above); the
  arm64 linker ignores `-no_pie`; `opt-level = "s"` or `"z"` trade speed like
  `opt-level = 2`; profile-guided optimization would make the shipped code
  depend on a training corpus and could slow inputs outside it.

Not addressed: six hash-map types (12.5 KB of map code, each with its own
rehash), the standard library's reservation check inlined at 252 sites (about
55 bytes each, from 180 `try_reserve_exact` calls), and drop code.

Equivalence: API comparisons on the 52 inputs, in Default with the 180-second
budget and in zero-budget Max, matched in all 104 runs; the 33 Default inputs,
`bored.png`, `elephant-2.png` and the completing Max run matched byte for
byte. Rust tests, including the private corpus, passed in release and debug
modes, with Python tests, formatting and all-feature Clippy with warnings
denied; the only new test is the key-order test.

Runtime: 127.9 seconds of CPU for the baseline and 127.0 for the candidate on
the 33 inputs. Instructions retired fell from 155.96 to 155.81 billion on the
completing Max run, from 35.40 to 35.28 billion on `bored.png` and from 50.28
to 50.07 billion on `elephant-2.png`.

Artifacts: `work/binary-size-review-20261022/pass3/`.

## 22 October 2026, baseline `97fda1e`

| Change | `__text` bytes | Release executable bytes |
| --- | ---: | ---: |
| Baseline | 1,305,872 | 1,612,752 |
| Report coordinator state in vectors | 1,292,944 | 1,596,208 |
| Report and view maps as sorted vectors | 1,276,496 | 1,579,696 |
| CLI in the library | 1,268,368 | 1,579,648 |
| Candidate building, deadline and progress helpers out of line | 1,261,008 | 1,563,136 |
| Route reports and ZIP entry checks out of line | 1,257,680 | 1,563,136 |
| Boxed route spawns | 1,245,264 | 1,546,624 |
| Source-max worker through `spawn_route` | 1,243,472 | 1,546,608 |
| One Zopfli pseudo-frequency function | 1,242,192 | 1,546,608 |

`__text` shrank by 63,680 bytes (4.9%) and the release executable by 66,144
(4.1%). The shipped executable fell from 1,108,112 to 1,058,784 bytes (4.5%),
its `__text` from 1,018,436 to 963,940, and its release archive from 613,195
to 584,545 bytes.

- **Report coordinator state in vectors.** The coordinator kept
  `BTreeMap<usize, BTreeSet<u8>>`, `BTreeSet<u8>` and `BTreeSet<usize>`. Its
  producers are `u8` tags, now bit sets, and its stream IDs run from 1 to the
  file's stream count, so a vector indexed by ID holds each stream's completed
  producers and sealed flag. A stream without a completion record stays
  distinct from one with an empty set, as before.
- **Report and view maps as sorted vectors.** The verbose reports and visual
  views were `BTreeMap`s keyed by report ID. Every reader sorts by a key that
  ends in the report ID or only tests a condition, so a vector sorted by ID,
  searched by bisection, gives the same output. `BTreeMap` code for the five
  Columbo key and value types took 18 KB.
- **CLI in the library.** `main.rs` declared `cli` and `terminal` itself, so
  the terminal module (spinner, capability checks and formatting) was compiled
  into both the library and the program, and its nine tests ran twice. The
  library now holds the CLI behind a hidden `run_cli`, which the program calls.
- **Helpers out of line.** `build_candidate_from_plans_with_progress` had 19
  inlined copies, each also dropping the unused parse;
  `SearchDeadline::remaining` 18, `candidate_progress` 29,
  `RouteContext::report` and `report_optional` 36, and ZIP's
  `entry_is_deflate_stream` and `entry_is_optimizable` 53, up to 684 bytes
  each. Each runs once per candidate, stream or member.
- **Boxed route spawns.** Each of the 11 closures passed to `spawn_route`
  compiled the standard library's scoped-thread machinery and the
  cancellation guard; boxing the route leaves one copy per result type, of
  which there are three. The source-max worker, which repeated
  `spawn_route`'s body, now calls it.
- **One Zopfli pseudo-frequency function.** It was compiled for 286 and 30
  symbols; one slice-based copy now uses a 286-entry buffer for both.

Measured and rejected:

- `opt-level = 2`: 65,264 bytes less code, with Default and APNG times
  unchanged, but a Max run that completes its search took 3.4% more CPU
  (14.27 against 13.80 seconds, medians of six), and Max is deadline-bound.
- Boxing every scoped thread's closure: 5,504 bytes larger than boxing only
  the route spawns, since the other spawns each have their own result type.
- Exposing `terminal` as a hidden module instead of moving the CLI: 1,344
  bytes less saving, through a wider hidden API.
- `plural` out of line: 64 bytes.

Not addressed: the release build carries the standard library's backtrace
printer, about 153 KB of its code with `gimli`, `addr2line`, `rustc_demangle`
and `object`, which the default panic hook links whatever the panic strategy;
the shipped build already drops it. The compiler inlines the
drop of a `Candidate` at 217 sites (about 19 KB), and the balanced-tree move
collectors keep a copy for each alphabet (about 5 KB).

Equivalence: API comparisons on the 52 inputs, in Default with the 180-second
budget and in zero-budget Max, matched in all 104 runs; the 33 Default inputs,
four large APNG files and a completing Max run matched byte for byte. Rust
tests, including the private corpus, passed in release and debug modes, with
documentation and Python tests, a locked build, formatting and all-feature
Clippy with warnings denied; the test names are unchanged.

Runtime: 130.2 seconds of CPU for the baseline and 130.1 for the candidate on
the 33 inputs, and 15.48 and 15.34 seconds for the completing Max run. Large
APNG medians stayed within 1% while other work loaded the machine; on
`bored.png` the medians of eight runs retired 37.627 and 37.634 billion
instructions in 11.50 and 11.54 billion cycles.

Artifacts: `work/binary-size-review-20261022/`.

### Release builds abort on panic

Release builds now set `panic = "abort"`, as distributable builds did; only
debug builds unwind. The CLI has no panic recovery boundary, and Cargo builds
test harnesses to unwind, so every test still runs, including the file
safety's `catch_unwind` test. The release executable fell from 1,546,608 to
1,364,672 bytes (11.8%): `__eh_frame` from 101,680 to 29,176 bytes,
`__gcc_except_tab` from 36,864 to 5,736, and `__text` from 1,242,192 to
1,174,248. Outputs were identical on the 33 Default inputs, in 130.4 seconds
of CPU unwinding and 130.8 aborting; the completing Max run retired 1.7% fewer
instructions.

The separate `distribution` profile, which only set `panic = "abort"`, is
gone: `utils/build-distribution.sh` builds the release profile, and its
executable is byte-identical to the previous route's (SHA-256 `fffcf0ae…`). It
took 128.3 seconds on the 33 inputs and retired 5.6% fewer instructions on the
completing Max run than an ordinary release build, so benchmarks use it.

The script no longer passes `-Zlocation-detail=none`: immediate abort already
leaves out panic locations, and the shipped executable is byte-identical
without it. Standard-library rebuilding and immediate abort still need the
pinned nightly. On the current code, a stable release build is 1,364,672 bytes;
rebuilding the standard library with `panic_abort` gives 1,330,960, and
immediate abort 1,064,832. Re-signing ad hoc brings it to 1,058,784: the
linker's signature hashes 4 KiB pages and `codesign` hashes 16 KiB pages.

### Distribution build review

The script now trims paths with Cargo's `-Z trim-paths` (the `object`
setting) instead of five `--remap-path-prefix` flags. Those flags missed the
registry sources of the standard library's dependencies, which `-Z build-std`
compiles from the Cargo home; no such path reached the executable only because
immediate abort keeps no source locations. In a build that keeps panic
locations, the flags left 104 home-directory paths, which the path check
rejects; trimming leaves `/rustc/<commit>/…`, `index.crates.io-…/<crate>/…`
and `src/…`, and the check passes.

The path check no longer rewrites the executable, since it found nothing to
rewrite, so the script re-signs only to shrink the signature. Trimming changes
Cargo's symbol hashes, so functions and constants are laid out in another
order: the instructions and size are the same (SHA-256 `0f9cfa39…`), and
the executable is byte-identical when built from another directory. The 33
Default inputs matched byte for byte in 124.9 and 124.7 seconds of CPU, and
the completing Max run matched, with medians of three retiring 156.00 and
155.94 billion instructions.

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
313,856 bytes (16.3%). The shipped executable fell from 1,354,464 to 1,108,112
bytes (18.2%), its `__text` from 1,269,340 to 1,018,436, and its release
archive from 738,800 to 613,195 bytes.

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
(for example, 3,256 copies of `RawVecInner::deallocate`, 38 KB). The release
build also carries the standard library's backtrace printer and its unwind
tables (`__eh_frame`, 108 KB); the shipped build drops both.

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
