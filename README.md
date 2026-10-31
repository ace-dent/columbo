# columbo

“Just One More Thing” – optimize the last few bytes in a Deflate stream. 🕵🏻‍♂️

<hr>

Combines methods from the following work:

- [DeflOpt](http://web.archive.org/web/20131208161446/http://www.walbeehm.com/download/index.html) v2.07 (05-Sep-2007) by Ben Jos Walbeehm.
- [defluff](https://web.archive.org/web/20230604215335/https://encode.su/threads/1214-defluff-a-deflate-huffman-optimizer) v0.3.2 (07-Apr-2011) by Joachim Henke.
- [deft4j](https://github.com/NeRdTheNed/deft4j) v1.0.0-beta-17 (12-Nov-2023) by Ned Loynd.
- [turtledeflate](https://github.com/rwillenbacher/turtledeflate) (as at 25-Jul-2026) by Ralf Willenbacher.

The code and reverse engineering in this project were completed with AI models.

## Usage

```text
columbo [options] input [input ...]
columbo [options] --out file input
columbo --dry-run [options] input [input ...]
```

Each `input` may be a PNG/APNG, GZIP, ZIP, or zlib file; its format is detected automatically. Multiple inputs are processed sequentially and optimized in place unless `--dry-run` is used. `--out` is available only when processing one input. An existing file is replaced only when the output is smaller, saves at least one meaningful Deflate bit, applies `--strip` changes, or changes a ZIP member of at most four bytes from Deflate to Store.

Input and decoded Deflate data are limited to 1 GiB per file, and decoded data to 512 times the input size or 64 MiB, whichever is larger. These limits bound data, not process memory: peak memory is typically 10–125 times the decoded size, highest with `--max`.

### Options

- `-h`, `--help`: Show command-line help and exit.
- `-v`, `--verbose`: Show completed stream reports in physical order, including route timings, bit gains, and final block choices.
- `-m`, `--max`: Enable slower block-boundary and token-spelling searches for potentially smaller output.
- `-d`, `--dry-run`: Run the complete optimization for each input and report savings without writing files. With one input, `--out` is ignored; with several, it is rejected.
- `--out <file>`: Write a single input to `file` instead of modifying it. When there is nothing to write, Columbo copies the input to a missing `file` and leaves an existing `file` unchanged.
- `-t <seconds>`, `--timeout <seconds>`: Stop starting new search routes after the specified time. The default is 180 seconds; values are clamped to 10–4000 seconds and fractions round up. An active route receives a grace period of 10% plus one second.
- `--strict <0|1>`: Emit conservative Deflate for strict and older decoders. On by default (`1`); `0` turns it off and permits compact empty or singleton Huffman alphabets and the non-standard length-258 alias. Files are still replaced only when Columbo saves space, so a file left unchanged keeps its original encoding.
- `--strip`: Remove supported metadata, credentials, signatures, and extraneous payloads, including unreferenced ZIP records and trailing data. Decoded content and supported structural information are preserved; ambiguous layouts are rejected. ZIP members that are neither Stored nor Deflate lose only their comments (with Info-ZIP's Unicode copy) and their time, owner, permission and padding extra fields, since their method may need the other fields; encrypted members keep every extra field. Without `--strip`, metadata and understood extraneous payloads are kept.
- `--raw`: Treat the input as a headerless RFC 1951 Deflate stream instead of detecting a wrapper.

Options that take a value also accept `--out=<file>`, `--timeout=<seconds>`, and `--strict=<0|1>`.

### Examples

```sh
columbo image.png
columbo --out optimized.png image.png
columbo --dry-run --max archive.zip
columbo --raw --timeout 60 stream.deflate
```

## Acknowledgements

Thanks to all contributors.

## Development

See the [contributor guide](docs/CONTRIBUTING.md) for build and test commands,
and the [codebase guide](docs/architecture.md) for module responsibilities.
The default test suite needs no private corpus. Corpus regressions are opt-in;
their fixtures remain local and are excluded from Git and source packages.

## Legal

All trademarks are the property of their respective owners.

This work is provided under the [MIT](LICENSE) license, on an "as is" basis, without warranty of any kind regarding accuracy, completeness, or fitness for any specific purpose. Use of the provided content is entirely at your own risk. Please see the LICENSE for full terms of use.

For alternative licensing arrangements, commercial use, or other permissions, please contact the project author directly.
