# Trait Architecture

Two independent trait systems serve different purposes: **ByteKey** for radix-trie
key genericity, and **CTree's trait chain** for B+ tree SIMD dispatch. The bench
harness has its own layer on top.

---

## ByteKey System (`key_store.rs`)

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
- `NonNullKey` will be required by `PolyTrie<K: NonNullKey>` (not yet applied)
- Other tries (BitTrie, NibTrie, FixedLenNibbleTrie) still use `Vec<u8>` directly;
  can adopt `ByteKey` later

### `NonZeroBytes`

Moved from bench-only (`bench/keygen.rs`) to public API (`key_store.rs`). A `Vec<u8>`
wrapper guaranteed to contain no `0x00`. Constructed via `NonZeroBytes::new(v)` which
returns `None` if `0x00` is present, or `unsafe new_unchecked(v)`.

---

## CTree Trait System (`tiny_btree.rs`)

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

### Composition graph

```
FixedLenKey ──┬──→ TreeKey (blanket: T: FixedLenKey)
               ├──→ Preview<NoPreview> (blanket: T: FixedLenKey)
               ├──→ StoredKey (fixed: K: FixedLenKey)
               ├──→ StoredKey (variable: K: FixedLenKey → Box<[K]>)
               │
               │   ┌── Fixed path ──────────────────────────────────┐
               ├──→ SearchStrategy<NoPreview> (K: FixedLenKey)        │ SIMD on key array
               │   │   P = NoPreview, previews = 0 bytes              │ directly
               │   └──────────────────────────────────────────────────┘
               │
TreeKey ───────┬──→ Preview<P> (specific: Vec<u8>, Box<[u8]>, [u8])
               │   P ∈ {u8, u16, u32, u64}
               │
               └──→ SearchStrategy<P> (K: TreeKey + Preview<P>, P: FixedLenKey,
                     K::Stored: StoredKey, K::Needle: Preview<P>)
                   │   ┌── Variable path ──────────────────────────────┐
                   │   │   P: FixedLenKey, preview array present     │ SIMD preview
                   │   │   then scalar StoredKey::cmp_key on collision │ then fallback
                   │   └──────────────────────────────────────────────┘
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

## Bench Harness Layer (`bench/mod.rs`)

### `Benchable<K>`

Per-contestant trait, generic over native key type `K`. Returns `Option` so
contestants can skip unsupported test modes.

### `Contestant` + `KeyVariant`

```rust
#[derive(Clone, Copy)]
enum KeyVariant { Bytes, NonZero, U64 }

struct Contestant {
    name: &'static str,
    max_size: Option<usize>,
    bytes: Option<Box<dyn Benchable<Vec<u8>>>>>,
    nonzero: Option<Box<dyn Benchable<NonZeroBytes>>>,
    u64: Option<Box<dyn Benchable<u64>>>,
}
```

Each contestant carries up to three `Benchable<K>` variants (one per key type).
`variant_for(mode)` selects the appropriate one for the current key mode — e.g.
`CTree` carries both `bytes` (Vec<u8> variable-length path) and `u64` (fixed SIMD
path). Multi-key contestants like `BTreeMap`, `HashMap`, etc. merge their former
`*U64` aliases into one entry.

### `BenchCtx<K>`

Shared lookup sets built once per size: `lookup_keys` (hit+miss), `hit_keys`
(unchecked), `fl_lookup_keys` (truncated), `lookup_keys_null` (null-terminated).

### `CTreeKey` adapter

Bench-side trait mapping harness key `K` to CTree's stored form. Bridges
`Benchable<K>` to CTree's `TreeKey`/`StoredKey` system. Now superseded by
`CTreeBenchKey` which uses the newer `Preview<P>` trait system.

---

## Relationship between the two systems

The **ByteKey** and **CTree trait** systems are **independent**:

- `ByteKey` provides `as_bytes()`/`from_bytes()` for radix-trie key abstraction
  (insert takes `K`, internal comparison is `&[u8]`)
- CTree's `TreeKey`/`StoredKey`/`Preview`/`SearchStrategy` provides B+ tree
  node search dispatch (insert takes `K: TreeKey`, internal search uses
  `FixedLenKey` SIMD or `Preview<P>` + scalar fallback)

Potential future unification: `ByteKey` types could implement `TreeKey` (e.g.,
`Vec<u8>: TreeKey<Stored = Box<[u8]>>` already exists), but currently the two
systems serve different purposes and don't interact.