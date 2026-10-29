# Defluff Relaxed Benchmark

This report compares Columbo's Default mode with `--strict 0` against
paired Defluff PNG outputs. Defluff emits empty or singleton Huffman
alphabets, which Columbo allows only with `--strict 0`, so the
comparison is like-for-like. A case passes only when Columbo is equal
or smaller in both complete-file bytes and meaningful Deflate bits.

- discovered source/reference pairs: 66
- completed rows for this binary: 66
- missing rows: 0
- rows from older Columbo binaries: 0
- parity misses: 0
- prior-result regressions over 10%: 0
- errors: 0
- equal results: 4
- strictly better results: 62
- net bytes versus Defluff: -109
- net Deflate bits versus Defluff: -938
- corpus-key SHA-256: `a2dcba76eac95f4139e964066313339f2aa8eb047ce9fe893bd642850b93a0c7`
- Columbo SHA-256: `2b08abe413074ccd2cf92ef8e8adf20bb66dec77d3a47a3654a3afa067b5c691`

## Corpus Summary

| family | rows | misses | equal | better | errors | net bytes | net bits |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| pkmn-bw-hard | 18 | 0 | 4 | 14 | 0 | -17 | -145 |
| pkmn-col-hard | 48 | 0 | 0 | 48 | 0 | -92 | -793 |

## All Results

| source | seconds | Columbo bytes | Defluff bytes | byte delta | Columbo bits | Defluff bits | bit delta |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| `pkmn-bw-hard/000-Logo-5.png` | 0.276 | 436 | 436 | +0 | 864 | 864 | +0 |
| `pkmn-bw-hard/000-Logo-6.png` | 0.021 | 412 | 413 | -1 | 671 | 679 | -8 |
| `pkmn-bw-hard/008-Wartortle-1.png` | 0.027 | 485 | 485 | +0 | 1172 | 1173 | -1 |
| `pkmn-bw-hard/014-Kakuna-1.png` | 0.020 | 450 | 454 | -4 | 920 | 947 | -27 |
| `pkmn-bw-hard/023-Ekans-2.png` | 0.018 | 461 | 464 | -3 | 1012 | 1038 | -26 |
| `pkmn-bw-hard/047-Parasect.png` | 0.019 | 488 | 488 | +0 | 1978 | 1984 | -6 |
| `pkmn-bw-hard/052-Meowth-0.png` | 0.019 | 472 | 472 | +0 | 1090 | 1095 | -5 |
| `pkmn-bw-hard/054-Psyduck-0.png` | 0.020 | 457 | 460 | -3 | 968 | 991 | -23 |
| `pkmn-bw-hard/054-Psyduck-1.png` | 0.020 | 457 | 457 | +0 | 963 | 963 | +0 |
| `pkmn-bw-hard/060-Poliwag-2.png` | 0.014 | 468 | 469 | -1 | 1056 | 1063 | -7 |
| `pkmn-bw-hard/061-Poliwhirl.png` | 0.021 | 505 | 506 | -1 | 2256 | 2263 | -7 |
| `pkmn-bw-hard/084-Doduo-1.png` | 0.021 | 455 | 455 | +0 | 965 | 965 | +0 |
| `pkmn-bw-hard/090-Shellder.png` | 0.030 | 476 | 477 | -1 | 1888 | 1891 | -3 |
| `pkmn-bw-hard/113-Chansey-0.png` | 0.017 | 456 | 457 | -1 | 959 | 968 | -9 |
| `pkmn-bw-hard/113-Chansey.png` | 0.023 | 456 | 458 | -2 | 1873 | 1891 | -18 |
| `pkmn-bw-hard/114-Tangela-1.png` | 0.017 | 480 | 480 | +0 | 1148 | 1151 | -3 |
| `pkmn-bw-hard/122-Mr. Mime-0.png` | 0.019 | 471 | 471 | +0 | 1070 | 1072 | -2 |
| `pkmn-bw-hard/139-Omastar-1.png` | 0.023 | 485 | 485 | +0 | 1188 | 1188 | +0 |
| `pkmn-col-hard/002-Ivysaur-1.png` | 0.024 | 509 | 510 | -1 | 2100 | 2111 | -11 |
| `pkmn-col-hard/003-Venusaur-1.png` | 0.046 | 514 | 515 | -1 | 2131 | 2141 | -10 |
| `pkmn-col-hard/005-Charmeleon-2.png` | 0.030 | 488 | 490 | -2 | 1907 | 1927 | -20 |
| `pkmn-col-hard/006-Charizard-2.png` | 0.027 | 507 | 508 | -1 | 2068 | 2079 | -11 |
| `pkmn-col-hard/015-Beedrill-0.png` | 0.033 | 491 | 493 | -2 | 1951 | 1967 | -16 |
| `pkmn-col-hard/017-Pidgeotto-1.png` | 0.036 | 473 | 475 | -2 | 1798 | 1816 | -18 |
| `pkmn-col-hard/017-Pidgeotto-2.png` | 0.035 | 483 | 486 | -3 | 1877 | 1903 | -26 |
| `pkmn-col-hard/024-Arbok-1.png` | 0.024 | 469 | 470 | -1 | 1795 | 1806 | -11 |
| `pkmn-col-hard/030-Nidorina-1.png` | 0.022 | 493 | 496 | -3 | 1967 | 1987 | -20 |
| `pkmn-col-hard/032-Nidoran-M-2.png` | 0.034 | 490 | 492 | -2 | 1889 | 1911 | -22 |
| `pkmn-col-hard/035-Clefairy-1.png` | 0.029 | 468 | 469 | -1 | 1764 | 1776 | -12 |
| `pkmn-col-hard/043-Oddish-0.png` | 0.029 | 474 | 475 | -1 | 1826 | 1840 | -14 |
| `pkmn-col-hard/048-Venonat-1.png` | 0.027 | 498 | 499 | -1 | 2010 | 2019 | -9 |
| `pkmn-col-hard/048-Venonat-2.png` | 0.031 | 481 | 483 | -2 | 1876 | 1892 | -16 |
| `pkmn-col-hard/052-Meowth-1.png` | 0.028 | 497 | 498 | -1 | 2012 | 2023 | -11 |
| `pkmn-col-hard/052-Meowth-2.png` | 0.028 | 503 | 504 | -1 | 2057 | 2068 | -11 |
| `pkmn-col-hard/055-Golduck-0.png` | 0.041 | 468 | 471 | -3 | 1769 | 1796 | -27 |
| `pkmn-col-hard/058-Growlithe-1.png` | 0.036 | 480 | 482 | -2 | 1850 | 1868 | -18 |
| `pkmn-col-hard/062-Poliwrath-0.png` | 0.032 | 489 | 490 | -1 | 1921 | 1933 | -12 |
| `pkmn-col-hard/063-Abra-2.png` | 0.036 | 474 | 476 | -2 | 1845 | 1864 | -19 |
| `pkmn-col-hard/073-Tentacruel-0.png` | 0.029 | 501 | 502 | -1 | 2012 | 2023 | -11 |
| `pkmn-col-hard/076-Golem-1.png` | 0.028 | 479 | 482 | -3 | 1878 | 1898 | -20 |
| `pkmn-col-hard/077-Ponyta-2.png` | 0.028 | 483 | 485 | -2 | 1897 | 1914 | -17 |
| `pkmn-col-hard/079-Slowpoke-1.png` | 0.030 | 443 | 445 | -2 | 1562 | 1584 | -22 |
| `pkmn-col-hard/080-Slowbro-1.png` | 0.029 | 486 | 489 | -3 | 1920 | 1941 | -21 |
| `pkmn-col-hard/086-Seel-0.png` | 0.026 | 467 | 470 | -3 | 1792 | 1815 | -23 |
| `pkmn-col-hard/086-Seel-1.png` | 0.037 | 466 | 467 | -1 | 1778 | 1790 | -12 |
| `pkmn-col-hard/096-Drowzee-0.png` | 0.030 | 465 | 468 | -3 | 1752 | 1769 | -17 |
| `pkmn-col-hard/096-Drowzee-1.png` | 0.031 | 462 | 463 | -1 | 1724 | 1731 | -7 |
| `pkmn-col-hard/099-Kingler-0.png` | 0.036 | 464 | 467 | -3 | 1742 | 1761 | -19 |
| `pkmn-col-hard/107-Hitmonchan-2.png` | 0.033 | 447 | 449 | -2 | 1584 | 1599 | -15 |
| `pkmn-col-hard/115-Kangaskhan-2.png` | 0.027 | 490 | 491 | -1 | 1926 | 1935 | -9 |
| `pkmn-col-hard/116-Horsea-1.png` | 0.025 | 456 | 459 | -3 | 1687 | 1712 | -25 |
| `pkmn-col-hard/118-Goldeen-0.png` | 0.029 | 485 | 487 | -2 | 1911 | 1928 | -17 |
| `pkmn-col-hard/122-Mr. Mime-1.png` | 0.026 | 489 | 493 | -4 | 1934 | 1967 | -33 |
| `pkmn-col-hard/122-Mr. Mime-2.png` | 0.032 | 523 | 525 | -2 | 2206 | 2224 | -18 |
| `pkmn-col-hard/123-Scyther-0.png` | 0.034 | 502 | 503 | -1 | 2043 | 2053 | -10 |
| `pkmn-col-hard/126-Magmar-2.png` | 0.030 | 516 | 517 | -1 | 2164 | 2172 | -8 |
| `pkmn-col-hard/131-Lapras-2.png` | 0.030 | 482 | 485 | -3 | 1896 | 1919 | -23 |
| `pkmn-col-hard/133-Eevee-1.png` | 0.038 | 465 | 468 | -3 | 1765 | 1785 | -20 |
| `pkmn-col-hard/135-Jolteon-1.png` | 0.035 | 492 | 495 | -3 | 1967 | 1985 | -18 |
| `pkmn-col-hard/135-Jolteon-2.png` | 0.041 | 517 | 520 | -3 | 2168 | 2190 | -22 |
| `pkmn-col-hard/137-Porygon-1.png` | 0.030 | 463 | 464 | -1 | 1729 | 1743 | -14 |
| `pkmn-col-hard/137-Porygon-2.png` | 0.035 | 527 | 530 | -3 | 2248 | 2270 | -22 |
| `pkmn-col-hard/141-Kabutops-0.png` | 0.030 | 471 | 473 | -2 | 1785 | 1805 | -20 |
| `pkmn-col-hard/142-Aerodactyl-2.png` | 0.035 | 511 | 512 | -1 | 2091 | 2104 | -13 |
| `pkmn-col-hard/146-Moltres-1.png` | 0.035 | 497 | 497 | +0 | 2003 | 2007 | -4 |
| `pkmn-col-hard/151-Mew-1.png` | 0.027 | 465 | 467 | -2 | 1778 | 1797 | -19 |
