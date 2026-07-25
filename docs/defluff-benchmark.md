# Defluff Default Benchmark

This report compares normal/default Columbo with paired Defluff PNG
outputs. A case passes only when Columbo is equal or smaller in both
complete-file bytes and meaningful Deflate-stream bits.

- discovered source/reference pairs: 66
- completed rows for this binary: 66
- missing rows: 0
- rows from older Columbo binaries: 0
- parity misses: 0
- errors: 0
- equal results: 31
- strictly better results: 35
- net bytes versus Defluff: -16
- net Deflate bits versus Defluff: -185
- corpus-key SHA-256: `8645165316ae2a5d4205fb716b36b6c4b1e40e821207d1d0684a392e5814b8c5`
- Columbo SHA-256: `e9f361f016c4d8df6e4c43415cdcc26aef35144d8b829253e3164118ef9510dd`

## Corpus Summary

| family | rows | misses | equal | better | errors | net bytes | net bits |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| pkmn-bw-hard | 18 | 0 | 11 | 7 | 0 | -4 | -38 |
| pkmn-col-hard | 48 | 0 | 20 | 28 | 0 | -12 | -147 |

## All Results

| source | seconds | Columbo bytes | Defluff bytes | byte delta | Columbo bits | Defluff bits | bit delta |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| `pkmn-bw-hard/000-Logo-5.png` | 0.517 | 436 | 436 | +0 | 864 | 864 | +0 |
| `pkmn-bw-hard/000-Logo-6.png` | 0.022 | 412 | 413 | -1 | 671 | 679 | -8 |
| `pkmn-bw-hard/008-Wartortle-1.png` | 0.032 | 485 | 485 | +0 | 1173 | 1173 | +0 |
| `pkmn-bw-hard/014-Kakuna-1.png` | 0.017 | 453 | 454 | -1 | 941 | 947 | -6 |
| `pkmn-bw-hard/023-Ekans-2.png` | 0.017 | 464 | 464 | +0 | 1037 | 1038 | -1 |
| `pkmn-bw-hard/047-Parasect.png` | 0.030 | 488 | 488 | +0 | 1984 | 1984 | +0 |
| `pkmn-bw-hard/052-Meowth-0.png` | 0.047 | 472 | 472 | +0 | 1095 | 1095 | +0 |
| `pkmn-bw-hard/054-Psyduck-0.png` | 0.043 | 459 | 460 | -1 | 980 | 991 | -11 |
| `pkmn-bw-hard/054-Psyduck-1.png` | 0.042 | 457 | 457 | +0 | 963 | 963 | +0 |
| `pkmn-bw-hard/060-Poliwag-2.png` | 0.016 | 468 | 469 | -1 | 1056 | 1063 | -7 |
| `pkmn-bw-hard/061-Poliwhirl.png` | 0.025 | 506 | 506 | +0 | 2263 | 2263 | +0 |
| `pkmn-bw-hard/084-Doduo-1.png` | 0.074 | 455 | 455 | +0 | 965 | 965 | +0 |
| `pkmn-bw-hard/090-Shellder.png` | 0.025 | 477 | 477 | +0 | 1891 | 1891 | +0 |
| `pkmn-bw-hard/113-Chansey-0.png` | 0.023 | 457 | 457 | +0 | 965 | 968 | -3 |
| `pkmn-bw-hard/113-Chansey.png` | 0.068 | 458 | 458 | +0 | 1891 | 1891 | +0 |
| `pkmn-bw-hard/114-Tangela-1.png` | 0.024 | 480 | 480 | +0 | 1149 | 1151 | -2 |
| `pkmn-bw-hard/122-Mr. Mime-0.png` | 0.043 | 471 | 471 | +0 | 1072 | 1072 | +0 |
| `pkmn-bw-hard/139-Omastar-1.png` | 0.024 | 485 | 485 | +0 | 1188 | 1188 | +0 |
| `pkmn-col-hard/002-Ivysaur-1.png` | 0.041 | 509 | 510 | -1 | 2104 | 2111 | -7 |
| `pkmn-col-hard/003-Venusaur-1.png` | 0.031 | 515 | 515 | +0 | 2138 | 2141 | -3 |
| `pkmn-col-hard/005-Charmeleon-2.png` | 0.030 | 489 | 490 | -1 | 1920 | 1927 | -7 |
| `pkmn-col-hard/006-Charizard-2.png` | 0.030 | 508 | 508 | +0 | 2075 | 2079 | -4 |
| `pkmn-col-hard/015-Beedrill-0.png` | 0.037 | 493 | 493 | +0 | 1967 | 1967 | +0 |
| `pkmn-col-hard/017-Pidgeotto-1.png` | 0.038 | 475 | 475 | +0 | 1811 | 1816 | -5 |
| `pkmn-col-hard/017-Pidgeotto-2.png` | 0.081 | 485 | 486 | -1 | 1896 | 1903 | -7 |
| `pkmn-col-hard/024-Arbok-1.png` | 0.029 | 470 | 470 | +0 | 1803 | 1806 | -3 |
| `pkmn-col-hard/030-Nidorina-1.png` | 0.039 | 495 | 496 | -1 | 1984 | 1987 | -3 |
| `pkmn-col-hard/032-Nidoran-M-2.png` | 0.057 | 492 | 492 | +0 | 1911 | 1911 | +0 |
| `pkmn-col-hard/035-Clefairy-1.png` | 0.033 | 469 | 469 | +0 | 1772 | 1776 | -4 |
| `pkmn-col-hard/043-Oddish-0.png` | 0.041 | 475 | 475 | +0 | 1840 | 1840 | +0 |
| `pkmn-col-hard/048-Venonat-1.png` | 0.061 | 499 | 499 | +0 | 2019 | 2019 | +0 |
| `pkmn-col-hard/048-Venonat-2.png` | 0.039 | 483 | 483 | +0 | 1892 | 1892 | +0 |
| `pkmn-col-hard/052-Meowth-1.png` | 0.031 | 498 | 498 | +0 | 2019 | 2023 | -4 |
| `pkmn-col-hard/052-Meowth-2.png` | 0.023 | 504 | 504 | +0 | 2068 | 2068 | +0 |
| `pkmn-col-hard/055-Golduck-0.png` | 0.036 | 471 | 471 | +0 | 1796 | 1796 | +0 |
| `pkmn-col-hard/058-Growlithe-1.png` | 0.037 | 482 | 482 | +0 | 1868 | 1868 | +0 |
| `pkmn-col-hard/062-Poliwrath-0.png` | 0.033 | 490 | 490 | +0 | 1933 | 1933 | +0 |
| `pkmn-col-hard/063-Abra-2.png` | 0.056 | 475 | 476 | -1 | 1849 | 1864 | -15 |
| `pkmn-col-hard/073-Tentacruel-0.png` | 0.032 | 502 | 502 | +0 | 2023 | 2023 | +0 |
| `pkmn-col-hard/076-Golem-1.png` | 0.031 | 482 | 482 | +0 | 1897 | 1898 | -1 |
| `pkmn-col-hard/077-Ponyta-2.png` | 0.026 | 485 | 485 | +0 | 1913 | 1914 | -1 |
| `pkmn-col-hard/079-Slowpoke-1.png` | 0.043 | 444 | 445 | -1 | 1571 | 1584 | -13 |
| `pkmn-col-hard/080-Slowbro-1.png` | 0.032 | 489 | 489 | +0 | 1938 | 1941 | -3 |
| `pkmn-col-hard/086-Seel-0.png` | 0.041 | 470 | 470 | +0 | 1810 | 1815 | -5 |
| `pkmn-col-hard/086-Seel-1.png` | 0.033 | 467 | 467 | +0 | 1790 | 1790 | +0 |
| `pkmn-col-hard/096-Drowzee-0.png` | 0.059 | 467 | 468 | -1 | 1764 | 1769 | -5 |
| `pkmn-col-hard/096-Drowzee-1.png` | 0.044 | 463 | 463 | +0 | 1731 | 1731 | +0 |
| `pkmn-col-hard/099-Kingler-0.png` | 0.071 | 466 | 467 | -1 | 1758 | 1761 | -3 |
| `pkmn-col-hard/107-Hitmonchan-2.png` | 0.067 | 448 | 449 | -1 | 1591 | 1599 | -8 |
| `pkmn-col-hard/115-Kangaskhan-2.png` | 0.027 | 490 | 491 | -1 | 1928 | 1935 | -7 |
| `pkmn-col-hard/116-Horsea-1.png` | 0.034 | 458 | 459 | -1 | 1703 | 1712 | -9 |
| `pkmn-col-hard/118-Goldeen-0.png` | 0.033 | 487 | 487 | +0 | 1925 | 1928 | -3 |
| `pkmn-col-hard/122-Mr. Mime-1.png` | 0.024 | 492 | 493 | -1 | 1957 | 1967 | -10 |
| `pkmn-col-hard/122-Mr. Mime-2.png` | 0.032 | 525 | 525 | +0 | 2222 | 2224 | -2 |
| `pkmn-col-hard/123-Scyther-0.png` | 0.035 | 503 | 503 | +0 | 2053 | 2053 | +0 |
| `pkmn-col-hard/126-Magmar-2.png` | 0.031 | 517 | 517 | +0 | 2172 | 2172 | +0 |
| `pkmn-col-hard/131-Lapras-2.png` | 0.025 | 485 | 485 | +0 | 1913 | 1919 | -6 |
| `pkmn-col-hard/133-Eevee-1.png` | 0.072 | 468 | 468 | +0 | 1785 | 1785 | +0 |
| `pkmn-col-hard/135-Jolteon-1.png` | 0.049 | 495 | 495 | +0 | 1985 | 1985 | +0 |
| `pkmn-col-hard/135-Jolteon-2.png` | 0.069 | 520 | 520 | +0 | 2188 | 2190 | -2 |
| `pkmn-col-hard/137-Porygon-1.png` | 0.057 | 464 | 464 | +0 | 1743 | 1743 | +0 |
| `pkmn-col-hard/137-Porygon-2.png` | 0.061 | 530 | 530 | +0 | 2265 | 2270 | -5 |
| `pkmn-col-hard/141-Kabutops-0.png` | 0.049 | 473 | 473 | +0 | 1805 | 1805 | +0 |
| `pkmn-col-hard/142-Aerodactyl-2.png` | 0.035 | 512 | 512 | +0 | 2104 | 2104 | +0 |
| `pkmn-col-hard/146-Moltres-1.png` | 0.036 | 497 | 497 | +0 | 2007 | 2007 | +0 |
| `pkmn-col-hard/151-Mew-1.png` | 0.032 | 467 | 467 | +0 | 1795 | 1797 | -2 |
