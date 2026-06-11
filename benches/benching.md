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