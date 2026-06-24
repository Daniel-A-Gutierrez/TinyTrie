# Benchmark Trait Structure

## Overview

The benchmark harness uses a layered design that keeps contestant operations
monomorphized per key type while still running heterogeneous structures side-by-
side.

## Core Types

### `Benchable<K>`

Every contestant implements this trait for its *native* key type `K` (`Vec<u8>`,
`NonZeroBytes`, or `u64`).  The trait methods each return `Option<()>` (or
`Option<f64>` for memory) so a contestant can signal “unsupported” for a
given test.

| Method | Purpose |
|--------|---------|
| `build(&mut self, keys: &[K], ctx: &BenchCtx<K>)` | Populate internal state once per size |
| `bench_insert` | Build a fresh tree from scratch |
| `bench_lookup` | Query against `ctx.lookup_keys` |
| `bench_fwd_iter` / `bench_rev_iter` | Range iteration |
| `bench_fwd_idx` / `bench_rev_idx` | Index-only iteration |
| `bench_optimize` | Run structure-specific optimize pass |
| `bench_memory` | Bytes-per-key via `read_allocated()` |
| `lookup_ops(&self, ctx) -> usize` | Override op count (default = `lookup_keys.len()`) |

### `BenchCtx<K>`

Shared lookup sets, built once per size and borrowed by contestants.  Contains
four key slices:

- `lookup_keys` — hit+miss interleaved (used by most contestants)
- `lookup_keys_null` — same keys with a trailing `0x00` (for null-terminator tries)
- `fl_lookup_keys` — truncated to ≤16 bytes (for `FixedLenBench`)
- `hit_keys` — only keys known to exist (for unchecked lookup benches)

Three type aliases exist: `BenchContext = BenchCtx<Vec<u8>>`,
`BenchContextNz = BenchCtx<NonZeroBytes>`, and the raw `BenchCtx<u64>`.

### `Bench` enum

```rust
enum Bench {
    Bytes(Box<dyn Benchable<Vec<u8>>>),
    NonZero(Box<dyn Benchable<NonZeroBytes>>),
    U64(Box<dyn Benchable<u64>>),
}
```

Only the *contestant type* is erased; `K` stays concrete.  This means a
`CTree<u64>` bench still inlines its SIMD `find_position` — the `dyn` vtable is
for `CTreeFixedBench`, not for `u64`.

`Bench::skip_for(mode)` filters contestants structurally:
- `Bytes` → skips fixed-width `u64` modes
- `NonZero` → skips modes that may emit `0x00`
- `U64` → skips non-`u64` modes

### `Contestant` & `Keys`

```rust
struct Contestant {
    name: &'static str,
    max_size: Option<usize>,   // None = no limit
    bench: Bench,
}

struct Keys {
    bytes: Vec<Vec<u8>>,
    nonzero: Vec<NonZeroBytes>,
    u64: Vec<u64>,
    ctx_b: BenchCtx<Vec<u8>>,
    ctx_nz: BenchCtx<NonZeroBytes>,
    ctx_u: BenchCtx<u64>,
}
```

`Contestant` dispatch methods (`build`, `bench_insert`, `bench_lookup`, …)
match on the `Bench` variant and forward the correct slice / context from
`Keys`.

## `bench_query_methods!` macro

Most trie contestants share the same query logic (lookups, iteration).  The macro
generates these methods so only `build`, `bench_insert`, `bench_optimize`, and
`bench_memory` need to be handwritten.

```rust
bench_query_methods! {
    field: trie,              // struct field holding the tree
    ctx: BenchContext,        // context type alias (BenchContext | BenchContextNz | BenchCtx<u64>)
    lookup: get(lookup),      // method(ctx_field)  see table below
    fwd_iter: trie_callback,  // iteration style      see table below
    rev_iter: trie_callback,
    index_iter: true,         // optional, generates bench_fwd_idx / bench_rev_idx
    unchecked: true,          // optional, overrides lookup_ops to hit_keys.len()
}
```

### Lookup arms

| Spec | Generated body |
|------|----------------|
| `trie_get(lookup)` | `self.trie.trie_get(k)` over `ctx.lookup_keys` |
| `trie_get(null)` | `self.trie.trie_get(k)` over `ctx.lookup_keys_null` |
| `get(lookup)` | `self.trie.get(k)` over `ctx.lookup_keys` |
| `get(null)` | `self.trie.get(k)` over `ctx.lookup_keys_null` |
| `get(truncated)` | `self.trie.get(k)` over `ctx.fl_lookup_keys` |
| `get_unchecked(hit)` | `unsafe { self.trie.get_unchecked(k) }` over `ctx.hit_keys` |

### Iteration arms

| Spec | Behavior |
|------|----------|
| `trie_callback` | `trie_iter_fwd(|k,v| { black_box(k); black_box(v); })` — callback-based |
| `dyn_callback` | `iter_fwd(&mut |k,v| { … })` — dynamic callback |
| `iter_kv` | Manual `iter()` / `iter_last()` cursor with `current()` + `next()` / `prev()` |
| `iter_kv_no_current` | Same but skips the initial `current()` call |
| `none` | No method generated (contestant supports forward but not reverse, etc.) |

## `NonZeroBytes`

A wrapper `Vec<u8>` guaranteed to contain no `0x00`.  Null-terminator tries
(`BitTrie`, `PolyTrie`) use this as their native key type so they only run in
modes that never produce nulls.  This fixes an earlier silent-drop bug where
`BitTrie` was mis-declared and `insert` quietly discarded null-containing keys.

## `CTreeKey` adapter (tiny_btree.rs)

CTree has two internal stored-key forms: `Box<[u8]>` (variable-length) and `u64`
(fixed-width).  `CTreeKey` is a bench-side trait that maps the harness key `K`
to CTree’s stored form:

```rust
trait CTreeKey: Clone + Ord + 'static {
    type Stored: StoredKey + Default;
    fn into_stored(&self) -> Self::Stored;
    fn as_needle(&self) -> &<Self::Stored as StoredKey>::Needle;
}
```

`CTreeBenchGen<K, const OPT: bool>` is generic over this adapter, producing four
type aliases: `CTreeBench` (`Vec<u8>`, no opt), `CTreeOptBench`,
`CTreeFixedBench` (`u64`, no opt), `CTreeFixedOptBench`.

## Key type → Contestant mapping

| Native key | Contestants | Supported modes |
|------------|-------------|-----------------|
| `Vec<u8>` | NibbleTrie, DynTrie, FixedLen, StackedTrie, CTree, std structures | Random, Sequential, Words, Lines |
| `NonZeroBytes` | BitTrie, PolyTrie | Sequential, Words, Lines (no null bytes) |
| `u64` | CTreeFixed, BTreeMapU64, HashMapU64, SortedVecU64, LinkedListU64 | RandomU64, SeqU64 |
