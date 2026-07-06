# Trait Architecture

Three independent trait systems serve different purposes: **BenchableMap** for benchmark
abstraction, **ByteKey** for radix-trie key genericity, and **CTree's trait chain**
for B+ tree SIMD dispatch.

---
## BenchableMap (`benchable_map` crate, formerly `tiny-trie-trait` / `TinyTrieMap`)

A minimal trait in its own crate, implemented by each tree structure independently.
Exists so the bench harness can treat all trees uniformly without orphan-rule issues.
The `map_` prefix avoids collisions with inherent methods (e.g. `insert`, `get`, `iter`).

```rust
pub trait BenchableMap: Sized {
    fn map_new() -> Self;
    fn map_insert(&mut self, key: Vec<u8>, value: usize);
    fn map_get(&self, key: &[u8]) -> Option<usize>;
    fn map_iter_fwd(&self, f: impl FnMut(&[u8], &usize));
    fn map_iter_rev(&self, f: impl FnMut(&[u8], &usize));
    fn map_iter_fwd_index(&self, _f: impl FnMut(usize)) { unimplemented!(...) }
    fn map_iter_rev_index(&self, _f: impl FnMut(usize)) { unimplemented!(...) }
    fn map_len(&self) -> usize;
    fn map_optimize(&mut self) {}  // default no-op
}
```

Each crate implements it for its own type:

| Crate | Implementation |
|-------|---------------|
| `tiny-trie` | `impl BenchableMap for NibbleTrie<Vec<u8>, usize>`, `NibTrie<usize>`, `BitTrie<Vec<u8>, usize>`, `FixedLenNibbleTrie<usize, u32>` |
| `poly-trie` | `impl BenchableMap for PolyTrie<usize>` |

The bench harness (`bencher` crate, lib name `tiny_trie_bench`) depends on `benchable_map`
and the tree crates, so it can compare them head-to-head.

---

## ByteKey System (`tiny-trie` crate, `key_store.rs`)

For radix tries (NibbleTrie, NibTrie, etc.) that store keys as byte slices internally
but want type-safe insert/consume at the API boundary.

### Traits

| Trait | Purpose | Methods |
|-------|---------|---------|
| `ByteKey` | Convert to/from `&[u8]` preserving ordering | `as_bytes(&self) -> &[u8]`, `from_bytes(&[u8]) -> Self` |
| `NonNullKey : ByteKey` | Marker: byte representation contains no `0x00` | *(none)* |

### Implementations

| Type | `ByteKey` | `NonNullKey` |
|------|-----------|---------------|
| `Vec<u8>` | ✅ identity | — |
| `String` | ✅ UTF-8 bytes | — |
| `U64Key` | ✅ big-endian 8 bytes | — |
| `NonZeroBytes` | ✅ delegate to inner `Vec<u8>` | ✅ guaranteed no `0x00` |

### Usage

- `NibbleTrie<K, T, PTR, LEN, STAK>` where `K: ByteKey` — insert takes `K`,
  `get` takes `&[u8]`, `into_keys_values` returns `Vec<K>`
- Other tries (BitTrie, NibTrie, FixedLenNibbleTrie) still use `Vec<u8>` directly;
  can adopt `ByteKey` later

### `NonZeroBytes`

A `Vec<u8>` wrapper guaranteed to contain no `0x00`. Constructed via
`NonZeroBytes::new(v)` which returns `None` if `0x00` is present, or
`unsafe new_unchecked(v)`. Used by the bench for null-terminator tries
(BitTrie, PolyTrie).

---

## CTree Trait System (`ctree` crate, `tiny_btree.rs`)

For the B+ tree's SIMD-accelerated node search. Two dispatch paths branch on whether
keys are fixed-width or variable-length.

### Traits

| Trait | Purpose | Key bound |
|-------|---------|-----------|
| `FixedLenKey` | SIMD `find_position`/`find_upper_bound` over fixed-width arrays | `Copy + Eq + Ord + Sized` |
| `TreeKey` | Map user key → stored form + lookup needle | `Ord + Clone` |
| `Preview<P>` | Extract fixed-width preview from a key for SIMD pre-filter | — |
| `SearchStrategy<P> : TreeKey` | Static dispatch: fixed path or variable path | inherits `TreeKey` |
| `StoredKey` (sealed) | Compare stored key vs borrowed needle | `Ord + Clone`, sealed |
| `NoPreview` | ZST marker for fixed-key trees (no preview array) | struct, not trait |
| `TrieIndex` | Arena index type (`u8`/`u16`/`u32`/`u64`) | `Copy + Clone + …` |

### `FixedLenKey` implementors

```
u8, u16, u32, u64, i8, i16, i32, i64
// char: commented out (line 97)
```

### Blanket impls (from `FixedLenKey`)

These are the **fixed path**: SIMD scans the key array directly, no preview array.

```
impl<T>         TreeKey              for T          where T: FixedLenKey
impl<T>         Preview<NoPreview>   for T          where T: FixedLenKey
impl<K>         SearchStrategy<NoPreview> for K     where K: FixedLenKey
impl<K>         StoredKey            for K          where K: FixedLenKey           // stored as-is
impl<K>         StoredKey            for Box<[K]>   where K: FixedLenKey           // stored as boxed array
```

### Specific impls (from `TreeKey`)

These are the **variable path**: SIMD filters on a preview array, then scalar
`StoredKey::cmp_key` resolves collisions.

```
// TreeKey concrete impls
impl            TreeKey              for Vec<u8>    (Stored = Box<[u8]>, Needle = [u8])
impl            TreeKey              for Box<[u8]>  (Stored = Box<[u8]>, Needle = [u8])

// Preview concrete impls (P ∈ {u8, u16, u32, u64})
impl            Preview<P>           for Vec<u8>    // stored key
impl            Preview<P>           for Box<[u8]>  // stored key
impl            Preview<P>           for [u8]       // needle (unsized) — needed by K::Needle: Preview<P>

// SearchStrategy blanket (varlen)
impl<K>         SearchStrategy<P>    for K          where K: TreeKey + Preview<P>,
                                                    P: FixedLenKey,
                                                    K::Stored: StoredKey,
                                                    K::Needle: Preview<P>
```

### Key type → trait satisfaction

| Key type | Fixed path (P=NoPreview) | Variable path (P=u64) |
|----------|------------------------|------------------------|
| `u64` | ✅ all blankets | — |
| `Vec<u8>` | — | ✅ specific impls |
| `Box<[u8]>` | — | ✅ specific impls |

### CTree bounds

```rust
CTree<K, V, PTR, N, NP1, P = NoPreview>
where
    K: TreeKey + Preview<P> + SearchStrategy<P>,
    K::Stored: StoredKey,
    PTR: TrieIndex,
    V: Sized,
    P: Copy,
```

### Type aliases

```rust
type FixedCTree<K, V, PTR, N, NP1> = CTree<K, V, PTR, N, NP1, NoPreview>;
type VarCTree<K, V, PTR, N, NP1, P> = CTree<K, V, PTR, N, NP1, P>;
```

### Bench adapter

```rust
trait CTreeBenchKey: TreeKey + Clone + Ord + 'static {
    type Preview: Copy + Eq + Ord;
    const DEFAULT_PREVIEW: Self::Preview;
}
u64      → Preview = NoPreview    // fixed path
Vec<u8>  → Preview = u64         // variable path
```

Produces: `CTreeBench` (varlen), `CTreeOptBench`, `CTreeFixedBench` (u64 SIMD),
`CTreeFixedOptBench`. Registered as two contestants (`CTree` + `CTreeOpt`) that
carry both bytes and u64 variants; `variant_for(mode)` dispatches by key mode.

---

## Relationship between the three systems

The three trait systems are **independent** and live in separate crates:

- **`benchable_map`** (`BenchableMap`) — bench abstraction, implemented by each tree
- **`tiny-trie`** (`ByteKey`/`NonNullKey`) — radix-trie key genericity, insert takes
  `K: ByteKey`, internal comparison is `&[u8]`
- **`ctree`** (`TreeKey`/`StoredKey`/`Preview`/`SearchStrategy`) — B+ tree node search
  dispatch, insert takes `K: TreeKey`, internal search uses `FixedLenKey` SIMD or
  `Preview<P>` + scalar fallback

Potential future unification: `ByteKey` types could implement `TreeKey` (e.g.,
`Vec<u8>: TreeKey<Stored = Box<[u8]>>` already exists in the ctree crate), but
currently the two systems serve different purposes and don't interact.

---

## Crate layout

```
benchable_map/      ← BenchableMap trait (no dependencies)
tiny-trie/          ← NibbleTrie, NibTrie, BitTrie, DynTrie, FixedLenNibbleTrie
                    ← depends on: benchable_map
btrees/             ← IntBTree/StrBTree B+ tree, TinyArray
                    ← depends on: benchable_map
poly-trie/          ← PolyTrie, Arena
                    ← depends on: benchable_map
benches/            ← Bench harness (bin: bencher, lib: tiny_trie_bench)
                    ← depends on: tiny-trie, benchable_map, btrees, poly-trie
```