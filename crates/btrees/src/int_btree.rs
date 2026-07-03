use std::cmp::Ordering;
use std::num::{NonZero, ZeroablePrimitive};
use std::simd::cmp::SimdPartialOrd;
use std::simd::Simd;

use crate::tiny_array::TinyArray;

// ---------------------------------------------------------------------------
// TrieIndex
// ---------------------------------------------------------------------------

/// Index type for arena-based node pointers.
pub trait TrieIndex:
    Copy + Clone + Default + PartialEq + Eq + std::fmt::Debug + 'static + ZeroablePrimitive
{
    fn as_usize(self) -> usize;
    fn max_value() -> usize;
    fn from_usize(n: usize) -> Self;
}

// ---------------------------------------------------------------------------
// FixedLenKey — SIMD search
// ---------------------------------------------------------------------------

/// Fixed-size keys that can be compared with SIMD broadcast.
pub trait FixedLenKey: Copy + Eq + Ord + Sized {
    fn find_position(needle: &Self, haystack: &[Self]) -> usize;
    fn find_upper_bound(needle: &Self, haystack: &[Self]) -> usize;
}

macro_rules! impl_fixed_len_key_simd {
    ($ty:ty, $lanes:expr) => {
        impl FixedLenKey for $ty {
            #[inline]
            fn find_position(needle: &Self, haystack: &[Self]) -> usize {
                let len = haystack.len();
                if len == 0 { return 0; }
                let broadcast = Simd::<$ty, $lanes>::splat(*needle);
                let mut i = 0;
                while i + $lanes <= len {
                    let chunk = Simd::<$ty, $lanes>::from_slice(&haystack[i..i + $lanes]);
                    let ge = chunk.simd_ge(broadcast);
                    let mask = ge.to_bitmask() as u32;
                    if mask != 0 {
                        return i + mask.trailing_zeros() as usize;
                    }
                    i += $lanes;
                }
                while i < len {
                    if haystack[i] >= *needle { return i; }
                    i += 1;
                }
                len
            }
            #[inline]
            fn find_upper_bound(needle: &Self, haystack: &[Self]) -> usize {
                let len = haystack.len();
                if len == 0 { return 0; }
                let broadcast = Simd::<$ty, $lanes>::splat(*needle);
                let mut i = 0;
                while i + $lanes <= len {
                    let chunk = Simd::<$ty, $lanes>::from_slice(&haystack[i..i + $lanes]);
                    let gt = chunk.simd_gt(broadcast);
                    let mask = gt.to_bitmask() as u32;
                    if mask != 0 {
                        return i + mask.trailing_zeros() as usize;
                    }
                    i += $lanes;
                }
                while i < len {
                    if haystack[i] > *needle { return i; }
                    i += 1;
                }
                len
            }
        }
    };
}

impl_fixed_len_key_simd!(u8, 16);
impl_fixed_len_key_simd!(u16, 8);
impl_fixed_len_key_simd!(u32, 4);
impl_fixed_len_key_simd!(u64, 4);
impl_fixed_len_key_simd!(i8, 16);
impl_fixed_len_key_simd!(i16, 8);
impl_fixed_len_key_simd!(i32, 4);
impl_fixed_len_key_simd!(i64, 4);

// ---------------------------------------------------------------------------
// KeyRef — inline or buffer-offset stored key for varlen byte strings
// ---------------------------------------------------------------------------

/// Stored form for variable-length keys. Short keys (≤14 bytes) are inlined
/// directly; longer keys reference `CTree::key_buf` via offset + length.
///
/// The derived `Ord` provides a total order but is **not** semantically
/// meaningful for `Buf` variants (it compares offset/length, not key content).
/// All actual key comparison goes through `StoredKey`, which reads from `key_buf`.
#[derive(Clone, Debug)]
pub enum KeyRef {
    /// Key data stored inline — no indirection.
    Inline(TinyArray<u8, 14>),
    /// Key data in the shared `key_buf` at byte offset `start` with length `len`.
    Buf { start: u64, len: u32 },
}

impl PartialEq for KeyRef {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (KeyRef::Inline(a), KeyRef::Inline(b)) => a.as_slice() == b.as_slice(),
            (KeyRef::Buf { start: s1, len: l1 }, KeyRef::Buf { start: s2, len: l2 }) => {
                s1 == s2 && l1 == l2
            }
            _ => false,
        }
    }
}

impl Eq for KeyRef {}

impl PartialOrd for KeyRef {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for KeyRef {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        match (self, other) {
            // Inline vs Inline: compare bytes directly.
            (KeyRef::Inline(a), KeyRef::Inline(b)) => cmp_slice_scalar(a.as_slice(), b.as_slice()),
            // Inline always orders before Buf (discriminant ordering).
            (KeyRef::Inline(_), KeyRef::Buf { .. }) => std::cmp::Ordering::Less,
            (KeyRef::Buf { .. }, KeyRef::Inline(_)) => std::cmp::Ordering::Greater,
            // Buf vs Buf: offset ordering (not semantically meaningful;
            // real comparison goes through StoredKey::cmp_stored with buf).
            (KeyRef::Buf { start: s1, len: l1 }, KeyRef::Buf { start: s2, len: l2 }) => {
                (s1, l1).cmp(&(s2, l2))
            }
        }
    }
}

impl KeyRef {
    /// Length of the key in bytes.
    pub fn key_len(&self) -> usize {
        match self {
            KeyRef::Inline(arr) => arr.len(),
            KeyRef::Buf { len, .. } => *len as usize,
        }
    }
}

/// Legacy alias — `BufKey` is now `KeyRef::Buf`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct BufKey {
    pub start: u32,
    pub len: u16,
}

// ---------------------------------------------------------------------------
// TreeKey
// ---------------------------------------------------------------------------

/// Maps a user's key type to the internal stored form and lookup needle.
pub trait TreeKey: Ord + Clone {
    /// Internal stored form: identity for fixed keys, `KeyRef` for varlen.
    type Stored: StoredKey<Needle = Self::Needle>;
    /// Borrowed needle for lookups.
    type Needle: ?Sized;
    /// Consume into stored form. For varlen keys, short keys (≤14 bytes) are inlined
    /// into `KeyRef::Inline`; longer keys append to `buf` and return `KeyRef::Buf`.
    /// For fixed keys, `buf` is unused and the key is returned as-is.
    fn into_stored(self, buf: &mut Vec<u8>) -> Self::Stored;
    /// Borrow self as lookup needle.
    fn as_needle(&self) -> &Self::Needle;
}

/// Auto-impl `TreeKey` for fixed-length keys (identity mapping).
impl<T: FixedLenKey> TreeKey for T {
    type Stored = T;
    type Needle = T;
    fn into_stored(self, _buf: &mut Vec<u8>) -> T { self }
    fn as_needle(&self) -> &T { self }
}

/// Threshold: keys up to this many bytes are stored inline in `KeyRef::Inline`.
const INLINE_KEY_MAX: usize = 14;

impl TreeKey for Vec<u8> {
    type Stored = KeyRef;
    type Needle = [u8];
    fn into_stored(self, buf: &mut Vec<u8>) -> KeyRef {
        if self.len() <= INLINE_KEY_MAX {
            let mut arr = TinyArray::new();
            for &b in &self {
                arr.push(b);
            }
            KeyRef::Inline(arr)
        } else {
            let start = buf.len() as u64;
            buf.extend_from_slice(&self);
            KeyRef::Buf { start, len: self.len() as u32 }
        }
    }
    fn as_needle(&self) -> &[u8] { self }
}

impl TreeKey for Box<[u8]> {
    type Stored = KeyRef;
    type Needle = [u8];
    fn into_stored(self, buf: &mut Vec<u8>) -> KeyRef {
        if self.len() <= INLINE_KEY_MAX {
            let mut arr = TinyArray::new();
            for &b in &*self {
                arr.push(b);
            }
            KeyRef::Inline(arr)
        } else {
            let start = buf.len() as u64;
            buf.extend_from_slice(&self);
            KeyRef::Buf { start, len: self.len() as u32 }
        }
    }
    fn as_needle(&self) -> &[u8] { self }
}

// ---------------------------------------------------------------------------
// SearchStrategy — dispatch fixed vs varlen search
// ---------------------------------------------------------------------------

/// Static dispatch for node search: fixed keys search the key array directly
/// via SIMD; variable keys use linear scan through the key buffer.
pub trait SearchStrategy: TreeKey {
    fn find_position(needle: &Self::Needle, keys: &[Self::Stored], buf: &[u8]) -> usize;
    fn find_upper_bound(needle: &Self::Needle, keys: &[Self::Stored], buf: &[u8]) -> usize;
}

// Fixed keys: search keys directly via SIMD (ignore buf).
impl<K: FixedLenKey> SearchStrategy for K {
    fn find_position(needle: &K, keys: &[K], _buf: &[u8]) -> usize {
        K::find_position(needle, keys)
    }
    fn find_upper_bound(needle: &K, keys: &[K], _buf: &[u8]) -> usize {
        K::find_upper_bound(needle, keys)
    }
}

// Variable keys (Stored = KeyRef): linear scan through key buffer or inline bytes.
impl SearchStrategy for Vec<u8> {
    fn find_position(needle: &[u8], keys: &[KeyRef], buf: &[u8]) -> usize {
        for (i, k) in keys.iter().enumerate() {
            if StoredKey::cmp_key(k, needle, buf) != Ordering::Less {
                return i;
            }
        }
        keys.len()
    }
    fn find_upper_bound(needle: &[u8], keys: &[KeyRef], buf: &[u8]) -> usize {
        for (i, k) in keys.iter().enumerate() {
            if StoredKey::cmp_key(k, needle, buf) == Ordering::Greater {
                return i;
            }
        }
        keys.len()
    }
}

impl SearchStrategy for Box<[u8]> {
    fn find_position(needle: &[u8], keys: &[KeyRef], buf: &[u8]) -> usize {
        for (i, k) in keys.iter().enumerate() {
            if StoredKey::cmp_key(k, needle, buf) != Ordering::Less {
                return i;
            }
        }
        keys.len()
    }
    fn find_upper_bound(needle: &[u8], keys: &[KeyRef], buf: &[u8]) -> usize {
        for (i, k) in keys.iter().enumerate() {
            if StoredKey::cmp_key(k, needle, buf) == Ordering::Greater {
                return i;
            }
        }
        keys.len()
    }
}

// ---------------------------------------------------------------------------
// StoredKey — sealed internal comparison trait
// ---------------------------------------------------------------------------

mod private {
    /// Marker sealing [`super::StoredKey`] to the canonical key forms:
    /// fixed keys (`K: FixedLenKey`) and variable keys (`KeyRef`).
    pub trait Sealed {}
}

pub trait StoredKey: Clone + private::Sealed
{
    /// Borrowed lookup needle: `K` for fixed, `[u8]` for variable.
    type Needle: ?Sized;
    /// Compare stored key against needle. `buf` is the CTree key buffer (unused for fixed keys).
    fn cmp_key(stored: &Self, needle: &Self::Needle, buf: &[u8]) -> Ordering;
    /// Check equality. `buf` is the CTree key buffer (unused for fixed keys).
    fn eq_key(stored: &Self, needle: &Self::Needle, buf: &[u8]) -> bool;
    /// Compare two stored keys through the buffer.
    fn cmp_stored(a: &Self, b: &Self, buf: &[u8]) -> Ordering;
}

// Fixed form
impl<K: FixedLenKey> private::Sealed for K {}

impl<K: FixedLenKey> StoredKey for K {
    type Needle = K;
    fn cmp_key(stored: &K, needle: &K, _buf: &[u8]) -> Ordering { stored.cmp(needle) }
    fn eq_key(stored: &K, needle: &K, _buf: &[u8]) -> bool { stored == needle }
    fn cmp_stored(a: &K, b: &K, _buf: &[u8]) -> Ordering { a.cmp(b) }
}

// KeyRef form: inline short keys or offset into CTree::key_buf
impl private::Sealed for KeyRef {}

impl StoredKey for KeyRef {
    type Needle = [u8];
    fn cmp_key(stored: &KeyRef, needle: &[u8], buf: &[u8]) -> Ordering {
        match stored {
            KeyRef::Inline(arr) => cmp_slice_scalar(arr.as_slice(), needle),
            KeyRef::Buf { start, len } => {
                let bytes = &buf[*start as usize..][..*len as usize];
                cmp_slice_scalar(bytes, needle)
            }
        }
    }
    fn eq_key(stored: &KeyRef, needle: &[u8], buf: &[u8]) -> bool {
        match stored {
            KeyRef::Inline(arr) => eq_slice_scalar(arr.as_slice(), needle),
            KeyRef::Buf { start, len } => {
                let bytes = &buf[*start as usize..][..*len as usize];
                eq_slice_scalar(bytes, needle)
            }
        }
    }
    fn cmp_stored(a: &KeyRef, b: &KeyRef, buf: &[u8]) -> Ordering {
        let a_bytes: &[u8] = match a {
            KeyRef::Inline(arr) => arr.as_slice(),
            KeyRef::Buf { start, len } => &buf[*start as usize..][..*len as usize],
        };
        let b_bytes: &[u8] = match b {
            KeyRef::Inline(arr) => arr.as_slice(),
            KeyRef::Buf { start, len } => &buf[*start as usize..][..*len as usize],
        };
        cmp_slice_scalar(a_bytes, b_bytes)
    }
}

// Scalar comparison helpers for varlen key comparison.
#[inline]
fn cmp_slice_scalar<K: Ord>(a: &[K], b: &[K]) -> Ordering {
    let n = a.len().min(b.len());
    for i in 0..n {
        match a[i].cmp(&b[i]) {
            Ordering::Equal => {}
            ord => return ord,
        }
    }
    a.len().cmp(&b.len())
}

#[inline]
fn eq_slice_scalar<K: PartialEq>(a: &[K], b: &[K]) -> bool {
    if a.len() != b.len() { return false; }
    for i in 0..a.len() {
        if a[i] != b[i] { return false; }
    }
    true
}

// ---------------------------------------------------------------------------
// Node types — unified over K: TreeKey
// ---------------------------------------------------------------------------

/// Internal (separator) node: keys plus `ptrs` to children.
struct KeyNode<K, PTR, const N: usize, const NP1: usize>
where
    K: TreeKey,
    PTR: TrieIndex,
    [(); N]: ,
    [(); NP1]: ,
{
    keys: TinyArray<K::Stored, N>,
    ptrs: [Option<NonZero<PTR>>; NP1],
}

/// Leaf node. Lives in the `leaves` arena `Vec`; the arena is kept in strict
/// sorted physical order (gaps are zeroed `LeafNode`s with `keys.len() == 0`),
/// so forward/backward cursor navigation scans slot-by-slot skipping gaps — no
/// per-leaf linked-list pointers are needed.
struct LeafNode<K, V, PTR, const N: usize>
where
    K: TreeKey,
    PTR: TrieIndex,
    V: Sized,
    [(); N]: ,
{
    keys: TinyArray<K::Stored, N>,
    values: TinyArray<V, N>,
    // `PTR` is retained in the type parameters even though no link fields use
    // it: `relocate`/`split_leaf` construct `LeafNode::<K, V, PTR, N>` and the
    // arena `Vec` is typed by `PTR`, so keeping it avoids touching every call
    // site. It costs nothing at runtime here.
    _ptr: std::marker::PhantomData<PTR>,
}

// ---------------------------------------------------------------------------
// CTree
// ---------------------------------------------------------------------------

/// B+ tree. `K` is the user's key type.
///   * `CTree<u64, ...>`  — fixed-size keys, SIMD search
///   * `CTree<Vec<u8>, ...>` — variable-length keys, linear scan through buffer
///
/// All tree operations are implemented once, generically over `SearchStrategy`.
pub struct CTree<K, V, PTR, const N: usize, const NP1: usize>
where
    K: TreeKey,
    PTR: TrieIndex,
    V: Sized,
    [(); N]: ,
    [(); NP1]: ,
{
    inodes: Vec<KeyNode<K, PTR, N, NP1>>,
    leaves: Vec<LeafNode<K, V, PTR, N>>,
    /// Contiguous byte buffer for varlen key data. Fixed-key trees leave this empty.
    key_buf: Vec<u8>,
    len: usize,
    /// Count of LIVE leaves (excludes gap sentinels).
    n_leaves: usize,
    /// Number of inode levels. 0 = root is a leaf.
    height: usize,
    /// Index of the root inode in `self.inodes`. Only valid when height >= 1.
    root_inode: usize,
}

pub struct Cursor<'a, K, V, PTR, const N: usize, const NP1: usize>
where
    K: TreeKey,
    PTR: TrieIndex,
    V: Sized,
    [(); N]: ,
    [(); NP1]: ,
{
    tree: &'a CTree<K, V, PTR, N, NP1>,
    leaf_idx: usize,
    position: usize,
}

pub struct CursorMut<'a, K, V, PTR, const N: usize, const NP1: usize>
where
    K: TreeKey,
    PTR: TrieIndex,
    V: Sized,
    [(); N]: ,
    [(); NP1]: ,
{
    tree: &'a mut CTree<K, V, PTR, N, NP1>,
    leaf_idx: usize,
    position: usize,
}

// ---------------------------------------------------------------------------
// KeyNode impl
// ---------------------------------------------------------------------------

#[allow(dead_code)]
impl<K, PTR, const N: usize, const NP1: usize> KeyNode<K, PTR, N, NP1>
where
    K: TreeKey + SearchStrategy,
    K::Stored: StoredKey,
    PTR: TrieIndex,
    [(); N]: ,
    [(); NP1]: ,
{
    const ASSERT_NP1: () = assert!(NP1 == N + 1, "NP1 must equal N + 1");

    fn new() -> Self {
        Self {
            keys: TinyArray::new(),
            ptrs: [None; NP1],
        }
    }

    /// Create a child node from parent's keys/ptrs in `[from..to)`.
    fn from_parent(from: usize, to: usize, parent: &Self) -> Self {
        let mut node = Self::new();
        for i in from..to {
            node.keys.insert_at(i - from, parent.keys.get(i).clone());
        }
        for i in from..=to {
            node.ptrs[i - from] = parent.ptrs[i];
        }
        node
    }

    #[inline]
    fn get(&self, i: usize) -> &<K as TreeKey>::Stored {
        self.keys.get(i)
    }

    #[inline]
    unsafe fn get_unchecked(&self, i: usize) -> &<K as TreeKey>::Stored {
        unsafe { self.keys.get_unchecked(i) }
    }

    #[inline]
    fn get_ptr(&self, i: usize) -> Option<usize> {
        debug_assert!(i <= self.keys.len());
        self.ptrs[i].map(|nz| nz.get().as_usize() - 1)
    }

    #[inline]
    unsafe fn get_ptr_unchecked(&self, i: usize) -> Option<usize> {
        self.ptrs[i].map(|nz| nz.get().as_usize() - 1)
    }

    #[inline]
    fn set_ptr(&mut self, i: usize, idx: usize) {
        self.ptrs[i] = NonZero::new(PTR::from_usize(idx + 1));
    }

    #[inline]
    fn clear_ptr(&mut self, i: usize) {
        self.ptrs[i] = None;
    }

    /// First index where `keys[i] >= needle` (lower bound).
    #[inline]
    fn find_position(&self, needle: &<K as TreeKey>::Needle, buf: &[u8]) -> usize {
        <K as SearchStrategy>::find_position(needle, self.keys.as_slice(), buf)
    }

    /// Find the child pointer index for `needle` in a B+ tree internal node.
    #[inline]
    fn find_child(&self, needle: &<K as TreeKey>::Needle, buf: &[u8]) -> usize {
        <K as SearchStrategy>::find_upper_bound(needle, self.keys.as_slice(), buf)
    }

    #[inline]
    fn adjacent_sibling_ptrs(&self, child_pos: usize) -> (Option<usize>, Option<usize>) {
        let left = if child_pos > 0 { self.get_ptr(child_pos - 1) } else { None };
        let right = if child_pos < self.keys.len() { self.get_ptr(child_pos + 1) } else { None };
        (left, right)
    }

    #[inline]
    fn would_split(&self) -> bool {
        self.keys.is_full()
    }

    #[inline]
    fn would_merge(&self) -> bool {
        self.keys.len() == N / 2
    }

    /// Insert stored key at position `pos`.
    fn insert_key_at(&mut self, pos: usize, stored: <K as TreeKey>::Stored) {
        debug_assert!(!self.would_split());
        let l = self.keys.len();
        if pos < l {
            for i in (pos + 1..=l).rev() {
                self.ptrs[i + 1] = self.ptrs[i];
            }
        }
        self.keys.insert_at(pos, stored);
    }

    /// Insert stored key into this internal node in sorted order.
    fn insert_leaf(&mut self, needle: &<K as TreeKey>::Needle, stored: <K as TreeKey>::Stored, buf: &[u8]) -> usize {
        let pos = self.find_position(needle, buf);
        self.insert_key_at(pos, stored);
        pos
    }

    /// Remove key at `pos` and its right child pointer.
    fn remove(&mut self, pos: usize) -> <K as TreeKey>::Stored {
        let l = self.keys.len();
        let k = self.keys.remove_at(pos);
        if pos + 1 < l {
            for i in pos + 1..l {
                self.ptrs[i] = self.ptrs[i + 1];
            }
        }
        k
    }

    #[inline]
    fn truncate(&mut self, newlen: u8) {
        self.keys.truncate(newlen);
    }
}

// ---------------------------------------------------------------------------
// LeafNode impl
// ---------------------------------------------------------------------------

#[allow(dead_code)]
impl<K, V, PTR, const N: usize> LeafNode<K, V, PTR, N>
where
    K: TreeKey + SearchStrategy,
    K::Stored: StoredKey,
    PTR: TrieIndex,
    V: Sized,
    [(); N]: ,
{
    fn new() -> Self {
        Self {
            keys: TinyArray::new(),
            values: TinyArray::new(),
            _ptr: std::marker::PhantomData,
        }
    }

    #[inline]
    fn find_position(&self, needle: &<K as TreeKey>::Needle, buf: &[u8]) -> usize {
        <K as SearchStrategy>::find_position(needle, self.keys.as_slice(), buf)
    }

    #[inline]
    fn would_split(&self) -> bool {
        self.keys.is_full()
    }

    /// Insert key-value at position `pos`.
    fn insert(&mut self, pos: usize, stored: <K as TreeKey>::Stored, value: V) {
        self.keys.insert_at(pos, stored);
        self.values.insert_at(pos, value);
    }

    fn remove(&mut self, pos: usize) -> (<K as TreeKey>::Stored, V) {
        let k = self.keys.remove_at(pos);
        let v = self.values.remove_at(pos);
        (k, v)
    }

    fn truncate(&mut self, newlen: u8) {
        self.keys.truncate(newlen);
        self.values.truncate(newlen);
    }
}

// ---------------------------------------------------------------------------
// TrieIndex impls
// ---------------------------------------------------------------------------

macro_rules! impl_trie_index {
    ($($ty:ty),* $(,)?) => {
        $(
            impl TrieIndex for $ty {
                #[inline]
                fn as_usize(self) -> usize { self as usize }
                #[inline]
                fn max_value() -> usize { <$ty>::MAX as usize }
                #[inline]
                fn from_usize(n: usize) -> Self { n as $ty }
            }
        )*
    };
}

impl_trie_index!(u8, u16, u32, u64);

// ---------------------------------------------------------------------------
// two_mut
// ---------------------------------------------------------------------------

#[inline]
fn two_mut<T>(slice: &mut [T], a: usize, b: usize) -> (&mut T, &mut T) {
    debug_assert_ne!(a, b, "two_mut: indices must differ");
    if a < b {
        let (left, right) = slice.split_at_mut(b);
        (&mut left[a], &mut right[0])
    } else {
        let (left, right) = slice.split_at_mut(a);
        (&mut right[0], &mut left[b])
    }
}

// ---------------------------------------------------------------------------
// CTree impl
// ---------------------------------------------------------------------------

#[allow(dead_code)]
impl<K, V, PTR, const N: usize, const NP1: usize>
    CTree<K, V, PTR, N, NP1>
where
    K: TreeKey + SearchStrategy,
    K::Stored: StoredKey,
    PTR: TrieIndex,
    V: Sized,
    [(); N]: ,
    [(); NP1]: ,
{
    const ASSERT_N_FITS: () = assert!(N <= 255, "N must be at most 255");
    const ASSERT_NP1: () = assert!(NP1 == N + 1, "NP1 must equal N + 1");

    #[inline]
    fn rebalance_target(s: usize) -> usize {
        (N + s) / 2
    }

    pub fn new() -> Self {
        let () = Self::ASSERT_NP1;
        let () = Self::ASSERT_N_FITS;
        let root = LeafNode::<K, V, PTR, N>::new();
        Self {
            inodes: Vec::new(),
            leaves: vec![root],
            key_buf: Vec::new(),
            len: 0,
            n_leaves: 1,
            height: 0,
            root_inode: 0,
        }
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub fn compact(&mut self) {
        self.relocate(false);
        self.inodes.shrink_to_fit();
    }

    pub fn optimize(&mut self) {
        self.relocate(false);
    }

    fn relocate(&mut self, gapful: bool) -> Vec<usize> {
        let old_len = self.leaves.len();

        // Dense pack of 0 or 1 live leaf is already optimal — and the empty
        // root (keys.len()==0, n_leaves==1) would be miscounted as a gap by the
        // forward scan below, so short-circuit it with the identity map.
        if !gapful && self.n_leaves <= 1 {
            return (0..old_len).collect();
        }

        // The arena is kept in strict sorted physical order, so a forward scan
        // over non-empty leaves *is* the sorted live-leaf order (no linked list
        // to walk).
        let mut order: Vec<usize> = Vec::with_capacity(self.n_leaves);
        for i in 0..old_len {
            if self.leaves[i].keys.len() > 0 {
                order.push(i);
            }
        }
        let live = order.len();
        debug_assert_eq!(live, self.n_leaves, "relocate: live count {live} != n_leaves {}", self.n_leaves);

        let slot_of = |rank: usize| if gapful { 2 * rank } else { rank };
        // Gapful spread doubles *current* arena capacity (`2 * old_len`), not
        // `2 * live`. The extra slots `[2*live, 2*old_len)` are trailing gaps.
        // This is what makes sequential inserts O(n): the last leaf's end-splits
        // land in the trailing gap at `child_idx+1` (no shift, no spread) until
        // the trailing gaps exhaust, then one spread doubles capacity again —
        // geometric, not the every-2-splits spread that `2 * live` causes. The
        // shift handles clustered split-twice (occupied `child_idx+1`) by
        // shifting into a forward gap, so doubling here does not cause the
        // exponential growth that "spread-if-occupied" would.
        let new_slots = if gapful { 2 * old_len } else { live };

        let mut new_pos = vec![usize::MAX; old_len];
        for rank in 0..live {
            new_pos[order[rank]] = slot_of(rank);
        }

        let mut old = std::mem::take(&mut self.leaves);
        let mut buf: Vec<LeafNode<K, V, PTR, N>> = Vec::with_capacity(new_slots);
        for i in 0..new_slots {
            // Even slots within the live region hold live leaves; odd slots and
            // even slots beyond `2*live` are gaps (the latter are the trailing
            // gaps that absorb sequential end-splits).
            let is_live_slot = !gapful || (i % 2 == 0 && i / 2 < live);
            if is_live_slot {
                let rank = if gapful { i / 2 } else { i };
                let old_idx = order[rank];
                let leaf = std::mem::replace(&mut old[old_idx], LeafNode::new());
                buf.push(leaf);
            } else {
                buf.push(LeafNode::new());
            }
        }
        drop(old);

        self.remap_bottom_leaf_ptrs(&new_pos);

        self.leaves = buf;
        self.n_leaves = live;
        new_pos
    }

    fn spread(&mut self) -> Vec<usize> {
        self.relocate(true)
    }

    /// Remap every bottom-level inode's leaf child pointer `c` to `new_pos[c]`.
    /// Descends from the root through `height - 1` inode levels (following
    /// inode→inode ptrs, which are untouched) to reach the bottom inodes, then
    /// rewrites their leaf child ptrs. Used by `relocate` (full remap) and by
    /// `split_leaf`'s shift path (local +1 remap for the moved leaves).
    fn remap_bottom_leaf_ptrs(&mut self, new_pos: &[usize]) {
        if self.height >= 1 {
            let mut level: Vec<usize> = vec![self.root_inode];
            for _ in 0..self.height - 1 {
                let mut next = Vec::new();
                for &ni in &level {
                    let node = &self.inodes[ni];
                    for ci in 0..=node.keys.len() {
                        if let Some(c) = node.get_ptr(ci) {
                            next.push(c);
                        }
                    }
                }
                level = next;
            }
            for &ni in &level {
                let node = &mut self.inodes[ni];
                let klen = node.keys.len();
                for ci in 0..=klen {
                    if let Some(c) = node.get_ptr(ci) {
                        node.set_ptr(ci, new_pos[c]);
                    }
                }
            }
        }
    }

    /// Targeted +1 remap for `split_leaf`'s shift: bump every bottom-inode leaf
    /// child pointer whose slot lies in `[child_idx+1, g)` (the moved run) by
    /// +1. The moved run is a contiguous run of `child_idx`'s sorted successors,
    /// so we walk the bottom-inode layer rightward as a B-tree in-order
    /// successor walk: scan `child_idx`'s parent from `child_pos+1`, and when a
    /// node is exhausted ascend a level, step to the right sibling, and descend
    /// to the leftmost bottom inode — so a run crossing a grandparent (or
    /// higher) boundary is fully covered. `path` is the descent path from
    /// `walk_to_leaf`. O(run + ascent), not O(n_leaves) — a full remap per split
    /// would make insertion O(n²).
    fn remap_shift_range(&mut self, path: &[(usize, usize)], child_idx: usize, g: usize) {
        if self.height == 0 || path.is_empty() {
            return;
        }
        let plen = path.len();
        // Per depth: the current inode and the next child position to scan.
        let mut cur_inode: Vec<usize> = path.iter().map(|(n, _)| *n).collect();
        let mut cur_pos: Vec<usize> = path.iter().map(|(_, c)| *c).collect();
        cur_pos[plen - 1] += 1; // first moved leaf = child_pos+1
        let mut level = plen - 1;

        loop {
            let inode_idx = cur_inode[level];
            let start = cur_pos[level];
            let klen = self.inodes[inode_idx].keys.len();
            // Read pass: collect in-range bumps; stop once a pointer reaches `g`.
            let mut bumps: Vec<(usize, usize)> = Vec::new();
            let mut at_end = false;
            for ci in start..=klen {
                if let Some(c) = self.inodes[inode_idx].get_ptr(ci) {
                    if c >= child_idx + 1 && c < g {
                        bumps.push((ci, c + 1));
                    } else if c >= g {
                        at_end = true;
                        break;
                    }
                }
            }
            for (ci, np) in bumps {
                self.inodes[inode_idx].set_ptr(ci, np);
            }
            if at_end {
                return;
            }
            // Exhausted this node's children without reaching `g`: ascend until
            // we find a level with a right sibling, then descend to the leftmost
            // bottom inode of that sibling.
            loop {
                if level == 0 {
                    return; // past the root's rightmost — run invalid (shouldn't happen)
                }
                level -= 1;
                cur_pos[level] += 1;
                if cur_pos[level] <= self.inodes[cur_inode[level]].keys.len() {
                    break; // right sibling exists at this level
                }
                // else keep ascending
            }
            // Descend from `level` to the bottom via leftmost children.
            let mut ni = self.inodes[cur_inode[level]].get_ptr(cur_pos[level]).unwrap();
            for d in (level + 1)..plen {
                cur_inode[d] = ni;
                cur_pos[d] = 0;
                if d < plen - 1 {
                    ni = self.inodes[ni].get_ptr(0).unwrap();
                }
            }
            level = plen - 1;
        }
    }

    fn walk_to_leaf(&mut self, needle: &<K as TreeKey>::Needle) -> (usize, Vec<(usize, usize)>) {
        if self.height == 0 {
            return (0, Vec::new());
        }
        let mut path = Vec::new();
        let mut node_idx: usize = self.root_inode;
        for _ in 0..self.height - 1 {
            let child = self.inodes[node_idx].find_child(needle, &self.key_buf);
            let child_idx = self.inodes[node_idx].get_ptr(child).unwrap();
            let mut child = child;
            if self.inodes[child_idx].would_split() && self.try_rebalance_inode(node_idx, child) {
                child = self.inodes[node_idx].find_child(needle, &self.key_buf);
            }
            let child_idx = self.inodes[node_idx].get_ptr(child).unwrap();
            path.push((node_idx, child));
            node_idx = child_idx;
        }
        let child = self.inodes[node_idx].find_child(needle, &self.key_buf);
        let leaf_idx = self.inodes[node_idx].get_ptr(child).unwrap();
        let mut child = child;
        if self.leaves[leaf_idx].would_split() && self.try_rebalance_leaf(node_idx, child) {
            child = self.inodes[node_idx].find_child(needle, &self.key_buf);
        }
        let leaf_idx = self.inodes[node_idx].get_ptr(child).unwrap();
        path.push((node_idx, child));
        (leaf_idx, path)
    }

    #[inline]
    fn find_leaf(&self, needle: &<K as TreeKey>::Needle) -> usize {
        if self.height == 0 {
            return 0;
        }
        let mut node_idx: usize = self.root_inode;
        for _ in 0..self.height - 1 {
            let child = self.inodes[node_idx].find_child(needle, &self.key_buf);
            node_idx = self.inodes[node_idx].get_ptr(child).unwrap();
        }
        let bottom = &self.inodes[node_idx];
        let child = bottom.find_child(needle, &self.key_buf);
        bottom.get_ptr(child).unwrap()
    }

    #[inline]
    fn leaf_has_room_for_two(&self, idx: usize) -> bool {
        self.leaves[idx].keys.len() + 2 <= N
    }

    #[inline]
    fn inode_has_room_for_two(&self, idx: usize) -> bool {
        self.inodes[idx].keys.len() + 2 <= N
    }

    fn pick_rebalance_sibling(
        &self,
        parent_idx: usize,
        child_pos: usize,
        sib_fill: impl Fn(usize) -> usize,
        has_room_for_two: impl Fn(usize) -> bool,
    ) -> Option<(usize, usize, bool)> {
        let p = &self.inodes[parent_idx];
        let (left, right) = p.adjacent_sibling_ptrs(child_pos);
        let node_idx = p.get_ptr(child_pos).unwrap();
        let pick_right = match (left, right) {
            (Some(l), Some(r)) => sib_fill(r) <= sib_fill(l),
            (Some(_), None) => false,
            (None, Some(_)) => true,
            (None, None) => return None,
        };
        let sib = if pick_right { right } else { left }.unwrap();
        if !has_room_for_two(sib) {
            return None;
        }
        Some((node_idx, sib, pick_right))
    }

    fn try_rebalance_leaf(&mut self, parent_idx: usize, child_pos: usize) -> bool {
        let Some((leaf_idx, sib, pick_right)) = self.pick_rebalance_sibling(
            parent_idx, child_pos,
            |i| self.leaves[i].keys.len(),
            |i| self.leaf_has_room_for_two(i),
        ) else {
            return false;
        };
        if pick_right {
            self.redistribute_leaf_right(parent_idx, child_pos, leaf_idx, sib);
        } else {
            self.redistribute_leaf_left(parent_idx, child_pos, leaf_idx, sib);
        }
        true
    }

    fn redistribute_leaf_right(
        &mut self,
        parent_idx: usize,
        child_pos: usize,
        leaf_idx: usize,
        sib_idx: usize,
    ) {
        let s = self.leaves[sib_idx].keys.len();
        let l_target = Self::rebalance_target(s);
        {
            let (leaf, sib) = two_mut(&mut self.leaves, leaf_idx, sib_idx);
            leaf.keys.drain_into_front(l_target, &mut sib.keys);
            leaf.values.drain_into_front(l_target, &mut sib.values);
        }
        let new_sep_stored = self.leaves[sib_idx].keys.get(0).clone();
        *self.inodes[parent_idx].keys.get_mut(child_pos) = new_sep_stored;
    }

    fn redistribute_leaf_left(
        &mut self,
        parent_idx: usize,
        child_pos: usize,
        leaf_idx: usize,
        sib_idx: usize,
    ) {
        let s = self.leaves[sib_idx].keys.len();
        let l_target = Self::rebalance_target(s);
        let m = N - l_target;
        {
            let (leaf, sib) = two_mut(&mut self.leaves, leaf_idx, sib_idx);
            leaf.keys.drain_front_into(m, &mut sib.keys);
            leaf.values.drain_front_into(m, &mut sib.values);
        }
        let new_sep_stored = self.leaves[leaf_idx].keys.get(0).clone();
        *self.inodes[parent_idx].keys.get_mut(child_pos - 1) = new_sep_stored;
    }

    fn try_rebalance_inode(&mut self, gparent_idx: usize, child_pos: usize) -> bool {
        let Some((l_idx, sib, pick_right)) = self.pick_rebalance_sibling(
            gparent_idx, child_pos,
            |i| self.inodes[i].keys.len(),
            |i| self.inode_has_room_for_two(i),
        ) else {
            return false;
        };
        if pick_right {
            self.redistribute_inode_right(gparent_idx, child_pos, l_idx, sib);
        } else {
            self.redistribute_inode_left(gparent_idx, child_pos, l_idx, sib);
        }
        true
    }

    fn redistribute_inode_right(
        &mut self,
        gparent_idx: usize,
        child_pos: usize,
        l_idx: usize,
        r_idx: usize,
    ) {
        let (s, sep0_stored) = {
            let g = &self.inodes[gparent_idx];
            (self.inodes[r_idx].keys.len(), g.keys.get(child_pos).clone())
        };
        let l_target = Self::rebalance_target(s);
        let m = N - l_target;

        let new_sep_stored = {
            let (l, r) = two_mut(&mut self.inodes, l_idx, r_idx);
            r.ptrs.copy_within(0..=s, m);
            for i in 0..m {
                r.ptrs[i] = l.ptrs[l_target + 1 + i].take();
            }
            if m > 1 {
                l.keys.drain_into_front(l_target + 1, &mut r.keys);
            }
            r.keys.insert_at(m - 1, sep0_stored);
            l.keys.remove_at(l_target)
        };
        *self.inodes[gparent_idx].keys.get_mut(child_pos) = new_sep_stored;
    }

    fn redistribute_inode_left(
        &mut self,
        gparent_idx: usize,
        child_pos: usize,
        l_idx: usize,
        sib_idx: usize,
    ) {
        let (s, sep0_stored) = {
            let g = &self.inodes[gparent_idx];
            (self.inodes[sib_idx].keys.len(), g.keys.get(child_pos - 1).clone())
        };
        let l_target = Self::rebalance_target(s);
        let m = N - l_target;

        let new_sep_stored = {
            let (l, sib) = two_mut(&mut self.inodes, l_idx, sib_idx);
            for i in 0..m {
                sib.ptrs[(s + 1) + i] = l.ptrs[i].take();
            }
            sib.keys.push(sep0_stored);
            if m > 1 {
                l.keys.drain_front_into(m - 1, &mut sib.keys);
            }
            l.keys.remove_at(0)
        };
        {
            let l = &mut self.inodes[l_idx];
            l.ptrs.copy_within(m..=N, 0);
            for i in (N - m + 1)..=N {
                l.ptrs[i] = None;
            }
        }
        *self.inodes[gparent_idx].keys.get_mut(child_pos - 1) = new_sep_stored;
    }

    #[inline]
    fn locate(&self, needle: &<K as TreeKey>::Needle) -> (usize, usize) {
        let leaf_idx = self.find_leaf(needle);
        let pos = self.leaves[leaf_idx].find_position(needle, &self.key_buf);
        (leaf_idx, pos)
    }

    pub fn get(&self, key: &<K as TreeKey>::Needle) -> Option<&V> {
        if self.leaves.is_empty() {
            return None;
        }
        let (leaf_idx, pos) = self.locate(key);
        let leaf = &self.leaves[leaf_idx];
        if pos < leaf.keys.len() {
            let stored = leaf.keys.get(pos);
            if StoredKey::eq_key(stored, key, &self.key_buf) {
                return Some(leaf.values.get(pos));
            }
        }
        None
    }

    pub fn get_mut(&mut self, key: &<K as TreeKey>::Needle) -> Option<&mut V> {
        if self.leaves.is_empty() {
            return None;
        }
        let (leaf_idx, pos) = self.locate(key);
        if pos < self.leaves[leaf_idx].keys.len() {
            let stored = self.leaves[leaf_idx].keys.get(pos);
            if StoredKey::eq_key(stored, key, &self.key_buf) {
                return Some(self.leaves[leaf_idx].values.get_mut(pos));
            }
        }
        None
    }

    pub fn insert(&mut self, key: K, value: V) -> Result<(), (K, V)> {
        let _ = Self::ASSERT_N_FITS;

        let needle = key.as_needle();

        let (child_idx, path) = self.walk_to_leaf(needle);
        let leaf = &self.leaves[child_idx];
        let pos = leaf.find_position(needle, &self.key_buf);

        // Key already exists?
        if pos < leaf.keys.len() && StoredKey::eq_key(leaf.keys.get(pos), needle, &self.key_buf) {
            return Err((key, value));
        }

        let stored = K::into_stored(key, &mut self.key_buf);

        if leaf.keys.len() >= N {
            let mid = N / 2;
            let (parent_idx, new_leaf_idx) = self.split_leaf(child_idx, path);
            if pos <= mid {
                self.leaves[parent_idx].insert(pos, stored, value);
            } else {
                self.leaves[new_leaf_idx].insert(pos - mid, stored, value);
            }
        } else {
            self.leaves[child_idx].insert(pos, stored, value);
        }

        self.len += 1;
        Ok(())
    }

    fn split_leaf(&mut self, child_idx: usize, mut path: Vec<(usize, usize)>) -> (usize, usize) {
        let mut child_idx = child_idx;

        // Proactive resize: at ~90% arena occupancy, re-disperse live leaves
        // into even slots with fresh gaps (grows capacity geometrically).
        if self.n_leaves * 10 >= self.leaves.len() * 9 {
            let map = self.spread();
            child_idx = map[child_idx];
        }

        // The new leaf's exact sorted slot is `child_idx+1`. Find the next free
        // gap at or after that slot. If the forward region is entirely full of
        // live leaves (no gap before the arena end), spread to grow + re-disperse
        // (post-spread `child_idx+1` is a gap, so the shift below collapses to a
        // direct place).
        let mut g = child_idx + 1;
        while g < self.leaves.len() && self.leaves[g].keys.len() != 0 {
            g += 1;
        }
        if g == self.leaves.len() {
            let map = self.spread();
            child_idx = map[child_idx];
            g = child_idx + 1; // guaranteed an in-range gap after a spread
            debug_assert!(g < self.leaves.len() && self.leaves[g].keys.len() == 0);
        }

        // Arena stable. Drain the split in place (child_idx's leaf keeps its slot).
        let mid = N / 2;
        let mid_stored = self.leaves[child_idx].keys.get(mid).clone();

        let mut new_leaf = LeafNode::<K, V, PTR, N>::new();
        self.leaves[child_idx].keys.drain_into(mid, &mut new_leaf.keys);
        self.leaves[child_idx].values.drain_into(mid, &mut new_leaf.values);

        // If `child_idx+1` wasn't the gap we found, shift the run of live leaves
        // `[child_idx+1, g)` right by one into the gap at `g` (rotate_right(1) on
        // the inclusive range is a safe Drop-preserving permutation). That frees
        // `child_idx+1` for `new_leaf` at its exact sorted position — strict
        // physical order is maintained. The moved leaves change slot by +1, so
        // remap their parent-inode child pointers by +1 too.
        if g > child_idx + 1 {
            self.leaves[child_idx + 1..=g].rotate_right(1);
            // The moved run `[child_idx+1, g)` is a contiguous run of `child_idx`'s
            // sorted successors, so their parent-inode child pointers (starting at
            // `child_pos+1` in `child_idx`'s parent, then that parent's right
            // siblings) need +1. Targeted O(run) walk — NOT a full arena remap.
            self.remap_shift_range(&path, child_idx, g);
        }
        self.leaves[child_idx + 1] = new_leaf;
        let new_leaf_idx = child_idx + 1;

        self.n_leaves += 1;
        self.insert_separator(mid_stored, new_leaf_idx, &mut path);
        (child_idx, new_leaf_idx)
    }

    fn insert_separator(&mut self, stored: <K as TreeKey>::Stored, new_child_idx: usize, path: &mut Vec<(usize, usize)>) {
        if path.is_empty() {
            let old_root_idx = self.root_inode;
            let mut root = KeyNode::<K, PTR, N, NP1>::new();
            root.keys.insert_at(0, stored);
            root.set_ptr(0, old_root_idx);
            root.set_ptr(1, new_child_idx);
            let root_idx = self.inodes.len();
            self.inodes.push(root);
            self.root_inode = root_idx;
            self.height += 1;
            return;
        }

        let (parent_idx, _) = path.pop().unwrap();

        if !self.inodes[parent_idx].would_split() {
            let pos = self.find_position_for_stored_in_inode(parent_idx, &stored);
            self.inodes[parent_idx].insert_key_at(pos, stored);
            self.inodes[parent_idx].set_ptr(pos + 1, new_child_idx);
        } else {
            self.split_inode(parent_idx, stored, new_child_idx, path);
        }
    }

    fn split_inode(
        &mut self,
        parent_idx: usize,
        new_stored: <K as TreeKey>::Stored,
        new_child_idx: usize,
        path: &mut Vec<(usize, usize)>,
    ) {
        let mid = N / 2;
        // Save the mid separator before we start moving things.
        let mid_stored = self.inodes[parent_idx].keys.get(mid).clone();

        // Save ptrs length before drain_into truncates keys.
        let old_len = self.inodes[parent_idx].keys.len();

        let mut new_inode = KeyNode::<K, PTR, N, NP1>::new();
        // Move keys [mid+1..old_len) to new inode.
        if mid + 1 < old_len {
            self.inodes[parent_idx].keys.drain_into(mid + 1, &mut new_inode.keys);
        }

        // Move ptrs [mid+1..=old_len] to new inode.
        for i in 0..=old_len - (mid + 1) {
            new_inode.ptrs[i] = self.inodes[parent_idx].ptrs[mid + 1 + i];
        }

        // Remove the mid separator key.
        self.inodes[parent_idx].keys.remove_at(mid);
        // Truncate ptrs: keep [0..=mid], clear the rest.
        for i in (mid + 1)..=old_len {
            self.inodes[parent_idx].ptrs[i] = None;
        }

        // Insert the new key/child into the appropriate inode.
        let goes_right = StoredKey::cmp_stored(&new_stored, &mid_stored, &self.key_buf) != Ordering::Less;
        if goes_right {
            let pos = self.find_position_for_stored_in_keys(&new_stored, &new_inode.keys);
            new_inode.insert_key_at(pos, new_stored);
            new_inode.set_ptr(pos + 1, new_child_idx);
        } else {
            let pos = self.find_position_for_stored_in_keys(&new_stored, &self.inodes[parent_idx].keys);
            self.inodes[parent_idx].insert_key_at(pos, new_stored);
            self.inodes[parent_idx].set_ptr(pos + 1, new_child_idx);
        }

        let new_inode_idx = self.inodes.len();
        self.inodes.push(new_inode);

        // Recurse: insert the mid separator into the grandparent.
        self.insert_separator(mid_stored, new_inode_idx, path);
    }

    /// Find the position where `stored` should be inserted among an inode's keys.
    fn find_position_for_stored_in_inode(&self, inode_idx: usize, stored: &K::Stored) -> usize {
        let keys = self.inodes[inode_idx].keys.as_slice();
        let buf = &self.key_buf;
        for (i, k) in keys.iter().enumerate() {
            if StoredKey::cmp_stored(stored, k, buf) != Ordering::Greater {
                return i;
            }
        }
        keys.len()
    }

    /// Find the position where `stored` should be inserted among a key array.
    fn find_position_for_stored_in_keys(&self, stored: &K::Stored, keys: &TinyArray<K::Stored, N>) -> usize {
        let buf = &self.key_buf;
        let slice = keys.as_slice();
        for (i, k) in slice.iter().enumerate() {
            if StoredKey::cmp_stored(stored, k, buf) != Ordering::Greater {
                return i;
            }
        }
        slice.len()
    }

    fn descend_to_leaf(&self, rightmost: bool) -> usize {
        if self.height == 0 {
            return 0;
        }
        let mut node_idx: usize = self.root_inode;
        for _ in 0..self.height - 1 {
            let ci = if rightmost { self.inodes[node_idx].keys.len() } else { 0 };
            node_idx = self.inodes[node_idx].get_ptr(ci).unwrap();
        }
        let ci = if rightmost { self.inodes[node_idx].keys.len() } else { 0 };
        self.inodes[node_idx].get_ptr(ci).unwrap()
    }

    fn first_leaf(&self) -> usize {
        self.descend_to_leaf(false)
    }

    fn last_leaf(&self) -> usize {
        self.descend_to_leaf(true)
    }

    pub fn get_cursor(&self) -> Cursor<'_, K, V, PTR, N, NP1> {
        let leaf_idx = self.first_leaf();
        Cursor {
            tree: self,
            leaf_idx,
            position: 0,
        }
    }

    pub fn get_cursor_mut(&mut self) -> CursorMut<'_, K, V, PTR, N, NP1> {
        let leaf_idx = self.first_leaf();
        CursorMut {
            tree: self,
            leaf_idx,
            position: 0,
        }
    }

    pub fn cursor_at(&self, key: &<K as TreeKey>::Needle) -> Cursor<'_, K, V, PTR, N, NP1> {
        let (leaf_idx, pos) = self.locate(key);
        Cursor {
            tree: self,
            leaf_idx,
            position: pos,
        }
    }
}

// ---------------------------------------------------------------------------
// Cursor impl
// ---------------------------------------------------------------------------

impl<'a, K, V, PTR, const N: usize, const NP1: usize>
    Cursor<'a, K, V, PTR, N, NP1>
where
    K: TreeKey + SearchStrategy,
    K::Stored: StoredKey,
    PTR: TrieIndex,
    V: Sized,
    [(); N]: ,
    [(); NP1]: ,
{
    pub fn current(&self) -> Option<(&<K as TreeKey>::Stored, &V)> {
        let leaf = &self.tree.leaves[self.leaf_idx];
        if self.position < leaf.keys.len() {
            Some((leaf.keys.get(self.position), leaf.values.get(self.position)))
        } else {
            None
        }
    }

    pub fn next(&mut self) -> Option<&V> {
        let leaf = &self.tree.leaves[self.leaf_idx];
        if self.position + 1 < leaf.keys.len() {
            self.position += 1;
            return Some(self.tree.leaves[self.leaf_idx].values.get(self.position));
        }
        // Skip forward over gap slots to the next live leaf.
        let mut i = self.leaf_idx + 1;
        while i < self.tree.leaves.len() && self.tree.leaves[i].keys.len() == 0 {
            i += 1;
        }
        if i >= self.tree.leaves.len() {
            return None;
        }
        self.leaf_idx = i;
        self.position = 0;
        Some(self.tree.leaves[self.leaf_idx].values.get(0))
    }

    pub fn prev(&mut self) -> Option<&V> {
        if self.position > 0 {
            self.position -= 1;
            return Some(self.tree.leaves[self.leaf_idx].values.get(self.position));
        }
        // Skip backward over gap slots to the previous live leaf.
        let mut i = self.leaf_idx;
        while i > 0 {
            i -= 1;
            if self.tree.leaves[i].keys.len() > 0 {
                self.leaf_idx = i;
                let last_pos = self.tree.leaves[i].keys.len() - 1;
                self.position = last_pos;
                return Some(self.tree.leaves[self.leaf_idx].values.get(last_pos));
            }
        }
        None
    }
}

impl<'a, K, V, PTR, const N: usize, const NP1: usize>
    CursorMut<'a, K, V, PTR, N, NP1>
where
    K: TreeKey + SearchStrategy,
    K::Stored: StoredKey,
    PTR: TrieIndex,
    V: Sized,
    [(); N]: ,
    [(); NP1]: ,
{
    pub fn current(&mut self) -> Option<(&<K as TreeKey>::Stored, &mut V)> {
        let leaf = &mut self.tree.leaves[self.leaf_idx];
        if self.position < leaf.keys.len() {
            Some((leaf.keys.get(self.position), leaf.values.get_mut(self.position)))
        } else {
            None
        }
    }

    pub fn next(&mut self) -> Option<&mut V> {
        let leaf = &self.tree.leaves[self.leaf_idx];
        if self.position + 1 < leaf.keys.len() {
            self.position += 1;
        } else {
            // Skip forward over gap slots to the next live leaf.
            let mut i = self.leaf_idx + 1;
            while i < self.tree.leaves.len() && self.tree.leaves[i].keys.len() == 0 {
                i += 1;
            }
            if i >= self.tree.leaves.len() {
                return None;
            }
            self.leaf_idx = i;
            self.position = 0;
        }
        Some(self.tree.leaves[self.leaf_idx].values.get_mut(self.position))
    }

    pub fn prev(&mut self) -> Option<&mut V> {
        if self.position > 0 {
            self.position -= 1;
        } else {
            // Skip backward over gap slots to the previous live leaf.
            let mut i = self.leaf_idx;
            while i > 0 {
                i -= 1;
                if self.tree.leaves[i].keys.len() > 0 {
                    self.leaf_idx = i;
                    self.position = self.tree.leaves[i].keys.len() - 1;
                    return Some(self.tree.leaves[self.leaf_idx].values.get_mut(self.position));
                }
            }
            return None;
        }
        Some(self.tree.leaves[self.leaf_idx].values.get_mut(self.position))
    }
}

// ---------------------------------------------------------------------------
// Instantiation aliases
// ---------------------------------------------------------------------------

#[cfg(test)]
#[path = "tests/int_btree.rs"]
mod tests;