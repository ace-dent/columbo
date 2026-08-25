<!-- SPDX-License-Identifier: MIT -->

# Columbo routes and optimization methods

This document describes the routes and methods implemented by the current
Columbo source tree. It is a source-grounded map, not a promise that every
optional route will run for every input: format, topology, work, memory, mode,
and deadline gates decide which complete candidates are built.

Columbo is a **post-optimizer**. It preserves the decoded byte stream and works
from literals, matches, blocks, and Huffman information already proved by the
source. It does not run a new LZ77 match finder, choose a new match distance, or
delegate recompression to Zopfli, libdeflate, or another compressor.

The routing ground truth lives primarily in:

| Area | Source |
| --- | --- |
| File and wrapper dispatch | `src/format/mod.rs`, `png.rs`, `zlib.rs`, `gzip.rs`, `zip.rs` |
| Raw-stream scheduling, floors, replays, and deadlines | `src/deflate/optimize.rs` |
| Stream grouping, merging, splitting, and boundary search | `src/deflate/stream.rs` |
| Token transformations and proven-submatch search | `src/deflate/search.rs` |
| Representation selection and route-local plan cache | `src/deflate/block.rs` |
| Huffman construction and dynamic-header search | `src/deflate/huffman.rs`, `header.rs` |
| deft4j-derived source route | `src/deflate/deft4j.rs` |

Thresholds below are implementation work bounds, not Deflate format limits.
Parser validity checks and the configurable input/decoded safety limits still
apply before or around every route. Corpus measurements validate a gate's cost
and a method's usefulness; they do not accept an output. Every retained
candidate is compared by exact complete-stream bytes and meaningful bits. The
bounded-depth terminal frontier deliberately uses mathematical feasibility and
deadline availability rather than a corpus-derived file-size band.

## Speed legend

| Indicator | Speed | Meaning |
| --- | --- | --- |
| 🟢 | Fast | Small, bounded work; normally close to linear in the relevant bytes, blocks, or tokens. |
| 🟡 | Medium | Rebuilds and compares several trees, token spellings, or block layouts. |
| 🔴 | Slow | Searches broad token, table, merge, replay, or boundary state, especially in max mode. |

The indicators are relative. A 🟢 method can still be noticeable on a very
large stream or a container with many streams. A method spanning two classes
uses the slower indicator.

## Overall pipeline

```mermaid
flowchart TD
    IN["Input file or raw stream"] --> DETECT["Explicit format or auto-detection"]
    DETECT --> PARSE["🟢 Parse wrapper and Deflate stream<br/>validate sizes and checksums"]
    PARSE --> STRICT{"Strict mode"}
    STRICT -- "Yes" --> NORMALIZE["🟢 Canonicalize length-258 aliases<br/>invalidate incompatible exact-reuse seeds"]
    STRICT -- "No" --> MODEL["Immutable parsed source model"]
    NORMALIZE --> MODEL

    MODEL --> ORIGINAL["🟢 Retain compatible source candidate"]
    MODEL --> SCHEDULE["Route scheduler<br/>mode + wrapper policy + gates + deadline"]
    SCHEDULE --> BLOCK["🟡 Block representation and header methods"]
    SCHEDULE --> TOKEN["🟡 Token-preserving and proven-token methods"]
    SCHEDULE --> STRUCTURE["🔴 Merge, group, split, and boundary methods"]
    SCHEDULE --> MAX["🔴 Max-only source, replay, and refinement lineages"]

    MODEL --> CACHE["🟢 Route-local canonical block-plan cache"]
    CACHE -. "reuse exact-verified completed kernels" .-> BLOCK
    CACHE -. "reuse within this planning run" .-> STRUCTURE

    ORIGINAL --> POOL["Complete candidate pool"]
    BLOCK --> POOL
    TOKEN --> POOL
    STRUCTURE --> POOL
    MAX --> POOL
    POOL --> VERIFY["🟢 Emit, reparse, and verify complete rewrites"]
    VERIFY --> ORDER["Byte count first<br/>then meaningful Deflate bits"]
    ORDER --> WRAP["🟢 Rebuild wrapper and metadata"]
    WRAP --> OUT["Selected output"]
```

Candidate ordering is strict and stable: fewer output bytes wins; at equal byte
length, fewer meaningful Deflate bits wins; an exact tie retains the earlier
candidate. Therefore a same-byte result that saves **one meaningful bit** is a
real win, and the CLI will write it unless `--dry-run` is used. A padding-only
change outside the meaningful Deflate bit count is not reported as a saving and
does not replace a file solely for compression.

For raw Deflate candidate selection, relaxed mode retains the original unless a
generated stream wins under that ordering. A wrapper can still be rebuilt for a
wrapper-level normalization, notably zlib FLEVEL. Strict mode may instead have
to emit a larger standards-compatible raw stream when the source uses a
noncanonical length-258 spelling or an incompatible Huffman alphabet.

## Routing decision tree

This section covers every condition that decides whether a top-level candidate
route runs. The diagrams use gate IDs; the tables immediately below give the
complete conditions. Lower-level block and boundary gates are listed separately
under [Inner stream-planner gates](#inner-stream-planner-gates).

| Term | Meaning in the routing diagrams |
| --- | --- |
| Candidate | One complete encoded alternative that can be compared with the source or another complete alternative. |
| Floor | A complete, safe baseline candidate secured before optional broader search. |
| Lineage | A sequence of transformations or replays starting from the same source or rewritten floor. |
| Fixed point | A completed replay that reproduces the same bytes and meaningful-bit count, proving that the same planner need not run again on that encoding. |
| `Complete` | Standalone raw/zlib and ZIP-default policy: finish the ordinary comparison route before max-only work for that stream. |
| `CompleteThenBounded` | Single unique PNG image-job policy: preserve the complete default result, then give max routes a bounded shared schedule. |
| `Shared` | Generic multi-stream and ZIP-max refinement policy: use a bounded per-stream heuristic floor so surrounding work retains time and decoded-size budget. |
| `SharedExact` | GZIP Max and uncached PNG-metadata Max policy: keep `Shared` scheduling, but retain the complete Default feedback endpoint before Max-only descendants. |
| `ApngDefault` | Multi-image APNG Default policy: run the full initial stream planner once, but leave additive reparse/replay, endpoint-proven, and compact feedback lineages to Max. |
| `ApngMax` | Multi-image APNG Max policy: retain shared per-stream scheduling while running the full Max route set against the simplified `ApngDefault` floor. Potentially useful direct deft4j work is now admitted by every Max floor policy. |
| `Established` | A caller already retains this complete, validated stream as its floor. Copy it into a new Max lineage without rebuilding Default; every selectable descendant is still reparsed and identity-checked. |

Within the same strictness, stripping, and safety policy, sufficient-time Max is
monotone over Default for every supported wrapper. Exact floor policies retain
the completed Default endpoint independently of Max's historical seed; APNG
Max runs a strict superset of its intentionally smaller per-frame Default
planner, with a complete file-level Default sibling for bounded models; and
container selection rejects any complete ZIP or PNG/APNG contender that
regresses either bytes or aggregate meaningful Deflate bits. A finite deadline
can still expire before an exact floor finishes on work outside the bounded
file races, so this is a sufficient-time quality guarantee rather than a claim
that equal short timeouts must always produce equal results.

### Wrapper and floor-policy routing

```mermaid
flowchart TD
    IN["Input"] --> EXPLICIT{"Format explicitly selected"}
    EXPLICIT -- "Yes" --> PARSER["Use selected parser"]
    EXPLICIT -- "No · auto" --> PNG{"PNG signature"}
    PNG -- "Yes" --> PPNG["PNG / APNG"]
    PNG -- "No" --> GZ{"GZIP signature"}
    GZ -- "Yes" --> PGZ["GZIP"]
    GZ -- "No" --> ZIP{"Recognizable ZIP structure"}
    ZIP -- "Yes" --> PZIP["ZIP"]
    ZIP -- "No" --> ZL{"Recognizable zlib method/window byte"}
    ZL -- "Yes" --> PZL["zlib"]
    ZL -- "No" --> PRAW["Raw Deflate"]

    PARSER --> POLICY{"Parsed format"}
    POLICY -- "Raw or zlib" --> COMPLETE
    POLICY -- "GZIP" --> SHARED
    POLICY -- "PNG / APNG" --> PPNGROUTES["PNG substreams"]
    POLICY -- "ZIP" --> ZMAX
    PRAW --> COMPLETE["Complete floor policy"]
    PZL --> COMPLETE
    PGZ --> SHARED["Default: Shared floor<br/>Max: SharedExact floor per serial member"]
    PPNG --> PPNGROUTES
    PPNGROUTES --> PMETA["Supported compressed metadata streams"]
    PMETA --> SHAREDMETA["Default: Shared floor<br/>Max: cached Complete or streaming SharedExact"]
    PPNGROUTES --> PJOBS{"Exactly one unique image job"}
    PJOBS -- "Yes" --> PMAX{"Bounded single-image Max"}
    PMAX -- "No" --> CTB["CompleteThenBounded"]
    PMAX -- "Yes" --> PRACE["Main CompleteThenBounded lineage<br/>+ eligible early transformed lineage"]
    PJOBS -- "No" --> PMULTI{"Mode and whole-file race bounds"}
    PMULTI -- "Default" --> APNGDEFAULT["ApngDefault per image stream"]
    PMULTI -- "Max · ≤8 MiB compressed<br/>≤64 MiB decoded · 2+ CPUs" --> PFILE["Race complete Default APNG<br/>against full-budget original-source Max"]
    PFILE --> PPAR["Max: up to 8 fixed worker lanes<br/>ApngMax floor per image stream"]
    PMULTI -- "Max · outside race bounds" --> SHAREDPNG["ApngMax floor per serial image job"]
    PZIP --> ZMAX{"--max"}
    ZMAX -- "No" --> ZDEFAULT["Complete default archive pass<br/>uniform members may use worker lanes"]
    ZMAX -- "Yes" --> ZBOUND{"At least 1 Deflate member; nonuniform archive ≤8 MiB<br/>optimizable decoded work ≤64 MiB"}
    ZBOUND -- "Yes" --> ZRACE["Complete Default archive + Established-source Max<br/>in parallel; 2+ bounded Default members may use worker lanes"]
    ZRACE --> ZREFINE["Refine completed Default archive<br/>and require byte+bit dominance"]
    ZBOUND -- "No" --> ZPHASE1["🟡 Phase 1 · complete Default archive<br/>uniform members may use worker lanes"]
    ZPHASE1 --> ZTIME{"Time remains"}
    ZTIME -- "No" --> ZWIN["Return phase-1 floor"]
    ZTIME -- "Yes" --> ZPHASE2["🔴 Phase 2 · refine finished archive<br/>Shared floors and actual remainder"]

```

`Shared` and `SharedExact` are per-stream floor and deadline policies. They do
**not** mean that separate members share a Huffman tree, candidate, or worker.

Every multi-stream wrapper has one authoritative file deadline. A proportional
child allowance is a scheduling slice, not a file timeout: the best complete
candidate is retained when that slice ends, and locally yielded PNG image jobs,
ZIP members, and concatenated GZIP members enter a weighted reclaim pass using
actual time left after the initial schedule. Only expiry of the shared file
deadline sets the public timeout result. Verbose and visual output produce the
CLI timeout warning, distinguish `search slice reached` from `deadline
reached`, and label a follow-up card `reclaimed time`; default output keeps its
compact per-file result to one line.

Reporting is observational. Default, verbose, and visual modes use the same
route gates, deadlines, candidate ordering, memory bounds, and worker
parallelism. Detailed stream updates are cached by the worker that produced
them. One file-level coordinator emits each completed physical stream as soon
as every earlier stream and every producer for that stream are final. Timed-out
jobs remain cached through reclaim passes; ZIP Max similarly waits for all of a
stream's archive lineages. Reporting never serializes or replaces optimization
work. Every interactive CLI mode shows an `optimizing` spinner and timeout
countdown on standard error during work. Verbose and Visual also show how many
physical streams have completed at least one optimizer report, even when an
earlier stream is temporarily holding those reports in source order. The
coordinator clears the spinner around each ordered report, then resumes it
until the complete file finishes.

| Wrapper route | Exact gate and scheduling behavior | Indicator |
| --- | --- | --- |
| Auto detection | In order: PNG signature, GZIP signature, recognizable ZIP structure, recognizable zlib method/window byte, then raw Deflate. | 🟢 |
| Raw / top-level zlib | Uses `Complete`; default and max route families run serially. | 🔴 |
| GZIP | Up to 16,384 concatenated members are preflighted before optimization, then run serially from smallest to largest compressed payload under one file deadline and cumulative decoded budget. Default members use `Shared`; Max members use `SharedExact`, retaining the complete Default endpoint before broader work. Initial slices are weighted by decoded work. Locally yielded members receive weighted reclaim passes from the actual remainder, and reconstruction restores physical source order. | 🟡 |
| PNG metadata | Compressed `zTXt`, compressed `iTXt`, and `iCCP` zlib streams use `Shared` in Default. A non-Max stream not selected for stripping and no larger than 4,096 bytes may receive a 100 ms probe, under a 64 MiB compressed-plus-decoded probe-work budget. Max precomputes and caches complete Default metadata floors when aggregate compressed metadata is ≤64 MiB. In a static PNG that bounded floor may overlap independent image work in every reporting mode; APNG avoids nested worker layers. If time remains after image work, reconstruction gives half of that remainder to an `Established` original-source Max branch, then continues the cached floor through its own `Established` branch with the actual remainder. The known exact decoded size validates both without charging the container budget twice, and either branch replaces the floor only when both metadata bytes and meaningful bits dominate it. Larger sets retain the streaming schedule and use `SharedExact`, so sufficient time still retains the complete Default endpoint without caching attacker-sized metadata. The unknown-unsafe-ancillary early return is the exception. | 🟡 |
| PNG / APNG image data | IDAT is always its own job. Exact-compressed duplicate fdAT frames share one optimization job only when both compressed bytes and exact decoded size match. Every IDAT/fdAT stream must decode to the exact IHDR/fcTL scanline size, including Adam7 passes. Exactly one job uses `CompleteThenBounded`. With two or more jobs, Default uses `ApngDefault`, retaining the complete initial planner while moving repeated replay, endpoint-proven, and compact-feedback work to Max; Max uses `ApngMax`, which keeps shared per-stream scheduling while running the full Max route set. One-block image streams remain direct-deft eligible, but that eligibility is now format-independent rather than an APNG exception. On a machine with at least two available CPUs, bounded APNG Max work (≤8 MiB compressed and ≤64 MiB decoded in aggregate) races a quiet complete Default file sibling against original-source Max with Max's full configured allowance. Max replaces that file floor only when it is no larger in bytes and has no more aggregate meaningful Deflate bits. The Max image branch uses up to eight fixed worker lanes; each lane runs its small-to-large slice serially. The initial image phase reserves 4% for container overhead. Default increases the reserve to 14% when a mandatory compressed-metadata pass still follows. Max has already cached that complete metadata floor, so optional metadata refinement consumes only time genuinely left by the dominant image search. Locally yielded image jobs then receive compressed-work-weighted reclaim passes while preserving any mandatory later-stream reserve. Work outside the whole-file race bounds retains the historical serial Max schedule. | 🔴 |
| PNG single-image Max | For ≤8 MiB compressed and ≤64 MiB decoded, the main `CompleteThenBounded` lineage races an early transformed lineage only when the source has a same-distance graph above the complete 512-match bound, or when a 2+-block stream of ≤768 KiB would otherwise serialize exact Default. The early lineage spends one fifth on an ordinary parent, then refines it through `Established`; the exact Default lineage remains the quality floor. | 🔴 |
| PNG decoded-equivalent frame reuse | After serial job optimization, checksum/size groups are decoded and byte-compared before the best compressed spelling is reused. Retained comparison data is capped at 32 MiB and comparison work at 64 MiB. | 🟡 |
| PNG unsafe ancillary fallback | If an unknown ancillary chunk is unsafe to copy and `--strip` does not remove it, Columbo validates every image stream and then preserves the complete source PNG. Metadata syntax was parsed and any completed metadata probe was validated, but an unprobed metadata payload is not definitively decoded on this early return. | 🟢 |
| PNG invalid-exporter repair | A color-type-6 RGBA PNG may carry one known indexed-conversion vestige: a non-empty, palette-sized `tRNS` after a valid suggested `PLTE` and before image data. PNG forbids `tRNS` for RGBA, so Columbo accepts only that structural signature and omits the chunk from every output. Bytes after `IEND` are outside the PNG datastream and are likewise discarded. Other invalid `tRNS` shapes remain errors; rewrite-sensitive unknown ancillary data also prevents this repair unless stripping is requested. | 🟢 |
| ZIP member scheduling | Unencrypted, nonempty method-8 entries are optimization jobs; unencrypted stored entries are validated but not Deflate-optimized, and encrypted entries are preserved without payload decoding. Default is largest-first and Max is small-first. Ordinary work uses up to eight balanced worker slices when at least eight optimizable members are similarly distributed, input is ≤8 MiB, and decoded work is ≤64 MiB. During a bounded nonuniform Max archive race, the mandatory Default sibling may use the same lanes for any set of at least two independent members; the direct Max sibling retains its normal schedule. Each exhaustive worker budgets only its own serial slice. After either the parallel or serial initial pass, locally yielded members receive weighted reclaim passes from the actual file remainder; byte length then meaningful Deflate bits select the retained result. Reconstruction emits local records in original physical order and central records in original directory order. | 🟡–🔴 |
| ZIP max archive lineages | An archive with at least one optimizable member races its complete Default archive against direct original-source Max when member work is nonuniform, input is ≤8 MiB, and total optimizable decoded work is ≤64 MiB. The direct branch uses the validated source as `Established`, so it does not rebuild the ordinary floor owned by the Default branch. With two or more members, bounded Default-member lanes complete that mandatory sibling sooner and leave the actual remainder for its distinct refinement. Larger or uniformly distributed archives avoid two simultaneous archive models: they complete Default, give original-source Max half of the actual remainder, then refine the completed floor with whatever time remains. Thus memory scheduling does not make the source deft4j basin permanently unreachable. Stored-only archives have no Deflate optimization jobs. A Max archive replaces its retained Default archive only when file bytes and aggregate meaningful Deflate bits are both no worse. | 🔴 |

### Raw-stream route tree

Any diamond labelled `+ G0` rechecks the soft route deadline and sibling
cancellation state at that point. Deterministic cleanup that has already been
admitted is explicitly labelled and may finish after the soft boundary.

#### Default and standalone max

```mermaid
flowchart TD
    RAW["Parsed raw Deflate stream"] --> MODE{"--max"}
    MODE -- "No" --> DEF["🟡 Ordinary planner + eligible proven endpoint<br/>at most 3 strictly improving replays"]
    DEF --> D1{"D1 eligible + G0"}
    D1 -- "Yes" --> D1RUN["🟡 Match-preserving feedback sibling"]
    D1 -- "No" --> D2
    D1RUN --> D2{"D2 eligible + G0"}
    D2 -- "Yes" --> D2RUN["🟡 Integrated multi-block feedback sibling"]
    D2 -- "No" --> D3
    D2RUN --> D3{"D3 · strict literal-only tree"}
    D3 -- "Yes" --> D3RUN["🟡 Up to 4 improving balanced-tree rounds"]
    D3 -- "No" --> PICK
    D3RUN --> PICK["Select byte/meaningful-bit winner"]

    MODE -- "Yes" --> POLICY{"Floor policy"}
    POLICY -- "Complete" --> CFLOOR["🟡 Mandatory ordinary comparison floor"]
    CFLOOR --> CM1{"M1 eligible + G0"}
    CM1 -- "Yes" --> CDEFT["🔴 Direct deft4j source candidate"]
    CM1 -- "No" --> LATE["Continue at late-max tree"]
    CDEFT --> LATE
    POLICY -- "CompleteThenBounded, Shared, SharedExact,<br/>ApngMax, or Established" --> BOUNDED["Continue at bounded-max tree"]
```

#### Bounded max

```mermaid
flowchart TD
    START["Max + bounded floor policy"] --> EST{"Established"}
    EST -- "Yes" --> EFLOOR["Retain caller-validated complete floor"]
    EST -- "No" --> M0{"M0 · prebuild floor"}
    M0 -- "Yes" --> PREFLOOR["Shared / ApngMax: bounded floor<br/>SharedExact / CompleteThenBounded: complete Default + retained Max seed<br/>eligible D1/D2 still check G0"]
    M0 -- "No" --> DEFER["Build floor inside bounded phase"]
    EFLOOR --> M1F
    PREFLOOR --> M1F["M1 · record deft eligibility"]
    DEFER --> M1F
    M1F --> M2F["M2 · record no-split eligibility"]
    M2F --> M3F["M3 · record compact proven-feedback eligibility"]
    M3F --> M4F["M4 · record compact balanced-tree eligibility"]
    M4F --> M5{"M5 · parallel work cap"}

    M5 -- "Fails" --> SERIAL["🟡 Standard serial phase<br/>floor + eligible M1/M2 under G0<br/>source max and M3 remain later"]
    M5 -- "Passes" --> BTYPE{"CompleteThenBounded"}
    BTYPE -- "No · Shared / SharedExact / ApngMax" --> SHARED["🔴 Bounded phase<br/>floor + eligible M1 under G0"]
    BTYPE -- "Yes" --> M6{"M6 · bounded PNG policy"}
    M6 -- "GenericParallel" --> GENERIC["🔴 Source max worker + floor lineage<br/>M1/M2 are ineligible by definition"]
    M6 -- "Standard" --> STANDARD["🔴 Floor + eligible M1/M2 under G0"]
    M6 -- "FloorExpansion" --> M10A{"M10a · dependency-first deft case<br/>M1 + G0"}
    M10A -- "Yes" --> PREDEP["🟡 Prebuild/refine deft prerequisite"]
    M10A -- "No" --> M8
    PREDEP --> M10CPRE{"M10c · dependency split parent admitted"}
    M10CPRE -- "Yes" --> PREDSPLIT["🟡 Deterministic compact split cleanup"]
    M10CPRE -- "No" --> M8
    PREDSPLIT --> M8{"M8 · compact initial source-max class"}
    M8 -- "No" --> EXPAND
    M8 -- "Yes" --> M9{"M9 · choose source-max / M3 token owner"}
    M9 --> EXPAND["🔴 FloorExpansion phase<br/>floor + eligible M1/M2<br/>source/M3 workers per M8/M9"]

    SERIAL --> M7
    SHARED --> M7
    GENERIC --> M7
    STANDARD --> M7
    EXPAND --> M7{"M7 · floor admitted + route window open"}
    M7 -- "No" --> POST
    M7 -- "Yes" --> DESC{"GenericParallel or FloorExpansion"}
    DESC -- "Yes" --> SEEDED["🔴 Established-floor max descendant"]
    DESC -- "No" --> MULTI{"Reparsed floor has multiple nonempty blocks"}
    MULTI -- "Yes" --> GROUP["🟡 Standalone bounded grouping"]
    MULTI -- "No" --> POST
    SEEDED --> POST
    GROUP --> POST["Bounded phase rejoined"]

    POST --> M3POST{"M3 eligible, not completed, + G0"}
    M3POST -- "Yes" --> M3RUN["🔴 Proven-feedback sibling<br/>+ header-aware spelling beam"]
    M3POST -- "No" --> S0
    M3RUN --> S0{"S0 · proven candidate ties floor bytes<br/>and wins meaningful bits"}
    S0 -- "Yes" --> SINTEGRATED["Later source max uses integrated compact order"]
    S0 -- "No" --> SORDINARY["Later source max uses ordinary order"]
    SINTEGRATED --> M10CSEED
    SORDINARY --> M10CSEED{"M10c · M4 flag + floor-seeded split parent admitted"}
    M10CSEED -- "Yes" --> SEEDSPLIT["🟡 Deterministic compact split cleanup"]
    M10CSEED -- "No" --> M4SEED
    SEEDSPLIT --> M4SEED{"M4 · resulting floor-seeded parent eligible"}
    M4SEED -- "Yes" --> M4SEEDRUN["🟡 Deterministic balanced-tree / feedback cleanup"]
    M4SEED -- "No" --> M2C
    M4SEEDRUN --> M2C{"M2c · changed no-split winner<br/>non-dominated + G0"}
    M2C -- "Yes" --> M2CRUN["🔴 One no-split continuation<br/>instead of simultaneous source max"]
    M2C -- "No" --> M10B
    M2CRUN --> M10B{"M10b · bounded deft / weak-split lineage"}
    M10B -- "Applicable" --> M10RUN["🔴 Timed refinements start under G0<br/>admitted compact cleanup may finish deterministically"]
    M10B -- "Not applicable" --> M10CPOST
    M10RUN --> M10CPOST{"M10c · normal/direct/refined parent admitted"}
    M10CPOST -- "Yes" --> POSTSPLIT["🟡 Deterministic compact split cleanup"]
    M10CPOST -- "No" --> LATE["Continue at late-max tree"]
    POSTSPLIT --> LATE
```

#### Late max sequence

```mermaid
flowchart TD
    START["From Complete or bounded max"] --> M11{"M11 · fragmented seed + G0"}
    M11 -- "Eligible" --> FRAG["🔴 Fragmented replay lineage"]
    M11 -- "Ineligible" --> M12
    FRAG --> M12{"M12 · earlier source-max state"}
    M12 -- "Completed" --> M4SRC
    M12 -- "Suppressed" --> M13
    M12 -- "Absent" --> M12G{"G0"}
    M12G -- "Yes" --> SMAX["🔴 Broad source-max route<br/>+ eligible forced-split escape"]
    M12G -- "No" --> M13
    SMAX --> M4SRC{"M4 · rewritten source-max parent eligible"}
    M4SRC -- "Yes" --> M4SRUN["🟡 Deterministic balanced-tree / feedback cleanup"]
    M4SRC -- "No" --> M13
    M4SRUN --> M13{"M13 · seed selectable, not stable,<br/>optional routes allowed, + G0"}
    M13 -- "Yes" --> SEEDRUN["🔴 Max planner on selected rewrite"]
    M13 -- "No" --> M4FINAL
    SEEDRUN --> M4FINAL{"M4 · final selected parent eligible + G0"}
    M4FINAL -- "Yes" --> QFINAL["🟡 Final balanced-tree cleanup"]
    M4FINAL -- "No" --> PICKMAX
    QFINAL --> PICKMAX["Select byte/meaningful-bit winner<br/>or relaxed raw source"]
```

Together the three diagrams expose every top-level gate. They are eligibility
and dependency maps: M1–M4 are first recorded as booleans for the bounded
phase, not started on those four arrows. Independent eligible branches may
overlap where M5 and the thread-site gates allow it, then rejoin in a fixed
comparison order.

### Route-gate reference

| Gate | Complete condition | Effect |
| --- | --- | --- |
| G0 · timed-route start | A new independent timed route starts only before its soft deadline and while no sibling cancellation flag is set. Active work that polls the hard deadline may finish until configured timeout + 10% + 1 second; a zero timeout has no grace. Multi-route bounded phases normally receive 4/5 of the then-remaining soft time, reserving 1/5 for follow-up. | Stops new timed starts; never skips parsing, validation, or a complete fallback. A currently active atomic price or candidate validation may finish before forwarding its completed incumbent. One already-admitted compact-split parent may use the file-level grace for the bounded coarse rescue described by M10c; a zero-grace container child and a route-window yield cannot multiply that allowance. |
| D1 · match-preserving default feedback | A complete non-exhaustive/default-floor pass; exactly one parsed block; at least one match; at most 4,000 tokens and 80,000 decoded bytes. | Runs proven-before-feedback and ordinary endpoint orders as independent complete candidates. Default mode and exact Max floors (`Complete`, `CompleteThenBounded`, and `SharedExact`) retain it independently of broader Max routes. |
| D2 · integrated default feedback | A complete non-exhaustive/default-floor pass; 2–4 parsed blocks; compressed stream at most 16 KiB; decoded stream at most 128 KiB; at most 16 Ki tokens total; at least one block satisfies D1's match/token/plain eligibility. | Runs a compact multi-block integrated-proven candidate in Default mode and exact Max floors (`Complete`, `CompleteThenBounded`, and `SharedExact`). Its second endpoint replay runs only if the first candidate did not beat the incumbent and hard time remains. |
| D3 · strict literal-only balanced tree | Strict complete Default/default-floor pass; source compressed bytes ≤8 KiB; exactly one nonempty dynamic source block; ≤4,096 tokens; no distance symbols. | Runs up to four strictly improving one-block balanced-tree rounds. Each round ranks bounded pair and quad moves with the ordinary header grid, exhaustively finalizes only the winning tree, reparses the complete candidate, and stops at a fixed point. This structurally targets the complete-distance-alphabet overhead that strict mode cannot omit. |
| M0 · bounded-floor prebuild | Max plus `Shared` or `ApngMax` always prebuilds a bounded floor; `SharedExact` always prebuilds the same Max seed plus its complete Default endpoint. Max plus `CompleteThenBounded` prebuilds when nonempty blocks ≤1, decoded bytes ≤768 KiB, decoded bytes >2 MiB, or more than 512 matches belong to source same-distance runs. `Established` directly retains the complete caller-provided floor; `Complete` builds its exact floor later in the serial tree. | Otherwise the multi-block single-image PNG floor is built inside the bounded phase. |
| M1 · deft4j source | Max. Any single nonempty fixed/dynamic block is eligible regardless of wrapper or floor policy. Every source list with at least two nonempty blocks is also eligible, including mixed or all-stored lists and lists longer than 128 blocks. An all-empty stream and a lone stored block are rejected because the direct graph has no table, token, boundary, or adjacent-merge operation that could improve them. Route-local byte accounting, the configured memory ceiling, and G0 bound actual work rather than an arbitrary source-list or state-iteration cap. | Adds the direct deft4j-derived source candidate. |
| M2 · no-split source | Max + `CompleteThenBounded`; compressed bytes ≤1 MiB; 2–128 nonempty blocks; every nonempty block fixed/dynamic. | Adds the bounded narrow source route. Its deft4j-derived per-block seed is followed by Columbo cumulative length-family pruning and adjacent source-order merges. Individual one-match pruning remains owned by source max, avoiding a duplicate walk that can starve later source blocks. If the emitted parent changes block boundaries or token spellings, one dependent Default refinement may use the file's existing hard grace; a header-only rewrite does not repeat planning. The 1 MiB/128-block pair bounds retained route-local models below the existing 64 MiB parallel class. |
| M2c · changed no-split continuation | M2 completed a strict score improvement over the source that changes tokens or block boundaries, no completed floor, floor-seeded, deft4j, source-max, or exact-Default sibling strictly beats it, and G0 remains open. | Re-enters the distinct no-split transformation once on the emitted parent. Equal-scoring siblings do not dominate a different encoding because their later endpoint order is not monotone. This dependency takes the source-max worker slot, preserving the existing worker/memory envelope; source max remains eligible later if time remains. |
| M3 · compact proven feedback | Max + `CompleteThenBounded`; exactly one parsed block; at least one match; ≤4,000 tokens; ≤80,000 decoded bytes. | Schedules the proven-first sibling in the initial phase under M8/M9, or tries it after that phase if it has not completed and G0 remains open. Its completed initial floor also receives the bounded header-aware spelling beam when the rewritten parent has ≤8,000 tokens and 2–128 matches. |
| M4 · compact balanced tree | The top-level flag requires max + `CompleteThenBounded`, compressed bytes ≤8 KiB, decoded bytes ≤128 KiB, exactly one nonempty dynamic source block, and ≤4,096 tokens in that block. Each rewritten parent must parse as exactly one dynamic block with ≤4,096 tokens and a retained source dynamic-tree seed. | On floor-seeded and source-max parents, admitted balanced-tree and feedback cleanup is deterministic and needs no new G0 start; the final selected-parent trial does require G0. Pair/quad moves are exact-priced independently on the literal/length and distance alphabets. Up to the four lowest-payload-delta moves from each side form at most sixteen paired candidates. Default admits paired work only at non-positive combined payload delta; Max permits the existing positive eighteen-bit margin. Matched Max streams omit only standalone literal pair pricing, while retaining distance and paired work. Every result survives only when complete header-plus-payload pricing wins. |
| M5 · route-level parallel cap | Max + bounded floor policy (`CompleteThenBounded`, `Shared`, `SharedExact`, `ApngMax`, or `Established`); compressed bytes ≤8 MiB; decoded bytes ≤64 MiB; estimated parsed model ≤64 MiB. | Allows broad independent candidate arenas to overlap. Failure selects Standard scheduling: floor, eligible M1/M2 routes, and dependent work remain serial, M8 is disabled, and source max remains eligible later; it does not promise an equivalent deferred floor-max descendant. |
| M6 · bounded PNG policy | Evaluated only for `CompleteThenBounded` after M5. `GenericParallel` when neither M1 nor M2 is eligible. Otherwise `FloorExpansion` when source has ≥2 nonempty blocks or the completed floor changes nonempty boundaries/token arrays. Otherwise `Standard`. | Chooses which independent source and floor lineages own the initial wall-clock window. |
| M7 · floor descendant | The floor must be selected (`strict` or strictly smaller than source) and its bounded route window must remain open. `GenericParallel` and `FloorExpansion` run an established-floor max descendant. Standard scheduling may instead run standalone bounded grouping when the reparsed floor has multiple nonempty blocks. | The established-floor max and standalone grouping descendants are mutually exclusive. If the max descendant rejoins as the strongest completed candidate without proving a fixed point, M7a continues that exact dependent state before weaker-parent refinements. |
| M7a · best-first floor continuation | A completed floor-seeded Max descendant strictly beats the retained normal floor, no direct-deft, no-split, or source-Max candidate beats it, it has not proved an exact Max fixed point, and G0 still permits a route start. | Reparses and continues the strongest dependent lineage first. In the bounded single-image work class, the independent deft4j refinement overlaps this continuation so a better encoded parent cannot permanently suppress a different planner topology. Its complete incumbent remains protected; other route families remain eligible with sufficient time. Candidate score, rather than a filename, size band, or measured elapsed time, determines priority. |
| M8 · initial compact source max | `FloorExpansion` + M5; compressed bytes ≤16 KiB. One nonempty block additionally needs decoded bytes ≤128 KiB, or ≤16 Ki tokens plus at least two repartition runs. For 2–4 nonempty blocks: decoded bytes ≤128 KiB × block count, ≤16 Ki tokens, and at least two repartition runs. | Makes source max eligible for the initial bounded worker phase. |
| M9 · compact source-token owner | Given M8: keep both roots when total tokens ≤2,048. In the 2,500–4,000-token band, source max alone owns the work when M3 exists and topology is distinct (a repartition run exists or decoded bytes >65,535). Otherwise source max requires at least two repartition runs or no M3 route; M3 is suppressed only when the single-owner source route actually starts. The 2,049–2,499 gap therefore normally favours M3 unless there are at least two repartition runs. | Avoids two long workers traversing substantially the same compact token graph. |
| S0 · source-max state order | A completed M3 candidate has the same byte length as the normal bounded floor but fewer meaningful bits. | Later source max begins with integrated compact proven order. A byte win, byte loss, or exact tie retains ordinary order; an already-running initial source max is unaffected. |
| M10a · dependency-first deft | `FloorExpansion` + M1 + G0, and either a unique one-nonempty-block floor descendant or a compact multi-block source: compressed ≤16 KiB, decoded ≤128 KiB, tokens ≤16 Ki, multiple nonempty blocks, and a non-dense repartition graph. Dense means every nonempty block has ≤4,000 tokens and repartition-run count ≥ nonempty-block count. | Prebuilds and ordinarily refines the direct deft parent before long independent beams; the compact multi-block case also completes its admitted split descendant. |
| M10b · bounded deft refinement | A bounded policy and G0. A weak deft signal requires `CompleteThenBounded`, a completed deft candidate, multiple nonempty blocks, and <2% meaningful-bit gain. Re-seeding from the normal floor requires that floor to strictly beat the source. In that weak floor-reseed branch, ordinary replay and terminal merge each recheck G0; terminal merge also requires a seed ≤1 MiB and no already-better M2 candidate. Refinement of an existing direct deft candidate uses the single M10b admission check. | Refines the direct or floor-seeded deft lineage while eligible independent work may run. If Default emits a genuinely new, smaller topology that also beats completed M2, that parent receives one no-split continuation. A changed direct-deft parent may then receive one full Max continuation; a header-only rewrite cannot trigger either. |
| M10c · compact split cleanup | A prepared parent is ≤16 KiB compressed and reparses to 2–4 blocks, none empty or stored, ≤128 KiB decoded, ≤16 Ki tokens, with at least one block having ≥16 tokens and ≥128 decoded bytes. A later refined parent must be nonduplicate and either beat every early parent or still pass G0. | Prices ordinary, floor-seeded, direct-deft, and admitted refined-deft parents. The timed route first covers every eighth cut with the ordinary Huffman planner, exhaustively finalizes only the winning alignment-independent suffix, then uses remaining hard time for the complete Max sweep. If an admitted parent reaches the hard boundary before its first cut, the single file-grace owner cheaply scores at most fourteen cuts on the largest eligible block and exactly prices only the best child pair. Every stage forwards a complete parent or descendant. The bounded phase records its completed parent exactly; when no cut or block-type transition won, it records the same-token/block endpoint as the deterministic fixed point so the follow-up phase does not repeat it. A structurally changed descendant remains eligible for another pass. |
| M11 · fragmented collection | Max; optional routes not suppressed; G0; at least 64 source blocks; encoded source size 16,000–100,000 bytes; no stored source block. Collection uses ≤4,096 tokens and ≤512 KiB plain per run and must produce fewer source blocks and 2–8 collected blocks. | Replays the independent fragmented seed until a metric fixed point or deadline; replay itself accepts 1–12 non-stored blocks. |
| M12 · later source max | Max. Reuse an earlier completed source-max candidate; skip when policy marked it suppressed; otherwise start the broad route only while G0 remains open. | Ensures original-source max precedes a rewritten seed without repeating an initial source-max route. |
| M13 · rewritten-seed refinement | Max; selected candidate is allowed (`strict` or it beats the source); no exact unexpired max-planner fixed point; optional routes not suppressed; G0. | Reparses and runs the max planner from the selected rewrite. |

Standalone `Complete` max mode builds the complete ordinary candidate with the
non-max stream planner and independently retains all eligible D1/D2/D3
feedback endpoints before continuing Max from the historical ordinary seed.
`SharedExact` applies the same separation to scheduled multi-stream work.
Ordinary/default and
compact-feedback lineages use at most three replays; broad, floor-seeded, and
fragmented Max lineages continue until a metric fixed point or deadline. Their
proof-derived ceiling is eight times the initially emitted byte length because
each byte length has only eight possible meaningful-bit residues and every
accepted replay strictly improves the byte/bit pair. Terminal merge uses at most two;
direct deft candidates use no replay until an explicitly scheduled refinement.
The initial no-split candidate may receive one Default refinement when it
exposes new boundaries or tokens. M2c can then run the distinct no-split
transformation once more when that changed parent is non-dominated; it is not
an unbounded replay loop.
A round is adopted only on a strict byte/meaningful-bit win. An unexpired
exhaustive round establishes a fixed point only when it reproduces the same
bytes and meaningful-bit count.

## Inner stream-planner gates

These gates are below the top-level scheduler. They control which block-layout
and boundary methods contribute to an already-started route.

| Method | Exact gate | Indicator |
| --- | --- | --- |
| All-stored repack | At least two blocks, every block stored, and total decoded bytes ≤64,000,000. Rechunk to payloads ≤65,535 bytes. | 🟢 |
| Regroup family | Floor work remains and either max is enabled or encoded source size is 16,000–100,000 bytes. | 🟡 |
| Source-aligned floor | Regroup allowed; 2–8 source blocks; no stored block. Each candidate range remains ≤250,000 tokens and ≤64,000,000 decoded bytes, and the optional composite model must stay within 48 MiB. | 🟡 |
| Whole-stream merged replay | Within an eligible source-aligned floor, the full merged range has ≥100,000 decoded bytes. A transformed fixed/dynamic seed no more than 256 bits above the best immediate recode may receive extended-floor replay at ≤50,000 tokens/10,000,000 decoded bytes and a four-step table ladder at ≤100,000 tokens/10,000,000 decoded bytes. | 🟡 |
| Greedy and bounded grouping | Ordinary regroup uses 9–128 blocks; the floor-seeded standalone form uses 2–128. Both reject a stored source block and require twice retained payload storage ≤48 MiB. Bounded grouping prices spans of at most 16 and also aborts if independently planning any source block selects a stored representation. | 🟡 |
| Collected and wide-collected layouts | Regroup allowed and >8 blocks. Three times retained payload storage must be ≤48 MiB and output is capped at 2,048 blocks. Bounded runs use ≤8,192 tokens/512 KiB plain. Wide collection is added in max or at ≥128 blocks, uses the normal 250,000-token/64,000,000-byte merge caps, and must differ from bounded collection. | 🟡 |
| Mandatory token floor | At most 128 blocks; the extended compact feedback floor applies only at ≤8 blocks. If same-distance repartition opportunities exist, proven resegmentation is integrated before feedback; otherwise proven search remains an independent endpoint lineage. | 🟡 |
| Selected-plan merge cleanup | Max; the searched source-order plan strictly replaced the fallback; 2–8 alignment-independent Huffman blocks; ≤8,192 tokens and ≤512 KiB plain. | 🟡 |
| Comparison-floor boundary reseat | Max + deadline remains; the secured comparison floor has 2–8 alignment-independent Huffman blocks, ≤8,192 tokens, and ≤512 KiB plain. Each adjacent union must satisfy the adaptive probe's histogram, ≥513-token, and ≥128-byte gates, and its discovered cut must differ from the existing boundary. Exactly price every completed replacement, retain only the strongest single reseat, and accept only a strict complete-plan bit win. | 🟡 |
| Forced-split boundary escape | Max + deadline remains; the secured comparison floor has 2–7 alignment-independent Huffman blocks, ≤8,192 tokens, and ≤512 KiB plain. Select the largest block with ≥513 tokens and ≥128 decoded bytes. Its adaptive histogram search must expose a sampled local minimum separated from the best basin by at least the greater of one seventh of the token span or 16 tokens. Force exactly that runner-up cut, exactly plan both children, and then try one existing adjacent-boundary reseat. Accept only a complete-plan bit win over the secured floor. | 🟡 |
| Per-source split | Otherwise falls back to whole-block search when tokens <16, decoded bytes <128, or default decoded bytes <32,768. Eligible blocks receive seven decoded-eighth probes. Max blocks ≤512 tokens also receive every interior 32-token anchor, capped with the source cut set at 32. | 🔴 |
| Exact inside-match siblings | Max always admits them. Default admits them when `token_count × 7 ≤ 32,768`. A source's own exact eighths still need ≥16 tokens, ≥128 decoded bytes, and in default ≥32,768 decoded bytes. With regroup and ≤8 sources, compact default as well as max also adds exact siblings for combined Huffman runs and their 2+-source subgroups; every combined interval still needs ≥16 tokens and ≥128 decoded bytes. | 🔴 |
| Default nested split | At least two exact-priced split candidates and runner-up cost within 64 bits of the best; refine only the larger child around its midpoint. Retain the snapped midpoint and, when the compact default inside-match gate passes, an exact inside-match sibling, for at most two nested cuts. | 🟡 |
| Adaptive split | Max; 513–250,000 tokens, at least 128 decoded bytes, a usable range histogram, and a cut not already present. At most 128 sample/probe positions; accept only after exact structural planning saves at least 32 bits. | 🟡 |
| Priority boundary graph | Max + established comparison floor + regroup + one or two blocks + ≤8,192 total tokens + ≥100,000 total decoded bytes + at least two same-distance repartition runs. The two-block case covers the first replay after a structural split while bounding the cut graph; broader block lists retain source-order priority. | 🔴 |
| Global boundary graph | Regroup allowed, deadline remains, and more than two distinct cuts exist. Building the composite and its range state must stay within the 48 MiB optional-model cap. Token-boundary cuts are retained before optional inside-match cuts; total cuts ≤2,048. | 🔴 |
| Boundary edge legality | Default same-source edges must touch that source's start or end; max may take middle ranges. Max cross-source edges stay ≤250,000 tokens/64,000,000 plain bytes. Stored edges cover whole sources and ≤65,535 bytes; mixed stored/Huffman ranges are rejected. Default cross-source Huffman ranges require regroup and ≤8 sources plus a touched source boundary, except ≤512-byte whole-source or all-fixed whole-source cases. Every legal edge is priced over all eight starting bit residues. | 🔴 |
| Adjacent merge | Merge search is enabled and either combined plain ≤512 bytes, max is enabled, or the route selected long-run Huffman merging; combined range ≤250,000 tokens/64,000,000 bytes. Exact fixed/fixed joining and identical-source-dynamic-tree reuse have cheaper dedicated candidates. | 🔴 |
| No-split route | Uses a complete direct fallback, narrow whole-block searches, and long-run adjacent merging; omits grouping, split probes, global boundary search, broad state queues, and unbounded iterative replay. A completed boundary/token rewrite may receive the single M2 Default refinement. One non-dominated changed winner may then receive the single M2c no-split continuation; a header-only result cannot trigger either step. | 🟡 |

### Inner token-search gates

| Method | Exact gate | Indicator |
| --- | --- | --- |
| Same-distance candidate | At least two tokens and one adjacent same-distance run; retain only a strict complete-block bit win. Non-exhaustive structural floors—including such floors inside max—use exact deficit pricing only when active shortened matches ≤16. Full max token search admits the wider bounded deficit table. | 🟢 |
| Mandatory block floor | ≤50,000 tokens and ≤10,000,000 decoded bytes. | 🟡 |
| Ordinary short-family floor | ≤250,000 tokens and ≤64,000,000 decoded bytes; price cumulative symbol ranges 260, 260–261, through 260–264. | 🟡 |
| Compact short-band floor | ≤12,000 tokens and ≤80,000 decoded bytes; price prefixes ending at 257, 258, 259, 260, 262, and 264. | 🟡 |
| Large no-split bands | Narrow/no-split block search only; decoded bytes ≥128,000 and tokens ≤80,000. Price cumulative endpoints 262, 265, 267, 269, and 270 plus the sparse 260–270-and-280 family. | 🟡 |
| All-literal endpoint | Decoded bytes ≤80,000, or ≤1,000,000 with at most 256 matches; the enclosing floor/search token and decoded-work bounds still apply. A frequency-only preflight rejects distant candidates before token expansion. | 🟡 |
| Match-expansion ladder | Begins only when matches exist; follows each seed for at most 12 token-expansion states, retaining strict winners and useful non-larger intermediates according to the route policy. | 🔴 |
| Match groups | Max; ≤250,000 tokens and ≤10,000,000 decoded bytes. For each of at most two deduplicated table seeds, price up to 20 individual groups and the first 2-, 3-, 4-, and 5-group prefixes. | 🔴 |
| Individual match pruning | Default: ≤20,000 tokens, ≤10,000,000 decoded bytes, and ≤32 matches. Max's dedicated individual-prune root is limited to ≤4,000 tokens; it shares exact state deduplication with the intact root. | 🔴 |
| Ordered token-state queue | Max; ≤12,000 tokens and ≤80,000 decoded bytes. Beam width 8, depth 5, at most 6 children per state, with a 256-bit admission margin. | 🔴 |
| Transformed-winner replay | Max; after a transformed token winner, at most four non-max child rounds. Outside block search, ordinary/compact lineages use at most three rounds; broad, floor-seeded, and fragmented Max lineages continue to their proof-derived metric fixed-point ceiling or deadline; terminal merge uses two; direct routes use zero until separately refined. M2 has the distinct Default refinement and at most one score-ordered M2c continuation described above. | 🔴 |
| Terminal tightening | Huffman plan ≤50,000 tokens and ≤10,000,000 decoded bytes. Extended floor replay uses the same bound; the four-pass table ladder allows ≤100,000 tokens at the same decoded bound. | 🟡 |

The stream planner secures a complete structural/token floor before optional
search. With more than eight source blocks, default can search the selected and
collected layouts but returns before the broad cross-source boundary graph. If
regroup is disabled, it returns after the per-source work. Max may continue into
the global graph until its deadline.

## Same-distance match-run normalization

🟢 A maximal sequence of adjacent matches using one semantic distance forms a
proven run. If it decodes `S` bytes, the minimum legal match count is
`ceil(S / 258)`:

- A run of at most 258 bytes becomes one canonical match.
- An exact multiple of 258 becomes all 258-byte matches.
- Other runs use a bounded deficit search under current Huffman costs.

Non-exhaustive structural work uses the exact cost search when at most 16
shortened matches are active; otherwise it emits a legal minimum-token fallback
requiring at most two shortened matches. This remains true for structural floors
inside max. The full max token search keeps the wider bounded cost table. The
distance never changes, the history window is never searched, and the complete
block is replanned before adoption.

Verbose mode reports source-only opportunity counters:

- Maximal same-distance runs.
- Matches and decoded bytes in those runs.
- Direct coalesces and longer repartition opportunities.
- Maximum removable match tokens.

The counter joins source token lists, so a source block boundary does not end a
reported opportunity. The transformation itself can cross that boundary only
after another route has legally merged the blocks into one candidate.

## Proven-submatch resegmentation and feedback

Within one existing match, 🔴 proven-submatch search constructs an acyclic
graph over decoded positions:

- A literal edge emits the already-known byte.
- A match edge emits 3–258 bytes at the source match's distance.
- No edge leaves the source match's decoded interval.

The source match wins estimated payload ties, but Columbo also prices
source-symbol-free and highest-used-length-symbol-elimination alternatives so a
small payload penalty can expose a larger header saving. Complete materialized
spellings are deduplicated, replanned, and exactly compared.

| Policy | Gate and breadth | Indicator |
| --- | --- | --- |
| Default targeted pass | Block ≤50,000 tokens and ≤10,000,000 decoded bytes with a match. Rank at most 8 targets, at most 2 per length symbol; one pass. | 🟡 |
| Max targeted pass | Block ≤250,000 tokens and ≤10,000,000 decoded bytes with a match. Rank at most 32 targets, at most 4 per length symbol. | 🔴 |
| Compact individual trials | ≤4,000 tokens and ≤80,000 decoded bytes. Price up to 4 default or 8 max individual siblings per rewrite family. | 🔴 |
| Full max graph | ≤12,000 tokens, ≤80,000 decoded bytes, and ≤512 matches. May target every eligible match. | 🔴 |
| Max repetition | Continue only after a strict token/bit winner, until stable, deadline, or 12 passes. Highest-symbol elimination is capped at 64 matches. | 🔴 |

Ranking favours high, rare, or expensive length symbols, positions within two
decoded bytes of a length-code transition, and positions within eight bytes of
a source split. Rare means frequency ≤2; expensive means at least nine code bits
or absent from the seed tree.

The ordinary planner stabilizes proven resegmentation as an independent
endpoint lineage so a locally smaller spelling cannot hide the ordinary replay
fixed point. Compact default and single-image PNG routes additionally retain
the proven-before-feedback and integrated-proven state orders described by D1,
D2, and M3.

Within Max's compact M3 route, 🔴 header-aware proven-spelling composition can
combine different choices across as many as eight ranked matches. Each menu
contains the source spelling and at most three frequency-distinct alternatives:
the current-tree payload minimum, the source-length-symbol-free path, and all
literals. The beam keeps at most 16 exact frequency/extra-bit states within 24
estimated payload bits of its best state. Only combinations rewriting at least
two matches are materialized, with a hard ceiling of 32 complete block prices.
The initial source has at most 4,000 tokens and 80,000 decoded bytes; the
rewritten parent has at most 8,000 tokens and 2–128 matches. Every emitted match
still uses its original proven distance, and the earlier complete M3 floor wins
every exact tie.

## Boundary polishing inside proven matches

🔴 A selected decoded cut may fall inside an existing match. Columbo retains
the snapped token-boundary candidate and may add an exact sibling at the decoded
target. A fragment of 3–258 bytes becomes a canonical match at the original
distance; a one- or two-byte fragment becomes known literals. No alternative
distance is invented.

Default exact siblings are work-bounded by `token_count × 7 ≤ 32,768`; max
admits them throughout its existing cut limits. This applies not only to
per-source eighths but, for regroupable lists of at most eight sources, to
combined Huffman runs and their subgroups. The boundary graph carries all eight
starting bit residues, so stored-block padding and Huffman representations are
compared at their real alignment. A combined run or subgroup must itself have
at least 16 tokens and 128 decoded bytes before those exact siblings are added.

The compact split floor is a separate bounded structural method. It prices
decoded-eighth splits without deep token or merge search and forwards a complete
best prefix when the deadline arrives. Its timed form is coarse-to-fine: a fast
ordinary-header sweep covers every structural cut, then only the winning suffix
after the last alignment-sensitive stored/original block receives exhaustive
header finalization. If hard time remains, the exact Max sweep follows, so the
unlimited route is never lost and the additional completed seed may still win.
If an admitted route reaches its hard boundary before the sweep begins, only
the child that owns file-level grace may run Columbo's bounded rescue. It
ordinary-prices every snapped/exact eighth of the largest eligible block and
exhaustively prices only the strongest topology—at most fourteen cheap probes
and one exact child pair. Earlier zero-grace container children and
route-window yields forward immediately, preventing per-stream multiplication.
Its parent gate is
compressed bytes ≤16 KiB, decoded bytes ≤128 KiB, 2–4 parsed blocks, no empty or
stored block, ≤16 Ki tokens total, and at least one block with ≥16 tokens and
≥128 decoded bytes. Completed parent identity is carried across the bounded
phase. A result that preserves the same shared token/plain arrays, block count,
and emitted block types is also a deterministic endpoint and is not immediately
replanned; an accepted split or type transition still exposes a distinct state
and remains eligible.

## Candidate reuse and duplicate-work control

Columbo has two bounded route-local caches, not a global route DAG or interval
cache. The block-plan cache stores completed alignment-independent fixed,
dynamic, and header results for identical token states within one planning run.

```mermaid
flowchart LR
    STATE["Block token state"] --> FP["🟢 Canonical fingerprint"]
    FP --> HIT{"Exact state and policy match"}
    HIT -- "Yes" --> REUSE["🟢 Reuse completed Huffman kernel"]
    HIT -- "No" --> PLAN["🟡 Complete deterministic planning"]
    PLAN --> INSERT{"Within 512 entries and 16 MiB token charge"}
    INSERT -- "Yes" --> STORE["🟢 Cache kernel"]
    INSERT -- "No" --> SAFE["Continue without insertion"]
    REUSE --> ALIGN["Layer on alignment, stored cost,<br/>and exact-source reuse"]
    STORE --> ALIGN
    SAFE --> ALIGN
```

Inside complete dynamic planning, the header-kernel cache uses a narrower key:
the exact trimmed literal/distance length sequences and header policy. It can
therefore reuse deterministic header work across different token orders while
each caller independently supplies its checked payload cost. The header cache
has a separate 512-entry cap and never stores incomplete work.

A hash hit is followed by exact verification of token spelling, symbol
frequencies, source dynamic-tree seed, and strict/default-or-max policy, making
hash collisions harmless. Timed work may consume a completed entry but never
publishes a partial plan. Separate route workers own separate caches; there is
no shared cache lock between them.

Other duplicate-work controls include:

- Reusing a completed default/floor candidate before max descendants.
- Using `Established` when a PNG worker already owns the complete transformed
  parent, so the descendant does not rebuild the same ordinary floor.
- Parsing a completed bounded floor once for its grouping and Max descendants.
- Skipping rewritten-seed replay after an exact unexpired max fixed point.
- Deduplicating exact compressed APNG frames with matching decoded geometry
  before optimization.
- Exact decoded comparison before cross-frame compressed-stream reuse.
- Racing `Established` source-Max ZIP work beside exact Default only for
  bounded nonuniform archives, without rebuilding an ordinary floor in the
  direct branch.
- Completing one outer ZIP lineage for uniform many-member archives and
  distributing independent members across balanced worker slices.
- Ordering distinct compact-split parents by their completed byte/bit score,
  while retaining every parent for a sufficiently long Max run.
- Selecting one compact source-token owner when proven and source-max beams
  would substantially overlap.
- Continuing a changed no-split parent only when it is not dominated by a
  completed sibling, and using the existing source-max worker slot rather than
  adding another simultaneous candidate arena.
- Pricing bounded grouping ranges once before selecting the least-cost ordered
  segmentation.

## Multithreading

Columbo does not parallelize files supplied to the CLI. GZIP members and
standalone raw/top-level-zlib planning remain algorithmically serial. A
structurally uniform many-member ZIP may parallelize independent members in
either mode. A bounded nonuniform ZIP Max race may also parallelize the
mandatory Default sibling's independent members. In other bounded `--max`
work, Columbo may overlap independent raw routes, APNG image streams,
static-PNG image and metadata-floor work, the two useful single-image PNG
lineages, or complete ZIP archive lineages. Every site has explicit
compressed, decoded, or model bounds and rejoins before wrapper
reconstruction.

Broad candidate-route overlap requires **all** of M5: max mode, a bounded floor
policy, compressed bytes ≤8 MiB, decoded bytes ≤64 MiB, and parsed-model
estimate ≤64 MiB. The estimate charges token storage, decoded bytes, and
per-block model overhead. Two narrower sites use their own tighter work caps:
compact-split follow-up and floor-seeded grouping range pricing. The wrapper
sites below have their own 8 MiB compressed and 64 MiB decoded bounds. There
is no persistent thread pool or CLI thread-count option.

| Internal thread site | When it runs | Work split | Indicator |
| --- | --- | --- | --- |
| ZIP archive lineages | Max; at least one optimizable member; nonuniform member distribution; input ≤8 MiB; total decoded bytes of optimizable entries ≤64 MiB. | One worker runs original-source Max from `Established` while the caller owns all ordinary Default work and later refines that finished archive. With at least two members, that bounded Default sibling may itself use up to eight member lanes. A complete Max archive wins only when it is no worse in both bytes and aggregate meaningful bits. | 🔴 |
| ZIP member lanes | In every reporting mode: either at least eight similarly distributed optimizable entries, or at least two entries in the bounded Default sibling of a nonuniform Max archive race; input ≤8 MiB; aggregate decoded bytes ≤64 MiB. | Up to eight balanced contiguous slices of the mode's entry order. Each worker owns cloned entry metadata/output. Exhaustive workers divide time using only their own serial slice; the mandatory Default-floor workers run ordinary routes. Locally yielded members rejoin a serial weighted reclaim pass under the shared file deadline. Reconstruction restores source order. | 🟡–🔴 |
| Single-image PNG lineages | Max; compressed ≤8 MiB; exact decoded image bytes ≤64 MiB; source either exceeds the 512-match complete graph or is a 2+-block ≤768 KiB serial-floor class. | One worker builds a quick ordinary parent and continues it through `Established`; the caller preserves exact Default and the direct bounded routes. | 🔴 |
| APNG whole-file lineages | Max; at least one fdAT frame; at least two available CPUs; aggregate image data ≤8 MiB compressed and ≤64 MiB decoded. | One quiet worker completes the APNG Default file pipeline while the caller gives original-source Max its full configured allowance. Both siblings own complete parsed/output models. The complete Max file is retained only when it dominates Default in bytes and meaningful bits; otherwise Default wins. | 🔴 |
| Multi-image PNG jobs | Max; at least two unique jobs; aggregate compressed ≤8 MiB; aggregate exact decoded bytes ≤64 MiB. | Inside the Max whole-file lineage, up to eight fixed lanes receive balanced contiguous slices of the small-to-large job order. Each lane runs its jobs serially with child-grace-aware proportional time. The initial phase retains 4% for container work. Because Max has already completed the mandatory compressed-metadata floor, optional metadata refinement does not reduce that image allowance. Locally yielded jobs then rejoin a serial weighted reclaim pass. | 🔴 |
| Initial bounded max phase | M5 plus the individual M1/M2/M8/M9 route gates and G0. | Up to four named workers for deft4j, no-split, initial source max, and initial proven feedback; caller builds/reuses the floor lineage. `Shared`, `SharedExact`, and `ApngMax` generally overlap any eligible direct deft work with their Max seed. | 🔴 |
| Generic single-image PNG phase | `CompleteThenBounded`, M5, G0, and M6=`GenericParallel`. | One source-max worker while the caller builds the floor→seeded-max lineage. | 🔴 |
| Bounded PNG follow-up | `CompleteThenBounded`, G0, and bounded refinement remains. A source-max worker additionally requires M5 and no completed or suppressing source-max route. A non-dominated M2c parent takes that worker slot instead. A compact-split worker requires at least one prepared weak-deft parent: multiple nonempty blocks, <2% direct deft gain, ≤16 KiB compressed, ≤128 KiB decoded, 2–4 nonempty non-stored blocks, ≤16 Ki tokens, and at least one block with ≥16 tokens and ≥128 decoded bytes. | The caller refines the deft lineage while compact split and exactly one of source max or the changed no-split dependency may run. This preserves the prior three-worker envelope. | 🔴 |
| Floor-seeded bounded grouping | The standalone grouping descendant from M7 is selected; grouping has 2–128 blocks, no stored source block, twice its retained payload storage is ≤48 MiB, and independently planning each source does not select a stored representation. | Up to four workers—further limited by available hardware threads and block count—price ranges beginning at assigned source blocks. Results return to source order, and the final best segmentation is selected serially. | 🟡 |
| CLI spinner | Every mode on interactive terminal stderr; detailed modes begin after format preflight. | Presentation-only thread: 300 ms delay, then roughly 500 ms updates. Every frame says `optimizing` and counts down the file timeout; Verbose and Visual atomically count physical streams that have completed at least one report as `checked/total`. Where terminal styling is allowed, the line is bold, its cursor uses 16-colour cyan, and the final three countdown seconds use 16-colour red. Detailed-mode spinners pause around ordered reports, resume during remaining work, and stop before the footer. It performs no optimization and does not affect scheduling or budgets. | 🟢 |

The ordinary stream planner deliberately prices bounded ranges serially; the
parallel range-pricing step is used only by the floor-seeded grouping
continuation. If only one hardware worker is available it falls back to serial
pricing.

Worker creation is optional. A failed archive, image-lane, route, or grouping
spawn runs equivalent work on the caller when appropriate or leaves the normal
later serial route eligible. Route siblings share
immutable parsed input but own their candidate arenas and caches. An error or
panic in a candidate-route sibling requests cooperative cancellation; all
successfully started scoped workers are joined before the error is returned or
the panic resumes. Grouping workers have no cancellation flag: allocation
failure marks the range-pricing step unsuccessful, and a panic is remembered
until every worker has joined. There is no force-kill.

Completed candidates are merged in a fixed route order, and equal contenders
retain the incumbent. The range-pricing result is deterministic because every
started range completes and results are restored by source index. With a finite
wall-clock deadline, however, operating-system scheduling can still change how
far a cooperative search progresses, so timed runs are not promised to be
bit-for-bit identical.

`Options` is immutable and reusable, so callers may run independent public API
calls on their own threads. That caller-created concurrency is separate from
the internal scheduler described here.

## Method catalogue

### Wrapper and scheduling methods

| Method or route | Current behavior | Indicator | Provenance |
| --- | --- | --- | --- |
| Original raw-candidate preservation | Keeps compatible raw Deflate source bytes as the relaxed fallback; strict repairs are the documented exception. Wrapper-level reconstruction or normalization can still change container bytes. | 🟢 | **Columbo** |
| One-bit output selection | At equal byte length, any positive meaningful-Deflate-bit saving—including one bit—sets `bits_saved` and permits a CLI write. Padding-only changes do not. | 🟢 | **Columbo** |
| Complete / CompleteThenBounded / Shared / SharedExact / ApngDefault / ApngMax / Established floors | Selects standalone, single-image PNG, heuristic multi-stream, exact multi-stream, multi-image APNG Default, multi-image APNG Max, or already-retained-parent deadline behavior without exposing wrapper policy as a public option. | 🟡 | **Columbo** |
| Multi-stream fair slices and reclaim | Gives independent PNG image streams, ZIP entries, and concatenated GZIP members bounded initial opportunities, retains every complete result, and redistributes actual remaining file time among locally yielded jobs. A child slice ending is reported separately and cannot by itself set the public file-timeout result. | 🟡–🔴 | **Columbo Rust** |
| Parallel and sequential ZIP max lineages | For nonuniform archives with optimizable work within 8 MiB input and 64 MiB decoded bounds, races exact Default and an `Established` original-source Max branch, then refines the completed Default archive. The direct branch does not rebuild ordinary work. The bounded Default sibling may use member lanes without multiplying the one file deadline or its grace. Larger or uniform archives serialize the same basins: complete Default, original-source Max for half the actual remainder, then floor refinement. A complete Max archive is selectable only when it dominates Default in both bytes and aggregate meaningful Deflate bits. | 🔴 | **Columbo** |
| Parallel ZIP members | Runs balanced slices of at least eight similarly distributed independent members without duplicating the archive lineage. The bounded Default sibling of a nonuniform Max archive race may also use lanes for two or more independent members so it leaves more actual remainder for refinement. Exhaustive workers receive slice-local schedules, locally yielded members receive a serial reclaim pass, and reconstruction restores physical and central source order. | 🟡–🔴 | **Columbo Rust** |
| PNG compressed-metadata scheduling | Gives small supported ancillary zlib streams a bounded early probe in non-Max runs. For bounded aggregate metadata, Max first caches complete Default floors so later image work cannot consume their opportunity. The dominant image search then owns the full remaining allowance. Reconstruction splits only the actual remainder between independent `Established` original-source and floor-seeded metadata descendants, retaining byte-and-bit dominance without double-charging decoded work. The unknown-unsafe-ancillary early return preserves the source first. | 🟡 | Original **Columbo C** probe; **Columbo Rust** floor preservation |
| PNG invalid-exporter repair | Removes only the specification-forbidden palette-shaped `tRNS` vestige from RGBA inputs and ignores bytes after the terminating `IEND`; every decoded image/frame stream is still validated. | 🟢 | **PNG specification / Columbo Rust** |
| APNG Default route budget | Runs the full initial planner once per multi-image stream, but reserves additive reparse/replay, endpoint-proven, and compact feedback lineages for Max. Static PNG, compressed metadata, GZIP, ZIP, raw, and zlib Default policies are unchanged. | 🟡 | **Columbo Rust** |
| APNG Max file floor | For bounded multi-image models on 2+ CPUs, races a quiet complete Default file pipeline against original-source Max with the full configured Max allowance. `ApngMax` keeps the shared image schedule; the direct deft4j route is independently available to potentially useful streams under every Max floor, including GZIP, ZIP, and compressed PNG metadata. GZIP and streaming PNG metadata use `SharedExact` for their Default guarantee. The Max artifact replaces the floor only when both its byte length and aggregate meaningful Deflate-bit count are no worse. | 🔴 | **Columbo Rust** |
| Exact PNG image geometry | Computes filtered IDAT/fdAT byte counts from IHDR/fcTL, including Adam7, and requires each zlib stream to decode to that exact size. | 🟢 | **PNG/APNG / Columbo** |
| Parallel PNG Max scheduling | Races distinct bounded single-image lineages and assigns independent APNG streams to at most eight fixed lanes, with explicit memory and wall-clock headroom followed by serial weighted reclaim for locally yielded image jobs. | 🔴 | **Columbo** |
| PNG duplicate and equivalent-frame reuse | Shares optimization for exact-compressed fdAT frames only when decoded geometry also matches, then uses bounded exact decoded comparison before reusing a better spelling across equivalent frames. IDAT remains a separate job. | 🟡 | **Columbo** |
| PNG packet coalescing and APNG renumbering | Coalesces IDAT/fdAT packetization, saving one 12-byte chunk envelope per removed packet, and rebuilds APNG sequence numbers. | 🟢 | **PNG/APNG / Columbo** |
| zlib effort-header normalization | Rebuilds RFC 1950 FLG with FLEVEL=3 and valid FCHECK while retaining CM/CINFO and rejecting preset dictionaries. This changes the API-selected wrapper even when the raw body is retained; the CLI still requires nonzero `bits_saved` before writing solely for compression. | 🟢 | **RFC 1950 / Columbo** |
| GZIP member scheduling | Preflights concatenated members, optimizes them serially small-first with decoded-work-weighted slices and reclaim under shared timeout/decoded budgets, reconstructs physical source order, and optionally strips metadata. | 🟡 | **RFC 1952 / Columbo** |

### Block, Huffman, and token methods

| Method or route | Current behavior | Indicator | Provenance |
| --- | --- | --- | --- |
| Strict length-258 normalization | Rewrites nonstandard symbol-284 length 258 and clears stale exact-source/tree reuse data. | 🟢 | **RFC 1951 / Columbo** |
| Empty-block and adjacent-stored preparation | Removes redundant empty blocks while retaining one for an otherwise empty stream, and joins adjacent stored payloads when their combined plain length is ≤65,535 and the three-view payload model stays ≤48 MiB. | 🟢 | **Columbo** |
| Exact source-block reuse | Reuses source bits when compatible. Stored originals require their original starting residue; fixed/dynamic originals are alignment-independent, and strict dynamic reuse requires compatible complete alphabets. When source and destination residues match, emission writes the partial edges and directly copies the aligned byte interior; other placements retain bounded bit copying. | 🟢 | **Columbo**; independently implemented portable emission hot path after reviewing general bitstream locality in **libdeflate** |
| Stored/fixed/dynamic comparison | Prices complete legal representations with real alignment, payload, and header cost. | 🟡 | **RFC 1951 / Columbo** |
| All-stored repack | Rechunks adjacent stored payload into legal 65,535-byte blocks without token search. | 🟢 | **RFC 1951 / Columbo** |
| Safe payload decode and emission hot paths | The validating parser materializes matches in a bounded 258-byte scratch buffer, doubles proven overlap periods, and updates its history ring in slices. It accounts retained payload bytes incrementally while preserving exact decoded and structural model limits. Profiled decode-table entries consume match codewords with their extra fields, while final emission packs each match codeword and extra field into one buffered write. | 🟢 | General fast-Deflate concepts reviewed in **libdeflate**; independent safe **Columbo** data structures and control flow |
| Strict/relaxed tree-shape policy | Strict completes required alphabets; relaxed permits explicitly supported empty or singleton forms and the compatibility length-258 alias. | 🟢 | **RFC 1951 / Columbo** |
| Route-local canonical plan cache | Reuses exact-verified completed fixed/dynamic/header kernels within one planning run. Its bounded structural fingerprint omits derived state for speed, but every hit still compares tokens, frequency tables, source tree, and policy exactly. | 🟢 | **Columbo** |
| Route-local header kernel cache | Reuses a completed zero-payload dynamic-header plan for exact trimmed literal/distance length sequences and header policy, including across different token orders. Hashes only select collision chains; full lengths and policy are verified, and each caller's payload cost is added independently with checked arithmetic. The cache is capped at 512 entries. | 🟢 | **Columbo** |
| Length-limited Huffman families | Default tries DeflOpt-heap and Columbo order-heap variants; max adds generic, Columbo/Defluff, exact Defluff, and deft4j Java-heap shapes under a capped cross-product. Generic variant zero uses an exact sorted two-front merge. Variant one consumes reverse-ordered equal-frequency branch runs; wrapped totals fall back to its exact heap. The two mixed-tie variants maintain both child orders in paired heaps with lazy removal. | 🔴 | **DeflOpt / Defluff / deft4j / Columbo** as labelled in source; independent **Columbo** hot paths |
| Reduced-depth payload-tree pair | Max only: builds exact Defluff literal/length and distance trees as one pair at maximum depths 10 and 9. These two candidates stay outside the existing tree-family cross-product, poll the route deadline, and replace the incumbent only when exact payload plus complete dynamic-header pricing wins. | 🟡 | Reduced-depth search dimension identified in **ECT**; independently written **Columbo** depth policy, pairing, gating, and exact integration |
| Bounded-depth terminal tree floor | After the main search completes, every dynamic block is eligible for one paired raw-count frontier. The first depth is derived from prefix-code capacity (`2^depth >= populated symbols`); every restricted ceiling through 14 is exactly priced, while the completed unrestricted 15-bit parent stays independent. Max applies exhaustive dynamic-header pricing to these candidates. Within the existing compact structural bound, Max also deduplicates each alphabet's feasible trees and exact-prices their complete cross-product, so the literal/length and distance ceilings may differ. Other streams retain the linear paired frontier and poll the hard stop between blocks. There is no corpus-trained size/token admission band, token feedback is never redirected, and only an exact whole-stream win is selectable. | 🟡 | Bounded-depth dimension identified in **7-Zip**; independently generalized **Columbo** feasibility rule, complete frontier, scheduling, and exact terminal integration |
| Compact payload-tree floor | After ordinary or Max search completes, compact Huffman streams receive one additive terminal pass comparing the raw bounded-depth frontier with independent Brotli fixed-point and classic Zopfli nearby-count seed families. Smoothed trees are exact-priced at paired maximum depths 15 through 9. A winner receives one bounded pair/quad tree-only closure and one final compact price. The route is bounded to 8 KiB compressed, 128 KiB decoded, eight blocks, and 4,096 tokens. It never changes tokens or redirects the parent search; exact whole-stream emission must strictly win. | 🟡 | Smoothers authored by **Google Brotli** (MIT) and **Google Zopfli** (Apache-2.0); bounded-depth dimension identified in **7-Zip/ECT**; independent bounded **Columbo** implementation and integration |
| Deflate decode tables | Payload canonical tables carry code lengths and extra-bit metadata in one independently laid-out 64-bit entry. Literal/length roots use ten bits and distance roots six: one bit above the balanced width implied by the 288- and 32-symbol maximum alphabets. Deflate's separate 19-symbol code-length alphabet has a seven-bit format maximum, so a checked 128-byte single-level table stores symbol and width directly and falls back to short probes when fewer than seven physical bits remain. | 🟢 | independent **Columbo** layouts and malformed-input handling; table dimensions identified while reviewing **7-Zip** and general decode-table tradeoffs in **libdeflate** |
| Planned bit emission | Final emission reserves the exactly priced output once, enforces that bit limit on every write, and therefore skips repeated capacity reservations. The safe 64-bit accumulator drains complete 32-bit little-endian words while retaining headroom for every legal write; aligned byte insertion flushes pending whole bytes first. Test and fixture writers without a plan retain ordinary fallible growth. | 🟢 | independent **Columbo** hot path, informed by the general buffered-emission theme in **libdeflate** |
| Header RLE search | Generates each distinct result of the eight repeat-code masks plus balanced/residual variants, then runs feedback, deft4j-pruned headers, and in max a fixed-cost shortest RLE parse. Trees for one spelling are scored exactly from its 19-symbol histogram; the initial DeflOpt trees feed the first feedback pass directly, and owned plans are materialized only for a new winner. | 🔴 | **DeflOpt / deft4j / Columbo** as labelled in source |
| Adjacency-quantized RLE tree | Max only: quantizes adjacent symbol frequencies into pseudo-weights, builds one RLE-friendly Huffman pair, then prices its complete header and payload against the original frequencies. | 🟡 | Zopfli/Turtledeflate-inspired pseudo-frequency shape; **Columbo** quantizer and exact scoring |
| Exact Zopfli RLE-friendly tree | Max only: preserves useful equal-count runs, averages eligible nearby-count strides, builds one paired literal/distance tree candidate, then prices its complete header and payload against the original frequencies. It remains one candidate outside the existing tree-family cross-product; Columbo does not rerun a separate pre-smoothing topology route. | 🟡 | **Zopfli** `OptimizeHuffmanForRle` behavior; independently written **Columbo** integration and exact scoring |
| Equal-frequency and code-length adjustments | Uses payload-neutral equal-frequency assignments and bounded length swaps when their complete header+payload cost wins. | 🔴 | **Columbo** |
| Same-distance normalization | Coalesces or cost-repartitions adjacent matches already using one proven distance. | 🟢 | **Columbo** |
| Cumulative short-length bands | Ordinary floors price five prefixes from symbol 260 through 260–264. Compact seeds use endpoints 257, 258, 259, 260, 262, and 264. Large no-split work uses endpoints 262, 265, 267, 269, and 270 plus one sparse family containing symbols 260–270 and 280. | 🟡 | **Columbo**, inspired by deft4j least-family pruning |
| Match-to-literal alternatives | Emits known literals for selected existing matches when complete repricing wins; never finds a new match. | 🔴 | **Columbo**; labelled primitives retain DeflOpt/deft4j attribution |
| Preflighted all-literals endpoint | Ordinary planning explicitly compares replacing every existing match by its known decoded literals. A one-tree estimate rejects distant candidates; dense blocks up to 80,000 decoded bytes and sparse-match blocks up to 1,000,000 bytes receive complete fixed/dynamic pricing. The expanded token vector is allocated only for a strict complete-block win. | 🟡 | all-literals comparison inspired by **libdeflate**; independent **Columbo** bounds, preflight, integration, and exact acceptance |
| Proven-submatch resegmentation | Searches literal/match paths wholly inside an already-proved match at its original distance. | 🔴 | **Columbo** |
| Header-aware proven-spelling composition | Max-only within compact M3: combines up to eight match menus in a 16-state frequency beam, deduplicates exact literal/distance frequencies plus match-extra bits, and exact-prices only multi-match combinations. | 🔴 | **Columbo** |
| Independent proven endpoint | Replays proven resegmentation after the ordinary candidate rather than allowing a local spelling to hide another fixed point. | 🔴 | **Columbo** |
| Match-preserving and integrated proven feedback | Adds the compact D1/D2/M3 state orders as independent complete candidates. | 🟡 | **Columbo** |
| Terminal tree tightening | Applies bounded feedback trees and one strictly improving existing-match-to-literal replay to eligible final Huffman blocks. | 🟡 | DeflOpt-inspired primitive; **Columbo** scheduling |
| Table replay ladder | Follows the selected table through up to four token-expansion/rebuild states, retaining the best intermediate. | 🟡 | **Columbo** |
| Compact pair lengthening | On one dynamic block, independently searches both data alphabets: shorten one length-L code to L-1 and lengthen two other length-L codes to L+1. This preserves the Kraft sum; bounded frequency-ranked candidates may spend at most 18 payload bits and are accepted only when the complete header-plus-payload result wins. | 🟡 | **Columbo Rust** |
| Compact quad lengthening | On one dynamic block, independently searches both data alphabets with the bounded one-shorter/four-longer equal-Kraft family, permits at most 18 extra payload bits, and prices complete header plus payload. | 🟡 | Original **Columbo C** literal/length method; **Columbo Rust** distance extension |
| Paired balanced-tree move | Retains at most four lowest-payload-delta moves per data alphabet and exactly prices their at-most-sixteen-member cross-product, including the dynamic-header RLE run across the alphabet boundary. Default requires a non-positive combined payload delta; Max permits up to 18 extra payload bits. | 🟡 | **Columbo Rust** |
| Compact balanced-tree cleanup | Selects the best standalone or paired tree, exhaustively finalizes only Default's winning tree, and may compose Max's tree result with compact proven feedback. The structural strict literal-only Default route runs at most four improving rounds; matched Max streams omit standalone literal pair pricing but retain distance and paired candidates. | 🟡 | **Columbo** scheduling |

### Structural and max routes

| Method or route | Current behavior | Indicator | Provenance |
| --- | --- | --- | --- |
| Fixed/fixed 10-bit join | Removes one fixed end code and the next fixed header when adjacent selected blocks are fixed. | 🟢 | **DeflOpt** |
| Shared source dynamic tree | Reuses an identical source dynamic tree over concatenated tokens and removes one header when cheaper. | 🟢 | **Columbo** |
| Source-aligned floor | Prices all contiguous ranges over at most eight sources—36 ranges at eight blocks—and chooses the best source-boundary segmentation. | 🟡 | **Columbo** |
| Whole-stream recode and replay | Gives a large complete merged range one extended token-preserving floor and a near-seed table replay before a shared container deadline can starve it. | 🟡 | **Columbo** |
| Greedy adjacent grouping | Carries a strictly winning merged block into the next adjacent comparison. | 🟡 | Original **Columbo C** block-list behavior |
| Bounded grouping with ordered selection | Prices each legal span up to 16 once, then selects the least-cost complete ordered segmentation; optional floor-seeded range pricing uses at most four workers. | 🟡 | **Columbo** |
| Bounded and wide collection | Folds long Huffman runs under bounded or broad token/plain limits before replay. | 🟡 | **Columbo** |
| Selected-plan merge cleanup | Prices every adjacent pair once on a newly selected compact Huffman plan without reparsing the whole stream. | 🟡 | **Columbo** |
| Comparison-floor boundary reseat | Before broad source search, joins each adjacent pair in a bounded Max comparison floor only for adaptive cut discovery, exactly prices the replacement, and retains at most the strongest single strict win. | 🟡 | **Columbo** |
| Forced-split boundary escape | In a compact Max comparison floor, retains one well-separated runner-up from the existing adaptive probes, forces that single cut, and tries one adjacent-boundary reseat. The original complete floor remains the exact acceptance bound. | 🟡 | Turtledeflate-inspired pushed-split concept; **Columbo** bounded implementation |
| Source split family | Prices seven decoded-eighth cuts, compact max 32-token anchors, child search, and a bounded default runner-up midpoint with an optional exact inside-match sibling. | 🔴 | **Columbo** |
| Inside-match boundary polishing | Adds exact decoded-boundary siblings inside proven matches and keeps the original distance. | 🔴 | **Columbo** |
| Adaptive split probe | Samples, smooths, and narrows a bounded histogram search, then requires a 32-bit exact win. | 🟡 | Turtledeflate-inspired search shape; **Columbo** implementation |
| Priority boundary graph | When a complete incumbent already protects source-order work, prices the independent global graph first for a compact one-block source or its first two-block replay. This recovers cut combinations before repeated per-block search can consume a finite Max budget; sufficient-time route coverage is unchanged. | 🔴 | **Columbo** |
| Global boundary graph | Prices legal block ranges across cut anchors and eight bit residues, then chooses a complete path. | 🔴 | **Columbo** |
| Dedicated deft4j source route | Runs the reconstructed deft4j source-state graph under M1. Every wrapper shares the same topology admission. Queue states and source-list working copies are charged to a 128 MiB route arena; there is no independent 4,096-state or 128-block ceiling, so elapsed time—not an unrelated iteration cap—controls compatible work that fits the safety budget. | 🔴 | **deft4j-inspired** where labelled in source |
| No-split source route | Starts each block from the bounded deft4j-derived table/state seed, then runs Columbo cumulative length-family pruning and adjacent source-order merges without grouping, split, global-boundary, broad state-queue, individual one-match pruning, or iterative replay work. Source max owns the omitted individual-prune family. If the emitted result changes a boundary or token spelling, it receives one Default refinement inside the existing file grace; exact token/header-only states skip the duplicate work. | 🟡 | deft4j-derived seed; route and cumulative/merge policy **Columbo** |
| Fragmented collection and replay | Builds a distinct seed from highly fragmented streams, capping each collected run at 4,096 tokens and 512 KiB decoded, then replays its compact 2–8-block structure. | 🔴 | **Columbo** |
| Floor-seeded max continuation | Starts max planning from a completed rewritten floor without rebuilding the already-secured token-preserving comparison floor. | 🔴 | **Columbo** |
| Best-first floor-seeded continuation | Continues an unfinished floor-seeded endpoint before weaker completed parents when it is the strict byte/bit best at the bounded-phase rejoin. This is dependency ordering rather than pruning: sufficient time still admits every sibling. | 🔴 | **Columbo** |
| Source max route | Applies the broad original-source token, table, split, merge, grouping, and boundary planner. | 🔴 | **Columbo** |
| Rewritten-seed refinement | Reparses a selected complete rewrite and runs max again unless an exact fixed point is known. | 🔴 | **Columbo** |
| Compact split floor | Prices bounded eighth splits on eligible completed parents, preserving complete progress at deadline. Timed work first covers every cut with fast ordinary headers, exhaustively finalizes only the alignment-safe suffix of that winning topology, then continues with the exact Max sweep. Distinct parents are attempted in completed byte/meaningful-bit order, retaining original route order on an exact score tie, but remain eligible when sufficient Max time is available because split gains are not monotone in parent size. Exact completed parents and same-token/block, same-type deterministic endpoints are suppressed across phases; structurally changed descendants remain eligible. | 🟡 | **Columbo** |
| Terminal merge | Greedily merges an eligible selected Huffman seed with deterministic floors, followed by at most two ordinary replays while time remains. | 🟡 | **Columbo** |
| Fixed-point suppression | Avoids another max replay when an unexpired exhaustive pass reproduced the exact bytes and meaningful-bit count. | 🟢 | **Columbo** |

## Scheduling and observability

Embedded streams share their top-level file deadline and cumulative decoded
budget. Timeout is a scheduling boundary, not a validation shortcut: later
GZIP/ZIP members and scheduled PNG image representatives are still parsed,
decoded, checksum-checked, and given a complete safe fallback. Exact-compressed
fdAT duplicates reuse their already validated representative but charge the
decoded budget once per occurrence. The unknown-unsafe-ancillary early return
has the narrower metadata-validation behavior described in the wrapper table.

`--dry-run` performs the complete optimization and reports the result without
writing output. It combines with `--verbose`, which reports route timings,
same-distance and balanced-tree opportunities, candidate bit gains, the
`Pricing block boundaries` phase, selected route, and final block plan.
Concurrent Verbose updates are cached as complete reports and written in
physical stream order as the contiguous completed prefix advances. Visual mode similarly retains the
final card for each optimizer trial and emits ordered, immutable cards without
cursor rewinds. A stream waits for all possible reclaim passes and ZIP Max
lineages before its report becomes eligible.
Concurrent ZIP Default and direct-Max lineages remain distinctly labelled.
Quiet, Verbose, and Visual runs use the same optimization, scheduling, and
memory policies.

## Attribution

These acknowledgements describe algorithmic inspiration and behavioral
reconstruction. They do not imply directly copied source code.

- **DeflOpt**, by Ben Jos Walbeehm — existing-stream Deflate optimization,
  especially dynamic-header transformations and the exact ten-bit fixed/fixed
  join. Columbo's arbitrary merging and regrouping are independent methods.
  See [Ben Jos Walbeehm's DeflOpt: what does it actually do?](https://encode.su/printthread.php?page=1&pp=30&t=455).
- **defluff**, by Joachim Henke (`jo.henke`) — repeated Huffman optimization of
  existing Deflate streams and data-section-aware feedback. See
  [defluff — a deflate Huffman optimizer](https://encode.su/threads/1214-defluff-a-deflate-huffman-optimizer).
- **deft4j**, by `NeRd` — existing-stream Deflate and archive optimization,
  including minimum-code and structural behavior reconstructed by Columbo's
  labelled deft4j route. See
  [deft4j and JarTighten](https://encode.su/threads/4112-deft4j-amp-JarTighten-yet-another-deflate-stream-amp-Zip-optimiser).
- **Turtledeflate**, by Ralf Willenbacher — inspiration for cumulative range
  histograms and the sample–smooth–narrow shape of Columbo's bounded adaptive
  split probe. Its randomized LZ77 path search is outside Columbo's scope. See
  [Turtledeflate](https://github.com/rwillenbacher/turtledeflate).
- **Zopfli**, by Google — a public reference for length-limited Huffman
  construction, header-aware histogram adjustment, and block splitting. Its
  LZ77 recompression is outside Columbo's scope. See
  [Zopfli](https://github.com/google/zopfli).
- **ECT**, by Felix Hanau — identified reduced maximum Huffman depth and the
  comparison of Brotli's newer fixed-point count smoother with Zopfli's
  classic nearby-count smoother as useful Deflate tree-search dimensions.
  Columbo independently retains paired depth-10/depth-9 Max candidates and a
  bounded dual-family terminal smoothed-tree frontier over existing token
  frequencies. It does not copy ECT source or adopt ECT's recompression,
  match-discovery, PNG pixel-transform, or JPEG routes.
  See [Efficient Compression Tool](https://github.com/fhanau/Efficient-Compression-Tool).
- **libdeflate**, by Eric Biggers — inspiration for retaining an explicit
  all-literals alternative beside iterative match paths. Columbo applies that
  idea only to already decoded source literals, with its own bounded
  frequency-only preflight and complete representation pricing; it does not
  use libdeflate's match finder or parsing code. See
  [libdeflate](https://github.com/ebiggers/libdeflate).
- **RFC 1951**, by L. Peter Deutsch — the normative Deflate format. See
  [RFC 1951](https://www.rfc-editor.org/rfc/rfc1951).

## Scope boundary

### Inside Columbo

- Parse and validate existing Deflate streams and supported wrappers.
- Reuse or rewrite Huffman tables and dynamic headers.
- Repack stored blocks or select stored, fixed, dynamic, or compatible exact
  source representations.
- Coalesce or repartition same-distance match runs.
- Replace selected existing matches with their decoded literals.
- Resegment proven matches at their original distance.
- Merge, split, collect, and regroup blocks, including bounded cuts inside
  proven matches.
- Compare complete output by file bytes and meaningful Deflate bits.

### Outside Columbo

- Searching the 32 KiB history for new matches.
- Choosing a new alternative distance for a match.
- Running Zopfli, libdeflate, or another recompressor.
- Randomized LZ77 path tracing.
- Sharing one dynamic tree between separately emitted Deflate blocks; each such
  block must transmit its own tree, so merging is the useful legal form.

Proven-submatch resegmentation and inside-match boundary polishing do not
search history. Every generated match remains within a source-proved decoded
interval and retains that source token's distance.
