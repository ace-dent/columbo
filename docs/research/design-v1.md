# columbo v1: design document

## Status

Proposed — awaiting Wira's approval before implementation.

## Language

**Rust** (edition 2024). Rationale:
- Memory safety for parsers of untrusted containers (malformed PNG, ZIP, GZIP).
- Cross-compilation to win7/win10 targets confirmed via project VMs.
- `cargo` + crates.io avoids build-system complexity.
- Bit manipulation ergonomics comparable to C.

## Architecture

Layered design with three modules:

```text
┌─────────────────────────────┐
│  Container layer            │
│  (gz, raw deflate)          │
│  Format detection → extract │
│  deflate stream → optimize  │
│  → rebuild container        │
└──────────┬──────────────────┘
           │ tokens, frequencies
┌──────────▼──────────────────┐
│  Block engine               │
│  5 candidates per block:    │
│  1. stored (multi-split)    │
│  2. fixed Huffman            │
│  3. dynamic rebuilt           │
│  4. dynamic original fallback │
│  5. dynamic pruned (≤2 iter) │
│  → strict min selection     │
└──────────┬──────────────────┘
           │ frequencies
┌──────────▼──────────────────┐
│  Huffman builder            │
│  Package-merge (deterministic│
│  freq→symbol tie-break)     │
│  Limits: 15 (lit/dist), 7 (clen)│
└──────────┬──────────────────┘
           │ code lengths
┌──────────▼──────────────────┐
│  Header RLE encoder         │
│  Greedy single-pass         │
│  + local strict replacements │
│  + HCLEN trailing trim      │
└─────────────────────────────┘
```

### Module: Container layer

- **Detection**: read first 2 bytes → GZIP (0x1f 0x8b) or raw Deflate (fallback).
- **GZIP**: validate CM=8, skip FEXTRA/FNAME/FCOMMENT/FHCRC, extract raw deflate.
- **Raw Deflate**: pass through directly.
- **Output**: rebuild container with updated CRC-32/ISIZE (GZIP) or raw bytes (Deflate).
- **Policy**: output written only if strictly smaller than input (byte-level comparison).
- Future: PNG, ZIP (v2).

### Module: Block engine

Each source block is parsed into tokens and scored under 5 candidates:

1. **Stored** — multi-block split for >65535 decoded bytes (defluff method).
2. **Fixed Huffman** — standard RFC 1951 fixed tables.
3. **Dynamic rebuilt** — package-merge trees from token frequencies (defluff method).
4. **Dynamic original fallback** — exact source bits if strictly smaller than candidate 3 (DeflOpt method, `0x406bcb`).
5. **Dynamic pruned** — scorer expands matches to literals (strict `<`), rebuilds trees
   from resulting frequencies, at most 2 iterations. Keeps best seen (deft4j pruned-recode method, bounded).

**Winner selection**: linear scan of costs, strict `<` comparison.
Scan order: stored (0) → fixed (1) → dynamic candidates (2). Ties favour earlier index.

**Token-count gate**: blocks with ≤25 tokens skip candidate search entirely,
emitted as fixed Huffman (defluff method). Saves computation on tiny blocks
where dynamic header overhead is prohibitive.

**Symbol-284 iteration**: if the source stream uses symbol 284 for length-258
matches, the dynamic candidate is refined once with scorer-result frequencies
(defluff method). At most one extra pass.

### Module: Huffman builder

**Algorithm**: Package-merge (Katajainen), deterministic.

- Sort leaves by `(frequency ASC, symbol ASC)` — counting sort for freq ≤287,
  qsort fallback for pathological cases (defluff method).
- Build length-limited tree with max depth 15 (lit/len, dist) or 7 (code-lengths).
- Output: canonical Deflate codes assigned by increasing length then increasing symbol.
- Single-tree construction per alphabet (no heap variants — package-merge is optimal).

### Module: Header RLE encoder

**Algorithm**: Greedy single-pass + local strict replacements (defluff method).

- Scan concatenated lit/len + dist length arrays.
- Zero runs: symbol 18 (11–138 zeros, 7 extra bits), symbol 17 (3–10 zeros, 3 extra bits),
  explicit symbol 0 for ≤2.
- Non-zero runs: emit value once, symbol 16 for 3–6 repeats (2 extra bits),
  explicit for ≤2.
- Strict replace pass: for each repeat token, compute `repeat_cost` vs `explicit_cost`;
  replace only when explicit is strictly smaller AND all values have non-zero code lengths.
- HCLEN trailing trim: remove zero entries from end of code-length array.

## Scope v1

**In**: GZIP, raw Deflate.
**Out**: PNG, ZIP (v2), block merging (v2+), multi-strategy RLE (v2+), heap Huffman variants (not needed).

## Cross-tool provenance

| Feature | Source |
|---|---|
| Package-merge builder | defluff 0.3.2 |
| Stored multi-block split | defluff 0.3.2 |
| 25-token gate | defluff 0.3.2 |
| Symbol-284 iteration | defluff 0.3.2 |
| Greedy RLE + local replacements | defluff 0.3.2 |
| Original bits fallback | DeflOpt 2.07 |
| Pruned match→literal rebuild (bounded) | deft4j β17 (adapted) |
| Strict no-larger output policy | DeflOpt 2.07, defluff 0.3.2 |

## Testing strategy

- **Unit tests**: Huffman builder (known freq sets vs expected code lengths), RLE encoder,
  bit I/O, CRC-32.
- **Roundtrip tests**: compress → optimize → decompress, verify byte-exact identity.
  Test vectors from the RE validation suite.
- **Regression tests**: output never larger than input; deterministic (same input → same output).
- **Cross-tool comparison**: compare output sizes against DeflOpt.exe (wine) and
  defluff.exe (wine) on a corpus of gzip files.
- **Windows validation**: run compiled binary on win7/win10 VMs.

## References

- `research/deflopt-methods.md` — DeflOpt 2.07 RE methods
- `research/deft4j-methods.md` — deft4j β17 RE methods
- `research/defluff-methods.md` — defluff 0.3.2 RE methods + validation
