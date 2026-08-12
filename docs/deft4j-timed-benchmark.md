# deft4j Timed Benchmark

This living report compares Columbo `--max` against timed `deft4j`
references. The timeout is read from each reference filename, then
two seconds are added and the result is clamped to 10..180 seconds.
deft4j parity is a sufficiently timed max-mode target, not a
normal/default-mode requirement.
A state-carried regression receives up to two serial confirmation runs.
Only a retry that is no larger in either size metric is retained.

Prior-result regression annotations are state-local: they are created
only when a row replaces an older result in the same state file. A
fresh state has no prior-row baseline, so the annotation count below
does not replace the independent historical-state audit.

- completed cases: 171
- misses: 5
- strict-policy misses reaching parity in relaxed audit: 5
- PNG preservation-policy differences: 0
- unresolved misses: 5
- errors: 0
- state-carried prior-result regression annotations over 10%: 0
- stale timeout rows: 0
- rows with recorded Columbo binary hash: 1621
- rows from current Columbo binary: 171
- rows from older Columbo binaries: 1450
- rows without Columbo binary hash: 0
- misses from current Columbo binary: 5
- misses from older Columbo binaries: 16
- misses without Columbo binary hash: 0

## Full-corpus Eligibility

This benchmark state contains all 1621 eligible timed source/reference pairs
in the fixed private corpus:

| format | eligible pairs |
| --- | ---: |
| png | 1559 |
| zip | 54 |
| gzip | 7 |
| zlib | 1 |

The corpus contains 1633 physical timed deft4j references.
The following 12 references are excluded:

Known-defective PNG source fixtures:

- `png/PngSuite/deft4j/cm7n0g04-deft4j-t1s.png`
- `png/PngSuite/deft4j/xc1n0g08-deft4j-t1s.png`
- `png/PngSuite/deft4j/xc9n2c08-deft4j-t1s.png`
- `png/PngSuite/deft4j/xd0n2c08-deft4j-t1s.png`
- `png/PngSuite/deft4j/xd3n2c08-deft4j-t1s.png`
- `png/PngSuite/deft4j/xd9n2c08-deft4j-t1s.png`
- `png/oxipng/deft4j/palette_should_be_reduced_with_bkgd-deft4j-t1s.png`

Intentionally unsupported ancient ZIP fixtures:

- `zip/oldunzip/deft4j/example-i-deft4j-t1s.zip`
- `zip/oldunzip/deft4j/example-r-deft4j-t1s.zip`
- `zip/oldunzip/deft4j/example-s-deft4j-t1s.zip`

ZIP references whose local headers cannot be validated:

- `zip/kensilverman-zip/deft4j/kzipmix-20230322-mac-deft4j-t3s.zip`
- `zip/kensilverman-zip/deft4j/pngout-20230322-mac-deft4j-t4s.zip`

The following source has no timed comparison reference and is therefore
not an eligible pair or a benchmark miss:

- `zip/zip-format-challenge/mathiasbynens-small-zip.zip`

## Unresolved Misses

| format | file | timeout | bytes vs deft4j | bits vs deft4j |
| --- | --- | ---: | ---: | ---: |
| png | `8x8-png/waves.png` | 10 | 1 | 4 |
| png | `8x8-png/symbols.png` | 10 | 0 | 3 |
| png | `PngSuite/basi0g01.png` | 10 | 0 | 3 |
| png | `PngSuite/cs5n3p08.png` | 10 | 0 | 2 |
| png | `PngSuite/cs8n3p08.png` | 10 | 0 | 2 |

## Strict-policy Differences

These strict-output max rows are slightly larger than timed deft4j.
Each was independently rerun with `--strict 0` under the same
timeout and binary; relaxed output matched or beat the reference
in both file bytes and meaningful Deflate bits. This explains the
source of the difference but does not resolve the strict-output
miss.

| format | file | timeout | strict bytes | strict bits | relaxed bytes | relaxed bits |
| --- | --- | ---: | ---: | ---: | ---: | ---: |
| png | `8x8-png/symbols.png` | 10 | 0 | 3 | -1 | -3 |
| png | `8x8-png/waves.png` | 10 | 1 | 4 | 0 | -2 |
| png | `PngSuite/basi0g01.png` | 10 | 0 | 3 | -1 | -7 |
| png | `PngSuite/cs5n3p08.png` | 10 | 0 | 2 | 0 | 0 |
| png | `PngSuite/cs8n3p08.png` | 10 | 0 | 2 | 0 | 0 |

## Rows

| format | file | deft4j seconds | timeout | Columbo seconds | bytes vs deft4j | bits vs deft4j |
| --- | --- | ---: | ---: | ---: | ---: | ---: |
| png | `8x8-png/CheckDiagonal.png` | 1 | 10 | 12.02 | -1 | -5 |
| png | `8x8-png/checked.png` | 1 | 10 | 12.00 | -1 | -12 |
| png | `8x8-png/lines.png` | 1 | 10 | 11.80 | 0 | -4 |
| png | `8x8-png/round.png` | 1 | 10 | 12.11 | -4 | -28 |
| png | `8x8-png/symbols.png` | 1 | 10 | 4.31 | 0 | 3 |
| png | `8x8-png/waves.png` | 1 | 10 | 7.30 | 1 | 4 |
| png | `PngSuite/PngSuite.png` | 1 | 10 | 11.80 | -79 | -631 |
| png | `PngSuite/basi0g01.png` | 1 | 10 | 11.94 | 0 | 3 |
| png | `PngSuite/basi0g02.png` | 1 | 10 | 11.82 | 0 | 0 |
| png | `PngSuite/basi0g04.png` | 1 | 10 | 11.80 | -1 | -11 |
| png | `PngSuite/basi0g08.png` | 1 | 10 | 11.95 | -1 | -10 |
| png | `PngSuite/basi0g16.png` | 1 | 10 | 11.89 | -2 | -13 |
| png | `PngSuite/basi2c08.png` | 1 | 10 | 11.85 | -4 | -33 |
| png | `PngSuite/basi2c16.png` | 1 | 10 | 11.85 | -1 | -11 |
| png | `PngSuite/basi3p01.png` | 1 | 10 | 11.80 | 0 | 0 |
| png | `PngSuite/basi3p02.png` | 1 | 10 | 11.79 | 0 | 0 |
| png | `PngSuite/basi3p04.png` | 1 | 10 | 11.80 | -6 | -49 |
| png | `PngSuite/basi3p08.png` | 1 | 10 | 11.80 | -18 | -143 |
| png | `PngSuite/basi4a08.png` | 1 | 10 | 11.93 | -1 | -9 |
| png | `PngSuite/basi4a16.png` | 1 | 10 | 12.04 | -2 | -15 |
| png | `PngSuite/basi6a08.png` | 1 | 10 | 11.87 | -2 | -14 |
| png | `PngSuite/basi6a16.png` | 1 | 10 | 11.79 | -2 | -11 |
| png | `PngSuite/basn0g01.png` | 1 | 10 | 11.80 | 0 | -5 |
| png | `PngSuite/basn0g02.png` | 1 | 10 | 0.47 | 0 | 0 |
| png | `PngSuite/basn0g04.png` | 1 | 10 | 4.09 | -1 | -14 |
| png | `PngSuite/basn0g08.png` | 1 | 10 | 11.79 | 0 | 0 |
| png | `PngSuite/basn0g16.png` | 1 | 10 | 11.99 | -1 | -6 |
| png | `PngSuite/basn2c08.png` | 1 | 10 | 11.79 | 0 | 0 |
| png | `PngSuite/basn2c16.png` | 1 | 10 | 11.85 | -1 | -4 |
| png | `PngSuite/basn3p01.png` | 1 | 10 | 0.20 | 0 | 0 |
| png | `PngSuite/basn3p02.png` | 1 | 10 | 7.20 | 0 | 0 |
| png | `PngSuite/basn3p04.png` | 1 | 10 | 3.34 | 0 | 0 |
| png | `PngSuite/basn3p08.png` | 1 | 10 | 11.80 | -21 | -169 |
| png | `PngSuite/basn4a08.png` | 1 | 10 | 11.79 | 0 | 0 |
| png | `PngSuite/basn4a16.png` | 1 | 10 | 11.87 | -3 | -23 |
| png | `PngSuite/basn6a08.png` | 1 | 10 | 11.82 | -1 | -6 |
| png | `PngSuite/basn6a16.png` | 1 | 10 | 11.86 | -2 | -13 |
| png | `PngSuite/bgai4a08.png` | 1 | 10 | 11.93 | -1 | -9 |
| png | `PngSuite/bgai4a16.png` | 1 | 10 | 12.14 | -2 | -15 |
| png | `PngSuite/bgan6a08.png` | 1 | 10 | 11.82 | -1 | -6 |
| png | `PngSuite/bgan6a16.png` | 1 | 10 | 11.86 | -2 | -13 |
| png | `PngSuite/bgbn4a08.png` | 1 | 10 | 11.79 | 0 | 0 |
| png | `PngSuite/bggn4a16.png` | 1 | 10 | 11.86 | -3 | -23 |
| png | `PngSuite/bgwn6a08.png` | 1 | 10 | 11.82 | -1 | -6 |
| png | `PngSuite/bgyn6a16.png` | 1 | 10 | 11.86 | -2 | -13 |
| png | `PngSuite/ccwn2c08.png` | 1 | 10 | 12.74 | -4 | -37 |
| png | `PngSuite/ccwn3p08.png` | 1 | 10 | 11.79 | 0 | 0 |
| png | `PngSuite/cdfn2c08.png` | 1 | 10 | 11.84 | -1 | -5 |
| png | `PngSuite/cdhn2c08.png` | 1 | 10 | 11.80 | -2 | -12 |
| png | `PngSuite/cdsn2c08.png` | 1 | 10 | 4.93 | -1 | -5 |
| png | `PngSuite/cdun2c08.png` | 1 | 10 | 11.99 | -2 | -16 |
| png | `PngSuite/ch1n3p04.png` | 1 | 10 | 3.33 | 0 | 0 |
| png | `PngSuite/ch2n3p08.png` | 1 | 10 | 11.80 | -21 | -169 |
| png | `PngSuite/cm0n0g04.png` | 1 | 10 | 11.84 | -4 | -29 |
| png | `PngSuite/cm9n0g04.png` | 1 | 10 | 11.84 | -4 | -29 |
| png | `PngSuite/cs3n2c16.png` | 1 | 10 | 11.81 | -1 | -6 |
| png | `PngSuite/cs3n3p08.png` | 1 | 10 | 11.81 | -1 | -10 |
| png | `PngSuite/cs5n2c08.png` | 1 | 10 | 11.82 | -1 | -9 |
| png | `PngSuite/cs5n3p08.png` | 1 | 10 | 11.79 | 0 | 2 |
| png | `PngSuite/cs8n2c08.png` | 1 | 10 | 11.80 | -1 | -3 |
| png | `PngSuite/cs8n3p08.png` | 1 | 10 | 11.79 | 0 | 2 |
| png | `PngSuite/ct0n0g04.png` | 1 | 10 | 11.84 | -4 | -29 |
| png | `PngSuite/ct1n0g04.png` | 1 | 10 | 11.84 | -4 | -29 |
| png | `PngSuite/cten0g04.png` | 1 | 10 | 11.81 | 0 | 0 |
| png | `PngSuite/ctfn0g04.png` | 1 | 10 | 9.70 | 0 | 0 |
| png | `PngSuite/ctgn0g04.png` | 1 | 10 | 11.30 | -1 | -3 |
| png | `PngSuite/cthn0g04.png` | 1 | 10 | 11.79 | -1 | -10 |
| png | `PngSuite/ctjn0g04.png` | 1 | 10 | 11.79 | -1 | -11 |
| png | `PngSuite/ctzn0g04.png` | 1 | 10 | 11.78 | -4 | -29 |
| png | `PngSuite/exif2c08.png` | 1 | 10 | 12.63 | -3 | -24 |
| png | `PngSuite/f00n0g08.png` | 1 | 10 | 11.89 | -16 | -130 |
| png | `PngSuite/f00n2c08.png` | 1 | 10 | 11.96 | -46 | -363 |
| png | `PngSuite/f01n0g08.png` | 1 | 10 | 12.09 | -3 | -25 |
| png | `PngSuite/f01n2c08.png` | 1 | 10 | 12.04 | -6 | -53 |
| png | `PngSuite/f02n0g08.png` | 1 | 10 | 11.79 | -2 | -13 |
| png | `PngSuite/f02n2c08.png` | 1 | 10 | 12.42 | -8 | -64 |
| png | `PngSuite/f03n0g08.png` | 1 | 10 | 12.45 | -4 | -36 |
| png | `PngSuite/f03n2c08.png` | 1 | 10 | 12.05 | -9 | -74 |
| png | `PngSuite/f04n0g08.png` | 1 | 10 | 11.92 | -3 | -22 |
| png | `PngSuite/f04n2c08.png` | 1 | 10 | 12.31 | -5 | -37 |
| png | `PngSuite/f99n0g04.png` | 1 | 10 | 12.14 | -8 | -64 |
| png | `PngSuite/g03n0g16.png` | 1 | 10 | 11.83 | -3 | -23 |
| png | `PngSuite/g03n2c08.png` | 1 | 10 | 11.96 | -2 | -18 |
| png | `PngSuite/g03n3p04.png` | 1 | 10 | 11.79 | 0 | 0 |
| png | `PngSuite/g04n0g16.png` | 1 | 10 | 12.11 | -4 | -35 |
| png | `PngSuite/g04n2c08.png` | 1 | 10 | 12.30 | -3 | -22 |
| png | `PngSuite/g04n3p04.png` | 1 | 10 | 12.00 | -2 | -15 |
| png | `PngSuite/g05n0g16.png` | 1 | 10 | 11.84 | -3 | -18 |
| png | `PngSuite/g05n2c08.png` | 1 | 10 | 11.99 | -1 | -8 |
| png | `PngSuite/g05n3p04.png` | 1 | 10 | 11.79 | 0 | 0 |
| png | `PngSuite/g07n0g16.png` | 1 | 10 | 11.81 | 0 | -1 |
| png | `PngSuite/g07n2c08.png` | 1 | 10 | 11.89 | -1 | -9 |
| png | `PngSuite/g07n3p04.png` | 1 | 10 | 11.79 | 0 | 0 |
| png | `PngSuite/g10n0g16.png` | 1 | 10 | 11.81 | 0 | -2 |
| png | `PngSuite/g10n2c08.png` | 1 | 10 | 12.00 | -1 | -6 |
| png | `PngSuite/g10n3p04.png` | 1 | 10 | 11.79 | -1 | -10 |
| png | `PngSuite/g25n0g16.png` | 1 | 10 | 12.07 | -2 | -18 |
| png | `PngSuite/g25n2c08.png` | 1 | 10 | 12.00 | -2 | -19 |
| png | `PngSuite/g25n3p04.png` | 1 | 10 | 11.88 | -1 | -9 |
| png | `PngSuite/oi1n0g16.png` | 1 | 10 | 11.99 | -1 | -6 |
| png | `PngSuite/oi1n2c16.png` | 1 | 10 | 11.84 | -1 | -4 |
| png | `PngSuite/oi2n0g16.png` | 1 | 10 | 11.99 | -1 | -6 |
| png | `PngSuite/oi2n2c16.png` | 1 | 10 | 11.84 | -1 | -4 |
| png | `PngSuite/oi4n0g16.png` | 1 | 10 | 11.99 | -1 | -6 |
| png | `PngSuite/oi4n2c16.png` | 1 | 10 | 11.84 | -1 | -4 |
| png | `PngSuite/oi9n0g16.png` | 1 | 10 | 11.99 | -1 | -6 |
| png | `PngSuite/oi9n2c16.png` | 1 | 10 | 11.84 | -1 | -4 |
| png | `PngSuite/pp0n2c16.png` | 1 | 10 | 11.84 | -1 | -4 |
| png | `PngSuite/pp0n6a08.png` | 1 | 10 | 11.80 | 0 | -4 |
| png | `PngSuite/ps1n0g08.png` | 1 | 10 | 11.79 | 0 | 0 |
| png | `PngSuite/ps1n2c16.png` | 1 | 10 | 11.84 | -1 | -4 |
| png | `PngSuite/ps2n0g08.png` | 1 | 10 | 11.79 | 0 | 0 |
| png | `PngSuite/ps2n2c16.png` | 1 | 10 | 11.85 | -1 | -4 |
| png | `PngSuite/s01i3p01.png` | 1 | 10 | 0.01 | 0 | 0 |
| png | `PngSuite/s01n3p01.png` | 1 | 10 | 0.01 | 0 | 0 |
| png | `PngSuite/s02i3p01.png` | 1 | 10 | 0.03 | 0 | 0 |
| png | `PngSuite/s02n3p01.png` | 1 | 10 | 0.01 | 0 | 0 |
| png | `PngSuite/s03i3p01.png` | 1 | 10 | 0.04 | 0 | 0 |
| png | `PngSuite/s03n3p01.png` | 1 | 10 | 0.01 | 0 | 0 |
| png | `PngSuite/s04i3p01.png` | 1 | 10 | 0.05 | 0 | 0 |
| png | `PngSuite/s04n3p01.png` | 1 | 10 | 0.05 | 0 | 0 |
| png | `PngSuite/s05i3p02.png` | 1 | 10 | 0.16 | 0 | 0 |
| png | `PngSuite/s05n3p02.png` | 1 | 10 | 0.04 | 0 | 0 |
| png | `PngSuite/s06i3p02.png` | 1 | 10 | 0.06 | 0 | 0 |
| png | `PngSuite/s06n3p02.png` | 1 | 10 | 0.07 | 0 | 0 |
| png | `PngSuite/s07i3p02.png` | 1 | 10 | 0.12 | 0 | 0 |
| png | `PngSuite/s07n3p02.png` | 1 | 10 | 0.09 | 0 | 0 |
| png | `PngSuite/s08i3p02.png` | 1 | 10 | 0.16 | 0 | 0 |
| png | `PngSuite/s08n3p02.png` | 1 | 10 | 0.17 | 0 | 0 |
| png | `PngSuite/s09i3p02.png` | 1 | 10 | 0.27 | 0 | 0 |
| png | `PngSuite/s09n3p02.png` | 1 | 10 | 0.42 | 0 | 0 |
| png | `PngSuite/s32i3p04.png` | 1 | 10 | 11.83 | -3 | -24 |
| png | `PngSuite/s32n3p04.png` | 1 | 10 | 11.80 | -2 | -17 |
| png | `PngSuite/s33i3p04.png` | 1 | 10 | 11.94 | -3 | -18 |
| png | `PngSuite/s33n3p04.png` | 1 | 10 | 12.03 | -5 | -40 |
| png | `PngSuite/s34i3p04.png` | 1 | 10 | 12.17 | -3 | -26 |
| png | `PngSuite/s34n3p04.png` | 1 | 10 | 11.81 | -2 | -11 |
| png | `PngSuite/s35i3p04.png` | 1 | 10 | 11.84 | -2 | -18 |
| png | `PngSuite/s35n3p04.png` | 1 | 10 | 12.04 | -6 | -52 |
| png | `PngSuite/s36i3p04.png` | 1 | 10 | 11.82 | -3 | -28 |
| png | `PngSuite/s36n3p04.png` | 1 | 10 | 11.79 | -2 | -17 |
| png | `PngSuite/s37i3p04.png` | 1 | 10 | 12.19 | -3 | -26 |
| png | `PngSuite/s37n3p04.png` | 1 | 10 | 12.04 | -6 | -43 |
| png | `PngSuite/s38i3p04.png` | 1 | 10 | 12.26 | -1 | -15 |
| png | `PngSuite/s38n3p04.png` | 1 | 10 | 11.79 | -1 | -9 |
| png | `PngSuite/s39i3p04.png` | 1 | 10 | 12.09 | -3 | -29 |
| png | `PngSuite/s39n3p04.png` | 1 | 10 | 11.84 | -4 | -30 |
| png | `PngSuite/s40i3p04.png` | 1 | 10 | 11.83 | -4 | -36 |
| png | `PngSuite/s40n3p04.png` | 1 | 10 | 12.20 | -3 | -26 |
| png | `PngSuite/tbbn0g04.png` | 1 | 10 | 11.91 | -1 | -11 |
| png | `PngSuite/tbbn2c16.png` | 1 | 10 | 11.89 | -1 | -9 |
| png | `PngSuite/tbbn3p08.png` | 1 | 10 | 11.80 | 0 | 0 |
| png | `PngSuite/tbgn2c16.png` | 1 | 10 | 11.89 | -1 | -9 |
| png | `PngSuite/tbgn3p08.png` | 1 | 10 | 11.79 | 0 | 0 |
| png | `PngSuite/tbrn2c08.png` | 1 | 10 | 12.54 | -4 | -37 |
| png | `PngSuite/tbwn0g16.png` | 1 | 10 | 11.80 | 0 | 0 |
| png | `PngSuite/tbwn3p08.png` | 1 | 10 | 11.80 | 0 | 0 |
| png | `PngSuite/tbyn3p08.png` | 1 | 10 | 11.80 | 0 | 0 |
| png | `PngSuite/tm3n3p02.png` | 1 | 10 | 0.09 | 0 | 0 |
| png | `PngSuite/tp0n0g08.png` | 1 | 10 | 11.79 | 0 | 0 |
| png | `PngSuite/tp0n2c08.png` | 1 | 10 | 11.86 | -4 | -32 |
| png | `PngSuite/tp0n3p08.png` | 1 | 10 | 11.79 | 0 | 0 |
| png | `PngSuite/tp1n3p08.png` | 1 | 10 | 11.79 | 0 | 0 |
| png | `PngSuite/z00n2c08.png` | 1 | 10 | 0.20 | -2580 | -20647 |
| png | `PngSuite/z03n2c08.png` | 1 | 10 | 12.19 | -3 | -21 |
| png | `PngSuite/z06n2c08.png` | 1 | 10 | 11.96 | -2 | -10 |
| png | `PngSuite/z09n2c08.png` | 1 | 10 | 11.96 | -2 | -10 |
| png | `apng-large/3d2.png` | 27 | 29 | 31.89 | -468 | -3731 |
| png | `apng-large/MediaWiki-2020-large-icon-spinning.png` | 49 | 51 | 55.04 | -1336 | -10662 |
| png | `apng-large/animation_icos4d.orig.png` | 18 | 20 | 22.46 | -2449 | -19559 |
| png | `apng-large/animation_icos4d.ref.png` | 71 | 73 | 78.20 | -1272 | -10192 |
