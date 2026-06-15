# Benchmarking

## Running the Suite

```bash
cargo run --release --bin bench
```

Runs all contestants at all sizes (10, 100, 1K, 10K, 100K, 10M) with 2s per bench, using sequential `"key_N"` keys.

`--release` is required for meaningful results — unoptimized builds are not representative.

## CLI Options

```
bench [OPTIONS]

Options:
  -t, --tests <TESTS>            Comma-separated test names [default: all]
  -s, --sizes <SIZES>            Comma-separated list or inclusive range (see below) [default: all]
      --structures <STRUCTURES>  Comma-separated substring filters [default: all]
      --keys <KEYS>              Key generation mode [default: sequential]
      --corpus <CORPUS>          Path to corpus file (required for --keys=words|lines)
      --time <TIME>              Seconds per benchmark [default: 2]
  -h, --help                    Print help
  -V, --version                  Print version
```

All options go after `--`:

```bash
cargo run --release --bin bench -- [OPTIONS]
```

### Tests

Comma-separated list of benchmark types. Defaults to all.

| Name       | Description                    |
|------------|--------------------------------|
| `insert`   | Key insertion                  |
| `lookup`   | Key lookup (hits + misses)     |
| `fwd`      | Forward iteration              |
| `rev`      | Backward iteration             |
| `fwd_idx`  | Forward index-only iteration    |
| `rev_idx`  | Backward index-only iteration   |
| `optimize` | Optimize (DFS buf rewrite)    |
| `memory`   | Bytes per key                  |

### Sizes

The `--sizes` flag accepts two formats:

**Comma-separated list** — must be from the canonical set (10, 100, 1K, 10K, 100K, 10M):

```bash
--sizes 10,100,1000
```

**Inclusive range** — selects all canonical sizes within the range (both ends inclusive):

```bash
--sizes 100..100000    # runs 100, 1000, 10000, 100000
--sizes 10..1000      # runs 10, 100, 1000
```

Omitting `--sizes` runs all canonical sizes.

### Structures

Comma-separated substring filters (case-insensitive). A contestant matches if its name contains any filter. Defaults to all.

### Key Sources

| `--keys` value | Description                                        |
|----------------|----------------------------------------------------|
| `sequential`   | Sequential `"key_N"` strings (default)              |
| `random`       | Random 4–16 byte keys (unique, via BTreeSet dedup)  |
| `words`        | Whitespace-delimited tokens from corpus file        |
| `lines`        | Newline-delimited lines from corpus file            |

`--keys=words` and `--keys=lines` require `--corpus <file>`. Corpus keys are sorted and deduplicated. If the requested size exceeds the corpus, all available keys are used with a warning.

Shared loading (`load_corpus_lines`, `load_corpus_words`) lives in `lib.rs` so `trie-stats` and bench both use the same code.

### Examples

```bash
# All tests, all sizes, all structures
cargo run --release --bin bench

# Lookup only, sizes 100 through 10000
cargo run --release --bin bench -- --tests lookup --sizes 100..10000

# Insert + lookup, explicit sizes
cargo run --release --bin bench -- --tests insert,lookup --sizes 10,100,1000

# Lookup, all sizes, only NibbleTrie variants
cargo run --release --bin bench -- --tests lookup --structures NibbleTrie

# All tests, all sizes, only unchecked variants
cargo run --release --bin bench -- --structures Unchecked

# Insert + lookup, size 1M, HashMap vs BTreeMap
cargo run --release --bin bench -- --tests insert,lookup --sizes 1000000 --structures HashMap,BTreeMap

# Random keys (tests real-prefix behavior)
cargo run --release --bin bench -- --tests lookup --sizes 1000 --keys random

# 5-second budget per bench
cargo run --release --bin bench -- --time 5

# Real text words from project source
cargo run --release --bin bench -- --tests lookup --sizes 1000 --keys words --corpus corpus.txt

# Real text lines from project source
cargo run --release --bin bench -- --tests lookup --sizes 1000 --keys lines --corpus corpus.txt
```

## Contestants

| Name                | Tests                                             | Notes                                    |
|---------------------|---------------------------------------------------|------------------------------------------|
| `NibbleTrie`        | insert, lookup, fwd, rev, fwd_idx, rev_idx, memory | u32/u32 default                          |
| `BTreeMap`          | insert, lookup, fwd, rev, memory                 | std::collections baseline                |
| `HashMap`           | insert, lookup, memory                           | std::collections baseline                |
| `SortedVec`        | insert, lookup, fwd, memory                      | Binary search on sorted vec              |
| `NibbleOpt`         | lookup, fwd, rev, fwd_idx, rev_idx, optimize, memory | NibbleTrie after optimize()              |
| `LinkedList`        | insert, fwd, rev, memory                         | O(n) lookup, baseline                    |
| `NibbleUnchecked`   | lookup                                            | get_unchecked (assumes key in set)      |
| `NibbleOptUnchecked`| lookup                                            | get_unchecked on optimized trie          |
| `DynTrie`    | insert, lookup, fwd, rev, memory                 | Auto-promoting PTR u8→u16→u32→u64       |
| `DynTrieOpt`      | lookup, fwd, rev, optimize, memory               | DynTrie after optimize()           |

## Structure Analysis

```bash
cargo run --bin trie-stats <corpus_file> [max_keys] [--words]
```

Walks every arena node and reports: fanout histogram, parent-child and sibling nibble overlap, STAK chain depth, terminal/leaf-only counts, depth distribution.

| Flag       | Description                                    |
|------------|------------------------------------------------|
| *(default)*| Lines from corpus as keys                       |
| `--words`  | Whitespace-delimited tokens from corpus as keys |

Examples:
```bash
cargo run --bin trie-stats corpus.txt 1000
cargo run --bin trie-stats corpus.txt --words
```

## Output

Results print to stdout as sorted tables (fastest first for rates, smallest first for memory) and persist to:

- `benches/bench_results.json` — full structured data
- `benches/bench_results.md` — markdown tables (sorted, merged across runs)

Each run merges into the existing results, overwriting only the contestants and sizes that were actually run. Previous results for other sizes/contestants are preserved.

## Unchecked Lookup

`NibbleUnchecked` and `NibbleOptUnchecked` use `get_unchecked()`, which skips key comparison at terminal and leaf nodes. The assumption is that the queried key **is present in the trie** — once the nibble path reaches a terminal node or leaf, the index is returned directly with no SIMD verification.

The bench uses `hit_keys` (keys known to be in the trie) rather than the mixed hit/miss `lookup_keys` used by other contestants. The ops/sec rate is computed per hit key, so the numbers are directly comparable across lookup methods.