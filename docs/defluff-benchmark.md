# Defluff Default Benchmark

This report compares normal/default Columbo with paired Defluff PNG
outputs. A case passes only when Columbo is equal or smaller in both
complete-file bytes and meaningful Deflate-stream bits.

- discovered source/reference pairs: 66
- completed rows for this binary: 66
- missing rows: 0
- rows from older Columbo binaries: 0
- parity misses: 2
- prior-result regressions over 10%: 0
- errors: 0
- equal results: 15
- strictly better results: 49
- net bytes versus Defluff: -22
- net Deflate bits versus Defluff: -243
- corpus-key SHA-256: `8645165316ae2a5d4205fb716b36b6c4b1e40e821207d1d0684a392e5814b8c5`
- Columbo SHA-256: `c8eea2e26a2a6300a3212fbf843a60fc27d5ed4272d5e03430c5631eb2716a44`

## Corpus Summary

| family | rows | misses | equal | better | errors | net bytes | net bits |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| pkmn-bw-hard | 18 | 2 | 8 | 8 | 0 | +0 | -20 |
| pkmn-col-hard | 48 | 0 | 7 | 41 | 0 | -22 | -223 |

## Parity Misses

| source | bytes vs Defluff | bits vs Defluff |
| --- | ---: | ---: |
| `pkmn-bw-hard/023-Ekans-2.png` | +1 | +5 |
| `pkmn-bw-hard/060-Poliwag-2.png` | +1 | +4 |

## All Results

| source | seconds | Columbo bytes | Defluff bytes | byte delta | Columbo bits | Defluff bits | bit delta |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| `pkmn-bw-hard/000-Logo-5.png` | 0.069 | 436 | 436 | +0 | 863 | 864 | -1 |
| `pkmn-bw-hard/000-Logo-6.png` | 0.028 | 413 | 413 | +0 | 678 | 679 | -1 |
| `pkmn-bw-hard/008-Wartortle-1.png` | 0.064 | 485 | 485 | +0 | 1173 | 1173 | +0 |
| `pkmn-bw-hard/014-Kakuna-1.png` | 0.052 | 454 | 454 | +0 | 945 | 947 | -2 |
| `pkmn-bw-hard/023-Ekans-2.png` | 0.077 | 465 | 464 | +1 | 1043 | 1038 | +5 |
| `pkmn-bw-hard/047-Parasect.png` | 0.072 | 488 | 488 | +0 | 1984 | 1984 | +0 |
| `pkmn-bw-hard/052-Meowth-0.png` | 0.140 | 472 | 472 | +0 | 1095 | 1095 | +0 |
| `pkmn-bw-hard/054-Psyduck-0.png` | 0.142 | 458 | 460 | -2 | 976 | 991 | -15 |
| `pkmn-bw-hard/054-Psyduck-1.png` | 0.136 | 457 | 457 | +0 | 963 | 963 | +0 |
| `pkmn-bw-hard/060-Poliwag-2.png` | 0.045 | 470 | 469 | +1 | 1067 | 1063 | +4 |
| `pkmn-bw-hard/061-Poliwhirl.png` | 0.087 | 506 | 506 | +0 | 2259 | 2263 | -4 |
| `pkmn-bw-hard/084-Doduo-1.png` | 0.115 | 455 | 455 | +0 | 965 | 965 | +0 |
| `pkmn-bw-hard/090-Shellder.png` | 0.072 | 477 | 477 | +0 | 1891 | 1891 | +0 |
| `pkmn-bw-hard/113-Chansey-0.png` | 0.065 | 457 | 457 | +0 | 965 | 968 | -3 |
| `pkmn-bw-hard/113-Chansey.png` | 0.219 | 458 | 458 | +0 | 1890 | 1891 | -1 |
| `pkmn-bw-hard/114-Tangela-1.png` | 0.059 | 480 | 480 | +0 | 1149 | 1151 | -2 |
| `pkmn-bw-hard/122-Mr. Mime-0.png` | 0.131 | 471 | 471 | +0 | 1072 | 1072 | +0 |
| `pkmn-bw-hard/139-Omastar-1.png` | 0.064 | 485 | 485 | +0 | 1188 | 1188 | +0 |
| `pkmn-col-hard/002-Ivysaur-1.png` | 0.120 | 509 | 510 | -1 | 2104 | 2111 | -7 |
| `pkmn-col-hard/003-Venusaur-1.png` | 0.205 | 514 | 515 | -1 | 2131 | 2141 | -10 |
| `pkmn-col-hard/005-Charmeleon-2.png` | 0.203 | 489 | 490 | -1 | 1920 | 1927 | -7 |
| `pkmn-col-hard/006-Charizard-2.png` | 0.158 | 508 | 508 | +0 | 2075 | 2079 | -4 |
| `pkmn-col-hard/015-Beedrill-0.png` | 0.151 | 493 | 493 | +0 | 1966 | 1967 | -1 |
| `pkmn-col-hard/017-Pidgeotto-1.png` | 0.184 | 474 | 475 | -1 | 1806 | 1816 | -10 |
| `pkmn-col-hard/017-Pidgeotto-2.png` | 0.316 | 485 | 486 | -1 | 1893 | 1903 | -10 |
| `pkmn-col-hard/024-Arbok-1.png` | 0.168 | 470 | 470 | +0 | 1803 | 1806 | -3 |
| `pkmn-col-hard/030-Nidorina-1.png` | 0.236 | 495 | 496 | -1 | 1984 | 1987 | -3 |
| `pkmn-col-hard/032-Nidoran-M-2.png` | 0.337 | 492 | 492 | +0 | 1906 | 1911 | -5 |
| `pkmn-col-hard/035-Clefairy-1.png` | 0.178 | 468 | 469 | -1 | 1767 | 1776 | -9 |
| `pkmn-col-hard/043-Oddish-0.png` | 0.201 | 475 | 475 | +0 | 1840 | 1840 | +0 |
| `pkmn-col-hard/048-Venonat-1.png` | 0.353 | 498 | 499 | -1 | 2016 | 2019 | -3 |
| `pkmn-col-hard/048-Venonat-2.png` | 0.167 | 483 | 483 | +0 | 1890 | 1892 | -2 |
| `pkmn-col-hard/052-Meowth-1.png` | 0.192 | 498 | 498 | +0 | 2019 | 2023 | -4 |
| `pkmn-col-hard/052-Meowth-2.png` | 0.143 | 504 | 504 | +0 | 2067 | 2068 | -1 |
| `pkmn-col-hard/055-Golduck-0.png` | 0.159 | 471 | 471 | +0 | 1794 | 1796 | -2 |
| `pkmn-col-hard/058-Growlithe-1.png` | 0.332 | 481 | 482 | -1 | 1864 | 1868 | -4 |
| `pkmn-col-hard/062-Poliwrath-0.png` | 0.155 | 490 | 490 | +0 | 1933 | 1933 | +0 |
| `pkmn-col-hard/063-Abra-2.png` | 0.255 | 475 | 476 | -1 | 1849 | 1864 | -15 |
| `pkmn-col-hard/073-Tentacruel-0.png` | 0.213 | 502 | 502 | +0 | 2021 | 2023 | -2 |
| `pkmn-col-hard/076-Golem-1.png` | 0.146 | 482 | 482 | +0 | 1897 | 1898 | -1 |
| `pkmn-col-hard/077-Ponyta-2.png` | 0.146 | 485 | 485 | +0 | 1913 | 1914 | -1 |
| `pkmn-col-hard/079-Slowpoke-1.png` | 0.204 | 444 | 445 | -1 | 1571 | 1584 | -13 |
| `pkmn-col-hard/080-Slowbro-1.png` | 0.138 | 488 | 489 | -1 | 1935 | 1941 | -6 |
| `pkmn-col-hard/086-Seel-0.png` | 0.198 | 470 | 470 | +0 | 1810 | 1815 | -5 |
| `pkmn-col-hard/086-Seel-1.png` | 0.162 | 466 | 467 | -1 | 1783 | 1790 | -7 |
| `pkmn-col-hard/096-Drowzee-0.png` | 0.253 | 467 | 468 | -1 | 1764 | 1769 | -5 |
| `pkmn-col-hard/096-Drowzee-1.png` | 0.187 | 463 | 463 | +0 | 1731 | 1731 | +0 |
| `pkmn-col-hard/099-Kingler-0.png` | 0.369 | 466 | 467 | -1 | 1758 | 1761 | -3 |
| `pkmn-col-hard/107-Hitmonchan-2.png` | 0.408 | 448 | 449 | -1 | 1587 | 1599 | -12 |
| `pkmn-col-hard/115-Kangaskhan-2.png` | 0.123 | 490 | 491 | -1 | 1928 | 1935 | -7 |
| `pkmn-col-hard/116-Horsea-1.png` | 0.146 | 458 | 459 | -1 | 1702 | 1712 | -10 |
| `pkmn-col-hard/118-Goldeen-0.png` | 0.171 | 486 | 487 | -1 | 1920 | 1928 | -8 |
| `pkmn-col-hard/122-Mr. Mime-1.png` | 0.134 | 492 | 493 | -1 | 1954 | 1967 | -13 |
| `pkmn-col-hard/122-Mr. Mime-2.png` | 0.206 | 525 | 525 | +0 | 2222 | 2224 | -2 |
| `pkmn-col-hard/123-Scyther-0.png` | 0.187 | 503 | 503 | +0 | 2053 | 2053 | +0 |
| `pkmn-col-hard/126-Magmar-2.png` | 0.187 | 517 | 517 | +0 | 2170 | 2172 | -2 |
| `pkmn-col-hard/131-Lapras-2.png` | 0.144 | 484 | 485 | -1 | 1911 | 1919 | -8 |
| `pkmn-col-hard/133-Eevee-1.png` | 0.319 | 468 | 468 | +0 | 1785 | 1785 | +0 |
| `pkmn-col-hard/135-Jolteon-1.png` | 0.196 | 495 | 495 | +0 | 1985 | 1985 | +0 |
| `pkmn-col-hard/135-Jolteon-2.png` | 0.383 | 520 | 520 | +0 | 2188 | 2190 | -2 |
| `pkmn-col-hard/137-Porygon-1.png` | 0.326 | 464 | 464 | +0 | 1742 | 1743 | -1 |
| `pkmn-col-hard/137-Porygon-2.png` | 0.363 | 530 | 530 | +0 | 2265 | 2270 | -5 |
| `pkmn-col-hard/141-Kabutops-0.png` | 0.244 | 473 | 473 | +0 | 1803 | 1805 | -2 |
| `pkmn-col-hard/142-Aerodactyl-2.png` | 0.207 | 512 | 512 | +0 | 2104 | 2104 | +0 |
| `pkmn-col-hard/146-Moltres-1.png` | 0.161 | 497 | 497 | +0 | 2005 | 2007 | -2 |
| `pkmn-col-hard/151-Mew-1.png` | 0.169 | 466 | 467 | -1 | 1791 | 1797 | -6 |
