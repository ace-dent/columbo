# columbo

“Just One More Thing” – optimize the last few bytes in a Deflate stream. 🕵🏻‍♂️

<hr>

## ⚠️ Alpha software

This project is alpha software and is relatively untested. Expect bugs, incomplete behavior, and breaking changes. Do not rely on it for production or other critical workloads without independently reviewing and thoroughly testing it first.

Code in this project has been generated with the assistance of OpenAI models. Review and validate the code before use.

Combines methods from the following work:

- [DeflOpt](http://web.archive.org/web/20131208161446/http://www.walbeehm.com/download/index.html) v2.07 (05-Sep-2007) by Ben Jos Walbeehm. Binary is compiled for Windows 32-bit `i386`.
- [defluff](https://web.archive.org/web/20230604215335/https://encode.su/threads/1214-defluff-a-deflate-huffman-optimizer) v0.3.2 (07-Apr-2011) by Joachim Henke. Binaries for Windows `i686`; macOS `i386` (and PowerPC); linux `i686` and `x86_64`.
- [deft4j](https://github.com/NeRdTheNed/deft4j) v1.0.0-beta-17 (12-Nov-2023) by Ned Loynd.
- [turtledeflate](https://github.com/rwillenbacher/turtledeflate) (as at 25-Jul-2026) by Ralf Willenbacher.

This project focuses on reverse-engineering the techniques used to save the last few bytes from deflate streams.

## Usage

```text
columbo [options] [--out file] input
columbo --dry-run [options] input
```

`input` may be a PNG/APNG, GZIP, ZIP, or zlib file; its format is detected automatically. Columbo optimizes the input in place unless `--out` or `--dry-run` is used. An existing file is replaced only when the output is smaller or saves at least one meaningful Deflate bit. Input and decoded Deflate data are limited to 1 GiB.

### Options

- `-h`, `--help`: Show command-line help and exit.
- `-v`, `--verbose`: Show route timings, bit gains, and final block choices.
- `--visual`: Show a live Deflate block map. Requires an interactive terminal and cannot be combined with `--verbose`.
- `-m`, `--max`: Enable slower block-boundary and token-spelling searches for potentially smaller output.
- `-d`, `--dry-run`: Run the complete optimization and report savings without writing a file. This mode accepts one positional input and ignores `--out`.
- `--out <file>`: Write the result to `file` instead of modifying the input. For compatibility, an output path may also be supplied as a second positional argument.
- `-t <seconds>`, `--timeout <seconds>`: Stop starting new search routes after the specified time. The default is 180 seconds; values are clamped to 10–4000 seconds and fractions round up. An active route receives a grace period of 10% plus one second.
- `--strict <0|1>`: Select conservative Deflate output. The default, `1`, supports strict and older decoders; `0` permits compact empty or singleton Huffman alphabets and the non-standard length-258 alias.
- `--strip`: Remove supported PNG, GZIP, and ZIP metadata or comment fields. Metadata is preserved by default.
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


## Legal

All trademarks are the property of their respective owners.

This work is provided under the [MIT](/.LICENSE) license, on an "as is" basis, without warranty of any kind regarding accuracy, completeness, or fitness for any specific purpose. Use of the provided content is entirely at your own risk. Please see the LICENSE for full terms of use.

For alternative licensing arrangements, commercial use, or other permissions, please contact the project author directly.
