# DeflOpt Benchmark

This living report compares Columbo against paired `deflopt`
references. Each case records file byte deltas and Deflate bitstream
bit deltas. Negative numbers mean Columbo is smaller.

The two standard modes are plain default Columbo and `--max` with
a timeout equal to the measured default runtime rounded up plus five
seconds, subject to Columbo's 10-second minimum. The runner completes
the corpus-wide default pass before starting timed max rows. ZIP
comparisons always pass `--strip` so their wrapper
metadata policy matches DeflOpt; other formats preserve metadata.
Known defective PNG fixtures and unsupported ancient ZIP fixtures are
excluded by the same corpus rules as the regular smoke tests.

- completed rows: 24
- misses: 0
- unresolved misses: 0
- errors: 0
- previous-result regressions over 5%: 0
- rows with both default and default-time-plus-5 max results: 12
- max rows worse than default: 0
- rows with recorded Columbo binary hash: 1914
- rows from current Columbo binary: 24
- rows from older Columbo binaries: 1890

## Summary

| format | mode | rows | misses | same | better | errors | byte misses | bit misses |
| --- | --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| png | default | 12 | 0 | 1 | 11 | 0 | 0 | 0 |
| png | max+5s | 12 | 0 | 1 | 11 | 0 | 0 | 0 |

## Max vs Default

The max trial uses the measured default runtime, rounded up to a whole
second, plus five seconds, with the current 10-second minimum. It
should never return a larger file or a longer Deflate bitstream than
default Columbo for the same source.
Comparisons include only rows from the same Columbo binary.

| format | compared rows | max worse | byte worse | bit worse |
| --- | ---: | ---: | ---: | ---: |
| png | 12 | 0 | 0 | 0 |

## Rows

| format | mode | file | timeout | Columbo seconds | bytes vs deflopt | bits vs deflopt |
| --- | --- | --- | ---: | ---: | ---: | ---: |
| png | default | `css-ig-net/24-chunks.png` | — | 0.01 | 0 | 0 |
| png | max+5s | `css-ig-net/24-chunks.png` | 10 | 0.03 | 0 | 0 |
| png | default | `css-ig-net/Apple512.png` | — | 3.99 | -6797 | -54370 |
| png | max+5s | `css-ig-net/Apple512.png` | 10 | 9.99 | -9999 | -79991 |
| png | default | `css-ig-net/Apricot512.png` | — | 4.33 | -987 | -7902 |
| png | max+5s | `css-ig-net/Apricot512.png` | 10 | 9.98 | -3426 | -27412 |
| png | default | `css-ig-net/Kiwi512.png` | — | 6.34 | -2807 | -22453 |
| png | max+5s | `css-ig-net/Kiwi512.png` | 12 | 12.10 | -5223 | -41783 |
| png | default | `css-ig-net/Mango512.png` | — | 3.79 | -9017 | -72141 |
| png | max+5s | `css-ig-net/Mango512.png` | 10 | 9.93 | -10873 | -86984 |
| png | default | `css-ig-net/Orange512.png` | — | 4.51 | -543 | -4348 |
| png | max+5s | `css-ig-net/Orange512.png` | 10 | 10.00 | -3289 | -26317 |
| png | default | `css-ig-net/Pear512.png` | — | 4.56 | -3377 | -27013 |
| png | max+5s | `css-ig-net/Pear512.png` | 10 | 10.01 | -6801 | -54411 |
| png | default | `css-ig-net/Strawberry512.png` | — | 6.39 | -6087 | -48698 |
| png | max+5s | `css-ig-net/Strawberry512.png` | 12 | 12.05 | -12975 | -103802 |
| png | default | `large/nerd.png` | — | 4.41 | -16221 | -129767 |
| png | max+5s | `large/nerd.png` | 10 | 9.92 | -31125 | -248996 |
| png | default | `medium/download_webp__260×280_.png` | — | 3.39 | -7353 | -58825 |
| png | max+5s | `medium/download_webp__260×280_.png` | 10 | 9.92 | -21149 | -169190 |
| png | default | `medium/floor pattern.png` | — | 10.96 | -24919 | -199345 |
| png | max+5s | `medium/floor pattern.png` | 16 | 16.42 | -131201 | -1049605 |
| png | default | `oxipng/grayscale_alpha_8_should_be_grayscale_trns_8.png` | — | 3.91 | -463 | -3703 |
| png | max+5s | `oxipng/grayscale_alpha_8_should_be_grayscale_trns_8.png` | 10 | 9.86 | -2311 | -18488 |
