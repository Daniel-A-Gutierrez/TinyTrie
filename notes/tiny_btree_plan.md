# Key Search Trait Design for DualTree

## Context

`tiny_btree.rs` has a `TreeKey` trait (stub) and `KeyNode<K, PTR, N>` with `[MaybeUninit<K>; N]`. We want two key strategies:

1. **Fixed-len keys** (`FixedLenKey`) — `Copy` types, SIMD broadcast comparison
2. **Variable-len keys** (`VarLenKey<K>`) — sequences of `K: FixedLenKey` chunks, heap-owned via `Box<[K]>`

No inlining/prefix optimization for variable-length keys yet. That's a future step.

## Trait Definitions

```rust
/// Fixed-size keys that can be compared with SIMD broadcast.
/// SAFETY: KEY_SIZE must match size_of::<Self>().
pub unsafe trait FixedLenKey: Copy + Eq + Ord + Sized {
    const KEY_SIZE: usize;

    /// Find insertion index for `needle` in `haystack[..len]`.
    /// Returns the first index where `haystack[i] >= needle`, or `len`.
    fn find_position(needle: &Self, haystack: &[Self], len: u8) -> u8 {
        let n = len as usize;
        for i in 0..n {
            if haystack[i] >= *needle { return i as u8; }
        }
        n as u8
    }
}

/// Variable-length key: a sequence of `K: FixedLenKey` elements.
/// `VarLenKey<u8>` = byte string, `VarLenKey<u32>` = u32 word sequence, etc.
pub trait VarLenKey<K: FixedLenKey>: Eq + Ord + Sized {
    fn as_chunks(&self) -> &[K];
    fn chunk_len(&self) -> usize { self.as_chunks().len() }
}
```

## SIMD `find_position` for FixedLenKey

Broadcast needle, compare N-at-a-time:

```rust
unsafe impl FixedLenKey for u64 {
    const KEY_SIZE: usize = 8;
    fn find_position(needle: &Self, haystack: &[Self], len: u8) -> u8 {
        let n = len as usize;
        let broadcast = Simd::<u64, 4>::splat(*needle);
        let mut i = 0;
        while i + 4 <= n {
            let chunk = Simd::<u64, 4>::from_slice(&haystack[i..i + 4]);
            let mask = chunk.simd_ge(broadcast);
            if mask.any() {
                return (i + mask.first_set().unwrap()) as u8;
            }
            i += 4;
        }
        while i < n {
            if haystack[i] >= *needle { return i as u8; }
            i += 1;
        }
        n as u8
    }
}
```

Similar for `u8` (16-wide), `u16` (8-wide), `u32` (8-wide).

## Variable-length key comparison

For `VarLenKey<K>`, leaf nodes store `Box<[K]>` per key. Search is binary search with `K`-by-`K` comparison (using `K::cmp`). Future optimization: inline prefix in `InlineKey<K, P>`, but not now.

## Node types (updated from current code)

### Fixed-len (existing `KeyNode` / `LeafNode`, bounds changed)

```rust
struct KeyNode<K, PTR: TrieIndex, const N: usize>
where K: FixedLenKey, [(); N + 1]:
{
    len: u8,
    keys: [MaybeUninit<K>; N],
    ptrs: [Option<NonZero<PTR>>; N + 1],
}

struct LeafNode<K, V, const N: usize>
where K: FixedLenKey, V: Sized, [(); N]:
{
    len: u8,
    keys: [MaybeUninit<K>; N],
    values: [MaybeUninit<V>; N],
}
```

### Variable-len (new)

```rust
struct VarKeyNode<K, PTR: TrieIndex, const N: usize>
where K: FixedLenKey, [(); N + 1]:
{
    len: u8,
    keys: [MaybeUninit<Box<[K]>>; N],   // heap-owned variable-length keys
    ptrs: [Option<NonZero<PTR>>; N + 1],
}

struct VarLeafNode<K, V, const N: usize>
where K: FixedLenKey, V: Sized, [(); N]:
{
    len: u8,
    keys: [MaybeUninit<Box<[K]>>; N],   // heap-owned variable-length keys
    values: [MaybeUninit<V>; N],
}
```

### Tree types

```rust
/// B+ tree for fixed-size keys (SIMD leaf search)
struct DualTree<K, V, PTR, const N: usize>
where K: FixedLenKey, PTR: TrieIndex, V: Sized, [(); N + 1]:
{
    inodes: Vec<KeyNode<K, PTR, N>>,
    leaves: Vec<LeafNode<K, V, N>>,
    len: usize,
}

/// B+ tree for variable-length keys (binary search with K-chunk comparison)
struct VarDualTree<K, V, PTR, const N: usize>
where K: FixedLenKey, PTR: TrieIndex, V: Sized, [(); N + 1]:
{
    inodes: Vec<VarKeyNode<K, PTR, N>>,
    leaves: Vec<VarLeafNode<K, V, N>>,
    len: usize,
}
```

## Implementation Steps

### Step 1: Define traits
- Replace `TreeKey` with `FixedLenKey` and `VarLenKey<K>`
- Update `KeyNode` bound from `K: TreeKey` to `K: FixedLenKey`

### Step 2: `FixedLenKey` impls
- `u8`, `u16`, `u32`, `u64` with SIMD `find_position`
- Uses `portable_simd` (already a feature flag)

### Step 3: `VarLenKey<u8>` impl
- For `Vec<u8>` and `[u8]`: `as_chunks()` returns `self` as `&[u8]`

### Step 4: Wire `find_position` into `KeyNode::find_position`
- Delegate to `K::find_position(needle, keys, len)`

### Step 5: Add `VarKeyNode` and `VarLeafNode` structs
- Simple heap-ownership via `Box<[K]>`, no inline prefix for now

### Step 6: `VarDualTree` skeleton
- `new()`, `insert()`, `get()` — B+ tree split/merge logic
- `Drop` impl frees `Box<[K]>` keys

## Verification

1. `cargo check` — trait bounds and const generics compile
2. `cargo test` — existing tests pass
3. Unit test: `u64` SIMD `find_position` correctness vs scalar
4. Unit test: `VarLenKey<u8>` comparison via `Ord` on `Box<[u8]>`

## Future: InlineKey prefix optimization

When we add inlining back, `InlineKey<K, P>` will store the first P chunks of the key inline:

```rust
#[derive(Copy, Clone)]
pub struct InlineKey<K: FixedLenKey, const P: usize = 8> {
    prefix: [K; P],    // first P chunks inline (zero-padded if shorter)
    len: usize,         // total chunk count
    ptr: *const K,      // pointer into Box<[K]> owned by key_store (null if len <= P)
}
```

Comparison: SIMD-compare prefixes first. Only dereference `ptr` on prefix match.
Keys owned by `Vec<Box<[K]>>` in `VarDualTree` (or `key_store` field).
Tunable: `P` is a const generic — set per tree instantiation.