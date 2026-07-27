# deft4j Timed Benchmark

This living report compares Columbo `--max` against timed `deft4j`
references. The timeout is read from each reference filename, then
two seconds are added and the result is clamped to 10..180 seconds.
A state-carried regression receives up to two serial confirmation runs.
Only a retry that is no larger in either size metric is retained.

Prior-result regression annotations are state-local: they are created
only when a row replaces an older result in the same state file. A
fresh state has no prior-row baseline, so the annotation count below
does not replace the independent historical-state audit.

- completed cases: 44
- misses: 3
- strict-policy misses resolved by relaxed audit: 3
- unresolved misses: 0
- errors: 0
- state-carried prior-result regression annotations over 10%: 0
- stale timeout rows: 0
- rows with recorded Columbo binary hash: 44
- rows from current Columbo binary: 44
- rows from older Columbo binaries: 0
- rows without Columbo binary hash: 0
- misses from current Columbo binary: 3
- misses from older Columbo binaries: 0
- misses without Columbo binary hash: 0

## Strict-policy Differences

These strict-default rows are slightly larger than timed deft4j.
Each was independently rerun with `--strict 0` under the same
timeout and binary; relaxed output matched or beat the reference
in both file bytes and meaningful Deflate bits.

| format | file | timeout | strict bytes | strict bits | relaxed bytes | relaxed bits |
| --- | --- | ---: | ---: | ---: | ---: | ---: |
| png | `8x8-png/symbols.png` | 10 | 0 | 6 | 0 | 0 |
| png | `8x8-png/waves.png` | 10 | 1 | 4 | 0 | -2 |
| png | `PngSuite/basi0g01.png` | 10 | 1 | 9 | 0 | 0 |

## Rows

| format | file | deft4j seconds | timeout | Columbo seconds | bytes vs deft4j | bits vs deft4j |
| --- | --- | ---: | ---: | ---: | ---: | ---: |
| png | `8x8-png/CheckDiagonal.png` | 1 | 10 | 6.80 | 0 | -1 |
| png | `8x8-png/checked.png` | 1 | 10 | 9.82 | -1 | -10 |
| png | `8x8-png/lines.png` | 1 | 10 | 9.81 | 0 | -4 |
| png | `8x8-png/round.png` | 1 | 10 | 9.81 | -3 | -22 |
| png | `8x8-png/symbols.png` | 1 | 10 | 2.74 | 0 | 6 |
| png | `8x8-png/waves.png` | 1 | 10 | 5.99 | 1 | 4 |
| png | `PngSuite/PngSuite.png` | 1 | 10 | 9.81 | -79 | -631 |
| png | `PngSuite/basi0g01.png` | 1 | 10 | 9.81 | 1 | 9 |
| png | `PngSuite/basi0g02.png` | 1 | 10 | 9.81 | 0 | 0 |
| png | `PngSuite/basi0g04.png` | 1 | 10 | 9.81 | -1 | -5 |
| png | `PngSuite/basi0g08.png` | 1 | 10 | 9.81 | 0 | -2 |
| png | `PngSuite/basi0g16.png` | 1 | 10 | 9.81 | -2 | -13 |
| png | `PngSuite/basi2c08.png` | 1 | 10 | 9.81 | -2 | -11 |
| png | `PngSuite/basi2c16.png` | 1 | 10 | 9.81 | 0 | -5 |
| png | `PngSuite/basi3p01.png` | 1 | 10 | 6.17 | 0 | 0 |
| png | `PngSuite/basi3p02.png` | 1 | 10 | 9.82 | 0 | 0 |
| png | `PngSuite/basi3p04.png` | 1 | 10 | 9.81 | -6 | -49 |
| png | `PngSuite/basi3p08.png` | 1 | 10 | 9.81 | -18 | -143 |
| png | `PngSuite/basi4a08.png` | 1 | 10 | 9.81 | -1 | -4 |
| png | `PngSuite/basi4a16.png` | 1 | 10 | 9.81 | -2 | -13 |
| png | `PngSuite/basi6a08.png` | 1 | 10 | 9.81 | -1 | -4 |
| png | `PngSuite/basi6a16.png` | 1 | 10 | 9.81 | -2 | -11 |
| png | `PngSuite/basn0g01.png` | 1 | 10 | 5.80 | 0 | -2 |
| png | `PngSuite/basn0g02.png` | 1 | 10 | 0.42 | 0 | 0 |
| png | `PngSuite/basn0g04.png` | 1 | 10 | 1.60 | -1 | -14 |
| png | `PngSuite/basn0g08.png` | 1 | 10 | 9.81 | 0 | 0 |
| png | `PngSuite/basn0g16.png` | 1 | 10 | 9.81 | 0 | 0 |
| png | `PngSuite/basn2c08.png` | 1 | 10 | 9.84 | 0 | 0 |
| png | `PngSuite/basn2c16.png` | 1 | 10 | 9.81 | -1 | -4 |
| png | `PngSuite/basn3p01.png` | 1 | 10 | 0.14 | 0 | 0 |
| png | `PngSuite/basn3p02.png` | 1 | 10 | 2.70 | 0 | 0 |
| png | `PngSuite/basn3p04.png` | 1 | 10 | 1.58 | 0 | 0 |
| png | `PngSuite/basn3p08.png` | 1 | 10 | 9.82 | -21 | -169 |
| png | `PngSuite/basn4a08.png` | 1 | 10 | 9.81 | 0 | 0 |
| png | `PngSuite/basn4a16.png` | 1 | 10 | 9.81 | -1 | -2 |
| png | `PngSuite/basn6a08.png` | 1 | 10 | 9.81 | 0 | 0 |
| png | `PngSuite/basn6a16.png` | 1 | 10 | 9.81 | -1 | -8 |
| png | `PngSuite/bgai4a08.png` | 1 | 10 | 9.81 | -1 | -4 |
| png | `PngSuite/bgai4a16.png` | 1 | 10 | 9.81 | -2 | -13 |
| png | `PngSuite/bgan6a08.png` | 1 | 10 | 9.81 | 0 | 0 |
| png | `PngSuite/bgan6a16.png` | 1 | 10 | 9.81 | -1 | -8 |
| png | `PngSuite/bgbn4a08.png` | 1 | 10 | 9.81 | 0 | 0 |
| png | `PngSuite/bggn4a16.png` | 1 | 10 | 9.81 | -1 | -2 |
| png | `PngSuite/bgwn6a08.png` | 1 | 10 | 9.81 | 0 | 0 |
