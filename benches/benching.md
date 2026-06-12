# Benchmarking

## Running the Suite

```bash
cargo bench
```

Runs all contestants at all sizes (10, 100, 1K, 100K, 10M) with 2s per bench.

## Targeted Runs

The bench accepts up to three space-separated arguments, each a comma-separated list:

```
cargo bench -- [TESTS] [SIZES] [STRUCTURES]
```

Any argument may be omitted or left empty to mean "all."

### Tests

Comma-list of benchmark types to run:

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

Comma-list of key counts. Must be one of the canonical sizes: `10`, `100`, `1000`, `100000`, `10000000`.

### Structures

Comma-list of substring filters (case-insensitive). A contestant matches if its name contains any filter.

### Examples

```bash
# All tests, all sizes, all structures
cargo bench

# Lookup only, sizes 10 and 100, all structures
cargo bench -- lookup 10,100

# All tests, size 10 only, all structures
cargo bench -- "" 10

# Lookup only, all sizes, only NibbleTrie variants
cargo bench -- lookup "" NibbleTrie

# All tests, all sizes, only unchecked variants
cargo bench -- "" "" Unchecked

# Insert + lookup, size 1M, HashMap vs BTreeMap
cargo bench -- insert,lookup 1000000 HashMap,BTreeMap
```

## Contestants

| Name                | Tests                                             | Notes                                    |
|---------------------|---------------------------------------------------|------------------------------------------|
| `TinyTrie`          | insert, lookup, fwd, rev, memory                 | 6-bit inline, null-terminated keys      |
| `NibbleTrie`        | insert, lookup, fwd, rev, fwd_idx, rev_idx, memory | u32/u32 default                          |
| `BitTrie`           | insert, lookup, fwd, rev, memory                 | Bit-level trie, null-terminated keys     |
| `PolyTrie`          | insert, lookup, fwd, rev, memory                 | Graduated node sizes                     |
| `BTreeMap`          | insert, lookup, fwd, rev, memory                 | std::collections baseline                |
| `HashMap`           | insert, lookup, memory                           | std::collections baseline                |
| `SortedVec`        | insert, lookup, fwd, memory                      | Binary search on sorted vec              |
| `NibbleOpt`         | lookup, fwd, rev, fwd_idx, rev_idx, optimize, memory | NibbleTrie after optimize()              |
| `PolyOpt`           | lookup, fwd, rev, optimize, memory               | PolyTrie after optimize()                |
| `LinkedList`        | insert, fwd, rev, memory                         | O(n) lookup, baseline                    |
| `NibbleUnchecked`   | lookup                                            | get_unchecked (assumes key in set)      |
| `NibbleOptUnchecked`| lookup                                            | get_unchecked on optimized trie          |
| `DynNibbleTrie`    | insert, lookup, fwd, rev, memory                 | Auto-promoting PTR u8→u16→u32→u64       |
| `DynNibbleOpt`      | lookup, fwd, rev, optimize, memory               | DynNibbleTrie after optimize()           |

## Output

Results print to stdout as sorted tables (fastest first for rates, smallest first for memory) and persist to:

- `benches/bench_results.json` — full structured data
- `benches/bench_results.md` — markdown tables (sorted, merged across runs)

Each run merges into the existing results, overwriting only the contestants and sizes that were actually run. Previous results for other sizes/contestants are preserved.

## Unchecked Lookup

`NibbleUnchecked` and `NibbleOptUnchecked` use `get_unchecked()`, which skips key comparison at terminal and leaf nodes. The assumption is that the queried key **is present in the trie** — once the nibble path reaches a terminal node or leaf, the index is returned directly with no SIMD verification.

The bench uses `hit_keys` (keys known to be in the trie) rather than the mixed hit/miss `lookup_keys` used by other contestants. The ops/sec rate is computed per hit key, so the numbers are directly comparable across lookup methods.