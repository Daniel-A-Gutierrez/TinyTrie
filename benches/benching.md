# Benchmarking

Run from the project root:

```bash
# Run all structures, all sizes
cargo bench --bench bench

# Run only a specific structure (case-insensitive substring match)
cargo bench --bench bench -- NibbleTrie
cargo bench --bench bench -- nibble
cargo bench --bench bench -- bit
cargo bench --bench bench -- BTreeMap

# Match multiple structures with a shared substring
cargo bench --bench bench -- trie    # TinyTrie, NibbleTrie, BitTrie, PolyTrie
cargo bench --bench bench -- opt    # NibbleOpt, PolyOpt
cargo bench --bench bench -- map    # BTreeMap, HashMap
```

The filter matches against: TinyTrie, NibbleTrie, BitTrie, PolyTrie, BTreeMap, HashMap, SortedVec, NibbleOpt, PolyOpt

Each bench runs for 3 seconds per size (10K, 100K, 10M keys). Benches run sequentially per structure within each section.

## Architecture

The bench uses a **trait-based design** with two layers:

1. **`TinyTrieMap`** (in `src/tiny_trie_map.rs`): A library-level trait unifying all four trie types behind a common API (`trie_new`, `trie_insert`, `trie_get`, `trie_iter_fwd`, `trie_iter_rev`, `trie_len`, `trie_optimize`). This abstracts away the different iterator types and null-terminator requirements.

2. **Per-test bench traits** (in `benches/bench.rs`): `InsertBench`, `LookupBench`, `FwdIterBench`, `RevIterBench`, `OptimizeBench` — each with one `run()` method. The `impl_trie_benches!` macro generates trivially identical impls for any type implementing `TinyTrieMap`. BTreeMap, HashMap, and SortedVec have manual impls.

3. **`mem_measure()`** function: Snapshots the allocator, runs a build closure, computes bytes/key. Not a trait — memory measurement needs allocation tracking before/after construction.

The `trie_` prefix on `TinyTrieMap` methods avoids collisions with inherent methods. `trie_iter_fwd`/`trie_iter_rev` use callbacks instead of returning named iterator types, since each trie has its own iterator type.

### Null-terminator normalization

`Structures` stores both `lookup_keys` (plain, for std collections and NibbleTrie) and `lookup_keys_null` (null-terminated, for TinyTrie, BitTrie, and PolyTrie). NibbleTrie's `LookupBench` impl uses plain keys; the other tries use null-terminated keys.

### Iterator semantics

`trie_iter_fwd` and `trie_iter_rev` use the `current() + next()/prev()` pattern, which correctly handles both TinyTrie (where `iter()` positions at the first key) and the other tries (where `iter()` positions before the first key). Similarly, all tries' `iter_last()` positions at the last key, so `current()` reads it before `prev()` advances backward.

## What gets filtered

- **Insertion**: TinyTrie, NibbleTrie, BitTrie, PolyTrie, BTreeMap, HashMap, SortedVec (7 structures)
- **Lookup, Memory**: all 9 structures
- **Forward iteration**: TinyTrie, NibbleTrie, BitTrie, PolyTrie, BTreeMap, SortedVec, NibbleOpt, PolyOpt (HashMap lacks iteration)
- **Backward iteration**: TinyTrie, NibbleTrie, BitTrie, PolyTrie, BTreeMap, NibbleOpt, PolyOpt (HashMap and SortedVec lack reverse iter)
- **Optimize**: NibbleOpt, PolyOpt only

If no structures in a section match the filter, that section is skipped entirely.

## Results files

`benches/bench_results.json` is the canonical data store — raw f64 values, persisted across runs. Filtering to a subset of structures only updates their rows; previous results for other structures are preserved.

`benches/bench_results.md` is regenerated from the JSON each run with human-readable formatting.

## Modifying bench parameters

- **Sizes**: edit `SIZES` constant in `benches/bench.rs`
- **Duration per bench**: edit `BENCH_SECS` constant in `benches/bench.rs`

## Adding a new structure

1. Implement `TinyTrieMap` in the struct's source file
2. Add `impl_trie_benches!(YourType);` in `benches/bench.rs`
3. Add `impl OptimizeBench` if it has `optimize()`
4. Add the name to `CONTESTANT_NAMES` and the corresponding `if active[N]` blocks in main()