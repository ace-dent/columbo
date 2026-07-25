# deft4j Timed Benchmark

This living report compares Columbo `--max` against timed `deft4j`
references. The timeout is read from each reference filename, then
two seconds are added and the result is clamped to 10..180 seconds.
A historical regression receives up to two serial confirmation runs.
After those runs, an isolated loss below eight Deflate bits is treated
as sub-byte timing variance only when Columbo still matches or beats
deft4j in both file bytes and stream bits.

Rows generated before this policy change should be refreshed after
meaningful optimizer improvements rather than solely to update binary
hashes.

Current audit note: all expected timed references use the shell-safe
`-deft4j-t<seconds>s` suffix and all state rows still point at the
current reference filenames. Rows whose only stale field is `timeout`
predate the newer `seconds + 2` timeout policy and should be refreshed
with real benchmark work, not just for metadata churn.
`work/benchmark_deft4j_timed.py --resume` skips timeout-only or
binary-hash stale rows by default; use `--refresh metadata` when an
intentional benchmark refresh should update those rows.

The miss list below reflects the saved benchmark rows. When the audit
reports stale timeout rows, those rows are useful progress markers but
not proof of the latest timeout policy until they are refreshed.

- completed cases: 1609
- misses: 16
- errors: 0
- previous-result regressions over 5%: 55
- stale timeout rows: 923
- rows with recorded Columbo binary hash: 1609
- rows from current Columbo binary: 7
- rows from older Columbo binaries: 1602
- rows without Columbo binary hash: 0
- misses from current Columbo binary: 0
- misses from older Columbo binaries: 16
- misses without Columbo binary hash: 0

Known defective PNG fixtures and unsupported ancient ZIP fixtures are
excluded by the same corpus rules as the regular smoke tests.

## Previous-result Regressions

These rows lost more than 5% of a previous Columbo advantage
over the same timed deft4j reference. The benchmark stops when
it records one so the case can be investigated before continuing.

| format | file | metric | previous saved | current saved | advantage lost |
| --- | --- | --- | ---: | ---: | ---: |
| png | `PngSuite/PngSuite.png` | bytes | 118 | 79 | 33.05% |
| png | `PngSuite/PngSuite.png` | bits | 940 | 631 | 32.87% |
| png | `PngSuite/basi6a16.png` | bytes | 3 | 2 | 33.33% |
| png | `PngSuite/basi6a16.png` | bits | 20 | 10 | 50.00% |
| png | `PngSuite/basn6a16.png` | bytes | 30 | 1 | 96.67% |
| png | `PngSuite/basn6a16.png` | bits | 235 | 8 | 96.60% |
| png | `PngSuite/bgan6a16.png` | bytes | 30 | 1 | 96.67% |
| png | `PngSuite/bgan6a16.png` | bits | 235 | 8 | 96.60% |
| png | `PngSuite/bgyn6a16.png` | bytes | 30 | 1 | 96.67% |
| png | `PngSuite/bgyn6a16.png` | bits | 235 | 8 | 96.60% |
| png | `PngSuite/cdhn2c08.png` | bytes | 4 | 1 | 75.00% |
| png | `PngSuite/cdhn2c08.png` | bits | 30 | 4 | 86.67% |
| png | `PngSuite/cm0n0g04.png` | bytes | 5 | -2 | 140.00% |
| png | `PngSuite/cm0n0g04.png` | bits | 35 | -14 | 140.00% |
| png | `PngSuite/cm9n0g04.png` | bytes | 5 | -2 | 140.00% |
| png | `PngSuite/cm9n0g04.png` | bits | 35 | -14 | 140.00% |
| png | `PngSuite/ct0n0g04.png` | bytes | 5 | -2 | 140.00% |
| png | `PngSuite/ct0n0g04.png` | bits | 35 | -14 | 140.00% |
| png | `PngSuite/ct1n0g04.png` | bytes | 5 | -2 | 140.00% |
| png | `PngSuite/ct1n0g04.png` | bits | 35 | -14 | 140.00% |
| png | `PngSuite/ctzn0g04.png` | bytes | 5 | -2 | 140.00% |
| png | `PngSuite/ctzn0g04.png` | bits | 35 | -14 | 140.00% |
| png | `PngSuite/f01n2c08.png` | bytes | 6 | 5 | 16.67% |
| png | `PngSuite/f01n2c08.png` | bits | 53 | 42 | 20.75% |
| png | `PngSuite/f99n0g04.png` | bytes | 7 | 6 | 14.29% |
| png | `PngSuite/f99n0g04.png` | bits | 53 | 43 | 18.87% |
| png | `PngSuite/g25n2c08.png` | bytes | 2 | 1 | 50.00% |
| png | `PngSuite/g25n2c08.png` | bits | 15 | 7 | 53.33% |
| png | `apng-large/3d2.png` | bytes | 288 | 272 | 5.56% |
| png | `apng-large/animation_icos4d.ref.png` | bytes | 805 | 305 | 62.11% |
| png | `apng-large/animation_icos4d.ref.png` | bits | 6422 | 2456 | 61.76% |
| png | `apng-medium/Lotus-buddha-APNG-animation.png` | bits | 300 | 275 | 8.33% |
| png | `apng-medium/ball.png` | bytes | 75 | 16 | 78.67% |
| png | `apng-medium/ball.png` | bits | 590 | 120 | 79.66% |
| png | `css-ig-net/Bullfinch.png` | bytes | 45 | 9 | 80.00% |
| png | `css-ig-net/Bullfinch.png` | bits | 364 | 77 | 78.85% |
| png | `css-ig-net/Gingerman.png` | bytes | 4 | 1 | 75.00% |
| png | `css-ig-net/Gingerman.png` | bits | 32 | 12 | 62.50% |
| png | `css-ig-net/Lemon512.png` | bytes | 956 | 7 | 99.27% |
| png | `css-ig-net/Lemon512.png` | bits | 7646 | 56 | 99.27% |
| png | `css-ig-net/Mango512.png` | bytes | 10589 | 8638 | 18.42% |
| png | `css-ig-net/Mango512.png` | bits | 84715 | 69105 | 18.43% |
| png | `css-ig-net/Sock.png` | bytes | 554 | 190 | 65.70% |
| png | `css-ig-net/Sock.png` | bits | 4432 | 1519 | 65.73% |
| png | `css-ig-net/art.png` | bytes | 6 | 3 | 50.00% |
| png | `css-ig-net/art.png` | bits | 43 | 20 | 53.49% |
| png | `css-ig-net/bad-paletted.png` | bytes | 73 | 67 | 8.22% |
| png | `css-ig-net/bad-paletted.png` | bits | 580 | 538 | 7.24% |
| png | `css-ig-net/batteryfull.png` | bytes | 2 | -1 | 150.00% |
| png | `css-ig-net/batteryfull.png` | bits | 20 | -8 | 140.00% |
| png | `css-ig-net/bikewheel.png` | bytes | 2 | 0 | 100.00% |
| png | `css-ig-net/bikewheel.png` | bits | 14 | 2 | 85.71% |
| png | `css-ig-net/bomb.png` | bytes | 1 | -1 | 200.00% |
| png | `css-ig-net/bomb.png` | bits | 5 | -14 | 380.00% |
| png | `css-ig-net/bookshelf.png` | bytes | 4 | 0 | 100.00% |
| png | `css-ig-net/bookshelf.png` | bits | 39 | 7 | 82.05% |
| png | `css-ig-net/carton-fs8.png` | bytes | 3 | 2 | 33.33% |
| png | `css-ig-net/carton-fs8.png` | bits | 24 | 11 | 54.17% |
| png | `css-ig-net/carwheel.png` | bytes | 4 | 3 | 25.00% |
| png | `css-ig-net/carwheel.png` | bits | 28 | 18 | 35.71% |
| png | `css-ig-net/check.png` | bytes | 6 | 4 | 33.33% |
| png | `css-ig-net/check.png` | bits | 46 | 33 | 28.26% |
| png | `css-ig-net/clock.png` | bytes | 2 | 1 | 50.00% |
| png | `css-ig-net/clock.png` | bits | 22 | 12 | 45.45% |
| png | `css-ig-net/compose.png` | bytes | 1 | -4 | 500.00% |
| png | `css-ig-net/compose.png` | bits | 4 | -35 | 975.00% |
| png | `css-ig-net/contrast.png` | bytes | 2 | 0 | 100.00% |
| png | `css-ig-net/contrast.png` | bits | 18 | 0 | 100.00% |
| png | `css-ig-net/crop.png` | bytes | 3 | 1 | 66.67% |
| png | `css-ig-net/crop.png` | bits | 23 | 9 | 60.87% |
| png | `css-ig-net/dolly.png` | bytes | 5 | 1 | 80.00% |
| png | `css-ig-net/dolly.png` | bits | 36 | 7 | 80.56% |
| png | `css-ig-net/dossier-blue-papier-fs8.png` | bytes | 21 | 12 | 42.86% |
| png | `css-ig-net/dossier-blue-papier-fs8.png` | bits | 173 | 99 | 42.77% |
| png | `css-ig-net/dossier-green-pictures-fs8.png` | bytes | 4 | 2 | 50.00% |
| png | `css-ig-net/dossier-green-pictures-fs8.png` | bits | 27 | 13 | 51.85% |
| png | `css-ig-net/drive-blue-network-fs8.png` | bytes | 78 | 66 | 15.38% |
| png | `css-ig-net/drive-blue-network-fs8.png` | bits | 623 | 528 | 15.25% |
| png | `css-ig-net/drive-green-network-fs8.png` | bytes | 69 | 60 | 13.04% |
| png | `css-ig-net/drive-green-network-fs8.png` | bits | 550 | 484 | 12.00% |
| png | `css-ig-net/file04.png` | bytes | 1557 | 843 | 45.86% |
| png | `css-ig-net/file04.png` | bits | 12452 | 6748 | 45.81% |
| png | `css-ig-net/file07.png` | bytes | 1609 | 1253 | 22.13% |
| png | `css-ig-net/file07.png` | bits | 12873 | 10030 | 22.08% |
| png | `css-ig-net/filmroll.png` | bytes | 2 | -3 | 250.00% |
| png | `css-ig-net/filmroll.png` | bits | 17 | -22 | 229.41% |
| png | `css-ig-net/frames.png` | bytes | 5 | -2 | 140.00% |
| png | `css-ig-net/frames.png` | bits | 43 | -16 | 137.21% |
| png | `css-ig-net/gamecontroller.png` | bytes | 2 | 0 | 100.00% |
| png | `css-ig-net/gamecontroller.png` | bits | 16 | -1 | 106.25% |
| png | `css-ig-net/genius.png` | bits | 2 | -1 | 150.00% |
| png | `css-ig-net/global.png` | bytes | 3 | 2 | 33.33% |
| png | `css-ig-net/global.png` | bits | 23 | 13 | 43.48% |
| png | `css-ig-net/icone_windows-fs8.png` | bytes | 27 | -1 | 103.70% |
| png | `css-ig-net/icone_windows-fs8.png` | bits | 221 | -1 | 100.45% |
| png | `css-ig-net/key.png` | bytes | 1 | -2 | 300.00% |
| png | `css-ig-net/key.png` | bits | 12 | -13 | 208.33% |
| png | `css-ig-net/lens.png` | bytes | 2 | 1 | 50.00% |
| png | `css-ig-net/lens.png` | bits | 18 | 7 | 61.11% |
| png | `css-ig-net/loupe-fs8.png` | bytes | 34 | 2 | 94.12% |
| png | `css-ig-net/loupe-fs8.png` | bits | 274 | 20 | 92.70% |
| png | `css-ig-net/map.png` | bytes | 3 | -7 | 333.33% |
| png | `css-ig-net/map.png` | bits | 20 | -56 | 380.00% |
| png | `css-ig-net/my-computer-off-fs8.png` | bytes | 3 | 1 | 66.67% |
| png | `css-ig-net/my-computer-off-fs8.png` | bits | 21 | 10 | 52.38% |
| png | `medium/BNDT_on_X____stack__https___t_co_3VRfGVC7bX____X.png` | bytes | 833 | 769 | 7.68% |
| png | `medium/BNDT_on_X____stack__https___t_co_3VRfGVC7bX____X.png` | bits | 6662 | 6149 | 7.70% |

## Saved Misses

| format | file | timeout | bytes vs deft4j | bits vs deft4j |
| --- | --- | ---: | ---: | ---: |
| png | `css-ig-net/map.png` | 10 | 7 | 56 |
| png | `css-ig-net/compose.png` | 10 | 4 | 35 |
| png | `css-ig-net/filmroll.png` | 10 | 3 | 22 |
| png | `css-ig-net/frames.png` | 10 | 2 | 16 |
| png | `PngSuite/cm0n0g04.png` | 10 | 2 | 14 |
| png | `PngSuite/cm9n0g04.png` | 10 | 2 | 14 |
| png | `PngSuite/ct0n0g04.png` | 10 | 2 | 14 |
| png | `PngSuite/ct1n0g04.png` | 10 | 2 | 14 |
| png | `PngSuite/ctzn0g04.png` | 10 | 2 | 14 |
| png | `css-ig-net/key.png` | 10 | 2 | 13 |
| png | `css-ig-net/bomb.png` | 10 | 1 | 14 |
| png | `css-ig-net/biker.png` | 10 | 1 | 12 |
| png | `css-ig-net/batteryfull.png` | 10 | 1 | 8 |
| png | `css-ig-net/icone_windows-fs8.png` | 10 | 1 | 1 |
| png | `css-ig-net/gamecontroller.png` | 10 | 0 | 1 |
| png | `css-ig-net/genius.png` | 10 | 0 | 1 |

## Stale Timeout Rows

These rows still use an older timeout value in the saved state.
Refresh them only when the current optimizer can preserve or beat
the timed `deft4j` reference in both file bytes and deflate bits.
Total stale timeout rows: 923 (gzip: 3, png: 879, zip: 41).
A short sample is shown below to keep this living document readable.

| format | file | saved timeout | expected timeout | bytes vs deft4j | bits vs deft4j |
| --- | --- | ---: | ---: | ---: | ---: |
| gzip | `kensilverman-gz/kzipmix-20200115-bsd.tar.gz` | 4 | 10 | -123 | -986 |
| gzip | `kensilverman-gz/kzipmix-20200115-linux.tar.gz` | 5 | 10 | -283 | -2271 |
| gzip | `kensilverman-gz/pngout-20200115-bsd.tar.gz` | 9 | 10 | -296 | -2370 |
| png | `large/T_Brick_house_islands.png` | 3 | 10 | -3 | -26 |
| png | `large/uob-original.png` | 4 | 10 | -1 | -3 |
| png | `medium/02-ct-c4-c0.png` | 4 | 10 | 0 | 0 |
| png | `medium/05-ct-c0-c3.png` | 5 | 10 | 0 | 0 |
| png | `medium/09-ct-c6-c4.png` | 4 | 10 | 0 | 0 |
| png | `medium/1.5.02.PNG` | 5 | 10 | -40 | -322 |
| png | `medium/12-c3-trans-first.png` | 4 | 10 | 0 | -1 |
| png | `medium/15-c3-color-filter-trans-first.png` | 4 | 10 | 0 | 0 |
| png | `medium/1_original.png` | 3 | 10 | -40 | -318 |
| png | `medium/20-dt-rgb-mod-free.png` | 5 | 10 | 0 | 0 |
| png | `medium/21-dt-rgb-mod-average.png` | 3 | 10 | 0 | 0 |
| png | `medium/280543107-79bbbda4-b565-493b-9088-d7647e86a1ca.png` | 3 | 10 | -1 | -12 |
| png | `medium/2_original.png` | 5 | 10 | -310 | -2482 |
| png | `medium/3_original.png` | 3 | 10 | -1076 | -8606 |
| png | `medium/3x5 font alternatives.png` | 3 | 10 | -273 | -2182 |
| png | `medium/4.1.01.PNG` | 5 | 10 | -858 | -6865 |
| png | `medium/4_original.png` | 5 | 10 | -77 | -615 |
| png | `medium/5.1.09.PNG` | 4 | 10 | -851 | -6814 |
| png | `medium/5.1.11.PNG` | 3 | 10 | -1477 | -11811 |
| png | `medium/5.1.13.PNG` | 3 | 10 | -99 | -792 |
| png | `medium/5_original.png` | 3 | 10 | -5 | -37 |
| png | `medium/81817-2.png` | 3 | 10 | -37 | -296 |
| png | `medium/AlphaBall.png` | 3 | 10 | -95 | -755 |
| png | `medium/AlphaEdge.png` | 3 | 10 | -765 | -6127 |
| png | `medium/Backy_ru.png` | 3 | 10 | 0 | -6 |
| png | `medium/Bag.png` | 6 | 10 | 0 | 0 |
| png | `medium/Bullfinch.png` | 8 | 10 | 0 | -1 |
| png | `medium/ColorTestCard.png` | 5 | 10 | -480 | -3836 |
| png | `medium/Column4.png` | 5 | 10 | -1 | -5 |
| png | `medium/Cookies.png` | 7 | 10 | 0 | 0 |
| png | `medium/Death.png` | 6 | 10 | -1 | -7 |
| png | `medium/Decorations.png` | 6 | 10 | 0 | 0 |
| png | `medium/End.png` | 7 | 10 | -1 | -4 |
| png | `medium/F2EXEs_WQAAfE38.png` | 3 | 10 | -20 | -156 |
| png | `medium/GK1fa8hXYAAFE1C.png` | 3 | 10 | -28 | -221 |
| png | `medium/Gingerman.png` | 7 | 10 | 0 | 0 |
| png | `medium/Grading-2018-1.png` | 3 | 10 | -436 | -3482 |

| format | file | deft4j seconds | timeout | Columbo seconds | bytes vs deft4j | bits vs deft4j |
| --- | --- | ---: | ---: | ---: | ---: | ---: |
| gzip | `kensilverman-gz/kzipmix-20200115-bsd-static.tar.gz` | 26 | 28 | 28.48 | -791 | -6335 |
| gzip | `kensilverman-gz/kzipmix-20200115-bsd.tar.gz` | 2 | 4 | 4.01 | -123 | -986 |
| gzip | `kensilverman-gz/kzipmix-20200115-linux-static.tar.gz` | 89 | 91 | 92.60 | -3712 | -29694 |
| gzip | `kensilverman-gz/kzipmix-20200115-linux.tar.gz` | 3 | 5 | 5.01 | -283 | -2271 |
| gzip | `kensilverman-gz/pngout-20200115-bsd.tar.gz` | 7 | 9 | 9.02 | -296 | -2370 |
| gzip | `kensilverman-gz/pngout-20200115-linux.tar.gz` | 8 | 10 | 10.02 | -711 | -5690 |
| gzip | `medium-gz/asyoulik-gzip.txt.gz` | 3 | 10 | 10.01 | -279 | -2233 |
| png | `8x8-png/CheckDiagonal.png` | 1 | 10 | 9.36 | 0 | -1 |
| png | `8x8-png/checked.png` | 1 | 10 | 9.81 | -2 | -17 |
| png | `8x8-png/lines.png` | 1 | 10 | 9.81 | 0 | -3 |
| png | `8x8-png/round.png` | 1 | 10 | 9.81 | -3 | -22 |
| png | `8x8-png/symbols.png` | 1 | 10 | 9.81 | 0 | 0 |
| png | `8x8-png/waves.png` | 1 | 10 | 9.81 | 0 | -2 |
| png | `PngSuite/PngSuite.png` | 1 | 10 | 9.81 | -79 | -631 |
| png | `PngSuite/basi0g01.png` | 1 | 10 | 9.81 | 0 | 0 |
| png | `PngSuite/basi0g02.png` | 1 | 10 | 9.81 | 0 | 0 |
| png | `PngSuite/basi0g04.png` | 1 | 10 | 9.81 | -1 | -5 |
| png | `PngSuite/basi0g08.png` | 1 | 10 | 9.81 | 0 | -2 |
| png | `PngSuite/basi0g16.png` | 1 | 10 | 9.81 | -2 | -13 |
| png | `PngSuite/basi2c08.png` | 1 | 10 | 9.81 | -2 | -11 |
| png | `PngSuite/basi2c16.png` | 1 | 10 | 9.81 | 0 | -5 |
| png | `PngSuite/basi3p01.png` | 1 | 10 | 4.05 | 0 | 0 |
| png | `PngSuite/basi3p02.png` | 1 | 10 | 9.81 | 0 | 0 |
| png | `PngSuite/basi3p04.png` | 1 | 10 | 9.81 | -6 | -49 |
| png | `PngSuite/basi3p08.png` | 1 | 10 | 9.81 | -18 | -143 |
| png | `PngSuite/basi4a08.png` | 1 | 10 | 9.81 | -1 | -4 |
| png | `PngSuite/basi4a16.png` | 1 | 10 | 9.81 | -2 | -13 |
| png | `PngSuite/basi6a08.png` | 1 | 10 | 9.81 | -1 | -4 |
| png | `PngSuite/basi6a16.png` | 1 | 10 | 9.81 | -2 | -10 |
| png | `PngSuite/basn0g01.png` | 1 | 10 | 9.81 | 0 | -2 |
| png | `PngSuite/basn0g02.png` | 1 | 10 | 0.29 | 0 | 0 |
| png | `PngSuite/basn0g04.png` | 1 | 10 | 2.59 | -1 | -14 |
| png | `PngSuite/basn0g08.png` | 1 | 10 | 8.69 | 0 | 0 |
| png | `PngSuite/basn0g16.png` | 1 | 10 | 8.81 | 0 | 0 |
| png | `PngSuite/basn2c08.png` | 1 | 10 | 9.81 | 0 | 0 |
| png | `PngSuite/basn2c16.png` | 1 | 10 | 9.81 | -1 | -4 |
| png | `PngSuite/basn3p01.png` | 1 | 10 | 0.09 | 0 | 0 |
| png | `PngSuite/basn3p02.png` | 1 | 10 | 1.93 | 0 | 0 |
| png | `PngSuite/basn3p04.png` | 1 | 10 | 0.91 | 0 | 0 |
| png | `PngSuite/basn3p08.png` | 1 | 10 | 9.81 | -21 | -171 |
| png | `PngSuite/basn4a08.png` | 1 | 10 | 9.81 | 0 | 0 |
| png | `PngSuite/basn4a16.png` | 1 | 10 | 9.81 | -1 | -1 |
| png | `PngSuite/basn6a08.png` | 1 | 10 | 9.81 | 0 | 0 |
| png | `PngSuite/basn6a16.png` | 1 | 10 | 9.81 | -1 | -8 |
| png | `PngSuite/bgai4a08.png` | 1 | 10 | 9.82 | -1 | -4 |
| png | `PngSuite/bgai4a16.png` | 1 | 10 | 9.81 | -2 | -13 |
| png | `PngSuite/bgan6a08.png` | 1 | 10 | 9.81 | 0 | 0 |
| png | `PngSuite/bgan6a16.png` | 1 | 10 | 9.81 | -1 | -8 |
| png | `PngSuite/bgbn4a08.png` | 1 | 10 | 9.81 | 0 | 0 |
| png | `PngSuite/bggn4a16.png` | 1 | 10 | 9.81 | -1 | -1 |
| png | `PngSuite/bgwn6a08.png` | 1 | 10 | 9.81 | 0 | 0 |
| png | `PngSuite/bgyn6a16.png` | 1 | 10 | 9.82 | -1 | -8 |
| png | `PngSuite/ccwn2c08.png` | 1 | 10 | 9.81 | 0 | -1 |
| png | `PngSuite/ccwn3p08.png` | 1 | 10 | 9.81 | 0 | 0 |
| png | `PngSuite/cdfn2c08.png` | 1 | 10 | 9.81 | -1 | -2 |
| png | `PngSuite/cdhn2c08.png` | 1 | 10 | 9.81 | -1 | -4 |
| png | `PngSuite/cdsn2c08.png` | 1 | 10 | 6.59 | 0 | -2 |
| png | `PngSuite/cdun2c08.png` | 1 | 10 | 9.81 | -1 | -8 |
| png | `PngSuite/ch1n3p04.png` | 1 | 10 | 0.91 | 0 | 0 |
| png | `PngSuite/ch2n3p08.png` | 1 | 10 | 9.81 | -21 | -171 |
| png | `PngSuite/cm0n0g04.png` | 1 | 10 | 9.81 | 2 | 14 |
| png | `PngSuite/cm9n0g04.png` | 1 | 10 | 9.81 | 2 | 14 |
| png | `PngSuite/cs3n2c16.png` | 1 | 10 | 9.81 | -1 | -6 |
| png | `PngSuite/cs3n3p08.png` | 1 | 10 | 9.81 | -1 | -10 |
| png | `PngSuite/cs5n2c08.png` | 1 | 10 | 9.81 | 0 | 0 |
| png | `PngSuite/cs5n3p08.png` | 1 | 10 | 9.81 | 0 | 0 |
| png | `PngSuite/cs8n2c08.png` | 1 | 10 | 7.70 | 0 | 0 |
| png | `PngSuite/cs8n3p08.png` | 1 | 10 | 9.81 | 0 | 0 |
| png | `PngSuite/ct0n0g04.png` | 1 | 10 | 9.81 | 2 | 14 |
| png | `PngSuite/ct1n0g04.png` | 1 | 10 | 9.81 | 2 | 14 |
| png | `PngSuite/cten0g04.png` | 1 | 10 | 6.65 | 0 | 0 |
| png | `PngSuite/ctfn0g04.png` | 1 | 10 | 3.03 | 0 | 0 |
| png | `PngSuite/ctgn0g04.png` | 1 | 10 | 4.14 | 0 | 0 |
| png | `PngSuite/cthn0g04.png` | 1 | 10 | 9.81 | -1 | -7 |
| png | `PngSuite/ctjn0g04.png` | 1 | 10 | 8.60 | -1 | -5 |
| png | `PngSuite/ctzn0g04.png` | 1 | 10 | 9.95 | 2 | 14 |
| png | `PngSuite/exif2c08.png` | 1 | 10 | 9.81 | -1 | -8 |
| png | `PngSuite/f00n0g08.png` | 1 | 10 | 9.81 | -15 | -120 |
| png | `PngSuite/f00n2c08.png` | 1 | 10 | 9.82 | -46 | -362 |
| png | `PngSuite/f01n0g08.png` | 1 | 10 | 9.81 | -1 | -13 |
| png | `PngSuite/f01n2c08.png` | 1 | 10 | 9.81 | -5 | -42 |
| png | `PngSuite/f02n0g08.png` | 1 | 10 | 9.81 | -1 | -11 |
| png | `PngSuite/f02n2c08.png` | 1 | 10 | 9.81 | 0 | -2 |
| png | `PngSuite/f03n0g08.png` | 1 | 10 | 9.81 | -4 | -32 |
| png | `PngSuite/f03n2c08.png` | 1 | 10 | 9.81 | -3 | -22 |
| png | `PngSuite/f04n0g08.png` | 1 | 10 | 9.81 | -1 | -7 |
| png | `PngSuite/f04n2c08.png` | 1 | 10 | 9.81 | -1 | -8 |
| png | `PngSuite/f99n0g04.png` | 1 | 10 | 9.81 | -6 | -43 |
| png | `PngSuite/g03n0g16.png` | 1 | 10 | 9.81 | -3 | -23 |
| png | `PngSuite/g03n2c08.png` | 1 | 10 | 9.81 | -1 | -8 |
| png | `PngSuite/g03n3p04.png` | 1 | 10 | 7.94 | 0 | 0 |
| png | `PngSuite/g04n0g16.png` | 1 | 10 | 9.81 | -2 | -19 |
| png | `PngSuite/g04n2c08.png` | 1 | 10 | 9.81 | -1 | -5 |
| png | `PngSuite/g04n3p04.png` | 1 | 10 | 9.81 | 0 | -2 |
| png | `PngSuite/g05n0g16.png` | 1 | 10 | 9.81 | -3 | -18 |
| png | `PngSuite/g05n2c08.png` | 1 | 10 | 9.81 | 0 | -3 |
| png | `PngSuite/g05n3p04.png` | 1 | 10 | 5.77 | 0 | 0 |
| png | `PngSuite/g07n0g16.png` | 1 | 10 | 9.81 | 0 | -1 |
| png | `PngSuite/g07n2c08.png` | 1 | 10 | 9.81 | -1 | -3 |
| png | `PngSuite/g07n3p04.png` | 1 | 10 | 5.67 | 0 | 0 |
| png | `PngSuite/g10n0g16.png` | 1 | 10 | 9.81 | 0 | -2 |
| png | `PngSuite/g10n2c08.png` | 1 | 10 | 9.81 | 0 | 0 |
| png | `PngSuite/g10n3p04.png` | 1 | 10 | 9.81 | -1 | -10 |
| png | `PngSuite/g25n0g16.png` | 1 | 10 | 9.81 | 0 | 0 |
| png | `PngSuite/g25n2c08.png` | 1 | 10 | 9.81 | -1 | -7 |
| png | `PngSuite/g25n3p04.png` | 1 | 10 | 9.81 | 0 | -2 |
| png | `PngSuite/oi1n0g16.png` | 1 | 10 | 8.74 | 0 | 0 |
| png | `PngSuite/oi1n2c16.png` | 1 | 10 | 9.81 | -1 | -4 |
| png | `PngSuite/oi2n0g16.png` | 1 | 10 | 8.77 | 0 | 0 |
| png | `PngSuite/oi2n2c16.png` | 1 | 10 | 9.81 | -1 | -4 |
| png | `PngSuite/oi4n0g16.png` | 1 | 10 | 8.75 | 0 | 0 |
| png | `PngSuite/oi4n2c16.png` | 1 | 10 | 9.81 | -1 | -4 |
| png | `PngSuite/oi9n0g16.png` | 1 | 10 | 8.79 | 0 | 0 |
| png | `PngSuite/oi9n2c16.png` | 1 | 10 | 9.81 | -1 | -4 |
| png | `PngSuite/pp0n2c16.png` | 1 | 10 | 9.81 | -1 | -4 |
| png | `PngSuite/pp0n6a08.png` | 1 | 10 | 9.81 | 0 | -4 |
| png | `PngSuite/ps1n0g08.png` | 1 | 10 | 8.73 | 0 | 0 |
| png | `PngSuite/ps1n2c16.png` | 1 | 10 | 9.81 | -1 | -4 |
| png | `PngSuite/ps2n0g08.png` | 1 | 10 | 8.71 | 0 | 0 |
| png | `PngSuite/ps2n2c16.png` | 1 | 10 | 9.81 | -1 | -4 |
| png | `PngSuite/s01i3p01.png` | 1 | 10 | 0.01 | 0 | 0 |
| png | `PngSuite/s01n3p01.png` | 1 | 10 | 0.01 | 0 | 0 |
| png | `PngSuite/s02i3p01.png` | 1 | 10 | 0.02 | 0 | 0 |
| png | `PngSuite/s02n3p01.png` | 1 | 10 | 0.02 | 0 | 0 |
| png | `PngSuite/s03i3p01.png` | 1 | 10 | 0.02 | 0 | 0 |
| png | `PngSuite/s03n3p01.png` | 1 | 10 | 0.01 | 0 | 0 |
| png | `PngSuite/s04i3p01.png` | 1 | 10 | 0.03 | 0 | 0 |
| png | `PngSuite/s04n3p01.png` | 1 | 10 | 0.03 | 0 | 0 |
| png | `PngSuite/s05i3p02.png` | 1 | 10 | 0.14 | 0 | 0 |
| png | `PngSuite/s05n3p02.png` | 1 | 10 | 0.03 | 0 | 0 |
| png | `PngSuite/s06i3p02.png` | 1 | 10 | 0.04 | 0 | 0 |
| png | `PngSuite/s06n3p02.png` | 1 | 10 | 0.05 | 0 | 0 |
| png | `PngSuite/s07i3p02.png` | 1 | 10 | 0.08 | 0 | 0 |
| png | `PngSuite/s07n3p02.png` | 1 | 10 | 0.06 | 0 | 0 |
| png | `PngSuite/s08i3p02.png` | 1 | 10 | 0.10 | 0 | 0 |
| png | `PngSuite/s08n3p02.png` | 1 | 10 | 0.11 | 0 | 0 |
| png | `PngSuite/s09i3p02.png` | 1 | 10 | 0.18 | 0 | 0 |
| png | `PngSuite/s09n3p02.png` | 1 | 10 | 0.44 | 0 | 0 |
| png | `PngSuite/s32i3p04.png` | 1 | 10 | 9.81 | -3 | -22 |
| png | `PngSuite/s32n3p04.png` | 1 | 10 | 9.81 | -1 | -9 |
| png | `PngSuite/s33i3p04.png` | 1 | 10 | 9.81 | -3 | -17 |
| png | `PngSuite/s33n3p04.png` | 1 | 10 | 9.81 | -5 | -38 |
| png | `PngSuite/s34i3p04.png` | 1 | 10 | 9.81 | -3 | -21 |
| png | `PngSuite/s34n3p04.png` | 1 | 10 | 9.81 | -2 | -11 |
| png | `PngSuite/s35i3p04.png` | 1 | 10 | 9.81 | -2 | -18 |
| png | `PngSuite/s35n3p04.png` | 1 | 10 | 9.81 | -5 | -40 |
| png | `PngSuite/s36i3p04.png` | 1 | 10 | 9.81 | -3 | -27 |
| png | `PngSuite/s36n3p04.png` | 1 | 10 | 9.81 | -1 | -10 |
| png | `PngSuite/s37i3p04.png` | 1 | 10 | 9.81 | -2 | -21 |
| png | `PngSuite/s37n3p04.png` | 1 | 10 | 9.81 | -5 | -37 |
| png | `PngSuite/s38i3p04.png` | 1 | 10 | 9.81 | -1 | -12 |
| png | `PngSuite/s38n3p04.png` | 1 | 10 | 9.81 | 0 | -2 |
| png | `PngSuite/s39i3p04.png` | 1 | 10 | 9.81 | -3 | -24 |
| png | `PngSuite/s39n3p04.png` | 1 | 10 | 9.81 | -4 | -30 |
| png | `PngSuite/s40i3p04.png` | 1 | 10 | 9.81 | -4 | -36 |
| png | `PngSuite/s40n3p04.png` | 1 | 10 | 9.81 | -2 | -18 |
| png | `PngSuite/tbbn0g04.png` | 1 | 10 | 9.81 | 0 | -2 |
| png | `PngSuite/tbbn2c16.png` | 1 | 10 | 9.81 | 0 | 0 |
| png | `PngSuite/tbbn3p08.png` | 1 | 10 | 9.81 | 0 | 0 |
| png | `PngSuite/tbgn2c16.png` | 1 | 10 | 9.81 | 0 | 0 |
| png | `PngSuite/tbgn3p08.png` | 1 | 10 | 9.81 | 0 | 0 |
| png | `PngSuite/tbrn2c08.png` | 1 | 10 | 9.81 | -3 | -28 |
| png | `PngSuite/tbwn0g16.png` | 1 | 10 | 9.81 | 0 | 0 |
| png | `PngSuite/tbwn3p08.png` | 1 | 10 | 9.82 | 0 | 0 |
| png | `PngSuite/tbyn3p08.png` | 1 | 10 | 9.81 | 0 | 0 |
| png | `PngSuite/tm3n3p02.png` | 1 | 10 | 0.05 | 0 | 0 |
| png | `PngSuite/tp0n0g08.png` | 1 | 10 | 9.81 | 0 | 0 |
| png | `PngSuite/tp0n2c08.png` | 1 | 10 | 9.81 | -3 | -27 |
| png | `PngSuite/tp0n3p08.png` | 1 | 10 | 9.81 | 0 | 0 |
| png | `PngSuite/tp1n3p08.png` | 1 | 10 | 9.81 | 0 | 0 |
| png | `PngSuite/z00n2c08.png` | 1 | 10 | 1.00 | -2581 | -20652 |
| png | `PngSuite/z03n2c08.png` | 1 | 10 | 9.81 | -2 | -16 |
| png | `PngSuite/z06n2c08.png` | 1 | 10 | 9.81 | -1 | -2 |
| png | `PngSuite/z09n2c08.png` | 1 | 10 | 9.81 | -1 | -2 |
| png | `apng-large/3d2.png` | 27 | 29 | 28.95 | -272 | -2189 |
| png | `apng-large/MediaWiki-2020-large-icon-spinning.png` | 49 | 51 | 50.93 | -1059 | -8471 |
| png | `apng-large/animation_icos4d.orig.png` | 18 | 20 | 19.97 | -2036 | -16246 |
| png | `apng-large/animation_icos4d.ref.png` | 71 | 73 | 72.79 | -305 | -2456 |
| png | `apng-large/elephant-2.png` | 17 | 19 | 18.97 | -123 | -987 |
| png | `apng-medium/APNG_throbber.png` | 21 | 23 | 22.96 | -10 | -101 |
| png | `apng-medium/Biker.png` | 15 | 17 | 16.95 | -42 | -330 |
| png | `apng-medium/Blueball.png` | 7 | 10 | 10.27 | -279 | -2227 |
| png | `apng-medium/Dharma_Wheelmmm-APNG-animation2.png` | 4 | 10 | 10.64 | -136 | -1086 |
| png | `apng-medium/FxAnimated128.png` | 8 | 10 | 9.99 | -216 | -1729 |
| png | `apng-medium/FxAnimated64.png` | 2 | 10 | 9.96 | -9 | -62 |
| png | `apng-medium/Ikosaeder-Animation.png` | 7 | 10 | 10.77 | -9 | -80 |
| png | `apng-medium/Lotus-buddha-APNG-animation.png` | 12 | 14 | 13.97 | -34 | -275 |
| png | `apng-medium/ball.png` | 6 | 10 | 10.59 | -16 | -120 |
| png | `apng-medium/clock.png` | 7 | 10 | 9.96 | -99 | -801 |
| png | `apng-medium/o_sample.png` | 9 | 11 | 10.99 | -66 | -520 |
| png | `apng-small/Animated.png` | 2 | 10 | 9.96 | -22 | -187 |
| png | `apng-small/Load.png` | 1 | 10 | 0.06 | 0 | 0 |
| png | `css-ig-net/16-c3-8bits-4bits.png` | 1 | 10 | 9.81 | -4 | -38 |
| png | `css-ig-net/24-chunks.png` | 1 | 10 | 0.03 | 0 | 0 |
| png | `css-ig-net/Apple512.png` | 95 | 97 | 95.24 | -2661 | -21290 |
| png | `css-ig-net/Apricot512.png` | 24 | 26 | 25.69 | -2381 | -19042 |
| png | `css-ig-net/Bag.png` | 7 | 10 | 9.89 | -115 | -920 |
| png | `css-ig-net/Ball.png` | 8 | 10 | 9.94 | -522 | -4174 |
| png | `css-ig-net/Banana512.png` | 44 | 46 | 45.21 | -2826 | -22603 |
| png | `css-ig-net/Bullfinch.png` | 8 | 10 | 9.92 | -9 | -77 |
| png | `css-ig-net/Cherry512.png` | 24 | 26 | 25.67 | -1468 | -11743 |
| png | `css-ig-net/Cookies.png` | 6 | 10 | 9.90 | -315 | -2523 |
| png | `css-ig-net/Decorations.png` | 6 | 10 | 9.90 | -230 | -1838 |
| png | `css-ig-net/Gifts.png` | 6 | 10 | 9.89 | -266 | -2128 |
| png | `css-ig-net/Gingerman.png` | 4 | 10 | 9.91 | -1 | -12 |
| png | `css-ig-net/Kiwi512.png` | 81 | 83 | 81.67 | -1280 | -10237 |
| png | `css-ig-net/Lemon512.png` | 97 | 99 | 97.33 | -7 | -56 |
| png | `css-ig-net/Mango512.png` | 14 | 16 | 15.82 | -8638 | -69105 |
| png | `css-ig-net/Mittens.png` | 3 | 10 | 9.86 | -96 | -772 |
| png | `css-ig-net/Nutcracker.png` | 5 | 10 | 9.87 | -38 | -305 |
| png | `css-ig-net/Orange512.png` | 41 | 43 | 42.37 | -1650 | -13204 |
| png | `css-ig-net/Peach512.png` | 143 | 145 | 142.31 | -3706 | -29649 |
| png | `css-ig-net/Pear512.png` | 59 | 61 | 59.96 | -3042 | -24341 |
| png | `css-ig-net/Sock.png` | 10 | 12 | 11.88 | -190 | -1519 |
| png | `css-ig-net/Strawberry512.png` | 88 | 90 | 88.43 | -914 | -7313 |
| png | `css-ig-net/Tomato512.png` | 103 | 105 | 103.14 | -1645 | -13158 |
| png | `css-ig-net/achtung-fs8.png` | 3 | 10 | 9.83 | -32 | -252 |
| png | `css-ig-net/anchor.png` | 1 | 10 | 9.82 | -1 | -10 |
| png | `css-ig-net/aperture.png` | 1 | 10 | 9.82 | -1 | -5 |
| png | `css-ig-net/apple-fs8.png` | 2 | 10 | 9.83 | -71 | -573 |
| png | `css-ig-net/arrow-down.png` | 1 | 10 | 9.81 | 0 | 0 |
| png | `css-ig-net/arrow-up.png` | 1 | 10 | 9.81 | -2 | -15 |
| png | `css-ig-net/art.png` | 1 | 10 | 9.82 | -3 | -20 |
| png | `css-ig-net/bad-paletted.png` | 1 | 10 | 9.81 | -67 | -538 |
| png | `css-ig-net/barchart.png` | 1 | 10 | 9.81 | -2 | -18 |
| png | `css-ig-net/batteryfull.png` | 1 | 10 | 9.81 | 1 | 8 |
| png | `css-ig-net/batterylow.png` | 1 | 10 | 9.82 | -1 | -12 |
| png | `css-ig-net/bike.png` | 1 | 10 | 9.81 | -2 | -13 |
| png | `css-ig-net/biker.png` | 1 | 10 | 9.81 | 1 | 12 |
| png | `css-ig-net/bikewheel.png` | 1 | 10 | 9.81 | 0 | -2 |
| png | `css-ig-net/blimp.png` | 1 | 10 | 9.82 | -1 | -3 |
| png | `css-ig-net/bluetooth-fs8.png` | 3 | 10 | 9.83 | -6 | -48 |
| png | `css-ig-net/bolt.png` | 1 | 10 | 9.81 | 0 | -3 |
| png | `css-ig-net/bomb.png` | 1 | 10 | 9.81 | 1 | 14 |
| png | `css-ig-net/booklet.png` | 1 | 10 | 9.81 | -2 | -9 |
| png | `css-ig-net/bookshelf.png` | 1 | 10 | 9.81 | 0 | -7 |
| png | `css-ig-net/bouee-fs8.png` | 2 | 10 | 9.83 | -3 | -26 |
| png | `css-ig-net/briefcase.png` | 1 | 10 | 9.82 | -5 | -46 |
| png | `css-ig-net/brightness.png` | 1 | 10 | 9.82 | -1 | -7 |
| png | `css-ig-net/browser.png` | 1 | 10 | 9.81 | 0 | -1 |
| png | `css-ig-net/brush-fs8.png` | 1 | 10 | 9.82 | -7 | -54 |
| png | `css-ig-net/brush-pencil.png` | 1 | 10 | 9.81 | -1 | -15 |
| png | `css-ig-net/cadenas-fs8.png` | 3 | 10 | 9.83 | -4 | -38 |
| png | `css-ig-net/calculator.png` | 1 | 10 | 9.81 | -1 | -7 |
| png | `css-ig-net/calendar.png` | 1 | 10 | 9.81 | -2 | -14 |
| png | `css-ig-net/camera.png` | 1 | 10 | 9.81 | -1 | -11 |
| png | `css-ig-net/car.png` | 2 | 10 | 9.81 | -11 | -87 |
| png | `css-ig-net/cart.png` | 1 | 10 | 9.81 | -2 | -12 |
| png | `css-ig-net/carton-fs8.png` | 3 | 10 | 9.84 | -2 | -11 |
| png | `css-ig-net/carwheel.png` | 1 | 10 | 9.82 | -3 | -18 |
| png | `css-ig-net/caution.png` | 1 | 10 | 9.81 | -3 | -25 |
| png | `css-ig-net/chat.png` | 1 | 10 | 9.81 | -1 | -7 |
| png | `css-ig-net/check.png` | 1 | 10 | 9.81 | -4 | -33 |
| png | `css-ig-net/circlecompass.png` | 1 | 10 | 9.82 | 0 | -3 |
| png | `css-ig-net/clapboard.png` | 1 | 10 | 9.81 | -3 | -26 |
| png | `css-ig-net/clipboard.png` | 1 | 10 | 9.82 | -2 | -18 |
| png | `css-ig-net/clock.png` | 1 | 10 | 9.81 | -1 | -12 |
| png | `css-ig-net/cloud.png` | 1 | 10 | 9.81 | -1 | -9 |
| png | `css-ig-net/cmyk.png` | 1 | 10 | 9.81 | -2 | -12 |
| png | `css-ig-net/colorwheel.png` | 1 | 10 | 9.81 | 0 | -3 |
| png | `css-ig-net/compass.png` | 1 | 10 | 9.82 | -1 | -3 |
| png | `css-ig-net/compose.png` | 1 | 10 | 9.81 | 4 | 35 |
| png | `css-ig-net/computer.png` | 1 | 10 | 9.81 | -3 | -22 |
| png | `css-ig-net/cone.png` | 1 | 10 | 9.81 | 0 | -1 |
| png | `css-ig-net/contacts.png` | 1 | 10 | 9.82 | -1 | -5 |
| png | `css-ig-net/contrast.png` | 1 | 10 | 9.81 | 0 | 0 |
| png | `css-ig-net/countdown.png` | 1 | 10 | 9.82 | -4 | -26 |
| png | `css-ig-net/creditcard.png` | 1 | 10 | 9.81 | -3 | -17 |
| png | `css-ig-net/crop.png` | 1 | 10 | 9.82 | -1 | -9 |
| png | `css-ig-net/crossroads.png` | 1 | 10 | 9.81 | -3 | -24 |
| png | `css-ig-net/cruise.png` | 1 | 10 | 9.81 | -1 | -11 |
| png | `css-ig-net/cursor.png` | 1 | 10 | 9.81 | -2 | -10 |
| png | `css-ig-net/denied.png` | 1 | 10 | 9.81 | -1 | -10 |
| png | `css-ig-net/dev.png` | 1 | 10 | 9.81 | -1 | -9 |
| png | `css-ig-net/die.png` | 1 | 10 | 9.82 | -3 | -25 |
| png | `css-ig-net/document.png` | 1 | 10 | 9.81 | 0 | -4 |
| png | `css-ig-net/dolly.png` | 1 | 10 | 9.81 | -1 | -7 |
| png | `css-ig-net/door.png` | 1 | 10 | 9.82 | -2 | -11 |
| png | `css-ig-net/dossier-blue-papier-fs8.png` | 4 | 10 | 9.83 | -12 | -99 |
| png | `css-ig-net/dossier-blue-pictures-fs8.png` | 3 | 10 | 9.84 | -7 | -57 |
| png | `css-ig-net/dossier-green-musique-fs8.png` | 4 | 10 | 9.83 | -17 | -135 |
| png | `css-ig-net/dossier-green-normal-fs8.png` | 3 | 10 | 9.84 | -35 | -284 |
| png | `css-ig-net/dossier-green-papier-fs8.png` | 3 | 10 | 9.83 | -3 | -24 |
| png | `css-ig-net/dossier-green-pictures-fs8.png` | 2 | 10 | 9.83 | -2 | -13 |
| png | `css-ig-net/download.png` | 1 | 10 | 9.81 | 0 | -3 |
| png | `css-ig-net/drive-blue-disk-fs8.png` | 3 | 10 | 9.84 | -73 | -590 |
| png | `css-ig-net/drive-blue-network-fs8.png` | 3 | 10 | 9.83 | -66 | -528 |
| png | `css-ig-net/drive-blue-usb-fs8.png` | 3 | 10 | 9.83 | -49 | -394 |
| png | `css-ig-net/drive-green-disk-fs8.png` | 3 | 10 | 9.83 | -61 | -484 |
| png | `css-ig-net/drive-green-network-fs8.png` | 2 | 10 | 9.84 | -60 | -484 |
| png | `css-ig-net/drive-green-usb-fs8.png` | 3 | 10 | 9.83 | -58 | -463 |
| png | `css-ig-net/easel.png` | 1 | 10 | 9.81 | -1 | -10 |
| png | `css-ig-net/eggz-fs8.png` | 3 | 10 | 9.83 | -2 | -15 |
| png | `css-ig-net/email.png` | 1 | 10 | 9.82 | -2 | -17 |
| png | `css-ig-net/eye.png` | 1 | 10 | 9.81 | -1 | -5 |
| png | `css-ig-net/eyedropper.png` | 2 | 10 | 9.81 | -3 | -24 |
| png | `css-ig-net/fashion.png` | 1 | 10 | 9.81 | -1 | -9 |
| png | `css-ig-net/file01.png` | 43 | 45 | 44.58 | -1645 | -13156 |
| png | `css-ig-net/file02.png` | 20 | 22 | 21.94 | -907 | -7253 |
| png | `css-ig-net/file03.png` | 21 | 23 | 23.00 | -549 | -4393 |
| png | `css-ig-net/file04.png` | 28 | 30 | 29.73 | -843 | -6748 |
| png | `css-ig-net/file05.png` | 34 | 36 | 35.52 | -1640 | -13121 |
| png | `css-ig-net/file06.png` | 23 | 25 | 24.84 | -865 | -6921 |
| png | `css-ig-net/file07.png` | 16 | 18 | 17.74 | -1253 | -10030 |
| png | `css-ig-net/file08.png` | 26 | 28 | 27.94 | -869 | -6953 |
| png | `css-ig-net/file09.png` | 95 | 97 | 95.42 | -1912 | -15296 |
| png | `css-ig-net/file10.png` | 20 | 22 | 21.91 | -1475 | -11800 |
| png | `css-ig-net/filmreel.png` | 1 | 10 | 9.81 | -9 | -68 |
| png | `css-ig-net/filmroll.png` | 1 | 10 | 9.81 | 3 | 22 |
| png | `css-ig-net/flag.png` | 1 | 10 | 9.81 | -8 | -62 |
| png | `css-ig-net/flame.png` | 1 | 10 | 9.82 | -1 | -8 |
| png | `css-ig-net/flash.png` | 1 | 10 | 9.81 | -2 | -11 |
| png | `css-ig-net/flower.png` | 1 | 10 | 9.81 | -1 | -7 |
| png | `css-ig-net/focus.png` | 1 | 10 | 9.81 | 0 | -1 |
| png | `css-ig-net/folder.png` | 1 | 10 | 9.81 | -2 | -21 |
| png | `css-ig-net/frames.png` | 1 | 10 | 9.81 | 2 | 16 |
| png | `css-ig-net/gamecontroller.png` | 1 | 10 | 9.81 | 0 | 1 |
| png | `css-ig-net/gas.png` | 1 | 10 | 9.82 | -4 | -27 |
| png | `css-ig-net/gear.png` | 1 | 10 | 9.81 | 0 | -3 |
| png | `css-ig-net/genius.png` | 1 | 10 | 9.81 | 0 | 1 |
| png | `css-ig-net/global.png` | 1 | 10 | 9.82 | -2 | -13 |
| png | `css-ig-net/globe.png` | 1 | 10 | 9.81 | 0 | 0 |
| png | `css-ig-net/gps.png` | 1 | 10 | 9.81 | -1 | -9 |
| png | `css-ig-net/hazard.png` | 1 | 10 | 9.81 | -1 | -5 |
| png | `css-ig-net/heart-fs8.png` | 2 | 10 | 9.83 | -5 | -40 |
| png | `css-ig-net/heart.png` | 1 | 10 | 9.81 | -5 | -39 |
| png | `css-ig-net/heart2-fs8.png` | 2 | 10 | 9.83 | -7 | -59 |
| png | `css-ig-net/helicopter.png` | 1 | 10 | 9.81 | -1 | -2 |
| png | `css-ig-net/hotair.png` | 1 | 10 | 9.82 | -2 | -16 |
| png | `css-ig-net/hourglass.png` | 1 | 10 | 9.82 | 0 | -1 |
| png | `css-ig-net/icone_windows-fs8.png` | 2 | 10 | 9.83 | 1 | 1 |
| png | `css-ig-net/image.png` | 1 | 10 | 9.82 | -2 | -12 |
| png | `css-ig-net/interstate.png` | 1 | 10 | 9.81 | 0 | 0 |
| png | `css-ig-net/key.png` | 1 | 10 | 9.81 | 2 | 13 |
| png | `css-ig-net/keyboard.png` | 1 | 10 | 9.81 | -1 | -10 |
| png | `css-ig-net/lens.png` | 1 | 10 | 9.81 | -1 | -7 |
| png | `css-ig-net/lightbulb.png` | 1 | 10 | 9.81 | 0 | -6 |
| png | `css-ig-net/loading.png` | 1 | 10 | 9.81 | -1 | -9 |
| png | `css-ig-net/location.png` | 1 | 10 | 9.81 | 0 | -1 |
| png | `css-ig-net/locked.png` | 1 | 10 | 9.81 | -3 | -21 |
| png | `css-ig-net/loupe-fs8.png` | 2 | 10 | 9.83 | -2 | -20 |
| png | `css-ig-net/magicwand.png` | 1 | 10 | 9.81 | -7 | -50 |
| png | `css-ig-net/magnifyingglass.png` | 2 | 10 | 9.82 | -1 | -10 |
| png | `css-ig-net/mail-fs8.png` | 4 | 10 | 9.84 | -55 | -437 |
| png | `css-ig-net/mail.png` | 1 | 10 | 9.81 | -1 | -9 |
| png | `css-ig-net/map.png` | 1 | 10 | 9.81 | 7 | 56 |
| png | `css-ig-net/megaphone.png` | 1 | 10 | 9.82 | -4 | -26 |
| png | `css-ig-net/megaphone2.png` | 1 | 10 | 9.81 | 0 | 0 |
| png | `css-ig-net/memorycard.png` | 1 | 10 | 9.81 | -1 | -8 |
| png | `css-ig-net/merge.png` | 1 | 10 | 9.81 | -1 | -7 |
| png | `css-ig-net/mic.png` | 1 | 10 | 9.81 | 0 | -1 |
| png | `css-ig-net/microphone.png` | 1 | 10 | 9.81 | -2 | -16 |
| png | `css-ig-net/money.png` | 1 | 10 | 9.81 | -2 | -9 |
| png | `css-ig-net/motorcycle.png` | 1 | 10 | 9.82 | -7 | -58 |
| png | `css-ig-net/music.png` | 1 | 10 | 9.81 | 0 | -2 |
| png | `css-ig-net/my-computer-off-fs8.png` | 3 | 10 | 9.83 | -1 | -10 |
| png | `css-ig-net/my-computer-on-fs8.png` | 3 | 10 | 9.81 | -1 | -12 |
| png | `css-ig-net/news.png` | 1 | 10 | 9.80 | -3 | -26 |
| png | `css-ig-net/paintbrush.png` | 2 | 10 | 9.80 | 0 | 0 |
| png | `css-ig-net/paintbrush2.png` | 1 | 10 | 9.80 | 0 | -3 |
| png | `css-ig-net/paintcan.png` | 1 | 10 | 9.80 | 0 | 0 |
| png | `css-ig-net/paintroller.png` | 1 | 10 | 9.80 | -1 | -3 |
| png | `css-ig-net/paperplane-fs8.png` | 2 | 10 | 9.81 | -43 | -340 |
| png | `css-ig-net/parachute.png` | 1 | 10 | 9.81 | -7 | -52 |
| png | `css-ig-net/pencil.png` | 1 | 10 | 9.80 | -5 | -38 |
| png | `css-ig-net/phone.png` | 1 | 10 | 9.80 | -1 | -14 |
| png | `css-ig-net/pie-chart.png` | 1 | 10 | 9.80 | -1 | -10 |
| png | `css-ig-net/pin.png` | 1 | 10 | 9.80 | -1 | -6 |
| png | `css-ig-net/pin2.png` | 1 | 10 | 9.80 | -5 | -37 |
| png | `css-ig-net/plane.png` | 1 | 10 | 9.80 | 0 | 0 |
| png | `css-ig-net/play.png` | 1 | 10 | 9.81 | -1 | -7 |
| png | `css-ig-net/plugin.png` | 1 | 10 | 9.80 | 0 | 0 |
| png | `css-ig-net/png-logo-graybad.png` | 1 | 10 | 9.80 | -1 | -8 |
| png | `css-ig-net/polaroid.png` | 1 | 10 | 9.80 | 0 | -3 |
| png | `css-ig-net/polaroidcamera.png` | 1 | 10 | 9.80 | -2 | -10 |
| png | `css-ig-net/polaroids.png` | 1 | 10 | 9.80 | -2 | -16 |
| png | `css-ig-net/power.png` | 1 | 10 | 9.80 | 0 | -2 |
| png | `css-ig-net/present.png` | 1 | 10 | 9.80 | -2 | -15 |
| png | `css-ig-net/profle.png` | 1 | 10 | 9.80 | 0 | -2 |
| png | `css-ig-net/quote.png` | 1 | 10 | 9.80 | -1 | -9 |
| png | `css-ig-net/racingflags.png` | 1 | 10 | 9.80 | -1 | -2 |
| png | `css-ig-net/radio.png` | 1 | 10 | 9.80 | -2 | -11 |
| png | `css-ig-net/radiotower.png` | 1 | 10 | 9.81 | -1 | -12 |
| png | `css-ig-net/rainbow.png` | 1 | 10 | 9.81 | -1 | -7 |
| png | `css-ig-net/recycle.png` | 1 | 10 | 9.81 | -2 | -15 |
| png | `css-ig-net/rgb.png` | 1 | 10 | 9.80 | -1 | -3 |
| png | `css-ig-net/ribbon.png` | 1 | 10 | 9.80 | 0 | -1 |
| png | `css-ig-net/roadblock.png` | 1 | 10 | 9.80 | -8 | -60 |
| png | `css-ig-net/rocket.png` | 2 | 10 | 9.81 | -1 | -13 |
| png | `css-ig-net/rssfeed-fs8.png` | 3 | 10 | 9.81 | -5 | -36 |
| png | `css-ig-net/rulertriangle.png` | 1 | 10 | 9.80 | -1 | -9 |
| png | `css-ig-net/running.png` | 1 | 10 | 9.80 | -1 | -10 |
| png | `css-ig-net/sailboat.png` | 1 | 10 | 9.80 | -1 | -1 |
| png | `css-ig-net/sample_01-fs8.png` | 1 | 10 | 9.81 | -2 | -23 |
| png | `css-ig-net/sample_02-fs8.png` | 1 | 10 | 9.80 | -10 | -78 |
| png | `css-ig-net/sample_02.png` | 1 | 10 | 9.80 | -3 | -19 |
| png | `css-ig-net/sample_03.png` | 1 | 10 | 9.80 | -1 | -7 |
| png | `css-ig-net/sample_04-fs8.png` | 1 | 10 | 9.80 | -44 | -355 |
| png | `css-ig-net/sample_05-fs8.png` | 1 | 10 | 9.81 | -1 | -6 |
| png | `css-ig-net/sample_08-fs8.png` | 1 | 10 | 9.81 | -8 | -63 |
| png | `css-ig-net/sample_08.png` | 2 | 10 | 9.81 | -6 | -50 |
| png | `css-ig-net/sample_09.png` | 1 | 10 | 9.81 | -2 | -14 |
| png | `css-ig-net/sample_11-fs8.png` | 1 | 10 | 9.80 | -35 | -273 |
| png | `css-ig-net/sample_12.png` | 1 | 10 | 9.81 | -2 | -18 |
| png | `css-ig-net/sample_14-fs8.png` | 2 | 10 | 9.81 | -89 | -715 |
| png | `css-ig-net/sample_14.png` | 4 | 10 | 9.81 | -16 | -129 |
| png | `css-ig-net/sample_15-fs8.png` | 1 | 10 | 9.80 | -23 | -186 |
| png | `css-ig-net/sample_17-fs8.png` | 2 | 10 | 9.80 | -11 | -87 |
| png | `css-ig-net/sample_17.png` | 2 | 10 | 9.81 | -32 | -257 |
| png | `css-ig-net/sample_18.png` | 3 | 10 | 9.81 | -5 | -39 |
| png | `css-ig-net/sample_19-fs8.png` | 2 | 10 | 9.80 | -12 | -99 |
| png | `css-ig-net/sample_19.png` | 1 | 10 | 9.81 | -9 | -65 |
| png | `css-ig-net/sample_20-fs8.png` | 1 | 10 | 9.80 | -8 | -67 |
| png | `css-ig-net/sample_20.png` | 1 | 10 | 9.81 | -115 | -923 |
| png | `css-ig-net/sample_21-fs8.png` | 1 | 10 | 9.80 | -10 | -85 |
| png | `css-ig-net/sample_22-fs8.png` | 2 | 10 | 9.81 | -4 | -28 |
| png | `css-ig-net/sample_22.png` | 2 | 10 | 9.81 | -10 | -86 |
| png | `css-ig-net/sample_23-fs8.png` | 1 | 10 | 9.81 | -27 | -212 |
| png | `css-ig-net/sample_24-fs8.png` | 1 | 10 | 9.81 | -18 | -145 |
| png | `css-ig-net/sample_24.png` | 1 | 10 | 9.81 | -2 | -18 |
| png | `css-ig-net/sample_25.png` | 2 | 10 | 9.81 | -1 | -9 |
| png | `css-ig-net/sample_26-fs8.png` | 1 | 10 | 9.81 | -16 | -127 |
| png | `css-ig-net/sample_26.png` | 1 | 10 | 9.81 | -5 | -39 |
| png | `css-ig-net/sample_27-fs8.png` | 2 | 10 | 9.81 | -5 | -39 |
| png | `css-ig-net/sample_28-fs8.png` | 1 | 10 | 9.81 | -35 | -279 |
| png | `css-ig-net/sample_28.png` | 2 | 10 | 9.81 | -1 | -13 |
| png | `css-ig-net/sample_29-fs8.png` | 1 | 10 | 9.80 | -9 | -73 |
| png | `css-ig-net/sample_29.png` | 1 | 10 | 9.81 | -1 | -10 |
| png | `css-ig-net/sample_30-fs8.png` | 2 | 10 | 9.81 | -22 | -176 |
| png | `css-ig-net/sample_31-fs8.png` | 2 | 10 | 9.80 | -17 | -136 |
| png | `css-ig-net/sample_32-fs8.png` | 1 | 10 | 9.81 | -17 | -136 |
| png | `css-ig-net/sample_32.png` | 2 | 10 | 9.81 | -1 | -11 |
| png | `css-ig-net/sample_33-fs8.png` | 2 | 10 | 9.80 | -4 | -37 |
| png | `css-ig-net/sample_34-fs8.png` | 2 | 10 | 9.81 | -46 | -371 |
| png | `css-ig-net/sample_35-fs8.png` | 2 | 10 | 9.81 | -12 | -91 |
| png | `css-ig-net/sample_35.png` | 2 | 10 | 9.81 | 0 | -2 |
| png | `css-ig-net/sample_36-fs8.png` | 2 | 10 | 9.81 | -13 | -106 |
| png | `css-ig-net/sample_37-fs8.png` | 2 | 10 | 9.80 | -4 | -38 |
| png | `css-ig-net/sample_37.png` | 1 | 10 | 9.81 | -3 | -25 |
| png | `css-ig-net/sample_38-fs8.png` | 2 | 10 | 9.80 | -13 | -101 |
| png | `css-ig-net/sample_39-fs8.png` | 2 | 10 | 9.81 | -72 | -574 |
| png | `css-ig-net/sample_39.png` | 2 | 10 | 9.81 | -39 | -311 |
| png | `css-ig-net/sample_40-fs8.png` | 2 | 10 | 9.81 | -7 | -58 |
| png | `css-ig-net/sample_40.png` | 1 | 10 | 9.81 | -2 | -20 |
| png | `css-ig-net/sample_41-fs8.png` | 1 | 10 | 9.81 | -18 | -143 |
| png | `css-ig-net/sample_42-fs8.png` | 2 | 10 | 9.80 | -1 | -9 |
| png | `css-ig-net/sample_43-fs8.png` | 2 | 10 | 9.80 | -40 | -319 |
| png | `css-ig-net/sample_44-fs8.png` | 2 | 10 | 9.81 | -16 | -122 |
| png | `css-ig-net/sample_45-fs8.png` | 2 | 10 | 9.80 | -2 | -16 |
| png | `css-ig-net/sample_45.png` | 11 | 13 | 12.75 | -55 | -442 |
| png | `css-ig-net/sample_46-fs8.png` | 2 | 10 | 9.81 | -3 | -21 |
| png | `css-ig-net/sample_46.png` | 1 | 10 | 9.81 | -1 | -3 |
| png | `css-ig-net/sample_47-fs8.png` | 1 | 10 | 9.81 | -3 | -29 |
| png | `css-ig-net/sample_48-fs8.png` | 1 | 10 | 9.81 | -8 | -62 |
| png | `css-ig-net/sample_49-fs8.png` | 2 | 10 | 9.81 | -9 | -71 |
| png | `css-ig-net/sample_50-fs8.png` | 1 | 10 | 9.81 | -31 | -249 |
| png | `css-ig-net/sample_51-fs8.png` | 2 | 10 | 9.80 | -7 | -61 |
| png | `css-ig-net/sample_52-fs8.png` | 2 | 10 | 9.81 | -4 | -26 |
| png | `css-ig-net/sample_53-fs8.png` | 1 | 10 | 9.81 | -2 | -17 |
| png | `css-ig-net/sample_53.png` | 2 | 10 | 9.81 | -29 | -234 |
| png | `css-ig-net/sample_54-fs8.png` | 2 | 10 | 9.81 | -7 | -56 |
| png | `css-ig-net/sample_55-fs8.png` | 2 | 10 | 9.80 | -2 | -14 |
| png | `css-ig-net/sample_56-fs8.png` | 1 | 10 | 9.81 | -4 | -30 |
| png | `css-ig-net/sample_57-fs8.png` | 2 | 10 | 9.81 | -4 | -35 |
| png | `css-ig-net/sample_57.png` | 1 | 10 | 9.81 | -2 | -11 |
| png | `css-ig-net/sample_58-fs8.png` | 2 | 10 | 9.81 | -6 | -46 |
| png | `css-ig-net/sample_59-fs8.png` | 2 | 10 | 9.81 | -28 | -222 |
| png | `css-ig-net/sample_59.png` | 4 | 10 | 9.81 | -28 | -224 |
| png | `css-ig-net/sample_60-fs8.png` | 2 | 10 | 9.81 | -24 | -194 |
| png | `css-ig-net/sample_60.png` | 5 | 10 | 9.81 | -11 | -88 |
| png | `css-ig-net/sample_61-fs8.png` | 2 | 10 | 9.81 | -4 | -36 |
| png | `css-ig-net/sample_61.png` | 3 | 10 | 9.81 | -1 | -10 |
| png | `css-ig-net/sample_62-fs8.png` | 2 | 10 | 9.80 | -10 | -76 |
| png | `css-ig-net/sample_63-fs8.png` | 3 | 10 | 9.81 | -40 | -318 |
| png | `css-ig-net/sample_63.png` | 3 | 10 | 9.82 | -73 | -585 |
| png | `css-ig-net/sample_64-fs8.png` | 2 | 10 | 9.81 | -1 | -7 |
| png | `css-ig-net/sample_64.png` | 2 | 10 | 9.81 | -5 | -39 |
| png | `css-ig-net/sample_65-fs8.png` | 1 | 10 | 9.80 | -19 | -150 |
| png | `css-ig-net/sample_65.png` | 4 | 10 | 9.81 | -3 | -29 |
| png | `css-ig-net/sample_66-fs8.png` | 2 | 10 | 9.81 | -5 | -41 |
| png | `css-ig-net/sample_66.png` | 7 | 10 | 9.84 | -63 | -503 |
| png | `css-ig-net/sample_67-fs8.png` | 3 | 10 | 9.81 | -14 | -108 |
| png | `css-ig-net/sample_67.png` | 2 | 10 | 9.82 | -3 | -23 |
| png | `css-ig-net/sample_68-fs8.png` | 2 | 10 | 9.81 | -18 | -148 |
| png | `css-ig-net/sample_69-fs8.png` | 4 | 10 | 9.80 | -4 | -36 |
| png | `css-ig-net/sample_69.png` | 1 | 10 | 9.81 | 0 | 0 |
| png | `css-ig-net/sample_70-fs8.png` | 2 | 10 | 9.81 | -3 | -30 |
| png | `css-ig-net/sample_70.png` | 3 | 10 | 9.82 | -42 | -335 |
| png | `css-ig-net/sample_71-fs8.png` | 3 | 10 | 9.81 | -13 | -103 |
| png | `css-ig-net/sample_71.png` | 4 | 10 | 9.82 | -8 | -61 |
| png | `css-ig-net/santa-hat-fs8.png` | 3 | 10 | 9.81 | 0 | -7 |
| png | `css-ig-net/schooolbus.png` | 2 | 10 | 9.80 | -9 | -72 |
| png | `css-ig-net/scissors.png` | 1 | 10 | 9.80 | -1 | -14 |
| png | `css-ig-net/scooter.png` | 1 | 10 | 9.81 | -2 | -19 |
| png | `css-ig-net/security.png` | 1 | 10 | 9.80 | 0 | 0 |
| png | `css-ig-net/selftimer.png` | 1 | 10 | 9.81 | -1 | -7 |
| png | `css-ig-net/settings.png` | 1 | 10 | 9.80 | -2 | -12 |
| png | `css-ig-net/shipwheel.png` | 1 | 10 | 9.81 | -2 | -20 |
| png | `css-ig-net/shoeprints.png` | 1 | 10 | 9.80 | -1 | -12 |
| png | `css-ig-net/shop.png` | 1 | 10 | 9.80 | -2 | -15 |
| png | `css-ig-net/sims-fs8.png` | 3 | 10 | 9.81 | -4 | -32 |
| png | `css-ig-net/skateboard.png` | 1 | 10 | 9.80 | -1 | -8 |
| png | `css-ig-net/slr.png` | 1 | 10 | 9.80 | 0 | 0 |
| png | `css-ig-net/smartphone.png` | 1 | 10 | 9.80 | -4 | -32 |
| png | `css-ig-net/spaceshuttle.png` | 1 | 10 | 9.81 | -2 | -19 |
| png | `css-ig-net/speaker.png` | 1 | 10 | 9.81 | -2 | -15 |
| png | `css-ig-net/speedometer.png` | 1 | 10 | 9.80 | 0 | -2 |
| png | `css-ig-net/spraypaint.png` | 1 | 10 | 9.80 | -4 | -34 |
| png | `css-ig-net/stack.png` | 1 | 10 | 9.81 | -1 | -12 |
| png | `css-ig-net/star.png` | 1 | 10 | 9.80 | 0 | 0 |
| png | `css-ig-net/stariconrt2-fs8.png` | 3 | 10 | 9.81 | -1 | -13 |
| png | `css-ig-net/steeringwheel.png` | 2 | 10 | 9.81 | -2 | -15 |
| png | `css-ig-net/stop.png` | 1 | 10 | 9.80 | -2 | -9 |
| png | `css-ig-net/sub.png` | 1 | 10 | 9.80 | -1 | -4 |
| png | `css-ig-net/submarine.png` | 1 | 10 | 9.81 | -1 | -9 |
| png | `css-ig-net/sucre-d-orge-fs8.png` | 2 | 10 | 9.80 | -20 | -162 |
| png | `css-ig-net/support.png` | 1 | 10 | 9.81 | 0 | -4 |
| png | `css-ig-net/swatches.png` | 1 | 10 | 9.80 | -1 | -7 |
| png | `css-ig-net/sys1-fs8.png` | 2 | 10 | 9.81 | -10 | -84 |
| png | `css-ig-net/tablet.png` | 1 | 10 | 9.80 | -6 | -50 |
| png | `css-ig-net/takeoff.png` | 1 | 10 | 9.80 | -1 | -6 |
| png | `css-ig-net/target.png` | 1 | 10 | 9.82 | -1 | -10 |
| png | `css-ig-net/taxi.png` | 1 | 10 | 9.80 | -2 | -15 |
| png | `css-ig-net/test-conversion-rgb-canalalpha.png` | 6 | 10 | 9.84 | -1655 | -13243 |
| png | `css-ig-net/test-conversion-truecoloralpha-grayscalealpha.png` | 4 | 10 | 9.83 | -20 | -158 |
| png | `css-ig-net/test-conversion-truecoloralpha-paletted.png` | 1 | 10 | 9.81 | -2 | -13 |
| png | `css-ig-net/test-convertir-grayscalealpha-trns.png` | 1 | 10 | 9.81 | -2 | -20 |
| png | `css-ig-net/test-convertir-truecoloralpha-trns.png` | 1 | 10 | 9.81 | -9 | -70 |
| png | `css-ig-net/test-palier-encodage.png` | 1 | 10 | 9.80 | -1 | -7 |
| png | `css-ig-net/test-profondeur-image.png` | 2 | 10 | 9.81 | -2 | -11 |
| png | `css-ig-net/toolbox.png` | 1 | 10 | 9.80 | -3 | -20 |
| png | `css-ig-net/tools.png` | 1 | 10 | 9.80 | -3 | -24 |
| png | `css-ig-net/tractor.png` | 1 | 10 | 9.80 | 0 | -5 |
| png | `css-ig-net/traffic.png` | 1 | 10 | 9.80 | 0 | 0 |
| png | `css-ig-net/train.png` | 1 | 10 | 9.80 | -9 | -67 |
| png | `css-ig-net/travelerbag.png` | 1 | 10 | 9.81 | -1 | -6 |
| png | `css-ig-net/trends.png` | 1 | 10 | 9.80 | 0 | 0 |
| png | `css-ig-net/tripod.png` | 1 | 10 | 9.80 | -1 | -4 |
| png | `css-ig-net/trophy.png` | 1 | 10 | 9.80 | 0 | -6 |
| png | `css-ig-net/truck.png` | 1 | 10 | 9.80 | -5 | -36 |
| png | `css-ig-net/trucsenvrac-fs8.png` | 1 | 10 | 9.81 | -45 | -355 |
| png | `css-ig-net/trucsenvrac2-fs8.png` | 2 | 10 | 9.81 | -17 | -134 |
| png | `css-ig-net/trucsenvrac3-fs8.png` | 2 | 10 | 9.81 | -14 | -105 |
| png | `css-ig-net/tv.png` | 1 | 10 | 9.80 | -6 | -47 |
| png | `css-ig-net/typography.png` | 1 | 10 | 9.80 | -1 | -10 |
| png | `css-ig-net/ufo.png` | 1 | 10 | 9.80 | -1 | -7 |
| png | `css-ig-net/umbrella.png` | 1 | 10 | 9.80 | -2 | -11 |
| png | `css-ig-net/unicycle.png` | 1 | 10 | 9.80 | -1 | -4 |
| png | `css-ig-net/unlocked.png` | 1 | 10 | 9.80 | 0 | 0 |
| png | `css-ig-net/upload.png` | 1 | 10 | 9.80 | 0 | 0 |
| png | `css-ig-net/user-fs8.png` | 2 | 10 | 9.80 | -4 | -39 |
| png | `css-ig-net/user2-fs8.png` | 2 | 10 | 9.81 | -2 | -15 |
| png | `css-ig-net/video.png` | 1 | 10 | 9.80 | -2 | -12 |
| png | `css-ig-net/videocameraclassic.png` | 1 | 10 | 9.80 | -1 | -7 |
| png | `css-ig-net/videocameracompact.png` | 1 | 10 | 9.80 | -1 | -7 |
| png | `css-ig-net/volume.png` | 1 | 10 | 9.80 | -4 | -32 |
| png | `css-ig-net/water.png` | 1 | 10 | 9.80 | -1 | -11 |
| png | `css-ig-net/weather.png` | 1 | 10 | 9.80 | 0 | 0 |
| png | `css-ig-net/windsock.png` | 1 | 10 | 9.81 | -2 | -16 |
| png | `css-ig-net/windy.png` | 1 | 10 | 9.80 | 0 | 0 |
| png | `css-ig-net/world-fs8.png` | 3 | 10 | 9.81 | -4 | -30 |
| png | `css-ig-net/x.png` | 1 | 10 | 9.81 | -2 | -19 |
| png | `css-ig-net/zoomin.png` | 1 | 10 | 9.80 | -1 | -9 |
| png | `css-ig-net/zoomout.png` | 1 | 10 | 9.80 | -2 | -12 |
| png | `imageworsener/256col.png` | 1 | 10 | 9.80 | 0 | 0 |
| png | `imageworsener/25x20.png` | 1 | 10 | 9.80 | -2 | -19 |
| png | `imageworsener/4x4.png` | 1 | 10 | 9.80 | -3 | -24 |
| png | `imageworsener/bt-gray.png` | 1 | 10 | 9.80 | 0 | 0 |
| png | `imageworsener/bt-white.png` | 1 | 10 | 9.80 | 0 | 0 |
| png | `imageworsener/g1.png` | 1 | 10 | 9.59 | 0 | 0 |
| png | `imageworsener/g16.png` | 1 | 10 | 9.80 | 0 | 0 |
| png | `imageworsener/g16a.png` | 1 | 10 | 9.80 | -1 | -5 |
| png | `imageworsener/g16t.png` | 1 | 10 | 9.80 | 0 | -5 |
| png | `imageworsener/g1t.png` | 1 | 10 | 9.80 | 0 | 0 |
| png | `imageworsener/g2.png` | 1 | 10 | 9.80 | 0 | 0 |
| png | `imageworsener/g2t.png` | 1 | 10 | 9.80 | 0 | 0 |
| png | `imageworsener/g4.png` | 1 | 10 | 9.80 | 0 | 0 |
| png | `imageworsener/g4t.png` | 1 | 10 | 9.80 | 0 | 0 |
| png | `imageworsener/g8.png` | 1 | 10 | 9.80 | 0 | 0 |
| png | `imageworsener/g8a.png` | 1 | 10 | 9.80 | 0 | -3 |
| png | `imageworsener/g8d.png` | 1 | 10 | 9.80 | 0 | 0 |
| png | `imageworsener/g8t.png` | 1 | 10 | 9.80 | -1 | -8 |
| png | `imageworsener/p1.png` | 1 | 10 | 9.50 | 0 | 0 |
| png | `imageworsener/p1t.png` | 1 | 10 | 9.80 | 0 | 0 |
| png | `imageworsener/p2.png` | 1 | 10 | 9.80 | 0 | 0 |
| png | `imageworsener/p2t.png` | 1 | 10 | 9.80 | 0 | 0 |
| png | `imageworsener/p4.png` | 1 | 10 | 9.80 | 0 | 0 |
| png | `imageworsener/p4t.png` | 1 | 10 | 9.80 | 0 | 0 |
| png | `imageworsener/p8-sbit.png` | 1 | 10 | 9.80 | -2 | -16 |
| png | `imageworsener/p8.png` | 1 | 10 | 9.80 | -2 | -16 |
| png | `imageworsener/p8t.png` | 1 | 10 | 9.80 | -1 | -9 |
| png | `imageworsener/p8tbg.png` | 1 | 10 | 9.80 | 0 | -2 |
| png | `imageworsener/rgb16.png` | 1 | 10 | 9.80 | 0 | -3 |
| png | `imageworsener/rgb16a.png` | 1 | 10 | 9.80 | 0 | -2 |
| png | `imageworsener/rgb16t.png` | 1 | 10 | 9.80 | 0 | 0 |
| png | `imageworsener/rgb8.png` | 1 | 10 | 9.80 | 0 | 0 |
| png | `imageworsener/rgb8a-sbit.png` | 1 | 10 | 9.80 | 0 | -4 |
| png | `imageworsener/rgb8a.png` | 1 | 10 | 9.80 | 0 | -4 |
| png | `imageworsener/rgb8abg.png` | 1 | 10 | 9.80 | 0 | -4 |
| png | `imageworsener/rgb8t.png` | 1 | 10 | 9.80 | -1 | -1 |
| png | `imageworsener/rgb8x1.png` | 1 | 10 | 9.80 | 0 | 0 |
| png | `imageworsener/rgb8x2.png` | 1 | 10 | 9.80 | 0 | -2 |
| png | `imageworsener/rings1.png` | 1 | 10 | 9.81 | -278 | -2222 |
| png | `large/16million-pschmidt.png` | 32 | 34 | 33.91 | 0 | -3 |
| png | `large/Partnership_Card___John_Lewis_Finance.png` | 139 | 141 | 139.95 | -301205 | -2409640 |
| png | `large/T_Brick_house_islands.png` | 1 | 3 | 2.96 | -3 | -26 |
| png | `large/T_Home.png` | 1 | 10 | 9.81 | 0 | -2 |
| png | `large/indycar.png` | 39 | 41 | 40.45 | -7 | -58 |
| png | `large/nerd.png` | 635 | 180 | 176.51 | -9512 | -76100 |
| png | `large/uob-original.png` | 2 | 4 | 3.94 | -1 | -3 |
| png | `medium/02-ct-c4-c0.png` | 2 | 4 | 3.93 | 0 | 0 |
| png | `medium/05-ct-c0-c3.png` | 3 | 5 | 4.91 | 0 | 0 |
| png | `medium/09-ct-c6-c4.png` | 2 | 4 | 3.93 | 0 | 0 |
| png | `medium/1.5.02.PNG` | 3 | 5 | 4.93 | -40 | -322 |
| png | `medium/12-c3-trans-first.png` | 2 | 4 | 3.93 | 0 | -1 |
| png | `medium/15-c3-color-filter-trans-first.png` | 2 | 4 | 3.93 | 0 | 0 |
| png | `medium/1_original.png` | 1 | 3 | 2.95 | -40 | -318 |
| png | `medium/20-dt-rgb-mod-free.png` | 3 | 5 | 4.91 | 0 | 0 |
| png | `medium/21-dt-rgb-mod-average.png` | 1 | 3 | 2.95 | 0 | 0 |
| png | `medium/280543107-79bbbda4-b565-493b-9088-d7647e86a1ca.png` | 1 | 3 | 2.95 | -1 | -12 |
| png | `medium/284.png` | 8 | 10 | 9.81 | -2 | -20 |
| png | `medium/2_original.png` | 3 | 5 | 4.91 | -310 | -2482 |
| png | `medium/319057651-61e74d7e-85cc-478d-8883-0d7a2962e685.png` | 27 | 29 | 28.47 | -115 | -921 |
| png | `medium/3_original.png` | 1 | 3 | 2.95 | -1076 | -8606 |
| png | `medium/3x5 font alternatives.png` | 1 | 3 | 2.97 | -273 | -2182 |
| png | `medium/4.1.01.PNG` | 3 | 5 | 4.94 | -858 | -6865 |
| png | `medium/4.2.03.PNG` | 15 | 17 | 17.15 | -576 | -4607 |
| png | `medium/4.2.07.PNG` | 8 | 10 | 9.82 | -783 | -6264 |
| png | `medium/4_original.png` | 3 | 5 | 4.92 | -77 | -615 |
| png | `medium/5.1.09.PNG` | 2 | 4 | 3.93 | -851 | -6814 |
| png | `medium/5.1.11.PNG` | 1 | 3 | 2.97 | -1477 | -11811 |
| png | `medium/5.1.13.PNG` | 1 | 3 | 2.95 | -99 | -792 |
| png | `medium/5_original.png` | 1 | 3 | 2.94 | -5 | -37 |
| png | `medium/81817-2.png` | 1 | 3 | 2.95 | -37 | -296 |
| png | `medium/AlphaBall.png` | 1 | 3 | 2.95 | -95 | -755 |
| png | `medium/AlphaEdge.png` | 1 | 3 | 2.96 | -765 | -6127 |
| png | `medium/BNDT_on_X____stack__https___t_co_3VRfGVC7bX____X.png` | 134 | 136 | 133.34 | -769 | -6149 |
| png | `medium/Backy_ru.png` | 1 | 3 | 2.95 | 0 | -6 |
| png | `medium/Bag.png` | 4 | 6 | 5.94 | 0 | 0 |
| png | `medium/Ball.png` | 9 | 11 | 10.80 | 0 | 0 |
| png | `medium/Bullfinch.png` | 6 | 8 | 7.89 | 0 | -1 |
| png | `medium/ColorTestCard.png` | 3 | 5 | 4.91 | -480 | -3836 |
| png | `medium/Column4.png` | 3 | 5 | 4.91 | -1 | -5 |
| png | `medium/Cookies.png` | 5 | 7 | 6.87 | 0 | 0 |
| png | `medium/Craig_Smith.png` | 35 | 37 | 36.33 | -98 | -787 |
| png | `medium/Death.png` | 4 | 6 | 5.89 | -1 | -7 |
| png | `medium/Decorations.png` | 4 | 6 | 6.26 | 0 | 0 |
| png | `medium/End.png` | 5 | 7 | 7.07 | -1 | -4 |
| png | `medium/F2EXEs_WQAAfE38.png` | 1 | 3 | 3.04 | -20 | -156 |
| png | `medium/FsqwhPuaIAIlojU.png` | 71 | 73 | 74.36 | -161 | -1293 |
| png | `medium/GK1fa8hXYAAFE1C.png` | 1 | 3 | 3.04 | -28 | -221 |
| png | `medium/Gingerman.png` | 5 | 7 | 7.21 | 0 | 0 |
| png | `medium/Grading-2018-1.png` | 1 | 3 | 3.08 | -436 | -3482 |
| png | `medium/Home___X.png` | 15 | 17 | 17.18 | -5 | -40 |
| png | `medium/IMG_8555.PNG` | 66 | 68 | 69.29 | -4417 | -35344 |
| png | `medium/LevelLoading.png` | 9 | 11 | 11.12 | -154 | -1233 |
| png | `medium/Matrix.png` | 18 | 20 | 20.21 | -157 | -1263 |
| png | `medium/Mittens.png` | 5 | 7 | 7.34 | 0 | 0 |
| png | `medium/Nutcracker.png` | 4 | 6 | 6.17 | -1 | -6 |
| png | `medium/OwlAlpha-0.5.png` | 6 | 8 | 8.51 | -330 | -2636 |
| png | `medium/Pay_a_customs_charge___Royal_Mail_Ltd.png` | 1 | 3 | 3.03 | -19 | -147 |
| png | `medium/RAW1.png` | 4 | 6 | 6.11 | 0 | 0 |
| png | `medium/Sock.png` | 7 | 9 | 9.25 | 0 | 0 |
| png | `medium/TruePNG calling-cleric.png` | 36 | 38 | 38.56 | -1244 | -9949 |
| png | `medium/ask_01.png` | 1 | 3 | 3.05 | -9 | -75 |
| png | `medium/ask_02.png` | 2 | 4 | 4.05 | -576 | -4611 |
| png | `medium/ask_03.png` | 2 | 4 | 4.14 | -1 | -6 |
| png | `medium/ask_04.png` | 1 | 3 | 3.08 | 0 | -2 |
| png | `medium/automator.png` | 2 | 4 | 4.04 | -41 | -330 |
| png | `medium/black817-480x360-3.5.png` | 11 | 13 | 13.37 | -30 | -235 |
| png | `medium/blocks.png` | 1 | 3 | 3.04 | -125 | -998 |
| png | `medium/butterfly.png` | 6 | 8 | 8.17 | -67 | -534 |
| png | `medium/buttons.png` | 1 | 3 | 3.01 | 0 | -1 |
| png | `medium/chess.png` | 1 | 3 | 3.39 | -1493 | -11939 |
| png | `medium/classr.png` | 1 | 3 | 3.02 | -35 | -280 |
| png | `medium/clegg.PNG` | 10 | 12 | 13.04 | -306 | -2452 |
| png | `medium/coconut.png` | 1 | 3 | 3.03 | -2 | -15 |
| png | `medium/cookie.png` | 1 | 3 | 3.01 | -3 | -19 |
| png | `medium/countdown.png` | 1 | 3 | 3.01 | -2 | -16 |
| png | `medium/dirty-tr-smpl1.png` | 8 | 10 | 10.19 | -41 | -328 |
| png | `medium/dossier-green-normal-fs8.png` | 2 | 4 | 4.03 | -2 | -16 |
| png | `medium/download_webp__260×280_.png` | 36 | 38 | 38.28 | -475 | -3798 |
| png | `medium/facebook.png` | 2 | 4 | 4.09 | -59 | -478 |
| png | `medium/fill_patterns_020.png` | 1 | 3 | 3.04 | 0 | -3 |
| png | `medium/filmreel.png` | 1 | 3 | 3.01 | 0 | 0 |
| png | `medium/floor pattern.png` | 182 | 180 | 181.34 | -403 | -3225 |
| png | `medium/france.PNG` | 1 | 3 | 3.03 | -62 | -498 |
| png | `medium/frymire.PNG` | 31 | 33 | 33.37 | -512 | -4094 |
| png | `medium/global.png` | 1 | 3 | 3.01 | 0 | 0 |
| png | `medium/globe-scene-fish-bowl-pngcrush.png` | 15 | 17 | 17.46 | -29 | -235 |
| png | `medium/hXlpvqi4.png-medium.png` | 2 | 4 | 4.20 | -99 | -796 |
| png | `medium/heart2-fs8.png` | 2 | 4 | 4.02 | -1 | -9 |
| png | `medium/icone_windows-fs8.png` | 2 | 4 | 4.02 | -1 | -4 |
| png | `medium/icons32-very original.png` | 1 | 3 | 3.09 | 0 | 0 |
| png | `medium/iconstrp.png` | 2 | 4 | 4.01 | -6 | -41 |
| png | `medium/images.png` | 1 | 3 | 3.01 | -11 | -88 |
| png | `medium/img11-greyscalepalette.png` | 1 | 3 | 3.01 | 0 | -3 |
| png | `medium/imgcomp-440x330.png` | 2 | 4 | 4.05 | -148 | -1181 |
| png | `medium/industry.png` | 2 | 4 | 4.04 | -5 | -40 |
| png | `medium/keyboard.png` | 1 | 3 | 3.01 | -2 | -13 |
| png | `medium/lens.png` | 1 | 3 | 3.01 | -2 | -15 |
| png | `medium/library.PNG` | 4 | 6 | 6.25 | -336 | -2693 |
| png | `medium/logo.png` | 5 | 7 | 7.17 | -5 | -46 |
| png | `medium/loupe-fs8.png` | 2 | 4 | 4.03 | 0 | 0 |
| png | `medium/menu.png` | 2 | 4 | 4.02 | -1 | -8 |
| png | `medium/micropython const.png` | 20 | 22 | 22.42 | -104 | -836 |
| png | `medium/monarch.PNG` | 14 | 16 | 17.58 | -441 | -3528 |
| png | `medium/my-computer-off-fs8.png` | 2 | 4 | 4.03 | 0 | -1 |
| png | `medium/nascar.png` | 3 | 10 | 9.81 | -6 | -44 |
| png | `medium/numbers.512.png` | 11 | 13 | 13.23 | -38 | -308 |
| png | `medium/objects.png` | 1 | 3 | 3.01 | -3 | -26 |
| png | `medium/parachute.png` | 1 | 10 | 9.81 | 0 | 0 |
| png | `medium/pengbrew_160x160.png` | 1 | 3 | 3.01 | -6 | -44 |
| png | `medium/phenix.png` | 2 | 4 | 4.01 | -1 | -11 |
| png | `medium/phoenix.png` | 3 | 5 | 5.08 | -74 | -591 |
| png | `medium/pngbook-cover.png` | 2 | 4 | 4.02 | -155 | -1242 |
| png | `medium/pokemon pixel art.png` | 1 | 3 | 3.03 | -54 | -435 |
| png | `medium/rgb.png` | 1 | 3 | 3.01 | 0 | 0 |
| png | `medium/road.png` | 1 | 3 | 3.01 | 0 | 0 |
| png | `medium/rtSn4NgH.png-medium.png` | 2 | 4 | 4.12 | 0 | 0 |
| png | `medium/sample.png` | 2 | 4 | 4.03 | -2 | -14 |
| png | `medium/serrano.PNG` | 7 | 9 | 9.14 | -315 | -2514 |
| png | `medium/shipwheel.png` | 1 | 3 | 3.01 | 0 | -5 |
| png | `medium/sims-fs8.png` | 3 | 5 | 5.03 | 0 | -4 |
| png | `medium/slope.PNG` | 1 | 3 | 3.03 | -154 | -1230 |
| png | `medium/ss1.png` | 4 | 6 | 5.95 | -9712 | -77699 |
| png | `medium/te_syntax.png` | 2 | 4 | 4.02 | -3 | -24 |
| png | `medium/test-conversion-truecoloralpha-grayscalealpha.png` | 5 | 7 | 7.12 | 0 | 0 |
| png | `medium/test-convertir-truecoloralpha-trns.png` | 2 | 4 | 4.02 | -1 | -4 |
| png | `medium/texmos3.s512.PNG` | 1 | 3 | 3.02 | -14 | -111 |
| png | `medium/text_1.png` | 4 | 6 | 6.11 | -11 | -83 |
| png | `medium/text_2.png` | 7 | 9 | 9.19 | -3 | -26 |
| png | `medium/text_3.png` | 2 | 4 | 4.09 | -422 | -3374 |
| png | `medium/tiger.png` | 2 | 4 | 4.01 | -1 | -9 |
| png | `medium/trucsenvrac2-fs8.png` | 2 | 4 | 4.02 | -13 | -102 |
| png | `medium/wolf.png` | 1 | 3 | 3.04 | 0 | 0 |
| png | `medium/world-fs8.png` | 2 | 4 | 4.05 | -1 | -10 |
| png | `medium/y2rc2_large.png` | 54 | 56 | 54.89 | -4856 | -38850 |
| png | `medium/yahoo.png` | 1 | 3 | 3.01 | -59 | -478 |
| png | `oxipng/badsrgb.png` | 1 | 3 | 3.01 | -6 | -47 |
| png | `oxipng/c2pa-signed.png` | 1 | 3 | 3.00 | 0 | 0 |
| png | `oxipng/filter_0_for_grayscale_16.png` | 6 | 8 | 8.48 | -130 | -1047 |
| png | `oxipng/filter_0_for_grayscale_8.png` | 1 | 3 | 3.04 | -123 | -989 |
| png | `oxipng/filter_0_for_grayscale_alpha_16.png` | 22 | 24 | 24.49 | -57 | -455 |
| png | `oxipng/filter_0_for_grayscale_alpha_8.png` | 4 | 6 | 6.07 | 0 | -2 |
| png | `oxipng/filter_0_for_palette_1.png` | 1 | 3 | 3.01 | 0 | 0 |
| png | `oxipng/filter_0_for_palette_2.png` | 1 | 3 | 3.01 | 0 | 0 |
| png | `oxipng/filter_0_for_palette_4.png` | 1 | 3 | 3.01 | 0 | 0 |
| png | `oxipng/filter_0_for_rgb_16.png` | 3 | 5 | 5.31 | -228 | -1827 |
| png | `oxipng/filter_0_for_rgb_8.png` | 16 | 18 | 18.21 | -75 | -593 |
| png | `oxipng/filter_0_for_rgba_16.png` | 12 | 14 | 13.76 | -51 | -407 |
| png | `oxipng/filter_0_for_rgba_8.png` | 4 | 6 | 5.89 | -39 | -313 |
| png | `oxipng/fully_optimized.png` | 1 | 10 | 0.63 | 0 | 0 |
| png | `oxipng/grayscale_16_should_be_grayscale_1.png` | 1 | 3 | 3.02 | -1 | -7 |
| png | `oxipng/grayscale_16_should_be_grayscale_8.png` | 1 | 3 | 3.26 | -105 | -836 |
| png | `oxipng/grayscale_2_should_be_grayscale_1.png` | 1 | 3 | 3.01 | 0 | -4 |
| png | `oxipng/grayscale_4_should_be_grayscale_1.png` | 1 | 3 | 3.01 | -1 | -5 |
| png | `oxipng/grayscale_4_should_be_grayscale_2.png` | 1 | 3 | 3.01 | -19 | -150 |
| png | `oxipng/grayscale_8_should_be_grayscale_1.png` | 1 | 3 | 3.01 | 0 | 0 |
| png | `oxipng/grayscale_8_should_be_grayscale_2.png` | 1 | 3 | 3.01 | -60 | -479 |
| png | `oxipng/grayscale_8_should_be_grayscale_4.png` | 1 | 3 | 3.01 | -5 | -42 |
| png | `oxipng/grayscale_8_should_be_palette_1.png` | 1 | 3 | 3.01 | 0 | 0 |
| png | `oxipng/grayscale_8_should_be_palette_2.png` | 1 | 3 | 3.02 | 0 | -1 |
| png | `oxipng/grayscale_8_should_be_palette_4.png` | 1 | 3 | 3.02 | -37 | -289 |
| png | `oxipng/grayscale_8_should_be_palette_8.png` | 8 | 10 | 9.81 | -68 | -542 |
| png | `oxipng/grayscale_alpha_16_reduce_alpha.png` | 1 | 3 | 3.04 | -89 | -717 |
| png | `oxipng/grayscale_alpha_16_should_be_grayscale_16.png` | 2 | 4 | 4.13 | -96 | -764 |
| png | `oxipng/grayscale_alpha_16_should_be_grayscale_8.png` | 2 | 4 | 4.15 | -127 | -1011 |
| png | `oxipng/grayscale_alpha_16_should_be_grayscale_alpha_16.png` | 19 | 21 | 21.94 | -376 | -3003 |
| png | `oxipng/grayscale_alpha_16_should_be_grayscale_alpha_8.png` | 3 | 5 | 5.49 | -293 | -2345 |
| png | `oxipng/grayscale_alpha_16_should_be_grayscale_trns_16.png` | 10 | 12 | 12.36 | -2 | -14 |
| png | `oxipng/grayscale_alpha_8_reduce_alpha.png` | 2 | 4 | 4.04 | -1 | -8 |
| png | `oxipng/grayscale_alpha_8_should_be_grayscale_8.png` | 5 | 7 | 7.22 | -280 | -2240 |
| png | `oxipng/grayscale_alpha_8_should_be_grayscale_alpha_8.png` | 3 | 5 | 5.07 | -55 | -438 |
| png | `oxipng/grayscale_alpha_8_should_be_grayscale_trns_1.png` | 1 | 3 | 3.02 | -31 | -249 |
| png | `oxipng/grayscale_alpha_8_should_be_grayscale_trns_8.png` | 139 | 141 | 141.09 | -605 | -4837 |
| png | `oxipng/grayscale_alpha_8_should_be_palette_8.png` | 5 | 7 | 7.08 | -6 | -53 |
| png | `oxipng/grayscale_trns_8_should_be_grayscale_1.png` | 1 | 3 | 3.02 | 0 | 0 |
| png | `oxipng/interlaced_0_to_1_other_filter_mode.png` | 2 | 4 | 4.21 | -1 | -9 |
| png | `oxipng/interlaced_grayscale_16_should_be_grayscale_16.png` | 7 | 9 | 9.53 | -13 | -108 |
| png | `oxipng/interlaced_grayscale_16_should_be_grayscale_8.png` | 2 | 4 | 4.13 | -1104 | -8830 |
| png | `oxipng/interlaced_grayscale_8_should_be_grayscale_8.png` | 3 | 5 | 5.09 | -618 | -4944 |
| png | `oxipng/interlaced_grayscale_alpha_16_should_be_grayscale_16.png` | 2 | 4 | 4.13 | -371 | -2971 |
| png | `oxipng/interlaced_grayscale_alpha_16_should_be_grayscale_8.png` | 1 | 3 | 3.17 | -378 | -3026 |
| png | `oxipng/interlaced_grayscale_alpha_16_should_be_grayscale_alpha_16.png` | 54 | 56 | 56.80 | -1049 | -8393 |
| png | `oxipng/interlaced_grayscale_alpha_16_should_be_grayscale_alpha_8.png` | 2 | 4 | 4.14 | -534 | -4274 |
| png | `oxipng/interlaced_grayscale_alpha_8_should_be_grayscale_8.png` | 2 | 4 | 4.20 | -363 | -2905 |
| png | `oxipng/interlaced_grayscale_alpha_8_should_be_grayscale_alpha_8.png` | 10 | 12 | 12.16 | -144 | -1154 |
| png | `oxipng/interlaced_odd_width.png` | 35 | 37 | 36.36 | -29346 | -234773 |
| png | `oxipng/interlaced_palette_1_should_be_palette_1.png` | 1 | 3 | 3.01 | -1 | -1 |
| png | `oxipng/interlaced_palette_2_should_be_palette_1.png` | 1 | 3 | 3.01 | 0 | 0 |
| png | `oxipng/interlaced_palette_2_should_be_palette_2.png` | 1 | 3 | 3.01 | -1 | -6 |
| png | `oxipng/interlaced_palette_4_should_be_palette_1.png` | 1 | 3 | 3.01 | 0 | -1 |
| png | `oxipng/interlaced_palette_4_should_be_palette_2.png` | 1 | 3 | 3.01 | -1 | -3 |
| png | `oxipng/interlaced_palette_4_should_be_palette_4.png` | 1 | 3 | 3.01 | -2 | -17 |
| png | `oxipng/interlaced_palette_8_should_be_grayscale_8.png` | 3 | 5 | 5.20 | -39 | -315 |
| png | `oxipng/interlaced_palette_8_should_be_palette_1.png` | 1 | 3 | 3.01 | -1 | -14 |
| png | `oxipng/interlaced_palette_8_should_be_palette_2.png` | 1 | 3 | 3.01 | 0 | -1 |
| png | `oxipng/interlaced_palette_8_should_be_palette_4.png` | 1 | 3 | 3.01 | -3 | -27 |
| png | `oxipng/interlaced_palette_8_should_be_palette_8.png` | 1 | 3 | 3.02 | -214 | -1715 |
| png | `oxipng/interlaced_rgb_16_should_be_grayscale_16.png` | 2 | 4 | 4.14 | -362 | -2897 |
| png | `oxipng/interlaced_rgb_16_should_be_grayscale_8.png` | 2 | 4 | 4.14 | -417 | -3340 |
| png | `oxipng/interlaced_rgb_16_should_be_palette_1.png` | 1 | 3 | 3.06 | -22 | -170 |
| png | `oxipng/interlaced_rgb_16_should_be_palette_2.png` | 1 | 3 | 3.06 | -44 | -352 |
| png | `oxipng/interlaced_rgb_16_should_be_palette_4.png` | 1 | 3 | 3.04 | -123 | -988 |
| png | `oxipng/interlaced_rgb_16_should_be_palette_8.png` | 1 | 3 | 3.05 | -343 | -2745 |
| png | `oxipng/interlaced_rgb_16_should_be_rgb_16.png` | 4 | 6 | 6.18 | -350 | -2800 |
| png | `oxipng/interlaced_rgb_16_should_be_rgb_8.png` | 2 | 4 | 4.20 | -375 | -2998 |
| png | `oxipng/interlaced_rgb_8_should_be_grayscale_8.png` | 2 | 4 | 4.16 | -402 | -3217 |
| png | `oxipng/interlaced_rgb_8_should_be_palette_1.png` | 1 | 3 | 3.03 | -23 | -188 |
| png | `oxipng/interlaced_rgb_8_should_be_palette_2.png` | 1 | 3 | 3.03 | -34 | -270 |
| png | `oxipng/interlaced_rgb_8_should_be_palette_4.png` | 1 | 3 | 3.04 | -101 | -803 |
| png | `oxipng/interlaced_rgb_8_should_be_palette_8.png` | 1 | 3 | 3.03 | -229 | -1830 |
| png | `oxipng/interlaced_rgb_8_should_be_rgb_8.png` | 7 | 9 | 9.20 | -2 | -20 |
| png | `oxipng/interlaced_rgba_16_should_be_grayscale_16.png` | 13 | 15 | 15.41 | -1022 | -8174 |
| png | `oxipng/interlaced_rgba_16_should_be_grayscale_8.png` | 2 | 4 | 4.12 | -559 | -4474 |
| png | `oxipng/interlaced_rgba_16_should_be_grayscale_alpha_16.png` | 25 | 27 | 27.60 | -7 | -57 |
| png | `oxipng/interlaced_rgba_16_should_be_grayscale_alpha_8.png` | 2 | 4 | 4.15 | -338 | -2704 |
| png | `oxipng/interlaced_rgba_16_should_be_palette_1.png` | 1 | 3 | 3.07 | -40 | -316 |
| png | `oxipng/interlaced_rgba_16_should_be_palette_2.png` | 1 | 3 | 3.08 | -24 | -195 |
| png | `oxipng/interlaced_rgba_16_should_be_palette_4.png` | 1 | 3 | 3.05 | -20 | -155 |
| png | `oxipng/interlaced_rgba_16_should_be_palette_8.png` | 1 | 3 | 3.06 | -368 | -2950 |
| png | `oxipng/interlaced_rgba_16_should_be_rgb_16.png` | 5 | 7 | 7.40 | -302 | -2418 |
| png | `oxipng/interlaced_rgba_16_should_be_rgb_8.png` | 3 | 5 | 5.21 | -320 | -2563 |
| png | `oxipng/interlaced_rgba_16_should_be_rgba_16.png` | 18 | 20 | 20.29 | -336 | -2692 |
| png | `oxipng/interlaced_rgba_16_should_be_rgba_8.png` | 1 | 3 | 3.10 | -457 | -3657 |
| png | `oxipng/interlaced_rgba_8_should_be_grayscale_8.png` | 2 | 4 | 4.17 | -370 | -2963 |
| png | `oxipng/interlaced_rgba_8_should_be_grayscale_alpha_8.png` | 4 | 6 | 6.18 | -234 | -1873 |
| png | `oxipng/interlaced_rgba_8_should_be_palette_1.png` | 1 | 3 | 3.04 | -36 | -283 |
| png | `oxipng/interlaced_rgba_8_should_be_palette_2.png` | 1 | 3 | 3.04 | -33 | -260 |
| png | `oxipng/interlaced_rgba_8_should_be_palette_4.png` | 1 | 3 | 3.04 | -196 | -1568 |
| png | `oxipng/interlaced_rgba_8_should_be_palette_8.png` | 1 | 3 | 3.05 | -231 | -1852 |
| png | `oxipng/interlaced_rgba_8_should_be_rgb_8.png` | 28 | 30 | 30.30 | -181 | -1452 |
| png | `oxipng/interlaced_rgba_8_should_be_rgba_8.png` | 4 | 6 | 6.06 | -309 | -2472 |
| png | `oxipng/interlaced_small_files.png` | 1 | 3 | 3.00 | 0 | 0 |
| png | `oxipng/interlaced_vertical_filters.png` | 37 | 39 | 39.23 | -162 | -1296 |
| png | `oxipng/interlacing_0_to_1_small_files.png` | 1 | 3 | 3.00 | 0 | 0 |
| png | `oxipng/issue-140.png` | 1 | 3 | 3.00 | 0 | 0 |
| png | `oxipng/issue-171.png` | 1 | 3 | 2.77 | 0 | 0 |
| png | `oxipng/issue-175.png` | 1 | 3 | 0.44 | 0 | 0 |
| png | `oxipng/issue-42.png` | 1 | 3 | 0.23 | 0 | 0 |
| png | `oxipng/issue-56.png` | 1 | 3 | 3.01 | 0 | 0 |
| png | `oxipng/issue-58.png` | 1 | 3 | 3.01 | 0 | 0 |
| png | `oxipng/issue-59.png` | 2 | 4 | 4.05 | -1 | -3 |
| png | `oxipng/issue-60.png` | 1 | 3 | 1.72 | 0 | 0 |
| png | `oxipng/issue-89.png` | 1 | 3 | 3.00 | 0 | 0 |
| png | `oxipng/json.png` | 2 | 4 | 4.09 | -3 | -28 |
| png | `oxipng/palette_2_should_be_grayscale_alpha_8.png` | 1 | 3 | 3.00 | 0 | 0 |
| png | `oxipng/palette_2_should_be_palette_1.png` | 1 | 3 | 3.01 | 0 | 0 |
| png | `oxipng/palette_4_should_be_palette_1.png` | 1 | 3 | 3.01 | -1 | -8 |
| png | `oxipng/palette_4_should_be_palette_2.png` | 1 | 3 | 3.01 | 0 | -1 |
| png | `oxipng/palette_8_should_be_grayscale_8.png` | 2 | 4 | 4.05 | -23 | -182 |
| png | `oxipng/palette_8_should_be_palette_1.png` | 1 | 3 | 3.01 | 0 | 0 |
| png | `oxipng/palette_8_should_be_palette_2.png` | 1 | 3 | 3.01 | 0 | 0 |
| png | `oxipng/palette_8_should_be_palette_4.png` | 1 | 3 | 3.01 | 0 | 0 |
| png | `oxipng/palette_8_should_be_palette_8.png` | 3 | 5 | 5.04 | -61 | -489 |
| png | `oxipng/palette_8_should_be_rgb.png` | 1 | 3 | 3.00 | 0 | -1 |
| png | `oxipng/palette_8_should_be_rgba.png` | 1 | 3 | 3.01 | -1 | -13 |
| png | `oxipng/palette_should_be_reduced_with_both.png` | 1 | 3 | 3.02 | -204 | -1637 |
| png | `oxipng/palette_should_be_reduced_with_missing.png` | 1 | 3 | 3.00 | 0 | 0 |
| png | `oxipng/palette_should_be_reduced_with_unused.png` | 1 | 3 | 3.02 | -205 | -1640 |
| png | `oxipng/profile_adobe_rgb_disallow_gray.png` | 4 | 6 | 6.11 | -7 | -51 |
| png | `oxipng/profile_gray_disallow_color.png` | 4 | 6 | 6.05 | -1 | -2 |
| png | `oxipng/profile_srgb_allow_gray.png` | 4 | 6 | 6.12 | -8 | -61 |
| png | `oxipng/rgb_16_should_be_grayscale_8.png` | 1 | 3 | 3.20 | 0 | -2 |
| png | `oxipng/rgb_16_should_be_palette_1.png` | 1 | 3 | 3.07 | 0 | 0 |
| png | `oxipng/rgb_16_should_be_palette_2.png` | 1 | 3 | 3.05 | 0 | 0 |
| png | `oxipng/rgb_16_should_be_palette_4.png` | 1 | 3 | 3.07 | 0 | -1 |
| png | `oxipng/rgb_16_should_be_palette_8.png` | 1 | 3 | 3.05 | -37 | -297 |
| png | `oxipng/rgb_16_should_be_rgb_8.png` | 3 | 5 | 5.30 | -189 | -1514 |
| png | `oxipng/rgb_8_should_be_grayscale_8.png` | 3 | 5 | 5.11 | -36 | -282 |
| png | `oxipng/rgb_8_should_be_palette_1.png` | 1 | 3 | 3.04 | -1 | -4 |
| png | `oxipng/rgb_8_should_be_palette_2.png` | 1 | 3 | 3.03 | -3 | -22 |
| png | `oxipng/rgb_8_should_be_palette_4.png` | 1 | 3 | 3.04 | 0 | 0 |
| png | `oxipng/rgb_8_should_be_palette_8.png` | 1 | 3 | 3.03 | -4 | -28 |
| png | `oxipng/rgb_trns_8_should_be_palette_8.png` | 2 | 4 | 4.07 | -1 | -10 |
| png | `oxipng/rgba_16_reduce_alpha.png` | 2 | 4 | 4.12 | -67 | -536 |
| png | `oxipng/rgba_16_should_be_grayscale_16.png` | 39 | 41 | 41.78 | -115 | -917 |
| png | `oxipng/rgba_16_should_be_grayscale_8.png` | 2 | 4 | 4.22 | -278 | -2223 |
| png | `oxipng/rgba_16_should_be_grayscale_alpha_16.png` | 42 | 44 | 46.05 | -83 | -668 |
| png | `oxipng/rgba_16_should_be_grayscale_alpha_8.png` | 5 | 7 | 7.29 | -145 | -1166 |
| png | `oxipng/rgba_16_should_be_palette_1.png` | 1 | 3 | 3.09 | -4 | -25 |
| png | `oxipng/rgba_16_should_be_palette_2.png` | 1 | 3 | 3.08 | -3 | -25 |
| png | `oxipng/rgba_16_should_be_palette_4.png` | 1 | 3 | 3.05 | 0 | -1 |
| png | `oxipng/rgba_16_should_be_palette_8.png` | 1 | 3 | 3.07 | -35 | -280 |
| png | `oxipng/rgba_16_should_be_rgb_16.png` | 3 | 5 | 5.30 | -100 | -799 |
| png | `oxipng/rgba_16_should_be_rgb_8.png` | 3 | 5 | 5.31 | -97 | -777 |
| png | `oxipng/rgba_16_should_be_rgb_trns_16.png` | 7 | 9 | 9.21 | -11 | -85 |
| png | `oxipng/rgba_16_should_be_rgba_16.png` | 12 | 14 | 14.19 | -683 | -5458 |
| png | `oxipng/rgba_16_should_be_rgba_8.png` | 1 | 3 | 3.10 | -232 | -1859 |
| png | `oxipng/rgba_8_reduce_alpha.png` | 3 | 5 | 5.15 | -6 | -52 |
| png | `oxipng/rgba_8_should_be_grayscale_8.png` | 3 | 5 | 5.11 | -46 | -366 |
| png | `oxipng/rgba_8_should_be_grayscale_alpha_8.png` | 5 | 7 | 7.15 | -161 | -1289 |
| png | `oxipng/rgba_8_should_be_palette_1.png` | 1 | 3 | 3.04 | 0 | -6 |
| png | `oxipng/rgba_8_should_be_palette_2.png` | 1 | 3 | 3.05 | 0 | 0 |
| png | `oxipng/rgba_8_should_be_palette_4.png` | 1 | 3 | 3.05 | 0 | 0 |
| png | `oxipng/rgba_8_should_be_palette_8.png` | 1 | 3 | 3.04 | -8 | -60 |
| png | `oxipng/rgba_8_should_be_rgb_8.png` | 32 | 34 | 34.20 | -203 | -1626 |
| png | `oxipng/rgba_8_should_be_rgb_trns_8.png` | 51 | 53 | 53.15 | -7 | -58 |
| png | `oxipng/rgba_8_should_be_rgba_8.png` | 1 | 3 | 3.04 | -59 | -475 |
| png | `oxipng/strip_chunks_all.png` | 16 | 18 | 18.21 | -77 | -607 |
| png | `oxipng/verbose_mode.png` | 15 | 17 | 17.21 | -75 | -593 |
| png | `pkmn-bw-hard/000-Logo-5.png` | 1 | 3 | 3.00 | -1 | -7 |
| png | `pkmn-bw-hard/000-Logo-6.png` | 1 | 3 | 3.00 | -1 | -9 |
| png | `pkmn-bw-hard/000-Logo.png` | 1 | 3 | 3.00 | 0 | 0 |
| png | `pkmn-bw-hard/001-Bulbasaur-0.png` | 1 | 3 | 3.00 | 0 | 0 |
| png | `pkmn-bw-hard/001-Bulbasaur-2.png` | 1 | 3 | 3.00 | 0 | 0 |
| png | `pkmn-bw-hard/001-Bulbasaur.png` | 1 | 3 | 3.00 | -1 | -7 |
| png | `pkmn-bw-hard/003-Venusaur-1.png` | 1 | 3 | 3.00 | 0 | 0 |
| png | `pkmn-bw-hard/005-Charmeleon-1.png` | 1 | 3 | 3.00 | 0 | 0 |
| png | `pkmn-bw-hard/005-Charmeleon-2.png` | 1 | 3 | 3.00 | 0 | 0 |
| png | `pkmn-bw-hard/007-Squirtle-0.png` | 1 | 3 | 3.01 | 0 | 0 |
| png | `pkmn-bw-hard/007-Squirtle.png` | 1 | 3 | 3.00 | 0 | 0 |
| png | `pkmn-bw-hard/008-Wartortle-1.png` | 1 | 3 | 3.00 | -1 | -6 |
| png | `pkmn-bw-hard/008-Wartortle-2.png` | 1 | 3 | 3.00 | 0 | 0 |
| png | `pkmn-bw-hard/009-Blastoise-2.png` | 1 | 3 | 3.00 | 0 | 0 |
| png | `pkmn-bw-hard/010-Caterpie.png` | 1 | 3 | 3.00 | 0 | 0 |
| png | `pkmn-bw-hard/014-Kakuna-0.png` | 1 | 3 | 3.00 | 0 | 0 |
| png | `pkmn-bw-hard/014-Kakuna-1.png` | 1 | 3 | 3.00 | -2 | -17 |
| png | `pkmn-bw-hard/015-Beedrill-1.png` | 1 | 3 | 3.00 | 0 | 0 |
| png | `pkmn-bw-hard/015-Beedrill.png` | 1 | 3 | 3.00 | 0 | 0 |
| png | `pkmn-bw-hard/016-Pidgey.png` | 1 | 3 | 3.00 | 0 | 0 |
| png | `pkmn-bw-hard/019-Rattata-2.png` | 1 | 3 | 3.00 | 0 | 0 |
| png | `pkmn-bw-hard/020-Raticate-2.png` | 1 | 3 | 3.00 | 0 | -2 |
| png | `pkmn-bw-hard/020-Raticate.png` | 1 | 3 | 3.00 | 0 | 0 |
| png | `pkmn-bw-hard/023-Ekans-2.png` | 1 | 3 | 3.00 | -2 | -13 |
| png | `pkmn-bw-hard/024-Arbok-0.png` | 1 | 3 | 3.00 | 0 | -2 |
| png | `pkmn-bw-hard/024-Arbok.png` | 1 | 3 | 3.00 | -1 | -5 |
| png | `pkmn-bw-hard/026-Raichu-0.png` | 1 | 3 | 3.00 | 0 | 0 |
| png | `pkmn-bw-hard/026-Raichu.png` | 1 | 10 | 9.81 | 0 | 0 |
| png | `pkmn-bw-hard/027-Sandshrew-2.png` | 1 | 3 | 3.00 | 0 | -2 |
| png | `pkmn-bw-hard/029-Nidoran♀-2.png` | 1 | 3 | 3.00 | 0 | 0 |
| png | `pkmn-bw-hard/030-Nidorina-2.png` | 1 | 3 | 3.00 | 0 | 0 |
| png | `pkmn-bw-hard/031-Nidoqueen.png` | 1 | 3 | 3.00 | 0 | 0 |
| png | `pkmn-bw-hard/038-Ninetales-1.png` | 1 | 3 | 3.00 | 0 | 0 |
| png | `pkmn-bw-hard/045-Vileplume-2.png` | 1 | 3 | 3.00 | 0 | 0 |
| png | `pkmn-bw-hard/046-Paras-1.png` | 1 | 3 | 3.00 | 0 | 0 |
| png | `pkmn-bw-hard/047-Parasect-0.png` | 1 | 3 | 3.00 | 0 | 0 |
| png | `pkmn-bw-hard/047-Parasect-1.png` | 1 | 3 | 3.00 | 0 | 0 |
| png | `pkmn-bw-hard/047-Parasect.png` | 1 | 3 | 3.00 | 0 | -1 |
| png | `pkmn-bw-hard/049-Venomoth-1.png` | 1 | 3 | 3.00 | 0 | -3 |
| png | `pkmn-bw-hard/050-Diglett.png` | 1 | 10 | 9.80 | 0 | 0 |
| png | `pkmn-bw-hard/051-Dugtrio-0.png` | 1 | 3 | 3.00 | 0 | 0 |
| png | `pkmn-bw-hard/051-Dugtrio.png` | 1 | 3 | 3.00 | 0 | 0 |
| png | `pkmn-bw-hard/052-Meowth-0.png` | 1 | 3 | 3.00 | 0 | 0 |
| png | `pkmn-bw-hard/052-Meowth.png` | 1 | 3 | 3.00 | 0 | 0 |
| png | `pkmn-bw-hard/054-Psyduck-0.png` | 1 | 3 | 3.00 | -1 | -5 |
| png | `pkmn-bw-hard/054-Psyduck-1.png` | 1 | 3 | 3.00 | 0 | 0 |
| png | `pkmn-bw-hard/054-Psyduck.png` | 1 | 3 | 3.00 | 0 | 0 |
| png | `pkmn-bw-hard/055-Golduck-0.png` | 1 | 3 | 3.00 | 0 | 0 |
| png | `pkmn-bw-hard/055-Golduck.png` | 1 | 3 | 3.00 | 0 | -3 |
| png | `pkmn-bw-hard/060-Poliwag-0.png` | 1 | 3 | 3.00 | 0 | -2 |
| png | `pkmn-bw-hard/060-Poliwag-2.png` | 1 | 3 | 3.00 | -3 | -17 |
| png | `pkmn-bw-hard/060-Poliwag.png` | 1 | 3 | 3.00 | 0 | 0 |
| png | `pkmn-bw-hard/061-Poliwhirl-2.png` | 1 | 3 | 3.00 | 0 | 0 |
| png | `pkmn-bw-hard/061-Poliwhirl.png` | 1 | 3 | 3.00 | 0 | 0 |
| png | `pkmn-bw-hard/062-Poliwrath-1.png` | 1 | 3 | 3.00 | -2 | -9 |
| png | `pkmn-bw-hard/063-Abra-2.png` | 1 | 3 | 3.00 | 0 | -1 |
| png | `pkmn-bw-hard/063-Abra.png` | 1 | 3 | 3.00 | 0 | 0 |
| png | `pkmn-bw-hard/065-Alakazam-1.png` | 1 | 3 | 3.01 | 0 | 0 |
| png | `pkmn-bw-hard/066-Machop-0.png` | 1 | 3 | 3.00 | 0 | 0 |
| png | `pkmn-bw-hard/068-Machamp-1.png` | 1 | 3 | 3.00 | 0 | 0 |
| png | `pkmn-bw-hard/068-Machamp.png` | 1 | 3 | 3.00 | 0 | -1 |
| png | `pkmn-bw-hard/069-Bellsprout-1.png` | 1 | 3 | 3.00 | 0 | 0 |
| png | `pkmn-bw-hard/074-Geodude.png` | 1 | 3 | 3.00 | 0 | 0 |
| png | `pkmn-bw-hard/075-Graveler.png` | 1 | 3 | 3.00 | 0 | 0 |
| png | `pkmn-bw-hard/076-Golem-0.png` | 1 | 3 | 3.00 | 0 | 0 |
| png | `pkmn-bw-hard/076-Golem-1.png` | 1 | 3 | 3.00 | 0 | -1 |
| png | `pkmn-bw-hard/077-Ponyta.png` | 1 | 3 | 3.00 | -1 | -4 |
| png | `pkmn-bw-hard/078-Rapidash.png` | 1 | 3 | 3.00 | 0 | -3 |
| png | `pkmn-bw-hard/079-Slowpoke-0.png` | 1 | 3 | 3.00 | 0 | 0 |
| png | `pkmn-bw-hard/080-Slowbro-1.png` | 1 | 3 | 3.00 | 0 | -1 |
| png | `pkmn-bw-hard/080-Slowbro-2.png` | 1 | 3 | 3.00 | 0 | 0 |
| png | `pkmn-bw-hard/080-Slowbro.png` | 1 | 3 | 3.00 | -1 | -7 |
| png | `pkmn-bw-hard/081-Magnemite-2.png` | 1 | 3 | 3.00 | 0 | -3 |
| png | `pkmn-bw-hard/082-Magneton-1.png` | 1 | 3 | 3.00 | -1 | -6 |
| png | `pkmn-bw-hard/082-Magneton-2.png` | 1 | 3 | 3.00 | 0 | 0 |
| png | `pkmn-bw-hard/083-Farfetch'd-1.png` | 1 | 3 | 3.00 | 0 | 0 |
| png | `pkmn-bw-hard/084-Doduo-1.png` | 1 | 3 | 3.00 | -1 | -6 |
| png | `pkmn-bw-hard/084-Doduo-2.png` | 1 | 3 | 3.00 | 0 | 0 |
| png | `pkmn-bw-hard/089-Muk-0.png` | 1 | 3 | 3.00 | 0 | 0 |
| png | `pkmn-bw-hard/090-Shellder-0.png` | 1 | 3 | 3.00 | 0 | -1 |
| png | `pkmn-bw-hard/090-Shellder.png` | 1 | 3 | 3.00 | 0 | 0 |
| png | `pkmn-bw-hard/091-Cloyster.png` | 1 | 3 | 3.00 | 0 | 0 |
| png | `pkmn-bw-hard/093-Haunter-2.png` | 1 | 3 | 3.00 | -1 | -3 |
| png | `pkmn-bw-hard/095-Onix.png` | 1 | 3 | 3.00 | 0 | -1 |
| png | `pkmn-bw-hard/096-Drowzee-2.png` | 1 | 3 | 3.00 | 0 | 0 |
| png | `pkmn-bw-hard/098-Krabby.png` | 1 | 3 | 3.00 | 0 | -3 |
| png | `pkmn-bw-hard/101-Electrode-0.png` | 1 | 3 | 3.00 | 0 | 0 |
| png | `pkmn-bw-hard/101-Electrode.png` | 1 | 3 | 3.00 | 0 | 0 |
| png | `pkmn-bw-hard/102-Exeggcute-1.png` | 1 | 3 | 3.00 | 0 | 0 |
| png | `pkmn-bw-hard/102-Exeggcute-2.png` | 1 | 3 | 3.00 | 0 | 0 |
| png | `pkmn-bw-hard/102-Exeggcute.png` | 1 | 3 | 3.00 | 0 | 0 |
| png | `pkmn-bw-hard/104-Cubone-2.png` | 1 | 10 | 9.80 | 0 | 0 |
| png | `pkmn-bw-hard/106-Hitmonlee-2.png` | 1 | 3 | 3.00 | 0 | 0 |
| png | `pkmn-bw-hard/109-Koffing-1.png` | 1 | 3 | 3.00 | 0 | 0 |
| png | `pkmn-bw-hard/113-Chansey-0.png` | 1 | 3 | 3.00 | 0 | 0 |
| png | `pkmn-bw-hard/113-Chansey-2.png` | 1 | 3 | 3.00 | 0 | 0 |
| png | `pkmn-bw-hard/113-Chansey.png` | 1 | 3 | 3.00 | 0 | 0 |
| png | `pkmn-bw-hard/114-Tangela-1.png` | 1 | 3 | 3.00 | 0 | 0 |
| png | `pkmn-bw-hard/114-Tangela.png` | 1 | 3 | 3.00 | -1 | -6 |
| png | `pkmn-bw-hard/115-Kangaskhan-1.png` | 1 | 3 | 3.00 | 0 | 0 |
| png | `pkmn-bw-hard/115-Kangaskhan.png` | 1 | 3 | 3.00 | -1 | -7 |
| png | `pkmn-bw-hard/116-Horsea-1.png` | 1 | 3 | 3.00 | 0 | 0 |
| png | `pkmn-bw-hard/116-Horsea-2.png` | 1 | 3 | 3.00 | 0 | 0 |
| png | `pkmn-bw-hard/122-Mr. Mime-0.png` | 1 | 3 | 3.00 | 0 | 0 |
| png | `pkmn-bw-hard/124-Jynx-2.png` | 1 | 3 | 3.00 | 0 | 0 |
| png | `pkmn-bw-hard/125-Electabuzz.png` | 1 | 3 | 3.00 | -2 | -17 |
| png | `pkmn-bw-hard/127-Pinsir-0.png` | 1 | 3 | 3.00 | 0 | 0 |
| png | `pkmn-bw-hard/128-Tauros-0.png` | 1 | 3 | 3.00 | 0 | 0 |
| png | `pkmn-bw-hard/128-Tauros-1.png` | 1 | 3 | 3.00 | 0 | 0 |
| png | `pkmn-bw-hard/128-Tauros-2.png` | 1 | 3 | 3.00 | 0 | -4 |
| png | `pkmn-bw-hard/129-Magikarp-2.png` | 1 | 3 | 3.00 | 0 | 0 |
| png | `pkmn-bw-hard/129-Magikarp.png` | 1 | 3 | 3.00 | 0 | 0 |
| png | `pkmn-bw-hard/132-Ditto.png` | 1 | 3 | 3.00 | 0 | 0 |
| png | `pkmn-bw-hard/134-Vaporeon-2.png` | 1 | 3 | 3.00 | 0 | -2 |
| png | `pkmn-bw-hard/137-Porygon-0.png` | 1 | 3 | 3.00 | 0 | 0 |
| png | `pkmn-bw-hard/137-Porygon-1.png` | 1 | 3 | 3.00 | 0 | 0 |
| png | `pkmn-bw-hard/139-Omastar-1.png` | 1 | 3 | 3.00 | 0 | 0 |
| png | `pkmn-bw-hard/140-Kabuto-2.png` | 1 | 3 | 3.00 | 0 | 0 |
| png | `pkmn-bw-hard/140-Kabuto.png` | 1 | 3 | 3.00 | 0 | -3 |
| png | `pkmn-bw-hard/141-Kabutops-2.png` | 1 | 3 | 3.00 | 0 | 0 |
| png | `pkmn-bw-hard/148-Dragonair-1.png` | 1 | 3 | 3.00 | 0 | -1 |
| png | `pkmn-bw-hard/149-Dragonite-0.png` | 1 | 3 | 3.00 | 0 | 0 |
| png | `pkmn-bw-hard/150-Mewtwo-2.png` | 1 | 3 | 3.00 | 0 | 0 |
| png | `pkmn-bw-hard/150-Mewtwo.png` | 1 | 3 | 3.00 | 0 | 0 |
| png | `pkmn-bw-hard/151-Mew-2.png` | 1 | 3 | 3.00 | 0 | 0 |
| png | `pkmn-bw/000-Logo-2.png` | 1 | 3 | 3.00 | -2 | -19 |
| png | `pkmn-bw/000-Logo-5.png` | 1 | 3 | 3.00 | -1 | -7 |
| png | `pkmn-bw/000-Logo-6.png` | 1 | 10 | 9.80 | -1 | -10 |
| png | `pkmn-bw/006-Charizard-2.png` | 1 | 3 | 3.00 | 0 | 0 |
| png | `pkmn-bw/006-Charizard.png` | 1 | 3 | 3.00 | -1 | -1 |
| png | `pkmn-bw/007-Squirtle-0.png` | 1 | 3 | 3.00 | 0 | -4 |
| png | `pkmn-bw/008-Wartortle.png` | 1 | 3 | 3.00 | -1 | -9 |
| png | `pkmn-bw/012-Butterfree-1.png` | 1 | 3 | 3.00 | 0 | 0 |
| png | `pkmn-bw/013-Weedle-0.png` | 1 | 3 | 3.00 | -1 | -5 |
| png | `pkmn-bw/013-Weedle-2.png` | 1 | 3 | 3.00 | -2 | -18 |
| png | `pkmn-bw/014-Kakuna-0.png` | 1 | 3 | 3.00 | -1 | -14 |
| png | `pkmn-bw/023-Ekans-1.png` | 1 | 3 | 3.00 | 0 | 0 |
| png | `pkmn-bw/023-Ekans-2.png` | 1 | 3 | 3.01 | -2 | -13 |
| png | `pkmn-bw/025-Pikachu.png` | 1 | 3 | 3.00 | -2 | -12 |
| png | `pkmn-bw/026-Raichu.png` | 1 | 10 | 9.80 | -1 | -6 |
| png | `pkmn-bw/029-Nidoran-F-2.png` | 1 | 3 | 3.00 | -1 | -4 |
| png | `pkmn-bw/034-Nidoking.png` | 1 | 3 | 3.00 | 0 | 0 |
| png | `pkmn-bw/035-Clefairy.png` | 1 | 3 | 3.00 | -1 | -7 |
| png | `pkmn-bw/036-Clefable.png` | 1 | 3 | 3.00 | 0 | 0 |
| png | `pkmn-bw/039-Jigglypuff-2.png` | 1 | 3 | 3.00 | -1 | -4 |
| png | `pkmn-bw/046-Paras-2.png` | 1 | 3 | 3.00 | -1 | -5 |
| png | `pkmn-bw/051-Dugtrio-1.png` | 1 | 3 | 3.00 | 0 | -1 |
| png | `pkmn-bw/054-Psyduck-0.png` | 1 | 3 | 3.00 | -1 | -5 |
| png | `pkmn-bw/054-Psyduck-1.png` | 1 | 3 | 3.00 | 0 | 0 |
| png | `pkmn-bw/054-Psyduck-2.png` | 1 | 3 | 3.00 | 0 | 0 |
| png | `pkmn-bw/056-Mankey-0.png` | 1 | 3 | 3.00 | 0 | 0 |
| png | `pkmn-bw/056-Mankey-1.png` | 1 | 3 | 3.00 | -1 | -8 |
| png | `pkmn-bw/056-Mankey.png` | 1 | 3 | 3.00 | 0 | -2 |
| png | `pkmn-bw/057-Primeape.png` | 1 | 3 | 3.00 | -1 | -7 |
| png | `pkmn-bw/059-Arcanine-0.png` | 1 | 3 | 3.00 | -1 | -4 |
| png | `pkmn-bw/060-Poliwag-0.png` | 1 | 3 | 3.00 | -2 | -12 |
| png | `pkmn-bw/060-Poliwag-2.png` | 1 | 3 | 3.00 | -3 | -17 |
| png | `pkmn-bw/061-Poliwhirl.png` | 1 | 3 | 3.00 | 0 | 0 |
| png | `pkmn-bw/062-Poliwrath-2.png` | 1 | 3 | 3.00 | -1 | -8 |
| png | `pkmn-bw/066-Machop.png` | 1 | 3 | 3.00 | -1 | -6 |
| png | `pkmn-bw/071-Victreebel-2.png` | 1 | 3 | 3.00 | -1 | -4 |
| png | `pkmn-bw/072-Tentacool-2.png` | 1 | 3 | 3.00 | 0 | 0 |
| png | `pkmn-bw/072-Tentacool.png` | 1 | 3 | 3.00 | -1 | -8 |
| png | `pkmn-bw/074-Geodude-0.png` | 1 | 3 | 3.00 | 0 | 0 |
| png | `pkmn-bw/081-Magnemite-1.png` | 1 | 3 | 3.00 | 0 | -6 |
| png | `pkmn-bw/083-Farfetch'd.png` | 1 | 3 | 3.00 | -1 | -4 |
| png | `pkmn-bw/084-Doduo.png` | 1 | 3 | 3.00 | 0 | -2 |
| png | `pkmn-bw/089-Muk.png` | 1 | 10 | 9.80 | -2 | -11 |
| png | `pkmn-bw/096-Drowzee.png` | 1 | 3 | 3.00 | -1 | -3 |
| png | `pkmn-bw/098-Krabby-1.png` | 1 | 3 | 3.00 | -1 | -4 |
| png | `pkmn-bw/103-Exeggutor.png` | 1 | 3 | 3.00 | -1 | -8 |
| png | `pkmn-bw/105-Marowak-0.png` | 1 | 3 | 3.00 | 0 | 0 |
| png | `pkmn-bw/110-Weezing.png` | 1 | 3 | 3.00 | 0 | 0 |
| png | `pkmn-bw/113-Chansey-0.png` | 1 | 3 | 3.00 | 0 | 0 |
| png | `pkmn-bw/113-Chansey.png` | 1 | 3 | 3.00 | 0 | 0 |
| png | `pkmn-bw/119-Seaking-0.png` | 1 | 3 | 3.00 | 0 | 0 |
| png | `pkmn-bw/122-Mr. Mime.png` | 1 | 3 | 3.00 | 0 | -7 |
| png | `pkmn-bw/124-Jynx-1.png` | 1 | 3 | 3.00 | 0 | 0 |
| png | `pkmn-bw/124-Jynx.png` | 1 | 3 | 3.00 | 0 | 0 |
| png | `pkmn-bw/126-Magmar-0.png` | 1 | 3 | 3.00 | 0 | -4 |
| png | `pkmn-bw/132-Ditto-2.png` | 1 | 3 | 3.00 | -2 | -17 |
| png | `pkmn-bw/133-Eevee-0.png` | 1 | 3 | 3.00 | 0 | -4 |
| png | `pkmn-bw/134-Vaporeon.png` | 1 | 3 | 3.00 | -1 | -5 |
| png | `pkmn-bw/136-Flareon-0.png` | 1 | 3 | 3.00 | 0 | 0 |
| png | `pkmn-bw/137-Porygon-0.png` | 1 | 3 | 3.00 | 0 | 0 |
| png | `pkmn-bw/148-Dragonair.png` | 1 | 3 | 3.00 | 0 | -2 |
| png | `pkmn-col-hard/001-Bulbasaur-0.png` | 1 | 3 | 3.00 | -2 | -11 |
| png | `pkmn-col-hard/002-Ivysaur-0.png` | 1 | 3 | 3.00 | 0 | 0 |
| png | `pkmn-col-hard/002-Ivysaur-2.png` | 1 | 3 | 3.01 | 0 | 0 |
| png | `pkmn-col-hard/003-Venusaur-1.png` | 1 | 3 | 3.00 | 0 | 0 |
| png | `pkmn-col-hard/003-Venusaur-2.png` | 1 | 3 | 3.00 | 0 | 0 |
| png | `pkmn-col-hard/004-Charmander-0.png` | 1 | 3 | 3.00 | 0 | -3 |
| png | `pkmn-col-hard/004-Charmander-1.png` | 1 | 3 | 3.01 | 0 | -2 |
| png | `pkmn-col-hard/004-Charmander-2.png` | 1 | 3 | 3.00 | 0 | 0 |
| png | `pkmn-col-hard/005-Charmeleon-0.png` | 1 | 3 | 3.01 | 0 | 0 |
| png | `pkmn-col-hard/005-Charmeleon-1.png` | 1 | 3 | 3.00 | 0 | 0 |
| png | `pkmn-col-hard/005-Charmeleon-2.png` | 1 | 3 | 3.00 | 0 | 0 |
| png | `pkmn-col-hard/006-Charizard-0.png` | 1 | 3 | 3.00 | 0 | 0 |
| png | `pkmn-col-hard/006-Charizard-2.png` | 1 | 3 | 3.00 | 0 | -4 |
| png | `pkmn-col-hard/007-Squirtle-0.png` | 1 | 3 | 3.00 | -1 | -8 |
| png | `pkmn-col-hard/007-Squirtle-1.png` | 1 | 3 | 3.00 | 0 | 0 |
| png | `pkmn-col-hard/007-Squirtle-2.png` | 1 | 3 | 3.00 | 0 | 0 |
| png | `pkmn-col-hard/008-Wartortle-1.png` | 1 | 3 | 3.00 | 0 | 0 |
| png | `pkmn-col-hard/009-Blastoise-0.png` | 1 | 3 | 3.00 | 0 | 0 |
| png | `pkmn-col-hard/010-Caterpie-0.png` | 1 | 3 | 3.00 | 0 | 0 |
| png | `pkmn-col-hard/010-Caterpie-1.png` | 1 | 3 | 3.00 | 0 | 0 |
| png | `pkmn-col-hard/010-Caterpie-2.png` | 1 | 3 | 3.00 | 0 | 0 |
| png | `pkmn-col-hard/011-Metapod-0.png` | 1 | 3 | 3.00 | -2 | -14 |
| png | `pkmn-col-hard/011-Metapod-1.png` | 1 | 3 | 3.00 | -3 | -20 |
| png | `pkmn-col-hard/011-Metapod-2.png` | 1 | 3 | 3.00 | 0 | 0 |
| png | `pkmn-col-hard/012-Butterfree-1.png` | 1 | 3 | 3.00 | 0 | -3 |
| png | `pkmn-col-hard/014-Kakuna-0.png` | 1 | 3 | 3.00 | -1 | -2 |
| png | `pkmn-col-hard/014-Kakuna-1.png` | 1 | 3 | 3.00 | -1 | -7 |
| png | `pkmn-col-hard/014-Kakuna-2.png` | 1 | 3 | 3.01 | 0 | 0 |
| png | `pkmn-col-hard/015-Beedrill-0.png` | 1 | 3 | 3.00 | 0 | 0 |
| png | `pkmn-col-hard/016-Pidgey-0.png` | 1 | 3 | 3.00 | 0 | 0 |
| png | `pkmn-col-hard/016-Pidgey-1.png` | 1 | 3 | 3.00 | 0 | -1 |
| png | `pkmn-col-hard/016-Pidgey-2.png` | 1 | 3 | 3.01 | 0 | -1 |
| png | `pkmn-col-hard/017-Pidgeotto-0.png` | 1 | 3 | 3.00 | 0 | 0 |
| png | `pkmn-col-hard/017-Pidgeotto-1.png` | 1 | 3 | 3.01 | 0 | 0 |
| png | `pkmn-col-hard/017-Pidgeotto-2.png` | 1 | 3 | 3.00 | -1 | -2 |
| png | `pkmn-col-hard/018-Pidgeot-0.png` | 1 | 3 | 3.00 | 0 | 0 |
| png | `pkmn-col-hard/018-Pidgeot-1.png` | 1 | 3 | 3.00 | 0 | -2 |
| png | `pkmn-col-hard/019-Rattata-0.png` | 1 | 3 | 3.00 | 0 | 0 |
| png | `pkmn-col-hard/020-Raticate-0.png` | 1 | 3 | 3.01 | 0 | 0 |
| png | `pkmn-col-hard/020-Raticate-1.png` | 1 | 3 | 3.00 | 0 | -1 |
| png | `pkmn-col-hard/020-Raticate-2.png` | 1 | 3 | 3.00 | -1 | -2 |
| png | `pkmn-col-hard/021-Spearow-2.png` | 1 | 3 | 3.00 | 0 | -3 |
| png | `pkmn-col-hard/022-Fearow-2.png` | 1 | 3 | 3.00 | 0 | -3 |
| png | `pkmn-col-hard/023-Ekans-2.png` | 1 | 3 | 3.00 | 0 | 0 |
| png | `pkmn-col-hard/024-Arbok-1.png` | 1 | 3 | 3.00 | -1 | -9 |
| png | `pkmn-col-hard/024-Arbok-2.png` | 1 | 3 | 3.00 | 0 | 0 |
| png | `pkmn-col-hard/025-Pikachu-1.png` | 1 | 3 | 3.00 | 0 | 0 |
| png | `pkmn-col-hard/025-Pikachu-2.png` | 1 | 3 | 3.00 | 0 | 0 |
| png | `pkmn-col-hard/026-Raichu-2.png` | 1 | 3 | 3.00 | -1 | -3 |
| png | `pkmn-col-hard/027-Sandshrew-0.png` | 1 | 3 | 3.00 | 0 | 0 |
| png | `pkmn-col-hard/027-Sandshrew-2.png` | 1 | 3 | 3.00 | -1 | -8 |
| png | `pkmn-col-hard/028-Sandslash-0.png` | 1 | 3 | 3.01 | 0 | 0 |
| png | `pkmn-col-hard/028-Sandslash-1.png` | 1 | 3 | 3.01 | 0 | 0 |
| png | `pkmn-col-hard/028-Sandslash-2.png` | 1 | 3 | 3.00 | 0 | 0 |
| png | `pkmn-col-hard/030-Nidorina-0.png` | 1 | 3 | 3.01 | 0 | 0 |
| png | `pkmn-col-hard/030-Nidorina-1.png` | 1 | 3 | 3.01 | 0 | 0 |
| png | `pkmn-col-hard/030-Nidorina-2.png` | 1 | 3 | 3.00 | 0 | 0 |
| png | `pkmn-col-hard/031-Nidoqueen-0.png` | 1 | 3 | 3.00 | 0 | 0 |
| png | `pkmn-col-hard/031-Nidoqueen-1.png` | 1 | 3 | 3.00 | 0 | -2 |
| png | `pkmn-col-hard/031-Nidoqueen-2.png` | 1 | 3 | 3.00 | 0 | -1 |
| png | `pkmn-col-hard/032-Nidoran♂-1.png` | 1 | 3 | 3.00 | -2 | -15 |
| png | `pkmn-col-hard/032-Nidoran♂-2.png` | 1 | 3 | 3.00 | 0 | 0 |
| png | `pkmn-col-hard/033-Nidorino-2.png` | 1 | 3 | 3.00 | 0 | 0 |
| png | `pkmn-col-hard/034-Nidoking-1.png` | 1 | 3 | 3.00 | 0 | 0 |
| png | `pkmn-col-hard/034-Nidoking-2.png` | 1 | 3 | 3.00 | 0 | 0 |
| png | `pkmn-col-hard/035-Clefairy-0.png` | 1 | 3 | 3.00 | -1 | -1 |
| png | `pkmn-col-hard/035-Clefairy-1.png` | 1 | 3 | 3.01 | 0 | 0 |
| png | `pkmn-col-hard/035-Clefairy-2.png` | 1 | 3 | 3.00 | 0 | 0 |
| png | `pkmn-col-hard/036-Clefable-0.png` | 1 | 3 | 3.00 | 0 | 0 |
| png | `pkmn-col-hard/037-Vulpix-1.png` | 1 | 3 | 3.00 | 0 | -2 |
| png | `pkmn-col-hard/037-Vulpix-2.png` | 1 | 3 | 3.00 | 0 | -3 |
| png | `pkmn-col-hard/038-Ninetales-0.png` | 1 | 3 | 3.00 | 0 | 0 |
| png | `pkmn-col-hard/038-Ninetales-1.png` | 1 | 3 | 3.00 | 0 | 0 |
| png | `pkmn-col-hard/038-Ninetales-2.png` | 1 | 3 | 3.00 | 0 | 0 |
| png | `pkmn-col-hard/039-Jigglypuff-1.png` | 1 | 3 | 3.00 | 0 | -2 |
| png | `pkmn-col-hard/040-Wigglytuff-0.png` | 1 | 3 | 3.00 | 0 | -5 |
| png | `pkmn-col-hard/040-Wigglytuff-2.png` | 1 | 3 | 3.00 | -1 | -4 |
| png | `pkmn-col-hard/041-Zubat-0.png` | 1 | 3 | 3.00 | 0 | 0 |
| png | `pkmn-col-hard/041-Zubat-1.png` | 1 | 3 | 3.01 | 0 | 0 |
| png | `pkmn-col-hard/041-Zubat-2.png` | 1 | 3 | 3.01 | 0 | -3 |
| png | `pkmn-col-hard/042-Golbat-2.png` | 1 | 3 | 3.00 | 0 | 0 |
| png | `pkmn-col-hard/043-Oddish-0.png` | 1 | 3 | 3.00 | 0 | 0 |
| png | `pkmn-col-hard/043-Oddish-1.png` | 1 | 3 | 3.01 | 0 | 0 |
| png | `pkmn-col-hard/043-Oddish-2.png` | 1 | 3 | 3.00 | 0 | 0 |
| png | `pkmn-col-hard/045-Vileplume-1.png` | 1 | 3 | 3.00 | 0 | -4 |
| png | `pkmn-col-hard/045-Vileplume-2.png` | 1 | 3 | 3.00 | 0 | 0 |
| png | `pkmn-col-hard/046-Paras-1.png` | 1 | 3 | 3.00 | 0 | 0 |
| png | `pkmn-col-hard/047-Parasect-1.png` | 1 | 3 | 3.00 | 0 | 0 |
| png | `pkmn-col-hard/048-Venonat-0.png` | 1 | 3 | 3.00 | 0 | -3 |
| png | `pkmn-col-hard/048-Venonat-1.png` | 1 | 3 | 3.00 | 0 | 0 |
| png | `pkmn-col-hard/048-Venonat-2.png` | 1 | 3 | 3.01 | 0 | 0 |
| png | `pkmn-col-hard/049-Venomoth-0.png` | 1 | 3 | 3.00 | 0 | 0 |
| png | `pkmn-col-hard/049-Venomoth-1.png` | 1 | 3 | 3.00 | 0 | 0 |
| png | `pkmn-col-hard/049-Venomoth-2.png` | 1 | 3 | 3.00 | 0 | -4 |
| png | `pkmn-col-hard/050-Diglett-1.png` | 1 | 3 | 3.00 | -1 | -12 |
| png | `pkmn-col-hard/051-Dugtrio-1.png` | 1 | 3 | 3.00 | 0 | 0 |
| png | `pkmn-col-hard/052-Meowth-1.png` | 1 | 3 | 3.00 | 0 | -1 |
| png | `pkmn-col-hard/052-Meowth-2.png` | 1 | 3 | 3.00 | 0 | -4 |
| png | `pkmn-col-hard/053-Persian-1.png` | 1 | 3 | 3.00 | 0 | 0 |
| png | `pkmn-col-hard/054-Psyduck-0.png` | 1 | 3 | 3.00 | 0 | -3 |
| png | `pkmn-col-hard/054-Psyduck-1.png` | 1 | 3 | 3.00 | -1 | -2 |
| png | `pkmn-col-hard/055-Golduck-0.png` | 1 | 3 | 3.00 | 0 | 0 |
| png | `pkmn-col-hard/056-Mankey-0.png` | 1 | 3 | 3.00 | 0 | 0 |
| png | `pkmn-col-hard/056-Mankey-2.png` | 1 | 3 | 3.00 | -1 | -4 |
| png | `pkmn-col-hard/057-Primeape-0.png` | 1 | 3 | 3.00 | -1 | -4 |
| png | `pkmn-col-hard/057-Primeape-2.png` | 1 | 3 | 3.00 | -1 | -3 |
| png | `pkmn-col-hard/058-Growlithe-0.png` | 1 | 3 | 3.01 | 0 | 0 |
| png | `pkmn-col-hard/058-Growlithe-1.png` | 1 | 3 | 3.00 | 0 | -2 |
| png | `pkmn-col-hard/058-Growlithe-2.png` | 1 | 10 | 9.81 | 0 | 0 |
| png | `pkmn-col-hard/059-Arcanine-0.png` | 1 | 3 | 3.00 | 0 | 0 |
| png | `pkmn-col-hard/059-Arcanine-2.png` | 1 | 3 | 3.00 | 0 | 0 |
| png | `pkmn-col-hard/060-Poliwag-1.png` | 1 | 3 | 3.01 | 0 | 0 |
| png | `pkmn-col-hard/060-Poliwag-2.png` | 1 | 3 | 3.00 | 0 | 0 |
| png | `pkmn-col-hard/061-Poliwhirl-2.png` | 1 | 3 | 3.00 | 0 | 0 |
| png | `pkmn-col-hard/062-Poliwrath-0.png` | 1 | 3 | 3.00 | 0 | 0 |
| png | `pkmn-col-hard/062-Poliwrath-2.png` | 1 | 3 | 3.00 | 0 | 0 |
| png | `pkmn-col-hard/063-Abra-0.png` | 1 | 3 | 3.00 | -1 | -7 |
| png | `pkmn-col-hard/063-Abra-1.png` | 1 | 3 | 3.00 | -1 | -9 |
| png | `pkmn-col-hard/063-Abra-2.png` | 1 | 3 | 3.01 | -1 | -8 |
| png | `pkmn-col-hard/064-Kadabra-0.png` | 1 | 3 | 3.00 | 0 | 0 |
| png | `pkmn-col-hard/064-Kadabra-1.png` | 1 | 3 | 3.00 | 0 | -2 |
| png | `pkmn-col-hard/064-Kadabra-2.png` | 1 | 3 | 3.00 | 0 | 0 |
| png | `pkmn-col-hard/065-Alakazam-0.png` | 1 | 3 | 3.01 | 0 | 0 |
| png | `pkmn-col-hard/065-Alakazam-1.png` | 1 | 3 | 3.00 | 0 | 0 |
| png | `pkmn-col-hard/066-Machop-1.png` | 1 | 3 | 3.00 | -1 | -3 |
| png | `pkmn-col-hard/066-Machop-2.png` | 1 | 3 | 3.00 | 0 | 0 |
| png | `pkmn-col-hard/067-Machoke-2.png` | 1 | 3 | 3.00 | 0 | -1 |
| png | `pkmn-col-hard/068-Machamp-1.png` | 1 | 3 | 3.00 | 0 | 0 |
| png | `pkmn-col-hard/068-Machamp-2.png` | 1 | 3 | 3.00 | 0 | -1 |
| png | `pkmn-col-hard/069-Bellsprout-2.png` | 1 | 3 | 3.00 | 0 | 0 |
| png | `pkmn-col-hard/070-Weepinbell-2.png` | 1 | 3 | 3.00 | 0 | 0 |
| png | `pkmn-col-hard/072-Tentacool-0.png` | 1 | 3 | 3.01 | 0 | 0 |
| png | `pkmn-col-hard/073-Tentacruel-0.png` | 1 | 3 | 3.00 | 0 | 0 |
| png | `pkmn-col-hard/073-Tentacruel-1.png` | 1 | 3 | 3.01 | 0 | 0 |
| png | `pkmn-col-hard/073-Tentacruel-2.png` | 1 | 3 | 3.00 | 0 | 0 |
| png | `pkmn-col-hard/074-Geodude-1.png` | 1 | 3 | 3.01 | 0 | 0 |
| png | `pkmn-col-hard/076-Golem-0.png` | 1 | 3 | 3.01 | 0 | 0 |
| png | `pkmn-col-hard/076-Golem-1.png` | 1 | 3 | 3.00 | 0 | 0 |
| png | `pkmn-col-hard/077-Ponyta-2.png` | 1 | 3 | 3.01 | 0 | 0 |
| png | `pkmn-col-hard/078-Rapidash-0.png` | 1 | 3 | 3.00 | 0 | 0 |
| png | `pkmn-col-hard/079-Slowpoke-1.png` | 1 | 3 | 3.00 | 0 | 0 |
| png | `pkmn-col-hard/080-Slowbro-0.png` | 1 | 3 | 3.00 | -1 | -13 |
| png | `pkmn-col-hard/080-Slowbro-1.png` | 1 | 3 | 3.00 | 0 | 0 |
| png | `pkmn-col-hard/081-Magnemite-2.png` | 1 | 3 | 3.00 | 0 | -1 |
| png | `pkmn-col-hard/082-Magneton-0.png` | 1 | 3 | 3.00 | 0 | -1 |
| png | `pkmn-col-hard/082-Magneton-1.png` | 1 | 3 | 3.00 | 0 | 0 |
| png | `pkmn-col-hard/082-Magneton-2.png` | 1 | 3 | 3.00 | 0 | 0 |
| png | `pkmn-col-hard/083-Farfetch'd-1.png` | 1 | 3 | 3.01 | 0 | 0 |
| png | `pkmn-col-hard/083-Farfetch'd-2.png` | 1 | 3 | 3.01 | 0 | 0 |
| png | `pkmn-col-hard/084-Doduo-2.png` | 1 | 3 | 3.00 | 0 | -3 |
| png | `pkmn-col-hard/086-Seel-0.png` | 1 | 3 | 3.00 | 0 | 0 |
| png | `pkmn-col-hard/086-Seel-1.png` | 1 | 3 | 3.01 | 0 | 0 |
| png | `pkmn-col-hard/088-Grimer-2.png` | 1 | 3 | 3.00 | 0 | 0 |
| png | `pkmn-col-hard/089-Muk-1.png` | 1 | 3 | 3.00 | -1 | -5 |
| png | `pkmn-col-hard/090-Shellder-0.png` | 1 | 3 | 3.00 | 0 | -5 |
| png | `pkmn-col-hard/090-Shellder-2.png` | 1 | 3 | 3.00 | 0 | 0 |
| png | `pkmn-col-hard/092-Gastly-0.png` | 1 | 3 | 3.00 | -1 | -3 |
| png | `pkmn-col-hard/092-Gastly-1.png` | 1 | 3 | 3.00 | 0 | 0 |
| png | `pkmn-col-hard/093-Haunter-1.png` | 1 | 3 | 3.01 | 0 | 0 |
| png | `pkmn-col-hard/094-Gengar-0.png` | 1 | 3 | 3.00 | 0 | 0 |
| png | `pkmn-col-hard/095-Onix-1.png` | 1 | 3 | 3.00 | -1 | -4 |
| png | `pkmn-col-hard/095-Onix-2.png` | 1 | 3 | 3.01 | 0 | 0 |
| png | `pkmn-col-hard/096-Drowzee-0.png` | 1 | 3 | 3.00 | -1 | -5 |
| png | `pkmn-col-hard/096-Drowzee-1.png` | 1 | 3 | 3.00 | 0 | 0 |
| png | `pkmn-col-hard/097-Hypno-2.png` | 1 | 3 | 3.00 | 0 | 0 |
| png | `pkmn-col-hard/099-Kingler-0.png` | 1 | 3 | 3.00 | -1 | -3 |
| png | `pkmn-col-hard/099-Kingler-1.png` | 1 | 3 | 3.01 | 0 | 0 |
| png | `pkmn-col-hard/100-Voltorb-0.png` | 1 | 3 | 3.00 | 0 | 0 |
| png | `pkmn-col-hard/100-Voltorb-1.png` | 1 | 3 | 3.00 | 0 | 0 |
| png | `pkmn-col-hard/100-Voltorb-2.png` | 1 | 3 | 3.00 | 0 | 0 |
| png | `pkmn-col-hard/101-Electrode-1.png` | 1 | 3 | 3.00 | 0 | 0 |
| png | `pkmn-col-hard/101-Electrode-2.png` | 1 | 3 | 3.00 | 0 | 0 |
| png | `pkmn-col-hard/102-Exeggcute-0.png` | 1 | 3 | 3.00 | -1 | -6 |
| png | `pkmn-col-hard/102-Exeggcute-1.png` | 1 | 3 | 3.00 | 0 | 0 |
| png | `pkmn-col-hard/103-Exeggutor-1.png` | 1 | 3 | 3.00 | 0 | 0 |
| png | `pkmn-col-hard/103-Exeggutor-2.png` | 1 | 3 | 3.00 | 0 | 0 |
| png | `pkmn-col-hard/104-Cubone-2.png` | 1 | 3 | 3.00 | 0 | -7 |
| png | `pkmn-col-hard/105-Marowak-0.png` | 1 | 3 | 3.00 | 0 | -3 |
| png | `pkmn-col-hard/105-Marowak-1.png` | 1 | 3 | 3.00 | 0 | -2 |
| png | `pkmn-col-hard/105-Marowak-2.png` | 1 | 3 | 3.00 | 0 | -4 |
| png | `pkmn-col-hard/106-Hitmonlee-0.png` | 1 | 3 | 3.00 | 0 | 0 |
| png | `pkmn-col-hard/107-Hitmonchan-0.png` | 1 | 3 | 3.00 | -1 | -9 |
| png | `pkmn-col-hard/107-Hitmonchan-1.png` | 1 | 3 | 3.00 | 0 | -2 |
| png | `pkmn-col-hard/107-Hitmonchan-2.png` | 1 | 3 | 3.00 | -1 | -2 |
| png | `pkmn-col-hard/108-Lickitung-0.png` | 1 | 3 | 3.00 | 0 | -4 |
| png | `pkmn-col-hard/108-Lickitung-1.png` | 1 | 3 | 3.00 | -1 | -9 |
| png | `pkmn-col-hard/108-Lickitung-2.png` | 1 | 3 | 3.00 | 0 | 0 |
| png | `pkmn-col-hard/109-Koffing-2.png` | 1 | 3 | 3.00 | 0 | 0 |
| png | `pkmn-col-hard/110-Weezing-2.png` | 1 | 3 | 3.00 | -2 | -10 |
| png | `pkmn-col-hard/111-Rhyhorn-1.png` | 1 | 3 | 3.00 | 0 | 0 |
| png | `pkmn-col-hard/111-Rhyhorn-2.png` | 1 | 3 | 3.00 | 0 | -3 |
| png | `pkmn-col-hard/112-Rhydon-0.png` | 1 | 3 | 3.01 | 0 | -1 |
| png | `pkmn-col-hard/112-Rhydon-1.png` | 1 | 3 | 3.00 | 0 | -3 |
| png | `pkmn-col-hard/112-Rhydon-2.png` | 1 | 3 | 3.00 | 0 | 0 |
| png | `pkmn-col-hard/113-Chansey-2.png` | 1 | 3 | 3.01 | 0 | 0 |
| png | `pkmn-col-hard/114-Tangela-1.png` | 1 | 3 | 3.00 | 0 | 0 |
| png | `pkmn-col-hard/114-Tangela-2.png` | 1 | 3 | 3.00 | 0 | 0 |
| png | `pkmn-col-hard/115-Kangaskhan-2.png` | 1 | 3 | 3.00 | 0 | 0 |
| png | `pkmn-col-hard/116-Horsea-0.png` | 1 | 3 | 3.00 | -1 | -5 |
| png | `pkmn-col-hard/116-Horsea-1.png` | 1 | 3 | 3.01 | 0 | 0 |
| png | `pkmn-col-hard/116-Horsea-2.png` | 1 | 3 | 3.00 | -2 | -9 |
| png | `pkmn-col-hard/117-Seadra-2.png` | 1 | 3 | 3.01 | 0 | 0 |
| png | `pkmn-col-hard/118-Goldeen-0.png` | 1 | 3 | 3.01 | 0 | 0 |
| png | `pkmn-col-hard/118-Goldeen-2.png` | 1 | 3 | 3.01 | 0 | 0 |
| png | `pkmn-col-hard/119-Seaking-2.png` | 1 | 3 | 3.00 | -1 | -8 |
| png | `pkmn-col-hard/120-Staryu-2.png` | 1 | 3 | 3.00 | 0 | 0 |
| png | `pkmn-col-hard/121-Starmie-0.png` | 1 | 3 | 3.00 | 0 | -3 |
| png | `pkmn-col-hard/121-Starmie-2.png` | 1 | 3 | 3.00 | 0 | 0 |
| png | `pkmn-col-hard/122-Mr. Mime-0.png` | 1 | 3 | 3.00 | 0 | 0 |
| png | `pkmn-col-hard/122-Mr. Mime-1.png` | 1 | 3 | 3.00 | 0 | 0 |
| png | `pkmn-col-hard/122-Mr. Mime-2.png` | 1 | 3 | 3.00 | 0 | -1 |
| png | `pkmn-col-hard/123-Scyther-0.png` | 1 | 3 | 3.01 | 0 | 0 |
| png | `pkmn-col-hard/124-Jynx-2.png` | 1 | 3 | 3.00 | 0 | 0 |
| png | `pkmn-col-hard/125-Electabuzz-1.png` | 1 | 3 | 3.00 | 0 | 0 |
| png | `pkmn-col-hard/126-Magmar-0.png` | 1 | 3 | 3.00 | -1 | -3 |
| png | `pkmn-col-hard/126-Magmar-2.png` | 1 | 3 | 3.01 | 0 | 0 |
| png | `pkmn-col-hard/128-Tauros-0.png` | 1 | 3 | 3.00 | 0 | 0 |
| png | `pkmn-col-hard/129-Magikarp-0.png` | 1 | 3 | 3.00 | 0 | 0 |
| png | `pkmn-col-hard/129-Magikarp-1.png` | 1 | 3 | 3.01 | 0 | 0 |
| png | `pkmn-col-hard/129-Magikarp-2.png` | 1 | 3 | 3.00 | 0 | 0 |
| png | `pkmn-col-hard/130-Gyarados-2.png` | 1 | 3 | 3.00 | 0 | 0 |
| png | `pkmn-col-hard/131-Lapras-0.png` | 1 | 3 | 3.00 | 0 | 0 |
| png | `pkmn-col-hard/131-Lapras-2.png` | 1 | 3 | 3.00 | 0 | 0 |
| png | `pkmn-col-hard/132-Ditto-1.png` | 1 | 10 | 9.80 | 0 | 0 |
| png | `pkmn-col-hard/132-Ditto-2.png` | 1 | 3 | 3.00 | 0 | 0 |
| png | `pkmn-col-hard/133-Eevee-1.png` | 1 | 3 | 3.00 | 0 | 0 |
| png | `pkmn-col-hard/134-Vaporeon-0.png` | 1 | 3 | 3.00 | 0 | 0 |
| png | `pkmn-col-hard/135-Jolteon-0.png` | 1 | 3 | 3.01 | 0 | -1 |
| png | `pkmn-col-hard/135-Jolteon-1.png` | 1 | 3 | 3.00 | 0 | 0 |
| png | `pkmn-col-hard/135-Jolteon-2.png` | 1 | 3 | 3.01 | 0 | 0 |
| png | `pkmn-col-hard/136-Flareon-1.png` | 1 | 3 | 3.01 | 0 | 0 |
| png | `pkmn-col-hard/136-Flareon-2.png` | 1 | 3 | 3.00 | 0 | 0 |
| png | `pkmn-col-hard/137-Porygon-0.png` | 1 | 3 | 3.00 | 0 | 0 |
| png | `pkmn-col-hard/137-Porygon-1.png` | 1 | 3 | 3.00 | 0 | 0 |
| png | `pkmn-col-hard/137-Porygon-2.png` | 1 | 3 | 3.00 | 0 | -6 |
| png | `pkmn-col-hard/138-Omanyte-0.png` | 1 | 3 | 3.00 | -1 | -7 |
| png | `pkmn-col-hard/138-Omanyte-2.png` | 1 | 3 | 3.00 | -1 | -8 |
| png | `pkmn-col-hard/139-Omastar-0.png` | 1 | 3 | 3.00 | 0 | 0 |
| png | `pkmn-col-hard/139-Omastar-1.png` | 1 | 3 | 3.00 | 0 | 0 |
| png | `pkmn-col-hard/139-Omastar-2.png` | 1 | 3 | 3.00 | 0 | 0 |
| png | `pkmn-col-hard/140-Kabuto-2.png` | 1 | 3 | 3.01 | 0 | -3 |
| png | `pkmn-col-hard/141-Kabutops-0.png` | 1 | 3 | 3.00 | 0 | 0 |
| png | `pkmn-col-hard/141-Kabutops-1.png` | 1 | 3 | 3.00 | 0 | 0 |
| png | `pkmn-col-hard/142-Aerodactyl-0.png` | 1 | 3 | 3.00 | 0 | 0 |
| png | `pkmn-col-hard/142-Aerodactyl-1.png` | 1 | 3 | 3.00 | 0 | 0 |
| png | `pkmn-col-hard/142-Aerodactyl-2.png` | 1 | 3 | 3.00 | 0 | 0 |
| png | `pkmn-col-hard/143-Snorlax-1.png` | 1 | 3 | 3.00 | 0 | 0 |
| png | `pkmn-col-hard/144-Articuno-0.png` | 1 | 3 | 3.00 | 0 | 0 |
| png | `pkmn-col-hard/144-Articuno-2.png` | 1 | 10 | 9.80 | 0 | 0 |
| png | `pkmn-col-hard/145-Zapdos-1.png` | 1 | 3 | 3.01 | 0 | 0 |
| png | `pkmn-col-hard/146-Moltres-0.png` | 1 | 3 | 3.01 | 0 | 0 |
| png | `pkmn-col-hard/146-Moltres-1.png` | 1 | 3 | 3.00 | 0 | 0 |
| png | `pkmn-col-hard/147-Dratini-0.png` | 1 | 3 | 3.00 | -1 | -6 |
| png | `pkmn-col-hard/147-Dratini-2.png` | 1 | 3 | 3.00 | -1 | -5 |
| png | `pkmn-col-hard/148-Dragonair-0.png` | 1 | 3 | 3.00 | -1 | -2 |
| png | `pkmn-col-hard/148-Dragonair-2.png` | 1 | 3 | 3.00 | 0 | 0 |
| png | `pkmn-col-hard/150-Mewtwo-2.png` | 1 | 3 | 3.00 | 0 | 0 |
| png | `pkmn-col-hard/151-Mew-0.png` | 1 | 3 | 3.01 | 0 | 0 |
| png | `pkmn-col-hard/151-Mew-1.png` | 1 | 3 | 3.00 | 0 | 0 |
| png | `pkmn-col-hard/151-Mew-2.png` | 1 | 3 | 3.00 | 0 | 0 |
| png | `pkmn-col/001-Bulbasaur-2.png` | 1 | 3 | 3.00 | -1 | -2 |
| png | `pkmn-col/002-Ivysaur-1.png` | 1 | 3 | 3.00 | -2 | -12 |
| png | `pkmn-col/003-Venusaur-1.png` | 1 | 3 | 3.01 | 0 | 0 |
| png | `pkmn-col/005-Charmeleon-2.png` | 1 | 3 | 3.00 | 0 | 0 |
| png | `pkmn-col/006-Charizard-2.png` | 1 | 3 | 3.01 | 0 | -4 |
| png | `pkmn-col/013-Weedle-1.png` | 1 | 3 | 3.00 | 0 | 0 |
| png | `pkmn-col/015-Beedrill-0.png` | 1 | 3 | 3.00 | 0 | 0 |
| png | `pkmn-col/016-Pidgey-1.png` | 1 | 3 | 3.00 | -1 | -10 |
| png | `pkmn-col/017-Pidgeotto-1.png` | 1 | 3 | 3.00 | 0 | 0 |
| png | `pkmn-col/017-Pidgeotto-2.png` | 1 | 10 | 9.81 | -1 | -2 |
| png | `pkmn-col/020-Raticate-0.png` | 1 | 3 | 3.00 | 0 | 0 |
| png | `pkmn-col/020-Raticate-2.png` | 1 | 3 | 3.01 | 0 | 0 |
| png | `pkmn-col/022-Fearow-0.png` | 1 | 3 | 3.00 | 0 | 0 |
| png | `pkmn-col/022-Fearow-2.png` | 1 | 3 | 3.00 | 0 | 0 |
| png | `pkmn-col/024-Arbok-1.png` | 1 | 3 | 3.00 | -1 | -9 |
| png | `pkmn-col/026-Raichu-2.png` | 1 | 3 | 3.01 | 0 | 0 |
| png | `pkmn-col/028-Sandslash-1.png` | 1 | 3 | 3.00 | 0 | 0 |
| png | `pkmn-col/029-Nidoran-F-2.png` | 1 | 3 | 3.00 | 0 | -1 |
| png | `pkmn-col/030-Nidorina-1.png` | 1 | 3 | 3.01 | 0 | 0 |
| png | `pkmn-col/032-Nidoran-M-2.png` | 1 | 3 | 3.00 | 0 | 0 |
| png | `pkmn-col/034-Nidoking-1.png` | 1 | 3 | 3.00 | 0 | 0 |
| png | `pkmn-col/034-Nidoking-2.png` | 1 | 3 | 3.00 | 0 | 0 |
| png | `pkmn-col/035-Clefairy-1.png` | 1 | 3 | 3.00 | 0 | 0 |
| png | `pkmn-col/037-Vulpix-1.png` | 1 | 3 | 3.01 | 0 | -2 |
| png | `pkmn-col/038-Ninetales-2.png` | 1 | 3 | 3.00 | 0 | 0 |
| png | `pkmn-col/039-Jigglypuff-2.png` | 1 | 3 | 3.00 | -1 | -5 |
| png | `pkmn-col/043-Oddish-0.png` | 1 | 3 | 3.00 | 0 | 0 |
| png | `pkmn-col/044-Gloom-0.png` | 1 | 3 | 3.00 | -1 | -3 |
| png | `pkmn-col/045-Vileplume-0.png` | 1 | 3 | 3.00 | -1 | -4 |
| png | `pkmn-col/048-Venonat-1.png` | 1 | 3 | 3.00 | 0 | 0 |
| png | `pkmn-col/048-Venonat-2.png` | 1 | 3 | 3.01 | 0 | 0 |
| png | `pkmn-col/052-Meowth-1.png` | 1 | 3 | 3.00 | 0 | -1 |
| png | `pkmn-col/052-Meowth-2.png` | 1 | 3 | 3.00 | 0 | -4 |
| png | `pkmn-col/055-Golduck-0.png` | 1 | 3 | 3.01 | 0 | 0 |
| png | `pkmn-col/055-Golduck-2.png` | 1 | 3 | 3.00 | 0 | 0 |
| png | `pkmn-col/057-Primeape-0.png` | 1 | 3 | 3.00 | -1 | -4 |
| png | `pkmn-col/058-Growlithe-0.png` | 1 | 3 | 3.01 | 0 | 0 |
| png | `pkmn-col/058-Growlithe-1.png` | 1 | 3 | 3.00 | 0 | -2 |
| png | `pkmn-col/060-Poliwag-1.png` | 1 | 3 | 3.00 | -1 | -1 |
| png | `pkmn-col/060-Poliwag-2.png` | 1 | 3 | 3.01 | -1 | -5 |
| png | `pkmn-col/062-Poliwrath-2.png` | 1 | 3 | 3.00 | 0 | 0 |
| png | `pkmn-col/063-Abra-2.png` | 1 | 3 | 3.00 | -1 | -8 |
| png | `pkmn-col/064-Kadabra-1.png` | 1 | 3 | 3.00 | 0 | 0 |
| png | `pkmn-col/066-Machop-0.png` | 1 | 3 | 3.01 | 0 | 0 |
| png | `pkmn-col/068-Machamp-0.png` | 1 | 3 | 3.00 | 0 | -2 |
| png | `pkmn-col/068-Machamp-1.png` | 1 | 3 | 3.00 | 0 | 0 |
| png | `pkmn-col/069-Bellsprout-2.png` | 1 | 3 | 3.00 | 0 | 0 |
| png | `pkmn-col/071-Victreebel-2.png` | 1 | 3 | 3.01 | 0 | 0 |
| png | `pkmn-col/073-Tentacruel-0.png` | 1 | 3 | 3.00 | 0 | 0 |
| png | `pkmn-col/074-Geodude-1.png` | 1 | 3 | 3.00 | -1 | -5 |
| png | `pkmn-col/075-Graveler-0.png` | 1 | 3 | 3.01 | 0 | 0 |
| png | `pkmn-col/076-Golem-1.png` | 1 | 3 | 3.01 | 0 | 0 |
| png | `pkmn-col/077-Ponyta-2.png` | 1 | 3 | 3.01 | 0 | 0 |
| png | `pkmn-col/078-Rapidash-0.png` | 1 | 3 | 3.00 | 0 | 0 |
| png | `pkmn-col/079-Slowpoke-1.png` | 1 | 3 | 3.01 | 0 | 0 |
| png | `pkmn-col/079-Slowpoke-2.png` | 1 | 3 | 3.01 | -1 | -8 |
| png | `pkmn-col/080-Slowbro-1.png` | 1 | 3 | 3.01 | 0 | 0 |
| png | `pkmn-col/083-Farfetch'd-1.png` | 1 | 3 | 3.01 | 0 | 0 |
| png | `pkmn-col/084-Doduo-0.png` | 1 | 3 | 3.00 | -1 | -9 |
| png | `pkmn-col/084-Doduo-1.png` | 1 | 3 | 3.00 | 0 | 0 |
| png | `pkmn-col/086-Seel-0.png` | 1 | 3 | 3.00 | 0 | 0 |
| png | `pkmn-col/086-Seel-1.png` | 1 | 3 | 3.00 | 0 | 0 |
| png | `pkmn-col/086-Seel-2.png` | 1 | 3 | 3.00 | -1 | -9 |
| png | `pkmn-col/087-Dewgong-0.png` | 1 | 3 | 3.00 | 0 | -6 |
| png | `pkmn-col/088-Grimer-2.png` | 1 | 3 | 3.01 | 0 | 0 |
| png | `pkmn-col/089-Muk-2.png` | 1 | 3 | 3.00 | 0 | 0 |
| png | `pkmn-col/091-Cloyster-0.png` | 1 | 3 | 3.00 | -1 | -3 |
| png | `pkmn-col/094-Gengar-1.png` | 1 | 3 | 3.00 | 0 | 0 |
| png | `pkmn-col/095-Onix-2.png` | 1 | 3 | 3.01 | 0 | 0 |
| png | `pkmn-col/096-Drowzee-0.png` | 1 | 3 | 3.00 | -1 | -5 |
| png | `pkmn-col/096-Drowzee-1.png` | 1 | 3 | 3.00 | 0 | 0 |
| png | `pkmn-col/098-Krabby-0.png` | 1 | 3 | 3.00 | -1 | -4 |
| png | `pkmn-col/100-Voltorb-0.png` | 1 | 10 | 9.80 | 0 | 0 |
| png | `pkmn-col/100-Voltorb-1.png` | 1 | 3 | 3.01 | 0 | 0 |
| png | `pkmn-col/106-Hitmonlee-0.png` | 1 | 3 | 3.01 | 0 | 0 |
| png | `pkmn-col/106-Hitmonlee-2.png` | 1 | 3 | 3.00 | -1 | -1 |
| png | `pkmn-col/107-Hitmonchan-2.png` | 1 | 3 | 3.00 | -1 | -2 |
| png | `pkmn-col/108-Lickitung-2.png` | 1 | 3 | 3.01 | 0 | 0 |
| png | `pkmn-col/110-Weezing-2.png` | 1 | 3 | 3.01 | -1 | -5 |
| png | `pkmn-col/112-Rhydon-0.png` | 1 | 3 | 3.00 | 0 | 0 |
| png | `pkmn-col/112-Rhydon-1.png` | 1 | 3 | 3.00 | 0 | -1 |
| png | `pkmn-col/113-Chansey-2.png` | 1 | 3 | 3.01 | 0 | 0 |
| png | `pkmn-col/115-Kangaskhan-2.png` | 1 | 3 | 3.01 | 0 | 0 |
| png | `pkmn-col/116-Horsea-1.png` | 1 | 3 | 3.00 | 0 | 0 |
| png | `pkmn-col/118-Goldeen-0.png` | 1 | 3 | 3.01 | 0 | 0 |
| png | `pkmn-col/119-Seaking-1.png` | 1 | 3 | 3.00 | 0 | 0 |
| png | `pkmn-col/119-Seaking-2.png` | 1 | 3 | 3.00 | -1 | -8 |
| png | `pkmn-col/121-Starmie-1.png` | 1 | 3 | 3.00 | 0 | -4 |
| png | `pkmn-col/122-Mr. Mime-1.png` | 1 | 3 | 3.01 | 0 | 0 |
| png | `pkmn-col/122-Mr. Mime-2.png` | 1 | 3 | 3.01 | 0 | -1 |
| png | `pkmn-col/123-Scyther-0.png` | 1 | 3 | 3.01 | 0 | 0 |
| png | `pkmn-col/125-Electabuzz-2.png` | 1 | 10 | 9.80 | -1 | -8 |
| png | `pkmn-col/126-Magmar-0.png` | 1 | 3 | 3.00 | -1 | -3 |
| png | `pkmn-col/126-Magmar-2.png` | 1 | 3 | 3.00 | 0 | 0 |
| png | `pkmn-col/129-Magikarp-1.png` | 1 | 3 | 3.00 | 0 | -2 |
| png | `pkmn-col/130-Gyarados-2.png` | 1 | 3 | 3.00 | 0 | 0 |
| png | `pkmn-col/131-Lapras-2.png` | 1 | 3 | 3.00 | 0 | 0 |
| png | `pkmn-col/133-Eevee-1.png` | 1 | 3 | 3.00 | 0 | 0 |
| png | `pkmn-col/133-Eevee-2.png` | 1 | 3 | 3.00 | 0 | 0 |
| png | `pkmn-col/135-Jolteon-0.png` | 1 | 3 | 3.00 | 0 | -1 |
| png | `pkmn-col/135-Jolteon-1.png` | 1 | 3 | 3.00 | 0 | 0 |
| png | `pkmn-col/135-Jolteon-2.png` | 1 | 3 | 3.00 | 0 | 0 |
| png | `pkmn-col/137-Porygon-1.png` | 1 | 3 | 3.00 | 0 | 0 |
| png | `pkmn-col/137-Porygon-2.png` | 1 | 3 | 3.00 | 0 | 0 |
| png | `pkmn-col/141-Kabutops-0.png` | 1 | 3 | 3.00 | 0 | 0 |
| png | `pkmn-col/142-Aerodactyl-2.png` | 1 | 3 | 3.00 | 0 | 0 |
| png | `pkmn-col/144-Articuno-1.png` | 1 | 3 | 3.01 | 0 | 0 |
| png | `pkmn-col/146-Moltres-0.png` | 1 | 3 | 3.00 | -1 | -6 |
| png | `pkmn-col/146-Moltres-1.png` | 1 | 3 | 3.00 | 0 | 0 |
| png | `pkmn-col/148-Dragonair-2.png` | 1 | 3 | 3.00 | 0 | 0 |
| png | `pkmn-col/151-Mew-1.png` | 1 | 3 | 3.00 | 0 | 0 |
| png | `samplelib-png/sample-alpha-checker-400x300.png` | 1 | 3 | 3.02 | 0 | 0 |
| png | `samplelib-png/sample-alpha-circle-400x300.png` | 1 | 3 | 3.03 | 0 | -4 |
| png | `samplelib-png/sample-alpha-radial-400x300.png` | 1 | 3 | 3.02 | -13 | -109 |
| png | `samplelib-png/sample-alpha-semi-400x300.png` | 1 | 3 | 3.03 | 0 | 0 |
| png | `samplelib-png/sample-blue-100x75.png` | 1 | 3 | 1.51 | 0 | 0 |
| png | `samplelib-png/sample-blue-200x200.png` | 1 | 3 | 3.00 | 0 | 0 |
| png | `samplelib-png/sample-boat-400x300.png` | 38 | 40 | 39.36 | -327 | -2619 |
| png | `samplelib-png/sample-bumblebee-400x300.png` | 18 | 20 | 20.37 | -15 | -120 |
| png | `samplelib-png/sample-clouds2-400x300.png` | 33 | 35 | 35.11 | -21 | -164 |
| png | `samplelib-png/sample-green-100x75.png` | 1 | 3 | 1.54 | 0 | 0 |
| png | `samplelib-png/sample-green-200x200.png` | 1 | 3 | 3.00 | 0 | 0 |
| png | `samplelib-png/sample-green-400x300.png` | 1 | 3 | 3.00 | 0 | 0 |
| png | `samplelib-png/sample-hut-400x300.png` | 8 | 10 | 10.63 | -7 | -57 |
| png | `samplelib-png/sample-indexed-400x300.png` | 1 | 3 | 3.01 | -6 | -46 |
| png | `samplelib-png/sample-interlaced-400x300.png` | 1 | 10 | 9.81 | 0 | 0 |
| png | `samplelib-png/sample-red-100x75.png` | 1 | 3 | 1.51 | 0 | 0 |
| png | `samplelib-png/sample-red-1x1.png` | 1 | 3 | 0.21 | 0 | 0 |
| png | `samplelib-png/sample-red-200x200.png` | 1 | 3 | 3.00 | 0 | 0 |
| png | `small/16-c3-8bits-4bits.png` | 1 | 3 | 3.00 | 0 | 0 |
| png | `small/FPVgbtMaIAILcRe2.png` | 1 | 3 | 3.01 | 0 | 0 |
| png | `small/GAMMA.PNG` | 1 | 3 | 3.01 | 0 | -3 |
| png | `small/ODF_empty_128x128.png` | 1 | 3 | 3.01 | -35 | -280 |
| png | `small/ODF_empty_32x32.png` | 1 | 3 | 3.01 | -1 | -12 |
| png | `small/ODF_empty_48x48.png` | 1 | 3 | 3.01 | -2 | -11 |
| png | `small/ODF_spreadsheet_128x128.png` | 1 | 3 | 3.01 | 0 | -6 |
| png | `small/ODF_spreadsheet_16x16.png` | 1 | 3 | 3.01 | 0 | -1 |
| png | `small/ODF_spreadsheet_32x32.png` | 1 | 3 | 3.01 | 0 | 0 |
| png | `small/ODF_spreadsheet_48x48.png` | 1 | 3 | 3.01 | -2 | -15 |
| png | `small/ODF_textdocument_128x128.png` | 1 | 3 | 3.01 | -19 | -152 |
| png | `small/ODF_textdocument_32x32.png` | 1 | 3 | 3.01 | -1 | -10 |
| png | `small/ODF_textdocument_48x48.png` | 1 | 3 | 3.01 | -1 | -10 |
| png | `small/T_Grass.png` | 1 | 3 | 3.00 | -1 | -3 |
| png | `small/absoluterecord.png` | 1 | 3 | 3.01 | -1 | -6 |
| png | `small/aperture.png` | 1 | 3 | 3.01 | -2 | -14 |
| png | `small/app-hearts.png` | 1 | 3 | 3.00 | -1 | -9 |
| png | `small/barchart.png` | 1 | 3 | 3.01 | -3 | -19 |
| png | `small/bars.png` | 1 | 3 | 3.00 | 0 | -2 |
| png | `small/bomb.png` | 1 | 3 | 3.01 | 0 | 0 |
| png | `small/briefcase.png` | 1 | 3 | 3.01 | -3 | -22 |
| png | `small/but_03.png` | 1 | 3 | 3.00 | -1 | -4 |
| png | `small/calendar.png` | 1 | 3 | 3.01 | -2 | -17 |
| png | `small/carwheel.png` | 1 | 3 | 3.01 | 0 | -3 |
| png | `small/caution.png` | 1 | 3 | 3.01 | -2 | -16 |
| png | `small/check.png` | 1 | 3 | 3.01 | -2 | -11 |
| png | `small/circlecompass.png` | 1 | 3 | 3.01 | -2 | -11 |
| png | `small/clapboard.png` | 1 | 3 | 3.01 | -1 | -2 |
| png | `small/cloud.png` | 1 | 3 | 3.01 | -1 | -6 |
| png | `small/cnc_06.png` | 1 | 3 | 3.01 | -3 | -17 |
| png | `small/cookie.png` | 1 | 3 | 3.01 | -2 | -20 |
| png | `small/cookie2.png` | 1 | 3 | 3.01 | 0 | 0 |
| png | `small/cookieG.png` | 1 | 3 | 3.01 | 0 | 0 |
| png | `small/cups.png` | 1 | 3 | 3.01 | -1 | -4 |
| png | `small/document.png` | 1 | 3 | 3.01 | -1 | -4 |
| png | `small/download.png` | 1 | 10 | 9.81 | -1 | -5 |
| png | `small/expat.png` | 1 | 3 | 3.01 | -1 | -3 |
| png | `small/gamecontroller.png` | 1 | 3 | 3.01 | -1 | -3 |
| png | `small/heart.png` | 1 | 3 | 3.01 | 0 | 0 |
| png | `small/keyboard.png` | 1 | 3 | 3.01 | -1 | -10 |
| png | `small/lc_flowchartshapes.flowchart-predefined-process.png` | 1 | 3 | 3.00 | -1 | -5 |
| png | `small/lch_distributerows.png` | 1 | 3 | 3.00 | -1 | -6 |
| png | `small/life3tb.png` | 1 | 3 | 3.01 | -1 | -8 |
| png | `small/lightbulb.png` | 1 | 3 | 3.01 | -1 | -14 |
| png | `small/lightfrombottom_22.png` | 1 | 3 | 3.01 | 0 | 0 |
| png | `small/map.png` | 1 | 3 | 3.01 | 0 | 0 |
| png | `small/mouse_image.png` | 1 | 3 | 3.00 | 0 | 0 |
| png | `small/mursky.png` | 1 | 3 | 3.01 | -2 | -13 |
| png | `small/news.png` | 1 | 3 | 3.01 | 0 | -2 |
| png | `small/notify3.png` | 2 | 4 | 4.01 | -2 | -18 |
| png | `small/paintbrush.png` | 1 | 3 | 3.01 | -1 | -4 |
| png | `small/panel_buttons.png` | 1 | 3 | 3.01 | -1 | -4 |
| png | `small/panel_zoom.png` | 1 | 3 | 3.00 | -1 | -1 |
| png | `small/png-corpus-test2.png` | 1 | 3 | 3.01 | 0 | 0 |
| png | `small/pngnow.png` | 1 | 3 | 3.01 | -3 | -18 |
| png | `small/polaroidcamera.png` | 2 | 4 | 4.01 | 0 | 0 |
| png | `small/present.png` | 1 | 3 | 3.01 | -1 | -10 |
| png | `small/profle.png` | 1 | 3 | 3.01 | 0 | 0 |
| png | `small/psydk-Pink.png` | 1 | 3 | 3.01 | 0 | 0 |
| png | `small/racingflags.png` | 1 | 3 | 3.01 | -1 | -11 |
| png | `small/radiotower.png` | 1 | 3 | 3.01 | 0 | -3 |
| png | `small/ribbon.png` | 1 | 3 | 3.00 | -1 | -10 |
| png | `small/schooolbus.png` | 1 | 3 | 3.01 | -3 | -28 |
| png | `small/skateboard.png` | 1 | 3 | 3.01 | -1 | -7 |
| png | `small/skin.png` | 2 | 4 | 4.01 | -2 | -20 |
| png | `small/smartphone.png` | 1 | 3 | 3.01 | 0 | 0 |
| png | `small/spaceshuttle.png` | 1 | 3 | 3.01 | -1 | -10 |
| png | `small/spinner03-32-hc_03.png` | 1 | 3 | 3.00 | -1 | -8 |
| png | `small/spinner03-32-hc_11.png` | 1 | 3 | 3.00 | 0 | 0 |
| png | `small/steeringwheel.png` | 1 | 3 | 3.01 | 0 | -3 |
| png | `small/str_do_opt3-oxi.png` | 1 | 3 | 3.01 | 0 | 0 |
| png | `small/sub.png` | 1 | 3 | 3.01 | 0 | -2 |
| png | `small/text.png` | 1 | 3 | 3.00 | -4 | -27 |
| png | `small/toolbar.png` | 1 | 3 | 3.01 | 0 | -3 |
| png | `small/umbrella.png` | 1 | 3 | 3.01 | 0 | -2 |
| png | `small/video.png` | 1 | 3 | 3.01 | 0 | 0 |
| png | `small/videocameracompact.png` | 1 | 3 | 3.01 | 0 | -3 |
| zip | `8x8-zip/Architecture.playdate-pulp.zip` | 11 | 13 | 13.00 | -21 | -183 |
| zip | `8x8-zip/Checked.playdate-pulp.zip` | 8 | 10 | 9.66 | -26 | -214 |
| zip | `8x8-zip/Dashes.playdate-pulp.zip` | 4 | 6 | 6.00 | -13 | -102 |
| zip | `8x8-zip/Dither.playdate-pulp.zip` | 8 | 10 | 9.67 | -10 | -90 |
| zip | `8x8-zip/Dots.playdate-pulp.zip` | 3 | 5 | 5.00 | -17 | -133 |
| zip | `8x8-zip/Grid.playdate-pulp.zip` | 5 | 7 | 7.00 | -9 | -67 |
| zip | `8x8-zip/Lines.playdate-pulp.zip` | 17 | 19 | 19.00 | -87 | -712 |
| zip | `8x8-zip/Nature.playdate-pulp.zip` | 9 | 11 | 11.00 | -35 | -250 |
| zip | `8x8-zip/Radial.playdate-pulp.zip` | 5 | 7 | 7.00 | -20 | -165 |
| zip | `8x8-zip/Rectilinear.playdate-pulp.zip` | 11 | 13 | 13.00 | -22 | -186 |
| zip | `8x8-zip/Round.playdate-pulp.zip` | 9 | 11 | 11.00 | -17 | -140 |
| zip | `8x8-zip/Symbols.playdate-pulp.zip` | 6 | 8 | 8.00 | -14 | -141 |
| zip | `8x8-zip/Waves.playdate-pulp.zip` | 9 | 11 | 11.00 | -21 | -214 |
| zip | `8x8-zip/Woven.playdate-pulp.zip` | 5 | 7 | 7.00 | -17 | -138 |
| zip | `kensilverman-zip/blank.zip` | 1 | 3 | 3.00 | 0 | 0 |
| zip | `kensilverman-zip/checkers_src.zip` | 1 | 3 | 3.00 | -2 | -16 |
| zip | `kensilverman-zip/findword.zip` | 8 | 10 | 10.09 | -33 | -266 |
| zip | `kensilverman-zip/floppy.zip` | 1 | 3 | 0.00 | 0 | 0 |
| zip | `kensilverman-zip/kpc.zip` | 2 | 4 | 4.02 | -3 | -22 |
| zip | `kensilverman-zip/kpic_src.zip` | 3 | 5 | 5.01 | -4 | -28 |
| zip | `kensilverman-zip/ks2.zip` | 7 | 9 | 9.02 | -7 | -55 |
| zip | `kensilverman-zip/kwincheat.zip` | 3 | 5 | 5.01 | -9 | -82 |
| zip | `kensilverman-zip/kwincheat_src.zip` | 1 | 10 | 10.01 | -1 | -11 |
| zip | `kensilverman-zip/nv.zip` | 1 | 3 | 0.00 | 0 | 0 |
| zip | `kensilverman-zip/rekzip.zip` | 1 | 3 | 3.00 | -1 | -6 |
| zip | `kensilverman-zip/suckthumbs.zip` | 1 | 3 | 3.01 | 0 | -2 |
| zip | `kensilverman-zip/textrend.zip` | 2 | 4 | 4.00 | -1 | -2 |
| zip | `kensilverman-zip/ves2.zip` | 4 | 6 | 6.01 | -6 | -48 |
| zip | `medium-zip/samples.zip` | 7 | 9 | 9.00 | -33 | -261 |
| zip | `medium-zip/skinsrc.zip` | 4 | 10 | 10.10 | -31 | -250 |
| zip | `samplelib-zip/sample-10mb.zip` | 1 | 3 | 2.52 | 0 | 0 |
| zip | `samplelib-zip/sample-1mb.zip` | 1 | 3 | 2.51 | -2 | -16 |
| zip | `samplelib-zip/sample-50mb.zip` | 1 | 3 | 3.38 | 0 | 0 |
| zip | `samplelib-zip/sample-many-files.zip` | 4 | 10 | 10.01 | 0 | 0 |
| zip | `samplelib-zip/sample-project.zip` | 1 | 3 | 3.00 | -3 | -25 |
| zip | `samplelib-zip/sample-simple.zip` | 1 | 3 | 3.00 | -1 | -6 |
| zip | `samplelib-zip/sample-with-html.zip` | 1 | 3 | 3.01 | -1 | -3 |
| zip | `small-zip/Alleyway (EMU).zophar.zip` | 2 | 4 | 4.00 | -8 | -72 |
| zip | `small-zip/SIEGE_AB_SOURCE_V1_1.zip` | 1 | 3 | 3.01 | -16 | -130 |
| zip | `small-zip/WindowsBatchFileMarkup.tmbundle.zip` | 1 | 3 | 3.00 | 0 | 0 |
| zip | `small-zip/gbcard6.zip` | 1 | 3 | 3.01 | -59 | -477 |
| zip | `small-zip/kskinmkr_src.zip` | 2 | 4 | 4.01 | -79 | -637 |
| zip | `small-zip/memory-viewer.1.05.zip` | 1 | 3 | 3.00 | -2 | -15 |
| zip | `small-zip/missing_directory.zip` | 1 | 3 | 0.00 | 0 | 0 |
| zip | `small-zip/nested_portion1.zip` | 1 | 3 | 0.00 | 0 | 0 |
| zip | `small-zip/schematic-symbol-and-pcb-footprint.zip` | 1 | 10 | 10.01 | 0 | -2 |
| zip | `small-zip/standard.sop.zip` | 2 | 4 | 4.00 | -1 | -4 |
| zip | `small-zip/testmake.zip` | 1 | 3 | 3.00 | -2 | -18 |
| zip | `small-zip/top_level_portion1.zip` | 1 | 3 | 0.00 | 0 | 0 |
| zip | `small-zip/wp-hide-dashboard.2.2.zip` | 1 | 3 | 3.00 | -2 | -10 |
| zip | `small-zip/zip_cp437_header.zip` | 1 | 3 | 2.06 | 0 | 0 |
| zip | `small-zip/zipdir.zip` | 1 | 3 | 0.00 | 0 | 0 |
| zip | `small-zip/ziptestdata1.zip` | 1 | 3 | 0.00 | 0 | 0 |
| zip | `small-zip/ziptestdata2.zip` | 1 | 3 | 0.00 | 0 | 0 |
| zlib | `oxipng-zlib/XYB.icc.zlib` | 1 | 10 | 10.00 | 0 | 0 |
