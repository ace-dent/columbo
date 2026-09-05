# Defluff Relaxed Benchmark

This report compares normal Columbo with `--strict 0` against paired
Defluff PNG outputs. Defluff uses compatibility-sensitive Huffman
alphabet forms enabled by Columbo's relaxed mode, making this a
like-for-like comparison. A case passes only when Columbo is equal or
smaller in both complete-file bytes and meaningful Deflate-stream bits.

- discovered source/reference pairs: 66
- completed rows for this binary: 66
- missing rows: 0
- rows from older Columbo binaries: 0
- parity misses: 0
- prior-result regressions over 10%: 0
- errors: 0
- equal results: 8
- strictly better results: 58
- net bytes versus Defluff: -102
- net Deflate bits versus Defluff: -852
- corpus-key SHA-256: `a2dcba76eac95f4139e964066313339f2aa8eb047ce9fe893bd642850b93a0c7`
- Columbo SHA-256: `cef3db1d92471ddfff1dc9b2ddd6661c7e2c37955deeb64daadb9bcf0289a79a`

## Corpus Summary

| family | rows | misses | equal | better | errors | net bytes | net bits |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| pkmn-bw-hard | 18 | 0 | 8 | 10 | 0 | -15 | -128 |
| pkmn-col-hard | 48 | 0 | 0 | 48 | 0 | -87 | -724 |

## All Results

| source | seconds | Columbo bytes | Defluff bytes | byte delta | Columbo bits | Defluff bits | bit delta |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| `pkmn-bw-hard/000-Logo-5.png` | 0.014 | 436 | 436 | +0 | 864 | 864 | +0 |
| `pkmn-bw-hard/000-Logo-6.png` | 0.012 | 412 | 413 | -1 | 671 | 679 | -8 |
| `pkmn-bw-hard/008-Wartortle-1.png` | 0.030 | 485 | 485 | +0 | 1173 | 1173 | +0 |
| `pkmn-bw-hard/014-Kakuna-1.png` | 0.018 | 450 | 454 | -4 | 920 | 947 | -27 |
| `pkmn-bw-hard/023-Ekans-2.png` | 0.016 | 461 | 464 | -3 | 1012 | 1038 | -26 |
| `pkmn-bw-hard/047-Parasect.png` | 0.033 | 488 | 488 | +0 | 1984 | 1984 | +0 |
| `pkmn-bw-hard/052-Meowth-0.png` | 0.048 | 472 | 472 | +0 | 1095 | 1095 | +0 |
| `pkmn-bw-hard/054-Psyduck-0.png` | 0.062 | 457 | 460 | -3 | 968 | 991 | -23 |
| `pkmn-bw-hard/054-Psyduck-1.png` | 0.058 | 457 | 457 | +0 | 963 | 963 | +0 |
| `pkmn-bw-hard/060-Poliwag-2.png` | 0.010 | 468 | 469 | -1 | 1056 | 1063 | -7 |
| `pkmn-bw-hard/061-Poliwhirl.png` | 0.043 | 506 | 506 | +0 | 2257 | 2263 | -6 |
| `pkmn-bw-hard/084-Doduo-1.png` | 0.044 | 455 | 455 | +0 | 965 | 965 | +0 |
| `pkmn-bw-hard/090-Shellder.png` | 0.033 | 477 | 477 | +0 | 1889 | 1891 | -2 |
| `pkmn-bw-hard/113-Chansey-0.png` | 0.031 | 456 | 457 | -1 | 959 | 968 | -9 |
| `pkmn-bw-hard/113-Chansey.png` | 0.089 | 456 | 458 | -2 | 1873 | 1891 | -18 |
| `pkmn-bw-hard/114-Tangela-1.png` | 0.025 | 480 | 480 | +0 | 1149 | 1151 | -2 |
| `pkmn-bw-hard/122-Mr. Mime-0.png` | 0.046 | 471 | 471 | +0 | 1072 | 1072 | +0 |
| `pkmn-bw-hard/139-Omastar-1.png` | 0.035 | 485 | 485 | +0 | 1188 | 1188 | +0 |
| `pkmn-col-hard/002-Ivysaur-1.png` | 0.052 | 509 | 510 | -1 | 2104 | 2111 | -7 |
| `pkmn-col-hard/003-Venusaur-1.png` | 0.157 | 514 | 515 | -1 | 2131 | 2141 | -10 |
| `pkmn-col-hard/005-Charmeleon-2.png` | 0.173 | 488 | 490 | -2 | 1911 | 1927 | -16 |
| `pkmn-col-hard/006-Charizard-2.png` | 0.099 | 507 | 508 | -1 | 2068 | 2079 | -11 |
| `pkmn-col-hard/015-Beedrill-0.png` | 0.139 | 492 | 493 | -1 | 1954 | 1967 | -13 |
| `pkmn-col-hard/017-Pidgeotto-1.png` | 0.132 | 473 | 475 | -2 | 1798 | 1816 | -18 |
| `pkmn-col-hard/017-Pidgeotto-2.png` | 0.186 | 483 | 486 | -3 | 1877 | 1903 | -26 |
| `pkmn-col-hard/024-Arbok-1.png` | 0.130 | 469 | 470 | -1 | 1795 | 1806 | -11 |
| `pkmn-col-hard/030-Nidorina-1.png` | 0.157 | 493 | 496 | -3 | 1967 | 1987 | -20 |
| `pkmn-col-hard/032-Nidoran-M-2.png` | 0.263 | 490 | 492 | -2 | 1889 | 1911 | -22 |
| `pkmn-col-hard/035-Clefairy-1.png` | 0.116 | 468 | 469 | -1 | 1764 | 1776 | -12 |
| `pkmn-col-hard/043-Oddish-0.png` | 0.125 | 474 | 475 | -1 | 1826 | 1840 | -14 |
| `pkmn-col-hard/048-Venonat-1.png` | 0.214 | 498 | 499 | -1 | 2013 | 2019 | -6 |
| `pkmn-col-hard/048-Venonat-2.png` | 0.116 | 481 | 483 | -2 | 1876 | 1892 | -16 |
| `pkmn-col-hard/052-Meowth-1.png` | 0.165 | 497 | 498 | -1 | 2012 | 2023 | -11 |
| `pkmn-col-hard/052-Meowth-2.png` | 0.098 | 503 | 504 | -1 | 2057 | 2068 | -11 |
| `pkmn-col-hard/055-Golduck-0.png` | 0.111 | 469 | 471 | -2 | 1777 | 1796 | -19 |
| `pkmn-col-hard/058-Growlithe-1.png` | 0.209 | 480 | 482 | -2 | 1856 | 1868 | -12 |
| `pkmn-col-hard/062-Poliwrath-0.png` | 0.116 | 489 | 490 | -1 | 1921 | 1933 | -12 |
| `pkmn-col-hard/063-Abra-2.png` | 0.213 | 474 | 476 | -2 | 1847 | 1864 | -17 |
| `pkmn-col-hard/073-Tentacruel-0.png` | 0.177 | 501 | 502 | -1 | 2012 | 2023 | -11 |
| `pkmn-col-hard/076-Golem-1.png` | 0.116 | 479 | 482 | -3 | 1878 | 1898 | -20 |
| `pkmn-col-hard/077-Ponyta-2.png` | 0.113 | 483 | 485 | -2 | 1897 | 1914 | -17 |
| `pkmn-col-hard/079-Slowpoke-1.png` | 0.136 | 443 | 445 | -2 | 1562 | 1584 | -22 |
| `pkmn-col-hard/080-Slowbro-1.png` | 0.118 | 486 | 489 | -3 | 1920 | 1941 | -21 |
| `pkmn-col-hard/086-Seel-0.png` | 0.141 | 467 | 470 | -3 | 1792 | 1815 | -23 |
| `pkmn-col-hard/086-Seel-1.png` | 0.119 | 466 | 467 | -1 | 1781 | 1790 | -9 |
| `pkmn-col-hard/096-Drowzee-0.png` | 0.193 | 465 | 468 | -3 | 1752 | 1769 | -17 |
| `pkmn-col-hard/096-Drowzee-1.png` | 0.124 | 462 | 463 | -1 | 1726 | 1731 | -5 |
| `pkmn-col-hard/099-Kingler-0.png` | 0.240 | 464 | 467 | -3 | 1744 | 1761 | -17 |
| `pkmn-col-hard/107-Hitmonchan-2.png` | 0.308 | 447 | 449 | -2 | 1584 | 1599 | -15 |
| `pkmn-col-hard/115-Kangaskhan-2.png` | 0.105 | 490 | 491 | -1 | 1928 | 1935 | -7 |
| `pkmn-col-hard/116-Horsea-1.png` | 0.106 | 456 | 459 | -3 | 1687 | 1712 | -25 |
| `pkmn-col-hard/118-Goldeen-0.png` | 0.125 | 485 | 487 | -2 | 1911 | 1928 | -17 |
| `pkmn-col-hard/122-Mr. Mime-1.png` | 0.107 | 489 | 493 | -4 | 1934 | 1967 | -33 |
| `pkmn-col-hard/122-Mr. Mime-2.png` | 0.168 | 523 | 525 | -2 | 2206 | 2224 | -18 |
| `pkmn-col-hard/123-Scyther-0.png` | 0.138 | 502 | 503 | -1 | 2043 | 2053 | -10 |
| `pkmn-col-hard/126-Magmar-2.png` | 0.155 | 516 | 517 | -1 | 2164 | 2172 | -8 |
| `pkmn-col-hard/131-Lapras-2.png` | 0.102 | 483 | 485 | -2 | 1903 | 1919 | -16 |
| `pkmn-col-hard/133-Eevee-1.png` | 0.217 | 466 | 468 | -2 | 1773 | 1785 | -12 |
| `pkmn-col-hard/135-Jolteon-1.png` | 0.121 | 492 | 495 | -3 | 1967 | 1985 | -18 |
| `pkmn-col-hard/135-Jolteon-2.png` | 0.319 | 518 | 520 | -2 | 2172 | 2190 | -18 |
| `pkmn-col-hard/137-Porygon-1.png` | 0.168 | 463 | 464 | -1 | 1729 | 1743 | -14 |
| `pkmn-col-hard/137-Porygon-2.png` | 0.292 | 527 | 530 | -3 | 2248 | 2270 | -22 |
| `pkmn-col-hard/141-Kabutops-0.png` | 0.144 | 471 | 473 | -2 | 1791 | 1805 | -14 |
| `pkmn-col-hard/142-Aerodactyl-2.png` | 0.180 | 511 | 512 | -1 | 2094 | 2104 | -10 |
| `pkmn-col-hard/146-Moltres-1.png` | 0.093 | 497 | 497 | +0 | 2005 | 2007 | -2 |
| `pkmn-col-hard/151-Mew-1.png` | 0.111 | 465 | 467 | -2 | 1778 | 1797 | -19 |
