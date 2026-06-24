# Preview Key Plan for CTree (v2)

## Corrected Generic Signature

```rust
pub struct CTree<K, V, PTR, const N: usize, const NP1: usize, P = K>
where
    K: TreeKey,
    P: FixedLenKey,
    K: Preview<P>,
```

`P` defaults to `K`. When `K: FixedLenKey`, the default is valid and the preview
is identity — no extra storage, no extra indirection. When `K` is variable-length
(e.g. `Vec<u8>`), `P = K` fails the `FixedLenKey` bound, forcing the user to
pick a preview size explicitly: `CTree<Vec<u8>, V, _, _, _, u64>`.

No wrapper types, no `WithPreview`. Just a type parameter with a default.

*(Rust syntax note: default type params must be rightmost, so the actual
signature is `CTree<K, V, PTR, N, NP1, P = K>`. The conceptual ordering
`Preview=Key` is preserved.)*

## Trait Architecture

### `TreeKey` (public)

Maps the user's type to its internal stored form and lookup needle. No preview
here — preview is parameterized separately.

```rust
pub trait TreeKey: Ord + Clone {
    /// Internal stored form: identity for fixed keys, `Box<[u8]>` for varlen.
    type Stored;

    /// Borrowed needle for lookups.
    type Needle: ?Sized;

    /// Consume into stored form.
    fn into_stored(self) -> Self::Stored;

    /// Borrow self as lookup needle.
    fn as_needle(&self) -> &Self::Needle;
}
```

### `FixedLenKey` (public)

Extends `TreeKey` with SIMD search. Auto-impls `TreeKey` as identity:

```rust
pub trait FixedLenKey: TreeKey<Stored = Self, Needle = Self> {
    fn simd_find_position(needle: &Self, haystack: &[Self]) -> usize;
    fn simd_find_upper_bound(needle: &Self, haystack: &[Self]) -> usize;
}

impl<T: FixedLenKey> TreeKey for T {
    type Stored = T;
    type Needle = T;
    fn into_stored(self) -> T { self }
    fn as_needle(&self) -> &T { self }
}
```

### `Preview<P>` (public)

Computes the SIMD-able preview of type `P`. Separate from `TreeKey` so a single
`K` can have multiple preview sizes.

```rust
pub trait Preview<P: FixedLenKey> {
    fn preview(&self) -> P;
}

// Auto-impl for fixed keys: preview is identity
impl<T: FixedLenKey> Preview<T> for T {
    fn preview(&self) -> T { *self }
}
```

### Impls for byte containers

```rust
// Stored form
impl TreeKey for Vec<u8> {
    type Stored = Box<[u8]>;
    type Needle = [u8];
    fn into_stored(self) -> Box<[u8]> { self.into_boxed_slice() }
    fn as_needle(&self) -> &[u8] { self }
}

// Same for Box<[u8]> and &[u8] (appropriate into_stored for each)

// Previews — multiple sizes for the same key type
impl Preview<u8>  for Vec<u8> { fn preview(&self) -> u8  { ... } }
impl Preview<u16> for Vec<u8> { fn preview(&self) -> u16 { ... } }
impl Preview<u32> for Vec<u8> { fn preview(&self) -> u32 { ... } }
impl Preview<u64> for Vec<u8> { fn preview(&self) -> u64 { ... } }
```

## Preview Encoding

Right-pad with zeros to the preview width, big-endian interpretation. Preserves
lexicographic ordering:

```rust
fn preview_bytes(input: &[u8], out: &mut [u8]) {
    let n = input.len().min(out.len());
    out[..n].copy_from_slice(&input[..n]);
    // remaining bytes already zero from mut ref init
}

// u64 preview for &[u8]
fn preview_u64(input: &[u8]) -> u64 {
    let mut buf = [0u8; 8];
    let n = input.len().min(8);
    buf[..n].copy_from_slice(&input[..n]);
    u64::from_be_bytes(buf)
}
```

For multi-byte element types (e.g. `Box<[u16]>` previewing to `u64`), copy element
bytes in order, pad with zeros. The invariant is: lexicographic order of the
input sequence matches numeric order of the preview.

## Collision Invariant

```
preview(a) < preview(b)  =>  a < b       (ordering preserved)
preview(a) == preview(b)   does not imply a == b    (collision)
```

This means SIMD lower-bound on previews gives a **conservative** position: all
entries strictly before it are `< needle`. Entries at/after it might be `>=`
needle, and need full comparison.

For random data with `u64` previews and `N=4`, expected collision count is
negligible. For pathological data (shared long prefix), collisions cluster and
we fall back to scalar comparison within the cluster.

## Node Layout

Two internal node types. Selection is at the type level via `NodeStorage` trait.

### Fixed keys (`K == P`)

Unchanged from today. Preview array *is* the key array.

```rust
struct FixedNode<K: FixedLenKey, V, const N: usize> {
    keys: [K; N],
    values: [V; N],
    len: u8,
}
```

Search uses `K::simd_find_position` directly on `keys`.

### Variable-length keys (`K != P`)

Two arrays: inline previews, indirect full keys.

```rust
struct VarLenNode<P: FixedLenKey, SK: StoredKey, V, const N: usize> {
    previews: [P; N],           // SIMD target
    keys: [SK; N],             // full stored keys
    values: [V; N],
    len: u8,
}
```

Search:
1. SIMD on `previews` to get conservative lower bound.
2. Linear scan within the equal-preview cluster, using `StoredKey::cmp_key`.

## TreeNode Trait

The `TreeNode` trait abstracts both node layouts behind a uniform search interface.
`CTree` holds nodes via `Box<[TreeNode<K, V, P, N>]>` (or arena indices), dispatching
to `FixedNode` or `VarLenNode` at the type level.

### Trait Definition

```rust
trait TreeNode<K, V, P, const N: usize>
where
    K: TreeKey,
    P: FixedLenKey,
    K: Preview<P>,
{
    /// Number of live entries.
    fn len(&self) -> usize;

    /// Reference to the preview at position `i`.
    fn preview_at(&self, i: usize) -> &P;

    /// Reference to the full stored key at position `i`.
    fn key_at(&self, i: usize) -> &K::Stored;

    /// Reference to the value at position `i`.
    fn value_at(&self, i: usize) -> &V;
    fn value_at_mut(&mut self, i: usize) -> &mut V;

    /// Insert a new entry at position `pos`, shifting existing entries right.
    /// Caller guarantees `pos <= len < N`.
    fn insert_at(&mut self, pos: usize, preview: P, key: K::Stored, value: V);

    /// Remove entry at position `pos`, shifting entries left.
    fn remove_at(&mut self, pos: usize) -> (K::Stored, V);

    /// Slice of previews for SIMD search.
    fn preview_slice(&self) -> &[P];

    /// Find lower bound (first `>= needle`) using preview SIMD + fallback.
    fn find_position(&self, needle: &K) -> usize;

    /// Find upper bound (first `> needle`) using preview SIMD + fallback.
    fn find_upper_bound(&self, needle: &K) -> usize;
}
```

### FixedNode Implementation

```rust
struct FixedNode<K: FixedLenKey, V, const N: usize> {
    keys: [MaybeUninit<K>; N],
    values: [MaybeUninit<V>; N],
    len: u8,
}

impl<K: FixedLenKey, V, const N: usize> TreeNode<K, V, K, N> for FixedNode<K, V, N> {
    fn len(&self) -> usize { self.len as usize }
    fn preview_at(&self, i: usize) -> &K { unsafe { self.keys[i].assume_init_ref() } }
    fn key_at(&self, i: usize) -> &K { unsafe { self.keys[i].assume_init_ref() } }
    fn value_at(&self, i: usize) -> &V { unsafe { self.values[i].assume_init_ref() } }
    fn value_at_mut(&mut self, i: usize) -> &mut V { unsafe { self.values[i].assume_init_mut() } }

    fn preview_slice(&self) -> &[K] {
        // SAFETY: len entries initialized, rest are MaybeUninit::uninit()
        unsafe { std::slice::from_raw_parts(self.keys.as_ptr() as *const K, self.len as usize) }
    }

    fn find_position(&self, needle: &K) -> usize {
        K::simd_find_position(needle, self.preview_slice())
    }

    fn find_upper_bound(&self, needle: &K) -> usize {
        K::simd_find_upper_bound(needle, self.preview_slice())
    }

    // insert_at, remove_at: shift MaybeUninit arrays, same as today
}
```

`FixedNode` implements `TreeNode<K, V, K, N>` — preview type `P = K`. No extra
storage, `preview_at` and `key_at` alias the same memory.

### VarLenNode Implementation

```rust
struct VarLenNode<P: FixedLenKey, SK: StoredKey, V, const N: usize> {
    previews: [MaybeUninit<P>; N],
    keys: [MaybeUninit<SK>; N],
    values: [MaybeUninit<V>; N],
    len: u8,
}

impl<K, V, P, const N: usize> TreeNode<K, V, P, N> for VarLenNode<P, K::Stored, V, N>
where
    K: TreeKey + Preview<P>,
    P: FixedLenKey,
{
    fn len(&self) -> usize { self.len as usize }
    fn preview_at(&self, i: usize) -> &P { unsafe { self.previews[i].assume_init_ref() } }
    fn key_at(&self, i: usize) -> &K::Stored { unsafe { self.keys[i].assume_init_ref() } }
    fn value_at(&self, i: usize) -> &V { unsafe { self.values[i].assume_init_ref() } }
    fn value_at_mut(&mut self, i: usize) -> &mut V { unsafe { self.values[i].assume_init_mut() } }

    fn preview_slice(&self) -> &[P] {
        unsafe { std::slice::from_raw_parts(self.previews.as_ptr() as *const P, self.len as usize) }
    }

    fn find_position(&self, needle: &K) -> usize {
        let p = needle.preview();
        let mut pos = P::simd_find_position(&p, self.preview_slice());
        while pos < self.len()
            && *self.preview_at(pos) == p
            && StoredKey::cmp_key(self.key_at(pos), needle.as_needle()) == Less
        {
            pos += 1;
        }
        pos
    }

    fn find_upper_bound(&self, needle: &K) -> usize {
        let p = needle.preview();
        let start = P::simd_find_position(&p, self.preview_slice());
        let mut pos = start;
        while pos < self.len()
            && *self.preview_at(pos) == p
            && StoredKey::cmp_key(self.key_at(pos), needle.as_needle()) != Greater
        {
            pos += 1;
        }
        pos
    }

    // insert_at: shift previews, keys, values separately
    // remove_at: same
}
```

`VarLenNode<P, K::Stored, V, N>` implements `TreeNode<K, V, P, N>`. The preview
array is separate from the key array — SIMD targets previews, fallback uses
`StoredKey::cmp_key` on `K::Stored`.

### CTree Node Storage

`CTree` stores nodes in an arena or `Vec`. The concrete node type is selected at
compile time via an associated type on a `NodeKind` marker trait:

```rust
// Marker: which node layout to use
sealed trait NodeKind<K, V, P, const N: usize> {
    type Node: TreeNode<K, V, P, N>;
}

// Fixed keys: K == P, both FixedLenKey
impl<K, V, const N: usize> NodeKind<K, V, K, N> for ()
where
    K: FixedLenKey,
{
    type Node = FixedNode<K, V, N>;
}

// Varlen keys: K != P, K previews to P
impl<K, V, P, const N: usize> NodeKind<K, V, P, N> for ()
where
    K: TreeKey + Preview<P>,
    P: FixedLenKey,
    K::Stored: StoredKey,
{
    type Node = VarLenNode<P, K::Stored, V, N>;
}
```

`CTree` then uses `<() as NodeKind<K, V, P, N>>::Node` internally:

```rust
pub struct CTree<K, V, PTR, const N: usize, const NP1: usize, P = K>
where
    K: TreeKey + Preview<P>,
    P: FixedLenKey,
    (): NodeKind<K, V, P, N>,
{
    nodes: Vec<<() as NodeKind<K, V, P, N>>::Node>,
    // ... other fields
}
```

**Why a marker trait instead of just `enum Node { Fixed(FixedNode), VarLen(VarLenNode) }`?**

An enum would require every node to reserve space for both variants — the whole
point of splitting was to avoid the preview-array overhead for fixed keys. The
trait approach means each `CTree<K, V, ..., P>` instantiates exactly one node
layout. No runtime dispatch, no wasted space.

## Search Algorithm

### `find_position`

```rust
fn find_position<K, P>(needle: &K, node: &impl NodeStorage<K, P>) -> usize
where
    K: TreeKey + Preview<P>,
    P: FixedLenKey,
{
    let p = needle.preview();

    // Step 1: SIMD lower bound on previews
    let mut pos = simd_lower_bound(p, node.preview_slice());

    // Step 2: Fallback within equal-preview cluster
    while pos < node.len()
        && node.preview_at(pos) == p
        && StoredKey::cmp_key(&node.key_at(pos), needle.as_needle()) == Less
    {
        pos += 1;
    }

    pos
}
```

### `find_upper_bound`

```rust
fn find_upper_bound<K, P>(needle: &K, node: &impl NodeStorage<K, P>) -> usize
where
    K: TreeKey + Preview<P>,
    P: FixedLenKey,
{
    let p = needle.preview();

    // Step 1: SIMD upper bound on previews (first preview > p)
    let mut pos = simd_upper_bound(p, node.preview_slice());

    // Step 2: But some entries before pos with preview == p might also be > needle.
    //    Walk backward to find the start of the equal-preview cluster,
    //    then scan forward for first full key > needle.
    let start = simd_lower_bound(p, node.preview_slice());
    let mut pos = start;
    while pos < node.len()
        && node.preview_at(pos) == p
        && StoredKey::cmp_key(&node.key_at(pos), needle.as_needle()) != Greater
    {
        pos += 1;
    }

    pos
}
```

For `N=4`, the equal-preview cluster is typically 0 or 1 entries. The fallback
is a single scalar comparison.

## StoredKey (internal)

Stripped to comparison only. Search logic lives in the node, not the trait.

```rust
trait StoredKey {
    type Needle: ?Sized;
    fn cmp_key(stored: &Self, needle: &Self::Needle) -> Ordering;
    fn eq_key(stored: &Self, needle: &Self::Needle) -> bool;
}

impl StoredKey for Box<[u8]> {
    type Needle = [u8];
    fn cmp_key(stored: &Self, needle: &[u8]) -> Ordering { stored.as_ref().cmp(needle) }
    fn eq_key(stored: &Self, needle: &[u8]) -> bool { stored.as_ref() == needle }
}
```

## CTree Public API

```rust
impl<K, V, PTR, const N: usize, const NP1: usize, P>
    CTree<K, V, PTR, N, NP1, P>
where
    K: TreeKey,
    P: FixedLenKey,
    K: Preview<P>,
{
    pub fn new() -> Self { ... }
    pub fn insert(&mut self, key: K, value: V) -> Result<(), (K, V)> { ... }
    pub fn get<Q>(&self, key: &Q) -> Option<&V>
    where
        Q: Preview<P>,              // or just require &K ?
        Q: ?Sized,
    { ... }
    pub fn cursor_at(&self, key: &K) -> Cursor<...> { ... }
}
```

`insert` takes `K` (user's type), converts via `into_stored()` internally.
`get` takes a borrowed needle — for fixed keys, `&K`. For varlen, `&[u8]` if
`K = Vec<u8>`.

## Migration Path

1. **Introduce `TreeKey`** trait with `Stored`, `Needle`, `into_stored`, `as_needle`.
2. **Shrink `StoredKey`** to `cmp_key` / `eq_key` only.
3. **Introduce `Preview<P>`** trait. Auto-impl for `FixedLenKey`.
4. **Impl `TreeKey + Preview<u8/u16/u32/u64>`** for `Vec<u8>`, `Box<[u8]>`, etc.
5. **Impl `TreeKey`** for `u64` via `FixedLenKey` auto-impl.
6. **Change `CTree<SK, V, ...>` → `CTree<K, V, ..., P = K>`**.
   - Update all internal methods to use `K::into_stored()`, `K::preview()`.
   - Introduce `NodeStorage` trait, `FixedNode` and `VarLenNode` impls.
7. **Implement preview-aware search** in `VarLenNode`.
8. **Delete `CTreeKey`** bench adapter.
9. **Update bench contestants**:
   - `CTreeBenchGen<K, P, OPT>` becomes `CTree<K, usize, u32, 4, 5, P>` directly.
   - Or keep a thin `CTreeBenchGen<K, P, OPT>` wrapper for `build`/`memory`/etc.

## Bench Simplification

| Before | After |
|--------|-------|
| `CTree<Box<[u8]>, V, ...>` | `CTree<Vec<u8>, V, ..., u64>` |
| `CTree<u64, V, ...>` | `CTree<u64, V, ...>` (unchanged, `P` defaults to `u64`) |
| `CTreeKey` adapter trait | **deleted** |
| `CTreeBenchGen<K, OPT>` with `CTreeKey` bound | `CTreeBenchGen<K, P, OPT>` with `TreeKey + Preview<P>` bound |

The bench no longer needs to know CTree's stored type. It just passes `Vec<u8>`
and CTree handles `into_stored()` internally.

## Future Inlining

Today: `TreeKey for Vec<u8>` sets `Stored = Box<[u8]>`. Tomorrow:

```rust
pub struct InlineKey16 { len: u8, data: [u8; 15] }

impl TreeKey for Vec<u8> {
    type Stored = InlineKey16;   // was Box<[u8]>
    // ...
}
```

`CTree<Vec<u8>, V, ..., u64>` consumers don't change. Only `StoredKey for
InlineKey16` and node layout need updating. The preview type `u64` stays valid.

## Decisions

| Question | Decision |
|----------|----------|
| Aliases for fat signature? | **No.** `CTree<K, V, PTR, N, NP1, P = K>` stays as-is. |
| `get` key type? | **`Q: AsRef<K::Needle>`** — accepts both `&K` and `&K::Needle` (e.g. `&[u8]` for `Vec<u8>`). |
| Node dispatch? | **Two node types via `TreeNode` trait.** Fixed keys use `FixedNode` (no preview array, no waste). Varlen keys use `VarLenNode` (inline previews + indirect keys). |
