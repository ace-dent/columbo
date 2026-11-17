# columbo

“Just One More Thing” – optimize the last few bytes in a Deflate stream. 🕵🏻‍♂️

Columbo is a lossless post-optimizer for files that are already Deflate-compressed: PNG/APNG, GZIP, ZIP, zlib, and raw Deflate streams. It does not recompress. It keeps the matches the original compressor found and re-encodes them with better block boundaries and Huffman codes, so run it last, after tools such as Zopfli or oxipng. The decoded data never changes, and metadata is kept unless you use `--strip`.

## Install

Columbo builds with [Rust](https://www.rust-lang.org/tools/install) 1.73 or newer and has no other dependencies:

```sh
cargo install --locked --git https://github.com/ace-dent/columbo
```

## Usage

```sh
columbo image.png                       # optimize in place
columbo --max first.png second.png      # search harder; slower
columbo --dry-run archive.zip           # report savings, write nothing
columbo --out smaller.gz original.gz    # keep the original
columbo - < image.png > smaller.png     # read stdin, write stdout
```

The format of each input is detected automatically, and several inputs are processed one after another.

> [!IMPORTANT]
> Inputs are optimized **in place** unless you pass `--out`, `--stdout`, or `--dry-run`. A file is replaced only when Columbo saves at least one bit or `--strip` removes something; the complete output is written to a temporary file and then renamed over the original. See [file safety](docs/file-safety.md) for details.

### Options

| Option | Description |
| --- | --- |
| `-m`, `--max` | Enable slower block-boundary and token-spelling searches for potentially smaller output. |
| `-d`, `--dry-run` | Run the complete optimization and report savings without writing anything. `--out` and `--stdout` are ignored. |
| `--out <file>` | Write a single input to `file` instead of replacing it. |
| `--stdout` | Write a single input's output to standard output. Reports then go to standard error. |
| `-t`, `--timeout <seconds>` | Stop starting new searches after this long on each input. Default 180; range 10–4000. Searches already running get a grace period of 10% plus one second. |
| `--strict <n>` | `1` (default) emits conservative Deflate that strict and older decoders accept. `0` permits a few more compact encodings that [some decoders reject](docs/routes-and-methods.md#strict-and-relaxed-output). |
| `--strip` | Remove metadata, credentials, signatures, and extraneous data such as trailing bytes, and merge concatenated GZIP members. |
| `--raw` | Treat the input as a headerless RFC 1951 Deflate stream instead of detecting its format. |
| `-v`, `--verbose` | Show a report for each stream: search timings, bit gains, and final block choices. |
| `-h`, `--help` | Show command-line help. |

Options that take a value also accept the `--option=value` form.

### Notes

- `--out` and `--stdout` always leave you with a complete file. When nothing was saved, `--stdout` emits the original and `--out` copies the input to `file`, or leaves `file` alone if it already exists.
- An input of `-` reads standard input and writes to standard output unless `--out` names a file. Use `./-` for a file named `-`. See [standard input and output](docs/file-safety.md#standard-input-and-output) for how piped output differs from a replaced file.
- `--strip` preserves the decoded content but not everything a file may rely on: on PNG it removes color information such as `iCCP` and `gAMA` along with other ancillary chunks. [Container methods](docs/routes-and-methods.md#container-methods) lists what each format loses and keeps.
- The exit status is 0 on success, 1 if any input failed, and 2 for a command-line error.

### Limits

- Each file may be up to 1 GiB and decode to at most 1 GiB. A file of 2 MiB or more may decode to the full 1 GiB; a smaller one is held to 512 times its size, but always at least 64 MiB, to guard against decompression bombs.
- Each Deflate stream is counted as its decoded bytes plus 14 bytes per literal or match and about 6 KB per block, up to 4 GiB; beyond that Columbo reports `Deflate structure exceeds internal memory safety limit`. A large stream that compresses poorly can reach this before the 1 GiB limit: one made only of literals stops at about 270 MiB decoded. Streams split into very small blocks (under 12 compressed bytes each on average) can be held to 256 MiB, as are all streams on 32-bit systems.
- Peak memory is typically 10–125 times the decoded size, highest with `--max`!

## Documentation

- [Routes and methods](docs/routes-and-methods.md): how every optimization works, and what is in and out of scope.
- [File safety](docs/file-safety.md): how outputs are written and what happens on interruption.
- Benchmarks against [DeflOpt](docs/deflopt-benchmark.md), [defluff](docs/defluff-benchmark.md), and [deft4j](docs/deft4j-timed-benchmark.md).
- [Contributor guide](docs/CONTRIBUTING.md) and [codebase guide](docs/architecture.md): build and test commands, and module responsibilities.

## Acknowledgements

Columbo combines methods and concepts from the following work:

- [DeflOpt](http://web.archive.org/web/20131208161446/http://www.walbeehm.com/download/index.html) v2.07 (05-Sep-2007) by Ben Jos Walbeehm.
- [defluff](https://web.archive.org/web/20230604215335/https://encode.su/threads/1214-defluff-a-deflate-huffman-optimizer) v0.3.2 (07-Apr-2011) by Joachim Henke.
- [deft4j](https://github.com/NeRdTheNed/deft4j) v1.0.0-beta-17 (12-Nov-2023) by Ned Loynd.
- [turtledeflate](https://github.com/rwillenbacher/turtledeflate) (as at 25-Jul-2026) by Ralf Willenbacher.

Thanks to all contributors, including @Enivex, @TPS and @YadeWira. 

🤖 The code and reverse engineering in this project were completed extensively with AI.


## Legal

All trademarks are the property of their respective owners.

This work is provided under the [MIT](LICENSE) license, on an "as is" basis, without warranty of any kind regarding accuracy, completeness, or fitness for any specific purpose. Use of the provided content is entirely at your own risk. Please see the LICENSE for full terms of use.

For alternative licensing arrangements, commercial use, or other permissions, please contact the project author directly.
