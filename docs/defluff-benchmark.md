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
- equal results: 15
- strictly better results: 51
- net bytes versus Defluff: -32
- net Deflate bits versus Defluff: -318
- corpus-key SHA-256: `a2dcba76eac95f4139e964066313339f2aa8eb047ce9fe893bd642850b93a0c7`
- Columbo SHA-256: `5bac688c214e576cdfb54bc04b463c8e102a7f3f46f545f06205c5daea8db48e`

## Corpus Summary

| family | rows | misses | equal | better | errors | net bytes | net bits |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| pkmn-bw-hard | 18 | 0 | 9 | 9 | 0 | -5 | -47 |
| pkmn-col-hard | 48 | 0 | 6 | 42 | 0 | -27 | -271 |

## All Results

| source | seconds | Columbo bytes | Defluff bytes | byte delta | Columbo bits | Defluff bits | bit delta |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| `pkmn-bw-hard/000-Logo-5.png` | 0.031 | 436 | 436 | +0 | 864 | 864 | +0 |
| `pkmn-bw-hard/000-Logo-6.png` | 0.013 | 412 | 413 | -1 | 671 | 679 | -8 |
| `pkmn-bw-hard/008-Wartortle-1.png` | 0.044 | 485 | 485 | +0 | 1173 | 1173 | +0 |
| `pkmn-bw-hard/014-Kakuna-1.png` | 0.056 | 453 | 454 | -1 | 941 | 947 | -6 |
| `pkmn-bw-hard/023-Ekans-2.png` | 0.017 | 464 | 464 | +0 | 1037 | 1038 | -1 |
| `pkmn-bw-hard/047-Parasect.png` | 0.043 | 488 | 488 | +0 | 1984 | 1984 | +0 |
| `pkmn-bw-hard/052-Meowth-0.png` | 0.061 | 472 | 472 | +0 | 1095 | 1095 | +0 |
| `pkmn-bw-hard/054-Psyduck-0.png` | 0.072 | 458 | 460 | -2 | 976 | 991 | -15 |
| `pkmn-bw-hard/054-Psyduck-1.png` | 0.078 | 457 | 457 | +0 | 963 | 963 | +0 |
| `pkmn-bw-hard/060-Poliwag-2.png` | 0.011 | 468 | 469 | -1 | 1056 | 1063 | -7 |
| `pkmn-bw-hard/061-Poliwhirl.png` | 0.047 | 506 | 506 | +0 | 2259 | 2263 | -4 |
| `pkmn-bw-hard/084-Doduo-1.png` | 0.052 | 455 | 455 | +0 | 965 | 965 | +0 |
| `pkmn-bw-hard/090-Shellder.png` | 0.034 | 477 | 477 | +0 | 1891 | 1891 | +0 |
| `pkmn-bw-hard/113-Chansey-0.png` | 0.033 | 457 | 457 | +0 | 965 | 968 | -3 |
| `pkmn-bw-hard/113-Chansey.png` | 0.100 | 458 | 458 | +0 | 1890 | 1891 | -1 |
| `pkmn-bw-hard/114-Tangela-1.png` | 0.031 | 480 | 480 | +0 | 1149 | 1151 | -2 |
| `pkmn-bw-hard/122-Mr. Mime-0.png` | 0.062 | 471 | 471 | +0 | 1072 | 1072 | +0 |
| `pkmn-bw-hard/139-Omastar-1.png` | 0.040 | 485 | 485 | +0 | 1188 | 1188 | +0 |
| `pkmn-col-hard/002-Ivysaur-1.png` | 0.066 | 509 | 510 | -1 | 2104 | 2111 | -7 |
| `pkmn-col-hard/003-Venusaur-1.png` | 0.169 | 514 | 515 | -1 | 2131 | 2141 | -10 |
| `pkmn-col-hard/005-Charmeleon-2.png` | 0.179 | 489 | 490 | -1 | 1920 | 1927 | -7 |
| `pkmn-col-hard/006-Charizard-2.png` | 0.113 | 507 | 508 | -1 | 2068 | 2079 | -11 |
| `pkmn-col-hard/015-Beedrill-0.png` | 0.144 | 493 | 493 | +0 | 1966 | 1967 | -1 |
| `pkmn-col-hard/017-Pidgeotto-1.png` | 0.137 | 474 | 475 | -1 | 1806 | 1816 | -10 |
| `pkmn-col-hard/017-Pidgeotto-2.png` | 0.203 | 485 | 486 | -1 | 1893 | 1903 | -10 |
| `pkmn-col-hard/024-Arbok-1.png` | 0.133 | 470 | 470 | +0 | 1801 | 1806 | -5 |
| `pkmn-col-hard/030-Nidorina-1.png` | 0.182 | 495 | 496 | -1 | 1981 | 1987 | -6 |
| `pkmn-col-hard/032-Nidoran-M-2.png` | 0.281 | 492 | 492 | +0 | 1906 | 1911 | -5 |
| `pkmn-col-hard/035-Clefairy-1.png` | 0.121 | 468 | 469 | -1 | 1764 | 1776 | -12 |
| `pkmn-col-hard/043-Oddish-0.png` | 0.128 | 475 | 475 | +0 | 1840 | 1840 | +0 |
| `pkmn-col-hard/048-Venonat-1.png` | 0.211 | 498 | 499 | -1 | 2016 | 2019 | -3 |
| `pkmn-col-hard/048-Venonat-2.png` | 0.111 | 483 | 483 | +0 | 1890 | 1892 | -2 |
| `pkmn-col-hard/052-Meowth-1.png` | 0.173 | 498 | 498 | +0 | 2019 | 2023 | -4 |
| `pkmn-col-hard/052-Meowth-2.png` | 0.090 | 504 | 504 | +0 | 2067 | 2068 | -1 |
| `pkmn-col-hard/055-Golduck-0.png` | 0.108 | 471 | 471 | +0 | 1794 | 1796 | -2 |
| `pkmn-col-hard/058-Growlithe-1.png` | 0.211 | 481 | 482 | -1 | 1864 | 1868 | -4 |
| `pkmn-col-hard/062-Poliwrath-0.png` | 0.112 | 490 | 490 | +0 | 1933 | 1933 | +0 |
| `pkmn-col-hard/063-Abra-2.png` | 0.219 | 474 | 476 | -2 | 1847 | 1864 | -17 |
| `pkmn-col-hard/073-Tentacruel-0.png` | 0.158 | 502 | 502 | +0 | 2019 | 2023 | -4 |
| `pkmn-col-hard/076-Golem-1.png` | 0.109 | 481 | 482 | -1 | 1892 | 1898 | -6 |
| `pkmn-col-hard/077-Ponyta-2.png` | 0.105 | 485 | 485 | +0 | 1913 | 1914 | -1 |
| `pkmn-col-hard/079-Slowpoke-1.png` | 0.137 | 444 | 445 | -1 | 1571 | 1584 | -13 |
| `pkmn-col-hard/080-Slowbro-1.png` | 0.096 | 488 | 489 | -1 | 1934 | 1941 | -7 |
| `pkmn-col-hard/086-Seel-0.png` | 0.129 | 469 | 470 | -1 | 1808 | 1815 | -7 |
| `pkmn-col-hard/086-Seel-1.png` | 0.115 | 466 | 467 | -1 | 1781 | 1790 | -9 |
| `pkmn-col-hard/096-Drowzee-0.png` | 0.157 | 467 | 468 | -1 | 1764 | 1769 | -5 |
| `pkmn-col-hard/096-Drowzee-1.png` | 0.108 | 463 | 463 | +0 | 1731 | 1731 | +0 |
| `pkmn-col-hard/099-Kingler-0.png` | 0.233 | 466 | 467 | -1 | 1755 | 1761 | -6 |
| `pkmn-col-hard/107-Hitmonchan-2.png` | 0.324 | 447 | 449 | -2 | 1584 | 1599 | -15 |
| `pkmn-col-hard/115-Kangaskhan-2.png` | 0.108 | 490 | 491 | -1 | 1928 | 1935 | -7 |
| `pkmn-col-hard/116-Horsea-1.png` | 0.099 | 458 | 459 | -1 | 1700 | 1712 | -12 |
| `pkmn-col-hard/118-Goldeen-0.png` | 0.119 | 486 | 487 | -1 | 1920 | 1928 | -8 |
| `pkmn-col-hard/122-Mr. Mime-1.png` | 0.100 | 492 | 493 | -1 | 1954 | 1967 | -13 |
| `pkmn-col-hard/122-Mr. Mime-2.png` | 0.154 | 525 | 525 | +0 | 2219 | 2224 | -5 |
| `pkmn-col-hard/123-Scyther-0.png` | 0.129 | 503 | 503 | +0 | 2053 | 2053 | +0 |
| `pkmn-col-hard/126-Magmar-2.png` | 0.137 | 517 | 517 | +0 | 2170 | 2172 | -2 |
| `pkmn-col-hard/131-Lapras-2.png` | 0.097 | 484 | 485 | -1 | 1911 | 1919 | -8 |
| `pkmn-col-hard/133-Eevee-1.png` | 0.288 | 468 | 468 | +0 | 1785 | 1785 | +0 |
| `pkmn-col-hard/135-Jolteon-1.png` | 0.131 | 495 | 495 | +0 | 1985 | 1985 | +0 |
| `pkmn-col-hard/135-Jolteon-2.png` | 0.339 | 520 | 520 | +0 | 2187 | 2190 | -3 |
| `pkmn-col-hard/137-Porygon-1.png` | 0.212 | 464 | 464 | +0 | 1737 | 1743 | -6 |
| `pkmn-col-hard/137-Porygon-2.png` | 0.330 | 530 | 530 | +0 | 2265 | 2270 | -5 |
| `pkmn-col-hard/141-Kabutops-0.png` | 0.154 | 473 | 473 | +0 | 1803 | 1805 | -2 |
| `pkmn-col-hard/142-Aerodactyl-2.png` | 0.197 | 512 | 512 | +0 | 2102 | 2104 | -2 |
| `pkmn-col-hard/146-Moltres-1.png` | 0.126 | 497 | 497 | +0 | 2005 | 2007 | -2 |
| `pkmn-col-hard/151-Mew-1.png` | 0.126 | 466 | 467 | -1 | 1791 | 1797 | -6 |
