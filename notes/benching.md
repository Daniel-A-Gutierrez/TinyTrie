# Benchmarking

## Running the Suite

```bash
cargo run --release -p bencher --bin bencher
```

The crate is `bencher` and the binary is `bencher`. The `-p` flag selects the workspace member; `--bin bencher` selects the binary (there's also `trie-stats`).

Runs all contestants at all sizes (10, 100, 1K, 10K, 100K, 10M) with 2s per bench, using sequential `"key_N"` keys.

`--release` is required for meaningful results — unoptimized builds are not representative.

## CLI Options

```
bencher [OPTIONS]

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
cargo run --release -p bencher --bin bencher -- [OPTIONS]
```

### Tests

Comma-separated list of benchmark types. Defaults to all.

| Name       | Description                    |
|------------|--------------------------------|
| `insert`   | Key insertion                  |
| `lookup`   | Key lookup (hits + misses)     |
| `fwd`      | Forward iteration              |
| `rev`      | Backward iteration              |
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
| `seq-u64`      | Sequential u64 keys (as 8-byte big-endian byte keys) |
| `random-u64`   | Random u64 keys (as 8-byte big-endian byte keys)    |

The `-u64` modes produce fixed-width 8-byte big-endian keys. These are `CTree`'s natural domain (u64 keys). `VarCTree` declares a **variable-length** key domain and is **skipped** on the `-u64` modes — feeding it fixed-width u64 reps would mischaracterize a variable-length-key store. To compare `CTree` vs `VarCTree` head-to-head, use a variable-length mode (`sequential`, `random`, `words`, `lines`); there `CTree` hashes the bytes to u64 and SIMD-searches, while `VarCTree` binary-searches the raw `Box<[u8]>`.

### Key domains

Each contestant declares a `KeyVariant` that the harness uses to skip it on incompatible key modes:

| Domain     | Meaning                                              | Skipped modes                          |
|------------|------------------------------------------------------|----------------------------------------|
| `Any`      | Accepts all key modes (default)                     | none                                   |
| `NonZero`  | No embedded null bytes (null-terminator stores)      | `random`, `random-u64`, `seq-u64`      |
| `U64`      | Fixed-width u64 keys                                | `sequential`, `random`, `words`, `lines` |

`CTree` carries both `Bytes` and `U64` variants; `variant_for(mode)` dispatches by key mode.

`--keys=words` and `--keys=lines` require `--corpus <file>`. Corpus keys are sorted and deduplicated. If the requested size exceeds the corpus, all available keys are used with a warning. Corpus files (`corpus.txt`, `wikipedia.txt`) live in the workspace root — pass the path relative to the workspace root:

```bash
cargo run --release -p bencher --bin bencher -- --keys words --corpus ../corpus.txt
```

Or run from the workspace root:

```bash
cargo run --release -p bencher --bin bencher -- --keys words --corpus corpus.txt
```

### Examples

```bash
# All tests, all sizes, all structures
cargo run --release -p bencher --bin bencher

# Lookup only, sizes 100 through 10000
cargo run --release -p bencher --bin bencher -- --tests lookup --sizes 100..10000

# Insert + lookup, explicit sizes
cargo run --release -p bencher --bin bencher -- --tests insert,lookup --sizes 10,100,1000

# Lookup, all sizes, only NibbleTrie variants
cargo run --release -p bencher --bin bencher -- --tests lookup --structures NibbleTrie

# All tests, all sizes, only unchecked variants
cargo run --release -p bencher --bin bencher -- --structures Unchecked

# Insert + lookup, size 1M, HashMap vs BTreeMap
cargo run --release -p bencher --bin bencher -- --tests insert,lookup --sizes 1000000 --structures HashMap,BTreeMap

# Random keys (tests real-prefix behavior)
cargo run --release -p bencher --bin bencher -- --tests lookup --sizes 1000 --keys random

# 5-second budget per bench
cargo run --release -p bencher --bin bencher -- --time 5

# Real text words from project source
cargo run --release -p bencher --bin bencher -- --tests lookup --sizes 1000 --keys words --corpus corpus.txt

# Real text lines from project source
cargo run --release -p bencher --bin bencher -- --tests lookup --sizes 1000 --keys lines --corpus corpus.txt
```

## Contestants

| Name                | Tests                                             | Notes                                    |
|---------------------|---------------------------------------------------|------------------------------------------|
| `NibbleTrie`        | insert, lookup, fwd, rev, fwd_idx, rev_idx, memory | u32/u32 default                          |
| `BTreeMap`          | insert, lookup, fwd, rev, memory                 | std::collections baseline                |
| `HashMap`           | insert, lookup, memory                           | std::collections baseline                |
| `SortedVec`        | insert, lookup, fwd, memory                      | Binary search on sorted vec              |
| `NibbleOpt`         | lookup, fwd, rev, fwd_idx, rev_idx, optimize, memory | NibbleTrie after optimize()              |
| `NibbleUnchecked`   | lookup                                            | get_unchecked (assumes key in set)      |
| `NibbleOptUnchecked`| lookup                                            | get_unchecked on optimized trie          |
| `DynTrie`           | insert, lookup, fwd, rev, memory                 | Auto-promoting PTR u8→u16→u32→u64       |
| `DynTrieOpt`        | lookup, fwd, rev, optimize, memory                | DynTrie after optimize()                 |
| `CTree`             | insert, lookup, fwd, rev, memory                 | B+ tree unified generic. In variable-length modes: `CTree<Vec<u8>,…>` with u64 preview + scalar fallback. In u64 modes: `CTree<u64,…>` with direct SIMD `find_position`. |
| `CTreeOpt`          | lookup, fwd, rev, optimize, memory                | CTree after `optimize()`. Same dual-path dispatch. |

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
cargo run -p bencher --bin trie-stats -- corpus.txt 1000
cargo run -p bencher --bin trie-stats -- corpus.txt --words
```

## Output

Results print to stdout as sorted tables (fastest first for rates, smallest first for memory) and persist to:

- `benches/bench_results_<keymode>.json` — full structured data
- `benches/bench_results_<keymode>.md` — markdown tables (sorted, merged across runs)

Each run merges into the existing results, overwriting only the contestants and sizes that were actually run. Previous results for other sizes/contestants are preserved.

## Unchecked Lookup

`NibbleUnchecked` and `NibbleOptUnchecked` use `get_unchecked()`, which skips key comparison at terminal and leaf nodes. The assumption is that the queried key **is present in the trie** — once the nibble path reaches a terminal node or leaf, the index is returned directly with no SIMD verification.

The bench uses `hit_keys` (keys known to be in the trie) rather than the mixed hit/miss `lookup_keys` used by other contestants. The ops/sec rate is computed per hit key, so the numbers are directly comparable across lookup methods.