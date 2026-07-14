# arrays

Standalone, zero-dependency benchmark sandbox. Not depended on by any other
workspace crate. The SIMD node-search machinery (`FixedLenKey`,
`SearchStrategy`, `impl_fixed_len_key_simd!`) lives in `btrees`, NOT here.

## Contents

- `tiny_array.rs` — a minimal `TinyArray<T, N>`: `len: u8` +
  `[MaybeUninit<T>; N]`, `Copy` when `T: Copy` (no `Drop`). API is just
  `new/get/get_mut/as_slice/insert_at/push/len/is_full`. **No** search,
  SIMD, `find_position`, or `ArraySearch` trait — this is a different,
  simpler copy than `btrees`/`tiny-trie`'s `TinyArray` (those own `Drop`
  and have `drain_*`/rebalance helpers).
- `flat_tree.rs` — three-way leaf-node micro-trie benchmark over interned
  `Box<[u8]>` keys: `SortedArray` (binary-search baseline) vs `FlatTree`
  (fnode pre-order `(depth,symbol,ptr)` depth-tracking linear scan) vs
  `DecisionTree` (adds a `next`-sibling skip). `run_benchmarks()` is
  invoked by `examples/bench.rs`; `correctness_check()` does 200
  randomized cross-checks vs `SortedArray`. `#![allow(dead_code)]`.
- `ssa.rs` — 2-line stub ("sparse sorted arena"), NOT declared in
  `lib.rs`. Dead.