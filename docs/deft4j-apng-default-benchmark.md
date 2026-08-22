# Steam Stickers APNG Default-mode Benchmark

This separate APNG phase runs plain Columbo Default mode and validates
its output against the source APNG. Default results are not compared
to deft4j. The completed Max row for the same source is used only for
two gates: Default must be faster, and Max must be no larger than
Default in both file bytes and meaningful Deflate bits.
Gate failures are recorded and never interrupt the corpus run.

## Current scoped revalidation (2026-08-21)

The accepted candidate
(`572c0711f23e628db79726c1be13fa8bcd7aadf90f97aac518a95858803d956d`)
was run in Default mode on all twelve former Max misses. All twelve Default
runs were faster than their corresponding Max runs, and every Max result was
no worse than Default in both file bytes and aggregate meaningful Deflate
bits. Aggregate time was 11.06 seconds for Default and 178.25 seconds for Max;
Max saved a further 17,769 bytes and 142,080 bits across the twelve files.

Default mode was also rerun on all 54 fixtures recorded as errors in the old
partial state. Every file completed, reconstructed with unchanged decoded
image/frame streams, and produced a valid comparison result. The comparator
permits only the specification-defined repair performed by Columbo: removal
of a non-empty, palette-sized `tRNS` from an RGBA PNG that has the matching
suggested `PLTE`. Other `tRNS` changes remain errors. It also naturally counts
the removal of bytes after `IEND`, because those bytes are outside the PNG
datastream. The historical error table has therefore been removed.

The summary, gate-failure, and row tables below are retained as the historical
stopped `f287245e…` partial state; they do not describe the current candidate.

## Historical APNG Default/Max policy validation (2026-08-20)

The historical partial run below identified 59 cases in which Default was
faster and beat Max in file bytes, meaningful Deflate bits, or both. Of those,
36 had fewer Default bytes, 18 had equal bytes with fewer Default bits, and 5
traded more Default bytes for fewer bits.

An initial serial file-floor implementation eliminated all 59 failures in a
targeted rerun, but it was too coarse. It charged Default's elapsed time
against Max's configured allowance. On
`1073810_105834_880a33fbcd8b79da3307349e7a4e2adeec08d5dd.png`
that changed a healthy historical Max result from 241242 bytes to the
248283-byte Default floor, a 7041-byte Max regression.

The replacement policy makes two narrower changes:

- Multi-image APNG Default keeps the full initial planner for every image
  stream, but does not repeat the reparse/replay, endpoint-proven, or compact
  feedback lineages that Max already covers.
- A bounded multi-image Max run on a machine with at least two CPUs races a
  quiet complete Default file sibling against original-source Max. Max retains
  its full configured allowance and replaces the floor only when both file
  bytes and aggregate meaningful Deflate bits are no worse. The race is
  limited to 8 MiB of compressed image data and 64 MiB decoded data so the
  second complete model cannot make unbounded memory use.

The final release binary after the bounded-gate audit is
`e4cdc2266c1c40985567a82446ef510f7742af4260486215f3e950783afb2c63`.
Targeted validation of the same bounded path produced the results below.
Default and Max artifacts were both decoded and compared with their source;
Default was not compared with deft4j. A full corpus rerun was intentionally not
started after the earlier stop request.

| case | Default seconds | Max seconds | Default bytes/bits | Max bytes/bits | Max dominates |
| --- | ---: | ---: | ---: | ---: | --- |
| `1016180_195793_5e51d7d4681ccb4bf297035dbcf0cf9b41dfc9da.png` | 0.083 | 11.577 | 5656 / 22246 | 5656 / 22244 | yes |
| `1217060_193913_2409644168203140379282f77f15d294c2daa444.png` | 1.369 | 12.717 | 288213 / 2277721 | 280293 / 2214354 | yes |
| `1073810_105834_880a33fbcd8b79da3307349e7a4e2adeec08d5dd.png` | 1.888 | 20.142 | 248387 / 1944938 | 241085 / 1886438 | yes |
| `1293230_103411_1e04224079d6db3360f8c1b9f60b3eaff0436530.png` | 7.782 | 20.076 | 210765 / 1657136 | 209319 / 1645617 | yes |
| `1693980_223080_b8608057a48217caa9fcfa74f10956191e0d2abe.png` | 6.985 | 29.836 | 246331 / 1921234 | 239878 / 1869603 | yes |
| `2157830_457063_26db4a13918de1e2bf474f71b154466f995664c7.png` | 1.697 | 22.174 | 300150 / 2358103 | 300100 / 2357708 | yes |

The pathological Default samples improved from 116.61 to 7.78 seconds and
61.17 to 6.99 seconds while retaining 911 and 238 bytes of file savings,
respectively. The three largest historical Max-floor failures fell from
6.92–12.36 seconds to 1.32–1.68 seconds in Default mode.

Max wall time remained at the historical timeout-plus-active-route-grace
boundary on every targeted case. Repeated deadline-sensitive runs of the
largest stress sample showed small byte-level variation: current outputs were
239878 and 239732 bytes, versus historical/confirmation outputs of 239771 and
239708 bytes. This is not a material corpus-quality regression, but the race
does add the bounded Default sibling's CPU work while it is active.

Verbose and Visual produced byte-identical 5656-byte output with 22244
aggregate meaningful Deflate bits on the reporting sample. The complete Rust
test suite also passed (407 tests), including the PNG compressed-metadata
same-byte bit-savings test that rejected the earlier overly broad `Shared`
policy change.

The summary and row tables below retain the earlier `f287245e…` partial-run
state for historical diagnosis. Their gate failures are the pre-fix evidence,
not failures of the current targeted binary above.

- completed cases: 1090
- historical errors in this stopped state: 32
- missing Max gate baselines: 0
- Default-not-faster gate failures: 161
- Max-quality gate failures: 66
- rows from current Columbo binary: 1122
- rows from other Columbo binaries: 0

## Historical stopped-state gate failures

| file | Default seconds | Max seconds | Default faster | Max bytes <= Default | Max bits <= Default |
| --- | ---: | ---: | --- | --- | --- |
| `1012790_244364_07998cd20b1f60747bc3d4d7e3c9e13c90631846.png` | 35.29 | 34.89 | False | False | False |
| `1016180_195793_5e51d7d4681ccb4bf297035dbcf0cf9b41dfc9da.png` | 0.70 | 11.57 | True | False | False |
| `1016180_195795_c139db3e4ed18fbe65577884c2e0e5a4f622fd5a.png` | 0.75 | 11.57 | True | False | False |
| `1025600_359593_249f89d7fe23f0e87e3b3eb163e5821730b2367e.png` | 31.07 | 15.84 | False | True | True |
| `102840_103125_4906c9b4c2940fc3a3ecb6b1b59c8676f8f07397.png` | 19.16 | 11.61 | False | True | True |
| `1037910_115270_3a514e239db5073285b0c5fc53d0b7858b5ced2c.png` | 12.39 | 11.63 | False | True | True |
| `1051690_224707_f19fff308e1b4e7508846063107c7d3872b453df.png` | 1.81 | 11.65 | True | False | False |
| `105600_79913_f7bbf0a8dd394260ed9fff2464ad4f39d96225ef.png` | 12.48 | 11.61 | False | True | True |
| `1057780_353006_50543dd5767b17246a281f16916de00a3deb2f63.png` | 1.39 | 11.59 | True | True | False |
| `1061180_104385_f45506ec0f99799b0cd653366f0f01d4e6272c94.png` | 42.09 | 30.69 | False | True | True |
| `1062830_143619_afc38fdaea3c04b9ed8a99ab2debcaac633481c3.png` | 67.66 | 59.74 | False | True | True |
| `1062830_143620_d3e2149371c2407fcb4e60376facc98294d8e1c5.png` | 90.36 | 89.87 | False | True | True |
| `1062830_143627_de12cd64d3ebed1c3fb42528422f8ee5a83a00e9.png` | 85.89 | 74.24 | False | True | True |
| `1070330_198283_4ce01ab86ac95878fe34f045ed09a9fb79cd17c3.png` | 27.52 | 19.1 | False | True | True |
| `1075550_135986_e22dc88738f1e6487b8221757a2e74971395f82a.png` | 1.29 | 11.64 | True | False | False |
| `1075550_135991_651491874c5b8eb0d2d5958f859ceb325cf003e9.png` | 0.84 | 11.61 | True | True | False |
| `1085260_288655_3a911a877b4f815b5b97552cb18acd2b928f811c.png` | 50.50 | 16.95 | False | True | True |
| `1085750_106610_60360e7aa0bf52ae34cebc8c6bd49971b6fe0b38.png` | 12.09 | 11.6 | False | True | True |
| `1091500_102625_0ba74e973d94ddef6351d300cfe575eceba9980c.png` | 180.87 | 135.04 | False | True | True |
| `1102880_219355_7df1c5bac38902a4032a11de415472c6dd598428.png` | 0.78 | 11.6 | True | False | False |
| `1102880_219356_f8d00dd10b43785d059474f2b70a49e1ddee805c.png` | 0.35 | 11.57 | True | True | False |
| `1124660_151462_3777c946a544eb5350f19824ac416479b137f641.png` | 38.44 | 25.37 | False | True | True |
| `1124660_151463_60a2dfe74bc81e4f277e96898117c8d4a19b6184.png` | 54.84 | 23.24 | False | True | True |
| `1124660_151464_ce424c481fd5b5ce4f6003273bee02145d3c3019.png` | 35.24 | 20.05 | False | True | True |
| `1127700_286495_8fe0009e48649c7a65a159649538b8cbdbcee630.png` | 21.80 | 11.64 | False | True | True |
| `1145350_430380_716e9f5b0c7890a5098e852ed7a2a46f190d1fd7.png` | 22.34 | 12.76 | False | True | True |
| `1145350_430381_898ab2cf0fa498601b7041f8a7908cc340f85120.png` | 38.78 | 17.96 | False | True | True |
| `1145350_430382_c6b713bb3d2330bcd9189be4df38edd8b3f67eb0.png` | 42.23 | 17.95 | False | True | True |
| `1145350_430383_85d1c487bb77f7c4650b6321a347ab027087a7fc.png` | 17.06 | 16.89 | False | True | True |
| `1145350_430384_5ef39689763eaba063043adee0e65fc47992ae0e.png` | 17.03 | 16.9 | False | True | True |
| `1145350_430385_87de4319b2a755ef773f16ea2a648634ec125d6c.png` | 28.73 | 18.99 | False | True | True |
| `1145350_430386_f951a5510c4ef847f49677928b8115c11afe6dc8.png` | 46.29 | 21.14 | False | True | True |
| `1145350_430387_e407fa0bca812b0f2a89c6a52ca270bac8bde95f.png` | 22.02 | 16.89 | False | True | True |
| `1145350_430388_b7b7e73475647bb0cbd146eb6f349c53c5883388.png` | 34.18 | 19.02 | False | True | True |
| `1159090_282934_5fe9d22c4eff740bcf69cf5c5118c949fac661b4.png` | 34.67 | 30.76 | False | True | True |
| `1159090_282937_2381d7211f26f2ab5f5b34544e5989572f19c6e0.png` | 70.20 | 49.78 | False | True | True |
| `1176710_142388_355109cf4f0270939f9acad2bc6b8d9f4205894b.png` | 63.92 | 62.87 | False | True | True |
| `1176710_142389_5db69f0bffd84fcd25faae26455ee7090d6e8f49.png` | 55.91 | 48.78 | False | True | True |
| `1190970_313726_40c4b593455f914ba990d6d55c33bd9c5ceadbfc.png` | 26.17 | 21.16 | False | True | True |
| `1191660_277350_154e9bcb004849a1a3f8d819cb30291fccd350a4.png` | 2.02 | 11.63 | True | True | False |
| `1203420_153564_bbc56545debc214970ea081bcc7e6eb383e7930e.png` | 18.33 | 14.89 | False | True | True |
| `1203420_153567_e58d362b8a21ac99d3043606f23932e6474a028e.png` | 24.82 | 16.95 | False | True | True |
| `1203420_153568_ff11b39f7cef8de7593047e22c7ae77bcda25049.png` | 16.80 | 14.82 | False | True | True |
| `1203420_153569_af0ba79f70d498d4ceb1fb08f113d0ce693c52be.png` | 21.08 | 16.99 | False | True | True |
| `1210230_163424_dce47607413c7aec7f70e0f85d272b526805a656.png` | 11.99 | 11.6 | False | True | True |
| `1210230_163425_2738e56e7690cc29b6fa3f6a3d7f122ee78fe90a.png` | 14.96 | 14.79 | False | True | True |
| `1210230_163426_a0dc6a583d67105a14d67d0be33cd7d33ed52edf.png` | 22.53 | 19.07 | False | True | True |
| `1210320_393938_f726f2b1a323964b9e4cdde594d134121e69c349.png` | 30.95 | 25.49 | False | True | True |
| `1210320_393941_25e4efd8b3df51a23e67019ddc0be6c9ba6d0cdf.png` | 32.25 | 27.75 | False | True | True |
| `1222670_363987_46a09f27bc307afac889df0b59b4ab4b039a0313.png` | 32.35 | 24.34 | False | True | True |
| `1222670_363992_02b07dc524f0a1d40dd643b0bb0b5cc0e271005e.png` | 46.10 | 24.36 | False | True | True |
| `1223590_201834_868eaa8574aec5dc90dc3f787142542c1e07a929.png` | 18.46 | 15.86 | False | True | True |
| `1223590_201840_a6167c0df3b2b0965b0512d1234b032423fd6abf.png` | 18.70 | 11.62 | False | True | True |
| `1237730_184005_57c70076c41a236f64afe2ff9b65c0bfa0a8cf73.png` | 25.39 | 17.35 | False | True | True |
| `1239300_150303_2ac7598f4913eab8c626cd9224f732c906096737.png` | 34.39 | 31.81 | False | True | True |
| `1239690_164751_3ead088c1f61dae59a9bc1d6c3d8ceec0cdfb853.png` | 14.13 | 11.66 | False | True | True |
| `1239690_164754_6b034a9af4c23ccfead0d4c2e5af549330bed0cd.png` | 18.99 | 20.11 | True | False | False |
| `1239690_164760_5c537fda8f07c55d97ab7dc5ec0b3cfde2e520d2.png` | 35.51 | 27.54 | False | True | True |
| `1245560_255371_ce8057df405235067434f9b9fa2665e3c05ee33d.png` | 2.00 | 11.65 | True | True | False |
| `1286320_231517_396a67d1fdb6dd2552d5d4279179991a8e6b3ed9.png` | 14.15 | 11.61 | False | True | True |
| `1286320_231520_2ad241cdf7384bc78d9ef40454260472e5f8859f.png` | 15.63 | 13.73 | False | True | True |
| `1290760_1332152_f49394cee5b554d3a15d790a7bfbe7a6294e6f6d.png` | 14.43 | 12.66 | False | True | True |
| `1293230_103411_1e04224079d6db3360f8c1b9f60b3eaff0436530.png` | 116.61 | 19.72 | False | True | True |
| `1293230_103412_a3bd2cd72e46972dceee9cac14e2daba7166b4d0.png` | 19.03 | 12.72 | False | True | True |
| `1293230_103413_2490e326f31e6b156ece0a6d5471603b0dcb333e.png` | 23.38 | 14.8 | False | True | True |
| `1293230_103415_56980451072225f851bf42b1a6b70076e2149c4b.png` | 16.14 | 11.62 | False | True | True |
| `1293230_103416_06e181e2a68637525d9d5b76c8b2c904af44c502.png` | 42.18 | 29.7 | False | True | True |
| `1293230_103420_b448832aa64e800e76be9252add71c4a90d48a9d.png` | 24.40 | 21.14 | False | True | True |
| `1293230_103421_060a80283a162975f44c52d10ed5a4731e8615eb.png` | 26.35 | 21.22 | False | True | True |
| `1293230_103422_b2089ee6f23c7b1f953f31a2ddfc870472cb493b.png` | 24.00 | 22.22 | False | True | True |
| `1295660_427187_77287e8aa384407a7c090c7e0c1d972e95f6d080.png` | 18.70 | 12.75 | False | True | True |
| `1313140_174331_d33ad2de01973e53eedfd3eb6d011302e1a6c43f.png` | 13.67 | 11.62 | False | True | True |
| `1328670_144864_46b17e0f1dfb49fe3553660c9dc130dc213f2a66.png` | 15.31 | 13.97 | False | True | True |
| `1328670_144866_d600b96130a3a97df25156dacd1501a2875f1ebb.png` | 11.79 | 11.77 | False | True | True |
| `1328670_144868_d7d7a122298df1a3f97fa354a4042a15df8fb531.png` | 20.89 | 14.84 | False | True | True |
| `1328670_144877_04a6d67f2bcdf8131f030168eb49659ab9d0879d.png` | 15.95 | 14.78 | False | True | True |
| `1337760_205735_27742990aa8273073c9622b6c41e8441680f935b.png` | 0.92 | 11.63 | True | False | False |
| `1337760_205736_365c31961b72af81ddbcc8ea4e446aa8633e2237.png` | 1.38 | 11.63 | True | False | False |
| `1337760_205738_3dafad15a162cc34d1702e79b82c2a606d3b20d9.png` | 1.63 | 11.64 | True | False | True |
| `1337760_205742_25db20197ee60ea4560d6fafabe4ff3f92cda0e6.png` | 0.65 | 11.57 | True | True | False |
| `1355220_79867_9dec4843421735e53866432552463c638aeb33ec.png` | 2.43 | 11.67 | True | False | False |
| `1355220_79872_93888b9bd8006a9aa5d8f8298330c51f8741c7f4.png` | 1.88 | 11.63 | True | False | False |
| `1379870_118664_9b683d2f9fa4fe20df6223df2971d0f422c68b42.png` | 1.23 | 11.65 | True | False | False |
| `1379870_118665_1b1b247aaa8175b9c31c660b08020a2ecabdf101.png` | 0.58 | 11.6 | True | True | False |
| `1379870_118666_df4350d5265a1cb20a05b92877325fc6fc876c97.png` | 0.95 | 11.62 | True | True | False |
| `1381770_457775_909efdf105b682a03b9261ce78cbc5c1475e21a4.png` | 13.21 | 11.62 | False | True | True |
| `1390700_244163_cf43d8de32e6c2a72f7a2a65c552e24f11cc8028.png` | 19.73 | 15.86 | False | True | True |
| `1411020_310832_06aea368efdd86d6f7c733aac5a625ee207c14bc.png` | 17.33 | 12.75 | False | True | True |
| `1418970_122280_e5dc068220be7647d2cf8f3cbd0f47cbbd1c10c3.png` | 35.28 | 23.36 | False | True | True |
| `1422650_1992058_2ab31e97a1d02eddd6e31a2820ffc4872da1aa40.png` | 26.36 | 24.37 | False | True | True |
| `1426210_223074_5c13f221b3c49f6804d2fa59a9e43362fc2515cb.png` | 16.96 | 12.67 | False | True | True |
| `1426210_223075_0f32f9cdf941934ecb28bf1c604ea480c4df2632.png` | 71.02 | 25.52 | False | True | True |
| `1426450_240957_bc2095db81b02335fea9cc7790d1d00778843f96.png` | 22.69 | 22.2 | False | True | True |
| `1430190_397385_8095e6f89e38b7f25fd9c79b25e1364cfc32d104.png` | 50.55 | 25.86 | False | True | True |
| `1435780_123028_0514dadbca67ecca8d647f7cbe419e8995a3d2b3.png` | 36.80 | 32.8 | False | True | True |
| `1435780_123030_b5c3a2279908ef60bffdfd2afd3b94ca2b228612.png` | 28.65 | 24.31 | False | True | True |
| `1451190_209696_12c1c9e2f9ec0319297e3f5f6e61870c82896be1.png` | 38.52 | 25.47 | False | True | True |
| `1472560_143586_15738d83d36fa4247df65306c6962281b1c28350.png` | 40.16 | 31.96 | False | True | True |
| `1472560_143587_de64e0be8c5033cd5947aee78fe77b035f9840ef.png` | 73.94 | 72.02 | False | True | True |
| `1480560_143645_2a93fa9ced1b938f7011bd226d81986f9fbcce6d.png` | 30.32 | 26.7 | False | True | True |
| `1480560_143646_86b014769ef14621c59fd521834c48a265c5b8b4.png` | 26.95 | 23.47 | False | True | True |
| `1492660_115372_9e2f7aab4b24e0874d61517559ce516ffe59f23d.png` | 12.89 | 11.63 | False | True | True |
| `1499240_164477_7f9ac16da2d9b4d81ad2ddf10d318f70ce2b2898.png` | 0.38 | 11.57 | True | False | False |
| `1499240_164478_757682cd22ea8bf2beac0a5c0cb575755f5c4d09.png` | 1.42 | 11.66 | True | True | False |
| `1529220_183212_e162e148531c9a8090bdf85b1ef68807832d5102.png` | 29.23 | 15.89 | False | True | True |
| `1529220_183213_9c248d0265b277d15e8b7ce88b4ade58cf7b2b8a.png` | 17.96 | 14.84 | False | True | True |
| `1529220_183214_78d47b3ffadae78a93aa25f962291518328fb49b.png` | 26.05 | 15.91 | False | True | True |
| `1529220_183215_6ba134e52a2c0567a7843a5ab069eefeed0411f7.png` | 19.71 | 16.99 | False | True | True |
| `1529220_183217_ac9e0774790152f00f74c13671c27543bcd90d7f.png` | 12.70 | 11.63 | False | True | True |
| `1536470_252478_1db44fa1b821b6e0661a4f4eb305b2a08a52b64e.png` | 15.76 | 11.65 | False | True | True |
| `1554600_218942_8c31ca571b55d13ce1de1a9bce4357605273ccb5.png` | 0.86 | 11.59 | True | True | False |
| `1574580_153545_ba9db478ed767dd480c69e9c70411d9b9d63e71c.png` | 65.68 | 27.61 | False | True | True |
| `1574580_153546_99021ac3f709f9d5369861c50cbf301c3be913ae.png` | 120.10 | 51.09 | False | True | True |
| `1574580_153547_c1bb9a5283a78fafb358449d09611cd7346efaf0.png` | 59.69 | 37.17 | False | True | True |
| `1574580_153548_348e98c8c8e61532fbedabaab68be685259a795b.png` | 75.17 | 47.87 | False | True | True |
| `1574580_153551_84ac7a440a221868ee8db2578391fbee8e340ce8.png` | 102.89 | 49.83 | False | True | True |
| `1594460_188481_92e5d58b89f1cdddcf89b12ec2d17d16b0574a27.png` | 46.31 | 31.75 | False | True | True |
| `1644500_219934_e5c5c1c2ad3a8330e61870a41ca883c8cd43962c.png` | 180.21 | 69.06 | False | True | True |
| `1644500_219935_8ce027efae46316d34a414f655bbc8a467e77a5a.png` | 99.88 | 47.1 | False | True | True |
| `1644500_219936_57fad7001946811af7f3de3467fc442fea3d1264.png` | 180.61 | 134.58 | False | True | True |
| `1644500_219937_3936f33cf6675a0fa00f9e61ba7d797510b1024c.png` | 159.72 | 39.0 | False | True | True |
| `1644500_219938_bb3573c8b272974a81295520593457c38587316a.png` | 180.37 | 50.03 | False | True | True |
| `1644500_219939_21c034de20c080baee1673843508c6f67de58428.png` | 180.19 | 41.83 | False | False | False |
| `1644500_219940_05c364eaeae4df990c95680b944aaaf45e1c15c9.png` | 180.04 | 55.43 | False | True | True |
| `1644500_219941_c161c6c79f6103c0277b05b88a298dda038e07a5.png` | 73.41 | 23.23 | False | True | True |
| `1644500_219942_8d9e705389d589d3794d1fea329266d734f26580.png` | 180.14 | 48.79 | False | False | False |
| `1644500_219943_85a5d333a0aefbd4498800129ec20bea7a61fee1.png` | 180.10 | 40.53 | False | False | False |
| `1644500_219944_0c94fda1768f1cc8911ed057bd90cf090efd18d7.png` | 24.23 | 18.22 | False | True | True |
| `1644500_219945_f4ab60f4aacdb28d71ceadb340233fee20400b07.png` | 40.96 | 20.1 | False | False | False |
| `1660840_310307_65aedbada889e266ed65b260f2668983f5cc71f6.png` | 33.76 | 30.67 | False | True | True |
| `1677770_198316_c5bb68badc041c8abed44d6c771bf10b3faf67c9.png` | 26.20 | 14.8 | False | True | True |
| `1677770_198317_09d596bb0889a27927afe9572194088322fe4a8d.png` | 20.31 | 13.72 | False | True | True |
| `1677770_198318_5a4c21fad978f8b0c2f10ac95809a172a01b722c.png` | 20.98 | 11.61 | False | True | True |
| `1677770_198319_f7b7399f6e5a7a5e1b18579306f514a760c43858.png` | 26.61 | 26.43 | False | True | True |
| `1684670_167107_c5d693022a4000d9c45ffdb6bc38b8602a69b0b4.png` | 19.83 | 11.6 | False | True | True |
| `1693980_223076_3800c0193a48a8cc47f28ec133ddaefca3ee0137.png` | 68.76 | 21.3 | False | True | True |
| `1693980_223077_d485d8e244008fdcf5c97d1b557b1788bbdbaf74.png` | 61.79 | 25.6 | False | True | True |
| `1693980_223078_784643d307a5131fe8954f27eeaa7bb46d0a8c21.png` | 79.42 | 39.32 | False | True | True |
| `1693980_223079_ac8947b7a50e89e51d2b2e41f9ecfe483127042c.png` | 26.21 | 15.96 | False | True | True |
| `1693980_223080_b8608057a48217caa9fcfa74f10956191e0d2abe.png` | 61.17 | 29.87 | False | True | True |
| `1693980_223081_b1a83a3661ee50764ff6f20c94c00a715b2fffee.png` | 77.01 | 29.86 | False | True | True |
| `1693980_223082_e32d4de0c76d082ffe982ae45572ca8e603f80b6.png` | 53.88 | 36.03 | False | True | True |
| `1708520_181368_6a2fbad108b7bb2fad7b2a5fbc8fbc44588ee2cb.png` | 28.25 | 20.15 | False | True | True |
| `1708520_181369_b97a1b43cef5ec11d33fbd6e7548d03bb302e9ae.png` | 44.33 | 23.38 | False | True | True |
| `1708520_181371_cb8aa575f430a066832e4de59ddc8f20af96e689.png` | 36.14 | 27.55 | False | True | True |
| `1708520_181372_0745b330493e107d6b225ad940905d2e60b241e1.png` | 47.21 | 27.52 | False | True | True |
| `1708520_181373_a545613b6b5debdeb1aad205cda89629f758a7dc.png` | 20.02 | 16.09 | False | True | True |
| `1724050_155697_a49ef354be93b53353cbd70981972ad32dbb580b.png` | 58.17 | 52.99 | False | True | True |
| `1724050_155698_00bf66bc4648deaf5a99d1cda6f24bb1539b34b6.png` | 60.69 | 51.88 | False | True | True |
| `1750200_190778_988cc1c1cab855313123fec42308ab2f25489d86.png` | 79.89 | 76.16 | False | True | True |
| `1792490_147007_7e4b487ff5dd0d29bed459abf237e82472d54fff.png` | 22.35 | 15.86 | False | True | True |
| `1792490_147014_ebe0138bd34ca06de61c60b2a87ea49f0ca8533c.png` | 21.40 | 16.92 | False | True | True |
| `1812860_458938_af3b63de860ec515519aebacff2e87f7f000de7e.png` | 27.72 | 24.63 | False | True | True |
| `1830910_256337_1d1d6ecb8481108ac7fbe7071a995e0f1b3c3d6a.png` | 41.24 | 16.93 | False | True | True |
| `1830910_256338_28d5cffdde0a3d2c2066617bf42ef55ad8fdc276.png` | 25.36 | 12.67 | False | True | True |
| `1846860_149834_3dc53c81aefef949c13958c2bcf8d2d555d055ea.png` | 29.15 | 28.58 | False | True | True |
| `1846860_149838_959ac85596736bbcca9ab23652dad3c36f2a2263.png` | 72.89 | 71.0 | False | True | True |
| `1875580_2357858_0b2943d1729429a8766cb6e7849a565cdddd35c8.png` | 1.02 | 11.62 | True | False | False |
| `1875580_2357859_eab4330c294f7f9dfc262039ceaec689468cfff4.png` | 0.35 | 11.57 | True | False | False |
| `1875580_2357860_85346ebeded829d3c01ee7f66352cd7f11daf394.png` | 1.94 | 11.67 | True | True | False |
| `1875580_2357861_92f359b5c39313931a96dd3f38476e870c995485.png` | 0.60 | 11.57 | True | False | False |
| `1875580_2357862_03d03ba9b6b187a5c5564a3f0daa8ade2214e072.png` | 0.67 | 11.58 | True | False | False |
| `1875580_2357863_f5872f6e3fe7dc0fd394ae18565091af756aa53e.png` | 0.51 | 11.57 | True | False | False |
| `1875580_2357864_b6a59b399d5134b4d97800fc004a07537599267e.png` | 0.25 | 11.57 | True | False | False |
| `1875580_2357865_378a29f61b8f7e7a2a59157ea946f6118bdd66bc.png` | 0.61 | 11.57 | True | True | False |
| `1875580_2357866_ac9079f116c0e878571c3a0bd8a4d2607f5defac.png` | 0.24 | 11.57 | True | True | False |
| `1875580_2357867_c5a2f36b5a9d448db375d865cca518ce882b1d5f.png` | 1.34 | 11.6 | True | False | False |
| `1888930_314153_8109b8dcac6088ae78e68dbc71253841486583f5.png` | 33.23 | 31.42 | False | True | True |
| `1892420_249998_ac35fe06b1b8344adea29938e59311f56c9b6b4a.png` | 17.38 | 15.85 | False | True | True |
| `1934800_182708_a6896481303a21c2e215a15d2bc4f82963448f01.png` | 13.09 | 20.06 | True | False | False |
| `1942280_449380_13575a4b25c4e1789896baa7096a35a8d8d2e3e5.png` | 24.57 | 24.03 | False | False | False |
| `1942280_449381_bb0479e9c664a9e3f9188c29385be8f462c842ab.png` | 15.31 | 11.65 | False | True | True |
| `1942280_449382_020570f0af222d4d48afaa4b7e7defff6c106f48.png` | 11.25 | 12.02 | True | False | False |
| `1953540_250380_981c40bebc616e25ff897cf6bfbbbc4bfb1f539a.png` | 70.05 | 56.18 | False | True | True |
| `1989120_1758297_c0c2c588c1ef71d423783c2d95fd621ac2dec704.png` | 50.00 | 21.16 | False | True | True |
| `1989120_1758298_8b3903e43ac743d49a6c476302468d389239490c.png` | 77.57 | 38.07 | False | True | True |
| `1989120_1758299_2eefc1b93312449fca094613d4986f0f94a4ae06.png` | 76.04 | 40.2 | False | True | True |
| `1999520_337154_b9df3b032177b717ce46b988e88f9a4e363ada79.png` | 1.12 | 11.6 | True | False | False |
| `1999520_337156_32c2acde83968cd6d6bfa1ee9b9f11cec89f82c2.png` | 3.27 | 11.58 | True | False | True |
| `2012670_315342_25afa8ea89e7fe9808b1415537c0befc6fa84a3f.png` | 1.07 | 11.62 | True | False | False |
| `2012670_315350_9f678b629b69ef6b4d338157e605a84b2540c584.png` | 0.75 | 11.59 | True | True | False |
| `2012670_315351_bf00761a5e6ef594f1c08bfec6bb20494fed0b1d.png` | 0.75 | 11.59 | True | True | False |
| `2012670_315355_9e4cdbba96651e5374f54dab8c5d46a13b94b31a.png` | 0.94 | 11.61 | True | False | False |
| `2012670_315356_6fc82b36a0841df4706c1d7e23f07f2de73da397.png` | 1.41 | 11.62 | True | True | False |
| `2012670_315357_5167c6063da1620479c302073bd320f05a659315.png` | 1.35 | 11.62 | True | True | False |
| `2012670_315360_9c49d6afe4c64064656c92799d2c03575563d815.png` | 0.84 | 11.61 | True | False | False |
| `2012670_315361_5e376c7cafc58b9cb2885cbc3ec674340c3c4e09.png` | 1.07 | 11.57 | True | True | False |
| `2012670_315363_663e009dd6241040a526c86fee57dabae1fac486.png` | 1.31 | 11.57 | True | False | False |
| `2015270_454493_93cb921fea3946d6369c93130d1b7d4300d2af9f.png` | 17.92 | 11.63 | False | True | True |
| `2015270_454496_ed70ed9974c7e30c3a877ba572a0b4522d78f823.png` | 23.12 | 12.66 | False | True | True |
| `2015270_454498_86d96ae588c55c8271c5a6e6b142c6d1c71c9361.png` | 26.10 | 22.22 | False | True | True |
| `2021850_170467_987a8c1cb9aa4c1b85ab060dd4221ce8c7941f74.png` | 34.44 | 16.07 | False | True | True |
| `2022180_198703_9130afd0e19c22d71d10fa8d188ee9b8f8f4faa4.png` | 75.27 | 35.01 | False | True | True |
| `2022180_198704_cec4fa9eded24bbdff6c27316ddafd11259f5a2e.png` | 32.43 | 19.05 | False | True | True |
| `2022180_198705_8bc927e1969c238ebaf2f177990e4f5457d17a16.png` | 22.13 | 13.77 | False | False | False |
| `2022180_198706_12c61de3e5a1edfb535a5e8c05c4e338091a1e07.png` | 34.07 | 16.95 | False | True | True |
| `2022180_198707_e35b05cba52c6abe7756b5a3c0e6116980c872ea.png` | 29.00 | 23.31 | False | True | True |
| `2055050_299581_01f4dbbb935fc035650a8491eb9841de4fae62a1.png` | 18.20 | 17.94 | False | True | True |
| `2055500_197283_33d2097dc72924dd032d965b1e97ed85e0bf8083.png` | 1.02 | 11.6 | True | True | False |
| `2055500_197284_d7d244470dd058973bd1520255f62bf27aa95ea0.png` | 2.18 | 11.65 | True | False | False |
| `2055500_197285_177476365a62ada17f3e7c7d086f7df20bfc3fff.png` | 1.42 | 11.61 | True | False | False |
| `2055500_197286_caf787c0980eb9ef97bc89ba5468514390919ba4.png` | 3.01 | 11.7 | True | False | False |
| `2064610_432358_51cf22438d1438317f099933c530d593390d17cc.png` | 1.23 | 11.57 | True | True | False |
| `2064610_432359_6f17a8f9486cd4a92571f530787f383b414cc8c3.png` | 2.05 | 11.66 | True | False | True |
| `2064610_432360_dd3495081e68522373e5b6642fae4ca44260dc70.png` | 1.30 | 11.61 | True | False | True |
| `2064610_432366_133361ce90710c035593e281f1ea84770736c7da.png` | 1.62 | 11.64 | True | True | False |
| `2111550_4066031_7794b311a220e444a3438f0d0c6ed42d557b9baa.png` | 49.53 | 27.46 | False | True | True |
| `2157060_403707_be9b780238e6f8a8675b2a1420a873d70fc961db.png` | 1.46 | 11.61 | True | True | False |
| `2157060_403710_d987b503edd0c26d2bc3bf38d6f34c680cae31a9.png` | 0.41 | 11.57 | True | True | False |
| `2157830_457062_07d74c751d0bb3c165f1af306afc5c0fefdd03ba.png` | 12.35 | 11.64 | False | True | True |
| `2157830_457063_26db4a13918de1e2bf474f71b154466f995664c7.png` | 12.36 | 22.18 | True | False | False |
| `2157830_457065_fa55d480b0868164e0558d756100648e03e5bc16.png` | 11.11 | 21.14 | True | False | False |
| `2157830_457066_a37a321daba0e14c681ca7d30544af7a0a6be76b.png` | 6.92 | 20.07 | True | False | False |
| `2181610_258533_4087cd4d10001b37bc80aaf3c8d483ec10660f11.png` | 14.63 | 13.75 | False | True | True |
| `2181610_258534_fb74e9c4c8aaf11b9bc707d96eac731279ecf4f9.png` | 17.30 | 12.68 | False | True | True |
| `2181610_258538_b8a15ce0ad80aef3a64a3fb3c7dad391e9a1bb6b.png` | 19.39 | 17.98 | False | True | True |
| `2181930_215580_9c7072806cf3dc51861eadd06b30e81114a8ab99.png` | 52.33 | 19.05 | False | True | True |
| `2186680_313588_7a75709d56b60dda0e811a4c65234044d2f966c0.png` | 28.26 | 21.19 | False | True | True |
| `2186680_313589_7a95cb1a604cc5cf26c0fcd37cb852936830d444.png` | 30.25 | 24.39 | False | True | True |
| `2186680_313590_bc4a39c808d34cded2aa827369c9b9c72ad91235.png` | 24.39 | 16.94 | False | True | True |

## Historical stopped-state rows

| file | Default seconds | Max seconds | Default bytes saved | Max bytes saved | Default bits saved | Max bits saved |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| `1000470_342936_bf6e39bb1fa9a5dee23dded9ba02eb1735286123.png` | 5.78 | 11.88 | 1299 | 1398 | 8877 | 9681 |
| `1000470_342938_35cafb8049a24395659efd3ea61944c6c6bf9128.png` | 0.74 | 11.58 | 69 | 106 | 111 | 408 |
| `1000770_159418_c0a796c1104f1b26c4ac274ff419a7d00c777c5f.png` | 0.40 | 11.58 | 133 | 252 | 1075 | 2015 |
| `1000770_159419_5f315a3c07e81c0c668ca503bc50d4929d4176f8.png` | 1.93 | 11.58 | 75 | 138 | 603 | 1103 |
| `1012790_244362_ff68e3acb0f0bf94138c9521b6dd51b0a53c0938.png` | 8.10 | 15.86 | 538 | 854 | 4254 | 6793 |
| `1012790_244363_2f82a411e061b44133ae02ad92c90ea1bd1014b6.png` | 10.18 | 15.88 | 198 | 2923 | 1560 | 23340 |
| `1012790_244364_07998cd20b1f60747bc3d4d7e3c9e13c90631846.png` | 35.29 | 34.89 | 2806 | 2795 | 22435 | 22362 |
| `1012790_244365_b1ba2397827f7f11feda367fdf36393ab2cc8523.png` | 9.23 | 16.92 | 304 | 2407 | 2433 | 19246 |
| `1012790_244366_c0d4124ddc6da444d2b1cae600b4bc088fcc3a81.png` | 1.03 | 11.61 | 21 | 2067 | 157 | 16544 |
| `1012790_244367_9b44d3e1dad2eb9be6d884695bef73e85e776a97.png` | 0.50 | 11.58 | 15 | 69 | 117 | 551 |
| `1012880_117136_4f565381630fd0c387c2d5d12ef1175bd508e599.png` | 2.27 | 11.58 | 72 | 106 | 574 | 856 |
| `1012880_117144_b7d14bf0d4c0e550b032a11c30c1e5f736f69733.png` | 3.00 | 11.59 | 31 | 220 | 252 | 1752 |
| `1012880_117145_66445e9863c5a207a8bce761e87a276853a4c8ca.png` | 5.69 | 11.59 | 58 | 196 | 467 | 1555 |
| `1012880_117146_dd37d1f819b106b0cd0d27913ff077c3f592dcb6.png` | 0.72 | 11.58 | 2 | 84 | 18 | 668 |
| `1012880_117147_ff64e13dd62eec3025fbc35cc160f110994b0b4a.png` | 1.54 | 11.59 | 85 | 423 | 684 | 3405 |
| `1012880_117148_69a202aca0b1dd9774f793edf0b4ca0bcaa1bf03.png` | 1.82 | 11.59 | 77 | 338 | 617 | 2738 |
| `1016180_195793_5e51d7d4681ccb4bf297035dbcf0cf9b41dfc9da.png` | 0.70 | 11.57 | 16 | 8 | 155 | 88 |
| `1016180_195794_441802412d55178cf51817324e7004deae7e0db6.png` | 0.71 | 11.60 | 4 | 4 | 20 | 26 |
| `1016180_195795_c139db3e4ed18fbe65577884c2e0e5a4f622fd5a.png` | 0.75 | 11.57 | 2 | 1 | 27 | 20 |
| `1025600_359593_249f89d7fe23f0e87e3b3eb163e5821730b2367e.png` | 31.07 | 15.84 | 2293 | 3174 | 18277 | 25355 |
| `102840_103124_b6257c5e0ab6bd282193b539457e94d584c7d086.png` | 2.28 | 11.59 | 12 | 224 | 107 | 1783 |
| `102840_103125_4906c9b4c2940fc3a3ecb6b1b59c8676f8f07397.png` | 19.16 | 11.61 | 172 | 906 | 1396 | 7262 |
| `1033950_150457_ca002c58b57300f8c0050687013fb6a17bc9e42b.png` | 1.53 | 11.61 | 25 | 46 | 194 | 366 |
| `1033950_150460_13627a6d7153139d38a508c04753a3c9d4a41bf6.png` | 6.61 | 11.66 | 35 | 39 | 288 | 301 |
| `1033950_150463_a1ce510bdb8fde2c6d0edb737a1368111dd65719.png` | 2.24 | 11.58 | 76 | 172 | 605 | 1378 |
| `1033950_150468_7790a008bb2b69409fd74481a5357155af0e53de.png` | 1.56 | 11.58 | 9 | 22 | 51 | 178 |
| `1037910_115268_3641f738148495adff51bcd051940413da16b9cb.png` | 6.01 | 11.67 | 135 | 395 | 1076 | 3175 |
| `1037910_115269_ffe47fa363a5cc2528fa3131e26882f3e1825916.png` | 2.96 | 11.59 | 41 | 128 | 333 | 1035 |
| `1037910_115270_3a514e239db5073285b0c5fc53d0b7858b5ced2c.png` | 12.39 | 11.63 | 59 | 765 | 471 | 6117 |
| `1037910_115271_defdc155767fd9d7ed1c1e92fc02d9a31d974476.png` | 5.69 | 11.65 | 73 | 690 | 590 | 5514 |
| `1038300_116089_f6193928f29562de6289cda18552853a88aa073b.png` | 7.10 | 25.35 | 309 | 1815 | 2467 | 14503 |
| `1038300_116090_5906e63eefdbe4db46396180ff9ba1d92aaf304a.png` | 3.14 | 11.63 | 112 | 784 | 905 | 6277 |
| `1038300_116091_928ac15a9de009d538d7ee0af68ac61640b339eb.png` | 10.10 | 26.40 | 529 | 2726 | 4225 | 21789 |
| `1038300_116092_035da9b323ad7457de5cf804fd6f60713eaabdbf.png` | 7.50 | 25.36 | 590 | 2159 | 4723 | 17266 |
| `1038300_116093_6158db2d2c7c37c715d461a5c61eb4cf9751c34a.png` | 13.10 | 29.59 | 3704 | 5279 | 29657 | 42244 |
| `1038300_116094_80da5741dabf8f874eb69cd697009a21cb7aa0ba.png` | 10.22 | 29.58 | 502 | 2275 | 3985 | 18176 |
| `1038300_116095_983765a2b4845746dfbce4f5d8cf955ddfb86c9d.png` | 5.72 | 22.17 | 782 | 2034 | 6247 | 16261 |
| `1038300_116096_5ec9f0b41249443a8e40b74ce8fb0b3807314b6d.png` | 6.90 | 29.56 | 2766 | 4117 | 22114 | 32941 |
| `1039280_227558_96fe783a54fe25d9f9eb1f6b9d6bd69349dd4f2a.png` | 7.66 | 12.65 | 111 | 357 | 883 | 2862 |
| `1039280_227559_baa7c59a253b57faefdc3623b38ba6d810fad02f.png` | 3.04 | 11.58 | 76 | 217 | 613 | 1728 |
| `1039280_227560_70f90c442b23e47e5c46e5ba4006d5993a4c5ab6.png` | 1.70 | 11.59 | 153 | 211 | 1291 | 1758 |
| `1039280_227561_7b676a3e5a957a762ad807294794e4f4feb0efff.png` | 2.54 | 11.59 | 37 | 166 | 297 | 1343 |
| `1039280_227562_6dd86edfa79e953aabf2b50ba539b90f0fd87d01.png` | 0.47 | 11.58 | 11 | 111 | 88 | 886 |
| `1039280_254113_8b712f5729817ced42a62444c10ea01d246266c1.png` | 3.72 | 11.59 | 468 | 583 | 3740 | 4657 |
| `1040230_104601_1af9ec2d57c30cded1fbf5b8189b885727022ef2.png` | 14.89 | 21.13 | 215 | 248 | 1709 | 1972 |
| `1040230_104602_d25f45e173984f20c35c4e67e6688755df2c14f9.png` | 11.32 | 14.82 | 107 | 503 | 846 | 4014 |
| `1040230_104603_38406fb54b632f3c542c69666f58da9828333586.png` | 35.07 | 37.01 | 382 | 714 | 3034 | 5695 |
| `1051690_224705_90b8e049754d4a7c11944c4733d97d2798eeb486.png` | 0.66 | 11.58 | 6 | 34 | 49 | 266 |
| `1051690_224706_2d2cbf6a7ecd593059ec92d25a47a404cf77da47.png` | 2.28 | 11.69 | 32 | 35 | 283 | 309 |
| `1051690_224707_f19fff308e1b4e7508846063107c7d3872b453df.png` | 1.81 | 11.65 | 15 | 13 | 131 | 102 |
| `1051690_224708_b27a6053aac8aa73acf8c27baebb3946b8c093f4.png` | 0.70 | 11.57 | 4 | 30 | 39 | 246 |
| `1051690_224709_35ea110528bf99393e129c619c1413834728bf3c.png` | 0.79 | 11.58 | 3 | 41 | 27 | 328 |
| `1054490_103547_c75c90bd92570b27ac8f58322a60d4661256b8a0.png` | 6.49 | 24.29 | 2754 | 2803 | 22041 | 22422 |
| `1054490_103548_f94e3d6f73eac8480a0251070b85aff6544f1397.png` | 7.56 | 19.01 | 3002 | 3144 | 24003 | 25141 |
| `1054490_103549_be308c7dc1c66670bf570eac884201571856e639.png` | 8.36 | 24.30 | 2041 | 2234 | 16334 | 17877 |
| `1054490_103550_058c0263d6f21fe02e141b3552c4df86b853a9d9.png` | 8.85 | 28.52 | 3930 | 4313 | 31427 | 34494 |
| `1054490_263249_bd9ae5d6a8309c3cd813286426e5d08bb8f1876c.png` | 10.02 | 17.94 | 1271 | 1307 | 9430 | 9721 |
| `1054490_398728_a24d9184aab64608d437ddf6602010e047ad212c.png` | 6.39 | 11.61 | 1515 | 2058 | 12106 | 16460 |
| `1054490_398736_5923fe17976132284cc49e27d15bf0c787c46181.png` | 3.79 | 11.59 | 49 | 97 | 396 | 776 |
| `105600_79913_f7bbf0a8dd394260ed9fff2464ad4f39d96225ef.png` | 12.48 | 11.61 | 94 | 195 | 740 | 1544 |
| `105600_79914_81a5deabbd5da0d668b046faa2455333771e4eb6.png` | 3.08 | 11.60 | 591 | 1219 | 4721 | 9769 |
| `1057780_353006_50543dd5767b17246a281f16916de00a3deb2f63.png` | 1.39 | 11.59 | 44 | 45 | 344 | 336 |
| `1061180_104374_cf3c65cbbef7c85adadb50cca353b2b435b64716.png` | 11.74 | 19.05 | 1286 | 1300 | 10298 | 10395 |
| `1061180_104379_f5f36d0a660b3b766e3d445be0218110c1bb5106.png` | 15.41 | 19.03 | 1342 | 1506 | 10740 | 12051 |
| `1061180_104385_f45506ec0f99799b0cd653366f0f01d4e6272c94.png` | 42.09 | 30.69 | 1847 | 2039 | 14818 | 16370 |
| `1062830_143619_afc38fdaea3c04b9ed8a99ab2debcaac633481c3.png` | 67.66 | 59.74 | 392 | 731 | 3103 | 5859 |
| `1062830_143620_d3e2149371c2407fcb4e60376facc98294d8e1c5.png` | 90.36 | 89.87 | 627 | 2243 | 5070 | 17961 |
| `1062830_143621_90c7ead375e27e4c99dc38e85b4a40238e03168e.png` | 45.79 | 51.39 | 148 | 1232 | 1186 | 9839 |
| `1062830_143626_6b9788353d3cbb4786239aa54a6a5db349c5fa5f.png` | 66.55 | 68.98 | 409 | 2401 | 3246 | 19170 |
| `1062830_143627_de12cd64d3ebed1c3fb42528422f8ee5a83a00e9.png` | 85.89 | 74.24 | 633 | 930 | 5075 | 7436 |
| `1065970_147780_4b61b9134e089780c87318e296ca6207f6f70567.png` | 30.03 | 68.74 | 380 | 453 | 3036 | 3640 |
| `1065970_147781_949a58d53630a6aac519f5e1fcd5880c2c3800cf.png` | 24.72 | 53.95 | 257 | 653 | 2078 | 5231 |
| `1065970_147782_8a38f64b336e0543819538a023b2ae9454e1c665.png` | 17.03 | 27.51 | 63 | 610 | 503 | 4878 |
| `1069740_103152_0a9cac42434539b34c61903679f2ba29d439db61.png` | 4.01 | 15.87 | 420 | 1213 | 3343 | 9705 |
| `1069740_103157_767e2335ee7cf5dc63a31678de938322b0a0c936.png` | 0.87 | 11.59 | 135 | 343 | 1063 | 2733 |
| `1069740_103158_29431a11b23215172f15d4653ea38f7ddfe01f49.png` | 3.67 | 11.64 | 122 | 1244 | 975 | 9962 |
| `1069740_103159_52f9c1723d7881f766f6b3697782cf1d78df5c9f.png` | 9.74 | 19.01 | 977 | 1510 | 7826 | 12072 |
| `1069740_103160_1878f87cd53d63c77e6b3eef2a1f7e1dbe4bac25.png` | 2.83 | 15.86 | 141 | 442 | 1126 | 3527 |
| `1069740_103161_13c233d573521421262b1ed5f534171217fe28d6.png` | 1.99 | 15.89 | 408 | 959 | 3255 | 7646 |
| `1070330_198280_3523684ad6df04b591382f889f1024c730b1bad2.png` | 3.96 | 19.06 | 489 | 2250 | 3928 | 18019 |
| `1070330_198281_0a13c738b9698a89c09f4a83a671f3373d1eade6.png` | 3.51 | 16.93 | 822 | 2910 | 6570 | 23273 |
| `1070330_198282_501881354f6c7f772cdbe08fe2d3db9e8f83e792.png` | 4.01 | 13.80 | 509 | 2362 | 4053 | 18887 |
| `1070330_198283_4ce01ab86ac95878fe34f045ed09a9fb79cd17c3.png` | 27.52 | 19.10 | 1336 | 1433 | 10675 | 11457 |
| `1070330_198284_1c14f45222806ad666d2f1d31b0555e85ef448e9.png` | 10.73 | 22.26 | 2862 | 3840 | 22901 | 30740 |
| `1070330_198285_1ea99ac49c2e0712b2a33c36bec088fb13794f12.png` | 5.19 | 11.62 | 194 | 1244 | 1564 | 9950 |
| `1071220_111384_4a93d0d2051e6f2ff0bfd5e45926dd80db89ef9b.png` | 1.33 | 11.58 | 7 | 26 | 51 | 209 |
| `1071220_111392_294e16fb37b8e0ff94696e53170f4c8a3696a4aa.png` | 0.39 | 11.58 | 2 | 36 | 24 | 289 |
| `1073810_105834_880a33fbcd8b79da3307349e7a4e2adeec08d5dd.png` | 17.58 | 20.13 | 164 | 7205 | 1305 | 57656 |
| `1075550_135986_e22dc88738f1e6487b8221757a2e74971395f82a.png` | 1.29 | 11.64 | 6 | 4 | 46 | 40 |
| `1075550_135987_f32ce3740cc6a6ecb518ba19e3f28dddce302cef.png` | 0.80 | 11.60 | 4 | 9 | 35 | 66 |
| `1075550_135988_46383bbf9ae4c677bf909963decbd2faf2f64e16.png` | 1.09 | 11.63 | 6 | 7 | 48 | 62 |
| `1075550_135989_66925b82e6d634e7b87a6bde719c743eeca76a4c.png` | 1.34 | 11.61 | 8 | 9 | 68 | 71 |
| `1075550_135990_898eb1b77073b85589432cb34aa7f0d87dee4f1f.png` | 1.73 | 11.65 | 16 | 24 | 143 | 214 |
| `1075550_135991_651491874c5b8eb0d2d5958f859ceb325cf003e9.png` | 0.84 | 11.61 | 11 | 13 | 101 | 95 |
| `1083790_256251_5994659bd9da6fb2233915d8d0a8022b7d649a1e.png` | 5.28 | 11.59 | 179 | 412 | 1429 | 3297 |
| `1083790_256252_9996658fa8481106155327a197310c212077ee99.png` | 1.98 | 11.59 | 13 | 217 | 114 | 1738 |
| `1083790_256253_f8f129fb7ee4fee39f1cc0ffa3c18bb36b29b042.png` | 1.88 | 11.58 | 20 | 111 | 162 | 886 |
| `1084600_283291_c010ad6826fa980a15237c8ace62451c16d2bd5c.png` | 2.67 | 12.64 | 2866 | 3040 | 22922 | 24318 |
| `1084600_283292_7642eb06030e53dc9f2b0d1382328f6ad28a6e7d.png` | 4.99 | 11.60 | 1352 | 1381 | 10832 | 11065 |
| `1084600_283293_861ce68a84f4d3e12db95080b7c5f4113986449b.png` | 8.59 | 27.46 | 6097 | 6369 | 48757 | 50945 |
| `1084600_283294_61b25ee87b4cd25cf4d3287d47571a57da4981e4.png` | 14.39 | 16.91 | 1717 | 1732 | 13736 | 13860 |
| `1084600_283295_97175e6ac807539fe60433b280ba0a6db612c907.png` | 7.07 | 28.51 | 8080 | 8278 | 64627 | 66223 |
| `1084600_283296_0bbd5a4fa9d24476ae642909f4f858bd5bd306a4.png` | 7.95 | 25.35 | 5912 | 6094 | 47300 | 48757 |
| `1084600_283297_fdc0702b02ee422dce4b8390fc0be1a258891e3c.png` | 4.35 | 16.88 | 3668 | 3757 | 29321 | 30033 |
| `1084600_283298_d9638149cbaf8ff21c8659348c342e3f360be4e7.png` | 4.58 | 15.83 | 4020 | 4219 | 32157 | 33745 |
| `1085260_288655_3a911a877b4f815b5b97552cb18acd2b928f811c.png` | 50.50 | 16.95 | 2934 | 3755 | 23456 | 30041 |
| `1085750_106608_dc0de8894cf2b62ab15ecca70317386c3b0c469d.png` | 7.32 | 11.60 | 71 | 666 | 575 | 5325 |
| `1085750_106609_cc58e4ab7e8976f43dde9b7f7bdb500e8938d4f0.png` | 7.84 | 22.19 | 987 | 1168 | 7909 | 9361 |
| `1085750_106610_60360e7aa0bf52ae34cebc8c6bd49971b6fe0b38.png` | 12.09 | 11.60 | 308 | 340 | 2462 | 2713 |
| `1085750_106611_412db28a887e7b89074e1c466ed7fadbe4221ef6.png` | 7.27 | 11.59 | 123 | 431 | 983 | 3447 |
| `1085750_106612_4f9f544315a08df5d6cdec889f7d2d2f48ef59e3.png` | 8.17 | 21.13 | 543 | 617 | 4331 | 4934 |
| `1091500_102625_0ba74e973d94ddef6351d300cfe575eceba9980c.png` | 180.87 | 135.04 | 45155 | 48851 | 361237 | 390782 |
| `1091500_102626_e0c99e66acaccb75e0349e37665abb0df7b41c28.png` | 16.79 | 53.94 | 5711 | 7300 | 45710 | 58419 |
| `1091500_102627_35b64542c59b6c62ff3383320a685d49dfe80d7c.png` | 35.84 | 89.87 | 10889 | 11451 | 87082 | 91584 |
| `1091500_102628_fdf3a2a17a20d71313d09af49b69912f5dcc781a.png` | 29.63 | 83.54 | 4673 | 8580 | 37441 | 68657 |
| `1091500_102629_bc003c956445fe939986abf0ccfd903eec0de22b.png` | 22.86 | 63.46 | 6898 | 7042 | 55151 | 56310 |
| `1091500_102630_da5fd9fc9ad72819e50a227bba95c3522716b0e9.png` | 10.22 | 31.70 | 1910 | 3269 | 15265 | 26146 |
| `1091500_102631_eef5c8bdc4a546dbf0f53eeb961aad28f7282499.png` | 41.59 | 115.25 | 10185 | 10327 | 81503 | 82634 |
| `1091500_102632_5dd60cc4b51b2c9944204d7442d2d204acc3efee.png` | 28.61 | 105.73 | 11151 | 12495 | 89242 | 100005 |
| `1102880_219353_b58d19dcc1c1de72ad0cde33d17e815207c8ca2d.png` | 2.08 | 11.66 | 6 | 12 | 50 | 98 |
| `1102880_219354_10b8ba16e332d88f520bf4514db3c081cb82f309.png` | 1.75 | 11.64 | 8 | 13 | 70 | 104 |
| `1102880_219355_7df1c5bac38902a4032a11de415472c6dd598428.png` | 0.78 | 11.60 | 6 | 5 | 37 | 31 |
| `1102880_219356_f8d00dd10b43785d059474f2b70a49e1ddee805c.png` | 0.35 | 11.57 | 1 | 1 | 8 | 6 |
| `1102880_219357_1e68254fc8c64e692c07494f61b416134c8d7bb3.png` | 0.64 | 11.60 | 2 | 5 | 9 | 33 |
| `1102880_219358_c947615af850fc1fe46858c0d2a7da6baf958b51.png` | 0.77 | 11.61 | 4 | 4 | 27 | 28 |
| `1102880_219359_747762748f87f3a834b3ec5f7b1f92864d99fbaf.png` | 0.61 | 11.60 | 3 | 3 | 15 | 19 |
| `1102880_219360_50a9dec58ab0cbc18ee436319a0ad6210a73437b.png` | 0.80 | 11.61 | 1 | 8 | 13 | 68 |
| `1117090_253496_4296c9a54009f8477f0e4ebf650d6e971b4355ca.png` | 3.49 | 14.77 | 802 | 2223 | 6394 | 17789 |
| `1117090_253497_3875daab5bda31a689bd0d55b52948b41b93ce30.png` | 4.15 | 11.63 | 980 | 1881 | 7827 | 15053 |
| `1117380_103424_3183f479934bb3e6c4bf067486ecb85e084284d8.png` | 1.70 | 11.59 | 535 | 916 | 4278 | 7368 |
| `1117380_103427_a605d0693d853a7ecf2fbde154d28a8822e40c58.png` | 2.78 | 11.59 | 191 | 717 | 1522 | 5723 |
| `1117380_103428_9fcf9f826b720885041be82ecbe15dabf90135ab.png` | 0.83 | 11.58 | 8 | 178 | 61 | 1427 |
| `1117380_103429_43119226810ffa42a70b066c8e292e7a383da959.png` | 1.11 | 11.58 | 64 | 273 | 532 | 2184 |
| `1121640_215797_36ae6848998580a79a682092eba47dd129b54cfc.png` | 1.37 | 11.60 | 99 | 377 | 810 | 3033 |
| `1121640_215798_b527f9f0a076998a64d6e29a284f45fc58a879f2.png` | 1.57 | 11.59 | 95 | 633 | 766 | 5064 |
| `1121640_215799_896e2c0b2bf3527af2783cecb61aa6a861ed519e.png` | 2.63 | 11.60 | 181 | 425 | 1445 | 3400 |
| `1121640_215800_76f7af8259bac9851b17e8b2aa8a133d71d55be0.png` | 1.12 | 11.59 | 129 | 680 | 1028 | 5423 |
| `1121640_215801_089306ce01d071ed3ea1fc7325a7e042704bddc2.png` | 1.17 | 11.59 | 16 | 338 | 125 | 2716 |
| `1121640_215802_8f6bb1eb8f96ebda408998274fe72bf57d27d2d7.png` | 0.98 | 11.59 | 84 | 409 | 666 | 3275 |
| `1121640_215803_b6a49b3bc2cd25fc8dea29fd9f523707dc364caf.png` | 1.18 | 11.60 | 143 | 501 | 1131 | 4005 |
| `1121640_215804_91a8edf22732f814af09b90c066b810f86ab4562.png` | 1.32 | 11.59 | 48 | 194 | 401 | 1552 |
| `1124660_151462_3777c946a544eb5350f19824ac416479b137f641.png` | 38.44 | 25.37 | 383 | 1271 | 3110 | 10235 |
| `1124660_151463_60a2dfe74bc81e4f277e96898117c8d4a19b6184.png` | 54.84 | 23.24 | 401 | 857 | 3238 | 6830 |
| `1124660_151464_ce424c481fd5b5ce4f6003273bee02145d3c3019.png` | 35.24 | 20.05 | 268 | 515 | 2119 | 4137 |
| `1124660_151465_0a96e433bfc675e9aa2e3405d6f2a879d69f0456.png` | 10.53 | 11.59 | 235 | 594 | 1901 | 4747 |
| `1127700_286467_f256f3c8b298884a5a2173bf10d3608ef321917c.png` | 6.15 | 11.60 | 299 | 579 | 2299 | 4530 |
| `1127700_286494_7c89fa0370c49962a3b0acfb4c260b057fbd1fa2.png` | 3.95 | 11.59 | 352 | 416 | 2723 | 3236 |
| `1127700_286495_8fe0009e48649c7a65a159649538b8cbdbcee630.png` | 21.80 | 11.64 | 295 | 745 | 2395 | 5996 |
| `1127700_286496_a1639472a74a782b6c5f4a93365f58674fd6c49c.png` | 1.22 | 11.61 | 17 | 2906 | 141 | 23233 |
| `1127700_286497_09be733fd5a316e166b1f603d335fdcf78ce4cee.png` | 3.65 | 12.66 | 562 | 801 | 4392 | 6304 |
| `1127700_286498_92175d67541708344caae4ddfe90d3f8c2801eb4.png` | 7.38 | 11.59 | 233 | 342 | 1755 | 2630 |
| `1145350_430379_1eb625b538596c7a3a9ea62c9756fdb5fab6f9a1.png` | 16.01 | 21.11 | 311 | 1157 | 2493 | 9266 |
| `1145350_430380_716e9f5b0c7890a5098e852ed7a2a46f190d1fd7.png` | 22.34 | 12.76 | 147 | 493 | 1195 | 3959 |
| `1145350_430381_898ab2cf0fa498601b7041f8a7908cc340f85120.png` | 38.78 | 17.96 | 476 | 1002 | 3822 | 8004 |
| `1145350_430382_c6b713bb3d2330bcd9189be4df38edd8b3f67eb0.png` | 42.23 | 17.95 | 1099 | 1305 | 8827 | 10472 |
| `1145350_430383_85d1c487bb77f7c4650b6321a347ab027087a7fc.png` | 17.06 | 16.89 | 151 | 907 | 1189 | 7280 |
| `1145350_430384_5ef39689763eaba063043adee0e65fc47992ae0e.png` | 17.03 | 16.90 | 162 | 1288 | 1314 | 10341 |
| `1145350_430385_87de4319b2a755ef773f16ea2a648634ec125d6c.png` | 28.73 | 18.99 | 231 | 727 | 1862 | 5824 |
| `1145350_430386_f951a5510c4ef847f49677928b8115c11afe6dc8.png` | 46.29 | 21.14 | 379 | 1035 | 3062 | 8291 |
| `1145350_430387_e407fa0bca812b0f2a89c6a52ca270bac8bde95f.png` | 22.02 | 16.89 | 1525 | 1828 | 12227 | 14647 |
| `1145350_430388_b7b7e73475647bb0cbd146eb6f349c53c5883388.png` | 34.18 | 19.02 | 1301 | 1651 | 10379 | 13228 |
| `1145360_100597_b6fb1ea388343548cb9bc238335a90232e1aa5a9.png` | 18.26 | 47.65 | 398 | 764 | 3170 | 6120 |
| `1145360_100598_8fad6e68d1d7ccaa20b6cc2d1da7cd34808d5d54.png` | 41.36 | 79.32 | 1622 | 3897 | 12966 | 31184 |
| `1145360_100599_e1b6f25fa190d4b7ed85b72ef93f85fc0455f84a.png` | 34.30 | 40.20 | 203 | 885 | 1616 | 7062 |
| `1145360_100600_0dd24aa67496f1716891cbe6452ee110a384027d.png` | 21.83 | 47.68 | 198 | 1647 | 1550 | 13154 |
| `1145360_100601_ef800127fe6d26435a9b26b63e47f74a21b451e5.png` | 29.00 | 44.44 | 367 | 1261 | 2972 | 10114 |
| `1145360_100602_7e181897b51a2d1cb6ed65de09cfd3578fdf54ec.png` | 30.70 | 60.27 | 287 | 2722 | 2297 | 21805 |
| `1145360_100603_9e7fc91cc4e1934908c179f9babcd84faebd1acc.png` | 23.70 | 39.21 | 441 | 958 | 3502 | 7684 |
| `1145360_100604_2952680ae01f0b7681872dbf9be9f416b90496ff.png` | 17.48 | 45.65 | 297 | 1042 | 2394 | 8352 |
| `1145360_100605_d6dbbfc5cbffbb5e2a668467023d21eb51c0ac05.png` | 50.86 | 57.18 | 1590 | 4152 | 12732 | 33228 |
| `1145360_100606_db6144fa6b4cf2dcacd9d2812b652ee27991a551.png` | 33.08 | 41.31 | 655 | 1873 | 5236 | 14977 |
| `1146170_167614_10c462e3bf095e045568e1be3d67ee9401441328.png` | 1.24 | 11.62 | 7 | 8 | 56 | 66 |
| `1146170_167624_7d6c8343d60198e8690ce4f0b29a6a10d19a329a.png` | 1.84 | 11.67 | 5 | 7 | 54 | 55 |
| `1150090_241706_3be1a7fb0cdd5b68b8783a3a6ccca7130c0d8cda.png` | 9.44 | 27.46 | 1712 | 1843 | 13708 | 14757 |
| `1150090_241707_b20f789a4dd2e99c50dcf7da028b0849c91f1476.png` | 8.08 | 28.64 | 2441 | 2566 | 19515 | 20512 |
| `1159090_282933_07e6d73f86a49959c3eb00bbfaa0b8044e11ee7b.png` | 4.48 | 11.60 | 112 | 466 | 895 | 3735 |
| `1159090_282934_5fe9d22c4eff740bcf69cf5c5118c949fac661b4.png` | 34.67 | 30.76 | 371 | 650 | 2936 | 5220 |
| `1159090_282935_2882511770a883b90c56e8c58f179a7ad0dac204.png` | 0.94 | 11.58 | 110 | 187 | 872 | 1486 |
| `1159090_282936_eb95175dcb2c86ed981dc19f0b181a61ca91937a.png` | 20.94 | 23.04 | 372 | 513 | 2929 | 4036 |
| `1159090_282937_2381d7211f26f2ab5f5b34544e5989572f19c6e0.png` | 70.20 | 49.78 | 3727 | 3759 | 29794 | 30111 |
| `1159090_282938_af290b7ac21bfe35ef93cd9bf31fa4dbd4642e43.png` | 16.80 | 28.91 | 267 | 338 | 2182 | 2771 |
| `1172470_101526_074087093d2d61d1b1480d250db5ced4b77293a0.png` | 25.73 | 83.61 | 10336 | 10340 | 75905 | 75928 |
| `1172470_101527_ee39f953ea5a67180ab8cb8a5d5c8ee61025e29a.png` | 27.90 | 100.50 | 14814 | 14852 | 111808 | 112114 |
| `1172470_101528_bc58c31ac31d4c4d89379dee0d9de5550642b543.png` | 34.28 | 131.23 | 15525 | 15565 | 117403 | 117711 |
| `1172470_101529_5953056809b16dbf00669417fbff185d3e2fd036.png` | 64.80 | 134.43 | 16227 | 16311 | 123083 | 123750 |
| `1172470_101530_5515c0c9c02087a5dfc022e441fc6fc748b585d6.png` | 61.45 | 97.59 | 17903 | 17980 | 136525 | 137119 |
| `1172470_101531_69b7e186fe8614fb5a30b73862dfb0185eb3a63d.png` | 57.60 | 103.68 | 13943 | 14506 | 111564 | 116074 |
| `1172470_101532_c283f0c4b37426a941d1555815cd9f10da53e2ea.png` | 38.43 | 139.58 | 16530 | 16537 | 125459 | 125517 |
| `1172470_101533_a76393521ab3f6ddbadfc9fe03432d7ea8b71f73.png` | 33.95 | 74.10 | 10813 | 10874 | 86501 | 86987 |
| `1172470_101534_18f5987644cc8176a33e941c00b8677419394d00.png` | 43.23 | 82.53 | 10145 | 10160 | 81046 | 81167 |
| `1172470_101535_54d64780205d5d6f457fe20983c0d4b92815c318.png` | 47.89 | 91.04 | 8990 | 9020 | 71926 | 72144 |
| `1172470_101536_f7a5d19719730ac365b5e38165796a442d555002.png` | 36.01 | 83.60 | 12893 | 12916 | 96401 | 96607 |
| `1172470_101537_b69ddcefb7d8d1de7f2554cae900fe9c0bf08a99.png` | 33.53 | 141.69 | 14917 | 14917 | 112587 | 112587 |
| `1172470_101538_02a3dec41b794ba2320ab2b07f3ed3eae93cf244.png` | 57.24 | 109.01 | 8701 | 8710 | 62815 | 62890 |
| `1172470_101539_96e0aed479e4a34beda1a4206bb0380f5d96a009.png` | 56.17 | 93.16 | 10530 | 10619 | 77474 | 78172 |
| `1176710_142387_9e18d4a508bd4d743dcc7af8c96aebd754cd739f.png` | 42.03 | 68.79 | 634 | 1322 | 5053 | 10557 |
| `1176710_142388_355109cf4f0270939f9acad2bc6b8d9f4205894b.png` | 63.92 | 62.87 | 817 | 2978 | 6546 | 23823 |
| `1176710_142389_5db69f0bffd84fcd25faae26455ee7090d6e8f49.png` | 55.91 | 48.78 | 406 | 1286 | 3271 | 10298 |
| `1177980_2962945_c4e88f645c7b0ba1633287089872aa9e884351cb.png` | 6.99 | 20.07 | 7989 | 8213 | 63826 | 65619 |
| `1177980_2962946_c2655b993181875630519ae151855698409026cc.png` | 9.79 | 11.61 | 131 | 623 | 1088 | 5001 |
| `1177980_2962947_4f777a1a5873671a08b9591e355eeefcea6bc4f9.png` | 6.71 | 11.60 | 149 | 566 | 1222 | 4558 |
| `1177980_2962948_883d5fb4a41bba5e90293abde38bab2f7c7c44a3.png` | 10.65 | 11.62 | 93 | 455 | 727 | 3627 |
| `1177980_2962949_8f59f8ef620101785eeb48f3f4e5251db5ead2ba.png` | 6.47 | 11.59 | 99 | 364 | 796 | 2925 |
| `1189220_399387_f1441891cfef0f7dadec6cbf05ea871da970b51b.png` | 0.91 | 11.65 | 6 | 8 | 49 | 55 |
| `1189220_399388_8677785d0e2cce653acbbabdb0670e04ffb42797.png` | 2.27 | 11.74 | 17 | 21 | 147 | 169 |
| `1189220_399389_fceb79942574f16374a02c3f2990829ac6c74f5e.png` | 0.64 | 11.61 | 0 | 1 | 4 | 11 |
| `1190970_313724_422f9c568773468454dee2836eadae5df95f0565.png` | 3.05 | 11.59 | 50 | 220 | 394 | 1759 |
| `1190970_313725_8aeb3ce9f7ca1676e643ede15396c42d5660e0c6.png` | 3.13 | 11.60 | 25 | 259 | 204 | 2082 |
| `1190970_313726_40c4b593455f914ba990d6d55c33bd9c5ceadbfc.png` | 26.17 | 21.16 | 394 | 811 | 3143 | 6506 |
| `1190970_313731_cd0c99024d937da6a365a4302b19e73c883458f8.png` | 5.51 | 14.77 | 247 | 321 | 1977 | 2580 |
| `1190970_3845995_47450882768b9533c38e0b9b07b2a988f3148826.png` | 18.69 | 25.40 | 909 | 1640 | 7255 | 13120 |
| `1191660_277350_154e9bcb004849a1a3f8d819cb30291fccd350a4.png` | 2.02 | 11.63 | 9 | 9 | 72 | 64 |
| `1203220_156353_d8afde77a4ea3f073a969e79e87968e495583476.png` | 5.02 | 14.86 | 221 | 2488 | 1769 | 19918 |
| `1203220_156358_fdcc1c088cb947a0914ad46f0675d7b923c7d4ba.png` | 2.49 | 11.65 | 20 | 4422 | 143 | 35359 |
| `1203220_156359_a2399b29c075f51fcf56fd233d8fa6b83466eda1.png` | 9.88 | 12.69 | 302 | 771 | 2400 | 6138 |
| `1203220_156360_457a86437c2f1ba58d5398b854fa44109b92d35f.png` | 5.22 | 11.64 | 32 | 1987 | 251 | 15905 |
| `1203420_153564_bbc56545debc214970ea081bcc7e6eb383e7930e.png` | 18.33 | 14.89 | 149 | 6274 | 1207 | 50203 |
| `1203420_153565_eec8feefd091e788a158fa26d0557b3d9cd749f2.png` | 8.03 | 11.62 | 250 | 257 | 2019 | 2039 |
| `1203420_153566_18f56cbc585b0979164534234276f91f47479dbb.png` | 8.18 | 11.64 | 68 | 802 | 529 | 6404 |
| `1203420_153567_e58d362b8a21ac99d3043606f23932e6474a028e.png` | 24.82 | 16.95 | 817 | 1597 | 6573 | 12801 |
| `1203420_153568_ff11b39f7cef8de7593047e22c7ae77bcda25049.png` | 16.80 | 14.82 | 583 | 739 | 4651 | 5920 |
| `1203420_153569_af0ba79f70d498d4ceb1fb08f113d0ce693c52be.png` | 21.08 | 16.99 | 304 | 2094 | 2429 | 16721 |
| `1206060_147969_2d764dc69aae80cb881635585fe274cac3e644a8.png` | 9.94 | 21.13 | 1108 | 1115 | 8873 | 8930 |
| `1206060_147970_a877a1f3a830cd878838b7fe2301bcd0963a176c.png` | 11.91 | 27.45 | 2093 | 2168 | 16733 | 17330 |
| `1206060_147971_48a40200bb12ab212f48f3ba05cb34ffaa18bd6f.png` | 23.80 | 33.81 | 2982 | 3044 | 23849 | 24350 |
| `1206060_147972_a06d97987d3e272c77d34d1ebce856eb9d2c4eab.png` | 20.87 | 25.36 | 1137 | 1337 | 9113 | 10711 |
| `1206060_147973_1fc8de2004e26ae0662cb888d909d103c404a63e.png` | 14.68 | 27.46 | 1730 | 2171 | 13854 | 17388 |
| `1206060_147974_f24f6e221848f70a441a2b8c8a59aef6e9740d34.png` | 7.96 | 24.31 | 303 | 624 | 2437 | 4996 |
| `1210230_163424_dce47607413c7aec7f70e0f85d272b526805a656.png` | 11.99 | 11.60 | 4021 | 4084 | 25085 | 25583 |
| `1210230_163425_2738e56e7690cc29b6fa3f6a3d7f122ee78fe90a.png` | 14.96 | 14.79 | 728 | 1549 | 2621 | 9193 |
| `1210230_163426_a0dc6a583d67105a14d67d0be33cd7d33ed52edf.png` | 22.53 | 19.07 | 3294 | 4009 | 19500 | 25226 |
| `1210320_393938_f726f2b1a323964b9e4cdde594d134121e69c349.png` | 30.95 | 25.49 | 222 | 2318 | 1773 | 18499 |
| `1210320_393939_f98e02ff8ed65e65305661fd8443814fb13a345c.png` | 42.26 | 75.21 | 331 | 2063 | 2643 | 16496 |
| `1210320_393940_be68d10627a5aeb9eff94f5cd491ee5f3d0ae853.png` | 34.86 | 79.42 | 322 | 1659 | 2557 | 13285 |
| `1210320_393941_25e4efd8b3df51a23e67019ddc0be6c9ba6d0cdf.png` | 32.25 | 27.75 | 195 | 1592 | 1584 | 12746 |
| `1210320_393942_af7923a7fe2a89e715a788dae9b2cf41de10b7a2.png` | 35.13 | 61.34 | 215 | 838 | 1719 | 6701 |
| `1210320_393943_39b23c5734e23731347f2135bce835342492524f.png` | 20.92 | 68.74 | 232 | 1934 | 1839 | 15459 |
| `1210320_393944_b30d6152896bcb7ed4401efff0872bb9350a2046.png` | 26.97 | 44.61 | 228 | 282 | 1834 | 2254 |
| `1217060_154308_6aea4bb22bfff08ee55e48c46dd563ce604be5e5.png` | 3.62 | 11.61 | 51 | 122 | 405 | 974 |
| `1217060_154309_18e7251785ea4b626f30681f71d92e64655db20a.png` | 4.61 | 11.58 | 24 | 129 | 186 | 1031 |
| `1217060_154310_99e922bb9c9854543008893d8170c6c00bad181a.png` | 4.88 | 11.58 | 63 | 156 | 506 | 1246 |
| `1217060_154311_854df1839a673daaa601c70a05169e57db4867df.png` | 3.03 | 11.57 | 38 | 69 | 291 | 541 |
| `1217060_154312_e04e280cab8c492baa02c535a1d4dd15f270a80c.png` | 4.94 | 11.58 | 19 | 138 | 156 | 1099 |
| `1217060_154313_904b0982a1585b22d096da73b078ad1ebb6570ec.png` | 7.25 | 11.59 | 44 | 170 | 352 | 1356 |
| `1217060_193913_2409644168203140379282f77f15d294c2daa444.png` | 2.88 | 12.76 | 33 | 7947 | 265 | 63580 |
| `1217060_193914_32930268b031dcac3ba5504f78278b65492ef2fe.png` | 3.53 | 14.84 | 62 | 4999 | 479 | 39998 |
| `1217060_193915_0137d99c3f4ad5d54af40f83172da007fe71412a.png` | 2.16 | 11.62 | 64 | 3432 | 538 | 27459 |
| `1217060_193919_4e41f9388efe6336e33b71deb461b609e846009a.png` | 14.09 | 15.87 | 95 | 3459 | 769 | 27681 |
| `1217060_193920_75a89bb0e559c15f9a36330912dd80ef0280bb21.png` | 3.15 | 11.64 | 52 | 4932 | 437 | 39445 |
| `1217060_193921_b70faf3acb212938908d1f231a2d16c02f947716.png` | 2.45 | 11.62 | 38 | 4478 | 323 | 35848 |
| `1217060_193922_158f17280fe32f0078db4f1ab080a499e70f6e14.png` | 2.46 | 11.65 | 18 | 1475 | 160 | 11775 |
| `1217060_193923_12821d1a1cb48af76d3044a60b8ab8d3b327706b.png` | 2.18 | 11.74 | 30 | 4459 | 230 | 35664 |
| `1217060_253463_ee1ed0b2a08746beca1f0f31fbc6a36178434b04.png` | 1.99 | 11.63 | 28 | 4932 | 226 | 39437 |
| `1217060_253464_68de90ed8a2c9220087b1cca23822f836474b799.png` | 18.70 | 40.26 | 357 | 1448 | 2812 | 11529 |
| `1217060_337146_b8c64c0149225e492b39348a62614aca1a571a6a.png` | 3.73 | 13.77 | 43 | 2491 | 349 | 19938 |
| `1217060_337149_fce927168f68979bf9c7fc8765cd94b9de30a3e9.png` | 7.21 | 15.92 | 92 | 5694 | 725 | 45515 |
| `1217060_337150_78df8fff3c73008f111872c5012cb698cf181693.png` | 2.52 | 11.67 | 35 | 5720 | 296 | 45794 |
| `1217060_337151_2b944333c0f35c701e5324a98c85b73558be487e.png` | 2.19 | 11.69 | 26 | 4986 | 215 | 39923 |
| `1217060_337152_e327c4470f3b291c2403c546b2cd35f33316287a.png` | 2.16 | 11.66 | 39 | 6428 | 324 | 51449 |
| `1217060_337153_6571122b76313ea43fb1978070fe5b01db099912.png` | 2.54 | 13.81 | 26 | 4919 | 230 | 39361 |
| `1217060_439533_9e84681709497b7d69af7206b9a28402ac1d948c.png` | 1.51 | 11.63 | 33 | 1142 | 260 | 9136 |
| `1217060_439534_7f2e2c2e61bea61680ee524fbfbf447f7a9361b2.png` | 1.58 | 11.64 | 15 | 1839 | 158 | 14732 |
| `1217060_439535_ddfd37f050c6d093c149642edad8559b35ba7459.png` | 1.42 | 11.61 | 30 | 1262 | 243 | 10102 |
| `1217060_439536_c9af39c18b3f0527ea89a5df52e2efef571f9d22.png` | 0.87 | 11.62 | 11 | 2185 | 102 | 17474 |
| `1217060_439537_77edb14476a3b9adeaa4016bcd16ec25dc079bf5.png` | 1.77 | 11.63 | 32 | 2908 | 248 | 23256 |
| `1217060_439538_f98c01575fd9fda9c3e594fa6c0470770de7711b.png` | 2.68 | 11.60 | 96 | 1114 | 765 | 8925 |
| `1222670_363987_46a09f27bc307afac889df0b59b4ab4b039a0313.png` | 32.35 | 24.34 | 925 | 1628 | 7360 | 13021 |
| `1222670_363988_913ff2c8e3111b120ec7374fa24150cb2115b421.png` | 2.26 | 11.61 | 53 | 524 | 433 | 4195 |
| `1222670_363990_6211d6e7b67a3d84abcffe707750fb86ecfc8ab2.png` | 4.16 | 11.67 | 32 | 588 | 247 | 4698 |
| `1222670_363991_7fbdf62f15a4176941281fb67a96d5bffb09d679.png` | 18.21 | 22.29 | 179 | 1725 | 1436 | 13819 |
| `1222670_363992_02b07dc524f0a1d40dd643b0bb0b5cc0e271005e.png` | 46.10 | 24.36 | 499 | 1546 | 4008 | 12395 |
| `1223590_201832_14778a4266f857031c20bd183930dd0fda69a040.png` | 7.53 | 15.88 | 125 | 2086 | 953 | 16681 |
| `1223590_201834_868eaa8574aec5dc90dc3f787142542c1e07a929.png` | 18.46 | 15.86 | 238 | 1563 | 1902 | 12505 |
| `1223590_201835_d4db0291b0c70bbcf15a4ecd3f60ac4e31b77b10.png` | 44.99 | 68.79 | 3005 | 3083 | 24030 | 24662 |
| `1223590_201836_408ff6f42b730a92b696573a0787de37f902fe2a.png` | 4.27 | 11.69 | 71 | 743 | 594 | 5937 |
| `1223590_201837_f165e9b0172cfe4ece9fe3faacd19b1650d0c15b.png` | 7.47 | 11.70 | 91 | 1138 | 723 | 9089 |
| `1223590_201838_edfcacf44704f41993f89ab6809f12df3e22749d.png` | 9.68 | 13.01 | 267 | 710 | 2134 | 5697 |
| `1223590_201839_444b81c96ba2391ddf4070ec6d82101498f3f640.png` | 4.22 | 11.58 | 831 | 1500 | 6681 | 12020 |
| `1223590_201840_a6167c0df3b2b0965b0512d1234b032423fd6abf.png` | 18.70 | 11.62 | 97 | 1023 | 761 | 8191 |
| `1223590_201841_6afef6a26a615879701da68e30deb33ce67d9ba0.png` | 13.13 | 19.11 | 171 | 2033 | 1345 | 16272 |
| `1223590_201842_e9d241d6516d93cf4f6159e0d44379dd2908da5d.png` | 8.68 | 11.62 | 73 | 1044 | 588 | 8345 |
| `1223590_201843_9296478837cac53831ac566b77c74603899ad997.png` | 10.19 | 11.65 | 161 | 1126 | 1278 | 8996 |
| `1223590_201844_4d892209ac792e787647c311234a1922052c8576.png` | 11.57 | 15.87 | 2409 | 3573 | 19255 | 28589 |
| `1223590_201845_9a1d0ddc830cb1430f2ffa505b6db25ec5aaf347.png` | 4.48 | 11.62 | 74 | 659 | 578 | 5264 |
| `1223590_201846_234d90dcd21858f49821ffd773354c676c15913f.png` | 11.11 | 13.83 | 279 | 855 | 2223 | 6845 |
| `1223590_201847_1abae40f15cbb00693a642b6024f9939843ac382.png` | 8.11 | 11.62 | 67 | 698 | 535 | 5565 |
| `1237730_184002_1130293b8f976af988d87a1636f523de68aac6ff.png` | 4.81 | 11.63 | 294 | 661 | 2348 | 5285 |
| `1237730_184003_77923aea52d0592e4359688ef502077764b3ff94.png` | 2.92 | 11.63 | 80 | 409 | 661 | 3259 |
| `1237730_184004_9a08675a317553ad6a1ce7ffd862427f1937174b.png` | 7.39 | 16.94 | 281 | 683 | 2262 | 5447 |
| `1237730_184005_57c70076c41a236f64afe2ff9b65c0bfa0a8cf73.png` | 25.39 | 17.35 | 559 | 1733 | 4451 | 13828 |
| `1239300_150303_2ac7598f4913eab8c626cd9224f732c906096737.png` | 34.39 | 31.81 | 1330 | 2199 | 10561 | 17537 |
| `1239300_150304_1f1531721d52085f4682300b3a124beb1d3b122d.png` | 10.71 | 13.86 | 275 | 2001 | 2225 | 16036 |
| `1239690_164751_3ead088c1f61dae59a9bc1d6c3d8ceec0cdfb853.png` | 14.13 | 11.66 | 125 | 423 | 1013 | 3394 |
| `1239690_164754_6b034a9af4c23ccfead0d4c2e5af549330bed0cd.png` | 18.99 | 20.11 | 1699 | 1654 | 13545 | 13208 |
| `1239690_164760_5c537fda8f07c55d97ab7dc5ec0b3cfde2e520d2.png` | 35.51 | 27.54 | 265 | 1322 | 2095 | 10605 |
| `1239690_164777_91344a66093bb589dd0624204dfcf6298d13e072.png` | 10.40 | 11.61 | 222 | 340 | 1763 | 2693 |
| `1240480_168070_4bb5f046dc7b3b716e31c2122fde17439c391fdb.png` | 2.59 | 11.59 | 99 | 125 | 800 | 1003 |
| `1245560_255371_ce8057df405235067434f9b9fa2665e3c05ee33d.png` | 2.00 | 11.65 | 9 | 9 | 87 | 70 |
| `1245560_255372_33431fe2e8a78f294d9fb093bf5c6641ab4729f6.png` | 0.45 | 11.61 | 1 | 3 | 12 | 24 |
| `1245560_255373_4cc6aed55df7a9e4af612292c3ccffc8546761d9.png` | 4.58 | 11.86 | 177 | 190 | 1414 | 1515 |
| `1245560_255374_ad8d4902448d64e55537ccc7f4294d7e565329f1.png` | 4.53 | 11.77 | 39 | 39 | 315 | 323 |
| `1245560_255375_672d84752737ffcbd53a4a3c130e22290a9f6807.png` | 1.27 | 11.67 | 8 | 9 | 64 | 81 |
| `1245560_255376_c627f6c569bf8ba62a50883b6f3469ba0ebaf83e.png` | 1.50 | 11.62 | 13 | 15 | 89 | 102 |
| `1245560_255377_ab7d2670d88387ae771d03d5b4bf345af526d7cd.png` | 0.92 | 11.61 | 6 | 12 | 47 | 93 |
| `1245560_255378_cb223b4ffca582be50aa44b95fbfafac225bcc43.png` | 3.32 | 11.72 | 20 | 23 | 164 | 175 |
| `1251930_183401_bac27e67c5c74dd6fbbd6965ec512c8a0283c040.png` | 0.01 | error | error | error | error | error |
| `1251930_183402_331df3402a847673bff01cdf64699b79f99f565b.png` | 0.00 | error | error | error | error | error |
| `1251930_183403_a69d01e3cc41d809026903afd270684cf04fbbfc.png` | 0.00 | error | error | error | error | error |
| `1251930_183404_4e15db83f308eda38291eff8f30e3c22d1e6ccaa.png` | 0.00 | error | error | error | error | error |
| `1251930_183405_0cb9bb24fa8efe3c238553a08a0a18c52e11b8bd.png` | 0.00 | error | error | error | error | error |
| `1251930_183406_a073781fb098e57ea2bc2d0aa187d6ac3f06aa93.png` | 0.00 | error | error | error | error | error |
| `1253250_277123_330880945461e3b596190a478bfa2a7bb823f4f3.png` | 6.03 | 20.07 | 199 | 251 | 1578 | 2017 |
| `1253250_277124_58f3094f9da40b896afc0a7487c75b337c66a7cd.png` | 19.04 | 35.96 | 2287 | 2332 | 18276 | 18633 |
| `1260320_316454_eab212d417641befc7f47245db84f96ea95b88c6.png` | 2.08 | 11.59 | 30 | 111 | 242 | 892 |
| `1260320_316455_d92d42f3ae786cb70d8e50779b1eefe8ce3eb312.png` | 1.58 | 11.59 | 36 | 122 | 297 | 983 |
| `1260320_316456_da5a0e2a4d047710e4be72b4025866982ba657d6.png` | 1.36 | 11.58 | 26 | 97 | 209 | 781 |
| `1260320_316457_cc368c64c6ecad7377f0481e3e4978894e11d854.png` | 5.04 | 17.94 | 808 | 808 | 6469 | 6469 |
| `1263950_78791_5d2e6234ab06945e768036ca72765260f4bd591e.png` | 7.83 | 22.62 | 302 | 1225 | 2416 | 9777 |
| `1263950_78793_95263c6c8e742301dcf288db5b70071cf58d99fe.png` | 1.43 | 11.60 | 59 | 138 | 452 | 1093 |
| `1263950_78794_e624aa3b3f96c3036eab7b3ca4a677babd14c389.png` | 2.66 | 16.92 | 91 | 1185 | 827 | 9540 |
| `1263950_78795_6753a45f83f9d336c9ccb70f8b8b261bda112a31.png` | 0.90 | 11.58 | 41 | 190 | 323 | 1507 |
| `1263950_78796_856f3f10f7d3ce3f3ad006e6f537d028f38bc7fc.png` | 8.81 | 13.75 | 353 | 659 | 2798 | 5260 |
| `1263950_78797_76cdcd6915620a61e9874d01f3a34b21a759eca9.png` | 2.66 | 11.59 | 86 | 297 | 692 | 2399 |
| `1263950_78798_2902d6e42aaabd81cb99a78d79d6b7ef5e55eeb1.png` | 10.06 | 15.88 | 212 | 1161 | 1720 | 9272 |
| `1263950_78799_d99683aea49157e8c9b3972b81e08a28d14105f4.png` | 9.81 | 11.63 | 187 | 671 | 1478 | 5368 |
| `1263950_78800_22a76406b250da8978cf333c24f12b8ef6837724.png` | 7.54 | 13.73 | 320 | 658 | 2550 | 5261 |
| `1263950_78801_22ffd2d17e26817fc68fb0f698a1ed272b85d7db.png` | 1.76 | 11.60 | 274 | 572 | 2210 | 4592 |
| `1263950_78802_9ec0884dbc3f7caf5794fd9e3bf89a2a62154a5d.png` | 3.09 | 11.62 | 89 | 383 | 711 | 3066 |
| `1263950_78804_b18b17983906706bffe19b50532eb32c8dd68267.png` | 2.12 | 11.61 | 45 | 125 | 382 | 1003 |
| `1263950_79912_bb7d0f75547e01caf03e6975430ea50da7f2ab78.png` | 4.14 | 11.58 | 14 | 47 | 115 | 377 |
| `1263950_98988_42794a248d877e8b3d0ac01ffdd01d46c378963f.png` | 1.39 | 11.61 | 202 | 314 | 1616 | 2519 |
| `1280930_252916_dc9fc8dd7a8ecf9258ee631f2c2e049b175a086e.png` | 4.38 | 11.64 | 221 | 379 | 1769 | 3038 |
| `1280930_252917_275dbd3f569813b3063c898dc012ed541e920c42.png` | 1.97 | 11.60 | 76 | 901 | 564 | 7200 |
| `1282270_427327_db0d655baea7941f715526168f23d65bca129733.png` | 1.22 | 19.00 | 6858 | 6876 | 54838 | 54984 |
| `1286320_231514_6e85203b59eafc4c36fbffa5d4e89eebf3e095b4.png` | 5.72 | 11.59 | 74 | 411 | 591 | 3290 |
| `1286320_231515_7daa81b6bc0d5af5a7efa0e9b5120cedd6b41378.png` | 6.46 | 11.61 | 120 | 585 | 969 | 4673 |
| `1286320_231516_b023dc27f6bc7e3baec125b4f80d64cbab4a1137.png` | 5.79 | 11.66 | 33 | 381 | 297 | 3066 |
| `1286320_231517_396a67d1fdb6dd2552d5d4279179991a8e6b3ed9.png` | 14.15 | 11.61 | 409 | 1095 | 3289 | 8773 |
| `1286320_231518_a0baa71f8b4b5a4ede7f6667f70611f75adba919.png` | 7.72 | 11.64 | 54 | 409 | 415 | 3259 |
| `1286320_231519_1317d411b3a708deeb15328a0802e2bf19a4ba89.png` | 7.04 | 11.73 | 45 | 364 | 373 | 2915 |
| `1286320_231520_2ad241cdf7384bc78d9ef40454260472e5f8859f.png` | 15.63 | 13.73 | 384 | 1023 | 3063 | 8187 |
| `1290760_1332150_e4b5097ae51e26ab297d81f51d08aa52e545bdc1.png` | 0.74 | 11.58 | 105 | 144 | 841 | 1151 |
| `1290760_1332151_5b028bfc50eca24d1e48b62240ea5aa1b889360d.png` | 1.18 | 11.58 | 48 | 108 | 383 | 858 |
| `1290760_1332152_f49394cee5b554d3a15d790a7bfbe7a6294e6f6d.png` | 14.43 | 12.66 | 62 | 699 | 487 | 5606 |
| `1291860_212256_af9ff143b65a6cb0c77a18d35e81597be615ec32.png` | 3.44 | 11.58 | 31 | 137 | 252 | 1114 |
| `1293230_103411_1e04224079d6db3360f8c1b9f60b3eaff0436530.png` | 116.61 | 19.72 | 1115 | 2093 | 8919 | 16724 |
| `1293230_103412_a3bd2cd72e46972dceee9cac14e2daba7166b4d0.png` | 19.03 | 12.72 | 116 | 1536 | 926 | 12271 |
| `1293230_103413_2490e326f31e6b156ece0a6d5471603b0dcb333e.png` | 23.38 | 14.80 | 158 | 1445 | 1270 | 11555 |
| `1293230_103414_8a6605ad5cfd660019b5aa566ddb8238a6d27ee0.png` | 7.45 | 11.63 | 47 | 2582 | 390 | 20646 |
| `1293230_103415_56980451072225f851bf42b1a6b70076e2149c4b.png` | 16.14 | 11.62 | 109 | 1600 | 852 | 12781 |
| `1293230_103416_06e181e2a68637525d9d5b76c8b2c904af44c502.png` | 42.18 | 29.70 | 242 | 2477 | 1935 | 19794 |
| `1293230_103417_b210cd82f837db00d37c2427b2e1a2fd65500671.png` | 22.51 | 23.28 | 129 | 417 | 1039 | 3325 |
| `1293230_103418_e1a8d918b1aca63c74ac0d51429921146ab3f6d5.png` | 15.82 | 16.90 | 109 | 976 | 884 | 7821 |
| `1293230_103419_888bf774376e553afbced3eaf2323baecfef0d0d.png` | 18.49 | 20.16 | 177 | 2873 | 1448 | 23037 |
| `1293230_103420_b448832aa64e800e76be9252add71c4a90d48a9d.png` | 24.40 | 21.14 | 124 | 659 | 975 | 5254 |
| `1293230_103421_060a80283a162975f44c52d10ed5a4731e8615eb.png` | 26.35 | 21.22 | 113 | 1686 | 940 | 13449 |
| `1293230_103422_b2089ee6f23c7b1f953f31a2ddfc870472cb493b.png` | 24.00 | 22.22 | 167 | 1024 | 1329 | 8188 |
| `1293230_103423_a5bbc0b022abfec6766063dcc7a45a77a662bae2.png` | 18.22 | 20.16 | 140 | 671 | 1102 | 5360 |
| `1294760_397940_0c9ba439c2445cec267e4bd2ae8af5af7d7cdcc6.png` | 2.80 | 11.59 | 1006 | 1287 | 8046 | 10310 |
| `1294760_397949_107dafce7f8497167a2a5bad959fcabec9abbfe9.png` | 8.80 | 11.59 | 826 | 855 | 6623 | 6849 |
| `1294760_397950_79318824de7818b36e03838967c4514c48afeb73.png` | 5.11 | 11.61 | 438 | 678 | 3515 | 5425 |
| `1295660_377613_7971308d145b7d04f566022d13831c6f67aa3c1b.png` | 2.76 | 15.78 | 6333 | 6579 | 50666 | 52653 |
| `1295660_377614_ec9921135a4f686680805cdb0d7faa16cd56ffd7.png` | 1.66 | 13.66 | 1397 | 1928 | 11182 | 15433 |
| `1295660_377615_e11bece8dc7ba2eaa6378c66452a3ea8ca010ce1.png` | 19.07 | 33.79 | 165 | 1737 | 1309 | 13840 |
| `1295660_377616_b0ff5711c63b5d114dcb22900983712ba755bd18.png` | 2.80 | 15.80 | 7832 | 8114 | 62647 | 64905 |
| `1295660_427187_77287e8aa384407a7c090c7e0c1d972e95f6d080.png` | 18.70 | 12.75 | 104 | 926 | 815 | 7392 |
| `1295660_427188_da2373b559d030013e6e40f4989e5c80b10039b9.png` | 5.43 | 11.59 | 150 | 232 | 1202 | 1862 |
| `1295660_427202_110202b01c5db22bdb338657763a7b882282e43b.png` | 3.43 | 11.65 | 26 | 33 | 205 | 262 |
| `1299690_315691_301a9db50b6b26348974cc87e04748803b39c050.png` | 4.41 | 11.60 | 413 | 554 | 3313 | 4435 |
| `1299690_315693_d70af89c6eb3b3d88e86abb4fe2108e9cace99f7.png` | 2.21 | 11.59 | 26 | 485 | 209 | 3894 |
| `1301950_145181_c5064f7067890588c94b537c480065ab132d9fa9.png` | 11.47 | 12.68 | 573 | 969 | 2340 | 5505 |
| `1303670_419671_34b60c5758b50f3dbe8b88266680594751fed664.png` | 23.67 | 24.30 | 656 | 661 | 5226 | 5299 |
| `1309820_155111_3d0137e163348289b2b037587dc7ba384673b720.png` | 6.55 | 11.64 | 1212 | 1697 | 9694 | 13551 |
| `1310330_251805_d256b0462fe4ed3f6ce7a404203c0ccafe40a2d8.png` | 0.78 | 11.58 | 10 | 55 | 73 | 433 |
| `1310330_251806_623017a955e7b50d981f1e0c6cdb36ccf8fd5ca7.png` | 0.18 | 11.59 | 0 | 113 | 3 | 905 |
| `1310330_251807_95ba9957ee305d5f9c739292971c53c4953f4558.png` | 0.82 | 11.60 | 6 | 44 | 46 | 356 |
| `1310330_251808_f9d6518eac9da278f9a7ddee1fed0585f930ae38.png` | 2.09 | 11.62 | 20 | 160 | 165 | 1273 |
| `1310330_251809_5742754725415aa0af49afdd984c2b3ef63c8958.png` | 0.78 | 11.57 | 5 | 51 | 40 | 410 |
| `1310330_251810_32de4366eac650c6563ace6aa98c5eba7cdc4fe3.png` | 0.89 | 11.59 | 17 | 235 | 132 | 1879 |
| `1313140_174316_48fa77ddb4e3c4435427b06e5b6cc53e16c790eb.png` | 1.29 | 11.59 | 175 | 203 | 1398 | 1618 |
| `1313140_174326_c2fcb485e3f4285d3460ce604679b24d25d243bc.png` | 4.92 | 15.85 | 210 | 879 | 1682 | 7023 |
| `1313140_174327_4737a5628c487e853da007eda12f47c24209e10f.png` | 1.58 | 11.58 | 38 | 159 | 304 | 1276 |
| `1313140_174328_a80519c5c7a84cda862b684347352706b16a13b0.png` | 5.16 | 11.61 | 313 | 728 | 2499 | 5828 |
| `1313140_174329_90882cf938fe07f125431eafb8a0af8a555d77d1.png` | 6.00 | 11.60 | 221 | 444 | 1768 | 3549 |
| `1313140_174330_48b872c248bddec0ef07a24fd9226095d9a928d1.png` | 0.71 | 11.59 | 6 | 79 | 52 | 630 |
| `1313140_174331_d33ad2de01973e53eedfd3eb6d011302e1a6c43f.png` | 13.67 | 11.62 | 336 | 574 | 2695 | 4608 |
| `1314563_102049_9d123555fb76efd356b837282399193a2c90aa85.png` | 3.73 | 15.86 | 429 | 483 | 3447 | 3883 |
| `1314563_102050_44ba71a3e423ffc69fdde074ae71782596ef6bab.png` | 8.73 | 15.83 | 2495 | 2516 | 19950 | 20122 |
| `1314563_102051_e072305145b2aba6c873e9c65fd734641aa69e04.png` | 16.49 | 25.42 | 2958 | 3217 | 23649 | 25726 |
| `1314563_102052_dfad58e94311d1ac975320f8c244d27002c017ef.png` | 6.59 | 19.02 | 843 | 940 | 6780 | 7551 |
| `1314563_102053_bf6350e564b05effd2e102a7002aa08afc2565f6.png` | 5.47 | 13.74 | 775 | 1119 | 6221 | 8953 |
| `1314563_102054_d81352903b22685b64ce300a42f69662c177c5fc.png` | 1.04 | 11.61 | 784 | 832 | 6268 | 6647 |
| `1325900_146758_6e5de43c7000d1893bb38c148ba3c99e500f5843.png` | 2.71 | 11.61 | 811 | 814 | 6491 | 6516 |
| `1325900_146759_1fa5bc1828c8d02287cf666329b8dcd01f31a222.png` | 4.09 | 17.95 | 550 | 639 | 4402 | 5106 |
| `1325900_146760_e7ae8290cf6a90e634c554bf178da5d5eca205df.png` | 7.41 | 21.12 | 730 | 768 | 5845 | 6144 |
| `1325900_146761_7b45542ecf59505ebc06f6dace83ba9bf509b17b.png` | 4.09 | 16.89 | 922 | 926 | 7372 | 7398 |
| `1328670_115967_b806c1aa84130abf3abd9beecbe8eb24d0983015.png` | 83.96 | 191.43 | 30746 | 30773 | 245968 | 246174 |
| `1328670_115968_72204422d7d719927b96e03d5e22bec268ff5180.png` | 30.63 | 70.84 | 3963 | 7152 | 31712 | 57200 |
| `1328670_115969_2cbf9a8410fc40783880b24cb09d7a50fed386f6.png` | 39.54 | 135.31 | 12851 | 12927 | 102766 | 103391 |
| `1328670_115970_fd7fcbc405377bf2ddba3d081473c83fe400adce.png` | 108.19 | 155.51 | 13199 | 13277 | 105571 | 106191 |
| `1328670_144864_46b17e0f1dfb49fe3553660c9dc130dc213f2a66.png` | 15.31 | 13.97 | 284 | 1341 | 2271 | 10724 |
| `1328670_144865_08b59f56540b7a7f1efea23454de411895ff3263.png` | 0.91 | 11.60 | 58 | 856 | 463 | 6855 |
| `1328670_144866_d600b96130a3a97df25156dacd1501a2875f1ebb.png` | 11.79 | 11.77 | 179 | 1413 | 1424 | 11300 |
| `1328670_144867_5c82e0ea1fd0fb087ec1c0c8011a6a2ff761439f.png` | 6.94 | 14.80 | 417 | 2485 | 3350 | 19897 |
| `1328670_144868_d7d7a122298df1a3f97fa354a4042a15df8fb531.png` | 20.89 | 14.84 | 357 | 1130 | 2825 | 9030 |
| `1328670_144869_12a4ec9ece1ac7339183d96aa34843199e3ce5d0.png` | 0.29 | 11.59 | 9 | 400 | 89 | 3216 |
| `1328670_144870_5c6eeacb880fb32e623d8bd96a7e66728e328b1c.png` | 0.74 | 11.60 | 25 | 702 | 188 | 5589 |
| `1328670_144871_5a65a359e8527373cf7c7305eaf8c3628b1fb5c3.png` | 0.49 | 11.59 | 8 | 804 | 53 | 6457 |
| `1328670_144872_9d197de02edf3982c9b65df184504a61fc278e89.png` | 0.73 | 11.60 | 13 | 902 | 106 | 7213 |
| `1328670_144873_2d6f573485e07080718dca6de4d770bc8fa92425.png` | 0.70 | 11.59 | 51 | 956 | 405 | 7642 |
| `1328670_144874_2a98961c56ccc86ae725365e183ee9ca333269f7.png` | 2.88 | 11.62 | 218 | 1900 | 1731 | 15183 |
| `1328670_144875_d26d05c0270dc74e851739ed302e34f5d8277106.png` | 6.57 | 11.63 | 248 | 865 | 1991 | 6914 |
| `1328670_144876_05aea5d52f3af94c4feb7a07949830ac4c217c0e.png` | 5.61 | 11.61 | 127 | 1123 | 1022 | 8980 |
| `1328670_144877_04a6d67f2bcdf8131f030168eb49659ab9d0879d.png` | 15.95 | 14.78 | 308 | 496 | 2434 | 3977 |
| `1328670_144878_5d7e7f219fa1a66a7c4db50ef6d9b1c57e19046e.png` | 0.75 | 11.59 | 21 | 383 | 171 | 3062 |
| `1328670_144879_54774556cb987b565aa10f28dc53d88661231cb3.png` | 0.62 | 11.58 | 28 | 446 | 222 | 3552 |
| `1328670_144880_4ddd216285fda20367efa8dab36b3814072f7381.png` | 0.71 | 11.58 | 55 | 158 | 455 | 1283 |
| `1328670_144881_2a7f06f2faac684e030685ef54cf018c9cc0a07c.png` | 0.25 | 11.58 | 11 | 474 | 99 | 3797 |
| `1328670_144882_d916f434a4e7f5644f1a77f2d1c17c36e51c64ea.png` | 0.46 | 11.58 | 30 | 122 | 231 | 971 |
| `1335230_137645_1b19259a309087c9b25166924141b67ed8855010.png` | 6.74 | 16.89 | 278 | 382 | 2220 | 3067 |
| `1335230_137646_b043dfe6c82bec964d650891b95cb29707b1b28f.png` | 8.34 | 15.83 | 177 | 292 | 1426 | 2350 |
| `1335230_137647_041f1e326c1d157d1313314fbb3560ac17695af1.png` | 8.92 | 22.19 | 159 | 217 | 1271 | 1738 |
| `1335230_137648_a7491d3376e712d5cac9041c0c972507c422cce5.png` | 8.97 | 20.06 | 140 | 187 | 1110 | 1487 |
| `1335230_137649_591807dd1aa5c200fcff659f4891feec51c28c73.png` | 16.18 | 33.82 | 116 | 132 | 919 | 1041 |
| `1335230_137650_e9d228d3fe9fa1588dec698ddc49e512cbf1f75d.png` | 8.02 | 22.19 | 122 | 185 | 983 | 1492 |
| `1337760_205734_c5576e1411a657da3ab4e054fe94103596fd80d5.png` | 1.75 | 11.61 | 16 | 20 | 144 | 165 |
| `1337760_205735_27742990aa8273073c9622b6c41e8441680f935b.png` | 0.92 | 11.63 | 11 | 9 | 81 | 69 |
| `1337760_205736_365c31961b72af81ddbcc8ea4e446aa8633e2237.png` | 1.38 | 11.63 | 5 | 3 | 51 | 39 |
| `1337760_205737_9938b3bbee2dd7af36529f9c6d78ee5e7a4996b5.png` | 0.70 | 11.60 | 2 | 2 | 16 | 17 |
| `1337760_205738_3dafad15a162cc34d1702e79b82c2a606d3b20d9.png` | 1.63 | 11.64 | 14 | 13 | 92 | 100 |
| `1337760_205739_981c9cc63bc3bd46f77b93f0dcdc88d61a77ead2.png` | 0.92 | 11.60 | 13 | 13 | 101 | 106 |
| `1337760_205740_b2c3b82abc425c06e92c780af0ca7920017fcf1d.png` | 0.19 | 11.57 | 2 | 8 | 14 | 42 |
| `1337760_205741_fed5b265c6cc25aadb86310bd8c92a5d11741479.png` | 1.20 | 11.57 | 8 | 8 | 61 | 70 |
| `1337760_205742_25db20197ee60ea4560d6fafabe4ff3f92cda0e6.png` | 0.65 | 11.57 | 5 | 5 | 35 | 33 |
| `1337760_205743_72b3db3fa5cfa41fe5de6bdf82daccd6c6caecc0.png` | 0.43 | 11.57 | 6 | 6 | 45 | 51 |
| `1347030_203651_28fc3cf73d9053051ef9b45d89361eb5f265d4df.png` | 1.00 | 11.58 | 314 | 341 | 2507 | 2725 |
| `1347030_203652_478f2757a78a3091afc64eb23dac6dac192bd525.png` | 0.64 | 11.58 | 18 | 200 | 142 | 1595 |
| `1347030_203653_0d2d229891ac5d8b8f92adad09900f631b369ebc.png` | 0.78 | 11.58 | 153 | 238 | 1220 | 1902 |
| `1347030_203654_826ca722eab5cef6df174822015253cdbd1ab1bb.png` | 1.01 | 11.59 | 300 | 304 | 2401 | 2436 |
| `1347030_203655_0c7e7867f8e4c4072e8e0b282aedea74d0963f5e.png` | 3.73 | 11.58 | 343 | 359 | 2612 | 2743 |
| `1347030_203656_fcf891c74a1f4277dd7ad30c67e8b8d07f93a8da.png` | 0.93 | 11.58 | 239 | 241 | 1902 | 1925 |
| `1348700_147790_df5dc6769bb2088371b2ef7ca329d882b262639e.png` | 44.03 | 60.29 | 261 | 325 | 2085 | 2606 |
| `1348700_147791_d1bffd95de89a0a85307e06e819189de67e55afb.png` | 28.65 | 57.12 | 217 | 1347 | 1737 | 10766 |
| `1355220_79867_9dec4843421735e53866432552463c638aeb33ec.png` | 2.43 | 11.67 | 145 | 138 | 1156 | 1112 |
| `1355220_79868_d6ccecc762ae76298169e0df947c84fe4a1feb32.png` | 2.22 | 11.67 | 40 | 44 | 313 | 349 |
| `1355220_79869_0fc935e2644670398f63f67f570b32fb9dcbc750.png` | 1.26 | 11.64 | 3 | 15 | 23 | 116 |
| `1355220_79870_71eb373dac6c09c0f8b0c76dedec289f416bbb3c.png` | 5.41 | 11.77 | 104 | 122 | 822 | 956 |
| `1355220_79872_93888b9bd8006a9aa5d8f8298330c51f8741c7f4.png` | 1.88 | 11.63 | 53 | 50 | 405 | 382 |
| `1355220_79873_e7e722996d19544a155b9e985b92ac2984707d2e.png` | 5.17 | 11.75 | 97 | 107 | 793 | 866 |
| `1355220_79874_97362052874f145f639392ba6363e109cec9adbf.png` | 1.50 | 11.62 | 86 | 89 | 708 | 716 |
| `1372280_415829_6df622513c4c8654da2b63d597c35276520a7d32.png` | 0.70 | 11.58 | 28 | 141 | 228 | 1125 |
| `1372280_415830_330446f7109c58dcf0ff740cfb85e4ad9bc35f8f.png` | 1.22 | 11.58 | 221 | 312 | 1765 | 2498 |
| `1372280_415831_b4163ea040fe709795c670acecf91d0c9deabdf2.png` | 2.01 | 11.58 | 186 | 218 | 1490 | 1741 |
| `1372280_415832_bde25350a0fd0a15dd92a66ede802eac887f6224.png` | 0.73 | 11.58 | 88 | 137 | 704 | 1094 |
| `1372280_415833_f626e5a22d653088d71e6bc84d9f1de25b83f0ba.png` | 0.53 | 11.58 | 3 | 45 | 23 | 361 |
| `1372280_415834_c23847ccd3578c5147c5d4fa7a48272be269cc38.png` | 0.34 | 11.58 | 6 | 21 | 44 | 163 |
| `1372280_415835_9a529dcb2257fa40e0f79f94bf08b8985c1453f8.png` | 0.52 | 11.58 | 36 | 85 | 298 | 689 |
| `1379870_118661_e64318ed0632c03aebc852d1251aa5bbb4e5a8f2.png` | 1.21 | 11.60 | 13 | 19 | 98 | 143 |
| `1379870_118662_59f1287d05f6b54b4934bb7f2d92d66fe6fb17d3.png` | 1.78 | 11.65 | 15 | 16 | 130 | 131 |
| `1379870_118663_270cdd68c02ea0bf1548019bc9dc93dbc8dce0ff.png` | 1.34 | 11.63 | 5 | 11 | 40 | 88 |
| `1379870_118664_9b683d2f9fa4fe20df6223df2971d0f422c68b42.png` | 1.23 | 11.65 | 20 | 18 | 145 | 130 |
| `1379870_118665_1b1b247aaa8175b9c31c660b08020a2ecabdf101.png` | 0.58 | 11.60 | 8 | 8 | 74 | 73 |
| `1379870_118666_df4350d5265a1cb20a05b92877325fc6fc876c97.png` | 0.95 | 11.62 | 6 | 6 | 51 | 44 |
| `1379870_118670_b80fc2d98b592a7bdd156f14afcbb1347509cab4.png` | 1.54 | 11.65 | 14 | 14 | 96 | 117 |
| `1381770_457775_909efdf105b682a03b9261ce78cbc5c1475e21a4.png` | 13.21 | 11.62 | 68 | 669 | 523 | 5338 |
| `1390700_205776_3b4c85f4430a41aa71d0623759f08ec1b89cbaf9.png` | 27.09 | 40.21 | 151 | 1164 | 1199 | 9321 |
| `1390700_205777_dc108d8eb62c513664235ddc83c0a46f64b57645.png` | 45.73 | 53.93 | 270 | 467 | 2179 | 3748 |
| `1390700_205778_cf86c20de8b83185a8366842dfcaed7cbe7fac6f.png` | 66.99 | 73.26 | 480 | 1818 | 3889 | 14563 |
| `1390700_205779_5f2530a95d56ebef6c94b70fc684f848e0e2bb58.png` | 22.11 | 78.26 | 370 | 3014 | 2982 | 24168 |
| `1390700_205780_612a6f59075e7c085ea34d07a28d18275ebdbf0b.png` | 45.89 | 73.04 | 471 | 1439 | 3773 | 11503 |
| `1390700_205781_88f1f7270420164caed47effb1d0dba492c2b5ac.png` | 2.85 | 13.71 | 668 | 914 | 5357 | 7306 |
| `1390700_205782_e2233e56d32f309ad662ea44289b0730a61dd802.png` | 47.42 | 48.64 | 220 | 401 | 1760 | 3220 |
| `1390700_205783_a61d00823801ddbc850d3ef74531a836c8c4a575.png` | 28.99 | 32.77 | 222 | 447 | 1794 | 3593 |
| `1390700_244159_712a028f04e0b79b58ef2a7d376c9942b39fc145.png` | 23.30 | 34.86 | 183 | 926 | 1446 | 7399 |
| `1390700_244160_ffa9b43fc2ae9667d5a5e68b6bb06049a550a583.png` | 3.08 | 12.66 | 54 | 324 | 452 | 2622 |
| `1390700_244161_f8b962b75d7e093ba186dd32a7e7682f3c814ba0.png` | 5.08 | 11.61 | 599 | 951 | 4780 | 7594 |
| `1390700_244162_35787563b8a9685b611557c13ebd2d96bf11a9ce.png` | 15.44 | 24.34 | 631 | 1585 | 5060 | 12678 |
| `1390700_244163_cf43d8de32e6c2a72f7a2a65c552e24f11cc8028.png` | 19.73 | 15.86 | 753 | 1595 | 6003 | 12754 |
| `1399930_110200_3673060fa58b8891b7d2e6e1d0b04385ad3050a5.png` | 7.65 | 17.98 | 66 | 443 | 512 | 3550 |
| `1399930_110201_c364423e81f14ee802e4e4b970458f2d9498af5b.png` | 22.72 | 26.43 | 135 | 339 | 1090 | 2728 |
| `1399930_110202_b0fc5b7ed7e635c5b0e5a13d26a80f018b61116d.png` | 35.22 | 35.95 | 270 | 498 | 2176 | 4020 |
| `1399930_110203_301a24dbf0e7da345f376e2b742901ea16fd62f2.png` | 20.41 | 26.42 | 142 | 737 | 1132 | 5860 |
| `1399930_110204_731b76c52ee071a6fb9b4291d79771b489822e58.png` | 1.59 | 11.61 | 4 | 321 | 37 | 2580 |
| `1399930_110205_f21cff0861e99a364d84c9312520c033f929d411.png` | 7.85 | 18.02 | 229 | 2251 | 1798 | 17979 |
| `1399930_110206_e53adc2e2d921b7354287b4f554e64d2ccc4bcb2.png` | 28.67 | 38.06 | 160 | 699 | 1252 | 5572 |
| `1399930_110207_5ed9337dbbb985ad8ff4b20790d5585cf3b762d4.png` | 25.93 | 32.81 | 215 | 907 | 1738 | 7241 |
| `1399930_110208_05336b6e979a1dfc6be6054ed282ff33eb5f5b9f.png` | 26.28 | 37.05 | 198 | 506 | 1593 | 4034 |
| `1400910_246344_dbbc4d4976b35b0d3afdeb7f9397e19a88e7af75.png` | 3.92 | 11.68 | 58 | 75 | 465 | 599 |
| `1406850_150455_bbe07a72527c9f49557a6d66207379b00e10c6e4.png` | 2.54 | 25.33 | 708 | 733 | 5661 | 5849 |
| `1406850_150458_268424df3be2c64a3bb3cef702d9616fbbf5da4b.png` | 6.92 | 15.83 | 356 | 508 | 2858 | 4074 |
| `1406850_150461_a996cd3bf564ee504b7aeb4a02e314ebbe8924a4.png` | 8.86 | 20.06 | 334 | 517 | 2670 | 4132 |
| `1406850_150464_dad3acd284465a9ed03816d206044ac577a32c72.png` | 6.72 | 17.94 | 433 | 486 | 3461 | 3884 |
| `1406850_150467_ed3a629e1a351bd5e07735b86cf7637a6f5baef2.png` | 6.14 | 13.73 | 117 | 442 | 954 | 3547 |
| `1406850_150470_e761f9243e455e3b6160f2e96dd9e163db08b5e4.png` | 6.58 | 12.67 | 275 | 653 | 2201 | 5227 |
| `1406850_150472_e7ef2df7de8b1b103c179a9bfcf533e63525caf0.png` | 5.99 | 26.40 | 1244 | 1420 | 9963 | 11366 |
| `1406850_150474_f9da128f852e2ea81596d80c7e90e465920c61e8.png` | 1.43 | 11.59 | 63 | 181 | 494 | 1454 |
| `1406990_103261_41600302fda54fba8c31db100a63e6b5f47bb68d.png` | 4.11 | 11.58 | 20 | 177 | 142 | 1410 |
| `1406990_103269_4d23c8a6ef290a766c6a9735de9d177941d0ff0d.png` | 1.42 | 11.59 | 60 | 239 | 474 | 1906 |
| `1406990_103270_39b8bd89d7078596ac89a66ded5927192470578f.png` | 3.01 | 11.59 | 70 | 155 | 565 | 1236 |
| `1406990_103271_eb788eaa58a9d69de67092f7bcf6656327aeb22d.png` | 3.39 | 11.59 | 38 | 233 | 301 | 1866 |
| `1406990_103272_2c40c63310162f1d40517cb46417f9bb6a0b7ddf.png` | 1.81 | 11.59 | 53 | 243 | 422 | 1948 |
| `1406990_103273_bd4947fe6e9e55ffc0cbcefbca331c2d7407d408.png` | 3.06 | 11.60 | 38 | 269 | 302 | 2165 |
| `1406990_103274_03a4d8b30d4dcf0fa7de3e2b2a5c9dfefc2968da.png` | 2.89 | 11.58 | 40 | 139 | 331 | 1119 |
| `1406990_103275_27bbb59771242b5641218320d96053d24e3246f4.png` | 3.09 | 11.59 | 28 | 202 | 208 | 1603 |
| `1407200_615251_993e1eb98b0d09a9456b3183792aae96f3dc1257.png` | 51.04 | 66.62 | 674 | 2981 | 5393 | 23876 |
| `1407200_615252_24adde2af4eb6f734552d4408f8870391b39dfd2.png` | 59.22 | 64.52 | 632 | 2955 | 5041 | 23637 |
| `1407200_615253_7a6e0c69233c8e00860a93a167a53764c54ffcfd.png` | 15.24 | 25.39 | 16374 | 16625 | 131039 | 133048 |
| `1411020_310829_5c4dc15dcb6ccedbd9e61d8af6248ef922e83eb1.png` | 10.36 | 11.64 | 66 | 4046 | 558 | 32388 |
| `1411020_310830_ee9305a11914e25392c09f4a497c31281fc15a18.png` | 10.51 | 25.34 | 21694 | 21923 | 173550 | 175388 |
| `1411020_310831_5b193581b484a1e1c6d12dabd9866ce3b357342d.png` | 3.51 | 11.62 | 41 | 3121 | 354 | 24975 |
| `1411020_310832_06aea368efdd86d6f7c733aac5a625ee207c14bc.png` | 17.33 | 12.75 | 180 | 3115 | 1464 | 24942 |
| `1411020_310833_54d52df250ef36d46ef0d6788f4cfbda99b1d312.png` | 1.46 | 11.62 | 19 | 3701 | 149 | 29614 |
| `1411020_310834_ae23ab5bc700e0a65126ba061781050b170c0247.png` | 1.74 | 11.61 | 24 | 3486 | 184 | 27867 |
| `1411020_310835_597dfaf6038d24bc9d1236d3f7683f55b688b6a4.png` | 1.91 | 11.62 | 18 | 3972 | 161 | 31812 |
| `1411020_310836_eb3364b2d5c2f1f214c6c50b73f8f241d2a4e73e.png` | 1.67 | 11.61 | 17 | 4783 | 153 | 38273 |
| `1418970_122280_e5dc068220be7647d2cf8f3cbd0f47cbbd1c10c3.png` | 35.28 | 23.36 | 405 | 2749 | 3249 | 22006 |
| `1418970_122281_942b27ac32eb2959867e31f5774dca4b3cce61b4.png` | 15.93 | 17.10 | 87 | 713 | 680 | 5724 |
| `1418970_122282_76fa5dff76db4b3e210165a5e8863570e7f236e4.png` | 2.86 | 11.61 | 67 | 108 | 522 | 852 |
| `1418970_122283_5d0adcceb909ede21beba6b9fc1283f03b4e7242.png` | 6.79 | 12.71 | 970 | 1667 | 7771 | 13338 |
| `1418970_122284_e09238996f38a85a3cc65b33b33f5f31fe3cb32e.png` | 5.87 | 11.61 | 1133 | 1652 | 9062 | 13201 |
| `1422650_1992054_f2d8078925d6603f27fe257c8e4b72782fe529fd.png` | 51.64 | 66.64 | 557 | 1076 | 4472 | 8597 |
| `1422650_1992055_c0213509c41cfd24182fdb880e6112ce5a6eae9d.png` | 24.98 | 26.56 | 291 | 538 | 2318 | 4315 |
| `1422650_1992056_fced50a092c725a15935773155f908c45f08a1b9.png` | 22.27 | 31.86 | 473 | 1112 | 3742 | 8887 |
| `1422650_1992057_00611e6976a5c95877402aba4939d7969615a3ab.png` | 9.71 | 15.85 | 175 | 538 | 1390 | 4282 |
| `1422650_1992058_2ab31e97a1d02eddd6e31a2820ffc4872da1aa40.png` | 26.36 | 24.37 | 244 | 746 | 1964 | 5993 |
| `1426210_223073_755356d2e57b8a6ecbda78fd446cf581edcd9f8b.png` | 2.47 | 11.60 | 26 | 967 | 188 | 7744 |
| `1426210_223074_5c13f221b3c49f6804d2fa59a9e43362fc2515cb.png` | 16.96 | 12.67 | 70 | 857 | 521 | 6828 |
| `1426210_223075_0f32f9cdf941934ecb28bf1c604ea480c4df2632.png` | 71.02 | 25.52 | 226 | 2471 | 1809 | 19741 |
| `1426450_240955_28837f3b1edfeda72e7e3ed53aec36c13e365f2c.png` | 35.46 | 44.41 | 294 | 589 | 2362 | 4715 |
| `1426450_240956_19e5fb3c13a0bc68403ac98eef18cbf7c961a8ed.png` | 20.43 | 34.89 | 259 | 1113 | 2057 | 8894 |
| `1426450_240957_bc2095db81b02335fea9cc7790d1d00778843f96.png` | 22.69 | 22.20 | 188 | 538 | 1515 | 4310 |
| `1430190_397385_8095e6f89e38b7f25fd9c79b25e1364cfc32d104.png` | 50.55 | 25.86 | 1580 | 2319 | 12635 | 18565 |
| `1430190_397386_2bc5124d4fe690fc901339c05872bcb1b8297958.png` | 1.90 | 11.59 | 109 | 117 | 871 | 933 |
| `1430190_397387_f45af5728fc4c2346f4fe11d7df1689c1b5d5b31.png` | 12.25 | 22.17 | 892 | 1985 | 7153 | 15882 |
| `1432050_155607_8a27d183319a1f1d5e1a58547e50363bf55d0f84.png` | 0.39 | 11.58 | 141 | 172 | 1138 | 1378 |
| `1435780_123028_0514dadbca67ecca8d647f7cbe419e8995a3d2b3.png` | 36.80 | 32.80 | 227 | 1054 | 1814 | 8423 |
| `1435780_123029_9303b462b8a552f595f774caddbf7de679ee0bb9.png` | 23.31 | 25.37 | 218 | 439 | 1741 | 3512 |
| `1435780_123030_b5c3a2279908ef60bffdfd2afd3b94ca2b228612.png` | 28.65 | 24.31 | 156 | 343 | 1223 | 2730 |
| `1435780_123031_16ec12fb558bbab583f664547894281bda9d0958.png` | 43.25 | 44.60 | 420 | 919 | 3383 | 7365 |
| `1435780_123032_9e4cff3b53ea7d6967306d9c75e1d91a75dc8e39.png` | 29.54 | 34.90 | 320 | 622 | 2543 | 4954 |
| `1435780_123033_e331c234f9c241118eff464a4a575da99625af91.png` | 21.59 | 26.42 | 263 | 388 | 2099 | 3104 |
| `1442530_422406_fbca1629000e748d39e54184dcc71454420101bd.png` | 7.77 | 20.12 | 305 | 398 | 2438 | 3197 |
| `1442530_422407_728b64e5e9eb60521b5e44b85525dcfe18364692.png` | 11.66 | 16.89 | 751 | 780 | 6000 | 6229 |
| `1442530_422408_cd85c0ab8e82bfe45baf7e29e3271a0673321f0c.png` | 9.52 | 26.45 | 1287 | 1318 | 10298 | 10535 |
| `1444300_156886_6d722bd8ecd1ff71d75ac0d35a08c7e8b2feef44.png` | 3.24 | 11.75 | 25 | 39 | 189 | 302 |
| `1444300_156887_20e879e615a1fe89216fc415d48f1a26fb961e4e.png` | 3.73 | 11.77 | 32 | 43 | 249 | 331 |
| `1451190_209696_12c1c9e2f9ec0319297e3f5f6e61870c82896be1.png` | 38.52 | 25.47 | 473 | 1897 | 3768 | 15158 |
| `1451190_209703_58462b3a85a4b6d0d727518299ac2df694fcdff7.png` | 13.29 | 22.19 | 2488 | 2514 | 19873 | 20081 |
| `1451190_209704_2d6dd009efbf2a11d1c3ecec2c032c6f920b1938.png` | 12.11 | 22.19 | 2182 | 2501 | 17437 | 20004 |
| `1472560_143583_0095ccffccea970618c9c190cacbae20af03556c.png` | 44.06 | 57.25 | 325 | 903 | 2625 | 7257 |
| `1472560_143585_e9462c67258d7d266e263bcd5815d473bddbef98.png` | 54.86 | 78.65 | 814 | 1538 | 6553 | 12304 |
| `1472560_143586_15738d83d36fa4247df65306c6962281b1c28350.png` | 40.16 | 31.96 | 347 | 407 | 2750 | 3260 |
| `1472560_143587_de64e0be8c5033cd5947aee78fe77b035f9840ef.png` | 73.94 | 72.02 | 799 | 901 | 6388 | 7208 |
| `1472560_143613_dcda88c8eee73c5e7661a951a18e9e6ab3d92ceb.png` | 52.43 | 62.48 | 544 | 2216 | 4339 | 17748 |
| `1472560_143614_c6430d220ed16b2bc34a0014030c95eaea2598c4.png` | 51.71 | 71.36 | 784 | 1355 | 6260 | 10846 |
| `1474200_221626_7caf10b4cf99f5516c53ce92bc578f6dd9b544bf.png` | 1.30 | 11.66 | 5 | 17 | 38 | 138 |
| `1474200_221627_24da4e7ea57851f503bc0c68f1e3622924d00918.png` | 1.32 | 11.64 | 4 | 23 | 35 | 177 |
| `1474200_221628_7c635a8bb09cecf3e646518f2537dd525867cd01.png` | 1.03 | 11.62 | 2 | 15 | 20 | 132 |
| `1474200_221629_a80c4f4979049325892bceaf783b04d32315904d.png` | 2.34 | 11.68 | 4 | 17 | 32 | 134 |
| `1474200_221630_44828b225d99655e1d76aa68fdd4635fbdf8173a.png` | 1.09 | 11.64 | 4 | 16 | 25 | 123 |
| `1474200_221631_057f2aa29afc2eee4ced044ef30ad78a212928e1.png` | 1.43 | 11.66 | 4 | 13 | 38 | 104 |
| `1478160_225415_95d2f7f2b004bc68c13f584cc92d20a89327348d.png` | 2.23 | 11.60 | 362 | 442 | 2790 | 3427 |
| `1478160_225416_6feb1117628adef1dcfd329028c7d709f43ab9e2.png` | 2.30 | 11.65 | 94 | 150 | 766 | 1222 |
| `1478160_225417_9b7b38cde7588330e79a7eac510127cc6af69cb7.png` | 1.24 | 11.59 | 106 | 132 | 845 | 1057 |
| `1478160_225418_bc589736e2c1c41ca56ca69ca2b6f0fa6bb0f33c.png` | 3.75 | 16.95 | 73 | 752 | 578 | 6022 |
| `1478160_225419_d16fa1fbe2601bdd1784fa9139278372842dccd7.png` | 2.23 | 11.59 | 225 | 229 | 1795 | 1826 |
| `1480560_143645_2a93fa9ced1b938f7011bd226d81986f9fbcce6d.png` | 30.32 | 26.70 | 223 | 1987 | 1797 | 15859 |
| `1480560_143646_86b014769ef14621c59fd521834c48a265c5b8b4.png` | 26.95 | 23.47 | 263 | 604 | 2129 | 4821 |
| `1480560_143647_bb1a6a20aa75dc2173ba624aebeaca3330c47f8d.png` | 30.21 | 34.95 | 342 | 1635 | 2704 | 13080 |
| `1492660_102594_3ac6381203d2272e3ce0dbb5edcd902d8b7820c9.png` | 2.58 | 11.62 | 401 | 936 | 3198 | 7474 |
| `1492660_102595_89a5767c0046579965d1f20b153e9af1636842d9.png` | 2.86 | 11.60 | 455 | 1228 | 3631 | 9804 |
| `1492660_102596_dc77821cf63bbe4c62e92390ad3425fbee886665.png` | 4.19 | 11.59 | 280 | 730 | 2220 | 5826 |
| `1492660_102597_fdb281844d9ed875676082b187c3de008d627027.png` | 4.87 | 11.60 | 315 | 645 | 2505 | 5151 |
| `1492660_102598_4daeea6f9fff64eaa7fcb4702c3d35cb8b386b35.png` | 6.04 | 12.68 | 1950 | 2216 | 15577 | 17702 |
| `1492660_102599_ab7c0f533fdb0af001f6aae589074eae396a7257.png` | 4.57 | 11.62 | 368 | 1445 | 2945 | 11551 |
| `1492660_102600_27265ceb67bda820724cde493c8116d66005fa4b.png` | 4.72 | 11.61 | 2428 | 2951 | 19436 | 23604 |
| `1492660_115372_9e2f7aab4b24e0874d61517559ce516ffe59f23d.png` | 12.89 | 11.63 | 5068 | 5494 | 40546 | 43952 |
| `1492660_115373_ac2f79fbf83fb63f07ac8d149208cf573278cfb5.png` | 4.33 | 11.64 | 952 | 1628 | 7626 | 13026 |
| `1499240_164472_11d92ecb539438d465463861964464581e3c3ddc.png` | 1.25 | 11.63 | 8 | 10 | 67 | 86 |
| `1499240_164473_c4d257243c8f0d0ff008802126522578090fdf21.png` | 1.17 | 11.69 | 3 | 22 | 21 | 180 |
| `1499240_164474_c32b6d242eaa5bef8f817aa15f8b9c07163ad9d3.png` | 0.87 | 11.64 | 12 | 15 | 88 | 126 |
| `1499240_164475_bc97ff3e361ca78632e8d661fafa75e4355045c8.png` | 0.57 | 11.62 | 2 | 7 | 20 | 61 |
| `1499240_164476_a965455a633d00dff5b4652f3cef8785cca1ad4c.png` | 2.02 | 11.70 | 18 | 36 | 148 | 284 |
| `1499240_164477_7f9ac16da2d9b4d81ad2ddf10d318f70ce2b2898.png` | 0.38 | 11.57 | 5 | 3 | 38 | 33 |
| `1499240_164478_757682cd22ea8bf2beac0a5c0cb575755f5c4d09.png` | 1.42 | 11.66 | 5 | 6 | 45 | 44 |
| `1499240_164479_0cc965d69e283bd7700b8a4c1346e187bd7512c9.png` | 1.01 | 11.70 | 6 | 7 | 46 | 58 |
| `1499240_164480_06936aca4be3ca0accc9068625d56b30c8cb1acb.png` | 0.85 | 11.63 | 4 | 19 | 24 | 147 |
| `1499240_164483_c42c64d731cb8cdfe097c7daa00677e4a5167c46.png` | 0.96 | 11.62 | 7 | 12 | 56 | 89 |
| `1502190_192159_8a2036fdb4a02b16b41b52d49134012b244d1a53.png` | 0.86 | 11.59 | 32 | 63 | 257 | 502 |
| `1502190_192160_86910d1be1a02cf67599a015065352a980d129b4.png` | 0.97 | 11.59 | 49 | 301 | 397 | 2414 |
| `1502190_192161_ad24ad3c4c14c3b032e16342b26e73ee96d025d6.png` | 5.89 | 11.64 | 32 | 116 | 249 | 918 |
| `1522820_125677_093618de67f3ec6dcea97f8e4ba4a39351470950.png` | 2.02 | 11.66 | 39 | 2829 | 306 | 22636 |
| `1522820_125678_1ae178920379cef87bed681995168d24956623c7.png` | 9.94 | 11.67 | 86 | 551 | 686 | 4414 |
| `1522820_125679_c26e82f195ec7107f21c5660c9c9c16f37c4ebdb.png` | 3.55 | 13.77 | 67 | 2115 | 560 | 16929 |
| `1522820_125680_9e0df047646aeba63ed51c38e40d8f5386c7035c.png` | 1.70 | 11.60 | 98 | 1071 | 794 | 8566 |
| `1522820_125681_795963bcb12eae726a85a76051a81b197e0b9035.png` | 8.30 | 11.72 | 56 | 4664 | 446 | 37297 |
| `1526200_104893_dd6c4f27d639bdb7c7ae18238dd7a9eea98a309d.png` | 1.20 | 11.61 | 30 | 287 | 229 | 2282 |
| `1526200_104894_073ca57787b4ea56ba1952d2304b16bb4e7c9a32.png` | 1.23 | 11.59 | 33 | 317 | 283 | 2555 |
| `1526200_104895_a06a06fdf0c0167b6f24bf9445cc32c9648b420e.png` | 3.09 | 12.66 | 302 | 598 | 2432 | 4808 |
| `1526200_104896_cf577a3505101fec400096e925ee803f9a459d22.png` | 2.49 | 12.68 | 145 | 403 | 1175 | 3266 |
| `1529220_183212_e162e148531c9a8090bdf85b1ef68807832d5102.png` | 29.23 | 15.89 | 248 | 1108 | 1994 | 8855 |
| `1529220_183213_9c248d0265b277d15e8b7ce88b4ade58cf7b2b8a.png` | 17.96 | 14.84 | 143 | 1209 | 1159 | 9683 |
| `1529220_183214_78d47b3ffadae78a93aa25f962291518328fb49b.png` | 26.05 | 15.91 | 192 | 1086 | 1553 | 8689 |
| `1529220_183215_6ba134e52a2c0567a7843a5ab069eefeed0411f7.png` | 19.71 | 16.99 | 152 | 1170 | 1228 | 9380 |
| `1529220_183216_cd0cda01e8e8020a65b6a74976f3b18c2838b0cc.png` | 11.38 | 11.62 | 78 | 540 | 623 | 4328 |
| `1529220_183217_ac9e0774790152f00f74c13671c27543bcd90d7f.png` | 12.70 | 11.63 | 82 | 531 | 674 | 4252 |
| `1529220_183218_7e8e02770ff741a416b5e6af1da9398833538de4.png` | 10.89 | 11.63 | 70 | 485 | 549 | 3887 |
| `1529220_183219_f901a274ad2505bd900cbf539adf4b57c6151881.png` | 11.14 | 11.61 | 80 | 489 | 642 | 3914 |
| `1532770_134362_1e5991ab6c595c1a1ec77920b835de431cd81900.png` | 59.42 | 64.59 | 159 | 474 | 1272 | 3795 |
| `1532770_134363_1b1775003ebc933e996c5d903cc772f1b08b1bcd.png` | 28.90 | 34.99 | 219 | 383 | 1744 | 3055 |
| `1532770_134364_0e638373b5f3058526308e1b58d101d42c0bd10d.png` | 44.30 | 48.79 | 238 | 399 | 1891 | 3185 |
| `1536470_252476_267e0513135a7fd46d6c3d803c4b7c2a1d0e91c4.png` | 9.59 | 11.63 | 388 | 1506 | 3086 | 12043 |
| `1536470_252477_3da48acbf9785e51b054fe67afba9b126e34724e.png` | 4.94 | 13.84 | 79 | 982 | 628 | 7863 |
| `1536470_252478_1db44fa1b821b6e0661a4f4eb305b2a08a52b64e.png` | 15.76 | 11.65 | 461 | 1370 | 3717 | 10973 |
| `1536470_252479_1296adb61d496cda043ef09d9bca64ca341427a2.png` | 10.44 | 14.80 | 432 | 1056 | 3433 | 8446 |
| `1554600_218937_ef53d40c41d8e2faab74658e00dd0c2f9ad4d168.png` | 0.51 | 11.66 | 2 | 2 | 15 | 21 |
| `1554600_218938_404d6a08a835b53ca1630e8384d3ce1fc69ad75b.png` | 0.45 | 11.63 | 1 | 1 | 8 | 8 |
| `1554600_218939_8d67b79609d98a4f50d84a970a4c07cbe26c9781.png` | 0.75 | 11.60 | 3 | 5 | 23 | 41 |
| `1554600_218940_2ccd37b847d2a8e0c28d35789e9bddde00281acb.png` | 0.64 | 11.63 | 1 | 9 | 10 | 89 |
| `1554600_218941_42baab6b3b32017d9be2a7d82917aa7645cced58.png` | 0.72 | 11.63 | 3 | 5 | 20 | 42 |
| `1554600_218942_8c31ca571b55d13ce1de1a9bce4357605273ccb5.png` | 0.86 | 11.59 | 3 | 3 | 22 | 21 |
| `1554600_218943_08a6eaf8947a85a94cbaf2e016ea8bdb01ae1d03.png` | 1.11 | 11.66 | 3 | 4 | 22 | 36 |
| `1554600_218947_1a4391680b34826e2eaa0e00c89e61d6177c20f7.png` | 2.20 | 11.69 | 16 | 20 | 124 | 151 |
| `1554600_218948_f48c3f0b3530efd506947a7be52f87ed7328993d.png` | 0.75 | 11.67 | 4 | 7 | 24 | 51 |
| `1554600_218949_3fc9a8892bad076716b1cc13bb6eaf7f7f5cae5f.png` | 0.91 | 11.64 | 4 | 10 | 20 | 86 |
| `1554600_218950_cd427088017892e219d0177bc172524d2a8315f4.png` | 1.04 | 11.65 | 3 | 14 | 32 | 109 |
| `1554600_218951_97b0db617f42f3b46d4b4a3586013e11f969936a.png` | 0.89 | 11.64 | 7 | 12 | 43 | 83 |
| `1554600_218952_9e91de1b2a9340aa4aa1cbae9cba0c0cba83379f.png` | 4.38 | 11.67 | 22 | 32 | 176 | 262 |
| `1554600_218953_ea5076664d4f249b8fa6d6e8e1c2dbbde60834be.png` | 1.76 | 11.68 | 12 | 21 | 88 | 164 |
| `1554600_218954_3d507783e1acfb651fbb41f3d8aa62f8942a0cf7.png` | 4.60 | 11.71 | 26 | 41 | 211 | 315 |
| `1554600_218955_ff89094d9280946ac81d0874c2fa86585a19e643.png` | 1.64 | 11.69 | 6 | 22 | 39 | 173 |
| `1554600_218956_1bd117f41b4bc5531d872c9f39dbfa260fd2269e.png` | 1.82 | 11.72 | 4 | 13 | 36 | 110 |
| `1554600_218957_7b6f51fbcefd5ad5e6cd6ff25272ca0998ff0650.png` | 1.87 | 11.67 | 10 | 21 | 90 | 157 |
| `1554600_218958_cbf9832509b46ef5dd1fd9a32174d6492a1731ad.png` | 2.28 | 11.71 | 10 | 13 | 97 | 108 |
| `1554600_218959_ae96ca2a86af6922a8bbef4de852b700a195efc2.png` | 1.82 | 11.67 | 6 | 13 | 49 | 111 |
| `1554600_218960_2566fa7467b70039ddbb100fe3526980c775b88f.png` | 1.39 | 11.67 | 8 | 17 | 46 | 131 |
| `1554600_218961_6e9edd85f363cc08b2365df02b3bd2c23a3db332.png` | 1.50 | 11.65 | 7 | 18 | 58 | 145 |
| `1554600_218962_c486265c6dc23260205b43ab56793abc475545f1.png` | 1.65 | 11.66 | 7 | 13 | 63 | 108 |
| `1554600_218963_8666280cd5b28f25797812910cde5abcafd6fa9b.png` | 1.38 | 11.65 | 5 | 14 | 41 | 110 |
| `1554600_218964_894f755fac3c925612cc22272a414ef804c7e2b4.png` | 1.60 | 11.68 | 3 | 14 | 29 | 114 |
| `1567080_200344_0aef8b7eb910fd53a4fd45002a037ab67213fc4a.png` | 44.40 | 46.60 | 403 | 878 | 3223 | 7006 |
| `1569520_226343_3c730119d64d8cf7e2ece469d4df2b5e4b22f13a.png` | 0.65 | 11.62 | 4 | 10 | 34 | 75 |
| `1574580_153545_ba9db478ed767dd480c69e9c70411d9b9d63e71c.png` | 65.68 | 27.61 | 262 | 1114 | 2054 | 8925 |
| `1574580_153546_99021ac3f709f9d5369861c50cbf301c3be913ae.png` | 120.10 | 51.09 | 415 | 1668 | 3312 | 13309 |
| `1574580_153547_c1bb9a5283a78fafb358449d09611cd7346efaf0.png` | 59.69 | 37.17 | 264 | 1289 | 2133 | 10299 |
| `1574580_153548_348e98c8c8e61532fbedabaab68be685259a795b.png` | 75.17 | 47.87 | 441 | 2029 | 3570 | 16266 |
| `1574580_153551_84ac7a440a221868ee8db2578391fbee8e340ce8.png` | 102.89 | 49.83 | 403 | 1850 | 3262 | 14825 |
| `1575450_222916_cae68c09f3490ae5938421cb5f1d57b333809a82.png` | 56.35 | 75.28 | 568 | 841 | 4527 | 6722 |
| `1583500_212572_6d35853d9b1932e75f6b0c7298f1eb2fcc8aecdf.png` | 2.71 | 14.82 | 37 | 1852 | 309 | 14835 |
| `1593500_152803_c5bf60ebc4e944a943616c4733f0ba1e0e8b8c1a.png` | 3.23 | 11.60 | 12 | 796 | 86 | 6365 |
| `1593500_152804_b9243b4bcb2a466983408e775c91c5cea3cbbde3.png` | 2.78 | 11.59 | 11 | 382 | 90 | 3057 |
| `1593500_152805_42b66f51c132a575ee17cf4fb4063c4e938c63a3.png` | 6.10 | 11.65 | 29 | 674 | 229 | 5403 |
| `1593500_152806_49106206ae2e76aa33a13af82567fee28f11e50c.png` | 4.79 | 11.60 | 17 | 887 | 131 | 7084 |
| `1593500_152807_d68779cd0a09a50046ad791733fca4290a7bd0cb.png` | 3.55 | 11.70 | 26 | 639 | 202 | 5120 |
| `1594460_188480_e70844a60bc77f05c4793eb15a7d72f5dde5dc9b.png` | 12.10 | 17.02 | 179 | 938 | 1406 | 7488 |
| `1594460_188481_92e5d58b89f1cdddcf89b12ec2d17d16b0574a27.png` | 46.31 | 31.75 | 303 | 2030 | 2418 | 16283 |
| `1595290_162573_0dc264ff3bb336668acff43e0756857e20515187.png` | 0.01 | error | error | error | error | error |
| `1595290_162574_465b32c1740a67d0829e080b3a75d634b60f288e.png` | 0.01 | error | error | error | error | error |
| `1604030_190918_f02dfef5a555c46adfe7044b3c3213fee9ec0f34.png` | 4.03 | 12.66 | 1182 | 1515 | 9460 | 12124 |
| `1604030_190919_cdfceddc86899c39cf903d9e1e0497182986be2b.png` | 2.40 | 11.62 | 1355 | 1582 | 10843 | 12658 |
| `1604030_190920_d22fbbcc565c1f4f597c35c65ad9770562dd3c3d.png` | 5.05 | 16.93 | 2071 | 2124 | 16561 | 16982 |
| `1604030_190921_fcb6bf587c06b7bf1ad19b1bcc7b582d2f32bdf9.png` | 2.60 | 11.62 | 586 | 704 | 4685 | 5643 |
| `1604380_372754_8092a329c132e7e8bf2cdb0db4d2508582c8ee84.png` | 3.32 | 11.60 | 299 | 313 | 2171 | 2283 |
| `1604380_372755_0923df36ee48689ac3109454cd12064e218d6a48.png` | 4.06 | 11.60 | 121 | 157 | 738 | 1027 |
| `1604380_372756_2ff05602dcdfc018c6f79adecbb86712da1422bb.png` | 2.58 | 11.60 | 199 | 201 | 1587 | 1606 |
| `1604380_372757_c552cff68bf19c868f5fc13770ef4396d208a00d.png` | 3.85 | 11.60 | 545 | 545 | 4137 | 4137 |
| `1604380_372758_6a88168fed4bf34956c13be37866d35938368fce.png` | 2.36 | 11.59 | 176 | 176 | 1412 | 1412 |
| `1606180_414796_8d03183291794c17fab5d135a924713baff8c4f9.png` | 2.12 | 11.60 | 413 | 413 | 2600 | 2600 |
| `1606180_414797_db7df105763f4ec5635f9ddf36912cf2b4243bce.png` | 2.00 | 11.59 | 577 | 578 | 3397 | 3404 |
| `1606180_414798_14fae73d9c81f6f78de22e9e45f588495aa872ac.png` | 1.79 | 11.59 | 520 | 520 | 3105 | 3105 |
| `1606180_414799_2a3593630ddeba805dc452b8985c0b24073856c9.png` | 6.82 | 11.59 | 870 | 883 | 4761 | 4860 |
| `1606180_414800_d351fd68a3c0f63943dc6dabb1eaabf255475967.png` | 6.35 | 11.59 | 591 | 640 | 3379 | 3780 |
| `1629910_216002_68775ee14dcd1978fea0015e30edcb5ec6061577.png` | 1.93 | 11.58 | 184 | 230 | 1472 | 1837 |
| `1629910_216012_dc5ec8be281cde8b9b081ace7a6febf984573e7e.png` | 3.14 | 18.99 | 689 | 691 | 5513 | 5528 |
| `1629910_216013_b886e90e3d8577b3585d21424287632e66cc2ddb.png` | 1.18 | 11.60 | 124 | 202 | 996 | 1625 |
| `1629910_216014_d31e4f1b8165cc3942d2c4f3e38e76cce857d710.png` | 9.15 | 12.64 | 1052 | 1086 | 8434 | 8707 |
| `1644500_219934_e5c5c1c2ad3a8330e61870a41ca883c8cd43962c.png` | 180.21 | 69.06 | 943 | 2166 | 7528 | 17330 |
| `1644500_219935_8ce027efae46316d34a414f655bbc8a467e77a5a.png` | 99.88 | 47.10 | 645 | 888 | 5254 | 7133 |
| `1644500_219936_57fad7001946811af7f3de3467fc442fea3d1264.png` | 180.61 | 134.58 | 3489 | 9474 | 27884 | 75764 |
| `1644500_219937_3936f33cf6675a0fa00f9e61ba7d797510b1024c.png` | 159.72 | 39.00 | 616 | 681 | 4941 | 5417 |
| `1644500_219938_bb3573c8b272974a81295520593457c38587316a.png` | 180.37 | 50.03 | 826 | 967 | 6579 | 7722 |
| `1644500_219939_21c034de20c080baee1673843508c6f67de58428.png` | 180.19 | 41.83 | 495 | 459 | 3972 | 3729 |
| `1644500_219940_05c364eaeae4df990c95680b944aaaf45e1c15c9.png` | 180.04 | 55.43 | 825 | 2056 | 6651 | 16517 |
| `1644500_219941_c161c6c79f6103c0277b05b88a298dda038e07a5.png` | 73.41 | 23.23 | 218 | 323 | 1716 | 2587 |
| `1644500_219942_8d9e705389d589d3794d1fea329266d734f26580.png` | 180.14 | 48.79 | 912 | 827 | 7316 | 6629 |
| `1644500_219943_85a5d333a0aefbd4498800129ec20bea7a61fee1.png` | 180.10 | 40.53 | 670 | 569 | 5368 | 4520 |
| `1644500_219944_0c94fda1768f1cc8911ed057bd90cf090efd18d7.png` | 24.23 | 18.22 | 65 | 99 | 529 | 801 |
| `1644500_219945_f4ab60f4aacdb28d71ceadb340233fee20400b07.png` | 40.96 | 20.10 | 181 | 140 | 1457 | 1116 |
| `1651960_158999_318fd87c8fc6fbaf6413f7d6f9ec5264aa9166f1.png` | 10.75 | 27.47 | 1447 | 1515 | 10675 | 11224 |
| `1651960_159000_24393e27ad63e42cb0018f239237374ecadb0870.png` | 8.73 | 26.41 | 1715 | 1759 | 13714 | 14068 |
| `1651960_159001_05fb9d32d8fc8c6760272c60ff4071080437f92a.png` | 8.05 | 22.20 | 1068 | 1071 | 8540 | 8566 |
| `1651960_159002_de148893bf0208fc9fc1acf149aa73cbdd0cf17e.png` | 3.84 | 11.61 | 109 | 243 | 869 | 1933 |
| `1651960_159003_fe3f513ab8ab11677b3716fa7da5abe15cbf3506.png` | 5.28 | 16.88 | 1014 | 1015 | 8111 | 8116 |
| `1651960_159004_f9ada27a24cad6bc9b1b3d7e25a76917e94e6f71.png` | 7.07 | 20.10 | 951 | 960 | 7598 | 7672 |
| `1651960_159005_082e8b9fdf439bcdbf2b5ee9ba2ffa05e68ffdb0.png` | 8.07 | 23.25 | 1205 | 1205 | 9645 | 9645 |
| `1651960_159006_0bef50031e1a98a4a64236f028df1c04bbd3ba04.png` | 8.06 | 24.29 | 1655 | 1657 | 13248 | 13269 |
| `1657630_1147294_5c42299e7102a9492f257a5b92b779096b14a274.png` | 3.52 | 11.59 | 209 | 234 | 1666 | 1862 |
| `1657630_441016_075d2d988b113b82f167933f5be440fb1d385cf9.png` | 17.57 | 30.64 | 312 | 1323 | 2508 | 10587 |
| `1657630_441017_627bc54e4e83f41fb71ee4e04982d71a31816fc7.png` | 4.55 | 12.66 | 1060 | 1093 | 8471 | 8732 |
| `1660840_310307_65aedbada889e266ed65b260f2668983f5cc71f6.png` | 33.76 | 30.67 | 441 | 3375 | 3524 | 26980 |
| `1660840_310308_2a7a453063663ac0f54118d56822ad60dbb1c5f7.png` | 8.83 | 11.59 | 1027 | 1209 | 8214 | 9684 |
| `1660840_310314_1852e82b215e3fd69ade04dab2856e7648c317a1.png` | 12.66 | 13.72 | 316 | 1072 | 2507 | 8552 |
| `1664220_213350_b02e5427f5be6cba1e9cf9b35f51e45e120510f4.png` | 1.07 | 11.62 | 14 | 2917 | 124 | 23331 |
| `1664220_213354_f8373c1a000eb11658845c95ddd8e3c9661b0a0d.png` | 1.61 | 11.61 | 18 | 5820 | 147 | 46559 |
| `1664220_213355_d4f8fd424e62ad75fe3f794223927beef3afdb80.png` | 4.08 | 11.63 | 1735 | 4914 | 13865 | 39317 |
| `1669000_1532111_741182d224ceceadd97971d225bdf07293b8cd1a.png` | 5.87 | 14.77 | 423 | 757 | 3369 | 6054 |
| `1669000_1532112_c6db29a9803f77c0e358b81fe464d5c3d54c09e4.png` | 1.58 | 11.59 | 83 | 91 | 654 | 723 |
| `1676840_368135_292d93fef56d81c3f6dfeb518c164d9958795140.png` | 10.88 | 15.83 | 172 | 554 | 1366 | 4422 |
| `1676840_368136_5b1db4d77e0074fdf97825089d6e0aef724a8d83.png` | 12.33 | 12.67 | 73 | 564 | 585 | 4503 |
| `1676840_368137_d21b2469ece827b7457f0457811298f414ae77c1.png` | 8.53 | 11.60 | 66 | 521 | 536 | 4175 |
| `1676840_368138_c9a7491e6d6a3e5714110639a2913298f6152d39.png` | 3.27 | 11.60 | 118 | 185 | 933 | 1477 |
| `1676840_368139_1266dc7addcff0abfd956ba96685b4358216c675.png` | 4.91 | 11.59 | 51 | 384 | 394 | 3059 |
| `1676840_368140_1bbabe7f93477e5d1505bf436637dd14294e0f6b.png` | 10.91 | 13.73 | 121 | 465 | 977 | 3728 |
| `1677770_198316_c5bb68badc041c8abed44d6c771bf10b3faf67c9.png` | 26.20 | 14.80 | 126 | 1033 | 1022 | 8245 |
| `1677770_198317_09d596bb0889a27927afe9572194088322fe4a8d.png` | 20.31 | 13.72 | 179 | 1008 | 1453 | 8127 |
| `1677770_198318_5a4c21fad978f8b0c2f10ac95809a172a01b722c.png` | 20.98 | 11.61 | 64 | 468 | 517 | 3739 |
| `1677770_198319_f7b7399f6e5a7a5e1b18579306f514a760c43858.png` | 26.61 | 26.43 | 183 | 1305 | 1469 | 10426 |
| `1684100_293490_02e6c81271f23421b514fd8a9598fa7f7bf0f14c.png` | 1.59 | 11.59 | 490 | 491 | 3020 | 3034 |
| `1684100_293491_47a81fd2d1a5c6efe4bfb9cf3699e0b00a1a0d86.png` | 4.48 | 16.88 | 1468 | 1606 | 7765 | 8887 |
| `1684100_293492_6292c6cae4c1c5273798b5c9a3678c91fbedbfee.png` | 2.39 | 11.59 | 80 | 153 | 649 | 1233 |
| `1684100_293493_6877cc4f4fef799fc29b1524a92dc7103e5a4b26.png` | 11.31 | 23.25 | 523 | 593 | 4187 | 4746 |
| `1684100_293494_f452f98364653e82d099f0760863af7d24635ca6.png` | 2.90 | 11.59 | 107 | 185 | 874 | 1492 |
| `1684100_293495_9a158691c28eb4c25dd6894fb4061429485152d0.png` | 1.65 | 11.59 | 750 | 751 | 5997 | 6011 |
| `1684100_293496_88bf89aba2445e38692226203b17151e59523860.png` | 0.32 | 11.58 | 15 | 45 | 125 | 364 |
| `1684100_293497_cca7c98f678f29cb609a8913731a44da1753d805.png` | 1.22 | 11.59 | 74 | 120 | 597 | 961 |
| `1684100_293498_7b11544fcb79a5563f3636de26dfd9afe28799a6.png` | 0.89 | 11.58 | 146 | 146 | 1173 | 1173 |
| `1684100_293499_758ae216c5aac517c81d5a12f43591b83986a59f.png` | 2.63 | 11.58 | 106 | 123 | 847 | 987 |
| `1684100_293500_06e99edc42bde43b0d3c4a80ec130f8e1b600d5e.png` | 0.72 | 11.59 | 99 | 108 | 787 | 860 |
| `1684100_293501_e3b9ba29239c9a646116df63fd8e29020d3b69a6.png` | 0.81 | 11.58 | 31 | 43 | 252 | 349 |
| `1684100_293502_7f658a8442b741515db0266711fd8e911ad3f2b0.png` | 1.48 | 11.59 | 46 | 75 | 364 | 596 |
| `1684100_293503_7a3e5d0c2b1bb23d6dcb8ba64fa667d5bf0ff378.png` | 1.68 | 11.58 | 43 | 61 | 349 | 491 |
| `1684100_293504_906c5fd09e3b58162bfc9c52f2cdf3618e40513f.png` | 0.30 | 11.58 | 48 | 60 | 379 | 480 |
| `1684670_167103_6003695f83b9d72eb8367258854cfe4d9587a3d9.png` | 7.13 | 14.78 | 1382 | 1842 | 11066 | 14730 |
| `1684670_167104_a2c83838b9565df2cdc9a04c121b41999bf28b66.png` | 4.17 | 11.59 | 1015 | 1198 | 8118 | 9588 |
| `1684670_167105_b8ece30eea7ece983b09412b197f0a3c33d6bc12.png` | 7.30 | 14.78 | 1245 | 1573 | 9972 | 12589 |
| `1684670_167106_0859178435800d6b5a006028b547dc9f0d55dbfd.png` | 39.41 | 46.46 | 19164 | 19598 | 153310 | 156773 |
| `1684670_167107_c5d693022a4000d9c45ffdb6bc38b8602a69b0b4.png` | 19.83 | 11.60 | 4097 | 4115 | 32784 | 32925 |
| `1684670_167108_127b51f3b5711aed11ad60945dd4483c4315d5e8.png` | 4.74 | 11.60 | 753 | 952 | 6028 | 7616 |
| `1688580_159085_3e2129a177757c3881d8e0862f14eeab1681f76f.png` | 0.99 | 11.59 | 114 | 141 | 914 | 1126 |
| `1688580_1992926_658fd693a4d290bba0a866446e16e8580ef0221a.png` | 1.35 | 11.58 | 101 | 107 | 809 | 858 |
| `1688580_1992927_b0e233ecbe68c717558c8818785cc0ecfb7762eb.png` | 1.00 | 11.59 | 57 | 78 | 446 | 617 |
| `1693980_223076_3800c0193a48a8cc47f28ec133ddaefca3ee0137.png` | 68.76 | 21.30 | 395 | 3261 | 3199 | 26086 |
| `1693980_223077_d485d8e244008fdcf5c97d1b557b1788bbdbaf74.png` | 61.79 | 25.60 | 238 | 3673 | 1890 | 29348 |
| `1693980_223078_784643d307a5131fe8954f27eeaa7bb46d0a8c21.png` | 79.42 | 39.32 | 462 | 3327 | 3730 | 26629 |
| `1693980_223079_ac8947b7a50e89e51d2b2e41f9ecfe483127042c.png` | 26.21 | 15.96 | 157 | 1250 | 1272 | 10018 |
| `1693980_223080_b8608057a48217caa9fcfa74f10956191e0d2abe.png` | 61.17 | 29.87 | 303 | 6798 | 2387 | 54372 |
| `1693980_223081_b1a83a3661ee50764ff6f20c94c00a715b2fffee.png` | 77.01 | 29.86 | 390 | 3184 | 3119 | 25522 |
| `1693980_223082_e32d4de0c76d082ffe982ae45572ca8e603f80b6.png` | 53.88 | 36.03 | 887 | 2168 | 7180 | 17386 |
| `1697700_193305_6a8046f57c57d63460ed60a3a3b40c6856d1072f.png` | 5.79 | 11.66 | 82 | 139 | 659 | 1101 |
| `1708520_181368_6a2fbad108b7bb2fad7b2a5fbc8fbc44588ee2cb.png` | 28.25 | 20.15 | 458 | 1359 | 3692 | 10935 |
| `1708520_181369_b97a1b43cef5ec11d33fbd6e7548d03bb302e9ae.png` | 44.33 | 23.38 | 643 | 1742 | 5158 | 13950 |
| `1708520_181370_9206c16d28632bb8b9a773c119105b278594d535.png` | 45.83 | 72.98 | 884 | 2707 | 7112 | 21654 |
| `1708520_181371_cb8aa575f430a066832e4de59ddc8f20af96e689.png` | 36.14 | 27.55 | 382 | 2823 | 3079 | 22608 |
| `1708520_181372_0745b330493e107d6b225ad940905d2e60b241e1.png` | 47.21 | 27.52 | 290 | 3122 | 2309 | 24950 |
| `1708520_181373_a545613b6b5debdeb1aad205cda89629f758a7dc.png` | 20.02 | 16.09 | 380 | 492 | 3054 | 3945 |
| `1714420_267533_90362607cad59d329b5e0ccff806aefefd9b2619.png` | 2.39 | 11.59 | 22 | 60 | 169 | 479 |
| `1714420_267539_6e06017b3eb84dc5b3a34c2c8e45d2fe88bdd841.png` | 2.78 | 11.59 | 24 | 70 | 185 | 538 |
| `1716740_241338_1052284c24bda824adf6ed31155060e343d6c792.png` | 3.06 | 11.62 | 83 | 667 | 648 | 5349 |
| `1716740_241339_4adda10de162d7c334472088bb96985b79b49f4b.png` | 5.76 | 13.75 | 136 | 978 | 1092 | 7842 |
| `1716740_241340_87d43b0ea9e19da66c2b745ff744271db0230e78.png` | 7.32 | 12.69 | 73 | 1301 | 580 | 10389 |
| `1717770_372998_81f45dddc051bd445fcffe56bb3060590693f51e.png` | 3.95 | 11.57 | 396 | 626 | 3168 | 5004 |
| `1717770_373000_556ddd483475357b25d300a258e90d6bb15921a7.png` | 13.71 | 20.05 | 108 | 957 | 865 | 7665 |
| `1724050_155696_ce29ecd53a5c81585361c469231945e5c1b6bffe.png` | 37.20 | 42.34 | 66 | 426 | 546 | 3464 |
| `1724050_155697_a49ef354be93b53353cbd70981972ad32dbb580b.png` | 58.17 | 52.99 | 286 | 406 | 2326 | 3281 |
| `1724050_155698_00bf66bc4648deaf5a99d1cda6f24bb1539b34b6.png` | 60.69 | 51.88 | 275 | 643 | 2166 | 5097 |
| `1726640_383343_cf66ab081cbe0010d0610ad11e2b185ea5f1999b.png` | 0.01 | error | error | error | error | error |
| `1726640_383344_8279ddf166f6eec4c286f53daed4fb5c9fb07f24.png` | 0.01 | error | error | error | error | error |
| `1726640_383345_8605636230826e6a0d99309bfe693fd98aa0af00.png` | 0.01 | error | error | error | error | error |
| `1726640_383346_72afaaf442a1c7066a13e1525eacf7e03af3e5d2.png` | 0.00 | error | error | error | error | error |
| `1742930_252569_2dad1770471af2110d4804fa735a02123b00fca1.png` | 5.11 | 11.59 | 123 | 368 | 996 | 2950 |
| `1742930_252570_0d207b7aea2078fda3f8461393e890fa752cc907.png` | 1.95 | 11.59 | 185 | 185 | 1488 | 1488 |
| `1742930_252571_b1c31878e8428380f292a9e6ed61f050e0d3a387.png` | 2.78 | 11.59 | 46 | 120 | 365 | 955 |
| `1742930_252572_70b60180eab3aeed049afdc6ad9320ea0316ef74.png` | 4.75 | 11.59 | 156 | 203 | 1256 | 1630 |
| `1742930_252573_d0f4dcee4530de0c5d5de9b9ebfe5b748bcbb34e.png` | 1.85 | 11.59 | 32 | 107 | 260 | 866 |
| `1742930_383287_c60c05a8f0a7986562e1a21dfcb6821583c10ec5.png` | 1.96 | 11.59 | 73 | 158 | 578 | 1261 |
| `1742930_383288_27b052430f5d30d9ac54aead92232f478441851e.png` | 1.24 | 11.59 | 191 | 270 | 1547 | 2175 |
| `1742930_383289_e32a81a8581c5c97470bce432f8282d40c476f5b.png` | 8.12 | 11.59 | 373 | 388 | 2993 | 3117 |
| `1749770_308267_85f35df7c2103645c6cc7ed327fe808644f2db10.png` | 19.21 | 21.10 | 4186 | 4210 | 33311 | 33502 |
| `1749770_308268_34c2248de112c8f282f850c8b83f7790c40a5a24.png` | 5.70 | 11.58 | 1144 | 1227 | 8957 | 9624 |
| `1750200_190776_4b0f524c7d270f1410d765bf888e9f2ad362d4cf.png` | 48.81 | 71.95 | 204 | 474 | 1627 | 3812 |
| `1750200_190777_2be04f542b69a4bffdbacbbe44c41b2c9cf9d570.png` | 41.19 | 45.51 | 114 | 981 | 903 | 7890 |
| `1750200_190778_988cc1c1cab855313123fec42308ab2f25489d86.png` | 79.89 | 76.16 | 271 | 952 | 2158 | 7586 |
| `1768020_140975_a61c16a714f3fc7f6616e3759cd932e5bcdd1f4e.png` | 5.16 | 12.66 | 319 | 456 | 2553 | 3639 |
| `1768020_140978_2f032b904876df0e87c40764cd2529d48a5526a2.png` | 6.22 | 11.58 | 385 | 689 | 3074 | 5509 |
| `1768020_140979_1f4185d87c5f129512c38ec7e404ec8cb917fecd.png` | 16.57 | 17.96 | 592 | 1222 | 4729 | 9805 |
| `1768020_140980_782f6fcee2cfd2ecb76b1ac7a68a68797aa5505e.png` | 5.21 | 14.77 | 171 | 345 | 1391 | 2772 |
| `1768020_140981_4173c7e0ef13dd14c8728de270c47ba5c6c08934.png` | 5.78 | 11.60 | 313 | 586 | 2504 | 4687 |
| `1768020_140982_a2205f4977cefe3355fd6f5893610c40278508e2.png` | 4.59 | 11.59 | 250 | 544 | 2007 | 4346 |
| `1768020_140983_5f8adf264f84dce6d6167a7a20070a28c2f74090.png` | 6.59 | 12.67 | 259 | 713 | 2077 | 5700 |
| `1771300_361246_3cac1a7390372f9e9e8396121810094105522839.png` | 4.00 | 11.60 | 117 | 325 | 909 | 2582 |
| `1771300_361247_219b2d44da9d462c20d2704e6135f35bc7774c93.png` | 6.73 | 11.60 | 1583 | 1593 | 7038 | 7111 |
| `1771300_361248_cfbdf0b2baa3d70d87d9e85b93bfaba5cb260b0b.png` | 3.36 | 13.72 | 166 | 624 | 1362 | 5027 |
| `1771300_361249_1d5954d1477c6847e16c95c9465f90500ad1d89f.png` | 12.15 | 12.66 | 931 | 1416 | 7446 | 11313 |
| `1771300_361250_6c8bb35bdaa2835fee6394e8caa60fde6eb8a3cf.png` | 1.52 | 11.59 | 1087 | 1091 | 5689 | 5714 |
| `1771300_361251_a20439432f1866a86cb95d631498ce01bde7e3ff.png` | 6.64 | 11.60 | 877 | 1646 | 7016 | 13178 |
| `1771300_361252_c8bb0b1f9f27deb9889ca3d4cc68ae6799ee173d.png` | 5.26 | 16.88 | 860 | 1407 | 6885 | 11264 |
| `1782460_416267_b8510b0dcfcdf031bab97fcb411d3318044cb9ac.png` | 10.21 | 20.06 | 798 | 829 | 6386 | 6634 |
| `1782460_416268_c1e60a626e9e6911f7d8739c9a8f6b3d47a772d2.png` | 11.06 | 22.16 | 285 | 1131 | 2310 | 9056 |
| `1782460_416269_df22be8dcc34b0da76ee76f5a3644dc4167513c4.png` | 9.69 | 15.82 | 87 | 671 | 682 | 5371 |
| `1782460_416270_691bca1234ae89732105623635894e93df50960e.png` | 4.79 | 11.77 | 197 | 250 | 1579 | 2000 |
| `1792490_146999_9fee5262c237f680ab63f8ec78333cde65fe1818.png` | 2.91 | 11.61 | 41 | 522 | 353 | 4200 |
| `1792490_147006_b561b673e40595b1a91d2c1cdffb915b30066803.png` | 5.77 | 15.84 | 1028 | 1373 | 8236 | 11024 |
| `1792490_147007_7e4b487ff5dd0d29bed459abf237e82472d54fff.png` | 22.35 | 15.86 | 1135 | 1175 | 9072 | 9387 |
| `1792490_147008_8536974ccbdce6d22c2c005a891fcea4ce86e14b.png` | 3.41 | 13.73 | 267 | 462 | 2113 | 3680 |
| `1792490_147009_c5f30248d1795c0c49bf768dfe138eff4a26e07f.png` | 6.37 | 22.18 | 193 | 792 | 1536 | 6321 |
| `1792490_147010_5cfee78b6f284a43280bef6403951e09e01e3fdd.png` | 6.73 | 24.32 | 348 | 841 | 2803 | 6736 |
| `1792490_147011_2764138d6eedf58458eaa4f08ae9efc055bada24.png` | 6.91 | 27.50 | 193 | 1346 | 1563 | 10773 |
| `1792490_147012_26e37b689fa1b1572311522ff6da5254549c0fe2.png` | 10.90 | 41.24 | 1843 | 2697 | 14778 | 21589 |
| `1792490_147013_08dd88bbaea21e8939b4cd71f3b0dcb686652b79.png` | 8.10 | 24.31 | 549 | 1122 | 4391 | 8985 |
| `1792490_147014_ebe0138bd34ca06de61c60b2a87ea49f0ca8533c.png` | 21.40 | 16.92 | 874 | 1666 | 6987 | 13315 |
| `1793250_354135_627d3795598ef05b2057c17108a3a192f8a2ddcc.png` | 2.03 | 11.60 | 461 | 461 | 3695 | 3695 |
| `1793250_354136_852e40a123b55bd1c2fc60daefd936db084ad2b9.png` | 24.92 | 28.52 | 1405 | 2231 | 11214 | 17837 |
| `1793250_354137_a319455ac1ea24fd201678184742e6d967888ade.png` | 10.94 | 24.30 | 2349 | 2623 | 18799 | 20985 |
| `1795190_444484_69d357e6be05f605186705ed787b0f2e79d5743b.png` | 5.89 | 11.61 | 619 | 704 | 4948 | 5630 |
| `1795190_444485_df487b420690728af97885245e10547e247d7374.png` | 4.70 | 13.72 | 1861 | 1921 | 14895 | 15376 |
| `1795190_444486_2f71db5eb86f9ba789a08b6e9a8e083f38d4c53f.png` | 10.13 | 21.12 | 1513 | 1579 | 12111 | 12639 |
| `1795190_444487_09ed57e7b92b1a983de57b6b276527fd2fc335d4.png` | 4.93 | 13.72 | 1013 | 1301 | 8101 | 10431 |
| `1802720_156412_73922c6b1f43beac2622c8d48486dcf92f8dcd3a.png` | 6.94 | 17.94 | 1026 | 1031 | 8223 | 8251 |
| `1812860_458936_1c35b39afa049ebef5fd44d29fbb2a74872163f9.png` | 17.46 | 19.03 | 109 | 990 | 863 | 7937 |
| `1812860_458937_e33105119c28199053f25f96be38e0909377fdf1.png` | 18.35 | 20.13 | 409 | 1483 | 3247 | 11819 |
| `1812860_458938_af3b63de860ec515519aebacff2e87f7f000de7e.png` | 27.72 | 24.63 | 211 | 2357 | 1693 | 18886 |
| `1812860_458939_8c1c91a5e8a7e01f4cd540922c985a2ea044eaa1.png` | 12.23 | 16.94 | 83 | 3298 | 668 | 26385 |
| `1812860_458940_377046baa342fd6883e52982378b12ed53a97fe0.png` | 2.20 | 15.83 | 5170 | 7784 | 41343 | 62242 |
| `1812860_458941_28a98e9dd5a303dee2b04f590c430f5a0f29b379.png` | 12.08 | 22.21 | 174 | 2739 | 1426 | 21915 |
| `1824220_171247_65b1e071b84e916615371ac446f8199b20f46b1e.png` | 10.33 | 23.22 | 476 | 921 | 3801 | 7364 |
| `1824220_171257_82ea74eee207c4b2653c57671e67b9b10b62803c.png` | 2.49 | 11.59 | 241 | 280 | 1928 | 2237 |
| `1824220_171258_a950a35a07024edf23bd2cc5756078a260912749.png` | 11.59 | 16.90 | 887 | 1430 | 7103 | 11436 |
| `1830910_256337_1d1d6ecb8481108ac7fbe7071a995e0f1b3c3d6a.png` | 41.24 | 16.93 | 918 | 2094 | 7366 | 16755 |
| `1830910_256338_28d5cffdde0a3d2c2066617bf42ef55ad8fdc276.png` | 25.36 | 12.67 | 461 | 1323 | 3654 | 10560 |
| `1845910_390988_c1df42dae6455f578748b6e3b6e0c04673cbbd3b.png` | 0.01 | error | error | error | error | error |
| `1845910_390989_c4b462cd2e30ed66c14ee1a93362b6b0cf4b1f1f.png` | 0.00 | error | error | error | error | error |
| `1845910_390990_1be987bf37c4518258cc12de0ad362d4f32edc10.png` | 0.01 | error | error | error | error | error |
| `1846860_149832_6bd3ec7428aefc3a7fae6a7321c50041912f3730.png` | 80.74 | 86.76 | 993 | 1543 | 7952 | 12357 |
| `1846860_149833_1c5993b5a6fe575c8c5ea04a5bc02be2d5464295.png` | 31.89 | 56.45 | 558 | 2236 | 4473 | 17862 |
| `1846860_149834_3dc53c81aefef949c13958c2bcf8d2d555d055ea.png` | 29.15 | 28.58 | 378 | 1118 | 3026 | 8985 |
| `1846860_149835_836b0720508ee9d8c48661b706bf4da73446bc61.png` | 28.79 | 29.64 | 312 | 379 | 2508 | 3039 |
| `1846860_149836_d64ee2f863181e1d62378c61c8ee709b622f406e.png` | 18.83 | 28.55 | 205 | 1745 | 1629 | 13936 |
| `1846860_149837_1c251483495acf823fa6f043b0d4f8ff5eca3e1c.png` | 22.73 | 29.84 | 335 | 1085 | 2723 | 8714 |
| `1846860_149838_959ac85596736bbcca9ab23652dad3c36f2a2263.png` | 72.89 | 71.00 | 518 | 2330 | 4139 | 18646 |
| `1853410_249244_e12b8cac2f496c505edb800e336025bb766c49ae.png` | 4.44 | 11.61 | 636 | 1071 | 5103 | 8570 |
| `1853410_249245_e80e35ccf5a09b2c65ec74df2cca9d50e4b0f7e7.png` | 6.84 | 11.61 | 1125 | 1866 | 8978 | 14908 |
| `1853410_249246_b98b31da3e91f91e3f7a985c7ec6c48f581ca7f9.png` | 2.62 | 11.60 | 84 | 540 | 660 | 4329 |
| `1861290_202179_44187a89cc9b8780be76a7595a4bb7e81302dbdc.png` | 3.66 | 11.58 | 12 | 112 | 85 | 888 |
| `1861290_202180_3f828c020f0c886944799c5fcde2969d44ecf122.png` | 2.38 | 11.58 | 75 | 133 | 606 | 1068 |
| `1861290_202181_cf02a1e449cf1e52cc4cf5aca2b2ef4944323838.png` | 3.70 | 11.58 | 95 | 172 | 757 | 1372 |
| `1861440_255074_cc91f95ca682f2a02e5110dbb36cc7636507adf5.png` | 7.59 | 11.69 | 38 | 148 | 299 | 1173 |
| `1861440_255076_2c17dd12afb519b2c864dad8aea486647bef42b5.png` | 5.60 | 11.58 | 27 | 255 | 223 | 2042 |
| `1861440_255077_14b689d52d6be8bab5f765ca841b6e6fa2a746e5.png` | 10.34 | 12.67 | 154 | 926 | 1129 | 7305 |
| `1861440_255078_023beef2192ad4ae9922ce813d4f92e8d1f0c08c.png` | 6.09 | 11.60 | 48 | 450 | 290 | 3492 |
| `1861440_255079_b80e77d2e74f53bc9ac8787d691a978b5835a8b0.png` | 9.11 | 11.61 | 86 | 398 | 592 | 3089 |
| `1865670_358700_74b2babc53b7d7b7bc3c7b04163d266fbd2f6370.png` | 1.69 | 11.59 | 7099 | 7099 | 56456 | 56456 |
| `1865670_358701_1ed69990f282754af9b35f7c36c0b231e05520dd.png` | 3.42 | 11.59 | 849 | 849 | 6319 | 6320 |
| `1865670_358702_6d0d0418a0cb85a53514cf1742a2a4010359ed07.png` | 2.01 | 11.58 | 25 | 90 | 205 | 716 |
| `1865670_358706_0b88085b44655af2eb914c73d1bcd095d82612ab.png` | 1.28 | 11.59 | 74 | 123 | 595 | 984 |
| `1865670_358716_e9b7d2bdd6700c3f357729a2c76532aca13d300a.png` | 1.51 | 11.58 | 61 | 99 | 485 | 794 |
| `1869270_377754_b04e22a6dc1d3966c12b21ea7a89a914204e6ad4.png` | 33.80 | 46.81 | 514 | 2779 | 4124 | 22204 |
| `1869270_377756_3399eaea4f2848e2297c738346799baa1944a2a6.png` | 25.02 | 38.21 | 415 | 2078 | 3348 | 16611 |
| `1869270_377759_8ee1e4b57a8163d18b599a444bf7a09b895154b2.png` | 10.46 | 20.08 | 632 | 1116 | 5062 | 8957 |
| `1869270_377760_32edc4e0d419cfb00bf518ff90f1b92bf1220ffe.png` | 10.21 | 13.74 | 276 | 794 | 2247 | 6372 |
| `1875580_2357858_0b2943d1729429a8766cb6e7849a565cdddd35c8.png` | 1.02 | 11.62 | 7 | 5 | 48 | 38 |
| `1875580_2357859_eab4330c294f7f9dfc262039ceaec689468cfff4.png` | 0.35 | 11.57 | 14 | 13 | 161 | 152 |
| `1875580_2357860_85346ebeded829d3c01ee7f66352cd7f11daf394.png` | 1.94 | 11.67 | 41 | 43 | 369 | 343 |
| `1875580_2357861_92f359b5c39313931a96dd3f38476e870c995485.png` | 0.60 | 11.57 | 11 | 9 | 91 | 76 |
| `1875580_2357862_03d03ba9b6b187a5c5564a3f0daa8ade2214e072.png` | 0.67 | 11.58 | 5 | 4 | 32 | 26 |
| `1875580_2357863_f5872f6e3fe7dc0fd394ae18565091af756aa53e.png` | 0.51 | 11.57 | 31 | 29 | 264 | 256 |
| `1875580_2357864_b6a59b399d5134b4d97800fc004a07537599267e.png` | 0.25 | 11.57 | 7 | 6 | 57 | 54 |
| `1875580_2357865_378a29f61b8f7e7a2a59157ea946f6118bdd66bc.png` | 0.61 | 11.57 | 65 | 65 | 632 | 591 |
| `1875580_2357866_ac9079f116c0e878571c3a0bd8a4d2607f5defac.png` | 0.24 | 11.57 | 11 | 11 | 83 | 78 |
| `1875580_2357867_c5a2f36b5a9d448db375d865cca518ce882b1d5f.png` | 1.34 | 11.60 | 45 | 42 | 344 | 319 |
| `1880140_152818_dfc834efeafdf25c8b4d5548f8c574ede2b078b9.png` | 7.51 | 14.80 | 120 | 446 | 975 | 3603 |
| `1880140_152819_84da5fbfc67d4497f6f7b343196267845b4d9d5a.png` | 16.99 | 22.19 | 364 | 923 | 2893 | 7351 |
| `1880140_152820_19289564d0ceb2639d2a4a8b9b8852d4898a3f16.png` | 4.65 | 16.93 | 1276 | 1386 | 10189 | 11062 |
| `1880140_152821_48ea15cd20e4dbf18b9d15d6a5eb75f2ab292cdb.png` | 2.33 | 11.60 | 329 | 373 | 2617 | 2970 |
| `1880140_152822_33071d83518bfbf3fc3c94b918ef6de0bf57a505.png` | 8.30 | 11.61 | 109 | 412 | 861 | 3300 |
| `1880140_152823_5064cae1a4bf57276fb93c813130e2fecfbac0cb.png` | 4.14 | 12.68 | 97 | 322 | 782 | 2596 |
| `1880140_152824_db1447ab0f79e3341026a34cd4837f89db6ff4b7.png` | 3.74 | 16.92 | 367 | 568 | 2929 | 4519 |
| `1888930_314149_340ed35e1c413eee5cd8293abab0c1bc9498c12e.png` | 19.80 | 28.52 | 1735 | 1736 | 13870 | 13877 |
| `1888930_314150_05e57de82965ce6605d6456db76d4e0971f98c42.png` | 0.38 | 11.58 | 9 | 25 | 75 | 203 |
| `1888930_314151_079f43b697e5a403062a62348c8cc165c2e9a42d.png` | 0.36 | 11.58 | 3 | 28 | 22 | 221 |
| `1888930_314152_34da4f16b2894ae0c4f0ffa95ca1a9f1e00000c9.png` | 23.11 | 40.12 | 2198 | 3122 | 17585 | 24968 |
| `1888930_314153_8109b8dcac6088ae78e68dbc71253841486583f5.png` | 33.23 | 31.42 | 1359 | 1598 | 10863 | 12785 |
| `1892420_249998_ac35fe06b1b8344adea29938e59311f56c9b6b4a.png` | 17.38 | 15.85 | 458 | 923 | 3670 | 7422 |
| `1892420_249999_85a2a0ee227be8b419b0def921a899ace23a0118.png` | 1.97 | 11.65 | 17 | 22 | 142 | 181 |
| `1907720_177888_7c05dabc562891cc44a029b48af37b848cf7122d.png` | 0.85 | 11.58 | 272 | 453 | 2165 | 3632 |
| `1907720_177889_ed2ae49b3e407053aa241b4de665b15c67da9ed9.png` | 0.49 | 11.58 | 194 | 282 | 1553 | 2257 |
| `1907720_177890_22c159c555933155722342b0caad7b2bf235beef.png` | 0.58 | 11.58 | 48 | 87 | 389 | 701 |
| `1928090_298209_cd9ea2cc82d4b8569024cef16e6e054d1b5915da.png` | 5.51 | 12.69 | 279 | 1332 | 2256 | 10668 |
| `1928090_298210_60f520531e3c45ef32fd5a609db15e98b2d77d65.png` | 6.55 | 16.92 | 280 | 783 | 2220 | 6242 |
| `1928090_298211_c97a50ed7e742d8b2cb42f75821227d0d2b27f27.png` | 19.87 | 22.27 | 2702 | 3402 | 21610 | 27217 |
| `1928090_298212_bd90ee77c2543e2aa2056c04d90cb9b94827f881.png` | 7.78 | 12.69 | 431 | 1952 | 3491 | 15637 |
| `1928090_298213_ce8dd679fe69e7f9fd5a5cb09835260f341a926b.png` | 5.69 | 14.82 | 176 | 1718 | 1399 | 13767 |
| `1928980_290963_db02b798adb7d4f96d2f899c85eafa4a43bd0a44.png` | 9.48 | 22.18 | 2857 | 2857 | 22851 | 22851 |
| `1928980_290964_254aa9fd5a7143c17f4021bbbd421e645369acf9.png` | 7.72 | 28.52 | 2490 | 2820 | 19928 | 22557 |
| `1928980_290965_1bbefb7ce2f1f03a8d6efd83d959024f78ddfa8f.png` | 8.69 | 13.72 | 875 | 876 | 6006 | 6010 |
| `1928980_290966_7f5c0fe589665ef61471972df0a6209203be70d1.png` | 4.81 | 14.77 | 1510 | 1514 | 12092 | 12133 |
| `1928980_290967_36dcd29d0b0e1fbe76a5275dd09c7890d7464b67.png` | 5.57 | 21.11 | 1827 | 1829 | 14618 | 14634 |
| `1928980_290968_4ad385d725b84d8045478a078102f6a0e3ef6f2e.png` | 5.78 | 21.12 | 2690 | 2735 | 20532 | 20897 |
| `1928980_290969_c476d52bc9139494786406a06e9e7752d97c9360.png` | 8.26 | 13.73 | 1229 | 1229 | 9850 | 9851 |
| `1934800_182708_a6896481303a21c2e215a15d2bc4f82963448f01.png` | 13.09 | 20.06 | 1545 | 1506 | 12334 | 12024 |
| `1942280_449380_13575a4b25c4e1789896baa7096a35a8d8d2e3e5.png` | 24.57 | 24.03 | 264 | 149 | 2136 | 1167 |
| `1942280_449381_bb0479e9c664a9e3f9188c29385be8f462c842ab.png` | 15.31 | 11.65 | 94 | 678 | 720 | 5420 |
| `1942280_449382_020570f0af222d4d48afaa4b7e7defff6c106f48.png` | 11.25 | 12.02 | 91 | 31 | 722 | 244 |
| `1953540_250379_642cea92c20dadaed54495cac9e760d43ad0104f.png` | 64.32 | 72.00 | 204 | 1404 | 1627 | 11184 |
| `1953540_250380_981c40bebc616e25ff897cf6bfbbbc4bfb1f539a.png` | 70.05 | 56.18 | 246 | 783 | 1985 | 6253 |
| `1953540_250381_0e347058d095c3e7638b4993af8f0325d00905bd.png` | 59.05 | 63.48 | 370 | 643 | 2983 | 5156 |
| `1955830_289928_a0e0ced389ddd27a3c65f61d70a18512ec43167e.png` | 5.65 | 15.95 | 131 | 2658 | 1048 | 21244 |
| `1955830_289929_bd7dc1b2e00644a9b2702991e0c4eed8505d355d.png` | 4.54 | 19.08 | 243 | 2013 | 1973 | 16152 |
| `1955830_289930_85f263b79f53ee7f84c823ea5db12e920a76f883.png` | 3.38 | 11.70 | 59 | 2267 | 464 | 18146 |
| `1955830_289931_1c748c3973398648bb0ed271d05b541227a580ec.png` | 1.49 | 11.61 | 14 | 463 | 131 | 3723 |
| `1955830_289932_1e491c2385364ea41838b50a82549fbf61103ebc.png` | 10.72 | 20.09 | 1639 | 2171 | 13104 | 17367 |
| `1955830_289933_2947b1fad826254a747608bf2d7f42188567680d.png` | 1.99 | 11.64 | 31 | 2604 | 252 | 20859 |
| `1967430_218634_3adbeb6ea1f2fe6f527303df09bf2775469bb800.png` | 0.01 | error | error | error | error | error |
| `1967430_218635_b911402f4c226a1134b59e8d01e4c22781f53cdc.png` | 0.01 | error | error | error | error | error |
| `1967430_218636_fb2f250abba7b19f56a4404b34cbec6ae4df652c.png` | 0.01 | error | error | error | error | error |
| `1967430_218637_85f779dfa4a00a034abdae94c4beddc02397dd5a.png` | 0.01 | error | error | error | error | error |
| `1967430_218638_7fb99dad407784fe1acc5219cbcd43e2c6420615.png` | 0.00 | error | error | error | error | error |
| `1972820_429525_82953419c4cd71b12b55f618489193a6cd6dd1e5.png` | 1.15 | 11.58 | 242 | 315 | 1940 | 2529 |
| `1985960_333493_9267fcb189e583a8dac5665ecd80c419ae2df6ec.png` | 3.10 | 11.59 | 93 | 342 | 735 | 2706 |
| `1985960_333494_428c5f2c1cad51cbcd27f7a941c3f980db933f23.png` | 4.04 | 11.60 | 125 | 380 | 981 | 3050 |
| `1985960_333495_bac48bf75ac4e3a0968687d5e3ee3036af90944c.png` | 6.37 | 11.59 | 107 | 237 | 856 | 1886 |
| `1985960_333496_e52282ce1e43b924f349ee4bfe9774a95ea68f98.png` | 5.08 | 11.58 | 59 | 364 | 477 | 2883 |
| `1985960_333497_3f3d6052d00055c42ac250d6ce2a660fb50054fb.png` | 2.89 | 11.58 | 128 | 251 | 1012 | 1993 |
| `1986840_178935_12ae7f3bec73788922a7bc39cb3091bea3090f13.png` | 2.66 | 11.59 | 1380 | 1567 | 11072 | 12556 |
| `1989120_1758297_c0c2c588c1ef71d423783c2d95fd621ac2dec704.png` | 50.00 | 21.16 | 194 | 1698 | 1576 | 13603 |
| `1989120_1758298_8b3903e43ac743d49a6c476302468d389239490c.png` | 77.57 | 38.07 | 229 | 2448 | 1849 | 19605 |
| `1989120_1758299_2eefc1b93312449fca094613d4986f0f94a4ae06.png` | 76.04 | 40.20 | 279 | 2693 | 2182 | 21493 |
| `1993180_276407_46d9e723b597c71d33f6d70375098d3e8c79221f.png` | 4.89 | 12.67 | 734 | 1353 | 5877 | 10845 |
| `1993180_276408_6146014d82522d471e0ebb0ba6d2b5b0ba05e326.png` | 11.57 | 11.59 | 2106 | 2132 | 16844 | 17048 |
| `1993180_276409_3c94b8bf5a09234f593c9953caa45c91d30c03fc.png` | 1.29 | 11.58 | 18 | 122 | 139 | 982 |
| `1993180_276410_c05cf9b30dab35d775611124c916326d89ba8456.png` | 4.86 | 13.77 | 353 | 728 | 2801 | 5795 |
| `1993180_276411_6a0843f8da51a6c00ea57a35ca41bf3dc7bfbe21.png` | 2.97 | 11.59 | 1475 | 2335 | 11796 | 18672 |
| `1995880_244065_a9aebc529deab9016ba850f602aa3a0dd94758e0.png` | 11.42 | 38.01 | 7494 | 7607 | 59944 | 60855 |
| `1995880_244066_60a241a4b0a2e79765c23f049951a56eeafd33cf.png` | 11.82 | 39.06 | 7020 | 7308 | 56171 | 58475 |
| `1999520_337154_b9df3b032177b717ce46b988e88f9a4e363ada79.png` | 1.12 | 11.60 | 4 | 2 | 37 | 26 |
| `1999520_337156_32c2acde83968cd6d6bfa1ee9b9f11cec89f82c2.png` | 3.27 | 11.58 | 307 | 306 | 911 | 912 |
| `1999520_337157_9ff87136a7b7e112165aa5ffd3eaf28415a14707.png` | 1.49 | 11.57 | 161 | 166 | 680 | 714 |
| `1999520_337158_e4c54918352361769f0daaecdd0c57b54693272e.png` | 2.92 | 11.58 | 1137 | 1243 | 5357 | 6224 |
| `1999520_337159_8671cf9b63b9eee7cf190ebea6fc19a767bb4906.png` | 2.76 | 11.58 | 535 | 691 | 1311 | 2522 |
| `2004640_364131_c6a4aafd0036e7171c7b81813dff964cea143d21.png` | 2.53 | 11.64 | 84 | 1897 | 622 | 15156 |
| `2004640_364132_fc0d83ed5ebb100507aacfc7cbde02b0397a2e21.png` | 16.79 | 19.36 | 292 | 5183 | 2373 | 41467 |
| `2012670_315315_58231386918fe374c00950d80824a22bff0b6665.png` | 0.88 | 11.60 | 4 | 6 | 36 | 50 |
| `2012670_315316_790375d89f5f34b3320abf390e9299730dda4fa1.png` | 0.94 | 11.62 | 3 | 5 | 29 | 42 |
| `2012670_315317_70f0d79ffee07ac9e0ee5305ee2940a2f402ad05.png` | 0.97 | 11.62 | 4 | 5 | 40 | 48 |
| `2012670_315334_2c0cfeb12558a9f1c2caddb354713aa1eab0c0d8.png` | 1.46 | 11.63 | 6 | 8 | 54 | 62 |
| `2012670_315335_74075853adc8fdcafb1fb2fcbdb5aa1326925aeb.png` | 1.30 | 11.66 | 5 | 7 | 46 | 51 |
| `2012670_315339_f9b62559f7a0bc1202129075cbe25dcfad8ae4b8.png` | 0.97 | 11.62 | 7 | 7 | 50 | 53 |
| `2012670_315340_465c240ffd2a44458c2b91573ffccb1b02b72de5.png` | 1.41 | 11.65 | 6 | 8 | 59 | 71 |
| `2012670_315341_f789f6b5e320ee544f134d7b8c1b739bd5048927.png` | 1.08 | 11.62 | 8 | 12 | 62 | 103 |
| `2012670_315342_25afa8ea89e7fe9808b1415537c0befc6fa84a3f.png` | 1.07 | 11.62 | 4 | 3 | 21 | 17 |
| `2012670_315343_072060be2ff51be97e23fababe7c813cb4b29993.png` | 1.04 | 11.60 | 5 | 7 | 41 | 53 |
| `2012670_315344_32abe4bd09aa0ee18bf3d5d9f331ad28e887f666.png` | 1.12 | 11.62 | 4 | 5 | 41 | 50 |
| `2012670_315345_424cf677562ceda94f96f6209491c9c57a9856c0.png` | 0.96 | 11.61 | 3 | 4 | 34 | 46 |
| `2012670_315346_1594178fe9b2ca365bf211b80b2ead720e2605d2.png` | 1.14 | 11.61 | 6 | 9 | 45 | 70 |
| `2012670_315347_10e51805112a90b09c4c6b60b64124a8fe7503a2.png` | 1.21 | 11.62 | 7 | 13 | 56 | 96 |
| `2012670_315348_74453ed401603410ddd2be585dec9099142776f9.png` | 1.17 | 11.62 | 7 | 13 | 56 | 96 |
| `2012670_315349_b43cd2b6a2e97157067b41b7bda4bd32d2f9a175.png` | 0.93 | 11.62 | 7 | 7 | 58 | 60 |
| `2012670_315350_9f678b629b69ef6b4d338157e605a84b2540c584.png` | 0.75 | 11.59 | 5 | 5 | 45 | 42 |
| `2012670_315351_bf00761a5e6ef594f1c08bfec6bb20494fed0b1d.png` | 0.75 | 11.59 | 5 | 5 | 45 | 42 |
| `2012670_315352_2349ead5dcf98852376f22bacde13ac5132e77bf.png` | 1.21 | 11.61 | 6 | 9 | 45 | 60 |
| `2012670_315353_28304812da9819e09a8c206a5dda1541a5d47264.png` | 1.02 | 11.64 | 7 | 15 | 52 | 107 |
| `2012670_315354_366d2838b70274ac67d6efdeabac56fb0483694b.png` | 1.06 | 11.63 | 6 | 12 | 41 | 81 |
| `2012670_315355_9e4cdbba96651e5374f54dab8c5d46a13b94b31a.png` | 0.94 | 11.61 | 4 | 3 | 31 | 24 |
| `2012670_315356_6fc82b36a0841df4706c1d7e23f07f2de73da397.png` | 1.41 | 11.62 | 5 | 5 | 35 | 30 |
| `2012670_315357_5167c6063da1620479c302073bd320f05a659315.png` | 1.35 | 11.62 | 5 | 5 | 40 | 35 |
| `2012670_315358_cfcb3e7bbcbbf46fceb897aeebdc20133f28267b.png` | 0.99 | 11.61 | 6 | 7 | 55 | 57 |
| `2012670_315359_ec9b2558efc9db5b29951b659962111e8ceeb094.png` | 1.21 | 11.63 | 7 | 8 | 51 | 60 |
| `2012670_315360_9c49d6afe4c64064656c92799d2c03575563d815.png` | 0.84 | 11.61 | 5 | 4 | 48 | 38 |
| `2012670_315361_5e376c7cafc58b9cb2885cbc3ec674340c3c4e09.png` | 1.07 | 11.57 | 5 | 6 | 45 | 42 |
| `2012670_315362_7b9148574b1aa3c73844839698bf7da2ac455d40.png` | 1.37 | 11.60 | 8 | 10 | 71 | 80 |
| `2012670_315363_663e009dd6241040a526c86fee57dabae1fac486.png` | 1.31 | 11.57 | 11 | 8 | 76 | 64 |
| `2012670_315364_27b878516eb0ce926986c3b1e28329f215108583.png` | 1.15 | 11.61 | 7 | 9 | 57 | 77 |
| `2012670_315365_886c849c871f4f1da86194bfeb42c79c72b0c430.png` | 1.14 | 11.62 | 6 | 9 | 48 | 67 |
| `2012670_315366_29c9119e2fcee0e37a8fc8c866bd4935de5cdd50.png` | 1.15 | 11.62 | 6 | 9 | 48 | 67 |
| `2012670_315367_5053afd411a38695a24350f1c6120da3e1add3e1.png` | 1.43 | 11.63 | 9 | 9 | 65 | 68 |
| `2015270_454493_93cb921fea3946d6369c93130d1b7d4300d2af9f.png` | 17.92 | 11.63 | 116 | 606 | 894 | 4871 |
| `2015270_454494_21a41f4e4b049eea331d2998ee24bf9224fe8e5a.png` | 3.35 | 11.59 | 196 | 524 | 1583 | 4203 |
| `2015270_454495_5e04991d796f396ed0836b3fab8d4234ec88224a.png` | 60.87 | 63.43 | 650 | 2547 | 5163 | 20330 |
| `2015270_454496_ed70ed9974c7e30c3a877ba572a0b4522d78f823.png` | 23.12 | 12.66 | 153 | 721 | 1223 | 5789 |
| `2015270_454497_237f073a1e4f037668ceaa91c164c9f0ba028bad.png` | 10.69 | 11.62 | 567 | 595 | 4523 | 4759 |
| `2015270_454498_86d96ae588c55c8271c5a6e6b142c6d1c71c9361.png` | 26.10 | 22.22 | 1360 | 1553 | 10865 | 12429 |
| `2021850_170467_987a8c1cb9aa4c1b85ab060dd4221ce8c7941f74.png` | 34.44 | 16.07 | 109 | 214 | 859 | 1715 |
| `2021850_170468_7f111e756e336bf8a0c09bd87598794b2ad4f7ea.png` | 4.12 | 12.65 | 73 | 286 | 611 | 2227 |
| `2021850_170469_c0e48d7499b1bf86e2ca59f74c65916418857325.png` | 18.98 | 20.61 | 219 | 314 | 1718 | 2514 |
| `2021850_170478_3d5ca8f4b2cf1d35dd27953c9298a9067a32ac98.png` | 14.93 | 20.10 | 99 | 361 | 764 | 2851 |
| `2022180_198703_9130afd0e19c22d71d10fa8d188ee9b8f8f4faa4.png` | 75.27 | 35.01 | 845 | 4192 | 6782 | 33559 |
| `2022180_198704_cec4fa9eded24bbdff6c27316ddafd11259f5a2e.png` | 32.43 | 19.05 | 355 | 1288 | 2847 | 10336 |
| `2022180_198705_8bc927e1969c238ebaf2f177990e4f5457d17a16.png` | 22.13 | 13.77 | 635 | 604 | 5083 | 4826 |
| `2022180_198706_12c61de3e5a1edfb535a5e8c05c4e338091a1e07.png` | 34.07 | 16.95 | 664 | 966 | 5288 | 7677 |
| `2022180_198707_e35b05cba52c6abe7756b5a3c0e6116980c872ea.png` | 29.00 | 23.31 | 396 | 1474 | 3175 | 11780 |
| `2022180_198720_6d263f1eaded51f00bbcf0c86a7524206bb8e438.png` | 5.29 | 11.60 | 318 | 400 | 2527 | 3195 |
| `2052990_311283_8e2f7b5a58d383437297d7b4a4bff335816bfbc7.png` | 0.01 | error | error | error | error | error |
| `2052990_311284_8f217dd8437316a552c54b809fc9aeedd4b653cf.png` | 0.01 | error | error | error | error | error |
| `2052990_311285_aa4261e2ffebc47c02973f0749dae42ff739eb4e.png` | 6.42 | 17.93 | 825 | 924 | 6461 | 7264 |
| `2052990_311286_05ec69ad05e06842d0042beda65949667c15ba03.png` | 0.01 | error | error | error | error | error |
| `2052990_311287_f2b0cdb6dac33b8aefd2bc3eba59da8e47a386f3.png` | 0.01 | error | error | error | error | error |
| `2054690_421736_4d19318b5c06e02ce5ff4f7207484732f856da94.png` | 6.53 | 15.82 | 2293 | 2954 | 18054 | 23322 |
| `2054690_421737_0cd509e8c0807be6704c3aa1ff5a9bf8fde1c128.png` | 10.94 | 11.61 | 314 | 523 | 2334 | 4015 |
| `2055050_299577_9bbbdd500b5b624599c5794afde67befa037d009.png` | 6.32 | 13.70 | 2461 | 2498 | 19508 | 19797 |
| `2055050_299578_02710da403dd81c6bc0b6f85186df9057ba624e3.png` | 2.79 | 17.93 | 4671 | 4745 | 37173 | 37731 |
| `2055050_299579_3a8edaff937d6d98c4f97b6965e914f4977fbf91.png` | 2.30 | 11.59 | 1551 | 1733 | 12207 | 13668 |
| `2055050_299580_4ca22ad84ecf2e26634bd8786149f6d72b868571.png` | 6.22 | 20.05 | 5252 | 5522 | 41832 | 43963 |
| `2055050_299581_01f4dbbb935fc035650a8491eb9841de4fae62a1.png` | 18.20 | 17.94 | 2857 | 3103 | 22689 | 24637 |
| `2055500_197283_33d2097dc72924dd032d965b1e97ed85e0bf8083.png` | 1.02 | 11.60 | 2 | 2 | 22 | 19 |
| `2055500_197284_d7d244470dd058973bd1520255f62bf27aa95ea0.png` | 2.18 | 11.65 | 7 | 6 | 62 | 58 |
| `2055500_197285_177476365a62ada17f3e7c7d086f7df20bfc3fff.png` | 1.42 | 11.61 | 8 | 6 | 66 | 54 |
| `2055500_197286_caf787c0980eb9ef97bc89ba5468514390919ba4.png` | 3.01 | 11.70 | 20 | 18 | 156 | 120 |
| `2055500_197287_044c34e97abd643faf69ab6641faa3bc2d8ace98.png` | 3.45 | 11.58 | 345 | 353 | 2770 | 2848 |
| `2058030_422392_6138000e69fb106154bec2fb2238e7841e55abcf.png` | 13.59 | 13.71 | 1099 | 1218 | 8818 | 9760 |
| `2058030_422401_b8a7779efc686a7655755ceb139f03b335cef01f.png` | 7.21 | 14.77 | 1177 | 1841 | 9432 | 14729 |
| `2058030_422402_364ecd4925470d8b589cd7559d9e55f45b0b5017.png` | 10.50 | 22.17 | 107 | 885 | 847 | 7096 |
| `2064610_432358_51cf22438d1438317f099933c530d593390d17cc.png` | 1.23 | 11.57 | 7 | 7 | 68 | 60 |
| `2064610_432359_6f17a8f9486cd4a92571f530787f383b414cc8c3.png` | 2.05 | 11.66 | 13 | 12 | 89 | 95 |
| `2064610_432360_dd3495081e68522373e5b6642fae4ca44260dc70.png` | 1.30 | 11.61 | 10 | 8 | 66 | 71 |
| `2064610_432361_d46f8d04418c14d9f7e71f27252f2318aff27107.png` | 2.07 | 11.65 | 10 | 19 | 69 | 143 |
| `2064610_432362_4624c760965cf6cf70d4e8da68c743f0e09dd5b4.png` | 0.76 | 11.57 | 4 | 6 | 31 | 52 |
| `2064610_432363_646dd9df915010cfa2cbb6bca68434bbeb100864.png` | 10.42 | 11.86 | 54 | 65 | 432 | 553 |
| `2064610_432364_6ba03aee9c745ac0e4f6fe78849f807afdd819f9.png` | 1.20 | 11.63 | 6 | 11 | 41 | 81 |
| `2064610_432365_19e77799d2b6e4ff0c753729077521df132c1c8d.png` | 2.35 | 11.66 | 28 | 29 | 226 | 229 |
| `2064610_432366_133361ce90710c035593e281f1ea84770736c7da.png` | 1.62 | 11.64 | 11 | 11 | 89 | 80 |
| `2064610_432367_8a0d31f6b9085306dc1d17ef680dbf91f3a27c8a.png` | 4.20 | 11.75 | 74 | 75 | 506 | 570 |
| `2073620_412050_12d3b17be91561f271fc2afe4c97b99493c82299.png` | 2.38 | 13.75 | 175 | 1764 | 1398 | 14089 |
| `2073620_412065_8a0d6777979c5d7a9b8acdc93056c20be8a379a6.png` | 1.93 | 11.61 | 23 | 924 | 176 | 7392 |
| `2073620_412066_c4aa2c2dc2588f2d58824d7b588a677937d64584.png` | 0.01 | error | error | error | error | error |
| `2073620_412067_0b41392f5ba12cc7864b97308becc6c06402b5b6.png` | 0.01 | error | error | error | error | error |
| `2073620_412068_05dc911b6c00b5505ef8de46c135dfdebad8e76e.png` | 0.01 | error | error | error | error | error |
| `2073620_412069_761e9b743fa1ad7a420fd88f45fb2805fa5bcf4d.png` | 3.97 | 11.63 | 695 | 1568 | 5513 | 12516 |
| `2092840_815987_85783523d5c59fe0a31462f1b288e6feb6425c03.png` | 6.09 | 11.60 | 359 | 820 | 2869 | 6555 |
| `2092840_815988_a442bbb1d55577dfaa327b31a1b2395e74e12ede.png` | 8.83 | 11.60 | 263 | 750 | 2108 | 6009 |
| `2092840_815989_45ea6cf6fbf15bf512f19c5f32910c9ed10717c0.png` | 14.42 | 17.95 | 837 | 1492 | 6715 | 11943 |
| `2092840_815990_b69282810c7554c9ca1a2ac84bc2ba82c1f0c729.png` | 10.26 | 15.84 | 1197 | 1647 | 9564 | 13174 |
| `2092840_815991_ff63a7b58ecf1b246d8d037f30221aff49624caf.png` | 7.30 | 13.72 | 304 | 1106 | 2430 | 8865 |
| `2111190_295894_6084c022ff88b1632a891a7c043c8610f71486d4.png` | 6.41 | 15.82 | 579 | 625 | 4151 | 4523 |
| `2111190_295895_e0c36dc71dd1f93da69562949ece3ada45b1cfd0.png` | 3.50 | 11.60 | 245 | 437 | 1968 | 3501 |
| `2111190_295896_5182ab7fba1c386a70cb34308845135459e63016.png` | 2.41 | 11.59 | 999 | 1314 | 8003 | 10523 |
| `2111190_295897_bdd8ab3e0cbda42ddd2d7c053f227a86d6d6afa9.png` | 8.40 | 27.45 | 2096 | 2096 | 16028 | 16028 |
| `2111550_4066026_dbdec17eb83491ae6f1449af40dcf4b078654d12.png` | 21.90 | 24.40 | 264 | 570 | 2118 | 4599 |
| `2111550_4066027_cb1d129c539c65b79d55a4896a1ce7679632fefc.png` | 35.55 | 40.21 | 478 | 1712 | 3839 | 13729 |
| `2111550_4066028_fd0134f24db2bb8b5b7a762b43d2530087580e11.png` | 38.97 | 39.42 | 377 | 1267 | 3017 | 10128 |
| `2111550_4066029_defeca7cfc2c0c6e42717b5d4aebe6b0baf4ab11.png` | 34.48 | 38.10 | 512 | 1501 | 4121 | 12044 |
| `2111550_4066030_4bb82b2c1f0e2456013c953d7a1b37f5f1cff20a.png` | 37.03 | 38.06 | 604 | 1433 | 4831 | 11492 |
| `2111550_4066031_7794b311a220e444a3438f0d0c6ed42d557b9baa.png` | 49.53 | 27.46 | 568 | 1658 | 4555 | 13242 |
| `2120070_3097582_eab51b8afafa8a996ac621eccef19ef52aef88ca.png` | 4.11 | 11.74 | 30 | 82 | 256 | 663 |
| `2120070_3097583_e77536958629ac34fde682b17b663b18a18d54fb.png` | 5.31 | 11.58 | 26 | 104 | 176 | 861 |
| `2120070_3097585_a41dc4b4eb3f485a2a94a3f0b4b5bc61c7299a39.png` | 6.01 | 11.78 | 45 | 140 | 378 | 1142 |
| `2120070_3097586_6958a75859fa72267294097a57eea40e0b8d9452.png` | 2.86 | 11.71 | 26 | 42 | 241 | 364 |
| `2120070_3097587_9d073879abfe641e6c92153ef1b30744a11d50e8.png` | 3.05 | 11.70 | 28 | 51 | 225 | 404 |
| `2120070_3097588_b95e91e69a6016ae97a455bb1227e705d4d2d811.png` | 2.60 | 11.69 | 18 | 103 | 119 | 778 |
| `212200_120439_168eef43ced42c0b626d95992823312386463552.png` | 5.16 | 11.60 | 1069 | 1072 | 8558 | 8586 |
| `212200_120440_b701d779568d9794af46fd51e419bb4e00d0b1be.png` | 7.28 | 11.59 | 994 | 1751 | 7956 | 14009 |
| `212200_120441_ffb8f91264ed3680721ef26cb08ca56a5edd571b.png` | 0.87 | 11.58 | 38 | 152 | 299 | 1199 |
| `212200_120442_772cc3a781d28e53385fdb238871e08e4e738fa2.png` | 0.78 | 11.58 | 120 | 186 | 969 | 1492 |
| `2129810_379948_e922910617e8dd0c8ed83bd3739d66ed44f8d115.png` | 0.64 | 11.58 | 18 | 815 | 40 | 6431 |
| `2129810_379949_a3384419941d68fba873aa8b4280b99ad1ca24e4.png` | 0.60 | 11.59 | 148 | 1369 | 1102 | 10872 |
| `2129810_379950_1d6cbcb06cdc5cbe2260264c97ed96ebb243d9e7.png` | 0.85 | 11.59 | 37 | 1042 | 209 | 8249 |
| `2129810_379951_4af57dfbb5f9abd4400ae60b3a0e942028ccb2f6.png` | 0.95 | 11.59 | 30 | 1139 | 135 | 9009 |
| `2129810_379952_119ec341eadd5b13e5937af02d0e542fcbb9a127.png` | 0.80 | 11.59 | 57 | 2206 | 380 | 17561 |
| `2129810_379953_435baf03bb5a32b9f65fe011d97d4fe0a877f21d.png` | 0.45 | 11.59 | 48 | 1286 | 294 | 10179 |
| `2147380_556783_b414c0ac74c4f2c43ca4df9c506a5bec3d43f661.png` | 6.24 | 13.72 | 641 | 642 | 5119 | 5135 |
| `2157060_403707_be9b780238e6f8a8675b2a1420a873d70fc961db.png` | 1.46 | 11.61 | 14 | 14 | 106 | 105 |
| `2157060_403709_7fac1fabf83964f3904a587e3c9d3a457897d211.png` | 0.55 | 11.57 | 51 | 53 | 397 | 415 |
| `2157060_403710_d987b503edd0c26d2bc3bf38d6f34c680cae31a9.png` | 0.41 | 11.57 | 14 | 14 | 110 | 107 |
| `2157830_457058_8b669a30005685e7a7ce809c6404bac07167d381.png` | 10.52 | 21.53 | 2114 | 2127 | 16859 | 16977 |
| `2157830_457059_c93493d99925459fa8a649ea6f5065c59ad35a8b.png` | 15.92 | 21.17 | 2445 | 3225 | 19566 | 25801 |
| `2157830_457060_9b19d2c79ab4f4bdfc2828ecc3d3ff021f8412ac.png` | 4.02 | 21.14 | 2853 | 3544 | 22816 | 28344 |
| `2157830_457061_82619971f048ae17438760ac88982405666326be.png` | 8.49 | 13.76 | 62 | 1237 | 498 | 9906 |
| `2157830_457062_07d74c751d0bb3c165f1af306afc5c0fefdd03ba.png` | 12.35 | 11.64 | 98 | 2102 | 762 | 16815 |
| `2157830_457063_26db4a13918de1e2bf474f71b154466f995664c7.png` | 12.36 | 22.18 | 3067 | 3059 | 24556 | 24460 |
| `2157830_457064_d95b788443622bc409543e55ee9d92c6b0d6f5d9.png` | 1.70 | 11.61 | 38 | 971 | 312 | 7765 |
| `2157830_457065_fa55d480b0868164e0558d756100648e03e5bc16.png` | 11.11 | 21.14 | 4576 | 4541 | 36568 | 36344 |
| `2157830_457066_a37a321daba0e14c681ca7d30544af7a0a6be76b.png` | 6.92 | 20.07 | 4163 | 4162 | 33287 | 33285 |
| `2157830_457067_3cb9a20df1891b10f5b0d16f0a10aa9e3fa49406.png` | 4.33 | 19.03 | 2663 | 2808 | 21327 | 22488 |
| `216150_117304_ae1a8acda55db65a8ea292e4f1863b5cfb30fe2c.png` | 0.78 | 11.58 | 35 | 321 | 275 | 2543 |
| `216150_117305_30de64ec261564e4dc8ff767e10391bc4cc09d9f.png` | 3.23 | 11.59 | 111 | 301 | 883 | 2401 |
| `216150_117306_1e0034ff7c508de992b157be97935ff4318b58d4.png` | 1.26 | 11.58 | 146 | 201 | 1170 | 1602 |
| `216150_117307_bc69d13ecbdc7e72e5e9281c948c435b9a271379.png` | 4.91 | 11.59 | 89 | 242 | 713 | 1930 |
| `216150_117308_09c6aada96e3c04df657002788062034941b7d70.png` | 8.41 | 11.60 | 92 | 347 | 732 | 2776 |
| `2173760_349519_de8c867ac02cb886980b0fb632c722ece43cd1e5.png` | 2.66 | 11.64 | 17 | 38 | 146 | 313 |
| `2173760_349520_83f463bd008631afd43d977970d0ab5a8f427d1a.png` | 9.63 | 15.84 | 2159 | 2850 | 17277 | 22789 |
| `2173760_349521_3e1ba7ae924758c1dfa821b2180724a8d7785cb0.png` | 7.36 | 13.73 | 1572 | 2708 | 12567 | 21656 |
| `2179850_252738_7aea39b7ced235a20c4e7e24ac7e8e90d88bb0c8.png` | 1.42 | 11.63 | 7 | 16 | 54 | 127 |
| `2181610_258533_4087cd4d10001b37bc80aaf3c8d483ec10660f11.png` | 14.63 | 13.75 | 204 | 1504 | 1647 | 12061 |
| `2181610_258534_fb74e9c4c8aaf11b9bc707d96eac731279ecf4f9.png` | 17.30 | 12.68 | 188 | 945 | 1534 | 7575 |
| `2181610_258535_adfcd18210d99e8a20764bae4844e9baaeaa0756.png` | 12.22 | 12.67 | 97 | 960 | 775 | 7689 |
| `2181610_258536_28322c92f5797e0619e6536ebf9734cabe10f14f.png` | 13.08 | 14.79 | 130 | 777 | 1014 | 6192 |
| `2181610_258537_f085d297f0ea930e235daf3f0776a1793f42792f.png` | 20.43 | 21.16 | 257 | 1743 | 2060 | 13955 |
| `2181610_258538_b8a15ce0ad80aef3a64a3fb3c7dad391e9a1bb6b.png` | 19.39 | 17.98 | 277 | 1822 | 2250 | 14592 |
| `2181930_215578_25e3a9ea5e24623dd316d8bb0aa12a568712cc2d.png` | 3.77 | 11.59 | 700 | 710 | 5596 | 5676 |
| `2181930_215579_4f7b81b5a9773ee6bf091dba6b197b7dab4eb262.png` | 0.90 | 11.58 | 293 | 354 | 2329 | 2828 |
| `2181930_215580_9c7072806cf3dc51861eadd06b30e81114a8ab99.png` | 52.33 | 19.05 | 345 | 674 | 2742 | 5370 |
| `2181930_215581_e5999f93c394b87228895106fc1149456fe2a2a7.png` | 3.85 | 11.59 | 633 | 670 | 5059 | 5362 |
| `2181930_215582_377a90a2bf3a4b0314cbd801fb4be9f27fd8a03a.png` | 2.88 | 14.76 | 1355 | 1516 | 10828 | 12125 |
| `2181930_215583_38106789b128ab05b3027e9aec8d3f6263dda914.png` | 4.61 | 19.00 | 1534 | 1583 | 12291 | 12683 |
| `2181930_215584_2b9fed6142b3ccc34b7c412a80f6dcdcdca8cd51.png` | 2.00 | 15.82 | 1531 | 1611 | 12268 | 12904 |
| `2181930_215585_554c93e282ca5392dbb88306c20a9dd93ca4b479.png` | 4.73 | 12.66 | 520 | 750 | 4157 | 6000 |
| `2181930_215586_cafcad11ff0f6681f73043f4ff88a3a5519b99f7.png` | 4.01 | 21.11 | 1221 | 1431 | 9749 | 11429 |
| `2181930_215587_07c0ee8a41cb5709aa93f0a1442037548ebd7155.png` | 3.01 | 11.60 | 1684 | 1927 | 13490 | 15428 |
| `2181930_215588_ecd99102e12144a7788544f3b0306c9defe6d6b3.png` | 4.33 | 14.77 | 603 | 763 | 4838 | 6124 |
| `2181930_215589_da7a7b8aeeb3bbaa6c7b22218d701be155b31d5c.png` | 0.64 | 11.58 | 147 | 185 | 1177 | 1484 |
| `2186680_313584_045a7afff332aeaf6fb30f99d346308102a12ffd.png` | 21.82 | 28.79 | 639 | 4202 | 5120 | 33605 |
| `2186680_313587_797bbfbf4bee27dad681828b6601534d054f0ace.png` | 6.69 | 11.64 | 130 | 2959 | 1004 | 23660 |
| `2186680_313588_7a75709d56b60dda0e811a4c65234044d2f966c0.png` | 28.26 | 21.19 | 419 | 2477 | 3343 | 19804 |
| `2186680_313589_7a95cb1a604cc5cf26c0fcd37cb852936830d444.png` | 30.25 | 24.39 | 352 | 1472 | 2843 | 11788 |
| `2186680_313590_bc4a39c808d34cded2aa827369c9b9c72ad91235.png` | 24.39 | 16.94 | 155 | 994 | 1247 | 7939 |
| `2186680_313591_cee188de6eea4be5632756962894ea1f53d351cb.png` | 3.15 | 12.75 | 129 | 3924 | 1055 | 31395 |
| `2191080_257006_04457817e0830a6088deb0e8aee8e8899b58805a.png` | 0.01 | error | error | error | error | error |
| `2191080_257028_0ebf90a345fb1f0347950dabf2a8a08f7ba3adc3.png` | 8.80 | 15.84 | 1027 | 1330 | 8207 | 10640 |
| `2192270_246884_9ca4bfaea5d893bc9d95df0fafcb06575068c278.png` | 0.01 | error | error | error | error | error |
| `2192270_246885_e7c6bd657da2e55aef1c70100ba419fa2fe1652d.png` | 0.01 | error | error | error | error | error |
| `2192270_246886_31c5a843eeb049c77b0fa79008951a7fd88211fc.png` | 0.00 | error | error | error | error | error |
| `2192270_246887_ea5070cd01449ba4276fed9f0d7ca0b6864dd2a8.png` | 0.00 | error | error | error | error | error |
| `2194530_338047_0874ffa50e09493c23ee13ede539d86b7cdbcd90.png` | 7.10 | 11.60 | 860 | 872 | 3909 | 3998 |
| `2194530_338048_41d2095eeb050aa085c50671acd5015866396843.png` | 5.14 | 16.88 | 2009 | 2009 | 12617 | 12617 |
| `2194530_338049_37c45a0b6b537465239847773c0291c78c3c596d.png` | 4.70 | 16.89 | 2502 | 2503 | 16733 | 16749 |
| `2194530_338050_ab1cf46d4d6ba0dcf11eb50d1e1ad6aff7931abc.png` | 3.78 | 11.59 | 1209 | 1214 | 7670 | 7714 |
| `2194790_427933_c3ec3946e61bb1f38c79cbfa295b125926bbda24.png` | 1.21 | 11.58 | 168 | 190 | 1344 | 1521 |
| `2196160_190994_078e4e91ad9ce4b5d0a5f5ad6e0dd4ffd4dd9469.png` | 28.46 | 97.25 | 6673 | 9866 | 53387 | 78940 |
| `2196160_190995_738c6dd3b81d6f47a438a64c87e4b9e6d68013cf.png` | 12.16 | 28.53 | 1906 | 1994 | 15273 | 15987 |
| `2196160_190996_72a563d49a1fb318729316a545364484cdd1d95f.png` | 5.93 | 19.03 | 1384 | 2605 | 11051 | 20823 |
| `2196160_190997_055b315edc5b3fe61679cae3d57f5fdb4eadb830.png` | 24.82 | 52.88 | 4864 | 6670 | 38923 | 53372 |
| `2196160_190998_3d6e559f39adbaeb23a16db0a722ebc609895f2f.png` | 30.04 | 81.41 | 4287 | 7609 | 34264 | 60872 |
| `2197740_434124_202bdb739e444e2e7c0f732d1c2353aec5cab3b1.png` | 24.87 | 57.11 | 310 | 1366 | 2506 | 10934 |
| `2197740_434125_d5897ef9ff8dadbd2d150c4e9a03b89f1ca6e2f5.png` | 23.10 | 51.79 | 298 | 1420 | 2380 | 11340 |
