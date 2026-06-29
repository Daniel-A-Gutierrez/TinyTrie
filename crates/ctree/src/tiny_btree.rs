use std::cmp::Ordering;
use std::num::{NonZero, ZeroablePrimitive};
use std::simd::cmp::SimdPartialOrd;
use std::simd::Simd;

use crate::tiny_array::TinyArray;

// ---------------------------------------------------------------------------
// NoPreview marker
// ---------------------------------------------------------------------------

/// Marker type indicating a fixed-length key with no separate preview array.
/// Used as the default `P` in `CTree<K, V, PTR, N, NP1, P = NoPreview>`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Default)]
pub struct NoPreview;

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
// impl_fixed_len_key_simd!(char, 4);
// ---------------------------------------------------------------------------
// TreeKey
// ---------------------------------------------------------------------------

/// Maps a user's key type to the internal stored form and lookup needle.
pub trait TreeKey: Ord + Clone {
    /// Internal stored form: identity for fixed keys, `Box<[u8]>` for varlen.
    type Stored: StoredKey<Needle = Self::Needle>;
    /// Borrowed needle for lookups.
    type Needle: ?Sized;
    /// Consume into stored form.
    fn into_stored(self) -> Self::Stored;
    /// Borrow self as lookup needle.
    fn as_needle(&self) -> &Self::Needle;
}

/// Auto-impl `TreeKey` for fixed-length keys (identity mapping).
impl<T: FixedLenKey> TreeKey for T {
    type Stored = T;
    type Needle = T;
    fn into_stored(self) -> T { self }
    fn as_needle(&self) -> &T { self }
}

// ---------------------------------------------------------------------------
// Preview
// ---------------------------------------------------------------------------

/// SIMD-able preview of a key. Separate from `TreeKey` so a single `K` can have
/// multiple preview sizes (e.g. `Vec<u8>` -> `u8`, `u16`, `u32`, `u64`).
pub trait Preview<P> {
    fn preview(&self) -> P;
}

/// Fixed keys preview to `NoPreview` (the marker default).
impl<T: FixedLenKey> Preview<NoPreview> for T {
    fn preview(&self) -> NoPreview { NoPreview }
}

// NOTE: No blanket `impl<P, T: Preview<P>> Preview<P> for &T` — it conflicts
// with the `FixedLenKey -> Preview<NoPreview>` blanket when downstream types
// implement `FixedLenKey` for `&T`.

// ---------------------------------------------------------------------------
// TreeKey + Preview impls for byte containers
// ---------------------------------------------------------------------------

impl TreeKey for Vec<u8> {
    type Stored = Box<[u8]>;
    type Needle = [u8];
    fn into_stored(self) -> Box<[u8]> { self.into_boxed_slice() }
    fn as_needle(&self) -> &[u8] { self }
}

impl TreeKey for Box<[u8]> {
    type Stored = Box<[u8]>;
    type Needle = [u8];
    fn into_stored(self) -> Box<[u8]> { self }
    fn as_needle(&self) -> &[u8] { self }
}

// Preview helpers: right-pad with zeros to preview width, big-endian.
#[inline]
fn preview_u8(src: &[u8]) -> u8 {
    if src.is_empty() { 0 } else { src[0] }
}

#[inline]
fn preview_u16(src: &[u8]) -> u16 {
    let mut buf = [0u8; 2];
    let n = src.len().min(2);
    buf[..n].copy_from_slice(&src[..n]);
    u16::from_be_bytes(buf)
}

#[inline]
fn preview_u32(src: &[u8]) -> u32 {
    let mut buf = [0u8; 4];
    let n = src.len().min(4);
    buf[..n].copy_from_slice(&src[..n]);
    u32::from_be_bytes(buf)
}

#[inline]
fn preview_u64(src: &[u8]) -> u64 {
    let mut buf = [0u8; 8];
    let n = src.len().min(8);
    buf[..n].copy_from_slice(&src[..n]);
    u64::from_be_bytes(buf)
}

// Vec<u8> previews
impl Preview<u8>  for Vec<u8> { fn preview(&self) -> u8  { preview_u8(self) } }
impl Preview<u16> for Vec<u8> { fn preview(&self) -> u16 { preview_u16(self) } }
impl Preview<u32> for Vec<u8> { fn preview(&self) -> u32 { preview_u32(self) } }
impl Preview<u64> for Vec<u8> { fn preview(&self) -> u64 { preview_u64(self) } }

// Box<[u8]> previews
impl Preview<u8>  for Box<[u8]> { fn preview(&self) -> u8  { preview_u8(self) } }
impl Preview<u16> for Box<[u8]> { fn preview(&self) -> u16 { preview_u16(self) } }
impl Preview<u32> for Box<[u8]> { fn preview(&self) -> u32 { preview_u32(self) } }
impl Preview<u64> for Box<[u8]> { fn preview(&self) -> u64 { preview_u64(self) } }

// [u8] previews (for SearchStrategy varlen path — needle type is [u8])
impl Preview<u8>  for [u8] { fn preview(&self) -> u8  { preview_u8(self) } }
impl Preview<u16> for [u8] { fn preview(&self) -> u16 { preview_u16(self) } }
impl Preview<u32> for [u8] { fn preview(&self) -> u32 { preview_u32(self) } }
impl Preview<u64> for [u8] { fn preview(&self) -> u64 { preview_u64(self) } }

// ---------------------------------------------------------------------------
// SearchStrategy — dispatch fixed vs varlen search
// ---------------------------------------------------------------------------

/// Static dispatch for node search: fixed keys search the key array directly;
/// variable keys search previews with SIMD then fall back to scalar comparison.
///
/// The `Needle` type is the borrowed lookup form: `K` for fixed keys, `[u8]`
/// for variable-length byte keys. This lets `get`/`cursor_at` accept `&K::Needle`
/// directly — no owned key needed for lookups.
pub trait SearchStrategy<P>: TreeKey {
    fn find_position(needle: &Self::Needle, previews: &[P], keys: &[Self::Stored]) -> usize;
    fn find_upper_bound(needle: &Self::Needle, previews: &[P], keys: &[Self::Stored]) -> usize;
}

// Fixed keys: P = NoPreview, search keys directly via SIMD (ignore previews).
impl<K: FixedLenKey> SearchStrategy<NoPreview> for K {
    fn find_position(needle: &K, _previews: &[NoPreview], keys: &[K]) -> usize {
        K::find_position(needle, keys)
    }
    fn find_upper_bound(needle: &K, _previews: &[NoPreview], keys: &[K]) -> usize {
        K::find_upper_bound(needle, keys)
    }
}

// Variable keys: P: FixedLenKey, search previews then fallback.
impl<K: TreeKey + Preview<P>, P: FixedLenKey> SearchStrategy<P> for K
where
    K::Stored: StoredKey,
    K::Needle: Preview<P>,
{
    fn find_position(needle: &K::Needle, _previews: &[P], keys: &[K::Stored]) -> usize {
        for (i, k) in keys.iter().enumerate() {
            if StoredKey::cmp_key(k, needle) != Ordering::Less {
                return i;
            }
        }
        keys.len()
    }
    fn find_upper_bound(needle: &K::Needle, _previews: &[P], keys: &[K::Stored]) -> usize {
        for (i, k) in keys.iter().enumerate() {
            if StoredKey::cmp_key(k, needle) == Ordering::Greater {
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
    /// Marker sealing [`super::StoredKey`] to the two canonical key forms.
    pub trait Sealed {}
}

pub trait StoredKey: Ord + Clone + private::Sealed
where
    Self: Borrow<Self::Needle>,
{
    /// Borrowed lookup needle: `K` for fixed, `[u8]` for variable.
    type Needle: ?Sized;
    /// Compare stored key against needle.
    fn cmp_key(stored: &Self, needle: &Self::Needle) -> Ordering;
    /// Check equality.
    fn eq_key(stored: &Self, needle: &Self::Needle) -> bool;
}

use std::borrow::Borrow;

// Fixed form
impl<K: FixedLenKey> private::Sealed for K {}

impl<K: FixedLenKey> StoredKey for K {
    type Needle = K;
    fn cmp_key(stored: &K, needle: &K) -> Ordering { stored.cmp(needle) }
    fn eq_key(stored: &K, needle: &K) -> bool { stored == needle }
}

// Variable form: Box<[K]>
impl<K: FixedLenKey> private::Sealed for Box<[K]> {}

impl<K: FixedLenKey> StoredKey for Box<[K]> {
    type Needle = [K];
    fn cmp_key(stored: &Self, needle: &[K]) -> Ordering {
        cmp_slice_scalar(stored.as_ref(), needle)
    }
    fn eq_key(stored: &Self, needle: &[K]) -> bool {
        eq_slice_scalar(stored.as_ref(), needle)
    }
}

// Scalar comparison helpers for the varlen (`Box<[K]>`) binary search.
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
// Node types — unified over K: TreeKey + Preview<P>
// ---------------------------------------------------------------------------

/// Internal (separator) node: `previews` + `keys` plus `ptrs` to children.
/// For fixed keys (`P = NoPreview`) the `previews` array is a ZST (0 bytes
/// per element) and the node searches the `keys` array directly via SIMD.
/// For variable keys (`P = u8/u16/u32/u64`) the node searches `previews` with
/// SIMD then falls back to `StoredKey::cmp_key` on collisions.
struct KeyNode<K, P, PTR, const N: usize, const NP1: usize>
where
    K: TreeKey + Preview<P>,
    PTR: TrieIndex,
    [(); N]: ,
    [(); NP1]: ,
{
    previews: TinyArray<P, N>,
    keys: TinyArray<K::Stored, N>,
    ptrs: [Option<NonZero<PTR>>; NP1],
}

/// Leaf node. Carries the leaf linked list (`prev`/`next`) for O(1) cursor
/// navigation.
struct LeafNode<K, P, V, PTR, const N: usize>
where
    K: TreeKey + Preview<P>,
    PTR: TrieIndex,
    V: Sized,
    [(); N]: ,
{
    previews: TinyArray<P, N>,
    keys: TinyArray<K::Stored, N>,
    values: TinyArray<V, N>,
    prev: Option<NonZero<PTR>>,
    next: Option<NonZero<PTR>>,
}

// ---------------------------------------------------------------------------
// CTree
// ---------------------------------------------------------------------------

/// B+ tree. `K` is the user's key type, `P` is the preview type.
///   * `CTree<K, V, ..., P = NoPreview>`  — fixed-size keys, SIMD search
///   * `CTree<K, V, ..., P = u64>`        — variable-length keys, preview+SIMD
///
/// All tree operations are implemented once, generically over `SearchStrategy<P>`.
pub struct CTree<K, V, PTR, const N: usize, const NP1: usize, P = NoPreview>
where
    K: TreeKey + Preview<P>,
    PTR: TrieIndex,
    V: Sized,
    P: Copy,
    [(); N]: ,
    [(); NP1]: ,
{
    inodes: Vec<KeyNode<K, P, PTR, N, NP1>>,
    leaves: Vec<LeafNode<K, P, V, PTR, N>>,
    len: usize,
    /// Count of LIVE leaves (excludes gap sentinels).
    n_leaves: usize,
    /// Number of inode levels. 0 = root is a leaf.
    height: usize,
    /// Index of the root inode in `self.inodes`. Only valid when height >= 1.
    root_inode: usize,
}

pub struct Cursor<'a, K, V, PTR, const N: usize, const NP1: usize, P = NoPreview>
where
    K: TreeKey + Preview<P>,
    PTR: TrieIndex,
    V: Sized,
    P: Copy,
    [(); N]: ,
    [(); NP1]: ,
{
    tree: &'a CTree<K, V, PTR, N, NP1, P>,
    leaf_idx: usize,
    position: usize,
}

pub struct CursorMut<'a, K, V, PTR, const N: usize, const NP1: usize, P = NoPreview>
where
    K: TreeKey + Preview<P>,
    PTR: TrieIndex,
    V: Sized,
    P: Copy,
    [(); N]: ,
    [(); NP1]: ,
{
    tree: &'a mut CTree<K, V, PTR, N, NP1, P>,
    leaf_idx: usize,
    position: usize,
}

// ---------------------------------------------------------------------------
// KeyNode impl
// ---------------------------------------------------------------------------

#[allow(dead_code)]
impl<K, P, PTR, const N: usize, const NP1: usize> KeyNode<K, P, PTR, N, NP1>
where
    K: TreeKey + Preview<P> + SearchStrategy<P>,
    K::Stored: StoredKey,
    PTR: TrieIndex,
    P: Copy,
    [(); N]: ,
    [(); NP1]: ,
{
    const ASSERT_NP1: () = assert!(NP1 == N + 1, "NP1 must equal N + 1");

    fn new() -> Self {
        Self {
            previews: TinyArray::new(),
            keys: TinyArray::new(),
            ptrs: [None; NP1],
        }
    }

    /// Create a child node from parent's keys/ptrs in `[from..to)`.
    fn from_parent(from: usize, to: usize, parent: &Self) -> Self {
        let mut node = Self::new();
        for i in from..to {
            node.keys.insert_at(i - from, parent.keys.get(i).clone());
            node.previews.insert_at(i - from, *parent.previews.get(i));
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
    fn find_position(&self, needle: &<K as TreeKey>::Needle) -> usize {
        <K as SearchStrategy<P>>::find_position(needle, self.previews.as_slice(), self.keys.as_slice())
    }

    /// Find the child pointer index for `needle` in a B+ tree internal node.
    /// Returns the index `i` such that `ptrs[i]` points to the subtree for `needle`.
    #[inline]
    fn find_child(&self, needle: &<K as TreeKey>::Needle) -> usize {
        <K as SearchStrategy<P>>::find_upper_bound(needle, self.previews.as_slice(), self.keys.as_slice())
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

    /// Insert stored key at position `pos`, with its preview.
    fn insert_key_at(&mut self, pos: usize, preview: P, stored: <K as TreeKey>::Stored) {
        debug_assert!(!self.would_split());
        let l = self.keys.len();
        if pos < l {
            for i in (pos + 1..=l).rev() {
                self.ptrs[i + 1] = self.ptrs[i];
            }
        }
        self.keys.insert_at(pos, stored);
        self.previews.insert_at(pos, preview);
    }

    /// Insert stored key into this internal node in sorted order.
    fn insert_leaf(&mut self, needle: &<K as TreeKey>::Needle, preview: P, stored: <K as TreeKey>::Stored) -> usize {
        let pos = self.find_position(needle);
        self.insert_key_at(pos, preview, stored);
        pos
    }

    /// Remove key at `pos` and its right child pointer.
    fn remove(&mut self, pos: usize) -> <K as TreeKey>::Stored {
        let l = self.keys.len();
        let k = self.keys.remove_at(pos);
        self.previews.remove_at(pos);
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
        self.previews.truncate(newlen);
    }
}

// ---------------------------------------------------------------------------
// LeafNode impl
// ---------------------------------------------------------------------------

#[allow(dead_code)]
impl<K, P, V, PTR, const N: usize> LeafNode<K, P, V, PTR, N>
where
    K: TreeKey + Preview<P> + SearchStrategy<P>,
    K::Stored: StoredKey,
    PTR: TrieIndex,
    V: Sized,
    P: Copy,
    [(); N]: ,
{
    fn new() -> Self {
        Self {
            previews: TinyArray::new(),
            keys: TinyArray::new(),
            values: TinyArray::new(),
            prev: None,
            next: None,
        }
    }

    #[inline]
    fn get_prev(&self) -> Option<usize> {
        self.prev.map(|nz| nz.get().as_usize() - 1)
    }

    #[inline]
    fn get_next(&self) -> Option<usize> {
        self.next.map(|nz| nz.get().as_usize() - 1)
    }

    #[inline]
    fn set_prev(&mut self, idx: usize) {
        self.prev = NonZero::new(PTR::from_usize(idx + 1));
    }

    #[inline]
    fn set_next(&mut self, idx: usize) {
        self.next = NonZero::new(PTR::from_usize(idx + 1));
    }

    #[inline]
    fn clear_prev(&mut self) {
        self.prev = None;
    }

    #[inline]
    fn clear_next(&mut self) {
        self.next = None;
    }

    #[inline]
    fn find_position(&self, needle: &<K as TreeKey>::Needle) -> usize {
        <K as SearchStrategy<P>>::find_position(needle, self.previews.as_slice(), self.keys.as_slice())
    }

    #[inline]
    fn would_split(&self) -> bool {
        self.keys.is_full()
    }

    /// Insert key-value at position `pos`.
    fn insert(&mut self, pos: usize, preview: P, stored: <K as TreeKey>::Stored, value: V) {
        self.previews.insert_at(pos, preview);
        self.keys.insert_at(pos, stored);
        self.values.insert_at(pos, value);
    }

    fn remove(&mut self, pos: usize) -> (<K as TreeKey>::Stored, V) {
        self.previews.remove_at(pos);
        let k = self.keys.remove_at(pos);
        let v = self.values.remove_at(pos);
        (k, v)
    }

    fn truncate(&mut self, newlen: u8) {
        self.keys.truncate(newlen);
        self.previews.truncate(newlen);
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
impl<K, V, PTR, const N: usize, const NP1: usize, P>
    CTree<K, V, PTR, N, NP1, P>
where
    K: TreeKey + Preview<P> + SearchStrategy<P>,
    K::Stored: StoredKey,
    PTR: TrieIndex,
    V: Sized,
    P: Copy,
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
        let root = LeafNode::<K, P, V, PTR, N>::new();
        Self {
            inodes: Vec::new(),
            leaves: vec![root],
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
        let mut order: Vec<usize> = Vec::with_capacity(self.n_leaves);
        let mut idx = self.first_leaf();
        order.push(idx);
        while let Some(nx) = self.leaves[idx].get_next() {
            order.push(nx);
            idx = nx;
        }
        let live = order.len();
        debug_assert_eq!(live, self.n_leaves, "relocate: linked-list length {live} != n_leaves {}", self.n_leaves);

        let slot_of = |rank: usize| if gapful { 2 * rank } else { rank };
        let new_slots = slot_of(live);

        let mut new_pos = vec![usize::MAX; old_len];
        for rank in 0..live {
            new_pos[order[rank]] = slot_of(rank);
        }

        let mut old = std::mem::take(&mut self.leaves);
        let mut buf: Vec<LeafNode<K, P, V, PTR, N>> = Vec::with_capacity(new_slots);
        for i in 0..new_slots {
            let is_live_slot = !gapful || i % 2 == 0;
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

        for rank in 0..live {
            let i = slot_of(rank);
            if rank > 0 {
                buf[i].set_prev(slot_of(rank - 1));
            } else {
                buf[i].clear_prev();
            }
            if rank + 1 < live {
                buf[i].set_next(slot_of(rank + 1));
            } else {
                buf[i].clear_next();
            }
        }

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

        self.leaves = buf;
        self.n_leaves = live;
        new_pos
    }

    fn spread(&mut self) -> Vec<usize> {
        self.relocate(true)
    }

    fn claim_slot(&self, after: usize) -> usize {
        let n = self.leaves.len();
        let mut i = after + 1;
        while i < n {
            if self.leaves[i].keys.len() == 0 {
                return i;
            }
            i += 1;
        }
        for i in 0..after {
            if self.leaves[i].keys.len() == 0 {
                return i;
            }
        }
        unreachable!("claim_slot: no free gap (caller must spread when full)")
    }

    fn walk_to_leaf(&mut self, needle: &<K as TreeKey>::Needle) -> (usize, Vec<(usize, usize)>) {
        if self.height == 0 {
            return (0, Vec::new());
        }
        let mut path = Vec::new();
        let mut node_idx: usize = self.root_inode;
        for _ in 0..self.height - 1 {
            let child = self.inodes[node_idx].find_child(needle);
            let child_idx = self.inodes[node_idx].get_ptr(child).unwrap();
            let mut child = child;
            if self.inodes[child_idx].would_split() && self.try_rebalance_inode(node_idx, child) {
                child = self.inodes[node_idx].find_child(needle);
            }
            let child_idx = self.inodes[node_idx].get_ptr(child).unwrap();
            path.push((node_idx, child));
            node_idx = child_idx;
        }
        let child = self.inodes[node_idx].find_child(needle);
        let leaf_idx = self.inodes[node_idx].get_ptr(child).unwrap();
        let mut child = child;
        if self.leaves[leaf_idx].would_split() && self.try_rebalance_leaf(node_idx, child) {
            child = self.inodes[node_idx].find_child(needle);
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
            let child = self.inodes[node_idx].find_child(needle);
            node_idx = self.inodes[node_idx].get_ptr(child).unwrap();
        }
        let bottom = &self.inodes[node_idx];
        let child = bottom.find_child(needle);
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
            leaf.previews.drain_into_front(l_target, &mut sib.previews);
            leaf.values.drain_into_front(l_target, &mut sib.values);
        }
        let new_sep_stored = self.leaves[sib_idx].keys.get(0).clone();
        let new_sep_preview = *self.leaves[sib_idx].previews.get(0);
        *self.inodes[parent_idx].keys.get_mut(child_pos) = new_sep_stored;
        *self.inodes[parent_idx].previews.get_mut(child_pos) = new_sep_preview;
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
            leaf.previews.drain_front_into(m, &mut sib.previews);
            leaf.values.drain_front_into(m, &mut sib.values);
        }
        let new_sep_stored = self.leaves[leaf_idx].keys.get(0).clone();
        let new_sep_preview = *self.leaves[leaf_idx].previews.get(0);
        *self.inodes[parent_idx].keys.get_mut(child_pos - 1) = new_sep_stored;
        *self.inodes[parent_idx].previews.get_mut(child_pos - 1) = new_sep_preview;
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
        let sep0_preview = *self.inodes[gparent_idx].previews.get(child_pos);
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
                l.previews.drain_into_front(l_target + 1, &mut r.previews);
            }
            r.keys.insert_at(m - 1, sep0_stored);
            r.previews.insert_at(m - 1, sep0_preview);
            l.keys.remove_at(l_target)
        };
        let new_sep_preview = *self.inodes[l_idx].previews.get(l_target);
        self.inodes[l_idx].previews.remove_at(l_target);
        *self.inodes[gparent_idx].keys.get_mut(child_pos) = new_sep_stored;
        *self.inodes[gparent_idx].previews.get_mut(child_pos) = new_sep_preview;
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
        let sep0_preview = *self.inodes[gparent_idx].previews.get(child_pos - 1);
        let l_target = Self::rebalance_target(s);
        let m = N - l_target;

        let new_sep_stored = {
            let (l, sib) = two_mut(&mut self.inodes, l_idx, sib_idx);
            for i in 0..m {
                sib.ptrs[(s + 1) + i] = l.ptrs[i].take();
            }
            sib.keys.push(sep0_stored);
            sib.previews.push(sep0_preview);
            if m > 1 {
                l.keys.drain_front_into(m - 1, &mut sib.keys);
                l.previews.drain_front_into(m - 1, &mut sib.previews);
            }
            l.keys.remove_at(0)
        };
        let new_sep_preview = *self.inodes[l_idx].previews.get(0);
        {
            let l = &mut self.inodes[l_idx];
            l.ptrs.copy_within(m..=N, 0);
            for i in (N - m + 1)..=N {
                l.ptrs[i] = None;
            }
            l.previews.remove_at(0);
        }
        *self.inodes[gparent_idx].keys.get_mut(child_pos - 1) = new_sep_stored;
        *self.inodes[gparent_idx].previews.get_mut(child_pos - 1) = new_sep_preview;
    }

    #[inline]
    fn locate(&self, needle: &<K as TreeKey>::Needle) -> (usize, usize) {
        let leaf_idx = self.find_leaf(needle);
        let pos = self.leaves[leaf_idx].find_position(needle);
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
            if StoredKey::eq_key(stored, key) {
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
            if StoredKey::eq_key(stored, key) {
                return Some(self.leaves[leaf_idx].values.get_mut(pos));
            }
        }
        None
    }

    pub fn insert(&mut self, key: K, value: V) -> Result<(), (K, V)> {
        let _ = Self::ASSERT_N_FITS;

        let preview = key.preview();
        let needle = key.as_needle();

        let (child_idx, path) = self.walk_to_leaf(needle);
        let leaf = &self.leaves[child_idx];
        let pos = leaf.find_position(needle);

        // Key already exists?
        if pos < leaf.keys.len() && StoredKey::eq_key(leaf.keys.get(pos), needle) {
            return Err((key, value));
        }

        let stored = K::into_stored(key);

        if leaf.keys.len() >= N {
            let mid = N / 2;
            let (parent_idx, new_leaf_idx) = self.split_leaf(child_idx, path);
            if pos <= mid {
                self.leaves[parent_idx].insert(pos, preview, stored, value);
            } else {
                self.leaves[new_leaf_idx].insert(pos - mid, preview, stored, value);
            }
        } else {
            self.leaves[child_idx].insert(pos, preview, stored, value);
        }

        self.len += 1;
        Ok(())
    }

    fn split_leaf(&mut self, child_idx: usize, mut path: Vec<(usize, usize)>) -> (usize, usize) {
        let mid = N / 2;
        let mid_stored = self.leaves[child_idx].keys.get(mid).clone();
        let mid_preview = *self.leaves[child_idx].previews.get(mid);

        let child_idx = if self.n_leaves == self.leaves.len() {
            let map = self.spread();
            map[child_idx]
        } else {
            child_idx
        };

        let old_next = self.leaves[child_idx].get_next();

        let mut new_leaf = LeafNode::<K, P, V, PTR, N>::new();
        self.leaves[child_idx].keys.drain_into(mid, &mut new_leaf.keys);
        self.leaves[child_idx].previews.drain_into(mid, &mut new_leaf.previews);
        self.leaves[child_idx].values.drain_into(mid, &mut new_leaf.values);

        let new_leaf_idx = self.claim_slot(child_idx);

        new_leaf.set_prev(child_idx);
        if let Some(ni) = old_next {
            new_leaf.set_next(ni);
        }
        self.leaves[child_idx].set_next(new_leaf_idx);
        self.leaves[new_leaf_idx] = new_leaf;
        if let Some(next_idx) = old_next {
            self.leaves[next_idx].set_prev(new_leaf_idx);
        }

        self.n_leaves += 1;
        self.insert_separator(mid_stored, mid_preview, new_leaf_idx, &mut path);
        (child_idx, new_leaf_idx)
    }

    fn insert_separator(&mut self, stored: <K as TreeKey>::Stored, preview: P, new_child_idx: usize, path: &mut Vec<(usize, usize)>) {
        if path.is_empty() {
            let old_root_idx = self.root_inode;
            let mut root = KeyNode::<K, P, PTR, N, NP1>::new();
            root.keys.insert_at(0, stored);
            root.previews.insert_at(0, preview);
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
            let needle: &<K as TreeKey>::Needle = stored.borrow();
            let pos = self.inodes[parent_idx].find_position(needle);
            self.inodes[parent_idx].insert_key_at(pos, preview, stored);
            self.inodes[parent_idx].set_ptr(pos + 1, new_child_idx);
        } else {
            self.split_inode(parent_idx, stored, preview, new_child_idx, path);
        }
    }

    fn find_position_in_inode(inode: &KeyNode<K, P, PTR, N, NP1>, needle: &<K as TreeKey>::Needle) -> usize {
        inode.find_position(needle)
    }

    fn split_inode(
        &mut self,
        parent_idx: usize,
        new_stored: <K as TreeKey>::Stored,
        new_preview: P,
        new_child_idx: usize,
        path: &mut Vec<(usize, usize)>,
    ) {
        let mid = N / 2;
        // Save the mid separator before we start moving things.
        let mid_stored = self.inodes[parent_idx].keys.get(mid).clone();
        let mid_preview = *self.inodes[parent_idx].previews.get(mid);

        // Save ptrs length before drain_into truncates keys.
        let old_len = self.inodes[parent_idx].keys.len();

        let mut new_inode = KeyNode::<K, P, PTR, N, NP1>::new();
        // Move keys/previews [mid+1..old_len) to new inode.
        // Only drain if there are keys beyond the separator.
        if mid + 1 < old_len {
            self.inodes[parent_idx].keys.drain_into(mid + 1, &mut new_inode.keys);
            self.inodes[parent_idx].previews.drain_into(mid + 1, &mut new_inode.previews);
        }

        // Move ptrs [mid+1..=old_len] to new inode.
        for i in 0..=old_len - (mid + 1) {
            new_inode.ptrs[i] = self.inodes[parent_idx].ptrs[mid + 1 + i];
        }

        // Remove the mid separator key/preview.
        self.inodes[parent_idx].keys.remove_at(mid);
        self.inodes[parent_idx].previews.remove_at(mid);
        // Truncate ptrs: keep [0..=mid], clear the rest.
        for i in (mid + 1)..=old_len {
            self.inodes[parent_idx].ptrs[i] = None;
        }

        // Insert the new key/child into the appropriate inode.
        let goes_right = new_stored >= mid_stored;
        if goes_right {
            let needle: &<K as TreeKey>::Needle = new_stored.borrow();
            let pos = new_inode.find_position(needle);
            new_inode.insert_key_at(pos, new_preview, new_stored);
            new_inode.set_ptr(pos + 1, new_child_idx);
        } else {
            let needle: &<K as TreeKey>::Needle = new_stored.borrow();
            let pos = self.inodes[parent_idx].find_position(needle);
            self.inodes[parent_idx].insert_key_at(pos, new_preview, new_stored);
            self.inodes[parent_idx].set_ptr(pos + 1, new_child_idx);
        }

        let new_inode_idx = self.inodes.len();
        self.inodes.push(new_inode);

        // Recurse: insert the mid separator into the grandparent.
        self.insert_separator(mid_stored, mid_preview, new_inode_idx, path);
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

    pub fn get_cursor(&self) -> Cursor<'_, K, V, PTR, N, NP1, P> {
        let leaf_idx = self.first_leaf();
        Cursor {
            tree: self,
            leaf_idx,
            position: 0,
        }
    }

    pub fn get_cursor_mut(&mut self) -> CursorMut<'_, K, V, PTR, N, NP1, P> {
        let leaf_idx = self.first_leaf();
        CursorMut {
            tree: self,
            leaf_idx,
            position: 0,
        }
    }

    pub fn cursor_at(&self, key: &<K as TreeKey>::Needle) -> Cursor<'_, K, V, PTR, N, NP1, P> {
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

impl<'a, K, V, PTR, const N: usize, const NP1: usize, P>
    Cursor<'a, K, V, PTR, N, NP1, P>
where
    K: TreeKey + Preview<P> + SearchStrategy<P>,
    K::Stored: StoredKey,
    PTR: TrieIndex,
    V: Sized,
    P: Copy,
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
        let next_leaf = leaf.get_next()?;
        self.leaf_idx = next_leaf;
        self.position = 0;
        Some(self.tree.leaves[self.leaf_idx].values.get(0))
    }

    pub fn prev(&mut self) -> Option<&V> {
        if self.position > 0 {
            self.position -= 1;
            return Some(self.tree.leaves[self.leaf_idx].values.get(self.position));
        }
        let prev_leaf = self.tree.leaves[self.leaf_idx].get_prev()?;
        self.leaf_idx = prev_leaf;
        let last_pos = self.tree.leaves[self.leaf_idx].keys.len() - 1;
        self.position = last_pos;
        Some(self.tree.leaves[self.leaf_idx].values.get(last_pos))
    }
}

impl<'a, K, V, PTR, const N: usize, const NP1: usize, P>
    CursorMut<'a, K, V, PTR, N, NP1, P>
where
    K: TreeKey + Preview<P> + SearchStrategy<P>,
    K::Stored: StoredKey,
    PTR: TrieIndex,
    V: Sized,
    P: Copy,
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
            let next_leaf = leaf.get_next();
            if let Some(nl) = next_leaf {
                self.leaf_idx = nl;
                self.position = 0;
            } else {
                return None;
            }
        }
        Some(self.tree.leaves[self.leaf_idx].values.get_mut(self.position))
    }

    pub fn prev(&mut self) -> Option<&mut V> {
        if self.position > 0 {
            self.position -= 1;
        } else {
            let prev_leaf = self.tree.leaves[self.leaf_idx].get_prev()?;
            self.leaf_idx = prev_leaf;
            self.position = self.tree.leaves[self.leaf_idx].keys.len() - 1;
        }
        Some(self.tree.leaves[self.leaf_idx].values.get_mut(self.position))
    }
}

// ---------------------------------------------------------------------------
// Instantiation aliases
// ---------------------------------------------------------------------------

/// Fixed-size-key B+ tree (SIMD search). `K: FixedLenKey`.
pub type FixedCTree<K, V, PTR, const N: usize, const NP1: usize> =
    CTree<K, V, PTR, N, NP1, NoPreview>;

/// Variable-length-key B+ tree (preview SIMD + scalar fallback).
/// `K: TreeKey + Preview<P>`, e.g. `Vec<u8>` with `P = u64`.
pub type VarCTree<K, V, PTR, const N: usize, const NP1: usize, P> =
    CTree<K, V, PTR, N, NP1, P>;

#[cfg(test)]
#[path = "tests/tiny_btree.rs"]
mod tests;
